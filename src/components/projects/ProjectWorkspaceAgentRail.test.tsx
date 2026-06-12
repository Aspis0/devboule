import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

import type { AgentSession, AgentSubagent } from "../../types/backend";
import { ProjectWorkspaceAgentRail } from "./ProjectWorkspaceAgentRail";

// Node-env render test (this repo's vitest has no jsdom): renderToStaticMarkup
// runs the component's render path WITHOUT effects, so useNow just returns the
// initial clock and no timer is set. We only need to launch the launcher panel to
// stay closed, so its container deps never load.

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
    lastSeenAt: new Date().toISOString(),
    ...partial,
  };
}

const noop = () => undefined;

function render(sessions: AgentSession[], selectedAgentId: string | null) {
  return renderToStaticMarkup(
    <ProjectWorkspaceAgentRail
      sessions={sessions}
      selectedAgentId={selectedAgentId}
      onSelectAgent={noop}
      projectId="p1"
      projectTitle="Alpha"
      tasks={[]}
      projectActive
      isBusy={false}
      launchMessage={null}
      onLaunch={noop}
      onCopyPrompt={noop}
      launcherOpen={false}
      onToggleLauncher={noop}
    />,
  );
}

describe("ProjectWorkspaceAgentRail mini rows", () => {
  it("renders a mini child as a selectable button with the MINI chip", () => {
    const html = render(
      [
        session({ agentId: "coder-1" }),
        session({ agentId: "mini-1", parentAgentId: "coder-1", client: "ollama" }),
      ],
      null,
    );
    // The parent still renders.
    expect(html).toContain("coder-1");
    // The mini renders with a MINI chip.
    expect(html).toContain("mini-1");
    expect(html).toContain(">Mini<");
    // The mini is a real <button> (selectable affordance), with aria-pressed like
    // the top-level rows. Two agent buttons => two aria-pressed buttons.
    const pressedCount = (html.match(/aria-pressed=/g) ?? []).length;
    expect(pressedCount).toBe(2);
  });

  it("does NOT render label-only subagents as buttons", () => {
    const sub: AgentSubagent = { label: "search", model: "haiku", count: 2 };
    const html = render([session({ agentId: "coder-1", subagents: [sub] })], null);
    // The subagent label appears as an info line...
    expect(html).toContain("search");
    expect(html).toContain("haiku");
    // ...but there is no MINI chip and exactly ONE selectable button (the parent).
    expect(html).not.toContain(">Mini<");
    const pressedCount = (html.match(/aria-pressed=/g) ?? []).length;
    expect(pressedCount).toBe(1);
  });

  it("renders an orphan mini at top level with the hint", () => {
    const html = render(
      [session({ agentId: "mini-1", parentAgentId: "gone" })],
      "mini-1",
    );
    expect(html).toContain("mini-1");
    expect(html).toContain(">Mini<");
    expect(html).toContain("orphaned mini");
  });
});
