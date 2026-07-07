# Devboule-on-pi — Architecture Design

**Author:** (agent) + M
**Date:** 2026-07-07
**Status:** Draft — pending M review before ANY code changes
**Decision model:** Route 2 (SDK embed, OpenClaw style)

---

## 1. TL;DR

Devboule stops being a custom agent harness and becomes a **product built on top of pi's engine**, exactly like OpenClaw did. The fragile ratatui TUI + custom agent loop (`devboule-coder/`, ~22K LOC) gets deleted. Everything that is genuinely Devboule value — the Tauri/React app, the Censor, the Oracle RAG, the mini-coder delegation, the Pigeon routing — **stays**, and is re-exposed to pi as custom tools, hooks, and subagents. pi becomes the harness (battle-tested: "can't edit before read", agent loop, tool dispatch). Devboule becomes the product shell + the domain-specific tooling on top.

---

## 2. Vision

Today the user talks to a **custom ratatui TUI** that drives a **custom agent loop** (planner, executor, model client). Two fragile layers we maintain ourselves.

Tomorrow the user talks to the **Devboule Tauri/React app**, which embeds **pi's engine** (via the pi SDK) as the agent loop. Devboule's domain logic (Censor, Oracle, mini-coder, Pigeon) is registered INTO pi as tools/hooks/subagents. pi does the reasoning; Devboule provides the product and the specialist capabilities.

**Precedent (real):** OpenClaw ([github.com/openclaw/openclaw](https://github.com/openclaw/openclaw), 500+ stars) imports `@mariozechner/pi-coding-agent` via `createAgentSession()` and builds a full multi-channel AI assistant product on top. It replaces built-in tools, customizes the system prompt, injects channel tools, adds auth failover. Same pattern, proven in production.

---

## 3. Why Route 2 (SDK embed), not Route 1 (pi package)

| | Route 1 — `pi install devboule` | Route 2 — SDK embed (chosen) |
|---|---|---|
| Devboule is | an extension of pi's CLI | a standalone product, pi is a library inside |
| "Mandatory" loading | ❌ no flag (global dir only) | ✅ the app IS the harness |
| Custom UI in Tauri/React | ↩️ must wrap pi's CLI output | ✅ full control, native Tauri UI |
| Oracle Python MCP | ⚠️ manual tool proxy (no `registerMCPServer`) | ✅ full control, can wrap MCP cleanly |
| Branding | "pi + devboule extension" | "Devboule" (pi invisible to user) |

**Route 1 only makes sense if we want Devboule to run *inside other people's pi CLIs*.** We don't. Devboule is a standalone Tauri product. → Route 2.

---

## 4. Target Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│  Devboule Tauri App (existing, KEEP)                               │
│                                                                     │
│  ┌───────────────────┐        ┌─────────────────────────────────┐  │
│  │  React/TS Frontend│◄─Tauri─│  Rust Backend (src-tauri)       │  │
│  │  (src/, ~130K)    │  IPC   │  - projects, settings, commands │  │
│  │  Work Console,    │        │  - Censor (KEEP, ~25K)          │  │
│  │  Kanban, Settings │        │  - mini_coder executor (ADAPT)  │  │
│  └───────────────────┘        │  - Oracle client (KEEP)         │  │
│                               │  - Pigeon client (KEEP)         │  │
│                               └──────────┬──────────────────────┘  │
│                                          │ spawns (Tauri sidecar)  │
│                               ┌──────────▼──────────────────────┐  │
│                               │  Node.js Sidecar (NEW)          │  │
│                               │  ┌────────────────────────────┐  │  │
│                               │  │  pi SDK                    │  │  │
│                               │  │  createAgentSession()      │  │  │
│                               │  │  subscribe() → events      │  │  │
│                               │  └─────────────┬──────────────┘  │  │
│                               │                │ registers       │  │
│                               │  ┌─────────────▼──────────────┐  │  │
│                               │  │  Devboule Extension (NEW)  │  │  │
│                               │  │  - registerTool: oracle_*  │  │  │
│                               │  │  - registerTool: spawn_mini│  │  │
│                               │  │  - on(tool_result): Censor │  │  │
│                               │  │  - on(before_agent_start): │  │  │
│                               │  │      Pigeon model routing  │  │  │
│                               │  │  - subagent: main/mini     │  │  │
│                               │  └────────────────────────────┘  │  │
│                               └──────────────────────────────────┘  │
│                                                                     │
│  Spawns (existing, KEEP):                                          │
│   - Python Oracle (MCP over stdio) ◄── called by oracle_* tools    │
│   - Pigeon FastAPI (HTTP)          ◄── called by routing logic     │
└─────────────────────────────────────────────────────────────────────┘

DELETED:
  devboule-coder/ (entire crate, ~22K LOC)
    ├─ app.rs + terminal.rs (ratatui TUI, 656L)
    ├─ agent_loop.rs (1.9K)      → pi's agent loop
    ├─ executor.rs (1.8K)        → pi's tool dispatch
    ├─ planner.rs (4.7K)         → pi's planner
    ├─ model_client.rs (2.2K)    → pi's model client
    ├─ rmcp_backend.rs (0.8K)   → pi's MCP client
    ├─ runner.rs (2.0K)         → pi subagent orchestration
    └─ activity/steer (0.8K)    → Tauri IPC + pi events
```

---

## 5. What gets deleted

The entire `devboule-coder/` crate (~22K LOC). It is a self-contained orchestrator binary whose every responsibility is now owned by pi:

| File | LOC | Replaced by |
|---|---|---|
| `app.rs` + `terminal.rs` | 656 | Tauri/React frontend (already exists) |
| `agent_loop.rs` | 1,888 | pi `AgentSession` agent loop |
| `executor.rs` | 1,787 | pi tool dispatch |
| `planner.rs` | 4,662 | pi planning |
| `model_client.rs` | 2,238 | pi model client |
| `runner.rs` | 2,015 | pi subagent orchestration |
| `rmcp_backend.rs` / `multi_mcp.rs` | ~800 | pi MCP client |
| `activity.rs` / `steer.rs` | ~800 | Tauri IPC + pi event stream |
| session_persist, preplan, reply_stream, conversation | ~7K | pi session + events |

The Tauri backend code that **launches** devboule-coder (`projects.rs` orchestration launcher, `main_coder.rs` command) is simplified, not deleted — it now launches the Node sidecar instead.

---

## 6. What stays and how it maps to pi

| Devboule subsystem | LOC | Stays? | How it integrates with pi |
|---|---|---|---|
| Tauri app (frontend + backend) | ~300K | ✅ KEEP | Becomes the product shell + UI. Spawns the Node sidecar. |
| **Censor** (code review) | ~25K | ✅ KEEP | Registered as a `tool_result` hook in the Devboule extension: after any `edit`/`write` tool, run Censor, surface findings to the agent. |
| **Oracle** (Python RAG MCP) | ~48K | ✅ KEEP | Spawns as today (MCP over stdio). Exposed to pi as `oracle_ask`/`oracle_context`/`oracle_find` custom tools in the extension (thin proxy → MCP). |
| **Mini coder** (delegation) | ~8K | ✅ KEEP + ADAPT | The mini stays a delegation target. Instead of `spawn_mini_coder` MCP tool → custom executor, pi calls a `spawn_mini` custom tool (or a pi **subagent**) that runs the bounded local-model write + Censor review + fix-once loop. |
| **Pigeon** (agent routing) | ~1.4K | ✅ KEEP | The auto local-vs-cloud routing becomes a `before_agent_start` hook in the extension: inspect the task, call `pi.setModel()` to pick local (oMLX/Ollama) or cloud (Claude/Codex) per the Pigeon policy. |
| Main coder / agentic loop | (was in devboule-coder) | ✅ concept KEEP, impl DELETE | "Main coder" becomes a pi **subagent** (multi-turn tool-using worker), built on the `examples/extensions/subagent/` pattern. |

---

## 7. The bridge: Tauri ↔ Node sidecar (pi SDK)

> **Resolved** — feasibility study complete. Findings below; companion note at `docs/devboule-on-pi-bridge-feasibility.md`.

**Verdict: MEDIUM, feasible.** All building blocks exist; the main risk is streaming latency Node→Rust→React.

- **Node bundling (solved):** Tauri 2 has an official Node.js sidecar guide — use `pkg` (`@yao-pkg/pkg`) to compile the Node app + pi SDK + Node runtime into one standalone binary. End users need **zero** Node install. (Alt: `bun build --compile` / `deno compile`.) Ref: <https://v2.tauri.app/learn/sidecar-nodejs/>
- **IPC mechanism (recommended):** **stdio JSONL** — it's exactly what `pi --mode rpc` already speaks, zero additional infrastructure, Tauri-native. Optional second channel: local HTTP (like the Python Oracle already uses) for richer bidirectional steering mid-stream. `tauri-plugin-shell`'s `Command.sidecar()` captures stdout events natively.
- **pi SDK streaming (confirmed):** `createAgentSession()` + `subscribe()` expose `text_delta`, `tool_execution_start/update/end`, `message_start/end`, `agent_start/end`, `turn_start/end`, `compaction_*`, `auto_retry_*` — everything needed to render agent activity in the React frontend. Ref: `docs/sdk.md` lines ~73-127.
- **Reuse existing code (confirmed):** the app ALREADY spawns long-lived subprocesses with `std::process::Command` + `Stdio::piped()` — Python Oracle (`src-tauri/src/oracle/python_oracle.rs:7,1539-1560`, HTTP at `127.0.0.1:<port>`, supervisor in `oracle_service.rs`) and the devboule-coder binary (resolved via `resolve_orchestrator_binary()` `projects.rs:8983`, launched in `prepare_or_launch_project_agent` `projects.rs:5715`, file bridges via `activity.rs`/`steer.rs`/`.aspis-agents.json`). The same `std::process::Command` pattern hosts a Node sidecar with minimal change.
- **Two integration paths (pick one):**
  - (1) `tauri-plugin-shell` sidecar — official, cleaner, but the app does NOT use it today (zero grep hits).
  - (2) Raw `std::process::Command` (as for Oracle) — simpler, already proven in this app, zero new deps.
- **Largest risk:** streaming latency of fine-grained `text_delta`/`tool_execution_*` events across Node→Rust→React. Mitigation: Rust forwards raw JSONL to the frontend via `app.emit()` (Tauri events), let React transform/render — no per-event Rust processing.
- **Open sub-questions:** (a) pi sidecar long-lived daemon (like Oracle) vs per-session spawn? `createAgentSession()` is cheap. (b) Who owns `~/.pi/agent` config — the app or the user? (c) Raw JSONL forward vs Rust-transform-first?

**Likely winner:** path (2) raw `std::process::Command` + stdio JSONL + `app.emit()` to React, with `pkg`-compiled binary for release distribution. Matches the existing Oracle pattern most closely.

---

## 8. Devboule concept → pi concept mapping

| Devboule concept | Becomes (in pi) | pi mechanism |
|---|---|---|
| Custom agent loop | pi agent loop | `createAgentSession()` |
| Custom planner | pi planning | built-in |
| Tool dispatch (executor.rs) | pi tool dispatch | built-in |
| Oracle `oracle_ask` etc. | custom tools | `pi.registerTool()` |
| Mini coder delegation | custom tool or subagent | `pi.registerTool()` + subagent example |
| Main coder (agentic) | subagent | `examples/extensions/subagent/` |
| Censor post-write review | lifecycle hook | `pi.on("tool_result")` |
| Pigeon local/cloud routing | lifecycle hook | `pi.on("before_agent_start")` + `pi.setModel()` |
| Model client (oMLX/Ollama) | pi providers | `pi.registerProvider()` |
| ratatui TUI | Tauri/React frontend | (existing, kept) |
| activity/steer file bridges | Tauri IPC + pi events | (simplified) |
| "can't edit before read" rule | pi built-in | (free — pi already enforces) |

---

## 9. Migration phases

1. **Phase 0 — Spike ✅ (bfd11bb):** minimal Node sidecar embeds pi SDK, registers `oracle_ask` tool, streams to React via Tauri IPC. Bridge proven end-to-end with `openrouter/tencent/hy3:free`.
2. **Phase 1 — Tool surface + infra ✅ (ea015be→c7c58e7):** per-session IDs, real vault→env adapter, UI trigger in WorkConsole. Oracle: removed canned `oracle_ask` (MCP already configured via `~/.pi/agent/mcp.json`). Censor LLM hook: `tool_execution_start`→`tool_execution_end`→`agent_end`, `DEVBOULE_CENSOR_REVIEW_ENABLED` toggle, uses correct pi SDK event fields.
3. **Phase 2 — Pigeon routing ✅ (8f8ade7):** multi-factor heuristic classifier (`classify_capability_needed()`, adapted from Puppetmaster MIT) in Rust: role base score, hard/easy/UI signal patterns, clamp 5→100 → `PromptTier` (`Cheap|Moderate|Expensive`). `AgentPath` (`Pi|Terminal`) for Claude. Vault-aware tier table. Sidecar JSONL protocol: `classify_prompt`→`classified`→`setModel()`. Self-learning bandit = TODO (Phase 5).
4. **Phase 3 — Subagents ✅ (096ec2a):** `main-coder.md` (full tools, cloud) + `mini-coder.md` (budget worker, local). Pi agent `.md` definitions in `.pi/agents/`. User/Pigeon spawns main-coder session; main-coder spawns mini subagents.
5. **Phase 3.5 — Reviewer/Verifier ✅ (now):** task-level mandatory reviewer subagent (`.pi/agents/reviewer.md`). Architecture per M:
   - **During writes**: Censor A (pi-lens, deterministic, instant) + Censor B (LLM, optional) fire after EACH file write.
   - **After task complete**: Reviewer (NOT optional) verifies the ENTIRE task: `git diff` → read files → construct targeted tests → execute (test/lint/type-check) → report (Critical/Warnings/Suggestions) + verdict (✅ VERIFIED / ⚠️ NEEDS FIX / ❌ FAILED). The deterministic slow tests run INSIDE the reviewer's verification pass — the reviewer orchestrates them, it doesn't just review code passively.
   - Model: `auto` (Pigeon → Moderate tier). Tools: `read, grep, find, ls, bash, run`.
6. **Phase 4 — Delete:** remove `devboule-coder/` crate and its launcher once all flows are covered. ~22K LOC gone. **IRREVERSIBLE — requires test pass before proceeding.**
7. **Phase 5 — Polish:** session persistence, error handling, packaging of the Node runtime (`pkg --sea`), self-learning bandit for Pigeon, sandbox wrapping for main-coder sidecar.

Each phase is independently shippable and reversible until Phase 4.

---

## 10. Risks

| Risk | Severity | Mitigation |
|---|---|---|
| Node runtime must ship to end users (no Node installed) | HIGH | Feasibility study picks bundling strategy (binary bundle vs compile vs prerequisite). |
| Rust↔Node IPC latency / complexity | MEDIUM | stdio JSONL is proven by `pi --mode rpc`; reuse existing subprocess pattern. |
| pi SDK API churn (pi is actively developed) | MEDIUM | Pin pi version (like OpenClaw pins `0.61.1`); upgrade deliberately. |
| Censor hook performance (runs after every write) | MEDIUM | debounce / async / per-file batching as today. |
| Oracle MCP wrapping (no `registerMCPServer`) | LOW | thin `registerTool` proxies — workable. |
| Losing the deterministic DAG runner (`runner.rs`) | LOW-MED | pi subagents + the Kanban can replace it; confirm in Phase 3. |
| Behavioral parity — pi's agent may reason differently than the custom loop | MEDIUM | accept it; pi is more battle-tested, that's the point. Tune via system prompt + hooks. |
| **Claude external-MCP blocked** (2026-07) — Claude no longer allows external MCP integrations inside its agent, so Claude **cannot run as a provider inside pi-dev**. | **HIGH** | Keep the existing **Claude-terminal subprocess path** as-is for Claude sessions. pi-dev hosts the non-Claude agents (main/mini via pi providers: local oMLX/Ollama, Codex/OpenRouter, etc.). Pigeon routes not just local-vs-cloud but also **pi-path vs terminal-path**: Claude → terminal subprocess (unchanged), everything else → pi sidecar. The eventual "Claude on pi" integration is **future work** (decide at a later phase, not Phase 0). |

---

## 11. Decisions (M, 2026-07-07)

Resolved by M (this section overrides the earlier open questions):

1. **Oracle — KEEP.** pi-lens is NOT a replacement; the Oracle RAG (Python) stays as the custom tool backend. pi-lens does not enter this architecture at all.
   **2026-07-07 update (verified by recon):** Oracle already exposes itself to pi via standard MCP — no custom-tool proxy needed. Two FastMCP stdio servers: `oracle/server/mcp_handler.py` (7 tools) + `oracle/server/aspis_mcp.py` (~25 tools). pi is already configured via `~/.pi/agent/mcp.json` (`oracle-figlyph`, eager) and `.pi/mcp.json` (project-level, with `directTools`). The sidecar's canned `oracle_ask` is redundant; Step 2 uses the existing MCP config, no proxies to build.
2. **Censor — SPLIT & RECYCLE.** Deterministic Part A → pi-lens (MIT). Devboule keeps only the LLM Censor (Gemma).
   **2026-07-07 update (verified by recon):** a Censor-style pi extension already exists at `~/.pi/agent/extensions/rust-reviewer.ts` (toggleable via `OMLX_REVIEW_ENABLED`). It has the right hooks (`tool_result` + `agent_end`) + CLR ensemble voting (K=5, temps). BUT it calls oMLX HTTP directly, NOT the Rust Censor backend (`src-tauri/src/backend/censor/gemma.rs`). Step 2: reuse hook pattern + voting; rewrite transport to call Rust GemmaClient (via Tauri command) so Censor respects vault config.
3. **Pigeon — KEEP & UPGRADE.** Pigeon becomes Devboule's automatic "OpenClaw-style" router (still alpha). It is the thing that recognizes local vs cloud agents and routes automatically. Lives in the extension as `before_agent_start` + `setModel()`. **Post-2026-07 update:** Pigeon also routes **pi-path vs terminal-path** — see decision #10 (Claude). It is no longer a pure local/cloud switch; it also decides whether an agent runs inside the pi sidecar or in the legacy Claude-terminal subprocess.
   **2026-07-07 Phase 2 design (verified by recon):** Pigeon today is a pure message-passing mailbox (`pigeon/dispatcher.py` — send/poll/done/fail) with **zero routing intelligence**. The Rust backend already has reusable pieces: `model_registry.rs:25-48` (model tier schema: `tier`, `roles`, `context_window`), `task_size.rs:47` (token estimation), `cost.rs:221` (cost estimation), `DirectiveTier::Mini|Main` (plan-level routing in `mini_coder.rs:401`). **Phase 2 builds on Rust:** (i) two new enums — `PromptTier` (`Cheap|Moderate|Expensive`) + `AgentPath` (`Pi|Terminal`); (ii) heuristic classifier (prompt length + regex complexity → tier → provider/model from registry); (iii) `before_agent_start` hook in the sidecar calls Rust → `setModel()`/`setProvider()` or forwards to Claude-terminal. **Self-learning = threshold-finding (bandit), NOT model ranking:** parallel cheap+Claude runs, compare via Censor/human review, adjust the complexity threshold where cheap is "good enough" — cheap fails → raise threshold; passes → lower. Claude is ground truth, not competition. Pigeon stays Python HTTP (existing process spawn in `pigeon_service.rs`), routing logic lives in Rust for zero-latency hook calls.
4. **Integration path — RAW `std::process::Command`** (like the existing Python Oracle / devboule-coder launchers). Do NOT adopt `tauri-plugin-shell`. Matches the proven in-app pattern; zero new deps.
5. **Node runtime bundler — `@yao-pkg/pkg --sea --compress Zstd`.** Bundle Node + pi SDK into one standalone binary so end users need nothing installed. Chosen over Bun/Deno/nexe after a perf + compatibility comparison (2026-07-07):
   - **pkg SEA + Zstd**: stock V8 (same engine as dev/test), native addons automatic, Node CVE fixes same-day, 152 MB binary, 570 ms cold start (irrelevant for a long-lived sidecar), cross-compiles to all Tauri targets. **Safest, zero compat surprises.**
   - Bun `--compile`: smaller (108 MB) + faster cold start (510 ms), but uses JavaScriptCore (not V8) — behavioral-divergence risk on a project tested on Node; `node-pre-gyp` native addons need manual pinning. Backup option only if binary size becomes critical AND pi's dep tree is verified free of `node-pre-gyp`.
   - Deno `compile --bundle`: **blocked** — confirmed bug fails on `@earendil-works/pi-coding-agent` (`deno/deno#34937`).
   - nexe: unmaintained, build broken on Node 20+. Rejected.
   - **Bundle Node inside the app** (M confirmed): whoever installs Devboule has everything, zero prerequisites.
6. **Mini coder / Main coder — YES.** Mini = bounded one-shot **tool**. Main = multi-turn agentic **subagent** (adapt `examples/extensions/subagent/`).
7. **Sidecar lifecycle — per-session (tentative).** `createAgentSession()` is cheap; spawn a sidecar per Devboule coding session rather than a long-lived daemon. M flagged this as "maybe" — confirm during Phase 0 spike (measure spawn cost + warm-start benefit).
8. **Branding — credit pi.** Devboule must disclose it is "built on top of Pi" (pi is MIT). Pi otherwise invisible in the UX.
9. **Config silo (AI providers) — RESOLVED → (b) vault stays source of truth, adapter at spawn.** Devboule's AI-provider config (`ProvidersModelsTab.tsx` → vault via `save_oracle_llm_settings`, `src-tauri/src/backend/vault.rs:927`) is the single source of truth and **stays**. pi's own provider system (`pi.registerProvider()` / `pi.setModel()` / `~/.pi/agent/models.json`) is **NOT written by Devboule**. Instead: the Rust backend (which already reads the vault and already spawns the sidecar via `std::process::Command`) resolves the relevant role's provider+model and passes it to the Node sidecar at spawn time (env var or JSONL handshake); the Devboule pi extension calls `pi.setProvider()`/`pi.setModel()` on session start. Rationale: (i) `~/.pi/agent/models.json` is the user's GLOBAL pi config — writing to it per-project would clobber their pi CLI setup; (ii) Devboule's value is per-role provider assignment (Oracle / Censor / Designer / coder / mini each their own), which pi's per-session `setModel()` would flatten; (iii) one edit surface (vault → ProvidersModelsTab) matches the "pride" UX; (iv) the adapter is minimal — Rust already spawns the sidecar, passing one extra config blob is trivial. **Scope note:** this only affects pi-driven agents (main coder, mini-as-subagent). Oracle, Censor LLM, Designer read the vault directly and are unchanged.
10. **Claude external-MCP block (2026-07) — FUTURE, do not block Phase 0.** Claude recently blocked external MCP integrations, so Claude **cannot be used as a provider inside pi-dev**. Consequence: the existing **Claude-terminal subprocess path stays as-is** for Claude sessions — it is NOT migrated onto pi in this plan. pi-dev hosts the non-Claude agents (main coder, mini-as-subagent, and any pi-supported provider: local oMLX/Ollama, Codex/OpenRouter, etc.). Pigeon therefore routes on **two axes**: (a) local-vs-cloud as before, and (b) **pi-path vs terminal-path** — Claude → terminal subprocess (unchanged), everything else → pi sidecar. The eventual integration of Claude onto pi is **future work** to be scoped in a later phase, NOT part of Phase 0–4.
11. **macOS sandbox (coder sandbox) — REUSE WITH ADAPTATION (verified 2026-07-07).** The sandbox (`src-tauri/src/backend/sandbox/`, ~400 LOC: `mod.rs` `SandboxPolicy`/`wrap()`, `seatbelt.rs` SBPL profile builder with kernel regression tests) is independent of the deleted `devboule-coder/` (never imported it). macOS `sandbox-exec` + Seatbelt: confines file writes (deny-by-default + allowlist), network, rlimits, `.git`/`.devboule` write guards. **Reuse:** in `pi_sidecar.rs`, wrap the bare `Command::new("node")` with `sandbox::wrap()`, confining pi's `edit`/`write`/`bash` to the project dir. `wrap()`/`build_profile()` need NO changes — add only a policy helper. macOS-only for OS confinement; pass-through (unrestricted) on Linux/Windows.

### Updated delete/keep table (post-decisions)

| Subsystem | Before decision | After decision |
|---|---|---|---
| devboule-coder crate (~22K) | DELETE | DELETE (unchanged) |
| Censor deterministic Part A (~most of 25K) | KEEP | **DELETE** → replaced by pi-lens (MIT) |
| Censor LLM (Gemma) | KEEP | KEEP (optional, the recycled Censor) |
| Oracle (Python RAG, 48K) | KEEP | KEEP (unchanged; pi-lens irrelevant) |
| Pigeon | KEEP | KEEP + UPGRADE (becomes the auto-router, alpha) |
| Mini coder | KEEP+ADAPT | mini = **tool** |
| Main coder | (was in devboule-coder) | main = **subagent** |
| Tauri app (frontend+backend) | KEEP | KEEP (unchanged) |

### Added 2026-07-07 (Console UX + Skills survival check)

Two more Devboule-specific UX pieces verified to survive the migration (read-only recon):

1. **Console UX (live websearch, Living Plan, streaming activity) — SURVIVES WITH ADAPTATION.** UI components change **zero lines**: `WorkConsole.tsx`, `LivingPlan.tsx`, `FocusStage.tsx`, `agentConsoleModel.ts`, `useAgentConsole.ts` already subscribe to Tauri events (`mini-activity://<agentId>`). The only adaptation: the data source swaps from the deleted `devboule-coder/src/activity.rs` file bridge → pi SDK streaming events (`text_delta`, `tool_execution_*`) forwarded via `app.emit()`. The Rust backend converts pi events to the existing `MiniActivityEvent` schema. React hooks are untouched. Websearch rows (`WebSearchEntry`), the Living Plan agent tree, and streaming tool/diff/censor rendering all carry over.
2. **Skills system — SURVIVES WITH ADAPTATION.** The 30+ Tauri commands + Rust storage (`project_skill.rs`, `global_skills.rs`, `skill_format.rs`) live in `src-tauri/`, NOT in `devboule-coder/` → they stay. UI (`SkillEditor.tsx`, `SkillsToolsModal.tsx`, `LibrarySearch.tsx`) stays, including the role-toggling value-add pi doesn't have. Format is `SKILL.md` (agentskills.io) — **identical to pi's own skills** → compatible, not conflicting. Adaptation: runtime injection moves from deleted `devboule-coder/src/model_client.rs` to the pi extension (`setSystemPrompt` prefix + a `load_skill` custom tool). **Merge needed**: Devboule reads project-local `.claude/skills/<role>/`; pi reads `~/.pi/agent/skills/` (user-global). The Devboule extension injects both.

Files kept (added to the keep list): all `src/components/work/*` Console components, `src/components/agents/agentConsoleModel.ts` + `useAgentConsole.ts`, `src/components/work/SkillEditor.tsx` + `SkillsToolsModal.tsx` + `LibrarySearch.tsx`, `src-tauri/src/backend/{project_skill,global_skills,skill_format,mini_activity}.rs`. Files that change (not delete): `mini_prompt.rs` (skill injection → pi), `agentChannel.ts` (steer target → pi proxy). Delete candidates in `devboule-coder/`: `activity.rs`, `steer.rs`, `skills.rs`.

### Added 2026-07-07 (Polis + Providers survival check)

Two more Devboule-specific pieces verified to survive the migration (read-only recon):

1. **Polis (codebase city-map) — SURVIVES AS-IS.** Pure frontend renderer + Rust file scanner; zero references to `devboule-coder/`. Backend `src-tauri/src/polis/` (11 files: `commands.rs`, `scanner.rs`, `watcher.rs`, `cloud.rs`, ...) depends only on `crate::backend::{model,state,fs_watch,agents,providers}` and the Oracle (`crate::oracle::commands::ask_oracle`, used for reclassification only) — all of which survive. The scanner reads the project folder (file walk + import graph), not orchestrator state. Frontend `src/components/polis/` (`PolisView.tsx`, `PolisRenderer.ts` PixiJS, `PolisBottomBar.tsx`, `store/cityStore.ts`) talks exclusively to Tauri commands registered in `src-tauri/src/lib.rs`. **Verdict: zero lines change.** M was right — Polis is attached to the project folder + Oracle, not the agent harness.
2. **Providers page — SURVIVES AS-IS (the pride stays).** Two provider surfaces exist, both independent of `devboule-coder/`:
   - **`ProvidersView`** (`src/components/views/ProviersView.tsx:254-695`, routed `App.tsx:172`): Cloudflare + Scaleway infrastructure management — account tokens, Workers inventory, deployment health, secret rotation; Scaleway instances/GPU/serverless/object storage/billing. Backed by `save_provider_token` / `save_provider_scope` / `get_provider_health` → `src-tauri/src/backend/providers.rs`; `ProviderId = Cloudflare | Scaleway` (`backend/model.rs:5-38`). Pure Devboule domain logic; **pi has no equivalent concept** → SURVIVES AS-IS, zero lines change.
   - **`ProvidersModelsTab`** (`src/components/settings/ProvidersModelsTab.tsx:297`, in Settings): AI model providers for Devboule roles (Scaleway, Infomaniak, Mistral, oMLX, Ollama for Oracle; Censor; Designer; coders). Persists to Devboule vault via `save_oracle_llm_settings` (`backend/vault.rs:927`), NOT to pi's config. SURVIVES AS-IS but carries the **config-silo flag** → see decision #9 above.

Files kept (added to the keep list): `src-tauri/src/polis/` (all 11 files), `src/components/polis/` (all 4 files), `src/components/views/ProviersView.tsx`, `src-tauri/src/backend/providers.rs`, `src/components/settings/ProvidersModelsTab.tsx`, vault provider settings. **No files in `devboule-coder/` are referenced by any of these.**

---

## 12. Companion study

- **Tauri sidecar + Node bridge feasibility** — complete. Key findings folded into §7 above. Full evidence: Tauri 2 supports Node sidecars via `pkg`-compiled binary (<https://v2.tauri.app/learn/sidecar-nodejs/>); stdio JSONL is the recommended IPC (matches `pi --mode rpc`); existing `std::process::Command` pattern (`oracle/python_oracle.rs:1539`, `backend/projects.rs:5715`) hosts the Node sidecar with minimal change; pi SDK `subscribe()` exposes all streaming events (`docs/sdk.md:73-127`). Verdict: MEDIUM, feasible.

---

**No code has been changed.** This document is for M's review. Decision needed on §11 before Phase 0 spike begins.
