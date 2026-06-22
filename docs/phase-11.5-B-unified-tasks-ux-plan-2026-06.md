# Phase 11.5 "Option B" — unified task store + plan/Console UX (2026-06-17)

> Status: APPROVED by owner, IN PROGRESS. UNTRACKED (docs/), do NOT commit. GPU-free
> throughout. Cadence: each piece → implement (veteran-coder) → verify on disk → 1 hostile
> reviewer → fix → next; whole-diff max-recall at the very end. Owner drives the UX; show
> mockups/decisions before coding (done).

## The model (owner-confirmed)
ONE pipeline, identical local & non-local: **CODER (writes and/or delegates to MINI) →
Censor → Kanban + human gate.** Non-local (Claude/codex): orchestrator = coder = one agent,
drives itself. Local: `devboule-coder` plans, and a DETERMINISTIC **runner** (Rust, no LLM)
executes the plan by delegating each task to the **mini** (an LLM). What differs local vs
non-local is ONLY the UX granularity + the human actions — NOT the architecture, NOT Censor.

**Decision: ONE shared task list = the existing PROJECT Kanban**, used by BOTH the local
runner (auto-drives plan tasks) and Claude/human (manual). The local plan's tasks live ON
that board; what's "local-only" is the auto-runner. `.devboule/tasks.json` is DELETED — the
Kanban is the single source of truth.

## The 4 pieces (order 1 → 4 → 2 → 3)

### Piece 1 — Unify the task store (foundation). Split into 1a (schema/Python) + 1b (devboule rework).
The local plan's tasks are created IN the Kanban; the planner + runner use the Kanban, not
tasks.json. No new UI yet.

### Piece 4 — Real diffs in the Console (self-contained, high visual value).
Compute the unified diff when the mini applies edits; feed it to the Console (renderer is
already done — `AgentConsole.tsx` `DiffBlock`, just fed empty today).

### Piece 2 — Live plan view in Plans (the "map" mockup).
Plans tab shows the plan's tasks live (todo→wip→review→done/blocked) + dependencies.

### Piece 3 — Per-task action buttons (pause/skip/retry), wired to the runner.

## The 3 resolved decisions
- **(a) Who sets "done":** unchanged — `done` is verifier/human-only (same as Claude). The
  runner sets a finished task to **review**, NOT done. The human/verifier gate stays.
- **(b) Runner proceeds vs waits:** **tira dritto** — a dependency is "satisfied" when the
  predecessor is `review` OR `done` (not only `done`). The runner executes the whole plan;
  finished tasks land in `review`; the human reviews the batch + the push gate. (Not per-task
  wait.)
- **(c) Plan tasks vs manual tasks on the shared board:** plan tasks are TAGGED with the
  approved `planId`; the runner auto-executes ONLY those. Manual tasks (no planId) are
  untouched by the runner.

## Ground-truth integration map (verified via Explore 2026-06-17, file:line)

### Kanban store
- Tasks live in the project `.md` `\`\`\`aspis-project\`\`\`` JSON state block. Python
  parse/serialize/validate: `oracle/server/aspis_mcp.py` `find_state_block` (~1581),
  `write_project_file` (~1700), `validate_project_state` (~1667). Rust mirror:
  `src-tauri/src/backend/model.rs` `ProjectTask` (~172–194).
- Statuses: `VALID_TASK_STATUSES = {todo, wip, review, blocked, done}` (aspis_mcp.py ~85).
  Transitions role-gated (`validate_transition` ~2355): coder/orchestrator set
  todo/wip/review/blocked; **done = verifier-only** (+ evidence + confidence ≥0.70).
- Tools (aspis_mcp.py handlers ~6034–6326, registry ~6843): `project_get` (full state),
  `project_list`, `project_next_task`, `project_claim_task` (45-min lease in agents.json),
  `project_update_status`, `project_append_note`, `project_create_followup` (the ONLY task
  creator; always `todo`; auto-id via `next_task_id` ~1746).
- `ProjectTask` today has NO `depends_on`/`scope`/`acceptance`/`plan_id`.

### devboule-coder (to rework)
- `planner.rs` `persist_tasks_json` (~778) writes `.devboule/tasks.json`; `run_planner`
  calls it after plan_submit. `plan_submit` returns `{planId, status}` (only status is parsed
  today in `parse_submit_status`).
- `runner.rs` `load_tasks_json` (~814) reads tasks.json; `run_tasks` (~126) loops. The Task
  struct ALREADY has `depends_on`; status vocab pending/running/done/blocked.

### Console diff
- `AgentConsole.tsx` `DiffBlock` (~123–161) renders `DiffLine[]` — COMPLETE, fed empty.
- `mini_activity.rs`: `Action.diff: Vec<DiffLine>` (~101); emitted `Vec::new()` in
  `push_write_action` (~554). `useAgentConsole.ts` expects `mini_activity_snapshot` cmd +
  `mini-activity://<id>` channel (VERIFY wiring — memory says the milestone bridge is live;
  confirm the rich channel during piece 4).
- `mini_coder_executor.rs` `apply_emitted_edits` (~2270): old content read ~2341, new content
  after `replacen` ~2359 — BOTH in scope → compute unified diff here, thread to push_write_action.

### Plans frontend
- `PlansPanel.tsx` shows plan-approval history only (no live task board) — calls
  `get_plan_markdown`. `PlanApprovalCard.tsx` polls `plan_approval_requests_list`, calls
  `approve/reject_plan_request`. `TaskCard.tsx` has Move/Launch buttons, NO pause/skip/retry.
  `taskBoard.ts` = column types only. Need NEW Tauri commands for live plan tasks + actions.

## Piece 1 detail
### 1a — schema + Python (foundation)
- `model.rs ProjectTask`: add `#[serde(default)] depends_on: Vec<String>`, `scope: Vec<String>`,
  `acceptance: String` (or Option), `plan_id: Option<String>`. Backward-compatible.
- `aspis_mcp.py validate_project_state`: accept the new fields; when `depends_on` present,
  validate it references existing task ids + is ACYCLIC (reuse/port the Kahn check).
- NEW MCP tool `project_create_plan_tasks(project_id, plan_id, tasks[])`: atomically create all
  plan tasks as `todo` tagged with `plan_id`, allocating FRESH `T<n>` ids and REMAPPING
  `depends_on` from the plan's internal ids → allocated ids; validate the DAG; return the
  created tasks (or id map). Add to the orchestrator role allowedTools.
- pytest: schema round-trip, dependsOn validation/acyclicity, bulk-create + id remap.

### 1b — devboule-coder rework
- `planner.rs`: on plan approval, instead of `persist_tasks_json`, call
  `project_create_plan_tasks` (extract `planId` from plan_submit). DELETE `persist_tasks_json`
  + `load_tasks_json` + TASKS_DIR/FILE.
- `runner.rs`: `run_tasks` reads the active plan's tasks via `project_get` (filter by the
  latest planId with incomplete tasks); status vocab → Kanban (todo/wip/review/blocked/done);
  pick a `todo` task whose deps are `review|done` → `project_update_status` wip → delegate to
  mini → on mini `done` set **review**, else **blocked**; stop on first block. Update
  runner tests for the Kanban vocab + a mock Kanban backend.
- DELETE `.devboule/tasks.json` path entirely.

## Open follow-ups (later, not now)
- Whether Claude should also get rich diffs (needs Censor-watch→Console normalization) — v2.
- Converging the claim/lease model with the runner's wip.
