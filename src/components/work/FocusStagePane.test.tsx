import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

// FocusStagePane pulls invokeBackendCommand transitively (FocusStage composer). Mock
// AppContext so this stays a pure node-env unit (no Tauri).
vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: vi.fn(async () => undefined),
  isTauriRuntime: () => false,
}));

import type { AgentSession } from "../../types/backend";
import { buildWorkConsoleModel } from "./workConsoleModel";
import { FocusStagePane } from "./FocusStagePane";

function session(partial: Partial<AgentSession>): AgentSession {
  return {
    agentId: "coder-1",
    role: "coder",
    model: null,
    status: "online",
    message: null,
    client: "claude",
    currentProjectId: "p1",
    currentTaskId: null,
    firstSeenAt: "2026-06-05T00:00:00Z",
    lastSeenAt: "2026-06-05T00:00:00Z",
    ...partial,
  };
}

function modelFor(sessions: AgentSession[]) {
  return buildWorkConsoleModel({ sessions, tasks: [], projectId: "p1" });
}

describe("FocusStagePane", () => {
  it("renders the FocusStage (Activity default) for a placed agent", () => {
    const sessions = [session({ agentId: "coder-1" })];
    const html = renderToStaticMarkup(
      <FocusStagePane
        agentId="coder-1"
        model={modelFor(sessions)}
        sessions={sessions}
        ptyAgents={new Set(["coder-1"])}
      />,
    );
    expect(html).toContain('data-view="activity"');
  });

  it("renders the not-placed fallback for an agent absent from the model", () => {
    const sessions = [session({ agentId: "coder-1" })];
    const html = renderToStaticMarkup(
      <FocusStagePane
        agentId="ghost"
        model={modelFor(sessions)}
        sessions={sessions}
        ptyAgents={new Set()}
      />,
    );
    expect(html).not.toContain('data-view="activity"');
    expect(html).toContain("isn&#x27;t placed in the work model");
  });

  it("shows a close affordance only when onClose is provided (the pinned split pane)", () => {
    const sessions = [session({ agentId: "coder-1" })];
    const withClose = renderToStaticMarkup(
      <FocusStagePane
        agentId="coder-1"
        model={modelFor(sessions)}
        sessions={sessions}
        ptyAgents={new Set(["coder-1"])}
        onClose={() => undefined}
      />,
    );
    expect(withClose).toContain('aria-label="Close this split pane"');

    const noClose = renderToStaticMarkup(
      <FocusStagePane
        agentId="coder-1"
        model={modelFor(sessions)}
        sessions={sessions}
        ptyAgents={new Set(["coder-1"])}
      />,
    );
    expect(noClose).not.toContain('aria-label="Close this split pane"');
  });
});
