# Work Console — "Living Plan + Focus/Split" (design + plan, 2026-06)

## TL;DR

Fuse the two Work-mode surfaces (the big **raw xterm terminal** `AgentTerminalViewer` +
the small structured **Console** `AgentConsole`) into **one professional unified console**.
It carries the app's **Polis** metaphor — agents *inhabit* the codebase (files = buildings,
features = districts) — but as a **sober schematic blueprint**, NOT the isometric PixiJS map,
NOT a cartoon village, no emoji.

Two columns + a thin strip:

- **LEFT — Living Plan navigator** (the hero): schematic of active districts; files are nodes;
  an agent *inhabits* the node it edits (marker + `coder · round 3`); mini-coder nested under
  its parent; orchestrator at the civic root; live pulses; selecting a node drives the right.
- **RIGHT — Focus stage**: structured activity stream of the selected node (reusing the shared
  renderers) with an `[ Activity | Raw ]` flip (Raw = the PTY terminal) and **split** to pin two.
- **BOTTOM — Censor strip**: sober inspection row (`◐ inspecting · ✓ CLEAN · ⚠ N findings`);
  DIRTY highlights the node coral (reuses verdict events; no fire/village vibe).

---

## Why this, and why it's unique

Researched the 2026 agent-harness field (Datadog agent flow chart, claude-devtools subagent
trees, swarm canvases, AG-UI, Claude Agent Teams split-pane / Agent View). They all treat
**agents** as the protagonists and **work** as scrolling text. None fuse an explicit **plan**
+ a **codebase-as-city** + a **review gate** into one living surface — because none of them
*have* those. We do: the devboule task DAG, the Polis city, and the Censor.

So the Work Console is not a bolt-on UI: it's the live fusion of three things we already built
(plan + Polis + Censor), expressed at console scale.

### Coherence with Polis (already exists)

From `src/types/city.ts` + `src-tauri/src/polis/` + `src/components/polis/AgentLayer.ts`:

- File = `Building`, feature/domain = `District`.
- Agents already walk the city to `Agent.currentFileId`; `figureForType` maps type → figure.
- Main coder = builder, **mini = watercarrier** selected by `Agent.parentAgentId`.
- `AgentSubagentBrief` (per-role subagents) already projected.
- DIRTY/`sins: UrbanSin[]` (fire), Oracle suspect smoke, kanban cards on buildings — all rendered.

The Work Console **shares the data model + palette + figure-mapping** with Polis but is a **new
lightweight CSS/SVG render** — it does NOT reuse the PIXI renderer (Polis stays the iso map view).

---

## Decisions locked with the owner

- Scope of unification: ONE professional console (not the playful "Cantiere" with emoji).
- Keep the **focus + split on-demand** layout the owner liked; add the **Living Plan** (Polis,
  professional) as the left navigator where agents *inhabit* files, live.
- **Orchestrator IS included in Work mode** (so you can change the plan mid-run) — but NOT as
  an inline node in the new Work Console. Re-planning **RE-USES the existing orchestrator/planner
  console (`PlannerPlanMode`) 1:1**, overlaid on the Work page, seeded with the current plan as
  context. On re-approval the orchestrator sleeps and the Work Console returns (see Phase 6).
- **The bottom task board IS the Living Plan's twin, not a separate surface.** The existing DAG
  board (colored dependency arrows + cards, `ProjectsView.tsx:2619-2700`) and the Living Plan
  share ONE selection + ONE model: selecting/moving in one reflects in the other; a re-plan
  reflows both. The board "moves with the console."
- **The small Console is DELETED, merged into the big one — no duplicates.** The dock-tab
  `AgentConsole` + `MiniSteerBar` (`ProjectWorkspace.tsx:736-761`) are removed; their job moves
  into the FocusStage (Phase 4).
- Censor = sober inspection indicator, not fire.
- Raw vs structured: **structured (Activity) is the default**, Raw is a per-node flip.
- **TWO-WAY communication with EVERY agent is a CORE requirement (the owner, 2026-06-24):**
  the owner must be able to message any agent he selects, AND every agent must be able to ask
  HIM a question when it doesn't understand (ask → answer → continue). Both directions
  already exist in the backend (see the dedicated section below); the Work Console SURFACES
  them per node, it does not invent new channels.

---

## Surface map (today)

| Surface | Component | Source | Role |
|---|---|---|---|
| Big terminal (top) | `AgentTerminalViewer` `ProjectWorkspace.tsx:629-647` | native PTY `agent-pty://` | raw, interactive, 1 agent |
| Small Console (dock tab) | `AgentConsole` `ProjectWorkspace.tsx:736-761` | activity bridge `useAgentConsole` / `mini-activity://` | structured, read-only, 1 agent |
| Planner Stage (landing) | `PlannerPlanMode` + `StageWebsearch`/`PlannerChat` | activity bridge (orchestrator) | websearch/plan/chat, pre-launch |

The duplication we kill: chat rendered 2 ways (`PlannerChat` bubbles vs `AgentConsole` inline
`:554-558,572-574`); websearch 2 ways (`StageWebsearch` carousel vs `AgentConsole` `:549-571`).

---

## Two-way per-agent communication (CORE requirement — backend already exists)

the owner's requirement: **talk to every agent, and let every agent ask him a question when it's
stuck.** The backend already carries BOTH directions for every agent type — the Work Console's
job is to surface them per selected node, NOT to build new transport.

### There is only ONE kind of local coder

Correction to earlier confusion: there are NOT two separate local agents ("mini" vs "local
coder"). There is **one** local-coder machine driven through `mini_coder_directives`. The
size/behaviour is just a `WriteMode` flag on the SAME directive (`mini_coder.rs:475,552`):

- **>20B → `AgenticIterative` ("write")** — full agentic loop, writes whole edits.
- **<20B → `emit-edits` ("edit")** — one-shot emit of edits.

Because both go through the same directive layer, **both already have the same steer/ask
channel**. "Main coder" and "mini" are roles over the same machine, identical for messaging.

### Direction A — the owner → agent (steer)  [already wired per `agentId`]

| Agent | Command | Mechanism | Note |
|---|---|---|---|
| Orchestrator (local) | `orchestrator_steer(agentId,msg)` | `DEVBOULE_STEER_FILE`, drained between rounds | ✅ |
| Orchestrator (cloud) | `project_cloud_orchestrator_send(agentId,msg)` | piped stdin (`CloudDuplexSessions`) | ✅ |
| **Any local coder** (write or edit) | `mini_coder_steer(agentId,msg)` | `steer_queue` in `.aspis-agents.json`, folded as `SUPERVISOR STEERING:` at the next retry | ✅ |
| Cloud coder in a PTY (Claude/Codex worker) | — | only `agent_pty_write` (raw keystrokes) | ⚠️ **only gap** |

The **single real gap** is the cloud PTY worker: no structured message command, only raw TTY
bytes. Closed by a thin wrapper that writes `msg + Enter` via `agent_pty_write` (Phase 3.5).

> Honest constraint: every steer lands at the **next round/retry boundary** (the agent finishes
> its current tool-call, then reads the message as a human turn). It is a *steer*, not a
> mid-token interrupt. Cloud stdin is the most immediate; the local file/queue waits for the
> round edge.

### Direction B — agent → the owner (ask, when it doesn't understand)  [already wired]

| Agent | Surface in backend | How it reaches the UI |
|---|---|---|
| Orchestrator | `ask → answer → continue` loop already in `run_once` (commit `0ca070e`) | planner chat today; FocusStage tomorrow |
| Coder (cloud/PTY) | `ask_user` MCP tool — agents are instructed to call it and WAIT when BLOCKED (`agents.rs:869,941`); session carries `pending_question` (`agents.rs:602`) | render `pending_question` as a question card |
| Local coder (write/edit) | `MiniCoderStatus::NeedsClarification` + `question: Option<String>` (`mini_coder.rs:191,280,372`) | render the question; the answer routes back via `mini_coder_steer` |

So an agent's question is already a first-class state (`pending_question` / `NeedsClarification`).
The Work Console renders it as a **question card in the FocusStage** with an answer box; the owner's
answer routes back through that agent's Direction-A channel and the agent continues.

### Seam in the Work Console

Scope: the Work Console's composer handles the **worker agents** (coders/minis). **Orchestrator
interaction — both quick steer and full re-plan — is routed to the RE-USED `PlannerPlanMode`
console** (Phase 6), not the new FocusStage, so we don't duplicate the orchestrator chat UI.

For workers: one dispatcher `sendToAgent(node, msg)` picks the channel by node type
(local coder → `mini_coder_steer` · cloud PTY → pty wrapper). One inbound selector
`pendingQuestionFor(node)` reads `pending_question` / `NeedsClarification` off the session and
renders the question card. The composer is the SAME widget for messaging and for answering a
question.

---

## Claude Design prompt (for the HTML mock)

```
Design a single self-contained HTML mockup (CSP-safe: inline <style> only, NO external
scripts/CDNs/fonts, no emoji) for a NEW desktop console surface in an AI-coding-agent
orchestration app called "devboule". Match the app's existing mock convention
(design/project/*.html): warm professional palette, system-ui font, tasteful.

=== WHAT IT IS ===
The unified "Work Console" — one surface that fuses two old views (a raw xterm terminal +
a structured agent-activity timeline) into one professional console. It must carry the app's
"Polis" metaphor — agents INHABIT the codebase (files = buildings, features = districts,
agents = workers living on the file they edit) — but rendered as a SOBER, SCHEMATIC
BLUEPRINT console, NOT a cartoon city, NOT an isometric map, NO emoji, NO village vibe.

=== PALETTE (use these exact tokens) ===
cream bg #FBF8F2 / panels #FFFFFF / borders #E4DDD0 #EFE7DA / text #3B362F #2A2621
muted #9c9488 #A89F90 / teal #2F7E7A (orchestrator) / sage #7FA468 (clean/done)
terracotta #C0894F (active/coder) / amber #C8945C / coral #C2542F (dirty/alert)
indigo #5B6CC0 (mini). Soft shadows, 9-13px radii, 1px hairline borders.

=== LAYOUT: two columns inside one bordered panel, plus a thin Censor strip ===

LEFT (~38%) — "LIVING PLAN" navigator (THE HERO; this is the Polis, professional):
  A clean schematic of the ACTIVE districts. Each district is a labeled frame
  (e.g. "auth", "projects"). Inside, files are small nodes. An agent INHABITS the
  node it is editing: a filled status marker + a one-line label like "coder · round 3".
  - Orchestrator: a distinct marker at the civic root, teal, label "orchestrator · planning".
  - Main coder: terracotta marker on its file.
  - Mini-coder: indigo marker, NESTED/indented under its parent coder's node, with a
    thin connector line.
  - Idle/done files: faint hollow nodes (sage check if done).
  - Live agents pulse softly. Selecting a node highlights it (it drives the right column).
  Keep it architectural/blueprint: thin lines, district frames, dotted connectors — not
  buildings/skyline drawings.

RIGHT (~62%) — "FOCUS" stage for the selected node:
  Header: file path + district + agent + status ("projects · main-coder · round 3").
  A segmented toggle top-right: [ Activity | Raw ].
  - Activity (default): a structured stream of the agent's work — rows for: an edit
    (mono diff line, "+ const …"), a websearch ("websearch · 3 sources"), a chat bubble
    with a blinking caret for streaming, a Censor verdict chip. Professional timeline,
    generous spacing.
  - Raw: a dark terminal block (xterm style) — just show one state.
  A small "split" control in the panel header that (show as a second state/screenshot)
  pins TWO focus columns side by side.
  AT THE BOTTOM of every focus column: a MESSAGE COMPOSER (input + send) so the owner can talk
  to THIS agent directly — this is present for every node, not just the orchestrator. Show a
  hint like "message coder · arrives next round".
  When the agent is BLOCKED and has asked a question, show a distinct QUESTION CARD inline in
  the stream (amber accent, "coder asks:" + the question text) and the composer turns into an
  answer box ("answer to continue"). This is the agent asking the human, awaiting a reply.

BOTTOM — "CENSOR" inspection strip (full width, thin):
  Sober status row, NOT fire/emoji: e.g. "CENSOR  inspecting model.rs   login.ts CLEAN
  utils.rs - 2 findings". Use a small ring/dot glyph drawn in CSS/SVG, amber=inspecting,
  sage=clean, coral=findings.

TOP BAR: project name · "live" dot · right-aligned controls [ split ] [ replay ].

=== STATES TO SHOW (separate sections in the same HTML) ===
1. Multi-agent live: orchestrator planning + 2 main coders + 1 nested mini all inhabiting
   different files; right column focused on one coder showing edit+websearch+streaming chat,
   with the message composer at the bottom.
2. Censor bounce: one file flagged DIRTY (coral) in both the Living Plan node and the
   Censor strip, with a "back to coder" hint.
3. Split view: two focus columns side by side.
4. Agent ASKS a question: a coder node in the Living Plan shows a distinct "awaiting input"
   marker (indigo/amber ring, label "coder · asks"); the focus stream shows the QUESTION CARD
   ("coder asks: which auth provider should I wire?") and the composer is an answer box.

=== CONSTRAINTS ===
- Pure HTML + inline CSS, self-contained, opens in a browser, CSP-safe.
- Professional/restrained: think a high-end devtool (Linear/Datadog/Vercel), not a game.
- All glyphs via CSS/inline SVG, never emoji.
- Realistic fake data (real-looking file paths, a real-looking diff line, a plausible chat).

Return the full HTML.
```

---

## Implementation plan

**Method (the owner's rules):** code written by the **local models** via `/tmp/delegate.py`
(Qwen3.6-35B), **TDD-strict** (I write the failing contract test, the model makes it green).
I do the cross-file wiring. **1 hostile `reviewer` (sonnet) per phase** → fix → next.
**Max-recall final** (3 reviewers + adversarial) on the cumulative diff.

### Phase 0 — `WorkConsoleModel` data contract (substrate)
- New `src/components/work/workConsoleModel.ts`: derive a `district → file → agent(+activity)`
  tree from EXISTING feeds.
- Source for "which file an agent inhabits": session (`agentId/client/status`) + latest
  `edit`/action target from the activity bridge (`useAgentConsole`). District/file from the
  Polis map (`city.ts` `District`/`Building`) or, if absent, from path-prefix.
- Pure types: `WorkNode { agentId, type: orchestrator|coder|mini|censor, file, district,
  status, label, parentAgentId, activity }`.
- **Tests (vitest):** grouping by district; mini nested under parent; agent with no file →
  "unplaced"; selection.

### Phase 1 — Shared per-event renderers (dedup, already agreed)
- Extract `src/components/activity/ChatThread.tsx` (bubbles+caret, from `PlannerChat:107-229`)
  and `WebsearchView.tsx` (carousel+findings, from `StageWebsearch:116-221`). Pure, prop-driven.
- `PlannerChat` = header + `ChatThread` + composer. `StageWebsearch` = header + mode + `WebsearchView`.
- **Tests:** bubbles/streaming caret; carousel pages/findings; parity with the old render.

### Phase 2 — **Living Plan navigator** (left) — *hero candidate*
- `src/components/work/LivingPlan.tsx`: CSS/SVG schematic of active districts, files as nodes,
  agent **inhabiting** the node (marker + label `coder · r3`), mini nested, orchestrator root,
  live pulse, selection. Shares palette + figure-mapping concept with Polis; **new light render**.
- **Tests:** a live agent → marker on its file; mini indented under parent; click → `onSelect`;
  done → sage node.

### Phase 3 — **Focus stage** (right) + Raw flip + **two-way composer**
- `src/components/work/FocusStage.tsx`: structured stream of the selected node reusing
  `AgentConsole` + `ChatThread`/`WebsearchView`. Toggle `[ Activity | Raw ]` → Raw mounts
  `AgentTerminalViewer` (existing PTY).
- **Composer (Direction A):** a message input at the bottom of every WORKER focus column. One
  dispatcher `sendToAgent(node, msg)` picks the channel by node type: local coder (write OR
  edit) → `mini_coder_steer`; cloud PTY worker → the Phase-3.5 wrapper. (Orchestrator is NOT
  here — it is handled by the re-used planner console, Phase 6.) Optimistic echo into the
  stream; "arrives next round" hint.
- **Question card (Direction B):** `pendingQuestionFor(node)` reads `pending_question`
  (`agents.rs:602`) / `MiniCoderStatus::NeedsClarification` + `question` (`mini_coder.rs:280`)
  off the session; render an inline amber question card; the composer becomes an answer box
  whose reply routes back through the SAME Direction-A channel; agent continues.
- **Tests:** Activity renders the stream; Raw mounts the terminal; header file/district/status;
  `sendToAgent` dispatches to the right command per node type; a `NeedsClarification`/
  `pending_question` session renders the question card + answer routes back.

### Phase 3.5 — Close the ONE backend gap: structured message into a cloud PTY worker
- Only the cloud (Claude/Codex) coder running in a PTY lacks a structured channel. Add a thin
  Tauri command (e.g. `agent_pty_send_message(agentId, msg)`) that writes `msg + Enter` via the
  existing `agent_pty_write` — reuse, no new transport. (Local coders + orchestrator already
  have their channels; NO `steer_rx` surgery in `run_agent_loop` is needed — earlier analysis
  was wrong: the local loop is driven by the directive/retry layer that already drains steer.)
- **Method:** delegated to the local model, TDD-strict (failing test: a queued message shows
  up as bytes written to the PTY mock).
- **Tests:** the command writes the framed message+newline to the PTY for the given `agentId`.

### Phase 4 — Split + graft into Work mode + board twinning + delete the small console
- Split: `FocusStage` ×2 side by side (pin a 2nd agentId).
- **Graft:** `ProjectWorkspace.tsx` — replace the big terminal block (`629-647`) with the Work
  Console (LivingPlan + FocusStage). The rail is absorbed by the Living Plan.
- **DELETE the small console (no duplicates):** remove the dock-tab `AgentConsole` + the
  `MiniSteerBar` (`ProjectWorkspace.tsx:736-761`). Its read-only stream is now the FocusStage
  Activity; its steer input is now the FocusStage composer. Verify NOTHING else mounts
  `AgentConsole` as a second surface.
- **Board twinning (board moves with the console):** lift selection to ONE shared state
  (`selectedAgentId` already exists at `ProjectWorkspace.tsx:178`; extend to a
  `selectedNode = {agentId|taskId}`). The DAG board (`TaskDependencyArrows` + `TaskCard`,
  `ProjectsView.tsx:2619-2700`) and the Living Plan both read/write this selection: clicking a
  board card selects the Living Plan node (and drives the FocusStage) and vice-versa; both
  derive from the SAME `workConsoleModel`, so a re-plan reflows board arrows + Living Plan
  nodes together. Map node↔task via `agentId`/task id.
- **Tests:** ProjectWorkspace integration (renders console, NO double surface, no `AgentConsole`
  dock tab); selecting a board card updates the Living Plan selection and the FocusStage;
  selecting a Living Plan node highlights the board card; a model change reflows both.

### Phase 5 — **Censor strip** + live verdict
- `CensorStrip.tsx`: sober row `◐ inspecting · ✓ CLEAN · ⚠ N findings` (reuse `verdict` bridge
  events; DIRTY highlights the Living Plan node coral — no village fire).
- **Tests:** clean→sage, dirty→coral+count, inspecting→amber.

### Phase 6 — Re-plan from Work mode by RE-USING the orchestrator console 1:1
the owner's model: while working in Work mode you see the plan and want to change it → **recall the
orchestrator**. It behaves EXACTLY like the main-page orchestrator, only seeded with the current
plan as context. We do NOT rebuild this in the new Work Console — we **re-use the existing
`PlannerPlanMode` console 1:1**, shown over the Work page. On re-approval the orchestrator sleeps,
that console disappears, and the Work Console returns.

- **Affordance:** a "Change plan" / "Recall orchestrator" control in the Work Console (top bar
  and/or the orchestrator civic-root marker).
- **Mount:** trigger it → mount `PlannerPlanMode` (the SAME component the main page uses:
  `StageWebsearch/StagePlan/StageDesign` + `PlannerChat` + `PlannerControls`) as an overlay/mode
  in the Work page. Pass the **current plan as context** (the existing tasks/plan the user wants
  to change) so the orchestrator re-plans from it, not from scratch. Re-use its existing steer
  (`orchestrator_steer` / `project_cloud_orchestrator_send`) and `ask → answer → continue` —
  no new wiring.
- **Return:** on plan re-approval (the existing `PlanApprovalCard` path) → orchestrator goes to
  sleep, `PlannerPlanMode` unmounts, the Work Console (LivingPlan + FocusStage + board) returns,
  now reflowed to the new plan.
- **Risk:** removing/relaxing the rail filter (`ProjectsView.tsx:875-876`) must not break the
  main-page Plan Mode landing (shared component) — verify both entry points.
- **Tests:** recall mounts `PlannerPlanMode` seeded with current plan; approval unmounts it and
  restores the Work Console; the new plan reflows the Living Plan + board; main-page planner
  still works unchanged.

### Phase 7 — Max-recall + verification
- 3 `reviewer` (frontend race/leak · model/wiring correctness · removed-behavior/cross-file)
  + adversarial. Then `tsc`, `vitest`, `cargo test`, app build.

**Order / hero:** Phases 0→1 are substrate. Then lead with **Phase 2 (Living Plan live)** OR
**Phase 3 (focus/split)** depending on the chosen hero. 4→6 follow.

---

## Honest risks

- **(a) agent↔file live source** — if the bridge doesn't always carry the edit target file,
  fall back to session/task.
- **(b)** the Living Plan is a **new** render (NOT the Polis PIXI; shares data + palette only).
- **(c)** Phase 6 touches the rail filter → verify it doesn't break the Plan Mode landing.
- **(d) ask surfacing** — `pending_question` / `NeedsClarification` must be observable from a
  session snapshot the frontend already polls; if a question is only transient in the agent
  loop, surface it through the activity bridge as an explicit event so the card can't be missed.
- **(e) steer timing** — messages land at the next round/retry boundary, not mid-token; the UI
  must say so ("arrives next round") so the owner doesn't expect an instant interrupt.

---

## Reuse vs new

- **Reuse:** activity bridge (`useAgentConsole`, `mini-activity://`), shared renderers
  (`ChatThread`/`WebsearchView`), Polis figure-mapping + district colors + `parentAgentId`,
  `AgentTerminalViewer` (Raw), verdict events. **Comms channels — all already exist:**
  `orchestrator_steer`, `project_cloud_orchestrator_send`, `mini_coder_steer` (Direction A);
  `pending_question` / `ask_user` MCP / `MiniCoderStatus::NeedsClarification` (Direction B).
  **Re-plan console: `PlannerPlanMode` re-used 1:1** (StageWebsearch/StagePlan/StageDesign +
  PlannerChat + PlannerControls + `PlanApprovalCard`) — overlaid on the Work page, NOT rebuilt.
  **Board: `TaskDependencyArrows` + `TaskCard`** re-used, just twinned to the shared selection.
- **New:** `workConsoleModel.ts`, `LivingPlan.tsx`, `FocusStage.tsx`, `CensorStrip.tsx`, the
  split layout, the ProjectWorkspace graft, the `sendToAgent`/`pendingQuestionFor` dispatchers,
  the shared `selectedNode` selection wiring (board ↔ Living Plan), the planner-overlay
  recall/return state, and ONE thin backend command `agent_pty_send_message` (the only new
  transport). **Deleted:** the dock-tab `AgentConsole` + `MiniSteerBar` (merged, no duplicates).
