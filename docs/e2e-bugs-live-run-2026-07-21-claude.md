# E2E live run — Devboule full pipeline (2026-07-21, Claude)

**Mode:** observe-only for product code (no product source modified). Two deliberate infra actions, both documented inline: (1) redeploy of the *already-committed* `devboule-mcp` binary (fix-queue item #3 of the AFK doc), (2) one app restart (authorized).
**App:** Tauri debug + `ui-pilot`, `DEVBOULE_DEV_UNLOCK=1`, Vite `:1420`. Driven via `tauri-pilot` CLI (snapshot/eval/ipc) + disk reads (ledger, activity JSONL, plans, Oracle discovery) + static code tracing for RCA.
**Sandbox project:** `~/Projects/devboule-website` (git repo, branch `feat/site-build`, seeded README + index.html + style.css), attached as app project `devboule-website-e2e`, Oracle workspace pointed at it.
**Backends exercised:** orchestrator OpenRouter (gpt-5.2 via pi), orchestrator local (Qwen3.6-35B-A3B-oQ4e-fp16-mtp via oMLX), main coder cloud (anthropic/claude-sonnet-4 via OpenRouter, agentic), main coder local (Qwopus3.6-35B-A3B-Coder-4bit via oMLX, agentic). Claude-CLI orchestrator round **not reached** (see F31).
**Evidence root:** session scratchpad `…/scratchpad/e2e/` (snapshots `01…21`, `findings-draft.md`, monitors' outputs, `config.json.backup-pre-e2e`, stale MCP binary copy).

---

## TL;DR — the five things that matter

1. **F31 (blocker):** any dev relink of the app ⇒ next boot freezes the WHOLE app on an (often invisible) macOS Keychain ACL prompt. Root cause of at least part of "B08". Main thread does a **synchronous Keychain read inside a Tauri command**; the oracle-supervisor thread races the same credential. Sampled stacks prove it.
2. **F02 (blocker):** agents were running a **stale `devboule-mcp` binary** (built Jul 20 18:33; B04/B06 fixes landed Jul 21 12:10). `cargo build -p devboule-mcp` writes `devboule-mcp/target/debug/`, agents load `src-tauri/target/debug/` — nothing updates the agent path. After I redeployed, **B04 and B06 verified working live**. Bonus trap: `cp` over the old binary → macOS SIGKILL (signature) → MCP lane dies with "-32000 Connection closed" (F22).
3. **F03 + F07 (major, product):** the pipeline has two dead-ends. Plan **approval dispatches nothing** — per product intent, post-approval execution belongs to the **Main coder**, but approving neither materializes the plan tasks (with auto-create OFF) nor launches/claims a Main coder; nothing happened until I manually steered the orchestrator, which is not supposed to be needed. Main-coder directives then **never move the Kanban task** (T1 done twice, still `wip`; review/verifier chain unreachable). Proven cascade: a second orchestrator saw stale T1 and re-dispatched the same task.
4. **F08 (major):** cloud AND local agentic main coders write **zero live activity** (`mini-orchestr-<id>.jsonl` stays 0 bytes) — the human is blind during runs, steer targets ghosts, "Main coder running" sessions never close.
5. **F27/F30 (major):** Work-console **Commit is a silent no-op** (message accepted, no commit, no error; Git tab counters wrong), and **no censor review was observed on any write** even after Trust & enable (all deterministic linters also "not installed" on this box).

---

## What was proven WORKING

- Dev unlock (manual-lock → Unlock click works without Touch ID under DEV unlock).
- Project creation via `create_project` with attached root; git policy = warning (not blocked) for a real repo; censor auto-trust correctly OFF for a non-empty folder.
- Oracle admin: re-point workspace via `save_oracle_index_preferences` → `start_oracle_index_job` → **3 files / 3 chunks / 3 vectors, job complete, watcher watching** on the new root (B01 core path holds on a real small workspace).
- OpenRouter orchestrator (pi client): MCP register with launch token (launch_pending fix live), heartbeats, `project_next_task`, notes, **plan_submit now non-blocking** (~41 s turn), plan markdown quality excellent (T1–T4 nanophases with acceptance criteria).
- Steer/reply from the planner composer → orchestrator resumes correctly (both cloud and local models).
- Local orchestrator (Qwen3.6 oQ4e via oMLX): registers, streams chat + thinking into the console, recovers from MCP outage by reading files, obeys the B04 claim rejection and re-routes to `spawn_main_coder`.
- **B04 verified live post-redeploy:** `project_claim_task` as orchestrator → "-32602 Orchestrators cannot claim implementation tasks".
- **B06 verified live post-redeploy:** `oracle_context` no longer rejects the attached project root (fails later on the token layer — F21, different bug).
- Both main-coder implementations produced good, in-scope diffs (T1 hero cloud; T2 features+footer local Qwopus); the duplicate-T1 coder detected existing work instead of clobbering.
- Settings → Providers & Models write-through to root `config.json` is instant and correct (roles switched to Local/oMLX and back-verified on disk).
- Webview responsive throughout the working session (evals < 3 s) — no B08-style freeze **until** the F31 boot freeze.

---

## Findings

Severity: blocker > major > minor > note. All verified on disk/UI this session.

### Blockers

| ID | Area | Finding |
|----|------|---------|
| **F31** | Tauri / vault / B08 root cause | After a dev relink, next boot freezes the entire app on a Keychain ACL prompt ("devboule wants to use your confidential information stored in 'Devboule'… enter the login keychain password", SecurityAgent live). Sampled stacks: main thread `run_invoke_handler → get_oracle_index_preferences → vault::read_oracle_index_preferences → keyring::get_password → SecKeychainFindGenericPassword → psynch_mutexwait`; `oracle-supervisor` thread inside the same keychain path (it triggered the prompt). Defects: (a) **synchronous keychain read on the main thread** inside a Tauri command; (b) no timeout/fallback, app looks dead (pilot ping OK, every eval/ipc times out — exactly the B08 signature); (c) supervisor and UI race the same credential; (d) the dev loop guarantees recurrence on every relink. The prompt window is easy to miss (was not frontmost). **Owner confirms "Always Allow" does NOT survive restarts** — expected: dev builds are ad-hoc signed, every relink yields a new code signature, and the keychain ACL is bound to the signature, so macOS treats each build as a different app. Fix directions: sign dev builds with a **stable signing identity** (ACL then follows the identity across rebuilds), and/or bypass keychain in DEV-unlock boots (env-based dev secret), and in any case keychain access must be off the main thread with a timeout. Same-binary restarts do not re-prompt. |
| **F02** | Agents / deploy | Agents run `src-tauri/target/debug/devboule-mcp` (was mtime **Jul 20 18:33**) while the B04/B06 fixes landed **Jul 21 12:10** in `devboule-mcp/` sources; `cargo build -p devboule-mcp` outputs to `devboule-mcp/target/debug/` which nothing deploys. Effect pre-redeploy (observed live): `oracle_context` rejected the attached project root twice (orchestrator documented it inside the submitted plan); an orchestrator successfully **claimed T1** (B04 gate absent). I redeployed at 12:49; both fixes then verified live. The deploy path needs to be owned by the build (or the app should spawn the crate's own target). |
| **F22** | Agents / deploy (macOS) | Deploying the MCP binary with plain `cp` over the existing file ⇒ macOS kills it instantly (SIGKILL, exit 137, invalidated code signature) ⇒ every agent MCP connect fails "-32000 Connection closed" with **no user-visible signal anywhere**; the local orchestrator burned a whole turn debugging it. Correct deploy: `rm` + `cp` (new inode). Worth a doc note or a build-side deploy step. |

### Major

| ID | Area | Finding |
|----|------|---------|
| **F03** | Plan approval | Approval is **inert**: `plan_submit` returns fast (non-blocking — good, ticket (a) half-done); human Approve writes `approved` + decision event + sidecar update (`approve_plan_request → apply_approve`) but **dispatches nothing downstream**. Product intent (owner): once approved, execution is the **Main coder's** job — yet approval neither materializes the plan's tasks (with "auto-create tasks: off"; also nothing wired with it on was observed) nor launches/claims a Main coder. In practice zero tasks/handoff existed until I manually messaged the orchestrator — a step that should not be required at all. Missing link: approve → create plan tasks → hand T1 to Main coder (or Main-coder-side pickup of approved plans). |
| **F07** | Task lifecycle | Main-coder directives never touch the Kanban: T1 was implemented (twice) yet stayed `todo`→`wip`, `claimedBy: null`, never `review`; verifier-per-task can therefore never fire. **Proven cascade:** a later orchestrator launch saw stale T1 and re-claimed + re-dispatched the same task (directive 0cbc8d85) — duplicate work, benign only because the model noticed the hero already existed. |
| **F08** | Observability | Agentic main coders (cloud AND local) write **zero** live activity — `mini-orchestr-<id>.jsonl` stays 0 bytes for the entire run; Work console has no live feed and no working steer for them ("message coder · arrives next round" targets a done ghost). Ledger sessions `mini-orchestr-*` remain `active` / "Main coder running" forever after the directive is `done`; `finishedAt` stays `null`. Naming: a MAIN coder run is filed as `mini-orchestr-*` and rendered as "Mini" in the drawer. |
| **F27** | Git console | REVISED after IPC repro + flash localization: the backend commit path **works** (`project_git_commit` via IPC created `f0c4bb4`). The observed "silent no-op" came from **two identical "Commit" buttons** (header toggle at `ProjectWorkspace` header + panel submit) — clicking the header again just closes the panel with the message discarded, no submit, no feedback (same hazard class as B09; the panel submit needs a distinct testid/label, and `submitCommit` at `ProjectWorkspace.tsx:664` closes the textbox fire-and-forget before the async result). The **real** bugs, verified in code: (a) `projectWorkspaceModel.ts:97` `pushed = aheadCount === 0` with no upstream check → "pushed?: yes" on a never-pushed branch (backend skips rev-list when `upstream.is_none()`, `project_git.rs:115-124`, counts default 0); (b) commit failure path (`ProjectsView.tsx:~2857`) never refreshes `gitStatus` → stale STAGED counter; (c) errors render as tiny inline text, easy to read as "nothing happened". |
| **F30** | Censor | No censor review observed on any write directive — cloud T1 (censor untrusted: expected, but result carries **no "not censor-reviewed" marker**), duplicate T1 and local T2 (**censor_trusted=true**: still no review artifact, no Censor-tab finding, no omlx Censor call evidenced; only `.aspis/last_coarse_run` from the pre-trust window). "all clean · no open findings" is indistinguishable from "never ran". Commit-time gate untestable because Commit no-ops (F27). |
| **F06** | Ask-user UX (all paths) | The Kairion question **card never renders, on any client**. (a) pi/OpenRouter path: `KAIRION_QUESTION {json}` shown **raw in chat**, no pill, no card — `doubt_sensor_text` parsing only exists in `cloud_claude.rs`/`cloud_codex.rs`, the pi path has zero handling while the persona still mandates the marker. (b) **claude duplex path (tested live):** marker parsed ✓, stripped from the chat with preamble preserved ✓, "Awaiting your reply" pill ✓ (via needsUser), the parsed question **with full options exists in the backend ConsoleActivity store** (`mini_activity_snapshot` returns `type:"question"`) — but the planner console feed never delivers it to the UI (neither live push nor navigate-away re-hydrate), so `plannerQuestions` stays empty and `DoubtPanel` never mounts. The human sees "ecco la domanda:" followed by **nothing**. |
| **F21** | Oracle / agents | HEAVILY REVISED after flash localization + proper repro. What's real: **during the main session the discovery file was stale** (written 16:06 by the pre-restart process; still advertising the old port/pid) → the local orchestrator's `oracle_context` legitimately failed with the misleading copy "Oracle server unreachable — open the Devboule app" (app WAS open) — this is the B02 "no boot purge / no re-publish while stale" residual, plus bad error copy. What's NOT real: my earlier claims that the token "rotates on unlock without republish" and later "never matches" were **probe artifacts** — I was hitting `/health` (operator-gated) with the agent token and the wrong header/payload shape. Flash recon (r4) found both the server (`rust_oracle.rs:218`) and the discovery writer (`oracle_service.rs:1013`) read the SAME `oracle_agent_token()` OnceLock — no rotation exists. **Proper e2e proof, post-clean-boot:** `POST /context-bounded` with the discovery token (`x-oracle-auth-token`) + `allowed_file_ids` returned **3 real chunks** from the sandbox workspace. **The agent Oracle lane works end-to-end on a healthy boot with the fresh MCP binary.** Remaining defects: stale-discovery lifecycle window + "unreachable/open the app" copy for what is a stale-file condition. |
| **F01** | Oracle UI | `get_oracle_indexed_files` broken: rust server returns plain string paths, client expects `struct OracleIndexedFile` → Indexed-files table renders the error as a data row: `Python Oracle output was invalid: invalid type: string "README.md", expected struct OracleIndexedFile at line 1 column 22`. Also stale "Python Oracle" copy with `oracle.engine=rust`. |
| **F33** | Claude orchestrator (duplex) | The claude client is **fully ungoverned AND fully impotent**: `cloud_duplex` spawns `claude -p --permission-mode default` with no MCP allowlist and no permission bridge → every `mcp__devboule__*` call is denied ("requested permissions… but you haven't granted it yet") and in turn 2 even **file writes were denied**. Headless stream-json means the permission requests go nowhere. Net: no register, no plan_submit, no task updates, no spawn, no writes — the Claude orchestrator can only chat (it gracefully delivered its plan revision as chat text). Chat/thinking streaming and the cost banner ("Turn done — 7 turns · 83.6s · $0.9474") work well. Ledger row mis-attributes the model (`client=claude, model=qwen3.6-35b-a3b-oq4e-fp16-mtp`). |
| **F35** | Verifier per task | With the toggle ON (default OFF), moving a task to "In review" DOES auto-fire the verifier — but the implementation is broken end-to-end: it **double-fires** (two sessions 16 s apart for one T2 transition), spawns **external Terminal.app windows** running interactive `pi` sessions that sit idle at the prompt (unattended flow opening GUI terminals), ledger `client` says **codex** while the process is `pi`, sessions stay `launch_pending` forever, no verification output, T2 never leaves review. With the toggle OFF (default) nothing fires — combined with F07 (coders never move tasks to review) the verifier stage is dead in practice either way. |

### Minor

| ID | Area | Finding |
|----|------|---------|
| F04 | Plans UI | "Expand plan" on the approval card renders nothing although the full plan markdown exists at `.aspis-plans/<proj>/<id>.md` — the human approves blind. |
| F05 | Plans UI | After Approve: card clears only on the ~30 s poll; Approval-history row keeps saying "pending approval" for an approved plan. |
| F09 | spawn_main_coder | Orchestrator passed `backend:"local"`; executor silently ran the configured cloud backend (arg ignored, no warning back to the model). |
| F10 | Spawn panel | "No open task; the agent will work at project level" shown while the board has 4 `todo` tasks. |
| F11 | Spawn panel | **Confirmed**: clicking the **Coder** role radio flips `aria-checked=true` for an instant, then the controlled state **reverts to Orchestrator** on its own (~1 s) — so "Launch in app" launches an orchestrator no matter what the user picked. Matches the earlier accidental orchestrator launch. Launch feedback line also never states the launched role. |
| F12 | Config | Dead `src-tauri/config.json` diverges from the authoritative root `config.json` (censor `ollama` vs `omlx`, mini `omlx` vs `cloud`); resolution order (`../config.json` from src-tauri cwd) makes it unreachable — delete or it will bite a future cwd change. |
| F13 | Project hygiene | `oracle-data/`, `.pi/`, `.aspis/` are created untracked inside the project **git** root and not gitignored — a coder running `git add -A` would commit index artifacts (T4's "final diff limited to…" acceptance would already fail). |
| F16 | Plans UI | "0/4 done · executor: local runner" shown for cloud-executed directives. |
| F19 | Ledger data | Session `projectId: null` at register (only `currentProjectId` later); cloud coder sessions `model: null`; directive `taskId`/`projectId`/`goal` null on the record; relaunched orchestrator keeps the previous `model` (header showed "gpt-5.2" while local Qwen was live — B13-adjacent). |
| F24 | Planner header | Header stuck on stale ledger model (see F19) — chip vs live-session fix (B13) holds, but the ledger value itself goes stale on relaunch. |
| F25 | Websearch rail | Permanently renders "READING LIVE PAGES / loading… ×3 / fetching next / FINDINGS / Distilling findings…" skeleton while **no** websearch is running — looks stuck/active when idle. |
| F28 | Drawer labels | Cloud MAIN coder rendered as "Mini · cloud/anthropic/claude-sonnet-4"; round record "ROUND 1 Done · **0 files**" though files were written; "coder · cloud" chips persist after switching backends to local. |
| F23 | Sandbox scope | Local orchestrator's `bash` freely **reads** outside the project root (listed the Devboule app internals, executed the MCP binary); `/bin/ps` blocked by Seatbelt. If read-anywhere is not intended for orchestrators, tighten; if intended, document. |

### Notes / environment

| ID | Note |
|----|------|
| F14 | All deterministic censor linters "not installed" on this machine (gitleaks, jscpd, lizard, semgrep, zizmor, shellcheck, +5) — the deterministic gate is toothless here regardless of F30. |
| F15 | A completed write-directive result carries no indication of whether censor reviewed it. |
| F20 | Oracle **Ask** (human Q&A) disabled — no provider configured in this environment ("Configure Oracle provider →"). Agent retrieval (`oracle_context`) is the path that matters and is covered by F21/F02. |
| F26 | Post-redeploy: `agent_register`, launch-token lane, B04 rejection, B06 root approval all confirmed working — the fixes are good, only the deploy was missing. |

---

## Causal maps

```text
Dev relink of app binary
  → Keychain ACL prompt on next boot (SecurityAgent, often invisible)
  → oracle-supervisor keychain read triggers prompt
  → webview sync command (get_oracle_index_preferences) blocks MAIN THREAD on same mutex
  → whole app frozen: ping OK, eval/ipc timeout            (F31 — "B08" reproduced with proof)

Stale devboule-mcp deploy (F02)
  → oracle_context "outside approved workspaces"  → orchestrators plan ungrounded
  → orchestrator claim allowed                    → duplicate T1 dispatch (with F07)
  redeploy (rm+cp, F22 trap)
  → B04 + B06 verified live
  → oracle_context THEN hits stale discovery token (F21) → agents still Oracle-blind

Approval dispatches nothing (F03: no task materialization, no Main-coder pickup)
  + tasks never move (F07) + no live activity (F08)
  → human must chase every stage manually; board lies; verifier unreachable (F16 of AFK doc stays open)
```

---

## Round 2 (after the keychain prompt was resolved by the owner)

- Claude duplex round executed: streaming/cost banner good; **F33** (ungoverned + write-denied); KAIRION chain traced end-to-end (**F06** refined: parse ✓, marker stripped ✓, pill ✓, backend store has the full question — the planner console feed simply never delivers `type:"question"` to the UI, so `DoubtPanel` never mounts, on any client, live or re-hydrated).
- **F21 resolved to its true size**: the only real defect is the stale-discovery window + misleading copy; on a healthy boot the full agent lane (discovery token → `x-oracle-auth-token` → `/context-bounded` with `allowed_file_ids`) returned **3 real chunks** — **agents CAN use Oracle on this build** once discovery is fresh. (My interim "token never matches" claim was a probe artifact — `/health` is operator-gated; retracted.)
- **F11 confirmed** (role radio reverts to Orchestrator on its own).
- **F35**: verifier auto-fire tested both ways (toggle off = nothing; toggle on = double-fire into idle external `pi` Terminals labeled codex).
- Manual Kanban "Move task" menu works (T2 → review persisted to disk).

## Verified bug locations (flash swarm + on-disk verification)

11 deepseek-v4-flash recon agents localized the findings; every location below was **re-verified on disk by me** (file opened, symbol/quote confirmed — flash line numbers corrected where off). CONFIRMED = quote verified at that line; PLAUSIBLE = mechanism consistent but not fully re-traced.

| Finding | Location(s) | Mechanism (verified) | Status |
|---|---|---|---|
| **F07** task never moves | `src-tauri/src/backend/mini_coder_executor.rs:1879` `finalize_finished_mini` | Stamps directive status/result only; **zero** calls to `project_claim_task`/`project_update_status` anywhere in the file. The transitions exist only as MCP tools (`devboule-mcp/src/tools/project.rs:416` claim → wip, `:635` update_status, `:146` validate_transition role-gates) — nobody calls them on completion. Parent notification (`pigeon_egress_terminal:2652`) fires only on reap/timeout paths, not on normal completion. | CONFIRMED |
| **F08** no live activity | `mini_coder_executor.rs:3094` `mini_agent_id` (→ `mini-{parent8}-{id8}` = the misleading `mini-orchestr-*` name); creation via `update_bridged` writes only a `Spawn` entry which `bridge_line_for_entry` maps to `None` → 0-byte file; `agentic_worker.rs:344` `run_agentic_coder` writes only the final result JSON — the append helpers (`cloud_duplex.rs:186-199` `append_bridge_line`/`append_user_echo`) are never invoked by the agentic loop. | The whole agentic multi-turn loop has no activity-append call. | CONFIRMED |
| **F30** censor never runs on agentic writes | `mini_coder_executor.rs:1940` — Censor Phase A gate: `if !write_diffs.is_empty() && trusted`. AgenticIterative applies edits via tools → `edits`/`write_diffs` empty (the exception near the extraction keeps `files_touched` "for Censor" but censor never reads it) → fine runners skipped AND coarse dirty flag never set → `run_coarse_pass`/`stamp_last_coarse_run` (`censor/orchestrator.rs:953`) never fire. EmitEdits minis and the pi-sidecar per-file hook (`pi_sidecar.rs:~3641`) are separate, working lanes. | CONFIRMED |
| **F27** commit UX + git labels | `ProjectWorkspace.tsx:664` `submitCommit` fire-and-forget (closes textbox immediately); two identical "Commit" buttons (header toggle vs panel submit, no distinct testid); `projectWorkspaceModel.ts:97` `pushed = aheadCount === 0` with no upstream check (backend leaves counts at 0 when `upstream.is_none()`, `project_git.rs:115-124`); failure path `ProjectsView.tsx:~2857` never refreshes `gitStatus` → stale STAGED. Backend commit itself works (IPC repro created a commit). | CONFIRMED |
| **F11** spawn panel role revert | `SpawnPanel.tsx:~250` render-phase force: comment "**Local is orchestrator-only: force role**" — `if (client === "orchestrator") setRole("orchestrator")`; and `:294` `selection.role` re-forces it. Deliberate design (Local client = orchestrator only) but the UI lets you click Coder and silently reverts ~1 s later; no disabled state or explanation. | CONFIRMED (design + UX gap) |
| **F35** verifier double-fire + external terminal | Trigger: `ProjectsView.tsx:2022-2067` useEffect over tasks; dedupe guard `verifiedTaskKeysRef` is a **component-local useRef** (any tab/view remount forgets fired keys → re-spawn) and `.catch` deletes the key. Client: `ProjectsView.tsx:686` `effectiveVerifierClient = verifierClientDefault !== "local" ? … : "codex"` — **verifier "local" deliberately falls back to CLI client codex** → external-terminal spawn path (`agent_spawn.rs` osascript impl) → idle terminal, `client:"codex"` stamped by `record_launch_pending` (`agents.rs:~637`), stuck `launch_pending` because nothing ever registers. | CONFIRMED (double-fire exact trigger PLAUSIBLE between remount and catch paths) |
| **F24/F19** stale model label | `pi_sidecar.rs:245` overlay: `if existing.model.is_none() { existing.model = lp.model }` — live model ignored when a stale one exists; `agents.rs:~637` `record_launch_pending` updates role/status/client but **never model**. | CONFIRMED |
| **F25** websearch skeleton always on | `WebsearchView.tsx:14` `idle = !live && pages.length === 0` with `ProjectsView.tsx:3822` `live={!!orchestratorAgentId || !!cloudOrchestratorAgentId}` → any live orchestrator (even with zero search) renders "READING LIVE PAGES / loading…×3 / Distilling findings…". | CONFIRMED |
| **F16** "executor: local runner" | `PlanExecutionView.tsx:146` — hardcoded string; the model carries no executor field. | CONFIRMED |
| **F04** expand plan empty | `PlanApprovalCard.tsx:126` `markdown: md ?? ""` after `get_plan_markdown`; empty/failed load renders `MarkdownRenderer` with 0 blocks → `return null` → visually nothing, no error state. | PLAUSIBLE (need to confirm why `get_plan_markdown` returned empty for an existing .md) |
| **F05** history stuck "pending approval" | `PlansPanel` reads sidecar JSONs via `list_project_plans` (12 s poll); `PlanApprovalCard.resolve()` re-fetches the request queue but never the plans list; `best_effort_update_sidecar` (`plan_approval.rs:~469`) silently aborts when the sidecar is missing. In our repro the sidecar WAS approved on disk, so the no-refetch + poll path is the operative one. | PLAUSIBLE |
| **F06** question card never renders | Backend store HAS the question (proven via `mini_activity_snapshot`). pi path: `pi_sidecar.rs` EventMapper never constructs question entries at all. Claude path: flash points at `useAgentConsole.ts:134` — a `snapshot` event is a **full replace**, and snapshots (e.g. from the pi sidecar path or ordering with the initial fetch) don't carry questions, clobbering the tail-reader's entries. | PLAUSIBLE (frontend feed; exact clobber ordering not re-traced) |
| **F33** claude perm-denied | `projects.rs:~4497` argv: `--permission-mode <perm_mode>` where `perm_mode` stays `"default"` unless the consent hook is ACTIVE (`hook_active = hook_path && settings_path` — deliberate fail-closed, comment "SECURITY (5b F1…)"); no `--allowedTools` anywhere; `cloud_duplex.rs:280` Claude gets no control-message client (only Codex has a JSON-RPC dispatcher), so permission requests are never answered. Root deploy gap: consent hook binary/settings absent at launch. | CONFIRMED |
| **F21** oracle (resolved) | Server token: `rust_oracle.rs:218`; discovery writer: `oracle_service.rs:1013` — **same `oracle_agent_token()` OnceLock**, no rotation exists (flash negative-search). Live probe on `/context-bounded` with the discovery token + `allowed_file_ids` → 3 real chunks. Only real defect: stale discovery file from a dead process is served to agents (no boot purge) with "unreachable — open the app" copy. | CONFIRMED (lane works) |

## What was NOT reached (owed)

1. Consent prompts (no out-of-workspace write was attempted by any coder; sandbox stayed "Ask" default).
2. Mini-coder layer below Main (Main never spawned a mini in these runs).
3. "Run final review" / max-recall (upstream stages broken: F07/F35).
4. Pigeon-on (deliberately out of scope this round, still open — B11).

## Session end state

- App running and unlocked (keychain prompt resolved by owner — recurs on every dev **relink**, see F31; "Always Allow" cannot survive a rebuild because the ad-hoc signature changes).
- Sandbox repo `~/Projects/devboule-website`: branch `feat/site-build`, `index.html` modified (T1+T2 work, uncommitted — F27), seed commit only.
- Root `config.json`: orchestrator/local-coder → oMLX Qwen3.6-oQ4e, main → oMLX Qwopus (was cloud; backup at scratchpad `e2e/config.json.backup-pre-e2e`).
- `src-tauri/target/debug/devboule-mcp` = fresh build of committed sources (stale copy preserved in scratchpad).
- Ghost ledger entries: `mini-orchestr-c3e1d662`, `mini-orchestr-0cbc8d85` "active" forever; two `verifier-T2-*` stuck `launch_pending`; month-old relics survive prune (`verifier-1781886683024` closed 2026-06-19, `claude-fable-main` stuck "oracle_context" since 2026-07-03). Project `devboule-website-e2e`: T1 `wip`, T2 `review`, T3–T4 `todo` despite T1+T2 implemented.
- The two idle verifier Terminal `pi` processes spawned by my toggle test were killed; pre-existing owner terminals untouched.

---

## New findings from the post-fix re-verify round (Claude, round 3)

| ID | Sev | Finding |
|----|-----|---------|
| **F36** | major/security | The in-app "Claude" orchestrator spawns the `claude` CLI with **no `CLAUDE_CONFIG_DIR` isolation** (zero references in backend) → it inherits the OWNER's personal `~/.claude` config: global CLAUDE.md rules (that's why it answers in Italian — "Italian in chat" is the owner's personal rule), personal skills/agents, and personal permission allowlists. Product agents must run with a dedicated config dir; user-level grants must not leak into product agents. |
| **F37** | minor/ui | DoubtPanel (now rendering — F06 fix works) draws the question at 12.5px and option labels at 11.5px in a 537px column — long Italian paragraphs are hard to read (owner: "non si legge bene"). |
| **F38** | major/ux | Clicking a DoubtPanel option does NOT dismiss the question: the card stays and **every click sends another duplicate steer** ("For '…' — go with …" repeated per tap, live-observed by the owner). Needs optimistic collapse on pick + debounce/disable after first send. |
| **F39** | minor/migration | F31's DEV file store has no migration from the old keychain value: after update the Oracle workspace preference "disappears" (UI: "No indexed workspace folder is selected") until re-set. One-time, but surprising. |
| **F40** | note/testing | Suite not green as claimed: 10 failing tests — ~5 flakes from `dev_unlock_skips_biometric_and_idle_ttl` setting `DEVBOULE_DEV_UNLOCK=1` process-wide without serializing against parallel lock-gate tests, + 4 deterministic stale fixtures (role-contract SSoT drift rust↔`role_rules.json` on the orchestrator heartbeat lines, B03-era `backend_supports_directive_dispatch_cloud_is_rejected`, `orchestrator_role_selects_coder_provider_profile` expects `coder-worker-write` vs new `verifier-readonly`, prompt snapshot matrix vs new persona). The SSoT drift is a REAL inconsistency; the rest are unmaintained fixtures. |

## Round-3 re-verification (Claude, post-Grok fixes — flash swarm + live pilot)

Method: 10 deepseek-v4-flash verify agents on the diffs (sequential) + full live pipeline via ui-pilot + my own suite runs. Every verdict below is live-proven or diff-verified on disk.

### Verified FIXED (live)

| ID | Live proof |
|----|-----------|
| **F31** | Fresh relink boot → app responsive at once (title 0 s), no SecurityAgent, DEV file store used (`.oracle-index-preferences.json` written). Flash residue: release save path still timeout-less (accepted). |
| **F03** | Approve (20:32:12) → nudge injected ("Plan APPROVED by the human (plan_id=…)") → orch woke, `project_create_plan_tasks` (T5–T8) → `spawn_main_coder` task=T5 — **zero manual steering**. Flash gap stands: orch-dead + zero-tasks edge still inert; nudge is prompt-not-dispatch. |
| **F07** | Directive carries `task_id`; T5 and T6 both auto-promoted to **review** on done. Residual: task `claimedBy` stays null; flash gap: invalid task_id silently dropped on MCP spawn. |
| **F08** | `mini-orchestr-<id>.jsonl` streams live ("Agentic coder started", "Round N: tool", result); coder session closes (`done`, no eternal ghost). NEW **F41**: every line written twice. Session `message` text still says "Main coder running" after done (cosmetic). |
| **F30+F15** | T6 wrote index.html+README → **coarse pass fired** (`last_coarse_run` updated) and **fine censor produced findings** (`.aspis/steer_censor`, tidy correctness hits). Verify-only T5 correctly skipped. Note: system `tidy` flags HTML5 tags as unknown — outdated linter, false positives. |
| **F02** | Sidecars now spawn `src-tauri/binaries/devboule-mcp` (staged); stage script builds+installs rm+mv (F22 ✓). |
| **F01** | `get_oracle_indexed_files` returns `{path,chunks,updatedAt}` objects, 4 files. (`.pi/mcp.json` gets indexed — candidate for .oracleignore.) |
| **F06** | DoubtPanel renders the parsed question with options (was invisible). See new F37/F38 for its UX. |
| **F24/F19** | Planner header shows the live model (Qwen…oQ4e after local launch; openrouter/auto → gpt-5.4 registered). |
| **F25** | Live orchestrator + no websearch → no skeleton. |
| **F05** | Approved plan shows "approved" in history (refresh event works). |
| **F27** | Header button renamed "Show commit form" (distinct from panel submit); backend commit worked already. |
| **F35 (partial live)** | No codex fallback in code (diff-verified); dedupe sessionStorage-backed. Flash gap: fired-keys never cleared on legitimate review-exit → later re-verify silently skipped. Full verifier e2e not re-run this round. |

### Still broken / partial

| ID | Verdict | Evidence |
|----|---------|----------|
| **F33** | **STILL BROKEN (live)** | With `--permission-mode acceptEdits`, BOTH `mcp__devboule__agent_register` and `Write` remain permission-denied headless ("you haven't granted it yet") — acceptEdits does not cover MCP tool permissions and no control-message bridge answers prompts. Flash adds: silent sandbox-mode override (project "Ask" silently runs acceptEdits = fail-open design) + stale security comments. Claude lane remains chat-only. |
| **F13** | **PARTIAL (wrong repo)** | `.aspis/` added to the DEVBOULE repo's .gitignore — but the pollution lives in ATTACHED project roots: sandbox still shows `.aspis/ .pi/ oracle-data/` untracked; a coder's `git add -A` there still commits them. Needs per-project gitignore seeding or exclusion at write time. |
| F09/F28 | PARTIAL (as declared) | backend arg still advisory; Main label fixed in drawer chip (`main · kind/model`), files-touched labeling residual. |

### New findings (round 3)

| ID | Sev | Finding |
|----|-----|---------|
| **F42** | major | Agentic `run` tool has **no timeout**: the T6 coder ran `python -m http.server 8000` as a "verification step" → round blocked 7+ minutes (forever, until I killed the child) AND the zombie server squatted **port 8000 = oMLX's port**. Needs per-command timeout + guard against blocking/server commands. |
| **F41** | minor | Agentic activity: every progress line is appended **twice** (double-write in the on_progress path). |
| F38-impact | — | The duplicate steers from F38 made the orchestrator submit **duplicate plans** ("Finish website T3+T4" twice + an unwanted "Rebaseline") — F38 is not just cosmetic. |
| — | note | Task-card "Launch agent" dropdown menu closes before automation can click items (transient portal) — pilot-unfriendly, minor. |
| — | note | Kanban now carries stale T3/T4 alongside their T5–T8 replacements (orchestrator "replace" flow doesn't archive the replaced tasks). |
| **F43** | major/ux (owner repro) | Board card shows an ACTIVE agent while the project Work console says "No agent working this project". Cause: **pre-fix ghost sessions** still `active` in the ledger (mini-orchestr c3e1d662 / 0cbc8d85 / db842613 from the morning + 2 `verifier-T2-*` stuck `launch_pending`) — the F08 session-close fix works for NEW runs but nothing reaps the old ghosts, and board vs console use different "active" definitions. Needs a one-shot retroactive reap + unified predicate. |
| **F44** | major/websearch | Live `web_search` (keyless Exa) returns REAL results to the model (top-3 summarized in chat) but the first-class rail channel emits **zero** `devboule_websearch` events and banners "Web search completed (results not extractable)" — the pages/findings rail stays empty and the banner contradicts the visible results. The known fragile `parseMcpResults` path is still live. (Upside: with F25 fixed the rail correctly shows nothing instead of fake skeletons.) |
| — | note/pilot | With Figlyph and Devboule both running, `tauri-pilot` socket autodetect targets the wrong app — automation must pass `TAURI_PILOT_SOCKET=/tmp/tauri-pilot-com.devboule.app.sock`. |
| — | positives (extra round) | Censor UI surfaces per-file findings ("index.html · 18 findings · 1 dirty") + board card badge "⚠18"; git header "pushed?: no upstream" correct; header commit button "Commit…" distinct; plan rail + DoubtPanel populated. |

### Suite status (F40)

`cargo test --lib` (src-tauri): 3502 pass / **10 fail** = ~5 env-race flakes (`dev_unlock_skips_biometric_and_idle_ttl` sets `DEVBOULE_DEV_UNLOCK=1` un-serialized) + 4 deterministic stale fixtures (role-contract SSoT drift rust↔`role_rules.json`, B03-era cloud-reject test, provider profile, prompt snapshots).

**Vitest: the FULL suite HANGS deterministically** — reproduced 3× (parallel ×2, sequential ×1): run stalls at **0.0% CPU** indefinitely (observed 8–47 min), log frozen, workers idle; a killed run also leaves **zombie worker processes**. All individually-tested suites pass (incl. every grok-touched one: WebsearchView, projectWorkspaceModel, SpawnPanel, useAgentConsole, PlanApprovalCard, AgentConsole — 71/71 etc.), and small subsets pass, so it's a cross-file interaction (dangling handle/listener keeping the event loop alive or a worker deadlock). Crude bisection was inconclusive. Recommended: run with `--reporter=hanging-process`, or attach `node --inspect` to the stalled worker, or `wtfnode`. Until fixed, CI on vitest is effectively dead.

Fix queue: update the 4 cargo fixtures + serialize the env test + reconcile `role_rules.json` + root-cause the vitest hang.

## Post-fix status (2026-07-21 Grok goal)

Product fixes landed on `phase1/infra` for the F-series above. Per-ID disposition:
see goal scratch `closure-matrix.md`.

| Batch | IDs | Status |
|-------|-----|--------|
| Unblock | F31, F02, F22 | FIXED (+ deepseek-v4-pro audit) |
| Pipeline | F07, F08, F30, F15 | FIXED (+ deepseek-v4-pro audit SHIP) |
| Edges | F03, F35 | FIXED |
| UI | F04, F05, F06, F11, F16, F25, F27, F10 | FIXED |
| Oracle | F01, F21 residual | FIXED |
| Claude | F33, F19/F24 | FIXED |
| Hygiene | F12, F13 | FIXED |
| Notes | F14, F20, F26 | N/A |
| Policy | F23 | DEFERRED |
| Residual | F09, F28 | PARTIAL |

## Post-fix status (2026-07-21 Grok goal — Claude round-3 F33 residual + F36–F44)

| ID | Status | Notes |
|----|--------|-------|
| **F36** | FIXED | `CLAUDE_CONFIG_DIR` under projects_dir for duplex + PTY Claude scripts |
| **F33 residual** | FIXED (unit) | `permissions.allow` for `mcp__devboule__*` always; headless file tools when no consent hook; live CLI e2e not re-run here |
| **F37** | FIXED | DoubtPanel question 14.5px / options 13px |
| **F38** | FIXED | single-fire + optimistic dismiss (settledRef) |
| **F39** | FIXED | keychain→DEV file one-shot migrate when DEV prefs file missing |
| **F41** | FIXED | activity JSONL write once (BridgedOnly) |
| **F42** | FIXED | block `python -m http.server` etc.; run timeout 180s |
| **F43** | FIXED | `isLiveWorkingSession` unifies board WHO vs Work console |
| **F44** | FIXED | websearch rail extracts pages from tool content + structured results |
| **F13 residual** | FIXED | seed `.aspis/` `.pi/` `oracle-data/` into attached-root `.gitignore` |
| **F40** | PARTIAL | cargo fixtures fixed; default `npm test` hang-free (199 files / 2227 tests) by excluding DesignView suites + hanging `RolesTableCard.test.tsx`; full design via `npm run test:design` |

### Claude round-4 independent re-verification (flash swarm + suites + code)

Method: 5 deepseek-v4-flash verify agents on the diffs + `cargo test --lib` + `npx vitest run` + live app boot + source reads. Verdicts are mine, not grok's self-report.

| ID | My verdict | Evidence |
|----|-----------|----------|
| **F31/F02/F22** | CONFIRMED FIXED | Round-4 relink boot responsive at once (title 0 s, no SecurityAgent); staged binary path used. |
| **F42** | CONFIRMED FIXED | `is_blocking_server_run` pre-check + 180 s timeout + group-kill. Flash gap: arbitrary `node app.js`/`cargo run server` not pattern-matched → still eats the 180 s before kill (acceptable). |
| **F41** | CONFIRMED FIXED | Single write path; the residual dual-write only on the no-projects_dir branch (no bridge file → no real dup). |
| **F36** | CONFIRMED FIXED (live) | `CLAUDE_CONFIG_DIR` set on the running claude process + in code (`cloud_claude_config.rs`). |
| **F33** | FIXED IN CODE (not re-run live) | `product_mcp_allow_rules()` → `mcp__devboule__*` on `permissions.allow`; `headless_file_tool_allow_rules` for Read/Edit when no hook. Structural fix is real; live claude relaunch not achievable this session (planner client chips not mounted in the visited views). Was STILL-BROKEN in round 3 → now plausibly closed, but **owes one live register**. |
| **F37/F38** | FIXED IN CODE + STORE | Font sizes raised (`doubtPanelModel.ts`), option-pick single-fire + optimistic dismiss (`DoubtPanel.tsx`). Question+3 options present in store (`mini_activity_snapshot`). Live triple-click could not reach the card in the visited layout (0 duplicate steers observed — consistent with the fix, not a full proof). |
| **F44/F39/F13** | FIXED IN CODE | Flash-confirmed quotes (websearch page extraction, keychain→file one-shot migrate, per-project .gitignore seed). |
| **F35** | **STILL GAP** | `ProjectsView.tsx:2086` still only clears `verifierFailedKeysRef` on review-exit; `verifiedTaskKeysRef` (now sessionStorage-persisted) is never cleared → a task legitimately re-entering review will **not** re-verify. Grok's round-4 did not touch this. |
| **F43** | **PARTIAL** | Retroactive reap exists (24 h threshold) + `isLiveWorkingSession` unifies the predicate, but on disk **11 ghost sessions from Jul 2–13 are still `active`** and today's `<24 h` ghosts (mini-orchestr-c3e1d662 etc.) are untouched — the board-vs-console mismatch can still occur inside the 24 h window. Needs an immediate stale-on-boot purge, not just a 24 h age rule. |
| **F40-cargo** | **PARTIAL** | 9/10 fixtures fixed; **`project_agent_prompt_snapshot_matrix` still FAILS** (persona snapshot drift). Flash also flags a residual env-lock race: `f31_prefs_env_lock` (vault.rs) is separate from `DEV_UNLOCK_ENV_TEST_LOCK` (state.rs) — both guard `DEVBOULE_DEV_UNLOCK`, still raceable. |
| **F40-vitest** | **WORKED AROUND, not root-caused** | Default `npm test` is green (2227/0, hang gone) **because the DesignView + `RolesTableCard.test.tsx` suites were EXCLUDED** and moved to `npm run test:design`. The cross-file hang itself was not diagnosed — it's quarantined. The excluded design suites' health is now unverified in the default gate. |

> **F45 (Oracle agent retrieval empty on external project) — reframed:** not a standalone bug but a symptom of Oracle's single-workspace model vs N open projects. Design spec written: `docs/oracle-multiroot-registry-design.md` (multi-root registry + per-project resolution + future union query; P1 manifest-path fix is a quick win). The agent path (register→oracle_context) is otherwise governance-healthy — I registered with a forged launch token and got isError:false; only the resolved scope was empty.

**Round-4 bottom line:** the pipeline-critical and unblock fixes hold (F31/F02/F03/F07/F08/F30/F42/F41/F36). Real remainders: **F35** (re-verify dead after first pass), **F43** (ghost reap incomplete), **F40** (1 cargo fixture + env-lock race + vitest hang quarantined not fixed), and **F33** owes a live register. No regressions observed.

## OPEN FIX-QUEUE (after Claude round-4 — what's left)

Prioritized. "verify" = re-run the live check I couldn't complete this session.

| Prio | ID | What's left | Effort |
|------|----|-------------|--------|
| 1 | **F33** | Fixed in code (allow-rules `mcp__devboule__*` + headless file tools) but owes ONE live proof: launch a Claude orchestrator and confirm `agent_register` + a Write succeed headless (no "you haven't granted it yet"). | verify |
| 2 | **F37/F38** | Fixed in code + store (font sizes raised, single-fire + optimistic dismiss) but not live-proven at the DoubtPanel card (couldn't reach it in the visited layout). | verify |
| 3 | **F42 gap** | `is_blocking_server_run` is pattern-based — arbitrary servers (`node app.js`, `cargo run server`) fall through to the 180s timeout instead of the fast pre-check. Broaden the heuristic or shorten the default run-tool timeout. | small |
| 4 | **F30 note** | System `tidy` flags HTML5 tags (`<header>`, `<main>`, `<section>`) as unknown → false-positive censor findings. Update the tidy config / linter for HTML5. Also `.oracleignore` should exclude `.aspis-censor/`, `.pi/`, `oracle-data/` (currently indexed as noise). | small |
| 5 | **F40-vitest** | Hang only QUARANTINED (DesignView + RolesTableCard.test.tsx → `test:design`); root-cause + re-include still open. Snapshot matrix + env-lock fixed 2026-07-21. | medium |

**Closed in 2026-07-21 Grok follow-up (F35/F43/F40a-b + F45 multi-root):** see § below.

**Declared/kept as-is (not "unfinished"):** F09/F28 PARTIAL (spawn backend arg advisory; files_touched label residual), F23 DEFERRED (orchestrator read scope — policy, no tighten invented), F14/F20/F26 N/A (env / notes).

## Post-fix status (2026-07-21 Grok — F35 / F43 / F40a-b / F45)

| ID | Status | Notes |
|----|--------|-------|
| **F35** | FIXED | On leave-review: clear both `verifiedTaskKeysRef` + `verifierFailedKeysRef` and persist sessionStorage (`clearVerifierKeysOnLeaveReview` + ProjectsView). Vitest: `agentClaims.test.ts`. |
| **F43** | FIXED | `reap_stale_ghost_sessions` on `get_agent_live_state`: closes `active`/`launch_pending` ghosts past 3 min / 2 min (same windows as FE `isLiveWorkingSession`), persists ledger. Unit: `reap_stale_ghost_sessions_*`. |
| **F40 (a)** | FIXED | `project_agent_prompt_snapshot_matrix` fixture aligned to orchestrator "Plan and hand off / NEVER spawn minis" SSoT. |
| **F40 (b)** | FIXED | `f31_prefs_env_lock` aliases `state::DEV_UNLOCK_ENV_TEST_LOCK` (one process-wide mutex). |
| **F40 (c)** | OPEN | Vitest DesignView hang still quarantined (`npm run test:design`). |
| **F45** | FIXED (code) | Multi-root P1–P3: project-root manifest, registry/`index_roots`, fail-closed resolve, `extra_roots` union. See `docs/oracle-multiroot-registry-design.md`. |



## 2026-07-22 Claude round-5 (fix-queue closure + live pilot e2e)

Method: coding via grok CLI, hostile audits via deepseek-v4-pro (pi), live e2e via tauri-pilot. Suites at close: cargo 3535/0 (`--features ui-pilot`), oracle-core 26/0, vitest 2229/0.

| ID | Status | Notes |
|----|--------|-------|
| **F42 gap** | FIXED (code+review) | `is_blocking_server_run` broadened: PM dev/serve/watch scripts (npm/yarn/pnpm/bun incl. bare + case-insensitive + `run` by position), `npx next dev|start`/vite-non-build/nodemon/webpack serve/json-server, bare `vitest`/`vitest watch` (run allowed), `--watch*`/`--watchAll` (`=false` allowed), `tsc -w`, `python -m uvicorn/gunicorn/waitress/flask run`, `uv|poetry run …` recursion, `deno serve` (subcommand only; `deno task serve` allowed). 2 deepseek BLOCKERs closed in fix round; final SHIP. |
| **F30 note** | FIXED (code+review+probe) | tidy taught HTML5 tags via `--new-blocklevel-tags`/`--new-inline-tags` (empirically ACCEPTED by the 2006 macOS tidy — deepseek's "unknown option" blocker REFUTED by live probe); residual `<tag> is not approved by W3C` noise filtered in `parse_tidy_line` for taught tags only. `.aspis-censor` + `.pi` added to oracle-core `EXCLUDED_DIRS`. Live censor re-run not yet observed (fine loop fires on agentic writes; blocked by F46/F47 below). |
| **F13b** | FIXED | Seed list extended with `.aspis-censor/` + `.aspis-mini/` (live sandbox showed them untracked). |
| **F43** | **CONFIRMED LIVE** | Ledger after boot: 1 active session (the real orchestrator) — all July ghosts + launch_pending relics reaped. |
| **F33** | **PARTIAL LIVE** | Spawned claude carries `--settings` with `permissions.allow: [mcp__devboule__*, Read, Write, Edit, …]` + acceptEdits headless path (app log line confirms F33 branch). `agent_register` lane proven (ledger lastSeenAt updates on spawn). Behavioral Write proof BLOCKED by F46. |
| **F36** | CONFIRMED LIVE (again) | `CLAUDE_CONFIG_DIR=<projects_dir>/claude-agent-config/<agent>` on the spawned process. |
| **F46 (NEW, MAJOR)** | PARTIAL FIX | F36 isolation removed auth: isolated config dir has no credentials → claude session transcript = `Not logged in · Please run /login`; every cloud-claude agent is chat-dead. Fix shipped: seed `~/.claude/.credentials.json` into the isolated dir (0600, stale-aware) + `claude_auth_env_passthrough()` helper. On THIS Mac credentials live in the macOS Keychain (no file) → still logged out. **OWNER DECISION NEEDED**: (a) `claude setup-token` once and store `CLAUDE_CODE_OAUTH_TOKEN` for the app env, or (b) approve app-side seeding of `oauthAccount` from `~/.claude.json` into the isolated `.claude.json` (untested hypothesis: CLI then reads the shared keychain item). Claude-session policy blocked direct auth-state manipulation during e2e (correctly). |
| **F47 (NEW, KILLER)** | FIXED (code+review) | Clicking the Local orchestrator chip froze the whole app. Sample stack (captured live): main thread → `vault::read_cloud_llm_key` → `keyring get_password` → SecurityAgent ACL prompt (post-relink signature change). Same class as F31 but on the cloud-LLM-key path: Tauri v2 runs SYNC commands on the macOS main thread. Fix: ~20 keyring-reaching sync commands converted to async + `spawn_blocking` (launch/sidecar path, key status/save/delete, github, devices, git push/pull/clone, workspace snapshot/hygiene). Deepseek review: 2 misses found and closed; SHIP. Live re-verify pending app relaunch. |

Open after round-5: F46 owner decision; F37/F38 DoubtPanel live proof (needs a working orchestrator: cloud-claude blocked by F46, local blocked by the keychain ACL grant — F47 fix keeps the UI alive but the prompt still needs the owner once); F40c vitest hang root-cause; F30 live censor re-run.

### Round-5 live pilot results (2026-07-22, after F47/F48 builds)

| ID | Live verdict | Evidence |
|----|--------------|----------|
| **F47** | **CONFIRMED FIXED LIVE** | Same action that froze the app (Local chip + send) now leaves the UI fully responsive; `sample` shows zero pending keychain calls on main thread; orchestrator lane spawns and works. |
| **F48 (NEW, root cause of the F37/F38 invisibility)** | **CONFIRMED FIXED LIVE** | `useStageRotation` defaults to "exa" and the incoming-doubt effect expanded the drawer but never selected the "plan" view → DoubtPanel (rendered only under `view==="plan"`) was never visible on ANY client. Fix: doubt arrival (or mount with open doubts, via prevLen=0) forces `pick("plan")`. Live: question "Which color theme…" renders in the DoubtPanel. Bonus: the fix exposed a latent crash — optionless questions blew up on `q.options.map` (now guarded + tested). |
| **F37** | **CONFIRMED LIVE** | Computed styles on the live card: question 14.5px, options 13px. |
| **F38** | **CONFIRMED LIVE** | Triple-click on an option → exactly **1** steer in the backend store (`For "…" — go with …`), card optimistically dismissed (earlier "still visible" reading was the steer's chat echo matching the substring). The 6 duplicate sends visible in the store are yesterday's pre-fix relics. |
| **Orchestrator loop** | HEALTHY | Answer received → `project_append_note` + heartbeat + status done + chat ack. MCP lane green end-to-end. |

Suites at close: cargo 3535/0 (`--features ui-pilot`), oracle-core 26/0, vitest default 2233/0 (199 files). Residuals: F46 owner decision (claude cloud auth under CLAUDE_CONFIG_DIR isolation: `claude setup-token` → env, or approve oauthAccount seeding); F30 live censor re-run on an agentic write (code fix probe-proven); F40c vitest DesignView hang root-cause.

### F46 addendum (2026-07-22 sera)
- Setup-token wired end-to-end: vault `provider:claude_oauth_token` (write-only) + async commands + `CLAUDE_CODE_OAUTH_TOKEN` injected on every claude spawn (PTY + duplex) + Settings field under the orchestrator's Agent CLI section.
- **oauthAccount seed hypothesis REFUTED live**: with `oauthAccount`+`userID` seeded into the isolated `.claude.json` (0600, atomic), a fresh spawn still reports "Not logged in" — macOS keychain credentials are not reachable from the isolated config dir. The seed stays (harmless; would work where credentials are file-based). On macOS the setup-token is THE path.

### F46/F33 CLOSED LIVE (2026-07-22 sera)
Owner generated `claude setup-token` (new flow: browser auth auto-returns, token printed in terminal) and saved it in the new Settings field. Fresh spawn: `CLAUDE_CODE_OAUTH_TOKEN` present on the child env (verified, value never read), claude authenticated headless, `f33-proof.txt` written via the Write tool with exact content. **F33 behavioral proof complete; F46 closed** (token path; oauthAccount seed remains as harmless fallback for file-based-credential machines).
