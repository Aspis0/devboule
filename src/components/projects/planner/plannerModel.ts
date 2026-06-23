import type { ProjectTask } from "../../../types/backend";

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
