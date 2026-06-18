// @vitest-environment jsdom
//
// Tests for ChangesDockTab — the project dock "Changes" tab. Proves the SECURITY +
// robustness fixes from the hostile review:
//   - the diff/editor commands are invoked by PROJECT ID (server-side root
//     confinement), never a caller-supplied raw `root` path (BLOCKER 1+2);
//   - a slow in-flight diff fetch cannot clobber a newer result after the project
//     changes (WARNING 5 stale-result guard);
//   - a very large diff renders a bounded number of lines + a truncation note
//     (WARNING 7 DOM cap).

import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { ProjectDetail } from "../../types/backend";

// ---------------------------------------------------------------------------
// Mocks (must precede component import)
// ---------------------------------------------------------------------------

// A controllable diff response: tests can hand back a fixed string OR a pending
// promise (resolved manually) to drive the stale-result race deterministically.
let diffResponder: () => Promise<string> = async () => "";
const invokeMock = vi.fn(async (...args: unknown[]) => {
  const cmd = args[0];
  if (cmd === "git_working_diff") return diffResponder();
  if (cmd === "list_external_editors") return ["code"];
  if (cmd === "open_in_editor") return undefined;
  return null;
});

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => invokeMock(...(args as [])),
}));

vi.mock("../../utils/safeOpenExternal", () => ({
  safeOpenExternal: vi.fn(),
}));

import { ChangesDockTab } from "./ChangesDockTab";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

function projectFixture(id: string, rootPath: string | null): ProjectDetail {
  return {
    metadata: { id, title: id, status: "active", updatedAt: "", rootPath },
    gitStatus: { pullRequestUrl: null },
  } as unknown as ProjectDetail;
}

beforeEach(() => {
  invokeMock.mockClear();
  diffResponder = async () => "";
});

describe("ChangesDockTab — security + robustness", () => {
  let container: HTMLDivElement;
  let root: Root;

  async function mount(project: ProjectDetail) {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => {
      root.render(createElement(ChangesDockTab, { project }));
    });
    // Flush the mount-effect microtasks (diff + editor probe).
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
  }

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it("fetches the diff by projectId, never a raw root path (BLOCKER 1+2)", async () => {
    diffResponder = async () => "diff --git a/x b/x\n+new\n";
    await mount(projectFixture("proj-a", "/some/registered/root"));

    const diffCall = invokeMock.mock.calls.find((c) => c[0] === "git_working_diff");
    expect(diffCall).toBeTruthy();
    const payload = diffCall?.[1] as Record<string, unknown>;
    // The server resolves the root from the id; the frontend must NOT leak a path.
    expect(payload.projectId).toBe("proj-a");
    expect(payload).not.toHaveProperty("root");
  });

  it("opens an editor by projectId, never a raw root path (BLOCKER 1+2)", async () => {
    await mount(projectFixture("proj-a", "/some/registered/root"));

    const openBtn = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Open in"),
    );
    expect(openBtn).toBeTruthy();
    await act(async () => {
      openBtn?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
    });

    const openCall = invokeMock.mock.calls.find((c) => c[0] === "open_in_editor");
    expect(openCall).toBeTruthy();
    const payload = openCall?.[1] as Record<string, unknown>;
    expect(payload.projectId).toBe("proj-a");
    expect(payload.editor).toBe("code");
    expect(payload).not.toHaveProperty("root");
  });

  it("drops a stale in-flight diff when the project changes (WARNING 5)", async () => {
    // First mount: a diff fetch that we keep PENDING (never resolves until we say so).
    let resolveSlow: (v: string) => void = () => {};
    const slow = new Promise<string>((res) => {
      resolveSlow = res;
    });
    diffResponder = () => slow;

    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => {
      root.render(createElement(ChangesDockTab, { project: projectFixture("proj-a", "/root-a") }));
    });

    // Re-render with a DIFFERENT project: the slow fetch from proj-a is now stale.
    diffResponder = async () => "FRESH-DIFF-B\n";
    await act(async () => {
      root.render(createElement(ChangesDockTab, { project: projectFixture("proj-b", "/root-b") }));
      await Promise.resolve();
      await Promise.resolve();
    });

    // Now resolve the OLD (proj-a) fetch with a value that must NOT appear.
    await act(async () => {
      resolveSlow("STALE-DIFF-A\n");
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(container.textContent).toContain("FRESH-DIFF-B");
    expect(container.textContent).not.toContain("STALE-DIFF-A");
  });

  it("caps rendered diff lines and shows a truncation note (WARNING 7)", async () => {
    // 5000 lines -> far over the 800-line render cap.
    const huge = Array.from({ length: 5000 }, (_, i) => `+line ${i}`).join("\n");
    diffResponder = async () => huge;
    await mount(projectFixture("proj-a", "/root-a"));

    const lineDivs = container.querySelectorAll("div.whitespace-pre");
    expect(lineDivs.length).toBeLessThanOrEqual(800);
    expect(container.textContent).toContain("View truncated");
  });
});
