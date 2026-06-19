import { Bot } from "lucide-react";
import { useMemo } from "react";
import type { ColumnId, MoveTarget } from "./taskBoard";
import { MiniMenu, type MiniMenuItem } from "./MiniMenu";
import type { ProjectTask } from "../../types/backend";
import {
  categoryChipClass,
  categoryLabel,
  isTaskCategory,
} from "./taskCategory";

const priorityDotTone: Record<string, string> = {
  high: "bg-coral",
  medium: "bg-amber",
  low: "bg-sage",
};

function taskTone(task: ProjectTask) {
  if (task.status === "blocked") return "border-coral/20";
  if (task.status === "done") return "border-sage/20";
  return "border-cream-200";
}

// Calm task card FACE for the Board Kanban — same "3 questions" rule as the
// project card: WHAT (title + a single subtle priority dot), WHO (assignee +
// one compact agent chip when the task is agent-controlled), and the due date
// only when present. Linked resources, IDs beyond the title, and other detail
// belong in the BACK, not on the face. The Move button grid is replaced by one
// "Move" MiniMenu and the launch button trio by one "Launch" MiniMenu.
//
// IMPORTANT: this component owns NO gating logic. All gating is computed by the
// parent (ProjectsView) and passed in as booleans / a pre-filtered targets
// list, and every action just invokes the parent's handler with identical
// arguments. The "agent-controlled blocks manual move" rule is preserved by the
// `agentControlled` flag disabling the Move menu exactly as before.
export function TaskCard({
  task,
  agentControlled,
  moveTargets,
  moveDisabled,
  manualMoveTitle,
  showLaunch,
  launchDisabled,
  launchTitle,
  coderDisabled,
  coderTitle,
  verifierDisabled,
  verifierTitle,
  manualDisabled,
  onMove,
  onLaunchCoder,
  onLaunchVerifier,
  onCopyManualPrompt,
}: {
  task: ProjectTask;
  agentControlled: boolean;
  moveTargets: MoveTarget[];
  moveDisabled: boolean;
  manualMoveTitle: string;
  showLaunch: boolean;
  launchDisabled: boolean;
  launchTitle: string;
  coderDisabled: boolean;
  coderTitle: string;
  verifierDisabled: boolean;
  verifierTitle: string;
  manualDisabled: boolean;
  onMove: (status: ColumnId) => void;
  onLaunchCoder: () => void;
  onLaunchVerifier: () => void;
  onCopyManualPrompt: () => void;
}) {
  // Face meta is limited to the "who" (assignee) and the due date when present.
  // Linked resources are detail and intentionally excluded from the face.
  const metaParts: string[] = [];
  if (task.assignee) metaParts.push(task.assignee);
  if (task.due) metaParts.push(task.due);

  // Memoize the menu item arrays (and the onSelect closures they carry) so the
  // 5s/10s poll-driven board re-renders don't hand MiniMenu a fresh `items`
  // array identity every tick. A new array identity each render would churn the
  // open menu's effects (re-measure, re-bind listeners) and can disrupt an
  // open dropdown. Each array recomputes only when its real inputs change.
  const taskId = task.id;
  const moveItems: MiniMenuItem[] = useMemo(
    () =>
      moveTargets.map((target) => ({
        key: target.id,
        label: target.label,
        onSelect: () => onMove(target.id),
        title: `Move to ${target.label}`,
        "aria-label": `Move ${taskId} to ${target.label}`,
        "data-help-title": `This moves the task to ${target.label}.`,
        "data-help-lines":
          "Moving a task rewrites the local project Markdown.|If an agent has an open claim, manual movement is blocked to avoid fighting the agent.|Done is verifier-gated and cannot be reached from this manual move set.|The board refreshes from the file after the mutation.",
      })),
    [moveTargets, onMove, taskId],
  );

  const launchItems: MiniMenuItem[] = useMemo(
    () => [
      {
        key: "coder",
        label: "Code",
        onSelect: onLaunchCoder,
        disabled: coderDisabled,
        title: coderTitle,
        "aria-label": `Launch coder for ${taskId}`,
        "data-help-title": "This launches a Codex coder for the task.",
        "data-help-lines":
          "A coder is allowed to edit code and use scoped provider write tools when configured.|The app opens a terminal at the project root and gives it a task prompt plus MCP config.|It should claim the task through MCP and move it toward Review when done.|Cloudflare role tokens may be injected only for matching coder profiles.",
      },
      {
        key: "verifier",
        label: "Verify",
        onSelect: onLaunchVerifier,
        disabled: verifierDisabled,
        title: verifierTitle,
        "aria-label": `Launch verifier for ${taskId}`,
        "data-help-title": "This launches a Codex verifier for the task.",
        "data-help-lines":
          "A verifier audits work and should not modify provider resources.|It can read Oracle, project state, and provider inventory.|It should mark the task done only when evidence supports closure.|Read-only Cloudflare and Scaleway access is the intended profile.",
      },
      {
        key: "manual",
        label: "Manual",
        onSelect: onCopyManualPrompt,
        disabled: manualDisabled,
        title: launchTitle,
        "aria-label": `Copy manual agent prompt for ${taskId}`,
        "data-help-title": "This copies a manual agent prompt.",
        "data-help-lines":
          "Use this when you want to paste the prompt into an existing terminal instead of launching one.|The prompt includes project id, task id, role, and MCP instructions.|The CLI still needs MCP configured to update project state automatically.|Manual runs are easier to misconfigure than app-launched terminals.",
      },
    ],
    [
      onLaunchCoder,
      onLaunchVerifier,
      onCopyManualPrompt,
      coderDisabled,
      verifierDisabled,
      manualDisabled,
      coderTitle,
      verifierTitle,
      launchTitle,
      taskId,
    ],
  );

  return (
    <article
      data-task-id={task.id}
      className={`rounded-lg border bg-white p-3 shadow-soft-sm ${taskTone(task)}`}
    >
      <div className="flex items-start justify-between gap-2">
        <span className="flex min-w-0 items-start gap-2">
          {task.priority && (
            <span
              className={`mt-1.5 h-2 w-2 shrink-0 rounded-full ${
                priorityDotTone[task.priority] ?? "bg-cream-300"
              }`}
              title={`Priority: ${task.priority}`}
              aria-label={`Priority ${task.priority}`}
            />
          )}
          <p className="min-w-0 break-words text-[12px] font-semibold leading-5 text-cream-800">
            {task.title}
          </p>
        </span>
        <span className="shrink-0 break-all font-mono text-[10px] text-cream-400">
          {task.id}
        </span>
      </div>

      {(agentControlled || metaParts.length > 0 || isTaskCategory(task.category)) && (
        <div className="mt-2 flex flex-wrap items-center gap-x-2 gap-y-1">
          {isTaskCategory(task.category) && (
            <span
              className={`inline-flex items-center rounded-md px-2 py-0.5 text-[10px] font-semibold ${categoryChipClass(task.category)}`}
              title={`Category: ${categoryLabel(task.category)}`}
            >
              {categoryLabel(task.category)}
            </span>
          )}
          {agentControlled && (
            <span
              className="inline-flex items-center gap-1 rounded-md bg-terracotta/10 px-2 py-0.5 text-[10px] font-semibold text-terracotta"
              title="An open agent claim or session controls this task; let MCP update status or wait for expiry."
            >
              <Bot className="h-3 w-3 shrink-0" aria-hidden />
              Agent
            </span>
          )}
          {metaParts.length > 0 && (
            <span className="min-w-0 truncate text-[10px] text-cream-400">
              {metaParts.join(" · ")}
            </span>
          )}
        </div>
      )}

      {(moveTargets.length > 0 || showLaunch) && (
        <div className="mt-3 flex gap-1">
          {moveTargets.length > 0 && (
            <div className="flex-1">
              <MiniMenu
                label="Move"
                items={moveItems}
                disabled={moveDisabled}
                title={moveDisabled ? manualMoveTitle : "Move task"}
                aria-label={`Move task ${task.id}`}
              />
            </div>
          )}
          {showLaunch && (
            <div className="flex-1">
              <MiniMenu
                label="Launch"
                items={launchItems}
                disabled={launchDisabled}
                align="right"
                aria-label={`Launch agent for task ${task.id}`}
              />
            </div>
          )}
        </div>
      )}
    </article>
  );
}
