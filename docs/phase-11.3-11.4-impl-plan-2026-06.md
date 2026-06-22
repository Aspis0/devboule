# Phase 11.3 + 11.4 — implementation plan (2026-06-17, autonomous)

> Status: DONE — code-complete, `cargo test` GREEN (196/0), 3 hostile reviews done+fixed,
> NOT committed (owner reviews/commits). ⚠️ Pre-existing `CALL_TOOL_TIMEOUT 120s < 1800s`
> flag must be resolved before any LIVE runner test (see end-of-session notes / memory).
> Autonomous night session. Owner directive: "fai piano per 11.3 e
> 11.4 e implementa. mai GPU. per ux 11.5 servo io quindi stoppa prima." → implement 11.3 +
> 11.4, **never touch the GPU** (another session is training), and **STOP before 11.5** (the
> UX work, owner wants to drive it). This doc is UNTRACKED (docs/), do NOT commit.

## Ground truth established (verified on disk 2026-06-17)

- The `devboule-coder` crate is at repo root `/devboule-coder/` (binary crate, NOT a
  src-tauri member). Modules: action, activity, agent_loop, app, config, conversation,
  executor, model, model_client, planner, prompt, rmcp_backend, terminal.
- **11.2 as-built DIVERGES from the 11.3 design doc.** The doc
  (`local-main-coder-harness-design`, §11.3) said "add `dependsOn` to the Kanban schema in
  `aspis_mcp.py`, no tasks.json". The ACTUAL 11.2 implementation persists
  `<project_root>/.devboule/tasks.json` (`planner.rs:753-790`) and its code comments
  repeatedly say "so Phase 11.3's DAG runner can consume it". The `Task` struct
  (`planner.rs:150-172`) ALREADY has `depends_on`, and the plan is ALREADY validated for an
  acyclic DAG (`detect_cycle`, Kahn's algorithm, `planner.rs:676-724`), unique ids, scope
  caps, etc. **Decision: follow the as-built reality — 11.3 is a tasks.json-based Rust
  runner, NOT a Python Kanban change.** Lower risk, reuses tested code, zero Python changes.
- The orchestrator role in `aspis_mcp.py` (`allowedTools`, line 346-365) ALREADY allows
  every tool the planner + runner need: `oracle_context`, `project_structure`,
  `spawn_mini_coder`, `plan_submit`, `plan_status`, `ask_user`, `project_create_followup`,
  `project_update_status`, etc. → **NO Python changes for 11.3.**
- `spawn_mini_coder` return contract (`aspis_mcp.py:4773-5011`): returns JSON text
  `{"directiveId": "...", "result": {"status": "<s>", ...}}`. The Rust `mcp.call_tool`
  returns this as a String (the planner already parses `plan_submit`'s `{status}` the same
  way). Terminal mini statuses (`mini_coder.rs:185-208`): `done | needs_clarification |
  aborted_by_human | failed | timeout | escalated`. **Runner semantics: `done` → task
  succeeded; ANYTHING ELSE → task BLOCKED** (conservative, mirrors planner's `from_status`).
- The burst loop (`agent_loop.rs:run_burst`) dispatches actions via the `ToolExecutor`
  seam, returns `BurstOutcome::{Done,AskUser,Escalated}`. `Plan { steps }` is dispatched
  like a tool → `RealExecutor::run_plan` → `run_planner` → persists tasks.json + plan_submit
  human gate. **There is NO run/execute-plan action yet** — the PLAN_FIRST prompt even says
  "once the plan is approved, proceed to implement it" but nothing implements it. 11.3 closes
  exactly that loop.
- `RealExecutor` (`executor.rs:543-606`) holds `mcp`, `fs`, `web`, `project_root`, `model`,
  `project_id`, `activity`. The runner needs only `mcp` + `project_root` + `activity` (it is
  DETERMINISTIC — no model). All already present.

## 11.3 — `dependsOn` DAG field + linear runner (Rust only, devboule-coder)

### New: `src/runner.rs`
A deterministic linear DAG runner. Public entry:
`pub async fn run_tasks(mcp: &dyn McpBackend, project_root: &Path, activity: &Activity) -> RunReport`.
- Load `<project_root>/.devboule/tasks.json` → `TasksPlan`. Missing file → clear error
  (`"no plan to run; run `plan` first"`).
- **Structural re-validation on load** (a hand-edited tasks.json must still be acyclic /
  well-formed). REUSE the DAG/id/scope checks but ACCEPT runtime statuses (not just
  "pending"). Refactor `planner::validate_plan` → split out `validate_plan_structure(plan)`
  (ids unique, scope cap, safe rel paths, dependsOn references-exist + acyclic) used by
  BOTH; the planner adds the plan-time-only checks (`status=="pending"`, `attempts==0`) on
  top. The runner calls only `validate_plan_structure` on reload.
- **Linear DAG loop:** repeat
  - find the FIRST task with `status == "pending"` (or a re-attemptable interrupted
    `"running"`) whose every `depends_on` task is `status == "done"`;
  - if none and all tasks `done` → success;
  - if none but pending tasks remain → STALL = remaining tasks depend on a non-done
    (blocked) predecessor → return the block (acyclicity rules out a circular wait);
  - else: increment `attempts`; if `attempts > MAX_TASK_ATTEMPTS` → blocked; set `running`,
    persist; milestone "running T00x: <title>"; delegate to `spawn_mini_coder` via
    `mcp.call_tool` with params `{task: "<title>\n\nAcceptance: <acceptance>\n\nRead-only
    context: <context_files>", files: <scope>, write: true, allow_oracle: true}` (the
    backend injects role/agent_id/session_token, as for the existing spawn_mini path);
  - parse `result.status`: `done` → set `done`, persist, continue; else → set `blocked`,
    persist, STOP, return blocked report (task id + title + status + reason).
- **Persistence:** REUSE `planner::persist_tasks_json` (expose `pub(crate)`) — atomic
  temp+rename — after every status change so a crash/restart resumes from disk.
- **Status vocabulary:** `pending | running | done | blocked`. `running` on reload = an
  interrupted task → re-attemptable (counts an attempt).
- `RunReport { completed: usize, total: usize, blocked: Option<{id,title,status,reason}> }`
  with a `compact_summary()` like the planner's (e.g. "Ran 3/5 tasks; BLOCKED on T004
  '<title>' (mini escalated). Ask the user how to proceed." or "Ran 5/5 tasks; all done.").

### `src/action.rs`
Add `AgentAction::RunPlan` (no fields). Wire tool name `run_plan`; `target()` → ""; not
egress; `validate()` → Ok. Round-trip test. (No-progress guard: target "" means two
`run_plan` in ONE burst trips the guard — but the flow is run_plan → done|ask_user which
ENDS the burst, so it is single-call-per-burst; a re-run happens in a fresh burst with a
fresh window. Documented; acceptable.)

### `src/executor.rs`
- `AgentAction::RunPlan => self.run_tasks().await` arm; `async fn run_tasks(&self)` calls
  `runner::run_tasks(self.mcp.as_ref(), &self.project_root, &self.activity)` and maps the
  `RunReport` → `ToolResult` (ok summary if all done; err summary naming the blocked task +
  "ask the user" if blocked; err if no tasks.json).
- `StubExecutor` (agent_loop.rs) + the agent_loop test enum match: add the `RunPlan` arm
  (returns `STUB_NOT_CONNECTED`).

### `src/prompt.rs`
Add `{"tool": "run_plan"}` to the PLAN / DELEGATE catalog with: "After a plan is APPROVED,
emit `run_plan` to execute the approved tasks in dependency order; each task is delegated to
a mini under Censor + the retry/escalate gate. If `run_plan` reports a task BLOCKED, use
`ask_user` (do not silently retry)." Update the "once approved, proceed to implement it"
line to name `run_plan`. Add a test asserting `run_plan` is documented.

### Tests (cargo test, GPU-free)
runner.rs: linear chain all-done; a mini escalates → stop+blocked names the task; resume
(T001 done, T002 pending → runs T002+); diamond DAG; reload rejects a cyclic/edited
tasks.json; attempts cap; missing tasks.json → clear error. action.rs: run_plan round-trip.
executor.rs: RunPlan → runner; no tasks.json → clear error; one-task plan → done. prompt.rs:
run_plan documented.

## 11.4 — watchdog refinement (orchestrator, safe scope)

### `src/agent_loop.rs` — output-hash loop-detector
In `run_burst`, keep a sliding window of hashes (std `DefaultHasher`, no new dep) of the RAW
model output per round. A repeat within the window → `Escalated("no progress: repeated
identical model output")`. Complements the existing (tool,target) executed-window guard
(which only catches repeated DISPATCHED actions) and the 3-consecutive-format-errors guard
(only consecutive). Catches a model emitting identical output non-consecutively / output
that never dispatches. Tests: same raw output twice → escalate; distinct outputs → unaffected.

### Deferred (documented, NOT done autonomously — needs owner review)
- "Per-role token budgeting in `build_mini_prompt`" lives in src-tauri
  `mini_coder_executor.rs` — the LIVE mini path (GPU-deferred behavior). Changing it at night
  risks the deferred live verification; defer for owner review. The orchestrator's
  `build_messages` ALREADY budgets (oldest-first eviction, `MAX_TRANSCRIPT_CHARS`, keeps
  system+human+newest) — no change needed there.
- "context-overflow auto-split" — fuzzy; the existing eviction already bounds the prompt.

## Cadence (per global CLAUDE.md)
implement (veteran-coder) → verify on disk → `cargo test` (GPU-free) → 1 hostile reviewer →
fix → next step. At the END (whole 11.3+11.4 diff) → max-recall 3 reviewers + adversarial.
NEVER run the app / oMLX / any GPU work.
