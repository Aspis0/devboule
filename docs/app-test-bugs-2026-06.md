# App manual-test bug log — 2026-06-19

Bugs + ideas from the owner's hands-on test of the dev build (`npm run tauri dev`). **Collect all,
fix at the END** (the owner's directive — not one-by-one). Each entry: status, what, where, fix idea.

Legend: 🔴 bug · 💡 idea · ⚪ observation (low priority) · ✅ fixed

---

## FIX STATUS (2026-06-19, committed on `mac-platform-fixes`, NOT pushed)
✅ **R1** working folder + folder picker, no app-dir fallback (`1442d04`) · ✅ **R2** plan→tasks
round trip — agent instructed to call `project_create_plan_tasks` (`2e09eba`, FIXED missing-`id`
in `5a0d87b`) · ✅ **R3** agent `agent_heartbeat status=done` on completion (`21e9ff5`) · ✅ **R4**
task-card launch → in-app PTY, visible (enters work mode) (`2a485e5`+`5a0d87b`) · ✅ **S8** oMLX
base-url default (`ac9778e`) · ✅ **S1** Settings collapsible groups (`f8030a1`) · ✅ **S2**
capability-driven mini write, toggle kept (`4661afb`+`d4774fb`) · ✅ **B17** board drag-and-drop
(`802aff2`) · ✅ **max-recall review** of the whole diff → 5 blockers + 4 warnings fixed (`5a0d87b`).
All code written by LOCAL models (oMLX) where substantive. tsc + cargo + py + 364 tests green.

**B2 DIAGNOSIS (no repro needed — traced in code):** the orchestrator launch does
`resolve_omlx_env(backend)` where `backend = read_local_coder_backend(app)`. If that is **None**
(no local-coder backend configured — exactly what happens if you hit the S8 base-url error and
couldn't SAVE the omlx config), the DEVBOULE_OMLX_* env is empty and **the binary runs its silent
Mock** (`projects.rs:1496`) → it never POSTs to oMLX → "model doesn't auto-load" + "CLI stuck on
thinking". **So B2 ≈ S8**: with the base-url now prefilled you can save the omlx backend and the
binary reaches oMLX. REMAINING (needs the owner's re-test): (a) verify the POST loads the model;
(b) UX — surface "local main coder not configured" instead of a silent Mock when the orchestrator
launches with a None backend. → tracked as a B2 follow-up.

**STILL OPEN:** S3 (Claude as mini), S5 (Codex as local main), S6 (Apple on-device all roles),
S7 (Oracle-MCP to censor+capable-minis + Codex toml) — the role/MCP-system cluster; B10 (5-min
freeze — environmental/RAM, needs Activity-Monitor repro); B15 (in-app terminal text-doubling —
intermittent/cosmetic); **R7 + Plan-board UI** (parallel DAG runner + colored arrows + external
subagents visible — the big "super figo" feature, substantial).

---

## ROOT CAUSES + FIX PRIORITY (the ~29 entries collapse to ~6 roots)
The in-app LOCAL flow (local main + mini + censor) is end-to-end broken — but it's GLUE/PLATFORM/
UX, not architecture. What WORKS: Claude main coder (external CLI registers), the reviewer reasons
correctly (clean advisory verdict), the agentic backend (loop/tools/run, all tested). The gap is
the in-app local-orchestration glue. Fix by root, highest-unblocking first:

1. **R1 — Project WORKING ROOT not configured** (B3, P1; cascades to B9, B16, B18). No folder
   picker at project creation → file written in the app dir → project_structure/visual_check/
   Censor inert → review can't run. **FIX FIRST**: folder picker at creation + persist rootPath;
   all tools + write location + task binding key off it. Unblocks the most.
2. **R2 — Plan → tasks not materialized** (B9). Approved plan's phases don't become board tasks →
   empty board → review has nothing → whole loop blocked. FIX: on approval, phases → ProjectTask.
3. **R3 — Completion / liveness detection missing** (B11 + B13 + B16 + B2). App never detects
   agent-done / CLI-end / oMLX-load → agents stuck active, project stuck Active or reset to
   Planned (not Verified). FIX: one completion + session-end + model-load signal drives
   agent-status + the macro state machine (Planned→…→Verified).
4. **R4 — macOS launch broken** (B19 + B20). manual/coder launch uses PowerShell on mac + no CLI;
   the two options undifferentiated. FIX: platform-detect shell + surface a terminal +
   differentiate manual(copy-prompt) vs coder(app-managed).
5. **R5 — Settings UX + role availability** (S1–S8). submenus, capability-driven mini-write,
   add roles (Claude-mini / Codex-main / Apple-all-roles), Oracle-MCP to censor+capable-minis,
   the oMLX base-url error, recommended+downloadable models.
6. **R6 — Stability + polish** (B10 5-min freeze [likely RAM], B15 terminal doubling, B17 DnD,
   B7/B12 external-CLI UX, B14 + O1 noise).

Suggested order: R1 → R2 → R3 → R4 → R2-verify(local main↔oMLX) → R5 → R6.

## CROSS-CUTTING PRINCIPLE (the owner): SIMPLIFY + AUTOMATE
The whole thing feels too complicated to manage right now — too many manual knobs/steps. Apply
this lens to EVERY fix, don't just repair the complexity:
- **Fewer editable knobs / one source of truth**: config lives in Settings ONCE; don't duplicate
  it on the project page (P1 advisory model → read-only from Settings).
- **Automate decisions instead of asking**: mini write-mode is capability-DRIVEN, not a manual
  toggle (S2); recommended config per detected PC = one click (S4/R5); placement auto.
- **Collapse confusing options**: the "manual agent" vs "code/coder" launch (B20) — automate or
  merge into ONE clear action (the app picks copy-prompt vs app-managed by context); don't make
  the user choose between two things that look identical.
- **Automate state transitions**: the user should NOT have to click Stop or manually invoke a
  reviewer (R3) — detect done → advance Active→Review→Verified automatically.
- **Sane defaults + progressive disclosure**: short panels (S1 submenus), advanced stuff hidden.
Goal: a user opens a project → picks a folder → writes a task → it just runs + tracks itself.

---

## ⚪ O1 — repeated "Oracle is starting" console warnings (non-fatal)
On launch the app prints (×4) `Oracle post-unlock refresh step failed (non-fatal): Oracle is starting — the server is not ready yet`. The app auto-starts the Oracle runtime; until ready it warns. Non-fatal. Note: the `oracle/` service is aspis-bio's (see memory) — on devboule this is just startup noise. Low priority: debounce/suppress the repeated warning, or don't auto-poll Oracle 4× on unlock.

---

## Settings (the owner — batch 1)

### 💡 S1 — Settings layout: break long scroll lists into SUB-MENUS
Especially **Providers & Models** — right now it's one long list you have to scroll. Split into
sub-sections / sub-menus. Apply to ALL long settings panels. "Niente di che, non complicato" —
just nested sub-navigation so each panel is short. (Frontend; the settings tabs/cards exist —
add a second level of grouping.)

### 🔴 S2 — Mini write behavior: make it CAPABILITY-driven, not a global toggle
Today `miniWriteBehavior` is a single global Safe/Auto/AgenticAllowed. It should depend on the
**mini coder's capability** (its registry tier): a capable model (>20B / agentic-tier) → agentic
tool-loop; a small one → emit-edits. So drive the write mode PER-MINI from the model's
capability automatically, and update/replace the global setting accordingly. (Ties to the
agentic wiring: `should_run_agentic` already keys on `write_mode` + base_url; make the registry
tier the driver.)

### 💡 S3 — Add Claude subscription as a MINI coder
Allow Claude (subscription) to be selectable as a mini coder too — "ognuno fa quello che vuole":
any backend usable for any role. (Add Claude-subscription to the mini-coder backend options.)

### 💡 S4 — Model registry: list too long → reorganize WITH downloadable + recommended models
The registry is too long a flat list. Rework it around the **recommended + downloadable models**
idea: per detected hardware show recommended models per role, with install state
(installed / downloadable via `ollama pull` or load into oMLX / too big for this PC) + a
download action. (This is the capstone recommend feature + S1's sub-menu grouping.)

### 💡 S5 — Add Codex as a LOCAL MAIN coder
Codex should be selectable as a local **main** coder (not just whatever role it's limited to now).

### 💡 S6 — Apple on-device (AppleFM) available for ALL roles
Apple on-device model is currently offered ONLY for Censor. Make it selectable for every role —
mini coder, main coder, censor, oracle, etc.

## Account / Oracle (the owner — batch 2)

### 🔴 S7 — Oracle MCP grant only wired for Claude; extend to Censor + CAPABLE mini coders
Settings → Account: the Oracle MCP grant appears available only for **Claude**. (Codex MCP still
needs its `toml` dependency added — known pending item.) Expected: Oracle callable ALSO by
**Censor** and ESPECIALLY by **"capable" mini coders** (the agentic-tier >20B minis). →
extend the Oracle-MCP grant / role matrix beyond Claude to censor + capable minis; finish the
Codex MCP toml wiring.
- Oracle is **folder-agnostic** (indexes whatever project folder it's pointed at) — so it
  serves any role on the current project. The fix is purely: widen the MCP grant beyond Claude
  (→ Censor + capable minis) + add the Codex MCP toml dependency. No scope ambiguity.

## Projects + Settings (the owner — batch 3)

### 🔴 P1 — Project page: "advisory model" independently editable, doesn't match Settings main coder
Creating/opening a project: the CLI is split into codex / claude / local (ok). BUT the
**advisory model** field is editable there AND does NOT correspond to the main-coder model set
in Settings → duplicated/conflicting config, too many editable knobs on the project page. Fix:
either make advisory model a **picker (table of installed models)**, OR BETTER — **not editable
from the project page; configured ONLY in Settings** (the project page reflects the Settings
value, read-only). the owner prefers the read-only-from-Settings option.

### 🔴 S8 — Settings: oMLX backend config shows an error (BACKEND / MODEL TAG / BASE URL)
In Settings (Providers & Models, backend config) an error shows around the backend fields:
`BACKEND / MODEL TAG / BASE URL — "Enter the oMLX server base URL (e.g. http://localhost:8000/v1)."`
The oMLX base-url field is in an error/required state. Investigate: spurious validation error
vs. just the empty-field prompt rendered as an error. (Check the oMLX backend validation in
`ProvidersModelsTab` / `validate_omlx_base_url` + the form's error display.)

## Local main coder (devboule CLI) — batch 4

### 🔴 B2 — Local main coder (qwen via devboule CLI): NO evidence the oMLX model ever loaded (SERIOUS, unverified)
Created a local main coder (qwen), CLI=devboule, sent a task → CLI sat on "thinking"; the owner
NEVER saw qwen auto-load on oMLX. Evidence: oMLX `/health` showed `loaded_count: 0` (and the
ceiling had dropped to 26.3 GiB under app/dev-build RAM pressure). The CLI's only "response" was
the B3 escalation, which may have been produced WITHOUT the model running at all.
**RETRACTED my earlier claim that "it works / auto-load is just slow" — there is NO evidence the
model loaded.** Two possibilities, both serious:
  (a) FUNCTIONAL: the devboule LOCAL main-coder path doesn't actually send a chat request to
      oMLX for the configured model (so it never loads) — it escalates first.
  (b) VISIBILITY: oMLX loads+evicts it without it ever showing in /health / the UI.
MUST VERIFY (fix phase): trace the devboule CLI → does a local main coder POST to
`{base_url}/v1/chat/completions` with the registry model id? Watch oMLX `/health` during a run.
Confirm the model id/base_url the CLI uses matches the registry. Also: show a "loading model…"
state, and surface oMLX load status in the UI. Do NOT assume it works until a load is observed.

### 🔴 B3 — New project "test": devboule agent fails — no working root + no Censor backend
The local main coder planned, then the `plan` / `project_structure` tool FAILED + escalated
(runner.py:727):
> `plan: project_structure failed: Error executing tool project_structure: Project…`
> `⚠ escalated: Project has no configured working root for Censor findings; plan tool execution`
> `failed due to missing project configuration. Cannot proceed without Censor backend setup.`
Root cause: a freshly-created project ("test") has NO configured **working root** (the folder the
agent operates on) AND no **Censor backend** → the agent can't run project_structure + escalates.
Fix: the create-project flow must capture/persist a **working root**; ensure/prompt a Censor
backend; `project_structure` needs the root. This is the real blocker behind the "hang".
- UPDATE (the owner): haiku reported *"The file was created successfully (the visual_check needs a
  project root, which this test project doesn't have). Let me document the completion."* → so
  **multiple** root-dependent tools degrade for the same cause (project_structure, **visual_check**,
  Censor findings), and the agent DID create the file (so the agent works — the missing working
  root is the SYSTEMIC blocker). Also: with no root, WHERE did the file get written? (ambiguous
  write location is part of this bug). The single fix — capture/persist a project working root at
  creation — unblocks project_structure + visual_check + Censor + a defined write location.
- UPDATE 2 (the owner) — ROOT CAUSE + UX: the file was written **inside the aspis-management repo
  itself** (the app's own cwd) because the project had no working root → it fell back to the app
  directory. That's wrong (pollutes the app repo + not where the user wants). FIX: project
  creation must let the user **pick the working FOLDER** (a folder picker / path field), and that
  folder becomes the project root all tools use. Never default to the app's own directory.
  This is THE central bug of the test session — it explains B3 (tools need a root), the
  ambiguous write location, and likely contributes to B9 (no board tasks). Highest priority.

### 🔴 B4 — Codex CLI launchable even when codex isn't installed → hangs silently (no feedback)
On macOS (no `codex` binary installed) the app still lets you launch the Codex CLI. Result:
everything hangs, NOTHING shown — no way to tell it failed. Fix: **pre-flight detect** whether
the selected CLI binary is installed (which/`command -v` style) BEFORE offering/launching it; if
missing, disable the option or show a clear error ("Codex CLI not found — install it or pick
another"). Never hang silently on a missing binary. Generalize to ALL CLIs (claude / codex /
devboule): detect availability + surface launch failures (exit code / "binary not found") in the
UI instead of an indefinite silent "running" state.

### ✅ O2 — Claude main coder WORKS (external CLI, haiku recognized + registers correctly)
Opened an external CLI with Claude as main coder + model = haiku → recognized and registers
correctly. Positive data point: the orchestration + agent-registration path works for Claude.
This SCOPES B2 — the "model never loads" problem is specific to the LOCAL oMLX main-coder path,
NOT a general registration/orchestration failure.

### 🔴 B5 — Main coder set as ORCHESTRATOR behaves like a worker (asks for tasks + queries Oracle "what to do")
Claude registered correctly (O2), and was configured as an **orchestrator**. Expected: it
orchestrates — plans, breaks work down, delegates to mini coders. Instead, right after
registering it immediately started **asking the user for the tasks** and **querying Oracle for
what it should do**. → the orchestrator role/prompt isn't being applied; the agent falls into a
generic "ask for direction" flow rather than the orchestrator behavior. Fix: ensure the
orchestrator role injects the orchestrator system prompt (coordinate + decompose + delegate to
minis), distinct from a worker/ask-the-user flow. Check how the role is passed to the CLI/agent
prompt at launch.

### 🔴 B6 — Orchestrator over-calls aspis-management (clarification loops) on a trivial task
Asked the orchestrator for a plan to "just make an HTML" → it keeps calling aspis-management
(MCP) every time for clarifications etc. Excessive for a trivial, clear task. Fix: calibrate the
orchestrator prompt for **proportional effort** — don't repeatedly query the management MCP for
clarifications on a simple/clear request; just plan and proceed. (Pairs with B5: the orchestrator
behavior is mis-calibrated — under-orchestrates OR over-consults. Needs one calibrated prompt:
decompose + delegate to minis, minimal clarification, effort scaled to task complexity.)
- DESIGN CLARIFICATION (the owner): asking questions + searching online to produce a SERIOUS plan IS
  the INTENDED orchestrator behavior — exactly like Claude CLI's `/plan` (clarifying questions →
  research → rigorous plan). So do NOT strip the questioning; the only fix is **proportionality**
  (the HTML task was trivial → it over-did it; a real task SHOULD get the full /plan treatment).
  REQUIREMENT: the orchestrator needs **web-search capability** ("cerca online quando puo") as
  part of planning. So B6 = scale effort to complexity, NOT remove rigor; and ensure the
  orchestrator can actually search online. Model the orchestrator prompt on Claude's `/plan`.

### 💡 B7 — External CLI: clarification questions route through aspis (futile); make in-CLI for external, aspis only for internal
The CLI is EXTERNAL but the agent's clarification questions still route through the aspis app
(you answer in the app UI). Mechanically correct, but futile when you're working in the external
CLI — the Q&A should happen directly IN that CLI. Suggestion: make it automatic — route
questions through aspis ONLY for the INTERNAL (in-app) CLI; for an external CLI, ask + answer
directly in the terminal. (the owner flagged "due bug" — 2nd may follow.)

### 🔴 B8 — Plan approval requested via app, but the plan ISN'T shown in the app
The agent asked for **plan approval through the app** (not the CLI), but the **plan content was
not visible** in the app → you're asked to approve a plan you cannot read. Fix: the app's
approval UI must DISPLAY the proposed plan (steps/content) alongside the approve/reject buttons.
(Pairs with B7: for an external CLI the approval arguably belongs in the CLI; regardless, the
plan must be shown wherever approval is requested.)
- UPDATE (the owner): the plan IS visible elsewhere — scroll down to **Plans** → click "plans" and
  the approved plan renders fine. So the rendering already exists; the fix is just to SURFACE/
  LINK that existing plan view from the approval prompt (show it inline, or jump to Plans) so you
  never approve blind. Low effort — reuse the Plans view component in the approval UI.

### 🔴 B9 — Approved plan produced NO tasks; the task board is empty
A plan was created + approved (trivial: make an HTML file), but the task board is completely
EMPTY — no tasks generated from the plan. Expected: an approved plan decomposes into tasks on the
Kanban (plan steps → ProjectTask entries). Either the plan→tasks generation didn't run/failed, or
the board isn't displaying tasks that exist in state. Fix: verify the plan→tasks pipeline (an
approved plan must create tasks) + that the board renders them. (Side effect: with no tasks, the
frecce dependency-arrows have nothing to draw, and the whole task-board workflow is blocked.)
- UPDATE (the owner): the plan IS well-written and ALREADY divided into **phases** (visible in
  Plans). He expected those phases to appear on the board as tasks. So the decomposition exists —
  the gap is purely the mapping **plan phases → ProjectTask entries on the Kanban**. Fix: on plan
  approval, materialize each phase (and/or its steps) as board tasks (status todo), so the board
  reflects the plan. Lower effort than feared — reuse the plan-phase data already in Plans.
- UPDATE 2 (the owner) — CASCADES INTO REVIEW: the Opus reviewer itself confirmed *"The Kanban is
  completely empty — 0 tasks in every column. There's nothing in review to claim or adjudicate.
  The only work signal is a coder's note claiming it created aspis.html. Let me check the Censor
  residual ledger … then inspect the actual artifact."* So the empty board doesn't just fail to
  display — it BREAKS the verifier flow (nothing to claim/adjudicate); the reviewer has to
  improvise off the coder's note + Censor ledger + the artifact. → B9 is HIGH impact (blocks the
  whole plan→task→review→done loop). The reviewer is robust (adapts), but the board MUST get the
  tasks. (Artifact is `aspis.html`.)

### 🔴 B10 — App freezes / closes itself after ~5 minutes (SERIOUS — stability)
The app "si richiude da sola (blocca)" ~5 min in. At check time the dev process (pid 7955) was
still alive + the claude-haiku CLI running, so not reproduced in-log yet; a Rust panic would
print to the tauri dev stderr (re-check the dev output when it freezes). Hypotheses (ranked):
  (a) **RAM exhaustion / swap freeze** — most likely. The oMLX ceiling dropped to 26.3 GiB under
      app + dev-build + node load; if oMLX loads an ~18 GB model on top, RAM blows → swap →
      freeze/forced-close. (Aligns with the ~5-min mark = when a model finishes loading.)
  (b) a 300s (5-min) timer/wall-clock cap mis-firing at the app level (check DEFAULT_WALL_CLOCK_*
      / any 300s timeout / vault auto-lock).
  (c) a deadlock — poisoned agent-state lock → executor thread panic cascade.
Fix: reproduce while watching Activity Monitor RAM + the tauri dev stderr for a panic; grep the
codebase for 300/`from_secs(300)` timers; watch memory growth. HIGH priority — makes a real
session impossible. (NB: a DEBUG dev build is heavier than a release build; confirm it also
happens in release.)

### 🔴 B11 — Macro Projects Kanban: finished project stays "Active", should move to "Review"
The agent finished the work, but in the BIG Projects Kanban (the macro ProjectsBoard:
Planned/Launching/Active/Review/Blocked/Verified — NOT the single-project task board, which was
always empty per B9), the "test" project stayed in **Active**. On completion it should transition
to **Review** (signal it needs reviewing). → the project-level status transition (Active →
Review when the main coder reports done) isn't firing. Fix: on agent completion, move the
project's macro status to Review (pending human/verifier). Check the completion → status-update
path (likely needs the same project working-root/config as B3).
- LINK (the owner's insight): **B11 + B13 are likely the SAME root cause** — the app doesn't detect
  agent completion / external-CLI termination. So neither the agent flips to inactive (B13) NOR
  the project moves Active→Review (B11). ONE detection fix (agent-done signal + CLI-end/MCP-
  session-end detection) should drive BOTH transitions. Fix them together.
- UPDATE (the owner): the agent EVENTUALLY went "stale" — so a staleness timeout EXISTS, but it's
  (1) LATE and (2) the WRONG semantic: a FINISHED project gets caught as "stale/abandoned"
  instead of recognized as "completed → needs Review." Need a **positive completion signal**
  (agent reports done) that immediately moves the project to Review — NOT the staleness fallback.
  the owner had to **manually invoke a Claude reviewer in-app** (workaround for the missing
  auto-transition). So: distinguish completed-OK (→ Review, auto) from stale/abandoned (→ other
  handling); don't conflate them.

### ❓ B12 — External CLI: must be closed manually (confirm + signal "done")
When the agent finishes, the external CLI session stays open. the owner asked whether it needs
manual close. ANSWER (the owner confirmed direction): an EXTERNAL CLI is a terminal the user launched
themselves → the app cannot reliably auto-close it, so **manual close is expected by nature**.
BUT the agent must clearly **signal completion** in the CLI ("✓ done") so the user knows to
close. For an INTERNAL (in-app) CLI the app CAN manage lifecycle (auto-close / mark done on
completion). Action: ensure a clear "done" signal in the external CLI; for internal CLI, wire
auto-close/done. (Pairs with B7 — external vs internal CLI handling.)

### 🔴 B13 — Closed external CLI manually, but the app still shows the agent ACTIVE (stale liveness)
After manually closing the external CLI, the app keeps showing the agent as **active**. → no
detection that the external CLI session ended; the agent's live-state never transitions to
stopped/done. The external CLI registers via MCP (token-bound); when it dies the MCP connection
drops — the app should detect that (MCP session end / heartbeat / claim TTL expiry) and mark the
agent inactive. Fix: add external-agent liveness detection → flip status to inactive when the CLI
is gone. (Ties to B11 stale macro-status + B12 external-CLI lifecycle + the B2 "no detection"
theme — a recurring gap: the app doesn't observe process/session death.)

### ⚪ B14 — Opus reviewer flags "No provider credentials configured (all missing)" on a local project
The manually-invoked Opus reviewer starts: *"No provider credentials configured (all missing).
Now reading the project."* For a simple LOCAL HTML test project (no Cloudflare/Scaleway/cloud
usage) this is irrelevant + slightly alarming noise. NON-BLOCKING — it proceeds to read the
project. Consider: contextualize/suppress the provider-creds check for local-only projects, or
make it informational rather than an "all missing" warning. Low priority. (Ties to B3 — the bare
test project has no config; the reviewer surfaces the absence. Positive: the reviewer DID start
+ proceed, so the manual-reviewer path works.)

### 🔴 B15 — In-app terminal (Claude) occasionally DOUBLES / duplicates text (render corruption)
The in-app Claude terminal works well but occasionally glitches: lines printed TWICE, text
fragments interleave, and the `❯ /model` prompt repeats between output lines (observed: "1. The
project has no rootPath…" shown twice; "the test project is meant to exe cis the full
coder→verifi r flow" garbled/split). Terminal-renderer bug — PTY output handling (likely a
double-write of PTY chunks, or line-buffer/reflow/resize duplication in the xterm component).
Fix: investigate the in-app terminal's PTY→renderer path (chunk dedup / reflow). (Matches a known
class of PTY-render corruption — cosmetic but confusing.)

### ✅ Reviewer confirmed B3 + B9 (strong validation)
The Opus reviewer (in-app terminal) independently diagnosed both core bugs + gave a clean verdict:
- Read `aspis.html` (154 lines) ADVISORY (no formal task) → "claims all check out … deliverable
  correct (~0.9 confidence)"; correctly did NOT mark anything done/blocked (verifier contract
  forbids it with nothing in review). **The manual reviewer path works + reasons correctly.**
- B3 confirmed verbatim: *"The project has no rootPath. That's why Oracle/Censor/visual_check are
  all inert and the coder's own visual_check failed with 'not under project root' — the file sits
  outside the project working root."* → exactly B3's root cause (no working root → tools inert +
  artifact written outside it).
- B9 confirmed: *"No tasks were ever created on the Kanban, so the work bypassed task tracking
  entirely."*

### 🔴 B16 — Stopping finished agents RESET the project to "Planned" (should be "Verified")
State observed: the two agents (`coder-…` = "RECONNECT", `verifier-…` = "WORKING") stayed marked
active despite having finished (reconfirms B11/B13 — no completion detection; "RECONNECT" =
the app lost the session but didn't mark it done). Clicking **Stop** on both WORKED. BUT the
project then jumped BACK to the initial macro-Kanban state "**Planned**" — when it should be
"**Verified**" (work created + reviewed/advisory-OK). So the macro project state machine is
doubly broken:
  (a) it never auto-advanced Active → Review → Verified on completion (B11), and
  (b) **Stop wrongly RESETS a finished project to Planned** instead of leaving it at / advancing
      it to its earned status.
Fix: Stop must NOT reset the project's macro status to the start; and completion + verification
must drive Planned → … → Verified. Same root detection gap as B11/B13 (no completion/verified
signal) + a wrong Stop-side transition. The full macro state machine
(Planned→Launching→Active→Review→Verified, +Blocked) needs the done/verified transitions wired.

### 💡 B17 — Task board: no drag-and-drop between columns (must click "Move")
Created a task (T1 "First step", Feature) on the single-project board → can't DRAG it between
columns; have to click the "Move" button (sigh). Add **drag-and-drop** across the Kanban columns
(todo/wip/review/blocked/done). NB: the board is CSS-grid with no DnD lib (per the frecce recon)
— adding DnD is real work, and it interacts with the frecce overlay (which anchors arrows to card
positions → arrows must follow a dragged card). Wire DnD → the same `moveTask` handler the Move
button calls (so status persistence is unchanged).

### 🔴 B18 — Manually-created task is orphaned: doesn't recognize the project
The task created from the board "remains avulso" — it doesn't recognize the project or anything →
inert: no project association, so agents/runner won't pick it up. Fix: a board-created task must
bind to the current project's context (project id + working root) so it's actionable + visible to
the runner/agents. Likely downstream of B3 (no working root/config → tasks can't bind to the
project). Verify the create-task flow stamps the project id + that the DAG runner / agents see
manually-created tasks (not only plan-generated ones).

### 🔴 B19 — "Launch manual agent" uses PowerShell on macOS (wrong shell) + no CLI to interact with it
Moved T1 "First step" to in-progress → clicked **Launch manual agent** → a Coder launched
(`coder-1781887291887`, shell = **PowerShell**) stuck in LAUNCHING. On macOS PowerShell is the
WRONG shell (it's the Windows shell) → the launch can't run. AND there is NO internal or external
CLI surfaced → impossible to talk to the agent. Two coupled fixes:
  (a) **platform-detect the shell** — sh/zsh/bash on macOS/Linux, PowerShell only on Windows. The
      manual-agent launch path still hard-picks PowerShell on mac (the `*_launch_script` platform
      selection / `build_mini_command` Windows-vs-macOS branch is mis-selected for this path).
  (b) the manual-agent launch must surface a USABLE terminal (internal CLI) or a copyable prompt
      for an external one — never leave a LAUNCHING agent you can't reach.
HIGH priority on mac. (NB: this is on the `mac-platform-fixes` branch — a platform path was
missed for the manual task→agent launch.)
- SCOPE (the owner): the OTHER launch option, **"Launch code"/coder, does EXACTLY the same thing** —
  same PowerShell-on-mac + no-CLI bug. So B19 affects BOTH launch buttons on the task card, not
  just "manual".

### 🔴 B20 — Task-card launch options ("manual" vs "code"/coder) are identical + undifferentiated
The task card offers two launches ("Launch manual agent" and "Launch code") but they behave
IDENTICALLY and the UI never explains the difference (if any). Expected (per the code's
`onCopyManualPrompt` vs `onLaunchCoder`): **manual** = copy a ready-to-paste prompt for YOUR OWN
terminal; **coder** = an app-launched/managed agent (PTY/internal CLI). Either the manual=
copy-prompt path is broken (it spawns an agent like coder instead of copying a prompt), or they
are confusingly redundant. Fix: make them actually differ (manual→copy prompt; coder→app-managed
agent) AND label/tooltip each so the difference is clear. (Coupled with B19 — both currently land
in the same broken PowerShell launch.)

<!-- more batches below as the owner sends them -->

<!-- more batches below as the owner sends them -->

