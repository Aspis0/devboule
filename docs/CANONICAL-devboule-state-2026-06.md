# Devboule — Canonical State & Architecture (updated 2026-06-26)

> **What this doc is.** The single source of truth for what Devboule (the app in this
> repo, formerly "Aspis") **actually is and does right now**. It unifies the scattered
> 2026-06 design/state docs (see [§11 Doc provenance](#11-doc-provenance)). The only
> other doc that stays authoritative is **`master-plan-2026-06-self-improving-mini-design.md`**
> — the forward-looking phase list / north star. This doc is the *present tense*; the
> master plan is the *future tense*.
>
> **Verification basis.** The original §3.1–§3.7 + §4 were code-verified line-by-line on
> branch `mac-platform-fixes` (2026-06-21). The **2026-06-26 update** adds the epics that
> landed since (§3.8–§3.12, the §4 UI rewrite, §5/§7/§8 refresh): those are verified
> against the **file tree + git HEAD on branch `sandbox-epic`** (every module cited was
> confirmed present on disk) and cross-checked against the session commits/memories — but
> they are **not** a fresh per-line audit. Memory-sourced claims that weren't re-read in
> code are flagged ⚠️. Where this doc and the code disagree, fix the doc.
>
> **Branches.** Most product work lives on **`mac-platform-fixes`**. The **Sandbox +
> permission-broker epic** is on **`sandbox-epic`** (current checkout, HEAD `66eeb5d`,
> pushed), which forked from `mac-platform-fixes` after the Pigeon commit — so it carries
> the broker/sandbox/cloud-duplex/Pigeon/Polis modules. A few Projects "Phase-D cloud
> duplex" pieces were reported green but **uncommitted** in their session; treat anything
> tagged ⚠️UNCOMMITTED as needing a git check.

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

Around that core, several **larger surfaces** have since landed (all detailed below):
a **Skills + marketplace** runtime (SKILL.md manuals vs MCP tools), a **Pigeon** async
mailbox (decouple agents in time), a redesigned **Projects "orchestrator-first"** page
with a live **Plan-Mode Stage/Chat TUI** and **cloud-duplex** streaming for Claude/Codex,
a unified **Work Console**, an OS-level **sandbox + permission broker**, and **Polis** —
an isometric "living city" map of the codebase.

The north-star (see master plan) is a **self-improving local mini-coder**: capable local
models emit edits → Censor validates → accepted edit-pairs are captured as ORPO training
data → the next fine-tune is better → repeat. The "flywheel" data capture is already live
(`training_export.rs`); the nightly training loop is future work (master-plan P13/P14).

---

## 2. Architecture (current, code-verified)

| Component | Location | Role |
|---|---|---|
| Tauri backend | `src-tauri/src/backend/` (~55 modules) | All Rust app logic |
| Frontend | `src/` (React/TS) | UI |
| Oracle MCP server | `oracle/aspis_mcp.py` | MCP server exposing Oracle/Kanban/plan/spawn/censor tools to any connected coder; **server-side role gate** |
| **devboule-coder** | `devboule-coder/` (standalone Rust binary, NOT a src-tauri member) | The bundled **local main coder / orchestrator** — REPL + bounded agentic loop + `rmcp` MCP client |
| Resource broker (L0) | `backend/budget.rs`, `hardware.rs`, `provider_detect.rs`, `model_registry.rs` | Detect machine, aggregate RAM, spawn-gate, recommend config, multi-project placement |
| Mini-coder executor | `backend/mini_coder_executor.rs`, `mini_coder.rs` | Runs local minis: one-shot **emit-edits** (default) or **agentic loop** (capable >20B tier) |
| Censor | `backend/censor/` (`gemma.rs`, `orchestrator.rs`, `watch.rs`, `ledger.rs`) | Deterministic linter/compiler gate (35 runners) + optional local-LLM semantic review |
| Kanban / projects | `oracle/aspis_mcp.py` + `backend/projects.rs`, `model.rs` | Task store + role-gated status transitions |
| Plan system | `backend/plan_approval.rs` + `aspis_mcp.py` (`plan_submit`) + `devboule-coder/planner.rs`, `runner.rs` | DAG planner → human approval gate → deterministic concurrent runner |
| Activity Console | `src/components/agents/AgentConsole.tsx` + `backend/mini_activity.rs` | Live event timeline + real unified diffs, fed by `mini-activity://<id>` channel |
| User MCP servers | `backend/user_mcp_config.rs` + `devboule-coder/multi_mcp.rs` | User-declared extra MCP servers (global + project scope), injected into main coders only |
| Training export | `backend/training_export.rs` | Write-only ORPO/flywheel capture rail (`.aspis-training/`) |
| **Skills runtime** | `backend/skill_format.rs`, `skill_vet.rs`, `skill_marketplace.rs`, `tdd_strict.rs` + `devboule-coder/skills.rs` | SKILL.md parse/vet/install/marketplace + LoadSkill client + TDD-strict gate engine |
| **Sandbox** | `backend/sandbox/{mod.rs,seatbelt.rs}` | `SandboxPolicy` + `wrap()` + Seatbelt profile builder (macOS) |
| **Permission broker** | `backend/broker/mod.rs` | `ConsentKind/Request/Decision` spine + transient grants; per-project sandbox modes |
| **Cloud duplex** | `backend/cloud_duplex.rs`, `cloud_claude.rs`, `cloud_codex.rs`, `consent_bridge.rs`, `consent_hook.rs` + `claude_consent_hook` bin | Stream Claude (`stream-json`) / Codex (`app-server` JSON-RPC) into the same bridge + unified consent card |
| **Pigeon** | `pigeon/*.py` (FastAPI + aiosqlite) + `backend/pigeon_service.rs` | Optional persistent async mailbox (default-off) |
| **Polis** | `src-tauri/src/polis/scanner.rs` + `src/components/polis/*` + `src/store/cityStore.ts` + `src/types/city.ts` | Isometric "living city" map of the codebase + the Augur |
| **Design pipeline** | `backend/design_request.rs` + frontend design canvas/registry | Orchestrator `design_request` → reuse the design-gen pipeline → surface in Stage |

**How they fit.** External coders are launched as subprocesses with an MCP config
pointing at `aspis_mcp.py`. They call Oracle tools (context, plan, spawn, Kanban,
censor). Spawned **minis** run *inside* `src-tauri` (sandboxed on macOS), emit edits
→ Censor watches the write → result lands on the Kanban in `review`. The
`devboule-coder` binary is the self-hosted local main coder: it connects to Oracle
via `rmcp`, registers as the `orchestrator` role, and runs a conversational loop with
a bounded tool-burst inner loop.

---

## 3. Backend state — implemented subsystems

### 3.1 Resource broker — **DONE (all 8 build-sequence steps)**
- **Hardware** (`hardware.rs`): CPU/RAM via sysinfo; GPU via `system_profiler` (mac) /
  DXGI (Win); Apple M-series forced "integrated"; discrete threshold 512 MiB VRAM.
- **Provider detect** (`provider_detect.rs`): concurrent probes for claude, codex,
  ollama (`:11434/api/tags`), omlx (`:8000/v1/models`), **appleFm** (`fm --help`, mac),
  api. Redirect-free, 1500 ms, 256 KiB body cap.
- **Budget** (`budget.rs`): aggregates oMLX (`:8000/health`) + Ollama (`:11434/api/ps`).
  Default **8 GiB reserve**. ⚠️ Oracle process RAM is NOT counted (only oMLX+Ollama).
- **Spawn-gate** (`admit_local_spawn`/`evaluate_local_spawn`): compute-cap → never-fits
  (`RouteToCloud`) → not-now (`Queue`) → `Admit`.
- **Recommend** (`recommend_config`): avail-GiB tiers `<6` minimal / `6-14` low /
  `14-40` mid / `≥40` high.
- **Placement** (`plan_placement`): greedy multi-project scheduler under budget+cap.
- **Model registry** (`model_registry.rs`): id, backend, size, **tier** (`agentic` |
  `emitEdits`), roles, sampling; `discover_installed_models` probes both backends live.

### 3.2 Mini-coder executor — **DONE**
- **Two paths.** One-shot **emit-edits** (PTY → single backend POST → structured JSON
  edits) is the default. **Agentic iterative** (detached worker thread, multi-turn
  in-process read/edit/grep loop) runs for capable models.
- **Write-mode is capability-driven (S2).** `MiniWriteBehavior`: `Safe` → always
  emit-edits; **`Auto` (default)** → registry tier decides; `AgenticAllowed` → explicit
  user override. The global behavior is a **ceiling** over the per-directive request.
- **Sandbox.** macOS **Seatbelt** wraps the one-shot PTY for loopback backends only.
  Plus `ulimit` rlimits. (Now unified under `backend/sandbox/` — see §3.9.)
- **Mini Oracle grant (P3).** A directive may set `allow_oracle` → exactly one read-only
  tool, `oracle_context` — **but only for Codex minis** today. (Residual bug S7.)
- `MAX_AGENTIC_FIX_ROUNDS = 2`.

### 3.3 Censor — **WIRED but OPT-IN**
- Deterministic tier (`orchestrator.rs`): **35 linter/compiler runners** (clippy, cargo
  check/audit/deny/fmt, tsc, eslint, oxlint, ruff, bandit, semgrep, gitleaks, shellcheck,
  hadolint, actionlint, …). Fine pass (per-file, 400 ms debounce) + coarse pass (4000 ms).
- LLM tier (`gemma.rs`): `GEMMA_MODEL` is **legacy in name only** → value is
  **`hf.co/nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF:Q4_K_M`**. Gemma is retired.
- **Opt-in**: AI tier OFF until the user picks a model; per-project `censor_trusted`
  defaults false.
- **Three providers** (Ollama default, oMLX, AppleFm), all loopback-clamped (privacy
  fail-safe: file content never leaves the device). `generate_with_images` for vision.
- ⚠️ **No DEEP / tool-calling mode** — single forward pass only.

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
| `project_set_title` | ✅ | ✅ | ✕ | ✕ |
| `design_request` | ✅ | ✅ | ✕ | ✕ |

The **role**, not the client binary, decides the grant. claude/codex self-register as
`coder`; devboule-coder as `orchestrator`. **Censor is not an Oracle role.**

### 3.5 devboule-coder (local main coder) — **DONE**
- **Concurrent DAG runner** (`runner.rs`): `MAX_PARALLEL_TASKS = 2`; deps satisfied on
  `{review, done}`; `MAX_TASK_ATTEMPTS = 2`; **runner only sets `review`, never `done`**.
- **Planner + human gate**: `plan_submit` waits for approval; `project_create_plan_tasks`
  populates the Kanban after approval.
- **Conversation-native session** (`run_session`, B1): split from one-shot `run_once`;
  `Escalated` is now RECOVERABLE → pause+wait (no longer kills); 48k conversation trim;
  folds the Done reply. (`run_once` kept for the headless `DEVBOULE_GOAL` plan-first path.)
- **Steering**: `steer_mini_coder` + `DEVBOULE_STEER_FILE` inbox (`steer.rs` `Steer::drain`,
  64 KB cap) drained between burst rounds → `TranscriptEntry::Human`.
- **Streaming output** (B14b): `OmlxModel::run_completion_streaming` (SSE) + `reply_stream.rs`
  `ReplyStreamExtractor` pulls partial `done.reply`/`ask_user.question` from the growing
  action JSON → `chat-delta` bridge events (blink-caret bubble).
- **rmcp client** (`rmcp_backend.rs`): per-tool timeouts — spawn/result **1920 s**,
  plan/push/ask **720 s**, visual_check 240 s, default 120 s.
- **Burst loop** (`agent_loop.rs`): `MAX_ROUNDS = 14`, output-hash loop-detector.

### 3.6 User MCP servers — **DONE**
- Config: global `<app-data>/user-mcp-servers.json` + project `<root>/.devboule/mcp-servers.json`
  (project wins, exact-command allowlist, fail-secure). Reserved prefixes `oracle/devboule/aspis`.
- Injected via `DEVBOULE_USER_MCP_SERVERS` into claude/codex + devboule-coder's `MultiMcpBackend`
  (Oracle always first + un-shadowable; user tools via `call_user_tool`).
- **HARD invariant**: minis **never** receive user MCP servers.

### 3.7 Training export — **DONE**
- `.aspis-training/` (self-`.gitignore`): `findings.jsonl`, `pairs.jsonl`,
  content-addressed `blobs/<sha256>` (256 KiB cap, deduped, 50 MiB rotation).
- ORPO `write_fix_pair` markers only for **emit-edits, attempt>0, Done** directives — not
  agentic trajectories (B3: clean preference signal).
- Sensitive-file blocklist + path-traversal guard; fire-and-forget.

### 3.8 Skills + marketplace runtime — **DONE (6 phases, committed + pushed origin 2026-06-22 on `mac-platform-fixes`)**
The pillar: **SKILL = manual** (prompt-injected, Rail A) vs **TOOL = machine** (MCP, Rail B).
Composed prompt = **FIXED prefix** (BASE + PROJECT-CONTEXT/AGENTS.md) + **MOBILE** (role +
language + marketplace skills) + **TASK** — separate ordered sources, cache-warm, not merged.
- **F1 `skill_format.rs`**: SKILL.md frontmatter parser (no-frontmatter ⇒ byte-identical);
  agentskills.io conformance via `validate_skill` + `metadata` map.
- **F2 PROJECT-CONTEXT layer**: `read_project_context` (AGENTS.override → AGENTS → CLAUDE
  precedence) → `fenced_project_context_block` as the FIXED prefix at all consumers
  (external-CLI prompt, mini prompt, devboule via `DEVBOULE_PROJECT_CONTEXT`).
- **F3 LoadSkill client** (`devboule-coder/skills.rs`): Level-1 catalog (sanitized+fenced)
  + `AgentAction::LoadSkill` Level-2 progressive disclosure (goose `canonical.starts_with`
  traversal guard; body returned FENCED + neutralized as untrusted).
- **F4 marketplace**: `skill_vet.rs` (SkillGate ruleset → Rust regex) + `skill_marketplace.rs`
  (SSRF-guarded fetch, IP-pinned, v6-smuggle-aware) + install/provenance(sha) + commands
  `skills_marketplace_preview`/`_install` + `MarketplaceInstall.tsx` vetting UI (Danger-gated).
- **F5 Tools(MCP) tab**: surfaces the existing user-MCP (allowlist-gated) in the unified Discover.
- **F6 clarity banner**: Skills(manuals)-vs-Tools(machines) + fixed(AGENTS.md)-vs-mobile(skills).
- **TDD-strict engine** (`tdd_strict.rs`, pure, present on disk): `assert_test_untouched`
  (immutability), `detect_test_gaming` (cross-lang skip/ignore/xfail/.only/@Disabled + trivial
  -assert + infra-tamper), `evaluate_gate` (red→green+gaming). ⚠️ The LIVE executor wiring
  (`tdd_test_path` directive + subprocess red→green run) is **GPU-deferred**; immutability is
  already enforced free via the directive-allowlist exclusion.
- ⚠️ **UNVERIFIED on `sandbox-epic`**: the follow-on (7 bundled library skills under
  `assets/skills/library/`, `SkillsDiscovery.tsx`, `featured_marketplaces`) was reported
  green+max-recall'd but was UNCOMMITTED in its session — `assets/skills/library/` is **empty
  on the current branch**. `tdd_strict.rs` IS present. Check git before relying on the bundles.

### 3.9 Sandbox + permission broker — **macOS DONE; Windows + live-UX pending** (branch `sandbox-epic`, HEAD `66eeb5d`, pushed)
The foundational gate of the go-live stack `Sandbox → Auto-mode → Pigeon`.
- **Sandbox foundation** (`backend/sandbox/{mod.rs,seatbelt.rs}`): `SandboxPolicy` + builder;
  `seatbelt::build_profile` (3 `NetPolicy`, absolute-path validation, real-kernel-parser
  regression test); `wrap()` → `SandboxedCommand`. The boundary is **writes + network**, not
  reads (broad read + confined write + loopback-only net = no exfiltration path).
- **Mini-agentico under sandbox** (Phase 1): `net=None` default + per-project net-unlock,
  `.git`/`.devboule` **regex** write-deny (blocks planted-git-hook RCE), net-blocked-failure
  HINT, rlimits via `libc::setrlimit` in `pre_exec` (CPU/AS; **no NPROC** — UID-wide).
- **base_url use-time gate** (Phase 6): agentic AND one-shot.
- **Censor sandbox = REVERTED by decision** (`1a3d2ac`): a uniform OS sandbox over 33 runners
  is too fragile (silent false-clean is worse than no sandbox for a security tool); the real
  risk (build.rs / proc-macro / eslint-plugin 3rd-party exec) is gated by **`censor_trusted`**
  per-project opt-in instead.
- **Local permission broker = Auto-mode** (`backend/broker/mod.rs`, Slices 0-3): provider-
  agnostic spine `ConsentKind/ConsentRequest{kind,project_id,agent_id,detail,path,approval_id}`
  / `ConsentDecision{AllowOnce|AllowRemember|Deny}` + `PermissionBrokerState` (transient
  net grants, one-shot consume/reinsert). Per-project on `ProjectMetadata`:
  `sandbox_mode {Ask | AutoAcceptInWorkspace | Unattended}` + `net_enabled` + working-set
  folders. **Unattended = fail-closed** (refuses even transient grants → the Pigeon gate).
- **Consent UX (ratified by the owner):** net = **prompt at the moment of failure**
  `[Consenti per il progetto] [Solo stavolta] [Nega]`; out-of-project folder write =
  **prompt + remember** `[Consenti+ricorda] [Solo ora] [Nega]`; modes = **per-project selector**.
  Physical constraint: Seatbelt can't hot-widen → consent **applies at next spawn**, not mid-run.
- **Cloud adapters** (Slice 5 / 5a / 5b / 5c): Claude/Codex are **NOT re-sandboxed** — reuse
  their native modes and **bridge their prompts into OUR clickable card**:
  - **Codex** (`cloud_codex.rs` + extends `cloud_duplex.rs`): `app-server` JSON-RPC
    `initialize→thread/start→turn/start`; `item/.../requestApproval` → our card via a
    non-blocking per-approval waiter (120 s, 32 in-flight cap, string/number JSON-RPC ids);
    `sandbox_mode` → `approvalPolicy` + `runtimeWorkspaceRoots`.
  - **Claude** (`cloud_claude.rs` + `consent_bridge.rs` + `consent_hook.rs` + compiled
    **`claude_consent_hook` bin**): a **PreToolUse hook** bridges Patch/Exec calls through the
    `.aspis-agents.json` file-bridge (reuses the git-push gate pattern, no Unix socket) into the
    same card; generated `settings.json` (deny rules + hook) written to a temp file, passed via
    `--settings <path>`; **never autonomous unless the hook is actually registered** (fail-closed).
  - **5c agent controls**: per-project `AgentControls{effort,system_prompt,max_turns,max_budget_usd}`,
    persisted via the **hand-rolled YAML frontmatter** (NOT serde — see §8 gotcha), emitted as
    Claude flags / Codex `thread/start` fields. `AgentControlsCard` dock.
- **NOT done** (needs env/hardware unavailable headless): **Phase 3 Windows** (no runner — steal
  codex `windows-sandbox-rs`: Restricted Token + WFP + capability SID); **Phase 2 frontend**
  broker UX live e2e; live e2e of the cloud adapters (the unstable wire-shapes — Codex
  `thread/start` field names, Claude `--settings`/hook firing — are flagged for the owner's eyes);
  Slice 4 (shell-EPERM heuristic) = deliberately documented-only.

### 3.10 Pigeon (async mailbox) — **v0.1 slice DONE + GREEN, inert/default-off** (committed `6632639` on `mac-platform-fixes`)
A persistent SQLite mailbox that decouples LLM agents in **time** (a main coder delegates to a
mini, gets unloaded from RAM, the task survives + the reply finds it on reload). Actor-model.
- `pigeon/{__init__,config,db,models,dispatcher}.py` (FastAPI + aiosqlite) + `pigeon/tests/*`;
  Rust supervisor + toggle in `backend/pigeon_service.rs`. Endpoints `/pigeon/send /poll /done
  /agent /status/{t} /queue/{a}` + `/health`. Claim = atomic `UPDATE…RETURNING`; `/done`
  auto-creates a `priority:10` reply (the temporal-decoupling proof). Residency = `agents.status`.
- **34 Python + 3 Rust gate tests, all green.** Reused oracle cross-platform patterns (supervisor,
  venv resolver, config-write-lock + atomic replace).
- **KEY constraint (the owner):** OPTIONAL + completely disableable. `config.json` `pigeon.enabled`
  (default **false**) → Rust `start_if_enabled` is a clean no-op when off; on = spawns
  `python -m pigeon.dispatcher` (separate process = clean kill switch). Built PARALLEL to the
  live dispatch (prove-first); **NOT wired to live mini-dispatch yet.**
- ⚠️ go-live (wire mini-dispatch onto `/pigeon/poll`) is gated by the `Sandbox → Auto-mode →
  Pigeon` stack: without auto-mode the sender is human-gated and never unloads mid-op. v0.2+
  (Scheduler DRR, Head, cloud push/SSE, MCP tools, dashboard) per the design doc §14.

### 3.11 Polis backend scanner — **present on disk** (`src-tauri/src/polis/scanner.rs`)
The Rust side of the isometric city map (§4 has the frontend). Walks the project, extracts
buildings (files → purpose/tier/coords), roads (imports → lastricata, semantic → terra battuta,
infra → acquedotto), districts, and feeds the `CityState` the Pixi renderer consumes. The Augur's
"urban sins" (hardcoded API keys, cyclic imports, committed secrets, stale files, …) are
deterministic Rust detectors. ⚠️ Live wiring depth (which Tauri commands are registered, Scaleway
integration) **not re-audited this pass** — design is `aspis-bio-polis-map.md`.

### 3.12 Cloud duplex + design pipeline — **implemented + wired** (some pieces ⚠️UNCOMMITTED in their session)
- `cloud_claude.rs` (`ClaudeNormalizer`, real-fixture TDD) + `cloud_codex.rs` (`CodexNormalizer`,
  synthetic from docs) → both emit the SAME bridge JSONL (`chat`/`chat-delta`/`websearch`/
  `milestone`). `cloud_duplex.rs`: `CloudDuplexSessions` registry + `spawn_cloud_duplex` (piped
  std child, reader thread runs the normalizer → appends to the activity file) + `cloud_duplex_send`
  (steer → stdin) + `kill_cloud_duplex`. Security: duplex env sets `GIT_CONFIG_NOSYSTEM=1` +
  a session gitconfig (cloud CLI can't read the user's real credential helper / bypass the push gate).
- **Approvals model (the owner):** reads always auto; WRITES use the CLI's own native approval
  (Claude `--permission-mode plan`, Codex on-request) surfaced in the Stage.
- **Design pipeline** (`backend/design_request.rs`): orchestrator MCP tool `design_request(prompt,
  context)` → directive → frontend `useDesignRequestWatcher` REUSES the design-gen pipeline
  (separate `designLlmBackend` model) → result lands in the Stage **Design** view.
- ⚠️ Codex `app-server` handshake (initialize/newThread) still NOT implemented (no Codex to test);
  the exact Claude flags + Codex field names need the owner's live e2e (cloud keys).

---

## 4. Frontend / UI state (current, code-verified)

- **Dashboard: REMOVED.** Legacy deep-links redirect to `projects`.
- **Projects page = "orchestrator-first"** (redesign from a Claude-Design mockup). Mental
  model (the owner): the page is **CREATE a project**, and **a project IS a plan** — talk to the
  orchestrator to shape project+plan → split into tasks → BUILD with a coder. Opens **EMPTY**;
  the **Kanban IS the history** (click a card → that project). A project depends on a working
  folder. **Agent names kept** (orchestrator / main coder / mini / Censor) — explained, not renamed.
- **Plan-Mode panel** (`src/components/projects/planner/`): the live planning surface, two regions
  (the owner's terms) — **Stage** (top, 3 rotating views: **Websearch** = real Exa pages+findings /
  **Plan** / **Design** read-only) + **Chat** (bottom, the TUI conversation). The orchestrator
  emits real chat turns (`Activity::chat` → bubbles) with an ask→answer→continue loop; `run_session`
  stays alive across replies; durable per-project transcript (`read_activity_chat` reads the .jsonl).
  Cloud orchestrators (Claude/Codex) drive the SAME Stage via the duplex bridge (§3.12). 10 `pp-*`
  keyframes + GSAP. Typed goal reaches the planner via `DEVBOULE_GOAL` (headless `run_once`,
  plan-first); auto-create gate (`DEVBOULE_AUTO_CREATE`).
- **Project Workspace (Work mode) = the unified Work Console** (`ProjectWorkspace.tsx`): the center
  is **`FocusStage`** (Activity/Raw + two-way composer + inline amber question card), NOT a raw
  terminal; the **`LivingPlan`** hero replaced the old agent rail (launcher moved to a top-bar
  "+ Launch" → SpawnPanel); a **`CensorStrip`** shows project-wide clean/dirty; "Change plan"
  recalls the orchestrator (reuses the planner). **Board ↔ console are twinned** via a zustand
  `workSelectionStore` (`selectBoth`/`clear`): click a board card → focus its agent + ring the card.
  **Split view** via `react-resizable-panels@2.1.9` (`FocusStagePane` per agent; "split" → two
  resizable panes). The old **Console dock tab was DELETED** (structured timeline now lives in
  FocusStage); dead rail/question-card/timeline-strip components removed.
- **Dock tabs** (the bottom dock that remains): `Censor` · `Git` · `Plans` · `MCP` · `Changes`
  (+ `AgentControlsCard`). `Changes` is **scoped** to a per-launch baseline (`ensure_diff_baseline`
  git-stash snapshot), not the dev's whole working tree.
- **Task Kanban**: 5 columns `todo / wip / review / blocked / done`; drag-and-drop (drop-into-`done`
  verifier-blocked). **Dependency arrows ("frecce")** = a **pixi.js / WebGL** overlay (~15 Hz,
  per-target-agent color, on/off toggle); read-only viz. ⚠️ **Magnetic anchors** to create deps by
  dragging are **NOT implemented** (open part of master-plan P17).
- **Task cards** (`TaskCard.tsx`): color-coded agent badges; selectable (twinning); Move + Launch
  menus. No pause/skip/retry buttons; no per-card cost strip.
- **Consent modals** (sandbox broker): `NetConsentModal` (prompt-at-failure) + `FolderConsentModal`
  (prompt+remember) + generic `AgentConsentModal` (cloud exec/patch) + `SandboxModeSelector`
  (per-project) + `WorkingSetCard` + `ConsentBridgePoller` (renderless, keeps ProjectWorkspace
  poller-free). FIFO consent queue (`netConsentModel.ts`).
- **Skills view** (`SkillsView`): folder picker, per-role SKILL.md editor (byte cap + truncation
  guard), on/off toggle, starter-template install; the unified Discover (marketplace + Tools/MCP tab).
- **Labs view** (`LabsView.tsx`, route `labs`, `FlaskConical` nav): on/off switches for **Pigeon**
  (default off) and **Oracle** (default **on**). Semantics = **applies on restart** (the owner confirmed
  restart-only; live on/off is a bigger feature, declined for now).
- **Polis view** (`src/components/polis/PolisView.tsx`, ~22k LOC across the polis modules:
  PolisRenderer, terrain, roadGraph, TradeRouteLayer, possession, props, renderProfile, rng +
  `cityStore.ts` + `types/city.ts`): the isometric living-city map.
- **Settings** (`SettingsView.tsx`): 4 tabs — `Account` · `Providers & Models` · `Workspace & Index`
  · `Security`. Per-model sampling + vendor-seeded defaults; "recommended for your machine";
  main-coder CLI select claude/codex; orchestrator model read-only in SpawnPanel.

---

## 5. Roadmap status (done vs pending)

The master plan covers ONE axis (the self-improving flywheel, P1–P18). Whole epics landed
outside it. Current reconciliation:

**DONE (master-plan phases):**
- **P1–P10 + P10.5** — oMLX wiring, Censor deterministic gate, mini read-only Oracle, mini
  write+allowlist, Seatbelt sandbox, bounded loop, ORPO capture, coder outer loop, Censor live
  tier, Skills rails, **local main coder + Activity Console**.
- **Resource-broker steps 1–8** + **devboule-coder L2** + **Phase 11.1–11.4** + **11.5-B pieces
  1, 2, 4** + **UI-reorg phases 0–2** + **P17 partial** (arrow overlay).

**DONE (epics OUTSIDE the master-plan list):**
- **Skills-marketplace + SKILL.md client + MCP tab** (6 phases, pushed 2026-06-22). §3.8.
- **Work Console** (unified FocusStage + twinning + split-view, pushed 2026-06-24). §4.
- **Projects orchestrator-first redesign** + Plan-Mode Stage/Chat TUI + B1–B16 backlog +
  **cloud-duplex Phase D** (some Phase-D pieces ⚠️UNCOMMITTED). §3.12 / §4.
- **Pigeon** v0.1 (inert/default-off, committed `6632639`). §3.10.
- **Sandbox + permission-broker epic** macOS (Slices 0-3 local + Slice 5 cloud + 5c controls,
  pushed `66eeb5d`). §3.9.
- **Polis** (frontend + backend scanner present on disk). §3.11 / §4.
- **Labs page** (Pigeon + Oracle toggles).

**PENDING — the real forward work:**
- **Go-live stack `Sandbox → Auto-mode → Pigeon`:** sandbox needs **Windows** (Phase 3) +
  live-UX e2e; auto-mode = the broker Slices are the first cut, frontend e2e owed; Pigeon must
  be **wired to live mini-dispatch** (+ v0.2 scheduler).
- **11.5-B piece 3** — per-task pause/skip/retry buttons wired to the runner.
- **P17 remainder** — magnetic anchors (drag A→B sets `depends_on`) + drag-into-column fires a
  column-specific agent.
- **Master-plan P11** (design objective scorers), **P13** (nightly ORPO — blocked on the MoE/dense
  decision + P15 + live GPU), **P14** (nightly SkillOpt — blocked on P11), **P15a** (external
  benchmarks + promotion policy — owner decision), **P16** (training/benchmark UX — parked),
  **P18** (in-app Lab + SkillOpt/Darwin — parked, now discussable post-marketplace).
- **P12 (vocab pruning) — DROPPED** (owner, 2026-06-26). No longer a phase.
- **Tier-C dynamic execution sandbox** (run the project's own test suite inside the sandbox) +
  **Tier-B Joern/CPG** — the unbuilt deep-review substrate (master-plan cross-cutting).
- **Local-coder live e2e** across the board — **GPU-deferred** (sandbox write-loop, ORPO, TDD-strict
  executor, design_request); plus all the new UI needs **the owner's eyes**, and cloud-duplex needs
  **cloud keys**.

---

## 6. The flywheel (how self-improvement is meant to close)

`capable local mini → emit-edits → Censor (deterministic + LLM) validates → accepted
(path,old,new) pairs + rejected-attempt pre-images logged to .aspis-training/pairs.jsonl →
(future P13) nightly ORPO fine-tune → better mini`. Everything left of "nightly ORPO" is live
today; the training loop itself is P13. (Vocab pruning, formerly P12, is no longer part of the path.)

---

## 7. Open bugs — the accurate TODO

**High:**
- **B2** — local oMLX main-coder may never POST to oMLX (silent **Mock** fallback); surface
  "local main coder not configured" instead. Needs a live trace.
- **B5 (broker-era)** — `orchestrator` client behaves like a worker (asks for tasks / over-queries
  Oracle) — role/prompt injection misfiring.
- **B10** — app **freezes/closes after ~5 min** (suspected RAM pressure or a 300 s timer). No repro.

**Medium:**
- **B4** — Codex/missing-binary launch silently hangs (pre-flight detect for all CLIs).
- **B6** — orchestrator over-consults Oracle on trivial tasks; lacks web-search.
- **B16-state** — `Stop` on a finished project resets macro status to **Planned** (should be **Verified**).
- **B18** — manually-created board task is orphaned (no project association → runner ignores it).
- **S7** — Oracle grant for **capable local (oMLX/Ollama) minis** is plumbing-only (wired for Codex
  minis only); Censor still has no Oracle path.

**Low:**
- **B15** — in-app terminal occasionally doubles text (cosmetic).
- **S3** — Claude subscription not selectable as a mini backend.
- **S6** — AppleFM restricted to Censor; widen to all roles.
- **O1** — ×4 repeated "Oracle is starting" warnings at launch.
- **Pre-existing test drift** — `user_mcp_config::tests::oracle_tool_names_list_has_no_drift_from_python`
  (Rust↔Python MCP-tool-name list drift) fails in the full `cargo test --lib`; unrelated to recent work.

---

## 8. Key decisions & invariants (cross-cutting — do not violate)

- **`done` is verifier-only.** The runner sets finished tasks to `review`; only a human/verifier
  moves to `done`. Drag-into-`done` is blocked.
- **Runner "tira dritto":** a dependency is satisfied when the predecessor is `review` OR `done`.
- **Kanban is the single task store.** `.devboule/tasks.json` is retired; plan tasks carry a `plan_id`.
- **Write-mode is capability-driven, not a raw toggle.** Registry tier drives the `Auto` default; an
  explicit user override still wins (Simplify = smart default + keep the manual override).
- **Minis never get user MCP servers**; only main coders do.
- **Censor privacy fail-safe:** all LLM-censor base-URLs are loopback-clamped; file content never
  leaves the device.
- **Sandbox boundary = writes + network, not reads.** Loopback-only this phase; cloud (Claude/Codex)
  is **NOT sandboxed by us** — reuse their native modes + **bridge their prompts into our card**.
- **Pigeon is OPTIONAL + completely disableable** (`pigeon.enabled` default false; separate process).
- **Broker consent UX:** net = prompt-at-failure; folder = prompt+remember; modes = per-project
  selector; Seatbelt can't hot-widen → consent **applies at next spawn**.
- **Unattended mode = fail-closed** (refuses even transient grants) — the Pigeon go-live gate.
- ⚠️ **`ProjectMetadata` persists via HAND-ROLLED YAML frontmatter** (`parse_frontmatter` /
  `replace_frontmatter` / `*_frontmatter_line` in `projects.rs`), **NOT serde** — adding a field
  needs the parse branch + a format `{}` slot + a line helper, else saves are **silently dropped**.
  (Python `replace_frontmatter` must round-trip every field too — a dropped `censor_trusted` was a bug.)
- **Licensing invariant:** no GPL/AGPL/FSL in any bundled component; GPL/LGPL gate tools are
  user-installed + subprocess-invoked only, never bundled. Devboule is sellable closed-source.
- **GPU rule:** never touch the GPU during daytime app sessions; the whole local-coder path is
  **unit-tested only — live e2e is GPU-deferred**.
- **Devboule is multi-language** — never hardcode Rust+TS-only assumptions.
- **Sampling defaults are per model family**; never blanket one temperature; never cap `max_tokens`
  on local models.
- **NEVER run a destructive git command on impulse** to "tidy up" (it discards uncommitted work);
  for a UX-bearing feature, **ask the owner the UX first**; **delegate code to local models**.

---

## 9. Censor / review-experts (app-side summary + pointer)

The app's Censor LLM tier is a **small base model + calibration**, not a fine-tune. The companion
research project **`~/Projects/review-experts`** found an **OOD wall** (trained arms don't beat the
untrained base OOD) — but recent work shows much of it was a measurement artifact; the direction is
a powered AI-code bench → distill+ORPO a non-thinking Qwen2.5-Coder-14B. That's **the other
project's concern**. For *this* app, the integration path is the **deterministic sandwich**: linters
(always) → small local model (opt-in, Nemotron default) → optional cloud escalation. Training
internals stay in `~/Projects/review-experts/docs/` (and the `review-experts-project` memory).

---

## 10. Build / test / run quick-reference

- Branches: **`mac-platform-fixes`** (product) + **`sandbox-epic`** (sandbox/broker/cloud epic, HEAD
  `66eeb5d`, pushed). Current checkout = `sandbox-epic`.
- Rust: `cargo test --lib` (src-tauri + devboule-coder; ~2663 lib green on `sandbox-epic`). Frontend:
  `tsc --noEmit` + `vitest`. Python Oracle: `pytest oracle`. Pigeon: `pytest pigeon -c pigeon/pytest.ini`.
- The local-coder path is **unit-tested only** — live e2e is GPU-deferred; the new UI + cloud-duplex
  + Pigeon-ON need the owner's eyes / cloud keys.

---

## 11. Doc provenance

**This doc folds in and supersedes** (for *current state*) the scattered 2026-06 design/state docs —
keep them as historical detail / deep dives: `resource-aware-orchestration-design`,
`local-main-coder-harness-design`, `phase-11.3-11.4-impl-plan`, `phase-11.5-B-unified-tasks-ux-plan`,
`ui-projects-reorg-plan`, `app-test-bugs`, `design-user-mcp-servers`, `local-model-sampling-defaults`,
`frecce-local-model-param-review`, `censor-model-benchmark`, `local-coder-bug-ledger`,
`work-console-living-plan`, `projects-page-backlog`, `agent-roles-architecture`, `provider-console-roadmap`,
`p5-sandbox-impl-spec`, and the **plan files** under `~/.claude/plans/` (`snuggly-hopping-pizza` =
local broker, `piped-gathering-rocket` = cloud adapters, `twinkly-honking-salamander` = Pigeon,
`playful-stirring-toast` = skills marketplace).

**Design docs for the big surfaces:** `aspis-bio-polis-map.md` (Polis) + `Polis-handoff/`.

**Stays authoritative (not folded):**
- `master-plan-2026-06-self-improving-mini-design.md` — the forward phase list (P12 vocab-pruning
  DROPPED 2026-06-26; status banner reconciles the out-of-plan epics).
- `local-coder-AGENTS.md` — the live system prompt for local coders.
- The review-experts / Censor-training docs — owned by `~/Projects/review-experts`.

---

*Updated 2026-06-26. The original (2026-06-21) was a code-verified audit on `mac-platform-fixes`;
this revision adds the post-21/06 epics (Skills-marketplace, Work Console, Projects-redesign,
Pigeon, Sandbox/broker, Polis, Labs) verified against the file tree + git HEAD on `sandbox-epic`
and the session commits. When this doc and the code disagree, fix the doc.*
