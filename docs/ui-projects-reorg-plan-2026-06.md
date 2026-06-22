# UI cleanup — remove Dashboard + Projects / project-page reorg (2026-06-18)

> Status: PLAN, owner-approved direction (2026-06-18). UNTRACKED (docs/), do NOT commit.
> GPU-free throughout. Cadence (CLAUDE.md): each phase → implement (veteran-coder) → verify
> on disk → 1 hostile reviewer → fix → next; whole cumulative diff at the very end →
> MAX-RECALL (3 reviewers, different angles + adversarial pass). Owner drives the UX — show
> before/after + confirm layout before the big moves (Fase 1).

## Why
The UI is a cluttered mess. **Dashboard** is a dead cloud-ops page (KPI/Workers/Scaleway +
an inline Oracle panel + provider-health + feed) — redundant with the Providers/Oracle
pages; to be **rebuilt later with a notifications system** (no notifications-doc exists yet;
today "notifications" are implicit via Kanban transitions). The big **Projects** page crams a
whole project's controls into a panel UNDER the overview (status, agent root, agent panel,
saved workflows, task board, notes) — all of it is project-specific and belongs in the
single-project page. Goal: **TWO clean levels.**

## The model (owner-confirmed 2026-06-18)
- **Level 1 — Projects (overview):** pick a project. ONLY the **stage board + calendar**.
  Click a project → open its page.
- **Level 2 — Project page (single project), task-centric (Cline-inspired):** slim header →
  the **task board** (central) → dock tabs (Censor/Activity/Git/Plans/Console/MCP) →
  **NOTES below the tabs** (owner: "sotto, non accanto").

## Ground truth (Explore 2026-06-18, file:line)
- **Routing:** state-based, no React Router. `activeView` in `src/context/AppContext.tsx:420`;
  switch in `src/App.tsx:155-228` (`renderView()`); nav in `src/components/Sidebar.tsx`
  (items from `AppContext` `navigation`).
- **Dashboard:** `src/components/Dashboard.tsx` + dir `src/components/dashboard/` (7 files:
  KpiCard, WorkersTable, ScalewayTable, OraclePanel, RiskFlags, ProviderHealth, ActivityFeed).
  Refs: `App.tsx:4` import, `App.tsx:172` `case "dashboard"`, `App.tsx:227` `default` fallback;
  `AppContext.tsx:82` nav entry; `Sidebar.tsx:26` icon (`LayoutDashboard`, ALSO the unknown-nav
  fallback icon at `Sidebar.tsx:107`).
- **Projects = ONE component** `src/components/views/ProjectsView.tsx`, branch on local state
  `workMode: boolean`:
  - **Board mode (overview):** `ProjectsBoard` (macro STAGE board — columns = project
    lifecycle stages, cards = whole projects) `:1737-1745`; `ProjectCalendar` (milestones
    across all projects) `:1750-1754`; then the **lower detail panel ("pannellone")** for the
    selected project `:1769-2218`:
    - A) `ProjectStatusHeader` (`ProjectStatusHeader.tsx:102`) at `:1771-1790`
    - B) **AGENT ROOT / Set root** — inline JSX `:1795-1826` (`setProjectRoot` →
      `update_project_metadata`)
    - C) `ProjectAgentPanel` (`ProjectAgentPanel.tsx`) at `:1832-1866`
    - D) **Saved workflows** — inline JSX `:1868-1948` (`list_saved_workflows`)
    - E) **BOARD** (per-project TASK kanban) — `CollapsibleSection` `:1951-2143`; columns
      defined `:121-127`; cards `TaskCard.tsx`; data `currentProject.state.tasks` grouped
      `tasksByColumn` `:293-308`. **NOT a duplicate of the top stage board** — different data.
    - F) **NOTES** — `CollapsibleSection` `:2145-2217`; data `currentProject.state.notes`;
      write `append_project_note` (`projects.rs:696`, `lib.rs:444`).
  - **Work mode (single project):** lazy `ProjectWorkspace` (`ProjectWorkspace.tsx:110`) at
    `:1600-1630` when `workMode && currentProject`. Dock tabs: `dockTab` state `:137`,
    default `"censor"` (`projectWorkspaceModel.ts:453`); `DockTab` type `:449`; `DOCK_TABS`
    `:455-465` (Censor/Activity/Git/Plans/Console/MCP); tab strip `:505-544`; bodies `:547-602`.
- **Task move mechanics today:** NO drag-and-drop. A **"Move" MiniMenu** dropdown on each card
  (`TaskCard.tsx:83-96`) → `moveTask` (`ProjectsView.tsx:963-973`) → Tauri `move_project_task`.
  Disabled when the task is agent-controlled (`:2083`). `done` is verifier-gated.
- **Dependencies already exist** (`ProjectTask.depends_on`, `model.rs:172-220`); the DAG runner
  obeys them (`devboule-coder/runner.rs`); shown ONLY as text ("dep: T2, T3") in
  `PlanExecutionView.tsx:69-72` — NO arrows on the Kanban (→ that's Phase 17).

---

## Phase 0 — Remove Dashboard  (isolated, low-risk, do FIRST)
1. Delete `src/components/Dashboard.tsx` and the whole `src/components/dashboard/` dir.
2. `src/App.tsx`: remove the import (`:4`); remove `case "dashboard"` (`:172`); repoint the
   `default` fallback (`:227`) → the Projects view (mirror the `case "projects"` render).
3. `src/context/AppContext.tsx`: remove the `{ id: "dashboard", … }` nav entry (`:82`).
4. `src/components/Sidebar.tsx`: KEEP the `LayoutDashboard` import — it's still the unknown-nav
   fallback icon (`:107`); only the nav entry removal (step 3) drops the button.
5. Verify no other importer of `Dashboard`/`dashboard/*` (OraclePanel etc. are dashboard-only;
   `askOracle`/`getOracleNode`/`getOracleSimilar` on AppContext STAY — used by the Oracle view).
6. **Gate:** `npx tsc --noEmit` + `npx vitest run` green; app boots on Projects.

## Phase 1 — Split overview ↔ project-page + relocate the panel + NOTES below tabs + rename board
**Owner drives the layout — confirm a mockup before the big move.** Target single-project page:
```
┌ ← Board  [Project name] ●Active root:…    git · Pull Commit Push  ← slim top bar (status/root folded in)
├ (push / plan approval cards, when pending)
├──────────────┬─────────────────────────────────────────────────────
│ Agent rail   │ TERMINAL — live xterm of the SELECTED agent          │ ← main coder up top (RAW stream;
│ (who's work. │ (main coder by default; a selected mini shows its own)│    a selected mini = its own)
│  + Launch)   │                                                      │
├──────────────┴─────────────────────────────────────────────────────
│ Tasks          [ Board | Grafo ]                                    │ ← relocated task BOARD (Grafo = Ph.17)
│  To do │ Working │ Review │ Blocked │ Done                          │
├───────────────────────────────────────────────────────────────────────
│ [ Censor │ Git │ Plans │ Console │ MCP │ (Changes) ]                │ ← dock tabs (Activity DROPPED)
│  Console body = STRUCTURED timeline: coder milestones + mini + censor│
├───────────────────────────────────────────────────────────────────────
│ ▸ Notes                                                             │ ← relocated, BELOW the tabs
└ ▸ Saved workflows                                                   │ ← relocated (collapsible / into Plans)
```
Steps:
1. **Rename board labels (frontend display only; backend statuses unchanged):**
   `ProjectsView.tsx:121-127` + the section title/summary — `Board`→**Tasks**;
   `TODO`→**To do**, `WIP`→**Working**, `REVIEW`→**Review**, `BLOCKED`→**Blocked**,
   `DONE`→**Done** (keep "Verifier gated" on Done); summary "… done / … wip / … review" →
   "… done · … in progress · … in review".
2. **Overview slims to stage-board + calendar:** in board mode, keep `ProjectsBoard`
   (`:1737-1745`) + `ProjectCalendar` (`:1750-1754`); REMOVE the lower panel (`:1769-2218`).
3. **Relocate the panel into `ProjectWorkspace`** (extract sub-components rather than copy JSX):
   - `ProjectStatusHeader` + AGENT ROOT (B `:1795-1826`) → slim header at the top.
   - `ProjectAgentPanel` (C `:1832-1866`) → compact, under the header.
   - The task BOARD (E `:1951-2143`) → central content above the dock tabs.
   - Saved workflows (D `:1868-1948`) → collapsible at the bottom (or into the Plans tab).
   - **NOTES (F `:2145-2217`) → BELOW the dock tabs** (after `ProjectWorkspace.tsx:603`). Thread
     `notes`/`noteDraft`/`appendNote`/`isBusy` as props OR extract a `<ProjectNotes>` component.
     Backend unchanged (`append_project_note`, `currentProject.state.notes`).
4. **Entering the page:** selecting a project in the overview opens the single-project page
   (verify/repoint the current `setWorkMode(true)` trigger).
5. **Consolidate the 3 console-ish surfaces (de-clutter — owner: "activity inutile"):** today
   there are THREE "what's the agent doing" surfaces; keep TWO, by ROLE (RAW vs STRUCTURED, NOT
   main-vs-mini):
   - **Terminal** (top-center, `ProjectWorkspace.tsx:460-478`, `AgentTerminalViewer`/xterm) = RAW
     stream of the SELECTED agent's CLI (main coder by default; a selected mini shows its own).
     Keep. (Optionally label the region "Terminal" for clarity.)
   - **Console** (dock tab, `AgentConsole.tsx` via `useAgentConsole`) = STRUCTURED timeline:
     main-coder milestones (`CoderEntry`) + nested mini runs (`MiniRun` rounds + emit-edits diffs)
     + per-round **Censor verdicts** (`Verdict`) — see `agentConsoleModel.ts`. Keep.
   - ❌ **DROP the "Activity" tab** (`DockActivity` `ProjectWorkspace.tsx:611`; body block `:558`;
     `DockTab` type + `DOCK_TABS` in `projectWorkspaceModel.ts:449/455-465`) — poorer third feed,
     redundant with Console. Remove the type member + array entry + body block + the function.
     `DEFAULT_DOCK_TAB` stays `"censor"` (unaffected). Resulting tabs: **Censor · Git · Plans ·
     Console · MCP** (+ a future "Changes" tab, Fase 2/deferred).
6. **Gate per sub-step:** `tsc` + `vitest` green; manual: overview clean, project page shows
   Terminal (top) + board + tabs (no Activity) + notes-below; no data lost (notes, tasks,
   workflows still work).

## Phase 2 — Cline easy-steals (clarity polish, low effort)
1. **Per-task cost badge** on `TaskCard` — reuse the cost-tracking already built (`cost.rs`
   `estimate_task_cost`). Show a small `~$X` hint (estimate via the project's configured coder
   model). DECISION: per-task estimate vs surfacing the ledger total — start with the cheapest
   that's honest.
2. **Task timeline strip** — render the agent's tool-call steps as a horizontal row of colored
   chips (Cline "Task Timeline") instead of the raw log, in the **Console** (`AgentConsole.tsx`
   already has the step data). May split as 2b if it grows.
3. **"Changes" dock tab — external review launcher (owner-confirmed 2026-06-18: OPEN, NOT embed).**
   A new dock tab = a **read-only diff** of the project's current changes (reuse the existing
   `DiffBlock` from `AgentConsole.tsx` / the Git-tab diff — an in-app glance at WHAT changed) + a row
   of **"Open in…" launchers** that hand the rich review + per-line comments to a THIRD-PARTY tool:
   VS Code (`code --goto <file>:<line>`), Cursor, Zed, JetBrains, or **Open PR** (GitHub/GitLab via
   `gh`/browser). Per-line commenting is NOT ours — the external tool does it.
   - **User-selectable providers**, auto-detected (`command_exists`; resolve the binary properly —
     heed the macOS no-bare-`python` PATH lesson: GUI-launched apps don't inherit the shell PATH),
     absent tool → hidden/disabled; no hardcoding (PRODUCT GENERALITY).
   - Add `"changes"` to `DockTab`/`DOCK_TABS` (`projectWorkspaceModel.ts:449/455-465`) + a body block;
     reuse the existing external-process/opener path the app already uses to launch the agent CLIs.
   - ❌ NOT embedding VS Code/Cline (separate Electron apps / extensions — can't iframe).
4. **Gate:** `tsc` + `vitest` green.

## Deferred → master plan **Phase 17** (`master-plan-2026-06-…md`)
Dependency **arrows ON the board** (Cline-style) + **magnetic anchors** (drag A▸B = set
`depends_on`) + live card movement + optional **drag-to-column fires a column-specific agent
prompt**. The ENGINE EXISTS (`depends_on` + DAG runner + role-gated transitions) → Phase 17 is
~mostly frontend. Builds on 11.5-B Piece 1 (done) → 2 → 3. GPU-free, owner-driven.

---
*Plan written 2026-06-18. Next: Fase 0 (remove Dashboard) → reviewer → Fase 1 (with owner mockup confirm).*
