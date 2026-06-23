import type { ProjectTask } from "../../../types/backend";
import type { DesignProjectEntry } from "../../../types/design";
import type { ConsoleEntry } from "../../agents/agentConsoleModel";

export type StageView = 'exa' | 'plan' | 'design';

export type PlanCardState = 'done' | 'forming' | 'pending';

export interface PlanCard {
  n: number;
  title: string;
  state: PlanCardState;
}

export interface StagePage {
  url: string;
  title: string;
  summary: string;
}

export interface StageFinding {
  text: string;
  task?: number;
}

export type ChatRole = 'user' | 'assistant';

export interface PlannerMessage {
  role: ChatRole;
  text: string;
}

export interface StatusPill {
  text: string;
}

/** Maps backend tasks to plan cards with derived states and titles. */
export function derivePlanCards(tasks: ProjectTask[]): PlanCard[] {
  return tasks.map((task, index) => {
    const n = index + 1;
    const state: PlanCardState =
      task.status === 'done'
        ? 'done'
        : task.status === 'wip'
          ? 'forming'
          : 'pending';
    const title = task.title.trim() || `Task ${n}`;
    return { n, title, state };
  });
}

/** Extracts the hostname from a URL string, handling missing protocols and invalid inputs. */
export function pageHostname(url: string): string {
  try {
    const parseable = url.startsWith('http') ? url : `https://${url}`;
    return new URL(parseable).hostname;
  } catch {
    return url.trim();
  }
}

/**
 * Selects the most-recently opened design entry matching the given project root path.
 * Matches a design at the root or in a folder UNDER it (exact path or `root + '/'`
 * prefix — never a bare sibling like '/proj2' for root '/proj'). Pure + total.
 */
export function pickProjectDesign(
  entries: DesignProjectEntry[],
  rootPath: string | null,
): DesignProjectEntry | null {
  if (rootPath == null || rootPath === "" || entries.length === 0) {
    return null;
  }

  let best: DesignProjectEntry | null = null;
  const prefix = rootPath + "/";

  for (const entry of entries) {
    if (
      entry.workingFolderPath === rootPath ||
      entry.workingFolderPath.startsWith(prefix)
    ) {
      if (best === null || entry.lastOpenedAt > best.lastOpenedAt) {
        best = entry;
      }
    }
  }

  return best;
}

export interface PlannerWeb {
  pages: StagePage[];
  findings: StageFinding[];
}

/** Extract the LATEST websearch row's real pages + derived findings from a console
 *  timeline (the orchestrator's `useAgentConsole` entries). Findings are the per-page
 *  summaries. Empty when the orchestrator hasn't searched yet. Pure + total. */
export function latestWeb(entries: ConsoleEntry[] | undefined): PlannerWeb {
  if (!entries) return { pages: [], findings: [] };
  for (let i = entries.length - 1; i >= 0; i--) {
    const e = entries[i];
    if (e.type === 'webSearch') {
      const pages: StagePage[] = e.pages.map((p) => ({
        url: p.url,
        title: p.title,
        summary: p.summary,
      }));
      const findings: StageFinding[] = pages
        .filter((p) => p.summary.trim().length > 0)
        .map((p) => ({ text: p.summary }));
      return { pages, findings };
    }
  }
  return { pages: [], findings: [] };
}

/** Returns the human-readable label for a given stage view. */
export function stripLabel(view: StageView): 'searching' | 'planning' | 'designing' {
  switch (view) {
    case 'exa':
      return 'searching';
    case 'plan':
      return 'planning';
    case 'design':
      return 'designing';
  }
}
