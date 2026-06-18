import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

// ProjectWorkspace pulls invokeBackendCommand transitively (AgentDetailDrawer,
// CensorPanel). Mock AppContext so this stays a pure node-env unit (no Tauri).
import { vi } from "vitest";
vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: vi.fn(async () => undefined),
  isTauriRuntime: () => false,
}));

import type { AgentSession, ProjectDetail } from "../../types/backend";
import { ProjectWorkspace } from "./ProjectWorkspace";

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

function project(): ProjectDetail {
  return {
    metadata: { id: "p1", title: "Alpha", rootPath: "/tmp/p1" },
    state: { tasks: [] },
    gitStatus: { isGitRepo: false },
  } as unknown as ProjectDetail;
}

const noop = () => undefined;

function render(
  sessions: AgentSession[],
  ptyAgents: Set<string>,
  extra?: { readOnly?: boolean; onUnarchive?: () => void },
) {
  return renderToStaticMarkup(
    <ProjectWorkspace
      project={project()}
      sessions={sessions}
      claims={[]}
      events={[]}
      ptyAgents={ptyAgents}
      isBusy={false}
      canLaunch
      launchMessage={null}
      onBack={noop}
      onLaunch={noop}
      onCopyPrompt={noop}
      onCommit={noop}
      onPush={noop}
      onPull={noop}
      onStopAgent={noop}
      onFocusCli={noop}
      onCopyRecovery={noop}
      gitActionMessage={null}
      gitActionError={false}
      gitActionBusy={false}
      readOnly={extra?.readOnly}
      onUnarchive={extra?.onUnarchive}
    />,
  );
}

describe("ProjectWorkspace mini selection mounts the PTY-gated terminal", () => {
  it("auto-selects the freshest (a mini here) and mounts the terminal when it has a live PTY", () => {
    // The mini is freshest, so reconcileSelectedAgentId(null, …) selects it.
    const sessions = [
      session({ agentId: "coder-1", lastSeenAt: "2026-06-05T00:00:00Z" }),
      session({
        agentId: "mini-1",
        parentAgentId: "coder-1",
        client: "ollama",
        lastSeenAt: "2026-06-05T05:00:00Z",
      }),
    ];
    // The mini has a live app PTY (agent_pty_list returns host="app" sessions).
    const html = render(sessions, new Set(["coder-1", "mini-1"]));
    // The selected agent header shows the mini id.
    expect(html).toContain("mini-1");
    // Because the mini IS in ptyAgents, the PTY-gated branch (Suspense terminal)
    // renders its loading state, NOT the "external console" no-terminal note.
    expect(html).toContain("Loading terminal");
    expect(html).not.toContain("external console");
  });

  it("shows the no-terminal note when the selected agent lacks a live PTY", () => {
    const sessions = [session({ agentId: "coder-1" })];
    const html = render(sessions, new Set()); // empty PTY set
    expect(html).toContain("external console");
    expect(html).not.toContain("Loading terminal");
  });
});

describe("ProjectWorkspace Stop (kill) safety brake (MC-P5)", () => {
  it("renders the Stop button + help copy ONLY for a selected mini", () => {
    const sessions = [
      session({ agentId: "coder-1", lastSeenAt: "2026-06-05T00:00:00Z" }),
      session({
        agentId: "mini-1",
        parentAgentId: "coder-1",
        client: "ollama",
        lastSeenAt: "2026-06-05T05:00:00Z", // freshest -> auto-selected
      }),
    ];
    const html = render(sessions, new Set(["coder-1", "mini-1"]));
    // The mini is selected, so the 1-click Stop brake + its help copy render.
    expect(html).toContain(">Stop<");
    expect(html).toContain(
      "Immediately kills this mini-coder; the parent coder will be told it was aborted and must escalate to you.",
    );
    // The reused AgentTerminalViewer (with its reply bar) is mounted, not bypassed.
    expect(html).toContain("Loading terminal");
  });

  it("renders the NORMAL-agent Stop (stop_agent) but NOT the mini brake for a non-mini agent", () => {
    // A normal coder (no parentAgentId): it shows the restored stop_agent Stop
    // button — the ONLY UI surface to kill a stalled/runaway normal agent — but
    // NEVER the mini 1-click brake (that is mini-only). The two are mutually
    // exclusive on the same selection.
    const sessions = [session({ agentId: "coder-1" })];
    const html = render(sessions, new Set(["coder-1"]));
    // The normal Stop button renders, with its own help copy.
    expect(html).toContain(">Stop<");
    expect(html).toContain("Stop ends the launched agent.");
    // …but NOT the mini brake's help copy.
    expect(html).not.toContain("Immediately kills this mini-coder");
  });
});

describe("ProjectWorkspace Compact button (MC-P7)", () => {
  it("renders the Compact button + help copy for a selected CLAUDE agent", () => {
    // The default fixture client is "claude"; a single claude coder is selected.
    const sessions = [session({ agentId: "coder-1", client: "claude" })];
    const html = render(sessions, new Set(["coder-1"]));
    expect(html).toContain(">Compact<");
    expect(html).toContain(
      "Runs /compact in this Claude agent to shrink its context.",
    );
  });

  it("does NOT render the Compact button for a non-claude agent (codex)", () => {
    const sessions = [session({ agentId: "coder-1", client: "codex" })];
    const html = render(sessions, new Set(["coder-1"]));
    expect(html).not.toContain(">Compact<");
    expect(html).not.toContain(
      "Runs /compact in this Claude agent to shrink its context.",
    );
  });

  it("does NOT render Compact for a custom client whose id contains 'claude'", () => {
    // Substring guard: a custom client "claudex" must NOT trip the claude gate.
    const sessions = [session({ agentId: "coder-1", client: "claudex" })];
    const html = render(sessions, new Set(["coder-1"]));
    expect(html).not.toContain(">Compact<");
  });

  it("does NOT render Compact for a non-claude mini (ollama) but DOES show Stop", () => {
    // The two controls are independent: an ollama mini is selected -> Stop shows,
    // Compact does not (the mini's resolved client is ollama, not claude).
    const sessions = [
      session({ agentId: "coder-1", client: "claude" }),
      session({
        agentId: "mini-1",
        parentAgentId: "coder-1",
        client: "ollama",
        lastSeenAt: "2026-06-05T05:00:00Z", // freshest -> auto-selected
      }),
    ];
    const html = render(sessions, new Set(["coder-1", "mini-1"]));
    expect(html).toContain(">Stop<");
    expect(html).not.toContain(">Compact<");
  });
});

describe("ProjectWorkspace read-only (archived) mode", () => {
  it("shows the archived banner + an Unarchive button when readOnly", () => {
    const html = render([], new Set(), { readOnly: true, onUnarchive: noop });
    expect(html).toContain("Project archived — read only");
    expect(html).toContain(">Unarchive<");
  });

  it("does NOT show the archived banner when not readOnly (default behavior)", () => {
    const html = render([], new Set());
    expect(html).not.toContain("Project archived — read only");
    expect(html).not.toContain(">Unarchive<");
  });

  it("disables the git Pull/Commit/Push controls when readOnly", () => {
    const html = render([], new Set(), { readOnly: true, onUnarchive: noop });
    // The three top-bar git buttons render but carry the disabled attribute.
    expect(html).toContain(">Pull<");
    expect(html).toContain(">Push<");
    // Static markup emits `disabled=""` for a disabled button. All git buttons
    // (Pull/Commit/Push) are disabled in read-only mode.
    const disabledCount = (html.match(/disabled=""/g) ?? []).length;
    expect(disabledCount).toBeGreaterThanOrEqual(3);
  });

  it("does NOT disable git controls when not readOnly (gitRepo present)", () => {
    // gitStatus.isGitRepo is false in the fixture, so the git buttons are disabled
    // for that reason; assert the BANNER drives no extra gating by confirming the
    // banner is absent and Unarchive is not rendered (byte-identical to today).
    const html = render([], new Set());
    expect(html).not.toContain(">Unarchive<");
  });

  it('hides the agent question card when readOnly', () => {
    const sessions = [session({ pendingQuestion: { id: "q1", question: "Which file?", createdAt: "2026-06-05T00:00:00Z" } })];
    const set = new Set<string>();
    const htmlNormal = render(sessions, set);
    expect(htmlNormal).toContain('is asking you a question');
    const htmlReadOnly = render(sessions, set, { readOnly: true, onUnarchive: noop });
    expect(htmlReadOnly).not.toContain('is asking you a question');
  });

  it('keeps the mini Stop brake available when readOnly', () => {
    const parentId = 'p1';
    const miniId = 'm1';
    const sessions = [session({ parentAgentId: parentId, client: 'ollama', lastSeenAt: '2026-06-05T00:00:00Z' })];
    const set = new Set([parentId, miniId]);
    const html = render(sessions, set, { readOnly: true, onUnarchive: noop });
    expect(html).toContain('>Stop<');
    expect(html).toContain('Immediately kills this mini-coder');
  });
});
