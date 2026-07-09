# App simplification plan — 2026-07-09

Owner-directed simplification of Devboule OUTSIDE Polis: less chrome, self-explaining
pages, a Help entry point, and a leaner backend. Grounded in four deepseek-v4-flash
recon passes (projects area, sidebar/views, board mode, work mode + backend) — every
file:line below comes from those reports.

**Owner directives (2026-07-09):**
1. Hide the cloud-Providers area from the sidebar (the provider-agnostic refactor
   comes LATER; today it is hardcoded Scaleway/Cloudflare — do not start that here).
2. Simplify the orchestrator/projects page further: the projects board FIRST and big;
   each project self-explanatory next to its title; websearch/plan/design stage panels
   UNDER the chat; the calendar behind a toggle button.
3. Simplify the single-project page: big console first with the available coders,
   below it the sections (activity, tasks, …) — and nothing else ("e basta").
4. Skills page: add a proper title that explains what it is.
5. New "Help" sidebar entry: easy, complete "how to start / how Devboule works".
6. General simplification at my (Fable's) discretion → added: richer project cards,
   work-mode dock consolidation, dead-code purge (frontend + 42 dead Tauri commands),
   `projects.rs` modular split.

**Division of labour:** a **Sonnet orchestrator executes S1→S9** following the runbook,
committing each task, and STOPS after S9 with a handoff report. **Fable runs the final
max-recall** (3 hostile reviewers + adversarial verify) and may declare one optional
discretionary polish round.

**Baseline:** recorded dynamically in S0 (the other Claude is adding tests on the same
branch — fixed numbers would go stale).

---

# COEXISTENCE RULES (read before anything else — two other Claudes share this repo)

1. **Polis is owned by another Claude session** working RIGHT NOW on this same branch
   and working tree. NEVER touch `src/components/polis/**` or `src-tauri/src/polis/**`
   (not even "dead" polis Tauri commands — they are theirs to judge). Their uncommitted
   files may appear in `git status` (AgentLayer/AmbientLayer/PolisRenderer/locomotion/
   navWalkable/props at the time of writing) — leave them alone.
2. **Design is owned by a third Claude**: never touch `src/components/design/**`.
3. **Commits**: never `git add -A` / `git add .`. Commit ONLY named paths, and use the
   pathspec form so concurrently-staged files from the other Claude can't leak in:
   `git commit -m "<msg>" -- <path1> <path2> ...`
4. **pi sessions persist per cwd, and the other Claude resumes theirs with `-c` from
   the repo ROOT.** Never run pi from the repo root: run frontend-task pi dispatches
   from `src/`, Rust-task dispatches from `src-tauri/`. Spec files must give absolute
   paths so the coder never depends on its cwd.
5. Never build the exe / run `npm run tauri build` (owner rule: no concurrent builds).

---

# ORCHESTRATOR RUNBOOK (follow verbatim)

## Roles
- **All coding goes to pi coders.** The orchestrator NEVER writes production code
  inline — not even one-liners. Coder model: `mimo-v2.5` (or `mimo-v2.5-pro` where a
  task says so). Reviewer model: `deepseek-v4-pro`. Both via the `pi` CLI.
- The orchestrator itself only: writes task-spec files, runs pi, reads reports,
  verifies files on disk, runs vitest/cargo, arbitrates review findings, commits.

## pi commands (exact — do not improvise flags)
Prompt ALWAYS via `$(cat file)`, ALWAYS `< /dev/null`, stdout ALWAYS redirected to a
file you then Read (terminal stdout is untrusted). NEVER run pi with
run_in_background. Frontend tasks: run from `<repo>/src`. Rust tasks: from
`<repo>/src-tauri`. Thinking ALWAYS `high`.

```sh
# coder (fresh task) — from src/ or src-tauri/ per the task
pi -ne --provider xiaomi-token-plan-sgp --model "mimo-v2.5" --thinking high \
  -t read,bash,edit,write -p "$(cat SPEC.md)" < /dev/null > OUT.md 2>&1
# coder, harder task
... --model "mimo-v2.5-pro" ...
# reviewer (read-only tools!)
pi -ne --provider deepseek --model "deepseek-v4-pro" --thinking high \
  -t read,bash -p "$(cat REVIEW-SPEC.md)" < /dev/null > REVIEW-OUT.md 2>&1
# fix pass — SAME session as the original author, SAME cwd as the original dispatch
pi -ne -c --provider xiaomi-token-plan-sgp --model "mimo-v2.5" --thinking high \
  -t read,bash,edit,write -p "$(cat FIXES.md)" < /dev/null > OUT2.md 2>&1
```
If the 10-min Bash timeout cuts a run, resume with `-c ... -p "Continue exactly where
you left off"`. Spec files live in the scratchpad dir, never in the repo.

## Safety preamble — PREPEND VERBATIM to every spec (coder AND reviewer)
A deepseek pi task once ran `git checkout -- src/` mid-task and silently wiped ~600
lines of uncommitted work. Hence:

> ABSOLUTE BAN: never run state-mutating git commands (checkout, restore, stash,
> reset, clean, commit, push) and never delete/revert files you did not create in
> this task. The dirty working tree is intentional — other agents have uncommitted
> work in it; NEVER touch `src/components/polis/`, `src-tauri/src/polis/` or
> `src/components/design/`. Read-only git (status, diff, log, show) is allowed.
> Do NOT run `cargo` (cold compile exceeds your timeout) and do NOT run the full
> vitest suite; targeted tests only: `npx vitest run <specific-file>`. All paths in
> this spec are absolute — do not rely on your cwd.

## Known coder pitfalls — paste into every CODER spec
- mimo loses earlier edits when it rewrites the same file region across fix passes:
  after EVERY fix pass, re-verify EVERY previously-completed item of that task on
  disk (grep each), not just the item being fixed.
- Emoji/unicode in source must be `\u{...}` escapes, never literal glyphs.
- Do not edit files via Python scripts — use your editor tools directly.
- Follow existing code idioms and Tailwind/design tokens already in the file you edit
  (cream palette, rounded-2xl cards, text-[12px] labels); introduce no new look.
- UI copy in English. Write tests in the style of the neighbouring `*.test.ts(x)`.

## Per-task cadence (repeat for each of S1..S9)
1. `git status --porcelain` — the ONLY acceptable dirt is the other Claude's polis
   files (and design files). Anything else uncommitted ⇒ investigate before starting.
2. Write the coder spec (safety preamble + pitfalls + the full task section below)
   to the scratchpad. Dispatch the coder from the task's cwd (src/ or src-tauri/).
3. Read the OUT file; then verify GROUND TRUTH on disk: grep each claimed change.
   `git status` must list ONLY expected files + the other Claudes' known dirt.
4. Run the task's targeted tests, then the SILENT-WIPE CHECK: full `npx vitest run`
   count ≥ (your recorded baseline + tests you added so far). The other Claude may
   ADD tests concurrently (count going UP is fine; DOWN is a wipe alarm — diff which
   files lost tests before panicking: their in-progress work can also churn).
   For Rust tasks YOU run `cargo test --lib` in `src-tauri` (never the pi coder).
5. Write the reviewer spec: safety preamble + "be hostile and paranoid, attack:
   perf/re-renders/allocations, null crashes, stale closures, race conditions,
   memory leaks (uncleaned listeners/timers), edge cases (empty/null/undefined),
   broken keyboard/a11y, regressions of removed behavior" + the task's acceptance
   criteria + scope `git diff HEAD -- <task paths>`. Dispatch deepseek-v4-pro.
6. Arbitrate: CONFIRMED BLOCKER/MAJOR ⇒ fix (back to the AUTHOR's session with `-c`);
   CONFIRMED MINOR ⇒ fix if cheap else note in handoff; PLAUSIBLE ⇒ read the code
   yourself and verify before acting; REFUTED/NIT ⇒ ignore. Re-verify after fixes
   (steps 3–4, incl. re-verify-every-earlier-item).
7. Commit: `git commit -m "app: <short description> (S<n>)" -- <named paths>` +
   `Co-Authored-By:` trailer per house rules.
8. Append 3–5 lines to scratchpad `handoff-s1-s9.md`: what shipped, test counts,
   review verdicts, deferred items.

## Hard rules
- ONE stateful tool call at a time; never parallelize a coder with the reviewer of
  the same work.
- A task failing twice on the same blocker ⇒ stop it, log the blocker in the handoff,
  move on.
- STOP after S9's commit. Final message = the handoff summary. Do not start the
  max-recall (Fable's job).

---

## S0 — Baseline (orchestrator only, no coder, no commit)
Run full `npx vitest run` and `cargo test --lib` (in `src-tauri`) yourself. Record
both counts as THE baseline at the top of `handoff-s1-s9.md`, plus the current HEAD
hash and the list of other-Claude dirty files from `git status`.

---

## S1 — Hide the cloud-Providers area from the sidebar
**Coder: mimo-v2.5 · Reviewer: deepseek-v4-pro · cwd: src/ · Rust: no**

Facts: nav base list is `EMPTY_CONFIG.navigation` (`src/context/AppContext.tsx:80-83`
— projects, providers, oracle) + 4 injected entries in `src/components/Sidebar.tsx:70-79`
(polis, design, skills, labs) + a fixed Settings button (`Sidebar.tsx:105-128`).
Role gating is a denylist (`src/utils/roles.ts:12`). The jump-search in
`src/components/Header.tsx:59-76` (`JUMP_TARGETS`) has entries "Providers",
"Cloudflare", "Scaleway / Compute", "Budget" targeting `providers#...`. Risk-flag
deep-links (`Header.tsx:69-80`) and `requestView` (`AppContext.tsx:2320`) can still
open the view. Cloud tokens live in Settings → Security → `SecretsView` and are NOT
affected. The App.tsx switch cases (`App.tsx:177-186`) stay.

Changes:
1. Remove `{ id: "providers", ... }` from `EMPTY_CONFIG.navigation`
   (`AppContext.tsx:80-83`). IMPORTANT: first check whether `config.navigation` can
   also arrive from persisted/back-end config (search where `config` is loaded/merged
   in AppContext). If a stored config could still contain `providers`, add a
   defensive filter in Sidebar's list build (`Sidebar.tsx:70-79`):
   `HIDDEN_NAV_IDS = new Set(["providers"])` applied before role filtering — with a
   one-line comment "cloud providers hidden until the provider-agnostic refactor".
2. Remove the four provider jump-search entries from `JUMP_TARGETS`
   (`Header.tsx:59-76`) — "Providers", "Cloudflare", "Scaleway / Compute", "Budget".
   LEAVE the risk-flag deep-link mapping (`viewForRisk`) untouched: notifications
   about provider risks must still land somewhere.
3. Do NOT delete ProvidersView/CloudflareView/ComputeView/BudgetView or their App.tsx
   cases — the views stay reachable by deep link and come back after the agnostic
   refactor.
4. `HelpModeOverlay.tsx` `pageUseLines` (lines 17-34): keep provider entries (the
   views still exist).

Tests: a Sidebar render test (follow existing Sidebar/App test style; if none exists,
create `Sidebar.test.tsx` with the store/context mocked): default config ⇒ nav does
NOT contain "Providers" but DOES contain Projects/Oracle/Polis/Design/Skills/Labs;
a config whose navigation still includes providers ⇒ still filtered out. A Header
test (or pure-data test on JUMP_TARGETS if it's exported): no jump target points at
`providers`.

Verification: targeted vitest on the new/changed test files, then full run.
Acceptance: fresh app shows no Providers in the sidebar; `requestView("providers")`
(e.g. a risk-flag click) still renders the view; Settings → Security token management
unchanged.

---

## S2 — Board mode reorder: board first, calendar behind a button
**Coder: mimo-v2.5 · Reviewer: deepseek-v4-pro · cwd: src/ · Rust: no**

Facts (all `src/components/views/ProjectsView.tsx`, Board branch at 3819): current
order = error banners (3820) → create bar (3832) → clone dialog (3896) →
PlannerPlanMode (3945, unconditionally mounted) → Board/Archived toggle (4202) →
archived list (4252) OR ProjectsBoard (4306) + ProjectCalendar (4325) → skeleton/empty
(4334). ProjectsBoard is a memoized 6-column grid `min-w-[1180px]`
(`ProjectsBoard.tsx:65+`). ProjectCalendar is purely presentational, no fixed height,
nothing depends on it being mounted; add/remove milestone via `add_project_milestone`
/ `remove_project_milestone`; milestone click = `selectProjectOnly`. PlannerPlanMode
has a gsap entrance animation on its own root ref (`PlannerPlanMode.tsx:168,224-247`)
— it must NOT be conditionally remounted or re-keyed. No vitest asserts board-mode
layout order (safe reorder).

Changes:
1. Reorder the Board branch to: error banners → create bar (+ clone dialog, stays
   with the bar) → **[Board/Archived toggle + Calendar toggle button in one row]** →
   **archived list OR ProjectsBoard** → **ProjectCalendar (only when open)** →
   **PlannerPlanMode** → skeleton/empty. PlannerPlanMode keeps the same JSX (same
   instance, no new conditional wrapper, no key) — only its position moves.
2. Calendar toggle: new state `calendarOpen` initialised from localStorage key
   `"devboule.projects.calendarOpen"` (same try/catch read/write style as
   `devboule.projects.selectedId`, `ProjectsView.tsx:325-330`), default `false`.
   Button in the toggle row: calendar icon + label `Calendar` + a count chip with the
   total milestone count across `activeProjects` (compute in a `useMemo` over
   `projects`); `aria-expanded`; when open, ProjectCalendar renders below the board
   with an unchanged props contract (`projects`, `onSelectProject`, `onChanged` —
   currently at 4325-4332).
3. Make the board the visual protagonist: ProjectsBoard section gets more presence —
   keep the 6 columns and `overflow-x-auto`, raise card min height via S3 (do not
   pre-empt S3 here), and ensure the board is the first content block under the
   create bar.
4. Empty-board affordance: when `activeProjects` is empty the board area must still
   explain itself — reuse/move the existing empty state ("Create a project to
   start.", 3670-3684) INTO the board position so the top of the page never looks
   dead.

Tests: extract nothing; add `ProjectsView.boardOrder.test.tsx` ONLY if a cheap
static-markup render is feasible with the existing mock patterns (see
`ProjectWorkspaceMiniSelection.test.tsx` which uses `renderToStaticMarkup`); at
minimum: a pure test for the calendar-open persistence helper (export it), and a
test that the milestone count memo sums milestones across projects. Calendar tests
(`ProjectCalendar.test.tsx`) must stay green unchanged.

Verification: targeted vitest + full run.
Acceptance: board (or archived list) is the first thing under the create bar;
calendar hidden by default, opens via the button, preference survives restart;
planner appears below the board with chat working exactly as before (send a message
path unchanged — no remount flicker of the gsap entrance on toggle interactions).

---

## S3 — Self-explanatory project cards
**Coder: mimo-v2.5 · Reviewer: deepseek-v4-pro · cwd: src/ · Rust: no**

Facts: `ProjectCard` (`src/components/projects/ProjectCard.tsx`, memoized at 118)
currently shows status dot + title, agent line, git chip, censor chip, done/total.
AVAILABLE on `ProjectSummary` but unshown: `updatedAt`, `rootPath`, full
`taskCounts` (todo/wip/review/blocked/done), `milestones`. No ProjectCard test file
exists. Card click = `enterWorkMode`.

Changes (keep the single `<button>` root, memoization, and aria attributes):
1. Line 1 (unchanged): status dot + title.
2. New line 2 — identity: folder basename from `rootPath` (mono, 10-11px, truncated,
   title attr = full path; hidden when rootPath is null) + relative `updatedAt`
   ("2h ago" — add/reuse a small pure helper `relativeTime(iso)`; check
   `src/utils/projectFormat.ts` first and extend it there if it exists).
3. Line 3 — work state: keep agent line + git/censor chips, and expand done/total
   into compact per-state counts, rendering ONLY non-zero states: e.g.
   `2 wip · 1 review · 1 blocked · 5 done` (blocked in the existing warn color).
   Zero tasks ⇒ show `no tasks yet` muted.
4. Line 4 (conditional) — next milestone: the soonest milestone with date ≥ today:
   `◇ <title> · <short date>`; overdue (date < today) renders in the warn color with
   `overdue`. Omit the line when no milestones. Use a `\u{25C7}` escape, not a glyph.
5. Density: the card grows vertically — verify the 6-column board still reads well;
   cap lines with truncation, no wrapping beyond one line each.
6. All new derivations are pure functions exported from a small
   `src/components/projects/projectCardModel.ts` (relativeTime if not shared,
   folderBasename, taskCountsLine, nextMilestone) so they are unit-testable without
   rendering.

Tests: new `projectCardModel.test.ts` covering: relativeTime buckets (now/minutes/
hours/days), folderBasename (posix + windows separators + null), taskCountsLine
(all-zero, mixed, blocked-only), nextMilestone (none / future picks soonest /
overdue flag). Plus one `ProjectCard.test.tsx` static render: full-featured project
shows all four lines; minimal project (no root, no tasks, no milestones) shows
title + "no tasks yet" and nothing else crashes.

Verification: targeted vitest + full run.
Acceptance: a stranger reading a card learns: name, status, which folder, how fresh,
who's working, git/censor state, task breakdown, next deadline — without clicking.

---

## S4 — Planner simplification: chat first, stages under the chat
**Coder: mimo-v2.5 · Reviewer: deepseek-v4-pro · cwd: src/ · Rust: no**

Facts (`src/components/projects/planner/PlannerPlanMode.tsx`): internal order today
= goal echo (231) → orchestrator selector row (269: Local | Claude | Codex | OpenAI)
→ stage container (303: fixed 316px, tabs Websearch|Plan|Design + auto-rotation
every 3800ms via `useStageRotation`, DoubtPanel inside plan view) → PlannerChat
(307: minHeight 340, maxHeight clamp(460px, 62vh, 1200px)) → PlannerControls (310).
The `banner` renders inside PlannerChat. gsap entrance on the root ref (168).

Changes:
1. Reorder to: goal echo → **PlannerChat** → **stage container** → PlannerControls.
   Chat becomes the protagonist: bump its minHeight to ~420px, keep the clamp max.
2. Orchestrator selector row: compact it into the PlannerChat header area (it already
   has a header) as a small segmented control or select — one row less of standalone
   chrome. Keep the live pulse affordance on the active orchestrator.
3. Stage container becomes collapsible: a slim header row (the three tabs + Auto
   toggle + a chevron). Default: collapsed when `!live && !artifactActive` and no
   stage has content; auto-expands when the orchestrator goes live or an artifact
   arrives (plan cards, findings, pages, design). Collapsed state shows the tab
   labels with content-count badges (e.g. `Plan (3)`, `Websearch (7)`) so nothing is
   invisible. Manual expand/collapse always wins over auto (a `userToggled` ref).
   Keep the 316px height when expanded; keep `useStageRotation` behavior when live.
4. DoubtPanel logic unchanged (renders inside the plan view when questions exist —
   when questions arrive while collapsed, auto-expand: unanswered doubts must never
   hide).
5. No prop contract changes toward ProjectsView (props listed in the recon stay
   identical), no remount of PlannerChat (component identity stable — the textarea
   draft must survive the reorder).

Tests: PlannerPlanMode has NO test file today — create
`PlannerPlanMode.test.tsx` (static markup + mocked children if needed, following
`PlannerChat.test.tsx` conventions): order chat-before-stages in the rendered
output; collapsed-by-default when idle/empty; expanded when `live`; badge counts
rendered; questions present ⇒ stage area expanded. Keep `PlannerChat.test.tsx`
green untouched.

Verification: targeted vitest + full run.
Acceptance: the planner reads as "a chat with the orchestrator" first; stage panels
are a secondary drawer under it that opens itself exactly when there is something
to show; no dead 316px box when idle.

---

## S5 — Work mode: console first, ONE tab bar below (mimo-v2.5-pro)
**Coder: mimo-v2.5-pro · Reviewer: deepseek-v4-pro · cwd: src/ · Rust: no**

Facts (`src/components/projects/ProjectWorkspace.tsx`, 1430 lines): current vertical
stack = archived banner → top bar (1239-1340: Back, title, Change plan, +Launch,
git badge, Pull/Commit/Push) → commit input → git msg → `{detailSlot}` (1390) →
PushApprovalCard → ConsentBridgePoller → PlanApprovalCard (1404) → consent modals →
SkillsToolsModal → SpawnPanel (1496) → LivingPlan+FocusStagePane grid (1518+, split
view + AgentDetailDrawer) → CensorStrip (1587) → `{taskBoardSlot}` (1593) → bottom
dock 5 tabs Censor/Git/Plans/MCP/Changes (1595-1670) → `{notesSlot}` (1672).
The three slots are opaque ReactNodes built by ProjectsView (taskBoard 3021+,
notes 2840, detail 2862) — do NOT move their handler logic, only where they render.
Guard tests: `workspaceNoSecondPoller.test.ts` (no setInterval/fetch in the file),
`ProjectWorkspaceMiniSelection.test.tsx` (static markup),
`projectWorkspaceModel.test.ts` (pure logic). SpawnPanel = role Coder/Verifier +
clients codex/claude/openai/orchestrator + custom clients + model chips.

Changes:
1. Keep at top, unchanged: archived banner, top bar, commit input, git message,
   PushApprovalCard, ConsentBridgePoller, consent modals, SkillsToolsModal,
   SpawnPanel toggle. These are gates/alerts and must stay above the fold.
2. **Console block becomes the protagonist**, immediately after the top bar:
   LivingPlan + FocusStagePane grid gets `min-h-[60vh]` (measure against current
   sizing — the intent: the console dominates the first screen). Split view and
   AgentDetailDrawer behavior unchanged.
3. Launch surface: SpawnPanel currently renders ABOVE the console when open — anchor
   it visually to the console (render it directly above the LivingPlan+FocusStage
   grid, where the user is looking when choosing a coder). When it is CLOSED, the
   top bar [+ Launch] button gains a subtitle line or title attr listing the
   available clients (from the same data SpawnPanel uses — expose the resolved
   client label list via the existing props/model, e.g. "codex · claude · openai ·
   Local (Devboule)") so "which coders can I use" is visible at a glance.
4. **ONE consolidated tab bar** replaces today's stack of CensorStrip + taskBoard +
   dock + notes + detail. Tabs, in order:
   `Tasks` (default) | `Censor` | `Git` | `Changes` | `Plans` | `Notes` | `MCP` | `Project`.
   - Tasks tab = the existing `{taskBoardSlot}` node (unwrapped from its
     CollapsibleSection if trivially possible — the tab IS the disclosure now; if
     the CollapsibleSection is baked into ProjectsView's slot JSX, change it there,
     it's ~5 lines).
   - Censor tab = CensorStrip (moved inside, as the summary header) + the existing
     CensorPanel dock content.
   - Git tab = DockGit inline content (unchanged). Changes tab = ChangesDockTab.
   - Plans tab = PlansDockTab, with PlanApprovalCard MERGED in: pending approval
     requests render at the top of this tab instead of as a standalone card above
     the console. The tab label gets an attention badge (count of pending plan
     requests) — pending approvals must be impossible to miss.
   - Notes tab = `{notesSlot}`. Project tab = `{detailSlot}` (status header, root
     editor, saved workflows).
   - Badges: Tasks (wip+review count), Censor (open findings count — same data
     CensorStrip uses), Plans (pending requests). Small count chips in the tab
     label, existing chip idiom.
5. Active tab persists per project in localStorage
   (`devboule.work.activeTab.<projectId>`, default "Tasks"); unknown stored value ⇒
   default.
6. Keep ALL existing functionality reachable — this task moves chrome, it deletes
   nothing. `readOnly` (archived) gating carries over per-tab exactly as today.
7. Respect `workspaceNoSecondPoller.test.ts`: no new setInterval/fetch/loadAgentState
   in ProjectWorkspace.

Tests: update `ProjectWorkspaceMiniSelection.test.tsx` for the new structure (it
asserts current layout pieces); add assertions: default tab renders taskBoardSlot;
switching tab strings renders the right slot/content (static markup with tab state
if feasible, else split the tab-content chooser into a pure function in
`projectWorkspaceModel.ts` and unit-test it: tabId → which section key, badge
counts from sessions/findings/planRequests, persistence key round-trip). Existing
model tests stay green; `workflowMode.removed.test.ts` exports must keep working
(S8 handles its fate — do not break it here).

Verification: targeted vitest (`ProjectWorkspaceMiniSelection`,
`projectWorkspaceModel`, `workspaceNoSecondPoller`) + full run.
Acceptance: opening a project shows top bar + big console; everything else lives in
one tab row below; pending plan approvals and censor findings surface as badges;
nothing that existed is unreachable; archived projects still read-only everywhere.

---

## S6 — Skills page title + explainer
**Coder: mimo-v2.5 · Reviewer: deepseek-v4-pro · cwd: src/ · Rust: no**

Facts: `src/components/views/SkillsView.tsx` has NO h1 — only a banner (31-34) and
Library/Tools tabs. Header titles come from `viewTitles` in
`src/components/Header.tsx:28-37` (check whether "skills" already has an entry).

Changes:
1. Add a proper page header at the top of SkillsView: `<h1>` "Skills" (match the
   heading style used by other views — check OracleView/SettingsView for the idiom)
   + one short subtitle paragraph in plain English explaining the page in one
   breath, e.g.: "Reusable instructions and tools for your agents. Skills are
   manuals agents read before working; Tools are MCP machines they can call. This
   library is global — every project's agents can use it; per-project skills live
   in the project's Work console." (Adapt wording to what the code actually does —
   the existing banner text is the source of truth; fold the banner INTO this
   subtitle and remove the old banner div to avoid saying everything twice.)
2. Ensure `viewTitles` has a "skills" entry ("Skills").

Tests: SkillsView render test (create/extend following existing view-test patterns):
h1 present with "Skills", subtitle mentions both Library and Tools, old duplicate
banner gone, both tab buttons still render.
Verification: targeted vitest + full run.
Acceptance: a user landing on Skills understands in 5 seconds what it is and where
per-project skills live.

---

## S7 — New "Help" view in the sidebar
**Coder: mimo-v2.5 · Reviewer: deepseek-v4-pro · cwd: src/ · Rust: no**

Facts: NO getting-started content exists anywhere (recon-confirmed). Only the
Alt-key `HelpModeOverlay` and the collaborator `OnboardingWizard`. Adding a view =
7 touch points: (1) nav entry in `EMPTY_CONFIG.navigation` (`AppContext.tsx:80-83`),
(2) icon import + `iconMap` in `Sidebar.tsx:12-28`, (3) `case "help"` in the
App.tsx switch (175-227), (4) roles: nothing (denylist — allowed by default),
(5) lazy import + ErrorBoundary + Suspense (copy the SkillsView pattern,
App.tsx:46-53 + 214-222), (6) the component `src/components/views/HelpView.tsx`,
(7) `viewTitles` in `Header.tsx:28-37`.

Changes:
1. Create `HelpView.tsx` (lazy-loaded, pattern above; icon: `LifeBuoy` from
   lucide-react added to iconMap; nav entry `{ id: "help", label: "Help", icon:
   "LifeBuoy" }` placed LAST in EMPTY_CONFIG.navigation so it sits near the bottom).
2. Content = a data-driven array `HELP_SECTIONS: { id, title, body: string[] |
   JSX }` rendered as anchored cards with a slim sticky in-page nav (match the
   cream/rounded-2xl card idiom). Sections, in this order, ALL in plain English:
   - **What is Devboule** — one paragraph: a local coordinator that turns a folder
     of code into a managed project: an orchestrator plans, coders (CLI agents)
     implement, the Censor reviews, you approve.
   - **Quick start (5 steps)** — 1. Projects → type a title (or pick a folder /
     clone from GitHub) → Create. 2. Tell the orchestrator your goal in the chat
     and let it plan (websearch/plan/design panels appear under the chat). 3.
     Approve the plan → tasks land on the project board. 4. Open the project and
     Launch a coder (choose codex / claude / openai / Local). 5. Watch the console,
     answer its questions, let the Censor review, then Commit/Push from the top bar.
   - **The projects board** — what the six columns mean (Planned → Launching →
     Active → Review → Blocked → Verified), what the card chips mean (git ↑↓∆,
     censor ⚠, task counts, next milestone), the calendar button.
   - **Inside a project (Work console)** — the console (Activity vs Raw), the tab
     bar (Tasks/Censor/Git/Changes/Plans/Notes/MCP/Project), stopping an agent,
     plan and push approvals.
   - **Agents & coders** — orchestrator vs coder vs verifier vs mini-coder; where
     models are configured (Settings → Providers & Models); external CLIs must be
     installed and are auto-detected.
   - **Censor** — the local review gate: deterministic linters + AI tiers; the
     per-project trust gate (inert until trusted).
   - **Oracle** — codebase Q&A over the indexed repo; where to configure it.
   - **Skills & Tools** — one paragraph + "see the Skills page".
   - **Keys & providers** — tokens live in Settings → Security; AI models in
     Settings → Providers & Models. (No mention of the hidden cloud pages.)
   - **Tips** — hold Alt anywhere for contextual help; the header search jumps to
     any page; the bell shows agents that need you.
   NOTE for the coder: verify every claim above against the code before writing it
   (e.g. actual column labels in `projectStage.ts:24-37`, actual tab names from S5,
   actual Settings tab names in `SettingsView.tsx:39-47`). Where the plan's wording
   and the code disagree, THE CODE WINS.
3. Cross-links: buttons/links inside sections that call `requestView("projects")`,
   `requestView("settings")`, `requestView("skills")`, `requestView("oracle")` via
   the existing context API.
4. Add `help: "Help"` to `viewTitles`.

Tests: `HelpView.test.tsx`: renders every HELP_SECTIONS title; quick-start section
lists 5 steps; a `requestView` mock fires on a cross-link click. Sidebar test from
S1 extended: nav now ends with Help. HELP_SECTIONS exported for the test.
Verification: targeted vitest + full run.
Acceptance: a new user can go from zero to "coder launched on my repo" using only
the Help page; every claim in it matches the shipped UI (post S1-S6 states).

---

## S8 — Dead-code purge (frontend + Tauri commands)
**Coder: mimo-v2.5 · Reviewer: deepseek-v4-pro · cwd: src-tauri/ · Rust: YES (orchestrator runs cargo)**

Facts (recon, to be RE-VERIFIED per item before deleting): ~320 commands in
`generate_handler![]` (`src-tauri/src/lib.rs:90-410`), ~42 with zero frontend call
sites. Dead frontend components: `src/components/agents/FleetSummary.tsx`,
`src/components/agents/AgentRow.tsx`, `src/components/work/WorkConsole.tsx` (+ its
test). Meta-test files: `workflowMode.removed.test.ts` (asserts transition exports
still exist), `agentsPageDissolved.test.ts`.

Rules of engagement — THE FILTER MATTERS MORE THAN THE LIST:
1. For EVERY deletion candidate (command or component), the coder greps the WHOLE
   repo for the exact name — `src/`, `src-tauri/src/`, `scripts/`, `oracle/`(py),
   docs excluded — including STRING literals (commands are invoked by name via
   `invokeBackendCommand("...")`, MCP dispatch tables, and prompts that tell agents
   which MCP tool to call). Any hit outside the definition/registration itself ⇒
   NOT dead ⇒ skip it and log why.
2. **Skip ALL polis-related commands** (`trigger_file_disaster`,
   `resolve_file_disaster`, `set_agent_location`, `update_agent_status`,
   `append_city_note`, `spawn_scaleway_resource`, `stop_scaleway_resource`,
   `refresh_scaleway_status` and anything in `src-tauri/src/polis/`) and all
   design-related ones (`design_*`) — other Claudes' domains.
3. High-suspicion candidates the recon flagged that MUST survive rule-1 scrutiny
   before deletion (they smell load-bearing): `spawn_agent_session` (pi_sidecar
   uses a same-named internal fn), `agent_pty_kill`, `classify_prompt_command`,
   the `skills_*` family (MCP/skills runtime may call them), the cost family
   (`estimate_task_cost`, `record_cost`, `get_cost_summary` — mini-coder cost
   window shipped once). When in doubt ⇒ keep and log.
4. Deleting a command = remove from `generate_handler![]` + delete the `#[tauri::
   command]` fn + its now-unused private helpers/types + its unit tests. If the fn
   body is shared with a live path, delete only the command wrapper.
5. Frontend: delete `FleetSummary.tsx`, `AgentRow.tsx`, `WorkConsole.tsx` and their
   test files after confirming zero production importers. For
   `workflowMode.removed.test.ts`: if the transition exports it guards are unused
   by production code, delete BOTH the exports and the test; otherwise keep both
   and log. `agentsPageDissolved.test.ts`: keep if it guards a live invariant
   (i.e. it would fail if someone re-added the page); delete only if it references
   files that no longer exist.
6. Output discipline: the coder's report must contain the final DELETED list and
   the SKIPPED-with-reason list. The handoff file gets both.

Tests: full `cargo test --lib` (run by the ORCHESTRATOR, from `src-tauri/`) — count
may go DOWN only by exactly the tests belonging to deleted code (coder must list
them); full vitest same rule.
Verification: cargo build succeeds (no unused-import warnings introduced — run
`cargo clippy --lib` if cheap), full vitest, wipe check vs S0 baseline adjusted for
deliberate removals.
Acceptance: handler list and frontend shrink with zero behavior change; every
deletion is justified by a logged whole-repo grep; polis/design untouched.

---

## S9 — Split `projects.rs` (17,062 lines) into modules
**Coder: mimo-v2.5-pro · Reviewer: deepseek-v4-pro · cwd: src-tauri/ · Rust: YES (orchestrator runs cargo)**

Facts: `src-tauri/src/backend/projects.rs` regions (recon): config lock (38-84),
CRUD (86-270), tasks (273-770), notes/milestones (763-870), config readers
(869-1410), agent launch (1411-2300), file mutation (2306-2540), settings/props
(2544-3015), file parsing (3045-3605), validation/normalization (3606-4146),
prompt building (4147-5481), agent terminal spawning (4476-5920), orchestrator
planner (5921-6205), cloud duplex launch (6206-6983), git ops (6984-9260), root
validators + tests (8982-end).

Pre-decided split — three sequential pi passes, ORCHESTRATOR runs
`cargo test --lib` between each (the coder never runs cargo):
- Pass 1: git operations (≈6984-9260) → new `src-tauri/src/backend/project_git.rs`.
- Pass 2: prompt building (≈4147-5481) → `backend/agent_prompt.rs`; agent terminal
  spawning (≈4476-5920) → `backend/agent_spawn.rs`.
- Pass 3: file parsing/writing (≈3045-3605) → `backend/project_file.rs`; cloud
  duplex launch glue (≈6206-6983) → merge INTO the existing
  `backend/cloud_duplex.rs` if its imports allow, else `backend/cloud_duplex_launch.rs`.

Rules (paste into the spec):
1. MOVE code verbatim — zero logic edits, zero renames, zero visibility narrowing.
   Functions/types that projects.rs still needs become `pub(crate)`; projects.rs
   re-exports what external callers reference (`pub use`) so `lib.rs`'s
   `generate_handler![]` and other modules need MINIMAL or no changes.
2. `#[cfg(test)]` tests move WITH their code; total `cargo test --lib` count must
   equal the pre-pass count exactly (record it before each pass).
3. Shared file-lock helpers (`project_write_lock`, `config_write_lock`) stay in
   projects.rs; moved modules import them.
4. After each pass the orchestrator runs `cargo test --lib` and
   `git diff --stat` sanity (moved lines ≈ added lines; a large negative delta =
   lost code ⇒ treat as wipe).
5. If a pass fails twice (borrow/visibility hell), revert THAT pass's plan (the
   orchestrator restores from git — pi must not), log it, continue with the next.

Tests: no new tests required — the invariant IS the unchanged test count + green.
Verification: `cargo test --lib` green with count == S8-end count, full vitest
untouched, `cargo clippy --lib` no new warnings if cheap.
Acceptance: projects.rs drops to roughly a third of its size; every module compiles
in isolation conceptually (single-purpose); behavior byte-identical.

---

# HANDOFF BOUNDARY — Sonnet STOPS here

After S9: run full `npx vitest run` + `cargo test --lib`, write final counts +
per-task summary + all deferred MINORs + S8's deleted/skipped lists into
`handoff-s1-s9.md`, end with that summary as the final message. Do NOT start the
max-recall.

---

## MAX-RECALL (FABLE) — whole cumulative diff S1..S9
3 hostile reviewers on different angles (deepseek-v4-pro line-by-line, mimo-v2.5-pro
cross-file/interaction + removed-behavior, Sonnet UX/altitude + a11y/keyboard) +
adversarial verify of every CONFIRMED finding, fixes via the author sessions, full
suites, then a fresh live-e2e checklist for the owner: sidebar (no Providers, Help
present), board-first layout + calendar toggle, card richness, planner chat-first +
stage drawer, work-mode single tab bar + badges, skills title, help content accuracy,
and a regression sweep of the S8 deletions (launch every agent client, censor run,
git commit/push, skills library, oracle ask).

## OPTIONAL discretionary polish round (FABLE)
Owner-authorized: if after max-recall the result is still unsatisfying (layout
rhythm, spacing, discoverability), Fable may define and run ONE more polish task
round (same cadence, mimo coder + deepseek review) before handing back for live e2e.

---

## Non-goals / explicitly rejected
- **Provider-agnostic cloud refactor** — owner-deferred; S1 only hides the entry.
- Anything in `src/components/polis/**`, `src-tauri/src/polis/**`,
  `src/components/design/**` — other Claudes' domains.
- Removing the standalone admin/infra views (Oracle, Settings, Devices, Secrets,
  Workspace, Labs) — recon found no true overlap with Work mode.
- Rewriting the slots pattern (ProjectsView → ProjectWorkspace ReactNodes) — S5
  moves where slots RENDER, not who owns the state (~200 lines of handler churn
  for zero user value today; revisit after the monolith split below).
- Splitting `ProjectsView.tsx` (3960 lines) itself — deliberately deferred: S2-S5
  churn its layout; splitting it mid-plan would double every diff. Queue it as the
  natural NEXT plan once this one is live-verified.
