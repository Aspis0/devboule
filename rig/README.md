# Self-Test Rig (Layer A) — README

Headless self-test rig for the Devboule pi-sidecar. Spawns `node sidecar.mjs` directly
against a mock OpenAI-compatible server — no Tauri app, no GUI, no real LLM.

## Quick Start

```bash
# From repo root — Layer A (python, ~50s) and Layer B (Rust executor integration)
npm run rig:smoke   # RIG=1 pytest rig/ -v
npm run rig:rust    # cargo test rig_tests -- --ignored (4 integration tests)
npm run rig         # both
```

Or directly: `RIG=1 oracle-data/venv/bin/python -m pytest rig/ -v`.
Requires `RIG=1` environment variable (prevents accidental runs in normal CI).

Opt-in live cells (real network/keys, normally skipped):
- `RIG_LIVE=1` — websearch live scenario (Exa MCP, keyless) + plan-console probe.

## Standing gate (house rule, P4 — 2026-07-15)

The rig is the **mandatory pre-e2e gate** for backend work:

1. **Every fix round runs `npm run rig` BEFORE asking the owner for live e2e.**
   Owner time is the scarcest resource; the rig catches the silent-success /
   dead-channel / wrong-auth bug classes headlessly.
2. **Every new bug found live becomes a rig scenario FIRST**, then gets fixed —
   the scenario must fail on the buggy code and pass on the fix (same TDD rule
   as regression tests). Six product bugs were found this way in P1–P3.
3. New backend surfaces (sidecar events, MCP tools, placement kinds) ship with a
   rig scenario in the same round, or an explicit note in
   `docs/future-work-2026-07.md` saying why not.

What the rig cannot cover (stays owner e2e): real GUI rendering, macOS
Keychain/App Nap, packaged-build resource paths, true `npm run tauri dev` env.

## Files

| File | Purpose |
|------|---------|
| `mock_llm.py` | Minimal OpenAI-compatible HTTP server (`/v1/models`, `/v1/chat/completions` with streaming + non-streaming). Programmable response queue, failure injection, request logging. |
| `world.py` | Sandbox world builder. Creates temp dir with: `project_root/` (git repo + planted bug), `agent_dir/` (settings.json), `projects_dir/` (empty). |
| `sidecar_driver.py` | `SidecarSession` — spawns `node pi-sidecar/sidecar.mjs` with correct env, JSONL stdin/stdout, reader thread, `wait_event()`, `send_prompt()`, `close()` with hard timeout guard. |
| `test_smoke.py` | Two pytest scenarios (skipped unless `RIG=1`): `test_ciao_smoke` (round-5 regression), `test_provider_failure_surfaces_error`. |

## Environment Contract

The sidecar is spawned with exactly these env vars (all leakage vars stripped):

| Variable | Value | Source |
|----------|-------|--------|
| `DEVBOULE_PI_PROVIDER` | `openai` | Fixed |
| `DEVBOULE_PI_MODEL` | `rig-model` | Fixed |
| `DEVBOULE_PI_BASE_URL` | `http://127.0.0.1:<mock_port>/v1` | Mock server |
| `OPENAI_API_KEY` | `rig-key` | Placeholder |
| `DEVBOULE_SESSION_ID` | Test-provided (e.g. `rig-test-ciao`) | Test |
| `DEVBOULE_AGENT_ROLE` | `orchestrator` \| `main-coder` \| `mini-coder` | Test |
| `DEVBOULE_PROJECT_ID` | `rig-project` | Fixed |
| `DEVBOULE_PROJECT_ROOT` | `world.project_root` | Sandbox |
| `DEVBOULE_PIGEON_ENABLED` | `false` (default) | Test |
| `PI_CODING_AGENT_DIR` | `world.agent_dir` | Sandbox |

**Stripped**: Any `DEVBOULE_*`, `OPENAI_*`, `OPENROUTER_*` from parent environment.

## Mock LLM Server (`mock_llm.py`)

### Endpoints

- `GET /v1/models` → `{"data":[{"id":"rig-model","object":"model"}]}`
- `POST /v1/chat/completions` → Supports `stream:false` (single JSON) and `stream:true` (SSE chunks ending with `data: [DONE]`). Returns scripted responses with `usage` fields.

### Programmable Behavior

```python
mock = MockLLMServer()
mock.start()

# Queue scripted responses (consumed FIFO)
mock.set_responses(["First reply", "Second reply"])

# Or set a default
mock.set_default_response("Fallback reply")

# Failure injection
mock.set_fail_mode(count=2, mode="500")      # Next 2 requests → HTTP 500
mock.set_fail_mode(count=1, mode="drop")     # Next 1 request → TCP reset
mock.clear_fail_mode()

# Inspection
log = mock.get_request_log()  # [{"method","path","body","timestamp"}, ...]
mock.clear_request_log()
```

### Context Manager

```python
with MockLLMServer() as mock:
    mock.set_responses(["Hello!"])
    url = mock.base_url  # http://127.0.0.1:XXXXX
    # test runs...
# auto-stops on exit
```

## World Builder (`world.py`)

```python
with build_world_in_temp() as world:
    world.project_root   # Path to git repo with planted bug
    world.agent_dir      # PI_CODING_AGENT_DIR with settings.json
    world.projects_dir   # Empty (for future phases)
    # auto-cleanup on exit
```

Project structure created:
```
project_root/
├── Cargo.toml
├── README.md
└── src/lib.rs          # Contains: pub fn add(a,b) -> i32 { a + b + 1 }  // BUG + TODO
```

## Sidecar Driver (`sidecar_driver.py`)

### `SidecarSession`

```python
session = SidecarSession(
    session_id="test-123",
    agent_role="orchestrator",
    mock_base_url="http://127.0.0.1:12345/v1",
    project_root=world.project_root,
    agent_dir=world.agent_dir,
    pigeon_enabled=False,  # default
)

# Wait for ready event (with oracleMCP bool)
ready = session.wait_ready(timeout=30.0)
assert ready.data["oracleMCP"] is True

# Send prompt
session.send_prompt("ciao")

# Wait for any event matching predicate
event = session.wait_event(lambda e: e.data.get("type") == "response", timeout=30.0)

# Access all collected events
for e in session.events:
    print(e.data)

# Clean shutdown (SIGTERM → grace → SIGKILL)
session.close()
```

### Key Methods

| Method | Description |
|--------|-------------|
| `wait_ready(timeout)` | Blocks until `{"type":"ready"}` event, returns it. |
| `send_prompt(text)` | Sends `{"type":"prompt","message":text}` to stdin. |
| `send_quit()` | Sends `{"type":"quit"}`. |
| `wait_event(predicate, timeout)` | Blocks until `predicate(event)` is true. |
| `events` | List of all `SidecarEvent` received (with timestamp, raw line). |
| `stderr_text()` | Collected stderr as string. |
| `close()` | Graceful shutdown with hard kill fallback. |

### Timeout Guard

All `wait_*` methods have a **hard 60s default timeout**. On timeout, the exception includes:
- Full event list (for debugging)
- Full stderr capture
- This is intentional: **debuggability is the whole point**.

## Adding a Scenario

1. Create a new test class in `test_smoke.py` (or new file `test_<name>.py`).
2. Use the `with build_world_in_temp() as world:` + `with MockLLMServer() as mock:` pattern.
3. Script mock responses with `mock.set_responses([...])`.
4. Spawn `SidecarSession` with desired role.
5. Assert on `session.events` — every pi SDK event is enriched with `_devboule` metadata.
6. Add the test to the class, run with `RIG=1 pytest rig/test_smoke.py::TestNewScenario -v`.

### Example: Testing a Tool Call Flow

```python
def test_tool_call_flow(self):
    with build_world_in_temp() as world:
        with MockLLMServer() as mock:
            # Script a response that includes a tool call
            mock.set_responses([{
                "content": "I'll read that file.",
                "tool_calls": [{"id": "call_1", "name": "read", "arguments": {"path": "src/lib.rs"}}]
            }])

            with SidecarSession(...) as session:
                session.wait_ready()
                session.send_prompt("read src/lib.rs")

                # Collect tool_execution_start/end events
                tool_events = [e for e in session.events if e.data.get("type", "").startswith("tool_execution")]
                assert len(tool_events) >= 2
```

## Known Contract Surprises (from sidecar.mjs)

| Expected (inventory) | Actual (sidecar.mjs) | Note |
|----------------------|---------------------|------|
| `agent_start`, `message_update`, `tool_execution_*`, `message_start` | pi SDK events: `agent_start`, `message_start`, `text_start`, `text_delta`, `text_end`, `toolcall_start`, `toolcall_delta`, `toolcall_end`, `agent_end` | The sidecar emits **raw pi SDK events** verbatim. Test assertions should match on `type` prefixes (`text_*`, `toolcall_*`, `agent_*`). |
| `ready` has `oracleMCP` | ✅ Confirmed — `oracleMCP` boolean present | |
| `response` event | ✅ Confirmed — `{type:"response",command:"prompt",success:bool,error?}` | |

## Troubleshooting

| Symptom | Likely Cause | Fix |
|---------|--------------|-----|
| `sidecar.mjs not found` | Repo root detection failed | Ensure test runs from repo root or set `repo_root` explicitly |
| Mock 500 but sidecar crashes | `bindExtensions` fails without MCP | Expected: sidecar emits `ready` with `oracleMCP:false` (degraded). If it crashes, check stderr in test output. |
| `OPENAI_API_KEY` leakage | Parent env has it | Driver strips all `OPENAI_*`/`DEVBOULE_*`/`OPENROUTER_*` — verify with `print(os.environ)` in test. |
| Timeout on `wait_ready` | Sidecar failed to start | Check `session.stderr_text()` in exception message. |
| Streaming chunks not parsed | Mock SSE format mismatch | Mock sends `data: {...}\n\n` + `data: [DONE]\n\n` — matches OpenAI spec. |

## Cleanup Guarantees

- All temp dirs: `tempfile.mkdtemp()` → cleaned in `World.__exit__` / `World.cleanup()`
- Mock server: `HTTPServer.shutdown()` + thread join in `MockLLMServer.__exit__`
- Sidecar: `SIGTERM` → 2s grace → `SIGKILL` + thread joins in `SidecarSession.close()`
- **No child processes, no temp files, no real-state pollution survive a test run.**

## Fixtures

Versioned backend snapshots committed under `rig/fixtures/`. The rig proves what
the backend EMITS; vitest proves what the UI models CONSUME. The seam between
them = these recorded fixtures. Fixtures are committed — vitest runs WITHOUT the
rig. Regen only when the wire shape changes intentionally.

| Fixture | Description | Regen test |
|---------|-------------|------------|
| `console-activity.json` | `ConsoleActivity` snapshot covering chat (user + assistant), thinking, coder/tool rows, websearch, and banner entries. | `regen_console_activity_fixture` (Rust, `#[ignore]`) |
| `agents-state.json` | Fleet state (`AgentLiveState`) with orchestrator/coder/mini sessions, two `MiniCoderDirective` rows (one running, one failed with `stuckReport`), and a directive with `censorSummary`. | `regen_agents_state_fixture` (Rust, `#[ignore]`) |
| `project-tasks.json` | Kanban tasks array (from `project_get`) with `dependsOn` edges, normalized timestamps. | `test_task_deps_roundtrip_for_arrows` (Python, `RIG_FIXTURES=1` opt-in) |

### Regen Commands

```bash
# Rust fixtures (run by the orchestrator — do NOT run during this task):
cargo test --manifest-path src-tauri/Cargo.toml regen_console_activity_fixture -- --ignored
cargo test --manifest-path src-tauri/Cargo.toml regen_agents_state_fixture -- --ignored

# Python fixture (run manually with RIG=1 RIG_FIXTURES=1):
RIG=1 RIG_FIXTURES=1 oracle-data/venv/bin/python -m pytest rig/test_task_lifecycle.py::test_task_deps_roundtrip_for_arrows -v --timeout=120
```