import type {
  ScalewayResourceAction,
  ScalewayResourceSummary,
} from "../types/backend";

export interface ScalewayActionChoice {
  action: ScalewayResourceAction;
  label: string;
  tone: "primary" | "neutral" | "danger" | "critical";
}

export function scalewayActionChoices(
  resource: ScalewayResourceSummary,
): ScalewayActionChoice[] {
  if (resource.resourceType === "Serverless") {
    return resource.availableActions.includes("deploy")
      ? [{ action: "deploy", label: "Deploy", tone: "primary" }]
      : [];
  }

  if (resource.resourceType !== "GPU" && resource.resourceType !== "CPU VM") {
    return [];
  }

  if (resource.state === "stopped") {
    return filterByAvailableActions(resource, [
      { action: "start", label: "Start", tone: "primary" },
      { action: "delete", label: "Delete", tone: "critical" },
    ]);
  }

  if (resource.state === "running") {
    return filterByAvailableActions(resource, [
      { action: "stop", label: "Stop", tone: "danger" },
      { action: "reboot", label: "Reboot", tone: "neutral" },
      { action: "delete", label: "Delete", tone: "critical" },
    ]);
  }

  return [];
}

function filterByAvailableActions(
  resource: ScalewayResourceSummary,
  choices: ScalewayActionChoice[],
) {
  if (resource.availableActions.length === 0) {
    return [];
  }

  return choices.filter((choice) =>
    resource.availableActions.includes(toScalewayApiAction(choice.action)),
  );
}

function toScalewayApiAction(action: ScalewayResourceAction) {
  if (action === "start") return "poweron";
  if (action === "stop") return "poweroff";
  if (action === "delete") return "terminate";
  return action;
}

export function scalewayActionImpact(
  resource: ScalewayResourceSummary,
  action: ScalewayResourceAction,
) {
  if (action === "delete") {
    return `Delete ${resource.name}. This maps to Scaleway terminate and can permanently remove the VM and attached local/scratch data.`;
  }
  if (action === "stop") {
    return `Stop ${resource.name}. Work running on this VM will be interrupted.`;
  }
  if (action === "reboot") {
    return `Reboot ${resource.name}. Active work on this VM may be interrupted.`;
  }
  if (action === "start") {
    return `Start ${resource.name}. Billing can resume while the VM is running.`;
  }
  return `Deploy ${resource.name}. Serverless code/config will be redeployed.`;
}

export function scalewayActionRequiresNameConfirm(action: ScalewayResourceAction) {
  return action === "delete";
}
