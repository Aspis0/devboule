# Projects page — bugs & features backlog (2026-06)

The first Projects page is becoming the heart of the app: **create a project = talk to the
orchestrator to shape a plan (a project IS a plan), which is then built by a coder**. It's a
**vibe-coding chat**. This doc tracks what's left to fix/build, from a live testing session
with the owner. Companion: `agent-roles-architecture-2026-06.md`.

## Mental model (settled)
- **Project = a plan.** You CREATE it on this page by talking to the orchestrator; the plan
  is split into tasks at the end; you then follow/run it on the single-project page.
- **A project depends on a working folder** (existing codebase, e.g. an Android app where the
  project is "a new page"; or a brand-new folder).
- **Names**: keep the internal agent names (orchestrator / main coder / mini-coder / censor)
  — DON'T rename in the UX; just EXPLAIN well what each does. "Planner" = orchestrator + main
  coder (same role, different phases; can be two different agents) — kept as our dev shorthand.

## DONE this session (context)
- Landing opens EMPTY; first message CREATES the project (with the chosen folder) + launches
  the Planner (`startNewProject`, commit `ef1e298`). Chat-persistence fixes: the chat no
  longer wipes on project creation / folder set; double-launch guard (`plannerLaunching`).
- Chat ordering fix: the bridge conversation is the chronological authority; optimistic
  notices no longer push a reply above the message it answers.
- Title placeholder "Untitled plan" (the name is decided in conversation — see B7).

---

## BUGS / FEATURES TO FIX

### B1 — It's a vibe-coding CHAT, not run-once by nature  ⚠️ ARCHITECTURE
**What:** the local orchestrator runs through `devboule-coder` `run_once` — designed as a
headless ONE-SHOT that exits on the first "Done". We had to bolt on a "stay alive and wait
for the next message" loop so the conversation doesn't die after one reply. That's a smell:
a vibe-coding agent should be a **persistent conversation by nature**, not a one-shot with a
keep-alive hack.
**Where:** `devboule-coder/src/main.rs` `run_once` (the conversation loop).
**Fix (proper):** make the orchestrator a first-class **interactive conversation session** —
its natural mode is "converse until the user/plan is done", with plan-submit / task-creation
as events within the conversation, not a terminal "Done" that ends the process. Re-think
`run_once` vs an interactive `run_session`.
**Severity:** high (architectural; the current keep-alive works but is fragile).

### B2 — Orchestrator backend selectable: local / Claude / Codex
**What:** the orchestrator (the one you talk to) must be choosable among **local devboule**,
**Claude**, **Codex** — same three as the main coder (per `agent-roles-architecture`).
- **Local** keeps OUR TUI (the Stage: chat + plan + websearch/design views).
- **Claude / Codex** keep their OWN terminal — we simply **embed/show their terminal here**
  (reuse `AgentTerminalViewer`), not the full Stage.
**Where:** `ProjectsView.tsx` (`planWithOrchestrator`/`startNewProject` hardcode
`client:"orchestrator"`); needs an orchestrator selector + conditional render (Stage for
local, terminal for cloud). `launch...` already accepts `client:"claude"|"codex"` (gate
`planFirst` on local).
**Severity:** medium-high (core to the multi-provider vision).
**Status (2026-06):** selector + cloud-terminal embed shipped (b3a4222 + 20595e0). Hostile
review found the cloud path needs REAL integration (cloud CLIs don't share the local
goal/steer/bridge infra):
- ✅ F3/F6 fixed: `plannerLaunching` now clears on a cloud bind (was a 30s composer lockout);
  `live` counts a cloud orchestrator (chip pulses).
- 🔴 **F1 (cloud goal):** Claude/Codex launch BLIND — `initialGoal`/`DEVBOULE_GOAL` only flow
  for `client==="orchestrator"` (`projects.rs` OrchestratorLaunchConfig is None for cloud). Fix:
  embed the goal in the cloud CLI's prompt (`build_*_agent_script`).
- 🔴 **F2 (terminal binding):** `cloudOrchestratorAgentId` matches by `client` only → after a
  hand-off it can bind to a CODER of the same CLI. Fix: capture the launched agentId at launch
  (a ref/state), don't `.find` by client.
- F4/F5 (deferred): no steer path for a live cloud orchestrator (you drive its own terminal —
  acceptable); switching the selector while one runs can leave two orchestrators (disable the
  switch while running). F8: autoCreate env encoding nitpick.
This is Phase-D-adjacent: a cloud orchestrator is a real integration, not just a launch.

**💡 KEY SIMPLIFICATION (the owner, 2026-06-23):** the TUI to talk to Claude/Codex ALREADY exists
and is ALREADY reused. The single-project (Work mode) page uses **`AgentTerminalViewer`**
(`ProjectWorkspace.tsx:78` — a real xterm whose input flows to the agent PTY via
`agent_pty_write` + a reply bar; "the same one the Agents room uses, one at a time"). That IS
how we talk to Claude/Codex today. **B2 part 2 already mounts that same `AgentTerminalViewer`**
in the planner for a cloud orchestrator — so the INTERACTION is solved by reuse (no new
chat/steer path needed for cloud; the review's "no steer for cloud" F4 is a non-issue — you
type into the terminal). The ONLY remaining cloud work is therefore small + launch-side, NOT a
new TUI:
- **F1** — pass the typed goal to the cloud CLI's prompt at launch (it currently starts blind).
- **F2** — bind the planner's `AgentTerminalViewer` to the RIGHT session (capture the launched
  agentId at launch instead of `find`-by-client, which collides with a hand-off coder).
So "cloud orchestrator" reduces to: launch the existing CLI with the goal + show it in the
existing terminal component. Much smaller than it first looked.

### B3 — Universal websearch UX (Phase D)
**What:** the top **websearch view** must show the SAME cool UX whether it's **Exa via local
devboule** OR **Claude/Codex's integrated web search**. Today only the local Exa path feeds
the Stage.
**Fix:** per-provider **activity adapters** that parse each provider's own web/tool activity
and normalize it into the SAME bridge events (`websearch`/`chat`) → identical Stage rendering
+ animations. (This is the big "Phase D universal activity engine".)
**Where:** `devboule-coder` ExaBackend → `Activity::websearch` bridge today; need cloud
adapters. `mini_activity.rs` parse + `StageWebsearch` consume already exist.
**Severity:** medium (big epic; after B1/B2).

### B4 — The TUI grows unbounded as messages pile up  ⚠️ UX BUG
**What:** the chat (our TUI) **keeps getting taller** as messages increase, blowing out the
app layout. It needs a **fixed/bounded height with internal scroll** (scroll inside the TUI,
newest at bottom, auto-stick-to-bottom).
**Where:** `PlannerChat.tsx` (the messages container) + the planner panel height in
`PlannerPlanMode.tsx` / `planner.css`.
**Fix:** give the chat a bounded height (e.g. max-height / flex with `min-height:0`) and
`overflow-y:auto` on the messages list; keep the near-bottom auto-scroll already present.
**Severity:** high (makes the app unusable after a few turns).

### B5 — Working folder not persisted when the app self-locks  ⚠️ DATA BUG
**What:** the chosen working folder is **lost when the app locks itself** (auto-lock /
re-lock). The project ends up with `rootPath: null` again.
**Where:** project create/update path (`create_project` / `update_project_metadata`) +
the lock/unlock lifecycle (`state.ensure_unlocked` / what happens to project state on lock).
Investigate whether the folder is written to the project `.md` durably vs held only in
memory, and whether a re-lock reloads stale project state.
**Severity:** high (data loss; directly blocks planning since a folder is required).

### B6 — Landing didn't show the live conversation  ✅ FIXED
**What looked like:** "the orchestrator doesn't respond." **Actually it DOES respond** — the
activity file proved it ("come stai" → "Tutto bene, grazie!…") — but the reply only showed in
the SINGLE-PROJECT work view (recognized as a coder), never on the create landing.
**Root cause:** the orchestrator registers its session via MCP with **`host=null`**, but the
landing bound the chat with `client==="orchestrator" && host==="app"` → never matched → the
landing never bound to the live session.
**Fix (done):** bind by `client==="orchestrator"` only (currentProjectSessions is already
scoped to this project + active sessions). tsc 0, 107 view tests.

### B10 — Orchestrator is TOO EAGER: auto-plans + auto-creates + auto-hands-off on message 1
**What:** the very first message makes the orchestrator immediately draft the plan, create
the tasks (they appear in the Kanban), and hand off to the main coder — instead of having a
CONVERSATION to shape the project/plan first (vibe coding). Too eager.
**Cause:** launched with `planFirst:true` + `autoCreate` → it plans + creates on turn 1
rather than conversing. Ties to **B1** (conversation-native): the orchestrator should discuss
first and only plan/create/hand-off when the conversation converges (the user signals ready),
not on the opening message.
**Severity:** high (it's the core vibe-coding UX — discuss, THEN plan).

### B7 — The Planner should NAME the project during the conversation
**What:** the project is created as "Untitled plan"; the **name should be decided during the
conversation** (the orchestrator renames it once the plan takes shape).
**Where:** needs an orchestrator capability/tool to set the project title (e.g. via an MCP
tool or the plan-submit carrying a title) → `update_project_metadata`.
**Severity:** low-medium (placeholder works meanwhile).

### B8 — Move the per-project detail panel OFF the landing; Kanban = history
**What:** the landing should be CREATE + the **Kanban as the project history** (click a card
→ that project's page). The per-project detail panel (status header + root editor + saved
workflows) belongs on the single-project page, not the create landing.
**Where:** `ProjectsView.tsx` board-mode render (the `<main>` detail block); keep
`<ProjectsBoard>` + `<ProjectCalendar>`.
**Severity:** medium (layout cleanup; agreed with the owner — "tieni la kanban, la history è la
kanban stessa").

### B9 — Explain what each agent does (no rename)
**What:** keep the agent names, but add clear **explanations/tooltips** of what the
orchestrator / main coder / mini-coder / censor do, so non-developers understand.
**Where:** the planner controls + wherever a role/agent is surfaced.
**Severity:** low-medium.

### B11 — Can't delete a project  ⚠️
**What:** there's no way to delete a project. The list fills with junk ("hola", "Untitled
plan", …) from testing, with no cleanup.
**Where:** `ProjectsView.tsx` (list / Kanban card / detail) + a `delete_project` (or archive +
purge) backend command — check whether one exists; if archive exists, expose a real delete.
**Severity:** medium-high (clutter + no recovery from test runs).

### B12 — Orchestrator shouldn't be on the single-project page by default
**What:** on the single-project page the orchestrator session is present from the start. But
the orchestrator is the **create-time** conversation — it couldn't have existed "at the
beginning". On the single-project page it should NOT appear automatically; instead you should
be able to **re-invoke** the orchestrator on demand (to change the plan / modify tasks) if you
want it. So: orchestrator = create flow + opt-in re-invocation, not a permanent project agent.
**Where:** Work-mode session display + how the create-time orchestrator session is associated
with / shown on the project (it lingers as a project session). Relates to B10 (after it plans
+ hands off, it shouldn't stick around as the project's agent).
**Severity:** medium (model/UX clarity).

### B13 — "Changes" view shows the whole working-tree diff, not the project's changes
**What:** the Changes section on the single-project page is genuinely nice, BUT it shows ALL
git changes in the working tree — e.g. unrelated edits to the SAME repo (the owner saw OUR edits
to `ProjectsView.tsx` while the project's working tree was the Devboule repo). It
should show only the changes THAT PROJECT's agents made, not the whole tree's diff.
**Where:** the changes/diff backend (`changes.rs`?) + the Changes view — scope to the
project's agent activity (e.g. a baseline ref captured when the agent starts, or the agent's
own commits), not a raw working-tree `git diff`.
**Severity:** medium (correctness of a good feature).

### B14 — Liveness in the chat TUI: streaming + "thinking/working" indicators
**What:** there's a noticeable wait between sending and the first reply (normal — model
warmup + first token). The chat feels dead during it. Add liveness:
- **B14a (quick win):** an immediate "thinking…/working" indicator the instant you send
  (animated dots / "Planner is thinking…"), shown until the first token arrives. Frontend-only.
  ⚠️ MUST be gated on the planner being ACTUALLY active (live OR launching) — a first naive
  attempt keyed it on "last message is the user's", which wrongly showed it with no planner
  running (e.g. after the session dies in B15). Thread a live/launching signal, don't infer
  from the message list alone.
- **B14b (bigger):** **token streaming** — render the reply token-by-token as it generates,
  instead of one complete bubble at the end. Needs the orchestrator to emit incremental chat
  DELTAS to the bridge (a `chat-delta` activity event), `mini_activity` to coalesce them into
  the live chat entry, and `PlannerChat` to render the growing text. The design pipeline
  already streams (`startDesignGeneration` onText accumulates) — reuse that shape for chat.
- Optional: small status lines for what the AI is doing (reading files, searching, planning)
  — these already exist as milestones/websearch on the Stage; surface a compact form in/near
  the chat too.
**Where:** `PlannerChat.tsx` (indicator + incremental render); `devboule-coder` burst
(emit chat deltas) + `mini_activity.rs` (coalesce) for streaming.
**Severity:** medium (perceived performance / it feels alive).

### B15 — Steering the live orchestrator kills it + the chat resets (history lost)  🔴 BLOCKER
**What:** asked the live orchestrator a follow-up ("can you call the designer for design
help?") → the orchestrator **turned off** (session ended), the **chat reset** (the prior
replies vanished), and **no response**.
**Two distinct failures:**
- **(a) The steer ends the orchestrator** instead of continuing the conversation. The 2nd
  turn kills it. Suspect: `run_once` `wait_for_steer_reply` (does it actually drain the steer
  and continue, or time out / exit?), or the design-related turn crashing the burst. Needs
  the activity file + whether the steer reached `DEVBOULE_STEER_FILE` and was drained.
- **(b) The chat is bound to the LIVE session, so when it closes the conversation VANISHES.**
  `plannerConvo` reads the bridge of the live `orchestratorAgentId`; when the session ends,
  `orchestratorAgentId` → null → the bridge (assistant replies) disappears and the chat
  collapses to just the optimistic user messages. The conversation must **persist from a
  durable source** (the activity file / a stored transcript per project) regardless of whether
  the orchestrator process is currently alive — like any chat app keeps history.
**Severity:** BLOCKER (the conversation collapses on the second turn). Tightly coupled to B1
(conversation-native) and B6 (binding). Likely fix B15b (durable transcript) + B1 together.

### B16 — Designer-help request inside the conversation (the design_request loop)
**What:** the user wants to ask the orchestrator, mid-conversation, to call the **designer**
for help ("can you call design?"). This is exactly the Phase-B `design_request` flow — but it
must work as a natural conversational request (the orchestrator decides to call the designer,
the result lands in the Design view) without killing the session (see B15a).
**Where:** Phase-B `design_request` (already built) + the orchestrator deciding to invoke it
from the conversation; verify the design_request turn doesn't crash/exit the burst.
**Severity:** medium (depends on B15a being fixed first).

---

## Suggested order
✅ **B6** done (binding). Then:
1. **B11** (delete projects — quick win, clears the test junk) →
2. **B5** (folder persistence — blocks planning) →
3. **B4** (scrollable TUI — usability) →
4. **B10 + B12** (stop the eager auto-plan/handoff; orchestrator off the single-project page
   by default — both are the "discuss first, orchestrator is the create flow" behavior) →
5. **B1** (make the conversation conversation-native, not run-once) →
6. **B13** (scope Changes to the project) →
7. **B2** (orchestrator selector + cloud terminal) →
8. **B8 / B9 / B7** (layout, explanations, naming) →
9. **B3** (universal websearch — the big Phase D epic).
