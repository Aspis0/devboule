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

**Live tools used:** `tauri-pilot` ping/snapshot/eval/ipc/screenshot; MCP `devboule_pilot__pilot_ping`; disk reads of agents ledger, activity JSONL, Oracle discovery.

---

## Matrix summary (criterion 2–4)

| Area | Attempted live? | Outcome | Key findings |
|------|-----------------|---------|--------------|
| Tauri unlock/session | Yes | Pass (dev unlock) | Unlocked without Touch ID (`04_auth_state.txt`) |
| Project select (openrouter-mock, board) | Yes | Partial | Board + cards OK; mock root not git repo (`16_list_projects.txt`) |
| Workspace **devboule** (Oracle index root) | Yes | Fail | Index 0; discovery stale; watcher fail |
| Orchestrator planning + questions | Partial | Blocked | Prior session chat visible; new send stalled; then WebView eval freeze |
| Bottom console chat | Yes | Pass (compression) + residual | Compression shows `… 17 earlier tool steps`; still shows old failed run |
| Main coder | Structural + residual live | Fail path | Orch skipped Main; no live Main session for mock |
| Mini coder | Residual live + static | Fail | Directive `failed`: cloud not supported on one-shot path |
| Tools advertised / used | Residual activity + role_rules | Mixed | Orch used mini (old allowlist); `write_mode: surgical` invalid; oracle_context root blocked |
| Async **without Pigeon** | Yes (default) | Confirmed | Startup + `get_pigeon_enabled` → false early |
| Sync **with Pigeon** | Attempted | Blocked / incomplete product | IPC enable hung after UI freeze; static: no `mini-pool` in `pigeon_service.rs` |
| Oracle indexing | Yes | **BUG confirmed** | See B01 |

---

## Findings

### B01 — Oracle does not index selected workspace (`devboule`)
| Field | Value |
|-------|--------|
| **Area** | Oracle |
| **Severity** | **blocker** |
| **Verdict** | **BUG** |
| **Repro / observed** | Open app → Oracle admin. Workspace shows `/Users/user/Projects/devboule`, "✓ Workspace is set". UI: Indexed 0 / Pending 0 / VECTORS 0, banners **"Oracle index watcher failed to start"** and later **"Oracle dense index job failed to start."** Click **Index now** → still 0. UI claims "Oracle server: running" but discovery file points at dead process. |
| **Evidence** | `{SCRATCH}/03_oracle_body.txt`, `08_after_index_now.txt`, `09_oracle_discovery.json`, `10_projects_view.txt` (dense index banner). Live: `curl 127.0.0.1:31520` connection refused; discovery pid **891** not running; `updatedAt` stuck at prior session. |
| **Notes** | Confirms owner half-bug. Additional lie: "server: running" with no listener + stale `.oracle-server.json`. |

### B02 — Stale Oracle discovery file / false "server running"
| Field | Value |
|-------|--------|
| **Area** | Oracle / Tauri |
| **Severity** | major |
| **Verdict** | **BUG** |
| **Observed** | `projects/.oracle-server.json` advertises `baseUrl http://127.0.0.1:31520` + old `pid` while nothing listens; UI HEALTH still says server running / "not ready yet" mixed messages. |
| **Evidence** | `09_oracle_discovery.json`, curl fail in session notes. |

### B03 — Cloud mini-coder fails: directive executor still rejects Cloud
| Field | Value |
|-------|--------|
| **Area** | mini-coder |
| **Severity** | **blocker** (for OpenRouter mini path) |
| **Verdict** | **BUG** |
| **Observed** | Directive `edbe87a2…` status **failed**, error: `cloud backend runs via the pi engine; the directive executor does not support it yet`. Config mini is `kind: cloud` OpenRouter. `index.html` **never** received T1 subtitle. |
| **Evidence** | `{SCRATCH}/29_role_tools.txt` / agents state; `25_activity_tail.txt`; live `index.html` has no subtitle. Code still hard-fails Cloud in `mini_command_build.rs` (~1070–1081) even though `mini_coder_executor.rs` has a Cloud agentic + Bearer path (~1502+). Live failure message matches the **hard-fail** string → one-shot path still hit. |
| **Notes** | Incomplete Cloud mini wiring / branch not taken for this directive (`write_mode` / agentic flag). |

### B04 — Orchestrator claimed implementation task and spawned mini (Main skipped)
| Field | Value |
|-------|--------|
| **Area** | orchestrator / tools / product-model |
| **Severity** | major |
| **Verdict** | **BUG** (post role_rules fix: residual session + allowlist was wrong at run time) |
| **Observed** | Session `orchestrator-openrouter-mock` role orchestrator, client `pi`, status **wip**, message still "Mini coder is implementing T1…" while mini **failed**. Activity: claim T1 → `spawn_mini_coder` (not `spawn_main_coder`). Product intent: Work console = Main; mini only from Main; orch only via Change plan. |
| **Evidence** | `13_agent_live_summary.txt`, `25_activity_tail.txt`, openrouter-mock notes/claim. Current `role_rules.json` correctly **removes** mini tools from orch (static re-check in `29_role_tools.txt`). |
| **Notes** | Stale **wip** session remains after failure — soft-lock / no finalize. |

### B05 — Invalid `write_mode: "surgical"` then retry
| Field | Value |
|-------|--------|
| **Area** | tools / orchestrator |
| **Severity** | minor |
| **Verdict** | **BUG** (model hallucinated mode; server correctly rejected) |
| **Observed** | First spawn: `write_mode must be one of emitEdits, agenticIterative, (got "surgical")`. Second spawn accepted without surgical. |
| **Evidence** | `25_activity_tail.txt`, earlier activity log. |

### B06 — `oracle_context` rejects project root outside approved workspaces
| Field | Value |
|-------|--------|
| **Area** | tools / Oracle |
| **Severity** | major |
| **Verdict** | **BUG** or **FEATURE?** — **LIKELY BUG** for multi-project roots |
| **Observed** | MCP: `rootPath '…/devboule-openrouter-mock' is outside approved Devboule workspaces; set ASPIS_WORKSPACE_ROOT to its parent to approve.` Orchestrator continued without grounded context. |
| **Evidence** | Activity log + assistant chat summary in UI (`10_projects_view.txt`). |
| **Why not pure FEATURE** | Product lets you attach arbitrary project roots; fail-closed without UI path to approve parent is broken UX for smoke mocks. |

### B07 — Planner stuck on "⏳ Awaiting your reply" with no clear ask_user card
| Field | Value |
|-------|--------|
| **Area** | planning-console |
| **Severity** | major |
| **Verdict** | **LIKELY BUG** |
| **Observed** | Bottom chat shows **Awaiting your reply** after old assistant turn; no obvious question options / reply affordance beyond freeform composer. Status may be residual `needs_user` / plan gate without surface. |
| **Evidence** | `10_projects_view.txt`. Fresh send attempt did not clear state. |

### B08 — WebView / pilot eval freezes under load (IPC+DOM timeout)
| Field | Value |
|-------|--------|
| **Area** | Tauri / pilot |
| **Severity** | **blocker** (for automation & sometimes UX) |
| **Verdict** | **BUG** |
| **Observed** | After Projects navigation + fill attempts, `tauri-pilot ping` still OK (socket) but **title/eval/ipc all timeout 10s** (`33_pilot_batch.txt`). Blocks further live planning, Settings, Pigeon toggle. |
| **Evidence** | `22–28*.txt`, `33_pilot_batch.txt`. |

### B09 — Composer send mis-clicks project card ("OpenRouter Mock…")
| Field | Value |
|-------|--------|
| **Area** | planning-console / UI |
| **Severity** | minor |
| **Verdict** | **LIKELY BUG** (automation-sensitive; may also hit fat-finger users if buttons poorly scoped) |
| **Observed** | Heuristic "send" button search clicked project card text `OpenRouter Mockdevboule-openrouter-mock…` (`19_send_goal.txt`). |
| **Notes** | Pilot/automation issue first; still indicates overlapping interactive targets. |

### B10 — Git policy: mock project root "not a git repo" / whole workspace blocked
| Field | Value |
|-------|--------|
| **Area** | Tauri / projects |
| **Severity** | minor |
| **Verdict** | **FEATURE?** (policy by design) with **UX gap** |
| **Observed** | `list_projects`: openrouter-mock `policyStatus: blocked`, warning "Use a specific code repo root, not the whole Devboule workspace" / not a git repo. |
| **Evidence** | `16_list_projects.txt`. |
| **Why FEATURE?** | Intentional safety. Still noisy for throwaway smoke projects. |

### B11 — Pigeon-off works; Pigeon-on not verified (UI freeze + incomplete mini-pool wiring)
| Field | Value |
|-------|--------|
| **Area** | Pigeon |
| **Severity** | major (for "sync with Pigeon" claim) |
| **Verdict** | **BUG** (incomplete path) + **INCONCLUSIVE** live sync |
| **Observed** | **Without Pigeon:** confirmed. Launch: disabled; early IPC `get_pigeon_enabled` → `false` (`17_pigeon_enabled.txt`). Agents/directives used **file/MCP poll path**, not mailbox. **With Pigeon:** `set_pigeon_enabled` IPC hung after freeze; static `pigeon_service.rs` has start/get/set but **no `mini-pool` string** in that file (`30_pigeon_static.txt`). Config has no `pigeon` object. |
| **Evidence** | Launch markers, `17`, `30`, `33`. |

### B12 — Fleet pollution: 35 sessions, many closed Aspis leftovers
| Field | Value |
|-------|--------|
| **Area** | Tauri / agents |
| **Severity** | minor |
| **Verdict** | **LIKELY BUG** (prune / isolation) |
| **Observed** | `get_agent_live_state` returns **35** sessions including ancient `test`/`hola` closed Aspis entries; only one mock-related wip. |
| **Evidence** | `13_agent_live_summary.txt`. |

### B13 — Orchestrator chip UI showed "Orchestrator · claude" while live session is `client: pi`
| Field | Value |
|-------|--------|
| **Area** | planning-console |
| **Severity** | minor |
| **Verdict** | **LIKELY BUG** (label/source mismatch) |
| **Observed** | Header text "Orchestrator · claude" + chips Local/Claude/Codex/OpenAI idle; ledger session `orchestrator-openrouter-mock` client **pi** model gpt-5.2. |
| **Evidence** | `10_projects_view.txt` vs `13_agent_live_summary.txt`. |

### B14 — Milestone compression works (regression check)
| Field | Value |
|-------|--------|
| **Area** | planning-console |
| **Severity** | nit (positive) |
| **Verdict** | **FEATURE?** / working |
| **Observed** | Chat shows `… 17 earlier tool steps · agent_register, provider_credentials_status…` then tail of spawn/heartbeat. |
| **Evidence** | `10_projects_view.txt`. |

### B15 — Dev unlock OK in debug
| Field | Value |
|-------|--------|
| **Area** | Tauri |
| **Severity** | nit (positive) |
| **Verdict** | working |
| **Observed** | `locked:false`, launch log DEV unlock active. |
| **Evidence** | `04_auth_state.txt`, launch log. |

### B16 — Main coder not exercised on live OpenRouter path this session
| Field | Value |
|-------|--------|
| **Area** | main-coder |
| **Severity** | major (coverage gap caused by product path failure) |
| **Verdict** | **INCONCLUSIVE** live Main success; **BUG** that Main was skipped in the only live work path |
| **Observed** | No `spawn_main_coder` / coder session for openrouter-mock; only orch + failed mini. UI freeze blocked fresh hand-off test. Static: Main owns mini tools; orch owns `spawn_main_coder` only (`29_role_tools.txt`). |

### B17 — Role allowlist now correct (orch no mini) — good; live session predates/enforces poorly
| Field | Value |
|-------|--------|
| **Area** | tools |
| **Severity** | nit |
| **Verdict** | working (SSoT) / **BUG** (stale session still wip) |
| **Evidence** | `role_rules.json` via `29_role_tools.txt`. |

---

## Coordination modes

### Async without Pigeon
- **Status:** Exercised by default.
- Agents register/heartbeat/spawn via MCP + `.aspis-agents.json` (not mailbox).
- Mini executor poll/file path used; result written as failed.

### Sync with Pigeon
- **Status:** Not fully entered.
- Blockers: (1) config default off; (2) after ~minutes UI eval freeze → cannot toggle Settings/IPC; (3) static code review shows incomplete mini-pool integration in `pigeon_service.rs`.
- Finding **B11**.

---

## Planning / questions / bottom chat

| Check | Result |
|-------|--------|
| Bottom chat shows tool milestones | Yes (compressed) |
| Assistant narrative visible | Yes (old turn) |
| Clarifying questions UI | **Not observed** for a fresh ask_user turn; stuck "Awaiting your reply" |
| Create plan / auto-create controls | Visible (`Create plan`, auto-create off) |
| New Local plan send | Fill OK earlier; after freeze, eval fails — **not completed** |

---

## What was *not* reached (honest gaps)

1. Full **Main coder** OpenRouter write cycle (hand-off → claim → mini → review).
2. Live **ask_user** multi-option question cards + reply round-trip.
3. **plan_submit** → human approve UI → `project_create_plan_tasks` on a fresh goal.
4. **Pigeon-on** mini dispatch latency / mailbox drain.
5. Local oMLX path (config is all Cloud OpenRouter).
6. Verifier / done transition.
7. Settings Roles consent Save (UI freeze).

---

## Severity rollup

| Severity | IDs |
|----------|-----|
| blocker | B01, B03, B08 |
| major | B02, B04, B06, B07, B11, B16 |
| minor | B05, B09, B10, B12, B13 |
| nit / working | B14, B15, B17 |

---

## Appendix — evidence map

| File | Content |
|------|---------|
| `01_app_state.txt` | title, state, interactive snapshot |
| `03_oracle_body.txt` | Oracle admin text (0 index, watcher fail) |
| `04_auth_state.txt` | unlocked session |
| `09_oracle_discovery.json` | stale discovery |
| `10_projects_view.txt` | board + chat + awaiting |
| `13_agent_live_*` | sessions + failed mini directive parent |
| `16_list_projects.txt` | git policy warnings |
| `17_pigeon_enabled.txt` | false |
| `25_activity_tail.txt` | tool trace surgical/spawn |
| `29_role_tools.txt` | allowlists + failed directive result |
| `30_pigeon_static.txt` | pigeon service surface |
| `33_pilot_batch.txt` | freeze: ping OK, eval/ipc timeout |
| `02_screenshot_raw.json` | full-page PNG data URL (large) |

---

## No fixes under this goal

`git status` shows only pre-existing untracked assets; **no product fix commits** from this AFK test session. This file is the sole intentional deliverable.
