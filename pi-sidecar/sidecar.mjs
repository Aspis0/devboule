#!/usr/bin/env node
/**
 * pi-sidecar — Phase 0 spike: embeds pi SDK in-process and bridges to Devboule's Rust
 * backend via JSONL over stdio.
 *
 * Protocol:
 *   stdin:  JSONL commands — {"type":"prompt","message":"..."}
 *   stdout: JSONL events  — raw pi SDK events + {"type":"response","command":"prompt","success":true}
 *
 * Env vars:
 *   DEVBOULE_PI_PROVIDER  — e.g. "openai" (default; Claude blocked per decision #10)
 *   DEVBOULE_PI_MODEL     — e.g. "gpt-4o" (default)
 *   OPENAI_API_KEY        — (or ANTHROPIC_API_KEY etc.) needed for the provider
 */

// ---------------------------------------------------------------------------
// JSONL helpers
// ---------------------------------------------------------------------------

function emit(obj) {
	// Strict JSONL: one JSON object per line, LF only (matches pi rpc.md framing).
	process.stdout.write(JSON.stringify(obj) + "\n");
}

function emitError(context, err) {
	emit({
		type: "error",
		context,
		message: err instanceof Error ? err.message : String(err),
	});
}

// ---------------------------------------------------------------------------
// JSONL framer — manual, NOT readline (finding #2: readline splits on
// U+2028/U+2029 which are valid inside JSON strings, corrupting the protocol).
// Reference: /opt/homebrew/lib/node_modules/@earendil-works/pi-coding-agent/docs/rpc.md §Framing
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
			// Strip optional \r (Windows compat) — the ONLY \r handling.
			if (line.endsWith("\r")) line = line.slice(0, -1);
			if (line.length > 0) onLine(line);
		}
	});
	stream.on("end", () => {
		// Flush trailing buffer (no trailing newline).
		if (buffer.length > 0) {
			let line = buffer;
			if (line.endsWith("\r")) line = line.slice(0, -1);
			if (line.length > 0) onLine(line);
		}
	});
}

// ---------------------------------------------------------------------------
// pi SDK bootstrap
// ---------------------------------------------------------------------------

let activeSession = null; // for SIGTERM cleanup

async function main() {
	// Dynamic import — the package is a local dependency of this sidecar.
	const {
		createAgentSession,
		SessionManager,
		AuthStorage,
		ModelRegistry,
		defineTool,
	} = await import("@earendil-works/pi-coding-agent");

	// Typebox for custom tool parameter schemas.
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
			// TODO: proxy to real Oracle MCP (Python RAG server).
			// For the spike, return a canned response clearly marked as placeholder.
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

	// ---- create session ---------------------------------------------------
	const authStorage = AuthStorage.create();
	const modelRegistry = ModelRegistry.create(authStorage);

	// Decision #10: Claude blocked for external MCP (2026-07). Default to OpenAI.
	const provider = process.env.DEVBOULE_PI_PROVIDER || "openai";
	const modelId = process.env.DEVBOULE_PI_MODEL || "gpt-4o";

	// Attempt to resolve the configured model. If unavailable (missing API key,
	// unknown provider), createAgentSession falls back to the first available.
	let resolvedModel;
	try {
		resolvedModel = modelRegistry.find(provider, modelId);
	} catch {
		// Ignore — the session will use its default.
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
		// Forward the raw pi SDK event verbatim — the Rust side maps it to the
		// existing MiniActivityEvent / ConsoleActivity schema.
		emit(event);
		// If stdin already closed and the in-flight prompt just finished,
		// tear down the sidecar gracefully.
		if (stdinClosed && event.type === "agent_end") {
			clearTimeout(stdinGraceTimer);
			cleanup(0);
		}
	});

	// ---- read JSONL commands from stdin (manual framer, NOT readline) -----
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
			// An agent turn is still running — let it finish.
			// Safety net: force-cleanup after 120 s so a hung agent
			// can't keep the process alive forever.
			stdinGraceTimer = setTimeout(() => cleanup(0), 120_000);
		} else {
			cleanup(0);
		}
	});

	// Signal readiness.
	emit({ type: "ready" });
}

// ---------------------------------------------------------------------------
// Process lifecycle (finding #6)
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
