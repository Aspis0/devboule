// Persisted UI preference for the Work-mode consolidated tab bar: which tab is
// active. Mirrors the try/catch read/write style used by the other small
// localStorage prefs in this codebase (e.g. calendarOpenPref.ts). The key is
// interpolated per project so different projects remember their own tab.

import type { DockTab } from "./projectWorkspaceModel";

const VALID_TABS = new Set<string>([
  "tasks",
  "censor",
  "git",
  "changes",
  "plans",
  "notes",
  "mcp",
  "project",
]);

function keyFor(projectId: string): string {
  return `devboule.work.activeTab.${projectId}`;
}

export function readActiveTabPref(projectId: string): DockTab {
  try {
    const stored = localStorage.getItem(keyFor(projectId));
    if (stored && VALID_TABS.has(stored)) return stored as DockTab;
    return "tasks";
  } catch {
    // storage unavailable (private mode / disabled) — default to tasks.
    return "tasks";
  }
}

export function writeActiveTabPref(projectId: string, tab: DockTab): void {
  try {
    localStorage.setItem(keyFor(projectId), tab);
  } catch {
    // storage unavailable — non-fatal; the in-memory state still holds.
  }
}
