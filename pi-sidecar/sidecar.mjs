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
import { tmpdir } from "node:path";

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
	const {
		createAgentSession,
		SessionManager,
		AuthStorage,
		ModelRegistry,
		defineTool,
	} = await import("@earendil-works/pi-coding-agent");

	const { Type } = await import("typebox");

	// ---- custom tool: oracle_ask (canned/echo) ----------------------------
	const oracleAskTool = defineTool({
		name: "oracle_ask",
		label: "Oracle Ask",
		description:
			"Query the Devboule Oracle RAG system for codebase context. " +
			"Pass a natural-language question about the project.",
		parameters: Type.Object({
			question: Type.String({ description: "The question to ask the Oracle." }),
		}),
		execute: async (_toolCallId, params) => {
			const answer =
				`[SPIKE PLACEHOLDER — Oracle not wired yet]\n` +
				`Question received: "${params.question}"\n` +
				`In production this would proxy to the Python Oracle MCP server ` +
				`and return real codebase context.`;
			return {
				content: [{ type: "text", text: answer }],
				details: {},
			};
		},
	});

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
		customTools: [oracleAskTool],
	};
	if (resolvedModel) {
		sessionOpts.model = resolvedModel;
	}

	const { session } = await createAgentSession(sessionOpts);
	activeSession = session;

	// ---- subscribe to all events and forward as JSONL ---------------------
	session.subscribe((event) => {
		emit(event);
		if (stdinClosed && event.type === "agent_end") {
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
