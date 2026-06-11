import type { ProjectAgentLaunchInput, SavedWorkflow } from "../../types/backend";

export const WORKFLOW_ARGS_MAX_LENGTH = 1000;

export function cleanWorkflowArgs(value: string): string {
  return value.replace(/\0/g, "").slice(0, WORKFLOW_ARGS_MAX_LENGTH).trim();
}

export function workflowLaunchError(
  name: string,
  workflows: SavedWorkflow[],
): string | null {
  const trimmed = name.trim();
  if (!trimmed) return "Choose a saved workflow.";
  if (!/^[A-Za-z0-9_-]{1,64}$/.test(trimmed)) {
    return "Workflow name is invalid.";
  }
  if (!workflows.some((workflow) => workflow.name === trimmed)) {
    return "Workflow is not available for this project.";
  }
  return null;
}

export function buildWorkflowLaunchInput(
  projectId: string,
  name: string,
  args: string,
  workflows: SavedWorkflow[],
  now = Date.now(),
): ProjectAgentLaunchInput {
  const error = workflowLaunchError(name, workflows);
  if (error) throw new Error(error);
  const workflowName = name.trim();
  return {
    projectId,
    role: "coder",
    client: "claude",
    agentId: `workflow-${workflowName}-${now}`,
    taskId: null,
    host: "app",
    workflowRun: {
      name: workflowName,
      args: cleanWorkflowArgs(args) || null,
    },
  };
}
