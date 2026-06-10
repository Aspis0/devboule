// Tiny deep-link convention so a single click can target a top-level view AND
// an inner sub-tab. After the sidebar was compressed to 6 entries, several pages
// (Secrets, Cloudflare, Compute, Budget, Devices, Workspace, Agents) live as
// tabs inside Providers / Projects / Settings. Risk-flag clicks and jump-search
// need to land on the right tab, so we encode the target as "view#tab".
//
// Pure module (no React, no DOM) — unit-tested in deepLink.test.ts.

export interface ViewTarget {
  view: string;
  /** The inner sub-tab to open, or null for the view's default tab. */
  tab: string | null;
}

/**
 * Parse a "view" or "view#tab" string. Extra "#" segments are ignored (only the
 * first tab is kept). An empty tab after "#" is treated as no tab. The view is
 * returned verbatim (possibly "") so the caller decides any fallback.
 */
export function parseViewTarget(target: string): ViewTarget {
  const trimmed = (target ?? "").trim();
  const hashIndex = trimmed.indexOf("#");
  if (hashIndex === -1) {
    return { view: trimmed, tab: null };
  }
  const view = trimmed.slice(0, hashIndex);
  const rest = trimmed.slice(hashIndex + 1);
  const nextHash = rest.indexOf("#");
  const tabRaw = (nextHash === -1 ? rest : rest.slice(0, nextHash)).trim();
  return { view, tab: tabRaw === "" ? null : tabRaw };
}

/** Format a view (+ optional tab) back into a "view" or "view#tab" string. */
export function formatViewTarget(view: string, tab?: string | null): string {
  const cleanTab = (tab ?? "").trim();
  return cleanTab === "" ? view : `${view}#${cleanTab}`;
}

/**
 * Result of consuming a `work:<projectId>` pending tab token: which project to
 * select and whether to enter Work mode. Returns null for any tab that is not a
 * (non-empty) work token, so a caller can ignore unrelated tabs.
 *
 * The Agents page was dissolved (Phase G): the Header attention bell now deep-links
 * the needs-you agent's project straight into Work mode via `projects#work:<id>`.
 * `parseViewTarget` already splits that into `{view:"projects", tab:"work:<id>"}`;
 * this maps the tab token to ProjectsView's selection state. Pure (no React) so the
 * mapping is unit-tested without rendering the view.
 */
export interface WorkTabSelection {
  selectedId: string;
  workMode: true;
}

const WORK_TAB_PREFIX = "work:";

export function parseWorkTab(
  tab: string | null | undefined,
): WorkTabSelection | null {
  if (!tab) return null;
  const trimmed = tab.trim();
  if (!trimmed.startsWith(WORK_TAB_PREFIX)) return null;
  const projectId = trimmed.slice(WORK_TAB_PREFIX.length).trim();
  if (projectId === "") return null;
  return { selectedId: projectId, workMode: true };
}

/**
 * Work-mode coherence decision (Phase G BLOCKER): should Work mode exit now?
 *
 * Work mode must stay on while the selected project's detail is still LOADING
 * (e.g. right after a bell deep-link's `enterWorkMode(id)` from another view,
 * when `currentProject` is briefly null because the `get_project` fetch is in
 * flight). Exiting then would bounce the user straight back to the Board before
 * the project ever appears — killing every deep-link.
 *
 * So: exit ONLY when Work mode is on AND there is genuinely no resolved project
 * AND no load is in flight for the currently-selected id. A truly missing /
 * archived id resolves with `currentProject` still null and `loadingProjectId`
 * cleared (back to null), so this returns true once the load settles empty. A
 * project that loads AFTER entry keeps `loadingProjectId === selectedId` until
 * the detail lands, so we hold Work mode through the load. Pure (no React) so the
 * decision is unit-tested without rendering the view.
 */
export function shouldExitWorkMode(args: {
  workMode: boolean;
  hasCurrentProject: boolean;
  selectedId: string | null;
  loadingProjectId: string | null;
  /** Synchronous bridge id set by enterWorkMode for the FIRST render after a
   *  deep-link, before the detail-load effect sets loadingProjectId. Treated as
   *  an in-flight load for the selected id. Null when no entry is pending. */
  pendingWorkEntryId?: string | null;
}): boolean {
  const {
    workMode,
    hasCurrentProject,
    selectedId,
    loadingProjectId,
    pendingWorkEntryId = null,
  } = args;
  if (!workMode || hasCurrentProject) return false;
  if (selectedId === null) return true;
  // A load is in flight for the selected project (state loadingProjectId, or the
  // synchronous enterWorkMode bridge covering the first render): hold Work mode
  // until it resolves — either the detail lands → hasCurrentProject true, or it
  // settles empty → both clear and this re-evaluates to true.
  if (loadingProjectId === selectedId) return false;
  if (pendingWorkEntryId === selectedId) return false;
  return true;
}

/**
 * Decide whether the synchronous `pendingWorkEntryId` bridge (set by
 * enterWorkMode for the FIRST render after a deep-link) can be cleared now.
 *
 * Once cleared, `loadingProjectId` / `currentProject` become the sole source of
 * truth for the work-mode-coherence guard, so the genuine missing/archived
 * fallback works. We may clear when ANY of:
 *   - the BRIDGE TARGET has resolved (`currentProjectId === bridge id`), or
 *   - the real loading state has caught up (`loadingProjectId === bridge id`), or
 *   - the selection has genuinely MOVED off the bridge target.
 *
 * BLOCKER (frontend audit), two coupled traps:
 *
 *   1. The "selection moved" check MUST compare against the synchronously-updated
 *      selection (`selectedIdRef.current`), NOT the stale `selectedId` state. On a
 *      bell deep-link from project A to project B, enterWorkMode(B) sets the ref
 *      to B in the same tick but the `selectedId` state is still A during that
 *      render. Comparing the bridge (B) to the stale state (A) reads "moved" and
 *      clears the bridge immediately.
 *
 *   2. The "resolved" check MUST be that the bridge TARGET resolved, not merely
 *      "some project is resolved". During the A→B render, the previously-selected
 *      project A is still the resolved `currentProject`; a bare `hasCurrentProject`
 *      would clear the bridge while A is showing, even though B has not started
 *      loading yet. We therefore compare the resolved id to the bridge id.
 *
 * Either trap clears the bridge a tick too early — then shouldExitWorkMode sees no
 * in-flight load for B (currentProject null, loadingProjectId not yet B) and
 * bounces the deep-link straight back to the Board. Holding the bridge until B's
 * load actually begins (loadingProjectId===B) or B resolves keeps it on screen.
 *
 * Pure (no React) so the clear decision is unit-tested without rendering.
 */
export function shouldClearWorkEntryBridge(args: {
  /** The pending bridge id, or null when nothing is pending. */
  pendingWorkEntryId: string | null;
  /** The id of the currently-RESOLVED project detail (currentProject), or null. */
  currentProjectId: string | null;
  loadingProjectId: string | null;
  /** The SYNCHRONOUS current selection (selectedIdRef.current), not the stale
   *  `selectedId` state value. */
  currentSelectedId: string | null;
}): boolean {
  const {
    pendingWorkEntryId,
    currentProjectId,
    loadingProjectId,
    currentSelectedId,
  } = args;
  if (pendingWorkEntryId === null) return false;
  return (
    currentProjectId === pendingWorkEntryId ||
    loadingProjectId === pendingWorkEntryId ||
    pendingWorkEntryId !== currentSelectedId
  );
}

/**
 * Map a legacy/deleted view id to its current canonical equivalent.
 *
 * The standalone "oracle" view was RESTORED, so it is no longer remapped — it
 * passes through verbatim like any real view. This guard is kept (composed at
 * the top of requestView) so a future removed view id can be added here without
 * per-call awareness; today it has no entries and every view passes through.
 *
 * Note: "settings" with tab "oracle" is a separate concern handled downstream by
 * mapLegacySettingsTab (the Oracle LLM settings sub-tab → "providers"); this
 * function does NOT touch it.
 */
export function mapLegacyViewTarget(
  view: string,
  tab?: string | null,
): { view: string; tab: string | null } {
  const cleanTab = tab !== undefined && tab !== null ? tab : null;
  return { view, tab: cleanTab };
}

/**
 * Build the deep-link the Header attention bell uses to land on a needs-you
 * agent's project. When the agent has a resolvable project id, target Work mode
 * for that project (`projects` + `work:<id>` tab). When it does NOT (project id
 * missing/empty — e.g. a project-less session), fall back to the Projects Board
 * (no tab) so the click never produces a dead `work:` token or crashes. Pure so
 * the fallback is unit-tested.
 */
export function attentionBellTarget(
  projectId: string | null | undefined,
): { view: string; tab: string | null } {
  const trimmed = (projectId ?? "").trim();
  if (trimmed === "") return { view: "projects", tab: null };
  return { view: "projects", tab: `${WORK_TAB_PREFIX}${trimmed}` };
}
