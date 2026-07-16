# Future work backlog — compiled 2026-07-15

Living document: everything known-open across the project, gathered from the
self-test-rig sessions, the orchestrator-chat fix rounds, and older
workstreams. Grouped by area; within each area, roughly most-urgent first.
(Owner = the owner decides priority; nothing here is committed work.)

## 1. Self-test rig (docs/backend-selftest-rig-plan-2026-07.md)

- ~~**P4 — standing gate**~~ **DONE 2026-07-15**: `npm run rig:smoke` /
  `rig:rust` / `rig` in package.json + "Standing gate" section in
  `rig/README.md` (rig before owner e2e; live bug → rig scenario first).
- ~~**devboule_websearch/plan channel fix**~~ **DONE 2026-07-15** (`bd4a9fc`):
  first-class stdout events devboule_websearch/devboule_plan + Rust arms +
  live test.
- **Layer B extensions** (Rust `#[ignore]` tests): goal-delivery through the
  real Rust spawn path (initial_goal_msg_id echo ordering); the full
  devboule_plan → ConsoleEntry chain; censor-after-write with a real cheap
  linter in a fixture project; executor `run_pass` loop test with a fake
  backend CLI on PATH (Cloud CLI cell).
- **Placement matrix completion**: Cloud CLI cells (fake codex/claude wrapper
  script, executor path); per-role `config.json` fixtures driven through the
  REAL `resolve_coder_env_for_sidecar` (needs Layer B / AppHandle strategy).
- **Event-only blind spots → write-throughs** (inventory §3): `mini://stuck`
  (store a lastStuckReport), `sandbox://consent-request` on the cloud-duplex
  branch (write to consentRequests), `censor://scan-started`,
  `censor://mini-findings` summary, `agent-terminal://` ring exposure.
  Each small; makes the corresponding flow rig-testable.
- Rig hygiene: mock non-stream text branch is untested (SDK always streams);
  m5 SIGKILL+2s fallback accepted as-is; censor planted-finding via REAL
  linter run (today only the MCP read/dispose path is covered).

## 2. Product bugs / tickets found by the rig (not yet fixed)

- ~~**`plan` tool does not exist**~~ **DONE 2026-07-15** (`b91cb78`): registered
  via `createAgentSession customTools` in sidecar.mjs (no extension needed),
  orchestrator-only; plain-string status schema (weak providers reject
  anyOf/const), normalized in execute. Rig scenario un-skipped + negative
  coder-role cell; 6 node unit tests. UI DONE too (`42f8818`): Plan stage
  renders the pi plan (title/steps/notes, skipped=line-through) when no
  project tasks exist yet; raw JSON never reaches chat bubbles (planner +
  AgentConsole filters); drawer auto-expands on first plan. Live e2e with
  a real model still owed.
- ~~**`project_claim_task` returns the full `public_agents_state`**~~ **DONE
  2026-07-15** (`6299019`, rig P5b): compact_session_ack + claim key
  {projectId,taskId,status,leaseUntil}; ack<4096B guarded in the rig; zero
  breaking consumers; 5 old-contract unit tests adapted.
- **Project status `draft` rejected** by project_get/oracle_context — decide
  whether draft should be readable (guards at aspis_mcp.py:8253/2935).
- `agent_heartbeat` does not echo sessionToken (by design — document it in the
  agent prompt/help so models don't retry).
- `censor_findings` requires `root_path` frontmatter + ASPIS_WORKSPACE_ROOT
  approval — confusing failure mode when missing; better error message.
- **P5d write-throughs DONE 2026-07-16**: `mini://stuck` → durable
  `stuckReport` on the directive row (`c69ce27`); `censor://scan-started` →
  CensorScanRegistry + `censor_scan_state` command, `censor://mini-findings`
  summary → `censorSummary` on the directive row (`88db1ae`);
  `agent-terminal://<id>` was NOT a blind spot (agent_pty_snapshot exists;
  ring memory-only by privacy design — inventory corrected). Still event-only:
  `sandbox://consent-request`, `design-stream:<id>` deltas.
- **UI gap (P5e finding, pinned by it.todo)**: drawerData/AgentsView has no
  visibility into the new directive-level `stuckReport`/`censorSummary` —
  surfaced only via the live `mini://stuck` event hook; a durable-state read
  in the drawer would make them visible after restart.
- **Deterministic censor gate on the session diff (salvage pi-lens diagnostics,
  NO autofix)** — owner design ticket 2026-07-16. Background: `npm:pi-lens` was
  the deterministic-censor-before-LLM experiment, but its autofix ran
  `cargo clippy --fix`/`cargo fix --allow-dirty --allow-staged` TREE-WIDE as a
  side effect of any agent .rs edit — invisible, unscoped, attributed to the
  coder models (the "mimo/kat cargo incidents" were pi-lens), and it would
  bypass the agentic executor's write_allowlist entirely. Owner removed the
  package 2026-07-16. What to build instead, inside Devboule's censor rail:
  * **Detection, report-only**: run the deterministic checks (clippy WITHOUT
    --fix, typos, ast-grep/opengrep patterns) on the DIFF of the coder's
    session — only files the coder touched — and feed findings back into the
    fix-pass loop exactly like tool `ERROR:` feedback. Attach on the (now
    fixed, 8aa592a/c6aa4e4) `devboule_censor_review` channel for pi sessions
    and on the emit-edits result for directive minis.
  * **Autofix never in the live tree**: at most rustfmt on the edited files at
    an explicit turn boundary; anything clippy-fix-shaped runs in an isolated
    worktree and comes back as a PATCH PROPOSAL the loop can accept — never
    --allow-dirty on the shared tree.
  * Related: [censor-gate-expansion-and-skills] linter list; the 3-tier local
    review design (censor-and-projects-ia-redesign).

## 3. Orchestrator / pi-sidecar (from the 6 fix rounds, still open)

- **Pi coder/mini rows visible but NOT steerable from UI** — `mini_coder_steer`
  targets the directive executor only; pi sidecar sessions need a steer route
  (design decision: route by session kind).
- **User-echo + goal delivery for coder/mini spawns** (orchestrator got it in
  round 1; coder path still lacks it).
- ~~**Bridge file for pi coder console**~~ **DONE 2026-07-16 (`e1cd083`, P5c)**:
  EventMapper write-through to `.devboule-activity/<id>.jsonl` + hydrate at
  construction; plans/chat/thinking/websearch/milestones replay after restart.
- **Bridge file for DIRECTIVE minis too** (found in P5a recon 2026-07-15): the
  directive executor's activity lives only in the in-memory MiniActivityStore
  (Tauri events); only the ORCHESTRATOR writes `.devboule-activity/<id>.jsonl`
  (projects.rs:~1974). Mini console is lost on restart — same fix class as the
  pi-coder ticket above (one write-through in the executor's update path).
- **node_modules in the packaged bundle** (Phase 5 question): debug `_up_`
  resource copy has no node_modules; packaged builds can't spawn the sidecar.
- **Sandbox projects_dir = tree-wide cross-project write** — scoping requires
  an aspis_mcp redesign (documented trade-off, owner ticket).
- Review leftovers from round 6: M6 (advanced LocalCoderBackendCard duplicates
  the key surface), mini cloud consent is a note only, m6/m10 shape-duplication
  seams.
- Verifier pi path: `read_verifier_backend` is forward-wired but the verifier
  role has no real pi runway yet.

## 4. Big deferred refactors (owner go required — from the 2026-07-11 audit)

- `censor/extract.rs` ~4k dark lines: per-language split.
- Agentic stack ~2.7k (agentic_worker/runner/transport): structure + tests.
- Double censor pipeline (fine batch vs pigeon review) unification.
- Megafile splits: projects.rs (~11k), pi_sidecar.rs (~5k+), scanner.rs (~12k).
- Oracle M3: delete the Python oracle server path (oracle-rs is live) —
  PLAN.md exists; needs the owner's go. Windows DirectML untested;
  candle-Metal exists but is not wired.

## 5. Platform / packaging

- Windows: B2 (audit batch) never compiled on Mac — needs a Windows check
  pass; DirectML (above); PowerShell launch scripts have tests but no live
  Windows e2e recently.
- macOS: Keychain ACL prompt on first vault access from a new binary (known,
  needs the owner to click); App Nap mitigation was best-effort (Info.plist).
- Packaged-exe launch requires cwd = project root (config.json resolution) —
  revisit with the Phase-5 bundling work.

## 6. Owner live-e2e still owed (features shipped, never verified in the real app)

- Roles settings 3-way placement UI (round 6) — **now that Bearer-dummy is
  fixed (56c16bd), the Cloud API cell can actually work**; re-run the checklist:
  Impostazioni→Roles → orchestrator su Cloud API o Local omlx → "ciao" nel
  planner → echo + ack piccolo + risposta.
- Websearch console entries in the real app (after the devboule_websearch fix
  lands): run a websearch from the planner and see the console entry.
- Polis: full e2e in the packaged exe (visual work approved in the harness).
- Pigeon unification, mini-coder loop, GitHub-in-AM, oMLX integration, design
  module (macOS capture flagged off) — all shipped with e2e pending.

## 7. Older parked workstreams (pointers, no action until owner reprioritizes)

- Devboule bench/prodbench: Slice 3 UI, Part B DEEP, harder prodbench tasks.
- ORPO/compression pipeline + SkillOpt text-space optimizer (parked).
- Censor: local fast semantic reviewer doesn't exist on 64GB Mac — the plan is
  deterministic gate + cloud on diff + future fine-tuned small model.
- Master plan (docs/master-plan-2026-06-self-improving-mini-design.md):
  P10(b) skills UX, P12 default-don't, remaining phases.
- Kanban bug #20 (deferred with plan).

## 8. Process debts

- Push the 11+ unpushed commits on phase1/infra (owner pushes; rig commits
  94d45bf..170a452 + the 5 pre-existing + today's P3c/P3d when committed).
- Coder roster hygiene: nemotron free = false reports (demand grep evidence);
  minimax = deadlock/hang history; mimo = 2026-07-15 tree-wide cargo-fix
  incident (always `git status` the whole tree after its dispatches); kat =
  reliable but upstream-rate-limited at times (retry with backoff).
- The `error: 1` event observed in the websearch probe stream — identify its
  source (likely extension noise) and whether it warrants a banner.
