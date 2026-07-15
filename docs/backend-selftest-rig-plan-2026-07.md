# Backend Self-Test Rig — plan (2026-07-15)

## Why

Six orchestrator-chat fix rounds proved most "e2e bugs" are reproducible headless:
the deterministic sidecar repro (forged launch token + `sidecar.mjs` spawn + stdin
prompt) root-caused rounds 1 and 5 without ever opening the GUI. Owner directive:
before any further owner e2e, build a rig that lets the orchestrating agent test the
backend itself — orchestrator, main coder, mini coder, censor, MCP tools, websearch,
cloud vs local — by simulating real work end-to-end.

## What already exists (validated building blocks)

1. **Deterministic sidecar repro** — forge `launchTokenHash=sha256(tok)` + fresh
   `launchTokenIssuedAt` + `status=launch_pending` in `.aspis-agents.json`, spawn
   `pi-sidecar/sidecar.mjs` with the app env, feed `{"type":"prompt",...}` on stdin,
   read the full event stream on stdout. Restore backup after.
2. **Real local model** — oMLX (loopback) answers for free; any pi provider can be the
   sidecar model (owner: any responding model is fine for sidecar tests).
3. **MiniActivityStore write-through** (round 1 fix) — snapshots are now readable from
   state, not just fire-and-forget Tauri events. This is the observation channel.
4. **aspis_mcp pytest suite** (356 tests) — the MCP tool surface is already
   headless-testable; `compact_session_ack` repro used it.
5. **`--ignored` integration-test pattern** (`dump_real_city_state`) — precedent for
   cargo tests that touch real state, opt-in only.
6. **Per-role backend resolution** (`resolve_coder_env_for_sidecar`, round 6) — the
   cloud/local switch is a pure-ish Rust surface, drivable without UI.

## Phases

### P0 — Inventory (flash recon, ~6-8 explorers, verify every finding on disk)
Map every headless entry point and its state contract:
- Tauri commands on the agent path (spawn/steer/stop for orchestrator, main, mini)
  and what each needs from managed state (AppHandle? plain state structs?).
- State files: `.aspis-agents.json`, `.devboule/pi-sessions.json`, `.pi/mcp.json`,
  project `config.json`, vault/keychain touchpoints (what can be stubbed with env or
  a temp vault vs what hits the real Keychain).
- The sidecar env contract (every DEVBOULE_* / model env the Rust spawn sets) — so the
  rig can reproduce it exactly, or better, call the same Rust builder function.
- Censor pipeline entry points (what triggers a review, where findings land).
- Websearch path (which tool, which role, what provider/key it needs).
- Event observation: which flows are observable via MiniActivityStore / state files vs
  Tauri-event-only (those need a collector or a small write-through, flagged as
  possible product fixes — round-1 taught us event-only paths ARE the bugs).

Deliverable: `docs/backend-selftest-inventory.md` — the ground truth the rig builds on.

### P1 — The rig skeleton
A headless harness in `src-tauri` (ignored integration tests, or a `devboule-rig`
bin — decide after P0 shows how much AppHandle is actually needed; `tauri::test`
mock app is the fallback):
- **Sandbox world builder**: temp dir with a golden fake project (small repo, planted
  bug, real git init), generated `config.json`, `.aspis-agents.json` with forged
  token, `.pi/mcp.json` — everything the app would have written.
- **Session driver**: spawn orchestrator/main/mini through the SAME Rust functions the
  UI calls (not re-implemented shell spawns), with the model pointed at oMLX or any
  live pi provider.
- **Observer**: poll MiniActivityStore snapshots + state files; assert on the
  choreography (user echo present, register ack < N bytes, final text non-empty,
  banner on failure) — exactly the invariants the 6 rounds fixed.
- Baseline scenario: **"ciao" smoke** — the round-5 repro, promoted from ad-hoc script
  to rig scenario #1. Regression guard for the whole 2-month bug class.

### P2 — Work simulation scenarios
Scripted scenarios over the golden project, each one a named rig case:
- **Goal → plan**: typed goal delivered (round-1 defect #4 regression), plan lands.
- **Main coder round-trip**: orchestrator dispatches a directive → main coder session
  spawns (ONE id, ledger row, console visible — round-2 regressions) → coder edits the
  planted-bug file → edits observable on disk.
- **Mini coder loop**: spawn_mini_coder → emit-edits → applied.
- **Censor**: coder output triggers review → findings land in the sin/censor channel.
- **MCP choreography**: agent_register/heartbeat compact acks, oracle_ask,
  project_get on the sandbox project (also covers the `draft`-status rejection ticket).
- **Failure injection**: kill the model mid-turn / wrong key / dead provider → Banner
  asserted, no silent success (the round-1 #3 and round-6 vault-key classes).

### P3 — The matrix
Run the P2 scenarios across placements, per role:
- **Local (oMLX)** vs **Cloud API** (openai-compatible endpoint — can point at a live
  cheap provider or a tiny local mock server speaking the OpenAI dialect for
  deterministic runs) vs **Cloud CLI** where scriptable.
- **Websearch**: one scenario where the orchestrator must call the websearch tool and
  the result reaches the final answer.
- Cloud-rejection guards: directive executor + agentic branch reject cloud loudly
  (round-6 invariant), coder does NOT inherit mini's cloud backend.
Not every cell every run: smoke set (local-only, ~2 min) vs full matrix (opt-in).

### P4 — Make it the standing gate — DONE 2026-07-15 (`0be5de2`)
- `npm run rig:smoke` / `rig:rust` / `rig` + "Standing gate" section in rig/README.md.
- Standing rule active: every future fix round runs the rig BEFORE asking owner e2e; new
  bug found live → first step is a new rig scenario that reproduces it.
- CI-ability deferred (needs oMLX or the mock provider on the runner).

## P5 — The coder lane (owner-mandated 2026-07-15)

P0–P4 covered the ORCHESTRATOR lane. P5 covers everything downstream: main coder →
mini coder, with every durable surface the UI feeds on asserted at each hop —
activity/console, kanban tasks, fleet rows, task-dependency arrows.

**Ground truth (inventory §1/§3 + frontend consumers, disk-verified 2026-07-15):**
two distinct coder paths that must NOT be conflated — (a) the **directive executor
path** (MCP `spawn_main_coder`/`spawn_mini_coder` → `.aspis-agents.json` directive →
`run_pass` 1.5s tick → PTY or agentic worker; cloud kind rejected loudly) and (b) the
**pi sidecar path** (`spawn_pi_coder_session` → `spawn_sidecar_for_role("main-coder"|
"mini-coder")`). UI feeds: mini console = `mini_activity_snapshot` (hydrates from
bridge file `.devboule-activity/<id>.jsonl`, 300ms tail) → `useAgentConsole`; kanban =
project tasks state (`project_claim_task`/`project_update_status` etc. → TaskCard/
projectWorkspaceModel); fleet rows = `get_agent_live_state` → `agentRowModel`
(AgentsView); frecce = `TaskDependencyArrows` over task `deps` on the board.

### P5a — Mini-coder work simulation end-to-end (directive path, python rig)
The full choreography a real work session produces, against the mock LLM:
`spawn_mini_coder` (MCP, forged orchestrator session) → directive row appears in
`.aspis-agents.json` (assert fleet-visible: the row that AgentsView renders) →
executor claims → agentic worker against mock → emit-edits → `apply_emitted_edits`
mutates the sandbox file → result file → `mini_coder_result` → activity events.
Assert at EVERY hop the durable channel, not the internal: fleet row status
transitions (queued→running→done), `mini_activity_snapshot` timeline entries
(spawn/coder/banner kinds), bridge file exists and replays (restart-durability:
re-read snapshot after killing nothing — hydrate-on-miss path), result content.
Negative cells: cloud directive rejected loudly (already covered in Layer B — link,
don't duplicate); malformed emit-edits → allowlist/cap violations surface as errors
not silent skips.

### P5b — Kanban + arrows: task lifecycle choreography (python rig)
MCP choreography from a forged coder session: `project_next_task` → `project_claim_task`
(task → wip; **known bug: returns full 110KB `public_agents_state` — FIX IN THIS PHASE**,
compact ack like register/heartbeat, then the scenario asserts ack <4096B, same guard
class as round 5) → `project_update_status` (wip→done) → `project_create_followup`
(new task with `deps` on the finished one). Assert the PROJECT STATE the UI renders:
tasks array statuses (kanban columns), `deps` edges present (what TaskDependencyArrows
draws), `project_append_note` lands. Negative: claim on paused project (covered — link),
double-claim same task, update_status on a task the session doesn't own.

### P5c — Pi coder lane: role parity + console durability (python rig + product fix)
Spawn the sidecar with `DEVBOULE_AGENT_ROLE=main-coder` and `mini-coder` (the rig
driver already does roles): assert role-scoped tool surface (plan tool ABSENT — covered;
censor hook ACTIVE on .rs writes for coder roles), turn round-trip, and the TWO known
product gaps in this lane, each as a scenario that pins CURRENT behavior first:
1. **Bridge file for pi coders** (backlog §3): pi coder sessions never write
   `.devboule-activity/<id>.jsonl` → console lost on restart. FIX (write-through on
   the Rust event arms, same writer the directive minis use), then flip the scenario
   from pin-the-gap to assert-durability.
2. **Steer no-op** (backlog §3): `mini_coder_steer` targets directive rows only → pi
   coder steer goes nowhere. Scenario asserts the current explicit error/no-op; the
   routing fix is a DESIGN decision (route by session kind) — pin now, fix only with
   owner go.

### P5d — Blind-spot write-throughs (small product fixes, Rust + rig cells)
The inventory §3 event-only channels, each a small write-through + one rig cell:
`mini://stuck` → persist a `lastStuckReport` readable via snapshot;
`censor://scan-started` + `censor://mini-findings` summary → durable censor state;
`agent-terminal://<id>` ring exposure (read command). Each makes a today-invisible
flow assertable; do them one at a time, cheapest first.

### P5e — UI-model unit coverage from rig fixtures (vitest)
Record REAL snapshots produced by P5a/P5b runs (activity snapshot JSON, tasks state,
fleet rows) as fixtures; vitest the frontend models against them: `agentConsoleModel`
apply/tail semantics, `agentRowModel` row derivation (status/kind/attention),
`projectWorkspaceModel` kanban columns, task-deps → arrows input. This is the same
pattern as plannerModel: the rig proves the backend emits it, vitest proves the UI
model consumes it — the seam in between is one recorded fixture, versioned in rig/.

### P5 execution order
P5b first (pure MCP choreography, fixes the 110KB claim bug — highest value/cost),
then P5a (executor simulation), P5c (pi lane + bridge fix), P5d one-by-one, P5e last
(needs the fixtures the others produce). Standing gate applies: every phase lands with
its rig cells green in `npm run rig`.

## What the rig can NOT cover (stays owner e2e, but the list is now short)
- Real GUI rendering/interaction (React state, focus, banners visually).
- macOS Keychain ACL prompts, App Nap behavior.
- Packaged-build resource paths (node_modules bundling — the open Phase-5 question).
- True `npm run tauri dev` process environment (though the round-3 cwd class is
  coverable by running rig cases from `src-tauri/` as cwd).

## Execution rules (house standard)
- Recon = deepseek-v4-flash via pi (findings verified on disk before use).
- Coding = current roster (hy3:free / nemotron free → paid hy3 / Kat coder air →
  mimo via opencode-go / minimax-m3-clean / deepseek), thinking high, coders never
  run cargo, git-mutation ban preamble, commit per phase, test-count check after
  every dispatch.
- Per-step review = deepseek-v4-pro.
- Claude orchestrates, verifies (cargo/vitest/pytest), runs the rig.
