# Backend Self-Test Rig — P0 Inventory (2026-07-15)

Ground truth for the rig design (plan: `backend-selftest-rig-plan-2026-07.md`).
Source: 6 deepseek-v4-flash recon reports (scratchpad rig-recon/), key claims
spot-verified on disk. Line numbers are as of `phase1/infra@0ff5508`.

## 1. Entry points (what the rig can drive)

### Orchestrator (pi sidecar path)
- Tauri commands: `launch_project_agent_terminal` (projects.rs:1383),
  `prepare_project_agent_prompt`, `orchestrator_steer` (projects.rs:4106),
  `planner_reset_chat` (projects.rs:4181).
- Chain: `prepare_or_launch_project_agent` → `fence_stale_orchestrator` (only
  role=orchestrator; kills predecessor by stable id) → `spawn_pi_orchestrator_session`
  → `spawn_sidecar_for_role` (pi_sidecar.rs:1883) → `get_or_spawn_session` →
  `spawn_pi_session_inner` (real `node sidecar.mjs` spawn).
- Pi sessions have NO ledger entry by design (pi route returns before
  `record_launch_pending`); visibility = read-time overlay in `get_agent_live_state`.
- Goal delivery: `inject_console_entry(Chat{user, msg_id=initial_goal_msg_id})`
  BEFORE prompt delivery (round-1 invariant to assert).

### Main / mini coder — TWO distinct paths (don't conflate)
- **Directive executor path**: MCP `spawn_main_coder`/`spawn_mini_coder` (Python,
  aspis_mcp.py:6314/6358) or Rust twin `append_main_coder_directive`
  (main_coder.rs:101) → directive in `.aspis-agents.json` → executor `run_pass`
  loop (1.5s tick) claims and launches (PTY or agentic worker). Cloud kind
  REJECTED here (`backend_supports_directive_dispatch`,
  mini_coder_executor.rs:1299; guard again in claim_and_launch; test at
  executor_tests.rs:4539) because `HttpAgentLlm` has no auth header
  (agentic_transport.rs:148-195).
- **Pi sidecar path**: `spawn_pi_coder_session` (projects.rs:1505) →
  `spawn_sidecar_for_role("main-coder"|"mini-coder")`, single id minted via
  `pi_override_agent_id` (pi_sidecar.rs:1539).
- **Steer gap (confirmed, known ticket)**: `mini_coder_steer`
  (mini_coder_executor.rs:3797) targets only directive-queue agents; pi coder
  sessions have no directive row → steer goes nowhere. Rig should assert current
  behavior (error/no-op), flip when the routing ticket lands.
- Emit-edits apply: `apply_emitted_edits` (mini_edit_apply.rs:444) — pure + fs,
  3-tier span match (exact/ws-normalized/fuzzy≥0.92), allowlist, symlink guard,
  40-edit cap, two-pass atomic. Directly callable in tests (already covered).
- Precedent for real-process tests: `headless_one_shot_writes_result_and_eofs`
  (`#[ignore]`, executor_tests.rs:215) spawns a real PTY, writes result, reads back.

## 2. Sidecar contract (VERIFIED — the standalone spawn recipe)

Spawn: `node sidecar.mjs`, cwd = the sidecar.mjs dir, stdin/stdout piped.

Env (spawn_pi_session_inner, pi_sidecar.rs:~1153-1347):
```
DEVBOULE_PI_PROVIDER=openai|openrouter     DEVBOULE_PI_MODEL=<id>
DEVBOULE_PI_BASE_URL=<url>                 # set → sidecar builds a private temp
                                           # models.json with ONLY this provider
DEVBOULE_SESSION_ID=<agent-id>             DEVBOULE_AGENT_ROLE=orchestrator|main-coder|mini-coder
DEVBOULE_PROJECT_ID=<id>                   DEVBOULE_PROJECT_ROOT=<abs path>
DEVBOULE_PIGEON_ENABLED=true|false         PI_CODING_AGENT_DIR=<agent dir with settings.json>
OPENAI_API_KEY=ollama|mlx|<real>           # placeholder for local
OPENROUTER_API_KEY=<real>                  # cloud
EXA/BRAVE/TAVILY/PERPLEXITY/GEMINI/PARALLEL_API_KEY  # websearch, optional (vault)
```

Stdin (verified sidecar.mjs:640-713): `{"type":"prompt","message":...}` (≤100k
chars, queue≤5), `{"type":"classified",...}` (Rust's reply to the sidecar's
`classify_prompt` request — pigeon only), `{"type":"quit"}`.

Stdout: `{"type":"ready","oracleMCP":bool}` after bindExtensions → pi SDK events
verbatim (`agent_start`, `message_update`, `tool_execution_*`, `message_start`...)
each with `_devboule{agentRole,projectId,sessionId}` → `{"type":"response",
"command":"prompt","success":bool,"error"?}` per turn. Internal events:
`devboule_censor_review`, `compaction_*`, `auto_retry_*`, `error`, `queue_dropped`.

MCP prerequisites: `<project_root>/.pi/mcp.json` with `aspis-management` entry
(Rust merge-writes it at spawn via `ensure_project_pi_mcp_config`), sibling
`node_modules` next to sidecar.mjs (resolver requirement), `PI_CODING_AGENT_DIR`
with `settings.json`.

Script/binary resolvers (pi_sidecar.rs:1119, pi_extensions.rs:165): cwd →
`CARGO_MANIFEST_DIR/..` (debug-only) → resource_dir → TAURI_RESOURCE_DIR.
Harness from repo root = candidate 1 wins.

## 3. Observability (what the rig asserts on)

Durable channels — POLL THESE:
- `mini_activity_snapshot(agent_id)` (mini_activity.rs:541) — richest trace;
  VERIFIED hydrate-on-miss from bridge file `<projects_dir>/.devboule-activity/
  <id>.jsonl` (256KB tail replay). CAVEAT: ids with no bridge file (pi minis,
  coders) fall through blank — pi-coder console durability is the known ticket.
- `get_agent_live_state` — fleet state + pi overlay (`.aspis-agents.json`,
  locked, atomic).
- `.devboule/pi-sessions.json` — PersistedSession {id, agent_role, project_id,
  created_at, last_active_at, status, model}; path root = **cwd** of the app
  process (pi_project_root = current_dir!), 24h active→crashed, 7d purge.
- Censor shards `<project>/.aspis-censor/<sha256(rel)>.json` + `censor_get_findings`
  / `censor_count_open` commands + steer file `.aspis/steer_censor`.
- ConsoleEntry variants: coder, spawn, websearch, banner, thinking, chat, question.
- ~45 pure-read Tauri commands catalogued in rig-recon/report4 (polling endpoints).

Event-only BLIND SPOTS (fire-and-forget, need small write-through product fixes
to become testable — ticket candidates):
`mini://stuck`, `sandbox://consent-request` (cloud-duplex branch),
`agent-terminal://<id>` ring buffer, `censor://scan-started`,
`censor://mini-findings` (partial), `design-stream:<id>` deltas.

Injection trick: the activity tail parses `.devboule-activity/<id>.jsonl` every
300ms — a test can APPEND bridge events itself to simulate an orchestrator
(kinds: milestone, websearch, chat, chat-delta, question, banner, thinking).

## 4. Censor pipeline

Triggers: mini write completion (executor:1847-1903, sync deterministic linters)
→ optional async LLM via Pigeon `censor-pool` mailbox (enqueued by aspis_mcp.py
`_maybe_enqueue_censor_reviews`, drained by `ingest_pigeon_censor_reviews` every
tick); explicit `censor_review_now` command; coarse pass (clippy/tsc) after write.
Providers: `censorLocalAi` config — ollama/omlx local, cloud via vault
`provider:censor_cloud`. Findings: shards on disk, stateless across restarts;
linter-only findings need NO LLM (good deterministic rig tier).

## 5. MCP surface

- Server: `oracle-data/venv/bin/python -m oracle.server.aspis_mcp --root <mgmt>
  --projects-dir <dir>` — stdio FastMCP, runs WITHOUT the app. But
  `spawn_mini_coder(write=true, wait=true)` needs the Rust executor running
  (directive sits in `.aspis-agents.json` otherwise) — a wait=true call without
  the app TIMES OUT; wait=false returns `{status:'running'}` forever.
- Choreography to assert: register → compact ack (~1-3KB, tokens stripped,
  sessionToken present) → heartbeat → project_get → claim → oracle_context →
  spawn_mini → censor_findings → dispose → update_status.
- Status guards: claim blocked on paused/archived/done (aspis_mcp.py:8253);
  provider mutations require active (2935). `draft` handling = open owner ticket.
- Existing pytest: oracle/tests/test_aspis_mcp.py (356 tests) covers tools in
  isolation; does NOT cover live sidecar, real Pigeon transport, censor LLM, or
  the chained work cycle — exactly the rig's job.
- oracle_context needs indexed `oracle-data/` (LanceDB+SQLite); oracle_ask
  generative needs an LLM key, else extractive.

## 6. Placement matrix fixtures (role × Local | Cloud API | Cloud CLI)

Backend resolution: `resolve_coder_env_for_sidecar(app, role)` (pi_sidecar.rs:
~679-880). VERIFIED: orchestrator arm → None → `localCoderBackend`; coder →
`mainCoderBackend` NO-fallback-to-mini (privacy); mini → `miniCoderBackend`;
verifier → `verifierBackend` (inherits main by design). Non-sidecar kinds
(api/codex/appleFm/openai) fall back to `localCoderBackend`; ultimate default =
openrouter hy3:free.

Settings fixture = write `config.json` directly (atomic temp+rename, located by
`locate_config_path`): `miniCoderBackend` / `mainCoderBackend` /
`verifierBackend` / `localCoderBackend` + `rolesConfig.{orchestrator,coder,
verifier}Client`.

- **Local cell**: `{kind:"omlx", model, baseUrl:"http://127.0.0.1:<port>/v1"}` +
  a mock/real OpenAI-dialect server on loopback (`GET /v1/models`,
  `POST /v1/chat/completions`). oMLX probe = :8000/v1/models, ollama =
  :11434/api/tags (provider_detect.rs:43, 1.5s timeout).
- **Cloud API cell**: `validate_cloud_base_url` (local_coder.rs:58-185) demands
  https + non-loopback FQDN → a loopback mock does NOT pass config validation.
  Options: (a) live cheap provider (works today), (b) test-only env override in
  the validator (product change, needs owner ok), (c) exercise the sidecar layer
  directly with DEVBOULE_PI_BASE_URL=mock (sidecar doesn't re-validate). Vault:
  `provider:cloud_llm` via keyring ONLY — NO env/file fallback (VERIFIED
  vault.rs); headless keychain write risks macOS ACL prompt → prefer (c) +
  assert the missing-key Banner path instead.
- **Cloud CLI cell**: `rolesConfig` client codex/claude; fake CLI wrapper script
  on PATH (executor path, not sidecar).

## 7. Websearch

Performed by the pi-web-access extension inside the sidecar; keys from vault →
env (`WEBSEARCH_ENV_MAP`, pi_sidecar.rs:~642); provider default in
`web-search.json` under the pi agent dir. Sidecar echoes `devboule.websearch`
custom messages → `ConsoleEntry::WebSearch` (observable via snapshot). Fully
deterministic websearch = not possible without a canned provider → start as a
live-only scenario (real Exa/Brave key), assert the WebSearch console entry.

## 8. Rig architecture decision (from the evidence)

**Two layers:**
- **Layer A — no app process (cheap, deterministic, covers most history-bugs):**
  standalone sidecar spawn (§2 recipe) + standalone MCP stdio server + mock
  OpenAI server + forged launch token in a sandbox projects dir. Asserts: ready
  event, user echo ordering, compact ack size, non-empty final text, failure
  banner events, MCP choreography, censor shard reads. This is the round-1/5
  repro generalized — proven approach.
- **Layer B — real Rust state (`#[ignore]` integration tests in src-tauri):**
  executor-path scenarios (directive lifecycle, PTY mini, emit-edits apply,
  cloud rejection, censor-after-write) using tempdir fixtures; precedent already
  in-tree (`headless_one_shot_writes_result_and_eofs`). Spawning the pi sidecar
  through `spawn_sidecar_for_role` needs an AppHandle → tauri `test` feature
  mock, or a small refactor injecting `PiSidecarState` — decide at P1 kickoff.

**Product changes the rig wants (owner-ticket candidates, NOT prerequisites):**
blind-spot write-throughs (§3), cloud-URL test override (§6b), vault test seam,
pi-coder bridge-file durability (existing ticket).

**Sequencing note:** pi-sessions.json root = app cwd → Layer B tests must pin
cwd per-test or the suite pollutes real state (this exact trap caused the
phantom 3393/1 failure, round 4).
