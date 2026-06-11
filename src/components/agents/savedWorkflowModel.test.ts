import { describe, expect, it } from "vitest";
import {
  WORKFLOW_ARGS_MAX_LENGTH,
  buildWorkflowLaunchInput,
  cleanWorkflowArgs,
  workflowLaunchError,
} from "./savedWorkflowModel";

const workflows = [
  { name: "release", description: "Release flow", scope: "project" },
  { name: "audit_docs", scope: "global" },
];

describe("saved workflow launch model", () => {
  it("builds a Claude app-hosted launch input for a discovered workflow", () => {
    const input = buildWorkflowLaunchInput("proj-1", "release", "--dry-run", workflows, 123);

    expect(input).toMatchObject({
      projectId: "proj-1",
      role: "coder",
      client: "claude",
      agentId: "workflow-release-123",
      taskId: null,
      host: "app",
      workflowRun: { name: "release", args: "--dry-run" },
    });
  });

  it("rejects undiscovered or unsafe workflow names", () => {
    expect(workflowLaunchError("missing", workflows)).toBe(
      "Workflow is not available for this project.",
    );
    expect(workflowLaunchError("release; rm", workflows)).toBe(
      "Workflow name is invalid.",
    );
  });

  it("cleans and caps workflow args as data", () => {
    const raw = `${"x".repeat(WORKFLOW_ARGS_MAX_LENGTH + 10)}\0secret`;
    const cleaned = cleanWorkflowArgs(raw);

    expect(cleaned).toHaveLength(WORKFLOW_ARGS_MAX_LENGTH);
    expect(cleaned).not.toContain("secret");
    expect(cleaned).not.toContain("\0");
  });
});
