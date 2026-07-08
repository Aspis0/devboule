# pi-sidecar — Devboule's agent harness

The Node.js sidecar that embeds the pi SDK (`@earendil-works/pi-coding-agent`) and
bridges it to Devboule's Rust backend via JSONL over stdio. This is the DEFAULT agent
path for orchestrator and coder sessions (Route 2 SDK embed, see
`docs/devboule-on-pi-architecture.md` — §11 lists the binding design decisions).

## What it does

1. **Event streaming**: ALL pi SDK events (`text_delta`, `thinking_*`,
   `tool_execution_*`, `compaction_*`, `auto_retry_*`, …) are forwarded verbatim as
   JSONL to stdout, enriched with a `_devboule: {agentRole, projectId, sessionId}`
   stamp. The Rust `EventMapper` (`src-tauri/src/backend/pi_sidecar.rs`) maps them to
   the existing `MiniActivityEvent`/`ConsoleActivity` schema — the React console
   renders pi sessions with zero frontend forks: chat bubbles, collapsed thinking
   rows, live tool-progress updates, and lifecycle banners.
2. **Prompt delivery with a FIFO queue**: prompts arriving while a turn is in flight
   are queued (up to 5) and drained at turn end — never silently rejected. Queued
   prompts still pending at `quit` are reported via a `queue_dropped` event.
3. **Extensions**: the sidecar loads pi extensions via the standard resource loader
   (`~/.pi/agent` global + `.pi/` project scope) — subagents
   (`@tintinweb/pi-subagents`), pi-lens, compactor, etc.
4. **Oracle MCP**: Oracle tools auto-connect via `~/.pi/agent/mcp.json` /
   `.pi/mcp.json`. The `ready` event carries `oracleMCP: false` when unavailable
   (surfaced as a console banner).
5. **Censor hook**: after any turn that edited `.rs` files, the sidecar emits a
   `devboule_censor_review` event (gated by `DEVBOULE_CENSOR_REVIEW_ENABLED`,
   default on). The Rust side runs the REAL Censor review (deterministic runners +
   voted local-LLM tier) on a detached thread and prompts confirmed findings back
   into the session (max 2 consecutive censor rounds, delivered findings deduped).

## Prerequisites

+ **Node.js** ≥ 20 (tested with v26.3.0)
+ A provider configured for the pi SDK: either an env API key the SDK recognizes
  (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, …) or a custom provider in
  `~/.pi/agent/models.json` (e.g. an OpenRouter-backed provider or a local oMLX
  endpoint). In the app path, provider/model/keys come from the vault (decision #9)
  and are passed via env.

## Setup (one-time)

```bash
cd pi-sidecar
npm install
```

## Running standalone (no Tauri)

```bash
cd pi-sidecar
printf '{"type":"prompt","message":"What files are in the current directory?"}\n' \
  | DEVBOULE_PI_PROVIDER=openrouter-curated DEVBOULE_PI_MODEL="tencent/hy3:free" node sidecar.mjs
```

JSONL events stream to stdout. Two back-to-back prompt lines exercise the queue
(the second gets `{"type":"response","command":"prompt","success":true,"queued":true}`).

## Running inside the app

The pi sidecar is ON by default. Launch an orchestrator/coder from the app (Projects →
Spawn); the Rust backend spawns one sidecar process per session, delivers the prompt,
and streams the console. Set `DEVBOULE_PI_ENABLED=false` to fall back to the legacy
launch paths (the legacy `devboule-coder` binary is archived — the fallback fails
closed by design).

## Configuration (env)

| Env var                          | Default  | Description                                        |
|----------------------------------|----------|----------------------------------------------------|
| `DEVBOULE_PI_ENABLED`            | `true`   | Opt-out kill switch for the whole pi path (Rust)   |
| `DEVBOULE_PI_PROVIDER`           | `openai` | pi provider id (app passes the vault's choice)     |
| `DEVBOULE_PI_MODEL`              | `gpt-4o` | Model id (app passes the vault's choice)           |
| `DEVBOULE_PI_SANDBOX`            | `true`   | macOS Seatbelt sandbox around the sidecar (Rust)   |
| `DEVBOULE_CENSOR_REVIEW_ENABLED` | `true`   | Post-edit Censor review hook (sidecar)             |

Boolean vars accept `0/false/no/off` (case-insensitive) as false; anything else is
true with a warning.

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│  Devboule Tauri App                                      │
│                                                          │
│  Rust (pi_sidecar.rs)                                    │
│  ├─ spawns `node pi-sidecar/sidecar.mjs` (≤8 sessions,   │
│  │  Seatbelt-sandboxed on macOS)                         │
│  ├─ writes prompt JSONL to stdin                         │
│  ├─ reads event JSONL from stdout (dedicated reader      │
│  │  thread — heavy work is detached, never blocks it)    │
│  ├─ EventMapper: pi events → ConsoleActivity snapshots   │
│  │  → app.emit("mini-activity://<session>")              │
│  └─ Censor: devboule_censor_review → voted review →      │
│     findings prompted back via send_prompt_to_session    │
│                                                          │
│  React (useAgentConsole.ts) — UNCHANGED                  │
└──────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────┐
│  Node Sidecar (sidecar.mjs)                              │
│  ├─ createAgentSession() + extensions + MCP              │
│  ├─ session.subscribe() → enrich with _devboule → stdout │
│  ├─ stdin commands: prompt (FIFO-queued mid-turn), quit  │
│  └─ Censor hook: tracks .rs edits per turn, emits        │
│     devboule_censor_review at agent_end                  │
└──────────────────────────────────────────────────────────┘
```

## Protocol (JSONL over stdio)

**stdin (commands)**:

```json
{"type":"prompt","message":"Your prompt here"}
{"type":"quit"}
```

**stdout (events, abridged)**:

```json
{"type":"ready","oracleMCP":true}
{"type":"response","command":"prompt","success":true}
{"type":"response","command":"prompt","success":true,"queued":true}
{"type":"agent_start"}
{"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","delta":"…"}}
{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"Hello"}}
{"type":"tool_execution_start","toolCallId":"…","toolName":"read","args":{...}}
{"type":"tool_execution_update","toolCallId":"…","partialResult":{...}}
{"type":"tool_execution_end","toolCallId":"…","result":{...},"isError":false}
{"type":"compaction_start","reason":"threshold"}
{"type":"auto_retry_start","attempt":1,"maxAttempts":3,"errorMessage":"…"}
{"type":"devboule_censor_review","files":["src/x.rs"],"diffs":["…"]}
{"type":"agent_end","messages":[...]}
{"type":"queue_dropped","count":1}
{"type":"error","context":"…","message":"…"}
```

Every event additionally carries `_devboule: {agentRole, projectId, sessionId}`.

## Known gaps / next phases

+ **Node binary bundling** (5c): still runs the system `node`; packaging via
  `@yao-pkg/pkg --sea --compress Zstd` per decision #5 is pending. Pin of extension
  versions ships with it.
+ **Sidecar-side session persistence**: the sidecar uses `SessionManager.inMemory()`;
  the RUST side persists session metadata (status, last-active) across app restarts,
  but a killed sidecar loses its in-process pi conversation.
+ **In-app extension bootstrap**: planned — app-managed agent dir with a curated
  extension set installed on first launch (testers won't need a global `~/.pi/agent`).
