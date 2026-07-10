#!/usr/bin/env node
/**
 * pi-sidecar — Phase 1: per-session lifecycle + vault-based model config.
 *
 * Protocol:
 *   stdin:  JSONL commands — {"type":"prompt","message":"..."}
 *   stdout: JSONL events  — raw pi SDK events + {"type":"response","command":"prompt","success":true}
 *
 * Env vars (set by the Rust backend at spawn time, decision #9):
 *   DEVBOULE_PI_PROVIDER  — pi provider id (e.g. "openrouter", "openai")
 *   DEVBOULE_PI_MODEL     — model id (e.g. "tencent/hy3:free", "qwen2.5-coder:7b")
 *   DEVBOULE_PI_BASE_URL  — custom base URL for local endpoints (optional; if set,
 *                            a temp models.json is written with a custom provider)
 *   OPENAI_API_KEY        — for openai provider (set by Rust for local omlx/ollama)
 *   OPENROUTER_API_KEY    — for openrouter provider (set by Rust for cloud backend)
 *   ANTHROPIC_API_KEY     — for anthropic provider (NOT used; Claude blocked per #10)
 */

import {
	writeFileSync,
	rmSync,
	mkdtempSync,
	existsSync,
	readFileSync,
} from "node:fs";
import { join } from "node:path";
import { tmpdir, homedir } from "node:os";
import { pathToFileURL, fileURLToPath } from "node:url";
import { realpathSync } from "node:fs";

// #5: bound the JSONL framer buffer. A single oversized line (e.g. a 500MB
// JSONL record) would otherwise accumulate unbounded and OOM the process.
const MAX_BUFFER_LEN = 10 * 1024 * 1024; // 10MB

// ---------------------------------------------------------------------------
// Devboule enrichment metadata (Task 1: enrichment layer)
// ---------------------------------------------------------------------------

// Module-level context attached to every forwarded event via the `_devboule`
// field. Enables PlannerPlanMode (orchestrator) and FocusStagePane (coder/mini)
// to render pi agent output without React changes.
//
// #9: declared with `let` and assigned inside `main()` so the context is NOT
// captured at module load time (which would leak stale env if the process were
// ever reused). It is reset on every `main()` entry.
let devbouleContext;

// ---------------------------------------------------------------------------
// JSONL helpers
// ---------------------------------------------------------------------------

function emit(obj) {
	try {
		process.stdout.write(JSON.stringify(obj) + "\n");
	} catch (e) {
		// Log non-EPIPE errors to stderr; swallow EPIPE (pipe closed).
		if (e.code !== "EPIPE") {
			console.error("[pi-sidecar] emit failed:", e);
		}
	}
}

function emitError(context, err) {
	emit({
		type: "error",
		context,
		message: err instanceof Error ? err.message : String(err),
	});
}

// #2: check whether the Oracle MCP server (`oracle-figlyph`) is configured in
// either `~/.pi/agent/mcp.json` or `~/.pi/mcp.json`. If not, `oracle_ask`
// (which the Rust side surfaces via the Oracle MCP) will not work. We only
// check config presence here — actual reachability is the MCP client's concern.
function isOracleMcpConfigured() {
	try {
		const candidates = [
			join(homedir(), ".pi", "agent", "mcp.json"),
			join(homedir(), ".pi", "mcp.json"),
		];
		for (const p of candidates) {
			if (!existsSync(p)) continue;
			const cfg = JSON.parse(readFileSync(p, "utf8"));
			if (cfg?.mcpServers?.["oracle-figlyph"]) return true;
			if (Object.hasOwn(cfg, "oracle-figlyph")) return true;
		}
	} catch (e) {
		console.error("[pi-sidecar] Oracle MCP config check failed:", e);
	}
	return false;
}

// ---------------------------------------------------------------------------
// JSONL framer — manual, NOT readline
// ---------------------------------------------------------------------------

function createJsonlReader(stream, onLine) {
	let buffer = "";
	stream.on("data", async (chunk) => {
		buffer += typeof chunk === "string" ? chunk : chunk.toString("utf8");
		// #5: cap the buffer BEFORE scanning for newlines. If it grows past
		// MAX_BUFFER_LEN, truncate at the last valid line boundary and warn
		// (emitError + stderr). The sidecar continues with a clean tail.
		if (buffer.length > MAX_BUFFER_LEN) {
			const lastNl = buffer.lastIndexOf("\n");
			if (lastNl !== -1) {
				const dropped = buffer.length - lastNl - 1;
				buffer = buffer.slice(lastNl + 1);
				emitError(
					"jsonl",
					`Buffer exceeded ${MAX_BUFFER_LEN} bytes; dropped ${dropped} bytes before last newline`,
				);
				console.error(
					`[pi-sidecar] JSONL buffer overflow: dropped ${dropped} bytes`,
				);
			} else {
				emitError(
					"jsonl",
					`Buffer exceeded ${MAX_BUFFER_LEN} bytes with no newline; clearing`,
				);
				console.error(
					"[pi-sidecar] JSONL buffer overflow: no newline found, clearing buffer",
				);
				buffer = "";
			}
		}
		while (true) {
			const nl = buffer.indexOf("\n");
			if (nl === -1) break;
			let line = buffer.slice(0, nl);
			buffer = buffer.slice(nl + 1);
			if (line.endsWith("\r")) line = line.slice(0, -1);
			if (line.length > 0) await onLine(line);
		}
	});
	stream.on("end", () => {
		if (buffer.length > 0) {
			let line = buffer;
			if (line.endsWith("\r")) line = line.slice(0, -1);
			if (line.length > 0) onLine(line);
		}
	});
}

// ---------------------------------------------------------------------------
// Phase 2 Pigeon routing hooks
// ---------------------------------------------------------------------------

// Module-level handle for the in-flight classification promise. Declared here
// (not inside main()) so requestClassification() can assign/clear it while the
// stdin `classified` handler in main() reads the same binding.
let pendingClassification = null;

/**
 * Emit a `classify_prompt` request to the Rust sidecar and await its `classified`
 * response (delivered on our stdin). Resolves with { tier, provider, model }.
 */
function requestClassification(text) {
	return new Promise((resolve, reject) => {
		// #7: never hang forever if the Rust side never delivers `classified`
		// (e.g. a write failure in write_jsonl_to_stdin). After 5s, proceed with
		// a default classification — accept the prompt without Pigeon routing.
		const defaultClassification = {
			tier: "default",
			provider: null,
			model: null,
		};
		const timeout = setTimeout(() => {
			console.error(
				"[pi-sidecar] requestClassification timed out (5s) — using default classification",
			);
			pendingClassification = null;
			resolve(defaultClassification);
		}, 5000);
		pendingClassification = { resolve, reject, timeout };
		emit({ type: "classify_prompt", text });
	});
}

/**
 * Apply a Pigeon classification to the live session. The pi SDK supports
 * `session.setModel(model)` mid-session (docs/sdk.md:91), so we switch the model
 * when the resolved provider/model is present in the current ModelRegistry.
 * If it is NOT (e.g. the spawn-time minimal models.json only carries the
 * configured provider), we defer and keep the existing session model — full
 * multi-tier switching is Phase 3.
 */
async function applyPigeonRouting(session, modelRegistry, classification) {
	const { tier, provider, model } = classification;
	console.error(
		`[pi-sidecar] Pigeon: tier=${tier} provider=${provider} model=${model}`,
	);
	// Defensive null-guard: keep the session model if the classification target is
	// null/undefined (e.g. the 5s timeout-fallback default where provider/model are
	// null, or a classification that resolved no routing target). Harmless in the
	// normal Rust-answered path, and protects against deref/setModel crashes.
	if (!provider || !model) {
		console.error(
			"[pi-sidecar] Pigeon: no routing target, keeping session model",
		);
		return;
	}
	try {
		const resolved = modelRegistry.find(provider, model);
		if (!resolved) {
			console.error(
				`[pi-sidecar] Pigeon: ${provider}/${model} not in registry, keeping session model`,
			);
			return;
		}
		await session.setModel(resolved);
		console.error(
			`[pi-sidecar] Pigeon: applied ${provider}/${model} (tier=${tier})`,
		);
	} catch (e) {
		console.error(
			`[pi-sidecar] Pigeon: setModel deferred — ${e instanceof Error ? e.message : String(e)} (keeping session model)`,
		);
	}
}

/**
 * Phase 2 Pigeon: classify the prompt via Rust (await the `classified` response),
 * then apply the tier→model routing and run the turn in pi.
 */
export async function handlePromptCommand(cmd, session, modelRegistry, pigeonEnabled = false) {
	if (!pigeonEnabled) {
		// Pigeon OFF (default): no classification, no model switch, no redirect —
		// run the turn on the spawn-time configured model.
		await session.prompt(cmd.message, {
			streamingBehavior: cmd.streamingBehavior,
		});
		emit({ type: "response", command: "prompt", success: true });
		return;
	}

	const classification = await requestClassification(cmd.message);
	// Pigeon ON: apply tier→model routing, then run the turn.
	await applyPigeonRouting(session, modelRegistry, classification);
	await session.prompt(cmd.message, {
		streamingBehavior: cmd.streamingBehavior,
	});
	emit({
		type: "response",
		command: "prompt",
		success: true,
	});
}

// ---------------------------------------------------------------------------
// Model configuration (decision #9: vault → env → sidecar)
// ---------------------------------------------------------------------------

// Track temp dir for cleanup.
let activeTmpDir = null;

// Fix 1: FIFO queue for prompts that arrive while a turn is in flight. A prompt
// sent while `promptInFlight` is true is queued (up to MAX_QUEUED_PROMPTS) and
// run when the current turn ends, so a Censor-findings prompt (or any prompt)
// sent minutes after a review started is never silently lost — Rust never reads
// the rejection response, so dropping it would report "found N issues" as if
// delivered.
const MAX_QUEUED_PROMPTS = 5;
let promptQueue = [];

// Censor hook config
// Delay: give the Rust side time to flush the final MiniActivityEvent snapshot
// before the review turn begins.
const CENSOR_REVIEW_DELAY_MS = 500;

/**
 * Build a MINIMAL temp models.json with ONLY the Devboule-configured provider.
 * Decision #9: do NOT read ~/.pi/agent/models.json — avoids leaking user's
 * cloud API keys (OpenRouter/Anthropic/OpenAI) to /tmp on crash.
 *
 * Returns the path to the temp models.json, or null if no custom base URL is set.
 */
function buildCustomModelsJson() {
	const baseUrl = process.env.DEVBOULE_PI_BASE_URL;
	if (!baseUrl) return null;

	const provider = process.env.DEVBOULE_PI_PROVIDER || "openai";
	const model = process.env.DEVBOULE_PI_MODEL || "gpt-4o";

	// Minimal config: only the Devboule provider. No user keys leaked.
	const minimalModels = {
		providers: {
			[provider]: {
				baseUrl,
				api: "openai-completions",
				apiKey: process.env.OPENAI_API_KEY || "dummy",
				compat: {
					supportsDeveloperRole: false,
					supportsReasoningEffort: false,
				},
				models: [{ id: model }],
			},
		},
	};

	const tmpDir = mkdtempSync(join(tmpdir(), "pi-sidecar-"));
	const modelsJsonPath = join(tmpDir, "models.json");
	writeFileSync(modelsJsonPath, JSON.stringify(minimalModels, null, 2));

	activeTmpDir = tmpDir;
	return modelsJsonPath;
}

// ---------------------------------------------------------------------------
// pi SDK bootstrap
// ---------------------------------------------------------------------------

let activeSession = null;
let pigeonEnabled = false;

async function main() {
	// #9: (re)build devbouleContext from the current process env at startup so
	// it never leaks stale values across runs.
	devbouleContext = {
		agentRole: process.env.DEVBOULE_AGENT_ROLE || "main-coder",
		projectId: process.env.DEVBOULE_PROJECT_ID || null,
		sessionId: process.env.DEVBOULE_SESSION_ID || null,
	};
	pigeonEnabled = process.env.DEVBOULE_PIGEON_ENABLED === "true";

	const { createAgentSession, SessionManager, AuthStorage, ModelRegistry } =
		await import("@earendil-works/pi-coding-agent");

	// NOTE: oracle_ask tool REMOVED — Oracle is exposed via MCP auto-connect
	// (~/.pi/agent/mcp.json + .pi/mcp.json). The pi agent picks up the
	// Oracle-figlyph MCP server (7 tools) automatically.

	// ---- model configuration (decision #9) --------------------------------
	const provider = process.env.DEVBOULE_PI_PROVIDER || "openai";
	const modelId = process.env.DEVBOULE_PI_MODEL || "gpt-4o";
	const customModelsJsonPath = buildCustomModelsJson();

	const authStorage = AuthStorage.create();

	// If a custom base URL is set, use the MERGED temp models.json.
	// Otherwise, use the default models.json (user's ~/.pi/agent/models.json).
	const modelRegistry = customModelsJsonPath
		? ModelRegistry.create(authStorage, customModelsJsonPath)
		: ModelRegistry.create(authStorage);

	let resolvedModel;
	try {
		resolvedModel = modelRegistry.find(provider, modelId);
	} catch (err) {
		console.error(
			`[pi-sidecar] Could not resolve model ${provider}/${modelId}: ${err instanceof Error ? err.message : String(err)}`,
		);
	}

	const sessionOpts = {
		sessionManager: SessionManager.inMemory(),
		authStorage,
		modelRegistry,
	};
	if (resolvedModel) {
		sessionOpts.model = resolvedModel;
	}

	const { session } = await createAgentSession(sessionOpts);
	activeSession = session;

	// ---- MCP server logging -----------------------------------------------
	const mcpServers = session.agent.state.mcpServers;
	if (mcpServers) {
		console.error(
			"[pi-sidecar] MCP servers configured:",
			JSON.stringify(mcpServers.map((s) => s.name)),
		);
	} else {
		console.error(
			"[pi-sidecar] MCP servers: not exposed by SDK (gap noted). Oracle tools available via MCP auto-connect.",
		);
	}

	// #2: report Oracle MCP availability so the Rust side can warn the user
	// (via a console banner) that `oracle_ask` will not work without it.
	const oracleMcpAvailable = isOracleMcpConfigured();
	if (!oracleMcpAvailable) {
		console.error(
			"[pi-sidecar] Oracle MCP (oracle-figlyph) NOT configured — oracle_ask will not work.",
		);
	} else {
		console.error("[pi-sidecar] Oracle MCP (oracle-figlyph) configured.");
	}

	// ---- Censor hook state -------------------------------------------------
	const censorEnabled =
		(process.env.DEVBOULE_CENSOR_REVIEW_ENABLED ?? "true") !== "false";
	if (!censorEnabled) {
		console.error("[pi-sidecar] Censor review disabled");
	}
	const editedRsFiles = new Map(); // filePath → { patch? }
	const pendingToolPaths = new Map(); // toolCallId → filePath (from tool_execution_start)
	let isReviewTurn = false;

	// ---- subscribe helpers (close over main() locals) ----------------------

	// Devboule custom messages: web_search + plan tool results are echoed
	// back as user-role messages so the Rust EventMapper can inject them
	// into ConsoleActivity for PlannerPlanMode.
	//
	// IMPORTANT: queue these BEFORE forwarding the event to Rust (emit
	// below). sendMessage must precede emit so the custom message is queued
	// into the SAME turn's event stream, not delayed to the next turn.
	async function echoDevbouleCustomMessages(event) {
		if (
			event.type === "tool_execution_end" &&
			event.toolName === "web_search"
		) {
			try {
				await session.sendMessage({
					role: "user",
					content: [
						{
							type: "text",
							text: JSON.stringify({
								type: "devboule.websearch",
								query: event.args?.query || "",
								results: event.result?.details || {},
								timestamp: Date.now(),
							}),
						},
					],
				});
			} catch {
				/* best-effort, don't break the stream */
			}
		}

		if (event.type === "tool_execution_end" && event.toolName === "plan") {
			try {
				await session.sendMessage({
					role: "user",
					content: [
						{
							type: "text",
							text: JSON.stringify({
								type: "devboule.plan",
								plan: event.result?.details || {},
								timestamp: Date.now(),
							}),
						},
					],
				});
			} catch {
				/* best-effort */
			}
		}
	}

	// Censor hook: capture file path from tool_execution_start args
	// (tool_execution_end does NOT carry args, only result)
	function captureCensorToolPath(event) {
		if (event.type === "tool_execution_start" && censorEnabled) {
			if (event.toolName === "write" || event.toolName === "edit") {
				const fp = event.args?.path;
				if (fp && fp.endsWith(".rs")) {
					pendingToolPaths.set(event.toolCallId, fp);
				}
			}
		}
	}

	// Censor hook: on tool end, correlate by toolCallId and extract patch
	function correlateCensorPatch(event) {
		if (
			event.type === "tool_execution_end" &&
			!event.isError &&
			censorEnabled
		) {
			const fp = pendingToolPaths.get(event.toolCallId);
			if (fp) {
				pendingToolPaths.delete(event.toolCallId);
				// For edit: event.result is AgentToolResult<EditToolDetails>
				//   → result.details.patch (unified diff string)
				// For write: event.result is AgentToolResult<undefined>
				//   → no patch available
				let patch = event.result?.details?.patch;
				if (patch && patch.length > 10_000) {
					patch = patch.slice(0, 10_000) + "\n... [truncated to 10KB]";
				}
				editedRsFiles.set(fp, { patch });
				console.error("[pi-sidecar] censor: queued", fp);
			}
		}
	}

	// Censor hook: trigger review at agent_end
	function handleCensorAgentEnd(event) {
		if (event.type === "agent_end") {
			if (isReviewTurn) {
				isReviewTurn = false;
				editedRsFiles.clear();
			} else if (editedRsFiles.size > 0 && !stdinClosed && censorEnabled) {
				// Defer so the outer session.prompt() fully resolves first
				setTimeout(() => triggerCensorReview(), 0);
			}
		}
	}

	function handleStdinCloseAtAgentEnd(event) {
		if (stdinClosed && event.type === "agent_end" && !isReviewTurn) {
			clearTimeout(stdinGraceTimer);
			// Fix 1: if prompts are queued (arrived while a turn was in flight),
			// the prompt handler's finally will shift + run the next one. Don't exit
			// here or we'd kill a queued prompt mid-flight. Only exit once the queue
			// has fully drained. Behavior is identical when nothing is queued.
			if (promptQueue.length === 0) {
				setImmediate(() => cleanup(0));
			}
		}
	}

	// ---- subscribe to all events and forward as JSONL ---------------------
	session.subscribe(async (event) => {
		const enriched = {
			...event,
			_devboule: {
				agentRole: devbouleContext.agentRole,
				projectId: devbouleContext.projectId,
				sessionId: devbouleContext.sessionId || session?.id,
			},
		};
		await echoDevbouleCustomMessages(event);
		emit(enriched);
		captureCensorToolPath(event);
		correlateCensorPatch(event);
		handleCensorAgentEnd(event);
		handleStdinCloseAtAgentEnd(event);
	});

	// ---- read JSONL commands from stdin -----------------------------------
	let promptInFlight = false;
	let stdinClosed = false;
	let stdinGraceTimer = null;

	// Fix 1: drain the FIFO queue of prompts that arrived while a turn was in
	// flight. Called after each turn ends (promptInFlight is set back to false in
	// the prompt handler's finally). Re-runs each queued command through the SAME
	// handlePromptCommand path (classification, routing, prompt). All queued
	// prompts run sequentially; the while loop plus the re-entrant call from the
	// next turn's finally cover every case.
	async function drainPromptQueue() {
		while (promptQueue.length > 0) {
			const nextCmd = promptQueue.shift();
			promptInFlight = true;
			try {
				await handlePromptCommand(nextCmd, session, modelRegistry, pigeonEnabled);
			} catch (err) {
				emit({
					type: "response",
					command: "prompt",
					success: false,
					error: err instanceof Error ? err.message : String(err),
				});
			} finally {
				promptInFlight = false;
			}
		}
	}

	createJsonlReader(process.stdin, async (line) => {
		let cmd;
		try {
			cmd = JSON.parse(line);
		} catch {
			emitError("parse", `Invalid JSON: ${line.slice(0, 200)}`);
			return;
		}

		// #4: JSON.parse("null") yields `null` (no throw). `cmd.type` would then
		// throw TypeError and crash the sidecar. Guard non-object values.
		if (cmd === null || typeof cmd !== "object") {
			emitError("parse", "Invalid JSON value: expected object");
			return;
		}

		switch (cmd.type) {
			case "prompt": {
				if (promptInFlight) {
					// Fix 1: queue the command instead of dropping it. A prompt sent
					// while a turn is in flight would otherwise be silently lost
					// (Rust's send_prompt_to_session only checks the stdin WRITE and
					// never reads the rejection response). Keep the rejection only
					// when the queue is ALSO full.
					const { accepted, queue: nextQueue } = enqueuePrompt(
						promptQueue,
						cmd,
						MAX_QUEUED_PROMPTS,
					);
					promptQueue = nextQueue;
					if (!accepted) {
						emit({
							type: "response",
							command: "prompt",
							success: false,
							error: "prompt queue full",
						});
					} else {
						emit({
							type: "response",
							command: "prompt",
							success: true,
							queued: true,
						});
					}
					break;
				}
				// #3: guard against an oversized prompt (e.g. a 10MB paste) that
				// would blow the JSONL buffer / sidecar memory. Truncate-and-error
				// instead of forwarding to pi.
				if (typeof cmd.message === "string" && cmd.message.length > 100_000) {
					emit({
						type: "response",
						command: "prompt",
						success: false,
						error: "Prompt exceeds 100KB limit",
					});
					break;
				}
				promptInFlight = true;
				try {
					// Phase 2 Pigeon: classify BEFORE prompting (handlePromptCommand
					// awaits the Rust `classified` response, then applies routing).
					await handlePromptCommand(cmd, session, modelRegistry, pigeonEnabled);
				} catch (err) {
					emit({
						type: "response",
						command: "prompt",
						success: false,
						error: err instanceof Error ? err.message : String(err),
					});
				} finally {
					promptInFlight = false;
					// Fix 1: process any prompts that queued while this turn ran.
					await drainPromptQueue();
				}
				break;
			}

			case "classified": {
				// Phase 2 Pigeon: Rust classified the prompt; resolve the pending
				// promise so the `prompt` handler can apply the routing.
				if (pendingClassification) {
					clearTimeout(pendingClassification.timeout);
					pendingClassification.resolve(cmd);
					pendingClassification = null;
				}
				break;
			}

			case "quit": {
				// Fix 1: don't lose queued prompts silently on quit.
				if (promptQueue.length > 0) {
					emit({ type: "queue_dropped", count: promptQueue.length });
				}
				cleanup(0);
				break;
			}

			default:
				emitError("unknown_command", `Unknown command type: ${cmd.type}`);
		}
	});

	process.stdin.on("end", () => {
		stdinClosed = true;
		if (promptInFlight) {
			stdinGraceTimer = setTimeout(() => cleanup(0), 120_000);
		} else {
			cleanup(0);
		}
	});

	emit({ type: "ready", oracleMCP: oracleMcpAvailable });

	console.error(
		`[pi-sidecar] enrichment active: role=${devbouleContext.agentRole} session=${devbouleContext.sessionId} project=${devbouleContext.projectId}`,
	);
}

// ---------------------------------------------------------------------------
// Censor hook — post-edit Rust code review via pi LLM
// Gated by DEVBOULE_CENSOR_REVIEW_ENABLED env var (default: "true").
// ---------------------------------------------------------------------------

/**
 * Compose a censor review prompt for the given edited files.
 * Exported for testing.
 *
 * TODO: replace with Rust GemmaClient via Tauri command
 *       (vault config at vault.rs:367, pipeline at censor/gemma.rs)
 */
export function composeCensorReviewPrompt(files) {
	const fileLines = [];
	const diffLines = [];

	for (const [fp, info] of files) {
		const label = info.patch ? "diff available" : "full write";
		fileLines.push(`- ${fp} (${label})`);
		if (info.patch) {
			diffLines.push(`--- ${fp}\n${info.patch}`);
		}
	}

	return [
		"## Censor Review — Post-Edit Rust Code Review",
		"",
		"Review the following Rust code changes for bugs, logic errors, and safety issues.",
		"Report findings as: [severity] file:line — description",
		"Severity: HIGH (must fix), MEDIUM (should fix), LOW (consider fixing)",
		"If clean, reply: CLEAN",
		"",
		"### Files edited:",
		...fileLines,
		"",
		...(diffLines.length > 0 ? ["### Diffs:", ...diffLines, ""] : []),
	].join("\n");
}

/**
 * Pure helper: attempt to enqueue a prompt command onto the queue (immutable
 * update). Returns `{ accepted, queue }`. Rejected (`accepted: false`) when the
 * queue is already at `max`, in which case `queue` is returned unchanged.
 * Exported for testing the queue push/shift logic without running the sidecar.
 */
export function enqueuePrompt(queue, cmd, max) {
	if (queue.length >= max) {
		return { accepted: false, queue };
	}
	return { accepted: true, queue: [...queue, cmd] };
}

async function triggerCensorReview() {
	const files = [...editedRsFiles.entries()];
	editedRsFiles.clear();

	if (files.length === 0) return;

	isReviewTurn = true;

	// Small delay to let the agent settle.
	if (CENSOR_REVIEW_DELAY_MS > 0) {
		await new Promise((r) => setTimeout(r, CENSOR_REVIEW_DELAY_MS));
	}

	const reviewPrompt = composeCensorReviewPrompt(files);
	const filePaths = files.map(([fp]) => fp);
	const diffs = files
		.filter(([, info]) => info.patch)
		.map(([fp, info]) => `--- ${fp}\n${info.patch}`);

	// #8: do NOT call `session.prompt()` from inside the subscribe callback —
	// the pi SDK may not support reentrant prompts and it deadlocks/panics.
	// Instead, surface the review request as an OUTGOING JSONL line that Rust
	// renders in the console. Actual review execution is deferred to Phase 5.
	// TODO(Phase 5): drive the Censor review from Rust (it owns the prompt).
	console.error(
		"[pi-sidecar] censor: emitting review trigger for",
		files.length,
		"file(s)",
	);
	emit({
		type: "devboule_censor_review",
		prompt: reviewPrompt,
		files: filePaths,
		diffs,
	});
	isReviewTurn = false;
}

// ---------------------------------------------------------------------------
// Process lifecycle
// ---------------------------------------------------------------------------

function cleanup(exitCode) {
	if (activeSession) {
		try {
			activeSession.dispose();
		} catch {
			// best-effort
		}
		activeSession = null;
	}
	// Clean up temp models.json dir (finding #2).
	if (activeTmpDir) {
		try {
			rmSync(activeTmpDir, { recursive: true, force: true });
		} catch {
			// best-effort
		}
		activeTmpDir = null;
	}
	process.exit(exitCode);
}

// FIX 3: only auto-start `main()` when this module is the entry point. When
// it is imported (e.g. by pigeon-flag.test.mjs) we must NOT spin up a full
// sidecar session (AgentSession / stdin listeners). Compare the module's real
// path to the executed script's real path: `realpathSync` resolves relative /
// symlink paths (e.g. macOS /tmp -> /private/tmp) so the guard is robust to how
// the file is invoked, unlike a naive `pathToFileURL(process.argv[1])` compare.
if (process.argv[1] && fileURLToPath(import.meta.url) === realpathSync(process.argv[1])) {
	process.on("SIGTERM", () => cleanup(0));

	process.on("unhandledRejection", (reason) => {
		emitError("fatal", reason);
		cleanup(1);
	});

	main().catch((err) => {
		emitError("fatal", err);
		cleanup(1);
	});
}
