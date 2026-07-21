# E2E AFK test — Devboule agent stack (2026-07-21)

**Mode:** observe-only (no product fixes). Owner AFK.  
**App:** Tauri debug + `ui-pilot`, `DEVBOULE_DEV_UNLOCK=1`, Vite `:1420`.  
**Evidence root:** `{SCRATCH}` =  
`/var/folders/7v/4dqj9qqs1q1fb3qllxbkqkbh0000gn/T/grok-goal-1f51c2fe4b41/implementer`  
(see `00_evidence_index.txt` for file list).

**Config snapshot (repo `config.json`):**
- Orchestrator / Main / Mini backends: **Cloud OpenRouter** (`kind: cloud`).
- `pigeon` key: **absent** (treated as disabled).
- Launch log: `Pigeon disabled (config pigeon.enabled=false); not starting.`

**Live tools used:** `tauri-pilot` ping/snapshot/eval/ipc/screenshot; MCP `devboule_pilot__pilot_ping`; disk reads of agents ledger, activity JSONL, Oracle discovery; static code path tracing for RCA.

---

## Matrix summary

| Area | Live? | Outcome | Findings |
|------|-------|---------|----------|
| Tauri unlock | Yes | Pass (dev unlock) | B15 |
| Project board / select | Yes | Partial | B10, B12, B13 |
| Oracle index (`devboule` folder) | Yes | Fail | **B01, B02** |
| Orch planning + questions | Partial | Blocked mid-session | B07, B08, B09 |
| Bottom console chat | Yes | Compression OK; residual bad session | B14, B04 |
| Main coder | Structural + residual | Skipped by orch | B04, B16 |
| Mini coder | Residual live + code | Fail | **B03**, B05 |
| Tools | Residual + role_rules | Mixed | B04–B06, B17 |
| Async without Pigeon | Yes (default) | Confirmed | B11 (off side) |
| Sync with Pigeon | Attempted | Blocked / incomplete | **B11** |

---

## Findings (with investigation)

### B01 — Oracle does not index selected workspace (`devboule`)

| Field | Value |
|-------|--------|
| **Area** | Oracle |
| **Severity** | **blocker** |
| **Verdict** | **BUG** |

**What we saw**

- Oracle admin: workspace `/Users/user/Projects/devboule`, badge “✓ Workspace is set”.
- Counters: Indexed **0**, Pending **0**, Vectors **0**, Chunks **0**.
- Banners: **“Oracle index watcher failed to start.”** and **“Oracle dense index job failed to start.”**
- Click **Index now** → still 0 after wait; mixed lines “Oracle server: running” vs “Oracle is starting — the server is not ready yet.”
- Evidence: `{SCRATCH}/03_oracle_body.txt`, `08_after_index_now.txt`, `10_projects_view.txt`.

**Cause chain (investigated)**

1. **UI is only an HTTP client to the resident Oracle server.**  
   `startOracleIndexJob` / `startOracleIndexWatcher` in `AppContext.tsx` invoke Tauri commands that POST to the resident server (`oracle/commands.rs`):
   - `start_oracle_index_job` → `POST /index/run?...&manual=...`
   - `start_oracle_index_watcher` → `POST /index/watch/start?...`  
   Failures surface as the exact banner strings when the invoke rejects (`AppContext.tsx` ~857–886).

2. **Those POSTs need a live server process.**  
   Discovery file `src-tauri/projects/.oracle-server.json` pointed at `http://127.0.0.1:31520` with **pid 891**.  
   Live check: **curl connection refused**; **no listener on 31520**; pid 891 **not running**.  
   So “Index now” cannot succeed: the client talks to a dead endpoint.

3. **Lifecycle is supposed to (re)start the server on unlock.**  
   `oracle_service::on_unlock()` calls `start_supervisor()` (best-effort; never blocks unlock).  
   Dev unlock starts the app unlocked immediately — supervisor should run — but discovery was **stale from a previous session** (`updatedAt` stuck earlier) and was **not rewritten** with a live child for this process.  
   So either: supervisor did not spawn a healthy server, or published/kept a dead discovery file (see B02).

4. **“Index now” does not heal a dead server.**  
   The command path assumes `oracle_http_post_blocking` can reach the resident server; it does not re-spawn the server first when the HTTP client fails. Result: permanent 0% index UI until something else brings Oracle up.

**Confidence:** high that the user-visible failure is “no live Oracle HTTP server → index/watch commands fail”. Medium on *why* supervisor failed to replace the dead process this boot (needs server stderr / supervisor tick logs; not captured).

**Owner half-bug:** **confirmed and refined** — not just “indexing slow”; the retrieval server endpoint is dead and jobs never start.

---

### B02 — Stale Oracle discovery / false “server running”

| Field | Value |
|-------|--------|
| **Area** | Oracle / Tauri |
| **Severity** | major |
| **Verdict** | **BUG** |

**What we saw**

- Discovery JSON still advertising dead pid/port (`09_oracle_discovery.json`).
- UI HEALTH: “Oracle server: running” while curl fails; also “not ready yet” from file browser.

**Cause chain**

1. Discovery is a **file contract** (`.oracle-server.json`) written when the supervisor publishes a live server (`oracle_service` DISCOVERY_FILENAME).
2. **Vault lock no longer tears down Oracle** (`on_lock` intentionally empty) so discovery is process-scoped — but **app restart** must replace or delete stale discovery. Leaving an old file after a kill/crash makes every client (UI + MCP) believe a server exists.
3. UI status aggregation likely treats “discovery file exists / last known good” or a partial status poll as “running” without verifying TCP liveness + matching pid.

**Why it matters:** agents and “Index now” both fail closed or hang on a ghost server; operator thinks Oracle is up.

**Confidence:** high on stale file + dead TCP; medium on exact UI status predicate (not fully reverse-engineered).

---

### B03 — Cloud mini-coder fails: “directive executor does not support it yet”

| Field | Value |
|-------|--------|
| **Area** | mini-coder |
| **Severity** | **blocker** (OpenRouter mini) |
| **Verdict** | **BUG** |

**What we saw**

- Directive `edbe87a2…`:
  - `parentAgentId`: `orchestrator-openrouter-mock`
  - `write: true`, `writeMode: "agenticIterative"`
  - `status: failed`
  - `result.error`: **`cloud backend runs via the pi engine; the directive executor does not support it yet`**
- Project file `devboule-openrouter-mock/index.html` **never** got the T1 subtitle.
- Config mini backend: `kind: cloud`, OpenRouter base URL.

**Cause chain**

1. **Error string is unique to `mini_command_build.rs`.**  
   Cloud arm of `build_mini_command_impl` **hard-returns** that error (~1070–1081 macOS; same on Windows arm). One-shot PTY/`/bin/sh` path cannot drive HTTPS OpenRouter.

2. **That function is only used by the one-shot spawn path** (`spawn_one_shot_mini` → `build_mini_command` in `mini_coder_executor.rs` ~3520).  
   So at claim/launch time this directive took **`run_agentic == false`** (one-shot branch), not the agentic HTTP branch.

3. **Contradiction with current Cloud agentic wiring.**  
   In current tree, for `MiniCoderBackendKind::Cloud`:
   ```text
   run_agentic = directive.write && base_url non-empty
   ```
   This directive has `write: true` and config has baseUrl. **If** that code ran with a correctly resolved Cloud backend, it should **not** call `build_mini_command`.  
   Therefore either:
   - **(A) Temporal:** the smoke ran against a binary **before** Cloud agentic force-path was complete, or  
   - **(B) Structural residual:** resolved backend at launch lacked baseUrl / was mis-classified so Cloud still fell through somewhere, or agentic spawn failed and a fallback still hit one-shot (less likely given exact hard-fail string).

4. **Even with agentic path present, one-shot still hard-fails Cloud.**  
   Any code path that still calls `spawn_one_shot_mini` for Cloud (mis-flagged write, empty baseUrl race, future regression) dies with the same message. Dual implementation is incomplete: agentic can do Cloud; one-shot cannot and is not auto-upgraded.

5. **Cascade:** orch thought mini was “running” (MCP spawn returned `status: running` at enqueue) while executor later wrote **failed** — no successful file edit, orch never re-synced to failure in the UI narrative.

**Confidence:** high that failure = Cloud + one-shot `build_mini_command`. Medium-high that current source *intends* agentic for Cloud write; live failure shows that intent was not effective for this run.

---

### B04 — Orchestrator claimed T1 and spawned mini (Main skipped)

| Field | Value |
|-------|--------|
| **Area** | orchestrator / product model / tools |
| **Severity** | major |
| **Verdict** | **BUG** |

**What we saw**

- Session `orchestrator-openrouter-mock`: role `orchestrator`, client `pi`, status still **`wip`**, message “Mini coder is implementing T1…” while mini **failed**.
- Activity: `project_claim_task` T1 → `spawn_mini_coder` (not `spawn_main_coder`).
- Product rule (owner + later role_rules): Work console = Main; mini only from Main; orch plans / `spawn_main_coder` then sleeps; Change plan recalls orch.

**Cause chain**

1. **At smoke time, role allowlist still permitted orch → mini** (pre-fix allowlist). Activity proves spawn succeeded after `surgical` reject.
2. **Model policy followed tools, not product stages.** With mini available, the LLM short-circuited Main.
3. **After allowlist fix** (`role_rules.json`: orch has `spawn_main_coder` only, no mini tools — verified in `29_role_tools.txt`), **new** orch sessions cannot spawn mini. **This session remains wip** — no automatic finalize when child directive fails; soft-lock / human not forced to clear.
4. **Claim semantics:** `CODER_LIKE_ROLES` still treats orchestrator like coder for Kanban claim (`aspis_mcp.py`). So even with mini tools removed, orch can still claim WIP tasks unless further restricted — product gap residual.

**Confidence:** high on wrong pipeline for this smoke; high that current SSoT blocks mini for orch; medium that claim-by-orch remains an open product hole.

---

### B05 — Invalid `write_mode: "surgical"`

| Field | Value |
|-------|--------|
| **Area** | tools / model behavior |
| **Severity** | minor |
| **Verdict** | **BUG** (model) / **working** (server gate) |

**What we saw**

- First `spawn_mini_coder`: MCP `-32602` — mode must be `emitEdits` | `agenticIterative`, got `"surgical"`.
- Second call without surgical → accepted (`status: running`).

**Cause**

- Allowed modes are fixed in Python MCP + Rust `WriteMode` serde (camelCase).  
- Model invented `"surgical"` (not in schema). Server **correctly** rejected.  
- Not a host crash; wasted a turn and confuses logs.

**Confidence:** high.

---

### B06 — `oracle_context` rejects project root outside approved workspaces

| Field | Value |
|-------|--------|
| **Area** | tools / Oracle / MCP |
| **Severity** | major |
| **Verdict** | **LIKELY BUG** (multi-root product) |

**What we saw**

- MCP error: root `…/devboule-openrouter-mock` outside approved workspaces; set `ASPIS_WORKSPACE_ROOT` to parent.  
- Orch continued planning/claiming without grounded code context.

**Cause chain**

1. Gate lives in `devboule-mcp/src/tools/oracle.rs` (~296–297): rootPath must be under an approved workspace set (management root / `ASPIS_WORKSPACE_ROOT`).
2. **Intent:** prevent arbitrary filesystem RAG (security FEATURE).  
3. **Product friction:** app allows project `root_path` anywhere the user attaches; MCP oracle tools do **not** auto-approve that root. No in-app “approve this project root for Oracle” surfaced during the smoke.  
4. Env fix (`ASPIS_WORKSPACE_ROOT=/Users/user/Projects`) is operator-only and invisible in the planner chat failure (only tool error milestone).

**Why LIKELY BUG not pure FEATURE:** security gate is right; missing product bridge for legitimate multi-project roots is the bug.

**Confidence:** high on mechanism; product intent on multi-root is the open question.

---

### B07 — “⏳ Awaiting your reply” without a clear ask_user card

| Field | Value |
|-------|--------|
| **Area** | planning-console |
| **Severity** | major |
| **Verdict** | **LIKELY BUG** |

**What we saw**

- Bottom chat: last assistant message about T1/mini; pill **Awaiting your reply**.  
- No visible multi-option AskUser UI; only freeform “Message the Orchestrator…”.

**Cause chain**

1. Pill is **not** wired only to `needs_user` / open questions.  
   In `ProjectsView.tsx` (~1332–1341):
   ```ts
   plannerAwaitingReply =
     !!plannerActivityAgentId &&
     plannerConvo.length > 0 &&
     plannerConvo[plannerConvo.length - 1].role === "assistant";
   ```
   So: **any live activity agent + last chat row is assistant** → “your turn”, even if the assistant is mid-pipeline narrative (“next: collect evidence”) rather than a real blocking question.

2. True `ask_user` / plan approval surfaces are separate (doubt panel / plan stage). If those are empty, the pill still shows from the heuristic above.

3. Residual **stale session** (`wip` orch) keeps `plannerActivityAgentId` truthy → pill stuck across reloads of the same transcript.

**Confidence:** high that the pill is over-broad relative to product copy “awaiting your reply (esp. after AskUser)”.

---

### B08 — WebView / pilot eval freezes (IPC + DOM timeout)

| Field | Value |
|-------|--------|
| **Area** | Tauri / pilot |
| **Severity** | **blocker** (automation; possible UX jank) |
| **Verdict** | **BUG** |

**What we saw**

- After Projects + fill attempts: `tauri-pilot ping` still OK (socket / plugin handshake).  
- `title`, `eval`, `ipc get_pigeon_enabled`, `set_pigeon_enabled` → **Eval error: timed out after 10s** (`33_pilot_batch.txt`).  
- Blocks Settings navigation, Pigeon toggle, further planning sends.

**Cause chain (partial)**

1. Pilot `eval`/`ipc` run **JavaScript in the WebView**. Ping does not need a responsive JS heap the same way. Pattern “ping works / eval dies” ⇒ **main WebView JS thread blocked or starved**, not total process death (Rust still up).
2. Trigger correlated with: large agent state (35 sessions), rich Projects UI + planner, prior long transcript render, possibly concurrent IPC.  
3. Not fully isolated: could be React re-render loop, long sync work on main thread, or pilot queue stuck behind a previous hung eval.  
4. Once frozen, **cannot complete live Pigeon-on or fresh Local plan** in this session.

**Confidence:** high on symptom; medium on root (needs Instruments / main-thread stack at freeze).

---

### B09 — Composer “send” mis-clicks project card

| Field | Value |
|-------|--------|
| **Area** | planning-console / UI |
| **Severity** | minor |
| **Verdict** | **LIKELY BUG** (automation-first; possible hit-target issue) |

**What we saw**

- Heuristic search for a send button clicked text matching **OpenRouter Mock…** project card (`19_send_goal.txt`).

**Cause**

- Automation used weak `button` text matching (`/send|plan it|go/i`). Project cards are buttons whose accessible name includes title/path.  
- Product risk: large clickable cards near composer increase mis-click surface for humans; for agents, selectors must be specific.

**Confidence:** high for automation cause; low-medium that human UI is broken without fat-finger evidence.

---

### B10 — Git policy blocks non-git mock roots

| Field | Value |
|-------|--------|
| **Area** | projects / git policy |
| **Severity** | minor |
| **Verdict** | **FEATURE?** (with UX noise) |

**What we saw**

- `list_projects`: openrouter-mock `policyStatus: "blocked"`, “not inside a Git repository”, requiredActions about feature branch / origin.

**Cause**

- Intentional policy for collaborator/PR workflows (`gitStatus` on summary).  
- Throwaway smoke folders without git will always warn/block policy-sensitive actions.

**Confidence:** high this is by design; not a crash.

---

### B11 — Pigeon off confirmed; Pigeon on not verified / incomplete

| Field | Value |
|-------|--------|
| **Area** | Pigeon |
| **Severity** | major (for “sync with Pigeon” product claim) |
| **Verdict** | **BUG** (incomplete product) + **INCONCLUSIVE** live sync |

**Without Pigeon (async / file+MCP)**

- Launch: `Pigeon disabled (config pigeon.enabled=false); not starting.`  
- Early IPC `get_pigeon_enabled` → `false` (`17_pigeon_enabled.txt`).  
- Mini directive lifecycle used `.aspis-agents.json` + executor poll (classic path). **Confirmed working mode for coordination plumbing** (even though mini Cloud failed later).

**With Pigeon (attempted)**

- After UI freeze, `set_pigeon_enabled` / re-get timed out (`33_pilot_batch.txt`).  
- Static: `pigeon_service.rs` has `start_if_enabled`, `get/set_pigeon_enabled`, default-off from config. **No `mini-pool` string in that file** — mailbox drain for minis is not obviously wired there (may live elsewhere, but smoke could not prove end-to-end mailbox dispatch).  
- Config has **no `pigeon` object** — feature remains off until Settings/IPC succeeds.

**Cause**

- Default-off + incomplete operator path + freeze blocked toggle. Historical docs already suggested incomplete Pigeon; this session did not refute that.

**Confidence:** high on off-mode; high that on-mode was not proven; medium on completeness of mini-pool wiring.

---

### B12 — Fleet pollution (35 sessions)

| Field | Value |
|-------|--------|
| **Area** | agents ledger |
| **Severity** | minor |
| **Verdict** | **LIKELY BUG** (prune / isolation) |

**What we saw**

- `get_agent_live_state`: 35 sessions including ancient Aspis `test`/`hola` closed entries; only one mock-related wip.

**Cause**

- Ledger prune caps exist in MCP Python (`MAX_SESSIONS`) but closed sessions linger; multi-repo/history sharing same projects dir accumulates noise. UI/fleet counts (“active:9”) still mention large fleet.

**Confidence:** medium (policy may intentionally keep history).

---

### B13 — Header “Orchestrator · claude” vs live session `client: pi`

| Field | Value |
|-------|--------|
| **Area** | planning-console |
| **Severity** | minor |
| **Verdict** | **LIKELY BUG** |

**What we saw**

- UI chrome: `Orchestrator · claude` while chips show Local/Claude/…  
- Ledger: `orchestrator-openrouter-mock` **client `pi`**, model gpt-5.2.

**Cause**

- Header model/client label is driven by **planner chip selection / localStorage** (`plannerOrchestratorClient`), not the **live session** client field. Selecting Claude chip for a new launch does not rewrite the running pi session label; residual transcript stays attached to project.

**Confidence:** high on dual-source mismatch.

---

### B14 — Milestone compression works

| Field | Value |
|-------|--------|
| **Area** | planning-console |
| **Severity** | nit (positive) |
| **Verdict** | working |

Chat showed `… 17 earlier tool steps · agent_register, …` then last tool lines — `compressMilestoneRuns` in `plannerModel.ts` doing its job (`10_projects_view.txt`).

---

### B15 — Dev unlock works

| Field | Value |
|-------|--------|
| **Area** | Tauri auth |
| **Severity** | nit (positive) |
| **Verdict** | working |

`get_auth_state`: `locked: false`, launch log DEV unlock active (`04_auth_state.txt`).

---

### B16 — Main coder not exercised on the live OpenRouter path

| Field | Value |
|-------|--------|
| **Area** | main-coder |
| **Severity** | major (coverage + product path) |
| **Verdict** | **INCONCLUSIVE** for “Main works on OpenRouter”; **BUG** that the only live work path skipped Main |

**Cause**

- No `spawn_main_coder` in activity; no `role: coder` session for openrouter-mock.  
- Dependent on B04 (orch mini shortcut) + B08 (could not force a clean hand-off after freeze).  
- Static: Main owns mini tools; orch owns `spawn_main_coder` only — correct **if** models comply.

---

### B17 — Role allowlist SSoT now correct (orch no mini)

| Field | Value |
|-------|--------|
| **Area** | tools |
| **Severity** | nit |
| **Verdict** | working (SSoT) |

`role_rules.json` / MCP load: orch **no** `spawn_mini_coder` / steer / result; **yes** `spawn_main_coder`. Coder has mini tools. Aligns with product after the role fix; does **not** rewrite stale sessions (B04).

---

## Coordination modes (detail)

### Async without Pigeon
- Default. Confirmed at process start and early IPC.  
- Directives and MCP poll/file co-write used. Mini enqueue returned `running` then executor failed (B03).

### Sync with Pigeon
- Not fully entered (B08 + B11).  
- Finding stands: cannot claim Pigeon sync works for mini/orch until enable + mailbox drain is live-tested.

---

## Planning / questions / bottom chat

| Check | Result | Cause note |
|-------|--------|------------|
| Tool milestones in chat | Yes | Compressed (B14) |
| Assistant narrative | Yes | Residual failed pipeline (B03/B04) |
| Clarifying questions UI | Not observed fresh | B08 blocked new plan; B07 pill ≠ ask_user |
| Create plan controls | Visible | Not driven to completion |
| New Local plan send | Incomplete | B08 / B09 |

---

## What was not reached

1. Full Main OpenRouter write cycle.  
2. Live multi-option `ask_user` + reply.  
3. `plan_submit` → approve → `project_create_plan_tasks` on a clean goal.  
4. Pigeon-on mailbox mini dispatch.  
5. Local oMLX backends (config all Cloud).  
6. Verifier / done.  
7. Settings Roles consent (UI freeze).

---

## Severity rollup

| Severity | IDs |
|----------|-----|
| blocker | B01, B03, B08 |
| major | B02, B04, B06, B07, B11, B16 |
| minor | B05, B09, B10, B12, B13 |
| nit / working | B14, B15, B17 |

---

## Causal dependency map (how bugs stack)

```text
Oracle server dead (B02)
    → index/watch HTTP fail (B01)
    → weak/no oracle_context for in-workspace roots too

Orch claims T1 + spawn_mini (B04)  [allowlist/history]
    → mini Cloud one-shot hard-fail (B03)
    → index.html unchanged
    → orch stuck wip + chat “awaiting reply” heuristic (B07)
    → tool spam then compression (B14)

UI freeze (B08)
    → cannot toggle Pigeon (B11 on-path)
    → cannot re-drive planning / Main hand-off (B16)
```

---

## Appendix — evidence map

| File | Content |
|------|---------|
| `01_app_state.txt` | title, state, snapshot |
| `03_oracle_body.txt` | Oracle admin 0-index / banners |
| `04_auth_state.txt` | unlocked |
| `09_oracle_discovery.json` | stale discovery |
| `10_projects_view.txt` | board + chat + awaiting |
| `13_agent_live_*` | sessions + failed mini parent |
| `16_list_projects.txt` | git policy |
| `17_pigeon_enabled.txt` | false |
| `25_activity_tail.txt` | surgical + spawn trace |
| `29_role_tools.txt` | allowlists + directive result |
| `30_pigeon_static.txt` | pigeon surface |
| `33_pilot_batch.txt` | freeze: ping OK, eval/ipc timeout |
| `02_screenshot_raw.json` | full-page PNG data URL |

---

## No fixes under this goal

Deliverable is this document only (plus `{SCRATCH}` evidence). Product code was not patched in the AFK test session. Later commits on `phase1/infra` (roles/UI/dev-unlock) are separate from this report’s observe-only charter; RCA above uses **current tree** to explain residual risks (e.g. Cloud one-shot hard-fail still present).
