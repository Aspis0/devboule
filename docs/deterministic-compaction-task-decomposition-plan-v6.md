# Plan v6 — Mini-Task Spoon-Feeding for Local Coders

**Date:** 2026-07-01
**Status:** Draft — supersedes `deterministic-compaction-task-decomposition-plan-v5.md`
**Verified against:** `censor-verifier-flow` HEAD `470e070` + the uncommitted working tree
(the v5 files `compact.rs` / `task_size.rs` / `task_decompose.rs` are on disk, untracked).

> **Why a v6.** v5 tried to build a giant mini-prompt and then cut it down with BM25
> *string-surgery* over marker text. A hostile review found that approach is fragile by
> construction (3 confirmed blockers, all rooted in re-parsing a flattened string). Meanwhile
> a study of `pi-code-planner` + `pi-soly` (both MIT) and first-hand testing showed the real
> lever: **keep each task small from the start and spoon-feed the local model**, not "compact
> a big prompt after the fact." v6 keeps the good half of v5 (the `context_window` field, the
> BM25 core as a *section-level* safety net) and replaces the fragile half with a mini-task
> execution model that devboule mostly **already has** but half-wired.

---

## 0. Philosophy (locked)

1. **Simplify, don't add features.** Make the existing pipeline actually work end-to-end.
   Every phase below either *finishes wiring* something already written, *fixes a real bug*,
   or *reshapes* context handling. New infra only where devboule has nothing at all.
2. **Local models are context-bound.** Verified first-hand: local models (even the 35B MoE,
   especially non-thinking) depend heavily on a small context window and are bad at multi-file
   tasks. The answer is atomic tasks + a bounded per-task context bundle, not a bigger prompt.
3. **Reuse method, not dependencies.** We copy the *design* of `pi-soly` (bounded per-task
   bundle, runnable acceptance) and `pi-code-planner` (state-as-truth, test-first gating,
   stuck-report, branch-per-task), and the *type vocabulary* of `ambush`. **Zero new external
   crates.** (See §3 for why each dependency was rejected.)

---

## 1. What devboule ALREADY has (the spine — extend, don't reinvent)

There is **no separate orchestrator service**: the orchestrator *is* `devboule-coder` running
in the `orchestrator` MCP role — a read/plan/dispatch loop with **no write action at all**
(`devboule-coder/src/action.rs:81-163`, the `AgentAction` enum has no `Write`/`Edit`). All file
writes happen inside the MINI coder.

| Component | Where | State |
|---|---|---|
| Planner (STRUCTURE→EXPLORE→PLAN) | `devboule-coder/src/planner.rs` | emits `TasksPlan{tasks:[{id,title,scope,acceptance,depends_on,…}]}`; human-gated `plan_submit`; caps `MAX_TASK_SCOPE=3`, `MAX_TASKS=40`, `MAX_PLAN_PROMPT_CHARS=24_000` |
| Concurrent DAG runner | `devboule-coder/src/runner.rs` | deterministic, no-LLM; `MAX_PARALLEL_TASKS=2`; every task → a MINI; runner sets `review`, never `done` (verifier-only invariant) |
| Kanban task | `src-tauri/src/backend/model.rs:236-284` (`ProjectTask`) | persisted on disk in `ProjectStateBlock` (`model.rs:310-327`), Oracle-indexed; lifecycle `todo→wip→review→done`(verifier-only)`/blocked` |
| Mini spawn prompt | `mini_coder_executor.rs:3898-4104` (`build_mini_prompt`) | assembles context-project → SKILL → persona → FILE SCOPE (`MAX_PROMPT_FILES=20`, `FUZZY_MAX_FILE_BYTES=256KB`) → HARD CONSTRAINTS → RESULT CONTRACT → TASK |
| Mini directive/outcome | `mini_coder.rs:435-547` / `:243-277` | `MiniCoderDirective` + `MiniCoderOutcome{output,files_touched,edits,question,partial,error,censor_findings}` |
| Watchdog | `mini_coder.rs:57-104` + `agent_loop.rs:49-55,432-445` | launch/wall-clock timeouts + no-progress + output-hash loop-detector. **Solid.** |

**Bottom line:** the mini-task machine exists. v6 finishes and de-bugs it.

---

## 2. The gap table (v6 piece → status today)

| # | Piece | Status | Evidence |
|---|---|---|---|
| a | Per-task **bounded context bundle** | Partial | `compact.rs::compact_built_prompt` wired only into the **one-shot** mini (`mini_coder_executor.rs:3621-3644`); agentic path + devboule-coder side missing |
| b | **Runnable** acceptance criteria | **Missing** | `ProjectTask.acceptance` is free text, appended to the task (`runner.rs:722-727`); **never executed** anywhere |
| c | Task **size estimate** / **decompose** | Estimate wired (one-shot); **decompose = dead** | `task_size::estimate_task_size` hooked at `mini_coder_executor.rs:3595-3612`; `task_decompose::decompose_by_files` has **0 callers** |
| d | **TDD-strict** dispatch | Written, tested, **dead** | `tdd_strict.rs` (`assert_test_untouched`/`detect_test_gaming`/`evaluate_gate`) has **0 callers**; no `tdd_test_path` on the directive |
| e | Checkpoint / clarification | Partial + **BUG** | `parse_mini_status` (`runner.rs:741-769`) reads `result.error` (empty on clean `needs_clarification`) instead of `result.question` → **the mini's question is dropped** |
| f | Structured **SUMMARY** feeding next task | **Missing** | `build_spawn_params` sends only `title+acceptance`; `set_review` writes a **fixed constant** `EVIDENCE_REVIEW` (`runner.rs:70`), not the real outcome |
| g | State survives restart/compaction | Split | Kanban + directive queue persist; the **orchestrator's in-memory `conversation`** (`main.rs:211-264`) does not — a restart loses burst context |
| h | Idle/stall watchdog | **Exists** | timeouts + loop-detector. Only missing the *structured stuck-report* |
| i | Git **branch-per-task** isolation | **Missing** | no branch-per-task anywhere; 2 parallel minis share the checkout |
| j | Deterministic **compaction** | Partial | `compact.rs` (BM25) one-shot only; devboule-coder uses the crude `trim_conversation` head/tail (`main.rs:159-178`) |

**Already fixed** (category-A regressions from the solo refactor, this branch, compiling clean):
model-registry `null contextWindow` save, `conversation_budget_chars` 5× shrink (now floored at
48_000), BM25 transcript value-dedup, duplicate `status_token` match arms. **A3** (synchronous
Censor wait stalling the single scheduler thread, `mini_coder_executor.rs:1869`) is folded into
**Phase 4** here, because that path is being reshaped anyway.

---

## 3. Reuse sources — what we take, what we reject

| Source | License | Take | Reject-as-dep because |
|---|---|---|---|
| **pi-soly** (`workflows-data/*.md`) | MIT | Template *skeleton* (`<purpose>`/`<read_first>`/acceptance/`<atomic_close_out>`/`<hard_rules>`), per-section budget numbers, "acceptance-runnable = sizing gate", SUMMARY-feeds-next | TS pi-runtime extension — not a Rust lib |
| **pi-code-planner** (`m62624/pi-code-planner`) | MIT | *Method*: state-as-truth read each turn, test-first **structural** gating, **stuck-report** (5-axis rubric + git-diff), **branch/worktree-per-task** with rollback | TS pi extension; tool-gating/`sendUserMessage`/elenchus-WASM are pi-specific |
| **ambush** (`crates.io`) | MIT | *Type vocabulary only* (`Simple/Moderate/Complex`, granularity) — rewritten by us, multi-language | repo `moltenlabs/molten` = **404 (dead)**; pulls sibling `warhorn`; LLM-decomposition cuts against "deterministic" |
| **rs-graph-llm** (`a-agmon`) | MIT | Nothing (reference only) | devboule **already has** a DAG runner (Phase 11) — adding it = replacing working infra |
| **pi-taskflow** (`heggria`) | MIT | Vocabulary only (gate/approval/loop) | full declarative-DAG engine = over-engineering vs "simplify" |

**We deliberately DON'T build:** pi's ~30-step state machine, dual-gate per-step tool
visibility, the elenchus SAT consistency check. Different product.

---

## 4. The phases (minimal-first; each verifiable)

> Implementation rule (per project HARD RULE): **the local coder writes the Rust; I orchestrate +
> verify + hostile-review.** Delegate via `pi -p --provider omlx --model Qwopus3.6-35B-A3B-Coder-4bit
> --thinking off -a "<task>"` (the model reads real signatures + edits files itself). One reviewer
> per phase; a max-recall fan-out on the whole diff at the end.

### Phase 0 — Cheap bug fixes (unblock the loop)
**Goal:** stop losing information the pipeline already produces.
- **(e)** In `parse_mini_status` (`runner.rs:741-769`): on `needs_clarification`, read
  `result.question` (fall back to `error`) and route it into the existing `AgentAction::AskUser`
  surface instead of collapsing to a bare `Blocked`.
- **(f-1)** In `set_review` (`runner.rs:70`): write the real `MiniCoderOutcome.output` +
  `files_touched` as the review evidence, not the `EVIDENCE_REVIEW` constant.
**Acceptance (runnable):** a mini returning `status=needs_clarification` with a `question`
surfaces that exact question to the user (assert in a runner unit test); `set_review` evidence
contains the mini's real changed-file list (unit test).

### Phase 1 — Acceptance becomes a runnable gate (replaces v5 Fase C/D)
**Goal:** the sizing/quality gate is "can this task's acceptance be *checked by a command*,"
not a token estimate (v5) or a keyword heuristic (ambush).
- Extend `ProjectTask` (`model.rs:236-284`) with an optional structured
  `acceptance_checks: Vec<String>` (shell/test commands) alongside the existing free-text
  `acceptance`. The planner (`planner.rs` PLAN stage) is instructed to emit at least one
  runnable check per task; a task with **no** runnable check is flagged "not atomic" back to
  the human (this is the sizing gate — reuse pi-soly's rule verbatim in the planner prompt).
- After a mini reports `done`, the runner runs the `acceptance_checks` (reuse the existing
  sandboxed command runner used elsewhere; **do not** invent a new exec path) before allowing
  `review`. A failed check → `blocked` with the command output as evidence.
- **Retire** `task_size::estimate_task_size` as the gate and delete the dead
  `task_decompose::decompose_by_files` (0 callers) — the runnable-acceptance gate replaces both.
  Keep `estimate_tokens`/`truncate_to_token_budget`/`bm25_score` from `compact.rs` (Phase 3 uses
  them as a section-level safety net).
**Acceptance (runnable):** a task with a failing `acceptance_checks` command never reaches
`review` (runner integration test); a planner output with a no-check task is rejected.

### Phase 2 — Wire test-first gating (reuse `tdd_strict.rs`, already written+tested)
**Goal:** for `kind: code` tasks, enforce red→green structurally, the way pi-code-planner does
— but reusing devboule's existing, fully-unit-tested `tdd_strict.rs` (0 callers today).
- Add `tdd_test_path: Option<String>` to `MiniCoderDirective` (`mini_coder.rs:435-547`). When
  set, the mini receives the failing test as a **read-only** skill (anti-cheat is CODE, per the
  `tdd-strict-dispatch` design: `assert_test_untouched` + `detect_test_gaming`).
- Runner flow: run the test (must fail) → spawn mini → run the test again (`evaluate_gate`) →
  only green passes to acceptance-checks (Phase 1). Reuse the Phase-1 exec runner.
**Acceptance (runnable):** a mini that edits the test file is rejected by `assert_test_untouched`
(integration test); a mini whose change leaves the test red stays `blocked`.

### Phase 3 — Bounded per-task bundle (replaces v5 `compact_built_prompt` re-parsing)
**Goal:** never build a giant prompt and re-parse it. Assemble the mini prompt from **structured
segments** with per-section char caps (pi-soly method), and keep BM25 only to trim *within* the
file section.
- Change `build_mini_prompt` (`mini_coder_executor.rs:3898-4104`) to build from typed segments
  (system / task / files / oracle / prior-SUMMARY) and apply per-section caps at build time —
  **delete** the marker-string re-parsing in `compact_built_prompt` (source of v5's 3 blockers).
  BM25 (`bm25_score`) now ranks/trims files *inside* the file segment only.
- Wire the bundle into **both** mini paths (one-shot **and** `spawn_agentic_worker` at
  `mini_coder_executor.rs:1316+` — today only one-shot is covered).
- Feed the predecessor's SUMMARY (Phase 4) into the bundle as a capped section, so a task sees
  what its dependencies did without the whole history.
- Add a `devboule-coder`-side equivalent for the burst loop: replace the crude `trim_conversation`
  (`main.rs:159-178`) with the same segment-budget approach against `context_window`.
**Acceptance (runnable):** `build_mini_prompt` output for a scope that includes
`mini_coder_executor.rs` itself is never corrupted (the v5 marker-collision test now passes);
the assembled prompt is ≤ 70% of the model's `context_window` for both mini paths.

### Phase 4 — SUMMARY-as-truth + orchestrator resume + A3
**Goal:** externalize state so a task/session survives compaction and restart (pi-code-planner's
core lesson), using the **existing Kanban store**, not a parallel `.agents/` tree.
- On mini completion, persist a structured `TaskSummary{outcome, files_touched, validation,
  decisions_needing_approval}` onto the Kanban card (extend `ProjectStateBlock`). This is the
  durable per-task record; the next task's bundle reads it (Phase 3).
- Make the orchestrator rebuildable from disk: on restart, reconstruct burst context from the
  Kanban + last-N `TaskSummary`s instead of the lost in-memory `conversation` (`main.rs:211-264`).
- **A3 fix:** move the synchronous `wait_for_censor_findings` off the single `run_pass` thread
  (`mini_coder_executor.rs:1869`). Findings now attach to the `TaskSummary` asynchronously; the
  scheduler never blocks. Delete the dead `claim_verdict`/`release_verdict` infra or wire it —
  don't leave it half-dead.
**Acceptance (runnable):** kill + restart `devboule-coder` mid-plan → it resumes from the Kanban
without re-running completed tasks (integration test); a pass with 3 finished minis does not
stall the scheduler (timing assertion).

### Phase 5 — Isolation + stuck-report (robustness)
**Goal:** the two robustness wins from pi-code-planner that pay off for local models.
- **(i)** Branch-per-task: each mini runs on `task/<planId>/<taskId>` off a plan branch, merged
  on green with automatic rollback on conflict (port pi's wrapper+rollback discipline; reuse the
  existing `ensure_diff_baseline` git plumbing as the starting point). Isolates the 2 parallel
  minis.
- **(h+)** Stuck-report: when the watchdog trips (timeout/loop-detector already exist), emit a
  structured stuck record (5-axis rubric + `git diff` capture) and force a context reset with a
  "don't repeat the last attempt" instruction, instead of a blind retry.
**Acceptance (runnable):** two parallel minis touching overlapping files don't corrupt each
other's output (integration test); a mini that hits the same failure 3× produces a stuck-report
artifact rather than looping.

---

## 5. Summary

| Phase | Theme | Mostly… | Files |
|---|---|---|---|
| 0 | Cheap bug fixes (e,f) | fix | `runner.rs` |
| 1 | Runnable acceptance gate (b; retires C/D) | finish+delete | `model.rs`, `planner.rs`, `runner.rs`, `task_size.rs`(del gate), `task_decompose.rs`(del) |
| 2 | Test-first gating (d) | wire existing | `tdd_strict.rs`, `mini_coder.rs`, `runner.rs` |
| 3 | Bounded bundle (a,j) | reshape | `mini_coder_executor.rs`, `compact.rs`, `devboule-coder/main.rs` |
| 4 | SUMMARY-as-truth + resume + A3 (f,g) | new+fix | `model.rs`, `runner.rs`, `mini_coder_executor.rs`, `main.rs` |
| 5 | Branch-per-task + stuck-report (i,h+) | new | `runner.rs`, git plumbing, watchdog |

**Zero new external crates.** Net effect: the fragile half of v5 (`compact_built_prompt`
re-parsing, token-estimate gate, dead decompose) is deleted; the mini-task machine devboule
already has is finished, de-bugged, and made restart-safe — spoon-feeding local models one atomic,
acceptance-checked task at a time.
