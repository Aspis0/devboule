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

import { writeFileSync, rmSync, mkdtempSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

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

// ---------------------------------------------------------------------------
// JSONL framer — manual, NOT readline
// ---------------------------------------------------------------------------

function createJsonlReader(stream, onLine) {
	let buffer = "";
	stream.on("data", (chunk) => {
		buffer += typeof chunk === "string" ? chunk : chunk.toString("utf8");
		while (true) {
			const nl = buffer.indexOf("\n");
			if (nl === -1) break;
			let line = buffer.slice(0, nl);
			buffer = buffer.slice(nl + 1);
			if (line.endsWith("\r")) line = line.slice(0, -1);
			if (line.length > 0) onLine(line);
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
// Model configuration (decision #9: vault → env → sidecar)
// ---------------------------------------------------------------------------

// Track temp dir for cleanup.
let activeTmpDir = null;

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

async function main() {
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

	// ---- Censor hook state -------------------------------------------------
	const censorEnabled =
		(process.env.DEVBOULE_CENSOR_REVIEW_ENABLED ?? "true") !== "false";
	if (!censorEnabled) {
		console.error("[pi-sidecar] Censor review disabled");
	}
	const editedRsFiles = new Map(); // filePath → { patch? }
	const pendingToolPaths = new Map(); // toolCallId → filePath (from tool_execution_start)
	let isReviewTurn = false;

	// ---- subscribe to all events and forward as JSONL ---------------------
	session.subscribe((event) => {
		emit(event);

		// Censor hook: capture file path from tool_execution_start args
		// (tool_execution_end does NOT carry args, only result)
		if (event.type === "tool_execution_start" && censorEnabled) {
			if (event.toolName === "write" || event.toolName === "edit") {
				const fp = event.args?.path;
				if (fp && fp.endsWith(".rs")) {
					pendingToolPaths.set(event.toolCallId, fp);
				}
			}
		}

		// Censor hook: on tool end, correlate by toolCallId and extract patch
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
				const patch = event.result?.details?.patch;
				editedRsFiles.set(fp, { patch });
				console.error("[pi-sidecar] censor: queued", fp);
			}
		}

		// Censor hook: trigger review at agent_end
		if (event.type === "agent_end") {
			if (isReviewTurn) {
				isReviewTurn = false;
				editedRsFiles.clear();
			} else if (editedRsFiles.size > 0 && !stdinClosed && censorEnabled) {
				// Defer so the outer session.prompt() fully resolves first
				setTimeout(() => triggerCensorReview(session), 0);
			}
		}

		if (stdinClosed && event.type === "agent_end" && !isReviewTurn) {
			clearTimeout(stdinGraceTimer);
			cleanup(0);
		}
	});

	// ---- read JSONL commands from stdin -----------------------------------
	let promptInFlight = false;
	let stdinClosed = false;
	let stdinGraceTimer = null;

	createJsonlReader(process.stdin, async (line) => {
		let cmd;
		try {
			cmd = JSON.parse(line);
		} catch {
			emitError("parse", `Invalid JSON: ${line.slice(0, 200)}`);
			return;
		}

		switch (cmd.type) {
			case "prompt": {
				if (promptInFlight) {
					emit({
						type: "response",
						command: "prompt",
						success: false,
						error: "A prompt is already in flight. Wait for agent_end.",
					});
					break;
				}
				promptInFlight = true;
				try {
					await session.prompt(cmd.message, {
						streamingBehavior: cmd.streamingBehavior,
					});
					emit({
						type: "response",
						command: "prompt",
						success: true,
					});
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
				break;
			}

			case "quit": {
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

	emit({ type: "ready" });
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

async function triggerCensorReview(session) {
	const files = [...editedRsFiles.entries()];
	editedRsFiles.clear();

	if (files.length === 0) return;

	isReviewTurn = true;

	// Small delay to let the agent settle.
	if (CENSOR_REVIEW_DELAY_MS > 0) {
		await new Promise((r) => setTimeout(r, CENSOR_REVIEW_DELAY_MS));
	}

	const reviewPrompt = composeCensorReviewPrompt(files);

	try {
		console.error(
			"[pi-sidecar] censor: sending review prompt for",
			files.length,
			"file(s)",
		);
		await session.prompt(reviewPrompt);
	} catch (err) {
		console.error("[pi-sidecar] censor review failed:", err);
	} finally {
		isReviewTurn = false;
	}
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

process.on("SIGTERM", () => cleanup(0));

process.on("unhandledRejection", (reason) => {
	emitError("fatal", reason);
	cleanup(1);
});

main().catch((err) => {
	emitError("fatal", err);
	cleanup(1);
});
