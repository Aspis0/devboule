# Devboule — Canonical State & Architecture (2026-06-21)

> **What this doc is.** The single, code-verified source of truth for what Devboule
> (the app in this repo, formerly "Aspis") **actually is and does right now**. It
> unifies and supersedes the scattered 2026-06 design/state docs (see
> [§11 Doc provenance](#11-doc-provenance)). The only other doc that stays
> authoritative is **`master-plan-2026-06-self-improving-mini-design.md`** — the
> forward-looking phase list / north star. This doc is the *present tense*; the
> master plan is the *future tense*.
>
> Every claim here was verified against the code on branch `mac-platform-fixes`
> (file:line / commit cited where it matters). Where older docs disagreed with the
> code, **the code won** and the divergence is flagged inline.

---

## 1. What Devboule is

A **Tauri desktop app** (Rust backend in `src-tauri/`, React/TS frontend in `src/`)
that runs an AI-driven software-project workflow on the developer's own machine:

- A **per-project Kanban** of coding tasks.
- One or more AI **coders** (claude CLI, codex CLI, or the bundled local
  `devboule-coder` binary) that plan and execute those tasks.
- A deterministic **Censor** (linters/compilers + an optional small local LLM) that
  gates AI-written code before it reaches human review.
- A **resource broker** that detects the machine (RAM / GPU / running backends),
  budgets it across roles, and gates local model spawns so they can't blow RAM.

The north-star (see master plan) is a **self-improving local mini-coder**: capable
local models emit edits → Censor validates → accepted edit-pairs are captured as
ORPO training data → the next fine-tune is better → repeat. The "flywheel" data
capture is already live (`training_export.rs`); the nightly training loop is future
work (master-plan P13/P14).

---

## 2. Architecture (current, code-verified)

| Component | Location | Role |
|---|---|---|
| Tauri backend | `src-tauri/src/backend/` (~40 modules) | All Rust app logic |
| Frontend | `src/` (React/TS) | UI |
| Oracle MCP server | `oracle/aspis_mcp.py` | MCP server exposing Oracle/Kanban/plan/spawn/censor tools to any connected coder; **server-side role gate** |
| **devboule-coder** | `devboule-coder/` (standalone Rust binary, NOT a src-tauri member) | The bundled **local main coder / orchestrator** — REPL + bounded agentic loop + `rmcp` MCP client |
| Resource broker (L0) | `backend/budget.rs`, `hardware.rs`, `provider_detect.rs`, `model_registry.rs` | Detect machine, aggregate RAM (oMLX+Ollama), spawn-gate, recommend config, multi-project placement |
| Mini-coder executor | `backend/mini_coder_executor.rs`, `mini_coder.rs` | Runs local minis: one-shot **emit-edits** (default) or **agentic loop** (capable >20B tier) |
| Censor | `backend/censor/` (`gemma.rs`, `orchestrator.rs`, `watch.rs`, `ledger.rs`) | Deterministic linter/compiler gate (35 runners) + optional local-LLM semantic review |
| Kanban / projects | `oracle/aspis_mcp.py` + `backend/projects.rs`, `model.rs` | Task store + role-gated status transitions |
| Plan system | `backend/plan_approval.rs` + `aspis_mcp.py` (`plan_submit`) + `devboule-coder/planner.rs`, `runner.rs` | DAG planner → human approval gate → deterministic concurrent runner |
| Activity Console | `src/components/agents/AgentConsole.tsx` + `backend/mini_activity.rs` | Live event timeline + real unified diffs, fed by `mini-activity://<id>` channel |
| User MCP servers | `backend/user_mcp_config.rs` + `devboule-coder/multi_mcp.rs` | User-declared extra MCP servers (global + project scope), injected into main coders only |
| Training export | `backend/training_export.rs` | Write-only ORPO/flywheel capture rail (`.aspis-training/`) |

**How they fit.** External coders are launched as subprocesses with an MCP config
pointing at `aspis_mcp.py`. They call Oracle tools (context, plan, spawn, Kanban,
censor). Spawned **minis** run *inside* `src-tauri` (sandboxed on macOS), emit edits
→ Censor watches the write → result lands on the Kanban in `review`. The
`devboule-coder` binary is the self-hosted local main coder: it connects to Oracle
via `rmcp`, registers as the `orchestrator` role, and runs a conversational REPL
with a bounded tool-burst inner loop.

---

## 3. Backend state — implemented subsystems

### 3.1 Resource broker — **DONE (all 8 build-sequence steps)**
- **Hardware** (`hardware.rs`): CPU/RAM via sysinfo; GPU via `system_profiler` (mac) /
  DXGI (Win); Apple M-series forced "integrated"; discrete threshold 512 MiB VRAM.
  `detect_hardware` is an ungated command.
- **Provider detect** (`provider_detect.rs`): concurrent probes for claude, codex,
  ollama (`:11434/api/tags`), omlx (`:8000/v1/models`), **appleFm** (`fm --help`, mac),
  api. Redirect-free, 1500 ms, 256 KiB body cap.
- **Budget** (`budget.rs`): aggregates oMLX (`:8000/health` → current model bytes) +
  Ollama (`:11434/api/ps` → Σ loaded). Default **8 GiB reserve**. ⚠️ *Oracle process
  RAM is NOT counted* (only oMLX+Ollama) — divergence from docs that say "+Oracle".
- **Spawn-gate** (`admit_local_spawn` / `evaluate_local_spawn`): compute-cap → never-fits
  (`RouteToCloud`) → not-now (`Queue`) → `Admit`.
- **Recommend** (`recommend_config`): avail-GiB tiers `<6` minimal / `6-14` low /
  `14-40` mid / `≥40` high.
- **Placement** (`plan_placement`, Phase 8): greedy multi-project scheduler under
  budget+cap. Implemented.
- **Model registry** (`model_registry.rs`): id, backend, size, **tier** (`agentic` |
  `emitEdits`), roles, sampling; `discover_installed_models` probes both backends live.

### 3.2 Mini-coder executor — **DONE**
- **Two paths.** One-shot **emit-edits** (PTY → single backend POST → structured JSON
  edits) is the default. **Agentic iterative** (detached worker thread, multi-turn
  in-process read/edit/grep loop) runs for capable models.
- **Write-mode is capability-driven (S2).** `MiniWriteBehavior`: `Safe` → always
  emit-edits; **`Auto` (default)** → registry tier decides (`agentic` tier ⇒ agentic,
  unknown ⇒ safe emit-edits); `AgenticAllowed` → explicit user override. The global
  behavior is a **ceiling** over the per-directive request.
- **Sandbox.** macOS **Seatbelt** (`sandbox-exec -f <profile.sb>`) wraps the one-shot
  PTY, **only for loopback backends** (oMLX/Ollama/AppleFm). Plus `ulimit` rlimits.
  ⚠️ Codex/API (remote) minis are **not** sandboxed in this phase; Windows has no
  Seatbelt.
- **Mini Oracle grant (P3).** A directive may set `allow_oracle`; when true the mini
  gets exactly one read-only tool, `oracle_context` — **but only for Codex minis**.
  oMLX/Ollama minis never get it at runtime yet (plumbing exists). *(This is the real,
  narrow form of bug S7 — see §7.)*
- `MAX_AGENTIC_FIX_ROUNDS = 2` (not 3 — keeps worst case ~1380 s under the 1800 s poll).

### 3.3 Censor — **WIRED but OPT-IN**
- Deterministic tier (`orchestrator.rs`): **35 linter/compiler runners** (clippy,
  cargo check/audit/deny/fmt, tsc, eslint, oxlint, ruff, bandit, semgrep, gitleaks,
  shellcheck, hadolint, actionlint, …). Fine pass (per-file, 400 ms debounce) + coarse
  pass (project, 4000 ms).
- LLM tier (`gemma.rs`): the constant `GEMMA_MODEL` is **legacy in name only** — its
  value is **`hf.co/nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF:Q4_K_M`**. Gemma is retired.
- **Opt-in**: `resolve_gemma_model()` returns `""` when unconfigured ⇒ the AI tier
  stays OFF until the user picks a model; per-project `censor_trusted` defaults false.
- **Three providers**: Ollama (default), oMLX, AppleFm. All base-URLs loopback-clamped
  (privacy fail-safe: file content never leaves the device). `generate_with_images`
  exists for vision critique.
- ⚠️ **No DEEP / tool-calling mode** is implemented — only a single forward pass. The
  "cross-file DEEP mode" is a comment/future, not a code path.

### 3.4 Oracle MCP — role-gated grants (`aspis_mcp.py`)
Roles: `coder`, `orchestrator`, `verifier`, `mini`. **Server-side enforced** via
`require_registered_role()` against `ROLE_ALLOWED_TOOLS`.

| Tool | coder | orchestrator | verifier | mini |
|---|---|---|---|---|
| `oracle_context` | ✅ | ✅ | ✅ | ✅ |
| `oracle_ask` | ✅ | ✅ | ✅ | ✕ |
| `spawn_mini_coder` | ✅ | ✅ | ✕ | ✕ |
| Kanban `project_*` | ✅ | ✅ | ✅ (read + limited) | ✕ |
| `censor_findings` | ✅ | ✅ | ✅ | ✕ |
| `plan_submit` | ✅ | ✅ | ✕ | ✕ |
| `request_git_push` | ✅ | ✅ | ✕ | ✕ |
| `project_structure` | ✅ | ✅ | ✅ | ✅ |

The **role**, not the client binary, decides the grant. claude/codex self-register as
`coder`; devboule-coder as `orchestrator`. **Censor is not an Oracle role** (it runs
inside the Rust backend, doesn't call Oracle).

### 3.5 devboule-coder (local main coder) — **DONE**
- **Concurrent DAG runner** (`runner.rs`): reads the Kanban, auto-detects the active
  plan, runs up to **`MAX_PARALLEL_TASKS = 2`** independent tasks concurrently
  (`buffer_unordered`); deps satisfied on `{review, done}`; `MAX_TASK_ATTEMPTS = 2`;
  **runner only sets `review`, never `done`** (done = verifier-only).
- **Planner + human gate**: `plan_submit` waits for approval; `project_create_plan_tasks`
  populates the Kanban after approval; runner errors "no active plan; run plan first"
  when none exists.
- **Steering**: `steer_mini_coder` appends to a per-directive `steer_queue`
  (`MAX_STEER_QUEUE_LEN = 8`, msg ≤ 2000, `"stop"` sentinel), folded into the next
  fix-round; bidi-sanitized.
- **Async spawn**: `spawn_mini_coder { wait:false }` returns `directiveId` immediately;
  `mini_coder_result(directiveId)` fetches later. Ownership-checked.
- **rmcp client** (`rmcp_backend.rs`): stdio child + `initialize` + `agent_register`;
  **per-tool timeouts** — `spawn_mini_coder`/`mini_coder_result` **1920 s**,
  `plan_submit`/`request_git_push`/`ask_user` **720 s**, `visual_check` 240 s, default
  120 s. *(The old flat-120 s `CALL_TOOL_TIMEOUT` gap is closed.)*
- **Burst loop** (`agent_loop.rs`): `MAX_ROUNDS = 14`, `MAX_FORMAT_ERRORS = 3`,
  `DEFAULT_BURST_BUDGET = 120 s`, output-hash loop-detector (`OUTPUT_HASH_WINDOW = 6`).
  `run_plan` time is excluded from the wall-clock budget.

### 3.6 User MCP servers — **DONE**
- Config: global `<app-data>/user-mcp-servers.json` + project
  `<root>/.devboule/mcp-servers.json`; project wins on collision and is additionally
  gated by an exact-command allowlist (fail-secure). Reserved name prefixes
  `oracle/devboule/aspis`. 256 KiB/file, ≤ 20 servers.
- Injected via `DEVBOULE_USER_MCP_SERVERS` into claude/codex launches and into
  devboule-coder's `MultiMcpBackend` (Oracle always first and un-shadowable; user
  tools reached only via `call_user_tool`).
- **HARD invariant**: minis **never** receive user MCP servers.

### 3.7 Training export — **DONE**
- `.aspis-training/` (self-`.gitignore`): `findings.jsonl`, `pairs.jsonl`,
  content-addressed `blobs/<sha256>` (256 KiB cap, deduped, 50 MiB rotation).
- ORPO `write_fix_pair` markers emitted only for **emit-edits, attempt>0, Done**
  directives — **not** for agentic trajectories (B3: keeps the preference signal clean).
- Sensitive-file blocklist (`.env`, keys, `wrangler.toml`, …) + path-traversal guard;
  fire-and-forget (never surfaces Err, logs paths only).

---

## 4. Frontend / UI state (current, code-verified)

- **Dashboard: REMOVED.** No component/route; legacy deep-links redirect to
  `projects` (`utils/deepLink.ts`). Any doc/mockup showing a Dashboard is stale.
- **Projects = two levels** in one component (`views/ProjectsView.tsx`), toggled by a
  `workMode` boolean (not a route): **Board mode** (`ProjectsBoard` stage-kanban +
  calendar overview) ↔ **Work mode** (`ProjectWorkspace`, full-bleed single-project IDE).
- **Project Workspace** = top git bar (Pull/Commit/Push) · left agent rail +
  SpawnPanel · center live terminal · the **task board** slot · a bottom **dock** ·
  **Notes** rendered *below* the dock (not a tab).
- **Dock tabs**: `Censor` (default) · `Git` · `Plans` · `Console` · `MCP` · `Changes`.
  There is **no "Activity" tab** (the structured timeline lives under **Console**).
- **Task Kanban**: 5 columns `todo / wip / review / blocked / done`; drag-and-drop
  across columns (drop-into-`done` is verifier-blocked).
  - **Dependency arrows ("frecce") are LIVE** — a **pixi.js / WebGL** overlay
    (`TaskDependencyArrows.tsx`) using `perfect-arrows` geometry, ~15 Hz, colored per
    target task's agent, with an **Arrows: on/off toggle**. `depends_on` is shown
    *only* as arrows (not as card text).
    ⚠️ **Magnetic anchors to create deps by dragging are NOT implemented** — arrows are
    read-only visualization (this is the open part of master-plan Phase 17).
- **Task cards** (`TaskCard.tsx`): color-coded **agent badges**; a **Move** menu
  (column targets) and a **Launch** menu (Code / Verify / Manual). **No pause/skip/retry
  buttons** on the card. No per-card cost strip.
- **Console** (`AgentConsole.tsx`): real unified **diffs** (`DiffBlock` fed live data
  via `mini_activity_snapshot` + `mini-activity://<id>`), timeline strip, cost footer
  (`est. task ~$x`, `total ~$x`). Marker parsing, no `dangerouslySetInnerHTML`.
- **Settings** (`views/SettingsView.tsx`): 4 tabs — `Account` · `Providers & Models` ·
  `Workspace & Index` · `Security`. **Collapsible groups live inside Providers &
  Models** (not at the top). Per-model sampling editing + vendor-seeded defaults
  (`ModelRegistryCard`); **"recommended for your machine"** (`RecommendedConfigCard`,
  display-only); **main-coder CLI select** claude/codex (`MainCoderClientCard`);
  **orchestrator model read-only** in `SpawnPanel`; oMLX base-URL prefill.

---

## 5. Roadmap status (done vs pending)

See the master plan for the full phase narrative. Current reconciliation:

**DONE & on `mac-platform-fixes`:**
- Master-plan **P1–P10** (oMLX wiring, Censor deterministic gate, mini read-only
  Oracle, mini write+allowlist, Seatbelt sandbox, bounded loop, ORPO capture, coder
  outer loop, Censor live tier, Skills rails).
- **Resource broker steps 1–8** (all).
- **devboule-coder L2** + **Phase 11.1/11.2** (structure graph, planner) + **11.3/11.4**
  (concurrent DAG runner + loop-detector) + **11.5-B pieces 1, 2, 4** (unified Kanban
  task store, live plan view, real Console diffs).
- **UI-reorg phases 0–2** (Dashboard removed, board+Notes into Work mode, slim panel,
  cost badge, timeline strip, Changes tab).
- **Phase 17 partial**: arrow overlay (Pixi/WebGL) + per-agent coloring.
- Jun-19 manual-test fix wave: R1–R7, S1, S2, S4, S5(partial), S8, B17, Q1, P1,
  Nanophase calibration.

**PENDING (the real forward work):**
- **11.5-B piece 3** — per-task pause/skip/retry buttons wired to the runner (partial /
  unconfirmed).
- **Phase 17 remainder** — magnetic anchors (drag A→B sets `depends_on`) +
  drag-into-column fires a column-specific agent.
- **Master-plan P11** (design objective scorers: tsc/tailwind/DTCG/Playwright/axe),
  **P13** (nightly ORPO on the Mac — blocked on dense/MoE decision + P15 data),
  **P14** (nightly SkillOpt — blocked on P11), **P15a** (external benchmarks +
  promotion policy — owner decision), **P16** (training/benchmark UX — parked).
- **P12** (vocab pruning) — intentionally deferred; only if P13 hits a RAM wall.

---

## 6. The flywheel (how self-improvement is meant to close)

`capable local mini → emit-edits → Censor (deterministic + LLM) validates →
accepted (path,old,new) pairs + rejected-attempt pre-images logged to
.aspis-training/pairs.jsonl → (future P13) nightly ORPO fine-tune → better mini`.
Everything left of "nightly ORPO" is live today; the training loop itself is P13.

---

## 7. Open bugs — the accurate TODO (2026-06-21)

> The bug log's old "STILL OPEN" block listing "R7 / parallel runner / colored arrows /
> frecce have nothing to draw" is **stale** — all of that shipped. Real open items:

**High:**
- **B2** — local oMLX main-coder may never POST to oMLX (silent **Mock** fallback);
  surface "local main coder not configured" instead of silent Mock. *(Needs a live
  trace; B2 also flagged for owner re-test.)*
- **B5** — `orchestrator` client behaves like a worker (asks for tasks / over-queries
  Oracle) — role/prompt injection misfiring.
- **B10** — app **freezes/closes after ~5 min** (suspected RAM pressure or a 300 s
  timer). No repro yet — needs Activity Monitor + dev stderr capture.

**Medium:**
- **B4** — Codex/missing-binary launch silently hangs (need pre-flight detect for all
  CLIs).
- **B6** — orchestrator over-consults Oracle on trivial tasks; lacks web-search.
- **B8** — plan not shown inline at the approval prompt (reuse the Plans view).
- **B11 / B13** (partial) — macro project `Active→Review` transition and external-CLI
  session-end liveness; R3 added the internal heartbeat, the macro transition is
  unconfirmed.
- **B16** — `Stop` on a finished project resets macro status to **Planned** (should be
  **Verified**) — wrong Stop-side state transition.
- **B18** — manually-created board task is orphaned (no project association → runner
  ignores it).
- **S7** — Oracle grant for **capable local (oMLX/Ollama) minis** is plumbing-only
  (P3 wired it for Codex minis only); Censor still has no Oracle path.

**Low:**
- **B15** — in-app terminal occasionally doubles text (cosmetic).
- **B3** (residual) — folder picker fixed (R1) but the "no Censor backend" inert-tools
  path remains until the user configures a Censor model.
- **S3** — Claude subscription not selectable as a mini backend.
- **S6** — AppleFM restricted to Censor; widen to all roles.
- **O1** — ×4 repeated "Oracle is starting" warnings at launch.

---

## 8. Key decisions & invariants (cross-cutting — do not violate)

- **`done` is verifier-only.** The runner sets finished tasks to `review`; only a
  human/verifier moves to `done`. Drag-into-`done` is blocked in the UI.
- **Runner "tira dritto":** a dependency is satisfied when the predecessor is `review`
  **or** `done` (no wait-for-`done`).
- **Kanban is the single task store.** `.devboule/tasks.json` is retired; plan tasks
  carry a `plan_id` tag and live on the Kanban.
- **Write-mode is capability-driven, not a raw toggle.** The registry tier drives the
  `Auto` default; an explicit user override still wins (Simplify = smart default + keep
  the manual override — never hard-block a user choice).
- **Minis never get user MCP servers**; only main coders do.
- **Censor privacy fail-safe:** all LLM-censor base-URLs are loopback-clamped; file
  content never leaves the device.
- **Sandbox is loopback-only (this phase).** Codex/API minis are not Seatbelt-confined
  yet; that's a future net-proxy phase.
- **Licensing invariant:** no GPL/AGPL/FSL in any bundled component.
- **GPU rule:** never touch the GPU during daytime app sessions (training is a separate
  session); the whole local-coder path is **unit-tested only — live e2e is
  GPU-deferred**.
- **Devboule is multi-language** — never hardcode Rust+TS-only assumptions in run-tool
  allowlists / linters / tooling.
- **Sampling defaults are per model family** (gemma 1.0 / qwen 0.6 / North-Mini 1.0);
  never blanket one temperature; never cap `max_tokens` on local models.

---

## 9. Censor / review-experts (app-side summary + pointer)

The app's Censor LLM tier is intended to be a **small base model + calibration**, not
a fine-tune. The companion research project **`~/Projects/review-experts`** confirmed an
**OOD wall**: across 6 trained arms (SFT / WiSE-FT / ORPO / linear-probe / GRPO×2) none
beats the untrained base (Seed-Coder-8B ≈ 0.39 recall / 0.33 FPR OOD). Direction there:
build a powered AI-code bench first, then distill+ORPO a non-thinking Qwen2.5-Coder-14B
— but that is **the other project's concern**. For *this* app, the integration path is
the **deterministic sandwich**: linters (always) → small local model (opt-in, Nemotron
default) → optional cloud escalation. The training internals stay in
`~/Projects/review-experts/docs/` (and the `review-experts-project` memory) — not here.

---

## 10. Build / test / run quick-reference

- Branch: **`mac-platform-fixes`** (pushed to `origin` through `a81ec4f`).
- Rust: `cargo test` (src-tauri + devboule-coder). Frontend: `tsc` + `vitest`. Python
  Oracle: `pytest`. All green as of the last fix-wave.
- The local-coder path is **unit-tested only** — see the GPU-deferred live e2e list in
  the master plan / §8.

---

## 11. Doc provenance

**This doc folds in and supersedes** (for *current state*) the following — keep them
only as historical detail / deep dives:

| Folded doc | Kept for |
|---|---|
| `resource-aware-orchestration-design-2026-06.md` | broker design rationale, §12 build sequence |
| `local-main-coder-harness-design-2026-06.md` | harness candidate analysis, security model |
| `phase-11.3-11.4-impl-plan-2026-06.md` | as-built runner detail |
| `phase-11.5-B-unified-tasks-ux-plan-2026-06.md` | the 4-piece UX rationale |
| `ui-projects-reorg-plan-2026-06.md` | file:line UI cleanup map |
| `app-test-bugs-2026-06.md` | full manual-test session log (bug repros) |
| `design-user-mcp-servers-2026-06.md` | consent model, allowlist design |
| `local-model-sampling-defaults-2026-06.md`, `frecce-local-model-param-review-2026-06.md` | per-model sampling tables |
| `censor-model-benchmark-2026-06.md` | Censor model selection benchmark |
| `local-coder-bug-ledger-2026-06.md` | local-model bug patterns (training signal) |

**Stays authoritative (not folded):**
- `master-plan-2026-06-self-improving-mini-design.md` — the forward phase list.
- `local-coder-AGENTS.md` — the live system prompt for local coders.
- The review-experts / Censor-training docs — owned by `~/Projects/review-experts`.

**Never-commit working logs** (untracked by design): `review-findings-log-2026-06.md`,
`review-findings-2026-06-17-localcoder-training.md`, `local-review-experts-*` working
files.

---

*Generated 2026-06-21 from a code-verified audit (frontend / backend / bug+roadmap).
When this doc and the code disagree, fix the doc.*
