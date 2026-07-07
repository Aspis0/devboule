# pi-sidecar — Phase 0 Spike

A minimal Node.js sidecar that embeds the pi SDK (`@earendil-works/pi-coding-agent`)
and bridges it to Devboule's Rust backend via JSONL over stdio.

## What it proves

1. **Tool registration**: `oracle_ask` custom tool is registered via `defineTool()` and
   invoked by the pi agent during a conversation.
2. **Event streaming**: All pi SDK events (`text_delta`, `tool_execution_*`, etc.) are
   forwarded as JSONL to stdout.
3. **JSONL protocol**: The Rust backend spawns this process, sends prompts via stdin,
   and receives streamed events via stdout.
4. **Console rendering**: The Rust side maps pi events to the existing
   `MiniActivityEvent` / `ConsoleActivity` schema so `WorkConsole.tsx` renders them
   WITHOUT any React changes.

## Prerequisites

- **Node.js** ≥ 20 (tested with v26.3.0)
- **An API key** for at least one provider. Set the env var for your provider, e.g.:

  ```bash
  export OPENAI_API_KEY="sk-..."
  ```

## Setup (one-time)

```bash
cd pi-sidecar
npm install
```

This installs `@earendil-works/pi-coding-agent@0.80.3` locally.

## Running the spike (end-to-end)

### Option A: Standalone test (no Tauri)

```bash
cd pi-sidecar
export OPENAI_API_KEY="sk-..."
echo '{"type":"prompt","message":"Use oracle_ask to search for authentication code"}' | node sidecar.mjs
```

You'll see JSONL events stream to stdout. When the agent calls `oracle_ask`, the
canned `[SPIKE PLACEHOLDER]` response appears in the `tool_execution_end` event.

### Option B: Full end-to-end with Tauri

1. **Start the Tauri dev server** (from the repo root):

   ```bash
   npm run tauri dev
   ```

2. **Open a project** in the Devboule app and select an agent in the Work Console.

3. **Send a prompt to the pi sidecar** — the spike registers a dev-only Tauri command
   `spike_pi_prompt`. You can invoke it from the browser devtools console:

   ```js
   window.__TAURI__.core.invoke("spike_pi_prompt", { text: "Use oracle_ask to search for auth code" });
   ```

   Or via the Tauri CLI:

   ```bash
   npx tauri invoke spike_pi_prompt --text "Use oracle_ask to search for auth code"
   ```

4. **Watch the Work Console**: pi events stream into the existing Console tab —
   text appears as `ChatEntry` rows, tool calls as `CoderEntry` rows, and the
   agent lifecycle as running/stopped state changes.

## Configuration

Pass provider/model via env vars to the Rust launcher (defaults shown):

| Env var                | Default                    | Description                     |
|------------------------|----------------------------|---------------------------------|
| `DEVBOULE_PI_PROVIDER` | `openai`  | pi provider name (Claude blocked per decision #10) |
| `DEVBOULE_PI_MODEL`    | `gpt-4o`  | Model ID within the provider    |

The API key for the chosen provider must be set as an env var the pi SDK recognizes
(`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, etc.) — see pi's `AuthStorage` docs.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│  Devboule Tauri App                                     │
│                                                         │
│  Rust (pi_sidecar.rs)                                   │
│  ├─ spawns `node pi-sidecar/sidecar.mjs`                │
│  ├─ writes prompt JSONL to stdin                        │
│  ├─ reads event JSONL from stdout                       │
│  └─ maps pi events → MiniActivityEvent → app.emit()     │
│                                                         │
│  React (useAgentConsole.ts) — UNCHANGED                 │
│  └─ subscribes to mini-activity://pi-agent              │
│     └─ renders via WorkConsole.tsx                      │
└─────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────┐
│  Node Sidecar (sidecar.mjs)                             │
│  ├─ createAgentSession()                                │
│  ├─ registerTool("oracle_ask") → canned response        │
│  ├─ session.subscribe() → emit events to stdout         │
│  └─ reads prompt commands from stdin                    │
└─────────────────────────────────────────────────────────┘
```

## Protocol (JSONL over stdio)

**stdin (commands)**:

```json
{"type":"prompt","message":"Your prompt here"}
{"type":"quit"}
```

**stdout (events)**:

```json
{"type":"ready"}
{"type":"response","command":"prompt","success":true}
{"type":"agent_start"}
{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"Hello"}}
{"type":"tool_execution_start","toolName":"oracle_ask","args":{"question":"..."}}
{"type":"tool_execution_end","toolName":"oracle_ask","result":{...},"isError":false}
{"type":"agent_end","messages":[...]}
```

## What's NOT wired (TODO for later phases)

- **Real Oracle proxy**: `oracle_ask` returns a canned placeholder. Phase 1 will
  proxy to the Python Oracle MCP server.
- **Provider/model from vault**: The spike hardcodes defaults. Decision #9 says the
  vault is the source of truth; the Rust side will read from
  `save_oracle_llm_settings` (`vault.rs:927`) and pass via env vars.
- **Node binary bundling**: The spike runs `node` directly. Phase 5 will use
  `@yao-pkg/pkg --sea --compress Zstd` per decision #5.
- **Session persistence**: Uses `SessionManager.inMemory()`. No session survives
  a restart.
