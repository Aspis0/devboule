// @vitest-environment jsdom
//
// useHandoff unit tests (Phase D): the packaging sequence ORDER, the non-blocking
// design.md warning + preview-capture skip, the EXACT single-dispatch payload (the
// parity proof against SpawnPanel's launch shape), the permanent double-dispatch
// guard, rootPath-prefix project preselection, the default client from the backend
// kind, and the "Open terminal" deep-link. Driven through the repo's raw-DOM
// renderHook harness (react-dom/client + act, no testing-library).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, createElement, useEffect } from "react";
import { createRoot, type Root } from "react-dom/client";
import {
  useHandoff,
  defaultClientFromBackend,
  preselectProjectId,
  type UseHandoff,
  type UseHandoffArgs,
} from "./useHandoff";
import type { ProjectSummary } from "../../../types/backend";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

interface HookHandle {
  readonly current: UseHandoff;
  unmount: () => void;
}

function renderUseHandoff(args: UseHandoffArgs): HookHandle {
  const ref: { value: UseHandoff } = { value: null as unknown as UseHandoff };
  function Probe() {
    const api = useHandoff(args);
    ref.value = api;
    useEffect(() => {
      ref.value = api;
    });
    return null;
  }
  const container = document.createElement("div");
  let root!: Root;
  act(() => {
    root = createRoot(container);
    root.render(createElement(Probe));
  });
  return {
    get current() {
      return ref.value;
    },
    unmount: () => act(() => root.unmount()),
  };
}

// Flush queued microtasks + effects between the async packaging/dispatch awaits.
async function flush() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

function project(id: string, rootPath: string | null, title = id): ProjectSummary {
  return {
    id,
    title,
    status: "active",
    updatedAt: "2026-06-10T00:00:00Z",
    rootPath,
    revision: "r",
    path: `projects/${id}.md`,
    taskCounts: {
      backlog: 0,
      todo: 0,
      inProgress: 0,
      review: 0,
      blocked: 0,
      done: 0,
      needsUser: 0,
    } as unknown as ProjectSummary["taskCounts"],
    gitStatus: {} as unknown as ProjectSummary["gitStatus"],
  };
}

function makeArgs(over: Partial<UseHandoffArgs> = {}): {
  args: UseHandoffArgs;
  invoke: ReturnType<typeof vi.fn>;
  runConsolidate: ReturnType<typeof vi.fn>;
  runExport: ReturnType<typeof vi.fn>;
  onOpenTerminal: ReturnType<typeof vi.fn>;
} {
  const invoke = vi.fn(async (cmd: string) => {
    if (cmd === "design_read_design_md") return "# Contract\nUse the olive palette.";
    if (cmd === "design_preview_capture") return { path: "preview.png", bytes: 10 };
    if (cmd === "launch_project_agent_terminal")
      return { agentId: "coder-xyz", role: "coder", client: "claude", rootPath: "C:/repo" };
    return {};
  });
  const runConsolidate = vi.fn(async () => true);
  const runExport = vi.fn(async () => true);
  const onOpenTerminal = vi.fn();
  const args: UseHandoffArgs = {
    workingFolderPath: "C:/repo/.devboule-design/landing",
    projects: [project("repo", "C:/repo")],
    backendKind: "claude",
    runConsolidate,
    runExport,
    invoke: invoke as unknown as UseHandoffArgs["invoke"],
    onOpenTerminal,
    ...over,
  };
  return { args, invoke, runConsolidate, runExport, onOpenTerminal };
}

describe("preselectProjectId", () => {
  it("matches the project whose rootPath is a prefix of the working folder", () => {
    const projects = [project("a", "C:/other"), project("repo", "C:/repo")];
    expect(
      preselectProjectId("C:/repo/.devboule-design/landing", projects),
    ).toBe("repo");
  });

  it("normalizes slashes + case + trailing slash and prefers the longest match", () => {
    const projects = [
      project("outer", "C:/work"),
      project("inner", "C:/work/app"),
    ];
    expect(preselectProjectId("c:\\work\\app\\.devboule-design\\x", projects)).toBe(
      "inner",
    );
  });

  it("returns null when no rootPath is a prefix", () => {
    expect(
      preselectProjectId("C:/elsewhere/x", [project("a", "C:/repo")]),
    ).toBeNull();
  });
});

describe("defaultClientFromBackend", () => {
  it("uses codex when the design backend is codex; claude otherwise", () => {
    expect(defaultClientFromBackend("codex")).toBe("codex");
    expect(defaultClientFromBackend("claude")).toBe("claude");
    expect(defaultClientFromBackend("ollama")).toBe("claude");
    expect(defaultClientFromBackend(null)).toBe("claude");
  });
});

describe("useHandoff packaging", () => {
  beforeEach(() => vi.clearAllMocks());

  it("runs save -> export(absolute) -> export(flow) -> contract -> capture, then reaches dispatch", async () => {
    const calls: string[] = [];
    const runConsolidate = vi.fn(async () => {
      calls.push("save");
      return true;
    });
    const runExport = vi.fn(async (mode: string) => {
      calls.push(`export:${mode}`);
      return true;
    });
    const invoke = vi.fn(async (cmd: string) => {
      calls.push(cmd);
      if (cmd === "design_read_design_md") return "# Contract";
      if (cmd === "design_preview_capture") return { path: "preview.png" };
      return {};
    });
    const { args } = makeArgs({
      runConsolidate,
      runExport: runExport as unknown as UseHandoffArgs["runExport"],
      invoke: invoke as unknown as UseHandoffArgs["invoke"],
    });
    const h = renderUseHandoff(args);
    act(() => h.current.openHandoff());
    await flush();

    expect(calls).toEqual([
      "save",
      "export:absolute",
      "export:flow",
      "design_read_design_md",
      "design_preview_capture",
    ]);
    expect(h.current.phase).toBe("dispatch");
    const contract = h.current.steps.find((s) => s.id === "contract");
    const capture = h.current.steps.find((s) => s.id === "capture");
    expect(contract?.status).toBe("done");
    expect(capture?.status).toBe("done");
    h.unmount();
  });

  it("warns (non-blocking) when design.md is missing and still reaches dispatch", async () => {
    const invoke = vi.fn(async (cmd: string) => {
      if (cmd === "design_read_design_md") return ""; // missing/empty contract
      if (cmd === "design_preview_capture") return { path: "preview.png" };
      return {};
    });
    const { args } = makeArgs({
      invoke: invoke as unknown as UseHandoffArgs["invoke"],
    });
    const h = renderUseHandoff(args);
    act(() => h.current.openHandoff());
    await flush();

    const contract = h.current.steps.find((s) => s.id === "contract");
    expect(contract?.status).toBe("warn");
    expect(contract?.detail).toContain("infer style");
    expect(h.current.phase).toBe("dispatch");
    expect(h.current.errorStage).toBeNull();
    h.unmount();
  });

  it("V4: tolerates a null design.md (Option::None) as a warning, not a crash", async () => {
    // design_read_design_md returns Rust Option<String> → null when there is no
    // design.md. The old `invoke<string>` typing + `.trim()` crashed on null, failing
    // the whole packaging. It must instead show the non-blocking warning row and continue.
    const invoke = vi.fn(async (cmd: string) => {
      if (cmd === "design_read_design_md") return null; // Option::None over IPC
      if (cmd === "design_preview_capture") return { path: "preview.png" };
      return {};
    });
    const { args } = makeArgs({
      invoke: invoke as unknown as UseHandoffArgs["invoke"],
    });
    const h = renderUseHandoff(args);
    act(() => h.current.openHandoff());
    await flush();

    const contract = h.current.steps.find((s) => s.id === "contract");
    expect(contract?.status).toBe("warn");
    expect(contract?.detail).toContain("infer style");
    // No error stage: packaging continued past the contract step to dispatch.
    expect(h.current.phase).toBe("dispatch");
    expect(h.current.errorStage).toBeNull();
    h.unmount();
  });

  it("skips (non-blocking) when the preview capture fails and still reaches dispatch", async () => {
    const invoke = vi.fn(async (cmd: string) => {
      if (cmd === "design_read_design_md") return "# Contract";
      if (cmd === "design_preview_capture") throw new Error("preview window is not open");
      return {};
    });
    const { args } = makeArgs({
      invoke: invoke as unknown as UseHandoffArgs["invoke"],
    });
    const h = renderUseHandoff(args);
    act(() => h.current.openHandoff());
    await flush();

    const capture = h.current.steps.find((s) => s.id === "capture");
    expect(capture?.status).toBe("skipped");
    expect(h.current.phase).toBe("dispatch");
    h.unmount();
  });

  it("hard-errors and stays in packaging when an export write fails", async () => {
    const runExport = vi.fn(async (mode: string) => mode !== "flow"); // flow fails
    const { args } = makeArgs({
      runExport: runExport as unknown as UseHandoffArgs["runExport"],
    });
    const h = renderUseHandoff(args);
    act(() => h.current.openHandoff());
    await flush();

    expect(h.current.errorStage).toBe("packaging");
    expect(h.current.errorMessage).toContain("Export failed");
    expect(h.current.phase).toBe("packaging");
    h.unmount();
  });

  it("hard-errors and stays in packaging when the save fails (no dispatch of a stale bundle)", async () => {
    const runConsolidate = vi.fn(async () => false); // save write failed
    const { args, invoke } = makeArgs({ runConsolidate });
    const h = renderUseHandoff(args);
    act(() => h.current.openHandoff());
    await flush();

    expect(h.current.errorStage).toBe("packaging");
    expect(h.current.errorMessage).toContain("Save failed");
    expect(h.current.phase).toBe("packaging");
    const save = h.current.steps.find((s) => s.id === "save");
    expect(save?.status).toBe("error");
    // No export ran (the sequence hard-stopped at save), so no bundle can dispatch.
    expect(h.current.canDispatch).toBe(false);
    expect(
      invoke.mock.calls.filter((c) => c[0] === "launch_project_agent_terminal"),
    ).toHaveLength(0);
    h.unmount();
  });

  it("a stale packaging run after close -> reopen cannot duplicate the save or patch state", async () => {
    // First run's save hangs until we release it, modeling an in-flight packaging.
    let releaseFirst!: () => void;
    const firstSave = new Promise<boolean>((res) => {
      releaseFirst = () => res(true);
    });
    let saveCalls = 0;
    const runConsolidate = vi.fn(() => {
      saveCalls += 1;
      // First call hangs; subsequent (reopen) calls resolve immediately.
      return saveCalls === 1 ? firstSave : Promise.resolve(true);
    });
    const { args, invoke } = makeArgs({
      runConsolidate: runConsolidate as unknown as UseHandoffArgs["runConsolidate"],
    });
    const h = renderUseHandoff(args);

    // Open: first packaging run starts and blocks awaiting the hung save.
    act(() => h.current.openHandoff());
    await flush();
    expect(saveCalls).toBe(1);
    expect(h.current.phase).toBe("packaging");

    // Close mid-packaging, then reopen — the reopen starts a FRESH run (epoch bumped).
    act(() => h.current.close());
    act(() => h.current.openHandoff());
    await flush();
    expect(saveCalls).toBe(2); // exactly one fresh save, the reopen's

    // Now release the stale first run. Its late patches/phase write must be dropped:
    // it no longer owns the epoch, so it cannot push the machine to dispatch or beyond.
    await act(async () => {
      releaseFirst();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    // The fresh run is the only one that drove the machine to dispatch.
    expect(h.current.phase).toBe("dispatch");
    expect(h.current.errorStage).toBeNull();
    // Exactly two saves total (one stale hung + one fresh); never a third.
    expect(saveCalls).toBe(2);
    // The fresh run packaged + can dispatch; only ITS launch path is reachable.
    expect(
      invoke.mock.calls.filter((c) => c[0] === "launch_project_agent_terminal"),
    ).toHaveLength(0);
    h.unmount();
  });

  it("openHandoff is a no-op while already open (no concurrent packaging run)", async () => {
    let saveCalls = 0;
    const runConsolidate = vi.fn(async () => {
      saveCalls += 1;
      return true;
    });
    const { args } = makeArgs({ runConsolidate });
    const h = renderUseHandoff(args);
    act(() => h.current.openHandoff());
    await flush();
    expect(saveCalls).toBe(1);
    // A second openHandoff while open must not start another packaging sequence.
    act(() => h.current.openHandoff());
    await flush();
    expect(saveCalls).toBe(1);
    h.unmount();
  });
});

describe("useHandoff dispatch", () => {
  beforeEach(() => vi.clearAllMocks());

  it("preselects the project by rootPath prefix once packaging completes", async () => {
    const { args } = makeArgs({
      projects: [project("other", "C:/x"), project("repo", "C:/repo")],
    });
    const h = renderUseHandoff(args);
    act(() => h.current.openHandoff());
    await flush();
    expect(h.current.selectedProjectId).toBe("repo");
    h.unmount();
  });

  it("fires ONE launch with the exact payload shape (parity proof) and reaches done", async () => {
    const { args, invoke } = makeArgs();
    const h = renderUseHandoff(args);
    act(() => h.current.openHandoff());
    await flush();
    expect(h.current.canDispatch).toBe(true);

    act(() => h.current.dispatch());
    await flush();

    const launchCalls = invoke.mock.calls.filter(
      (c) => c[0] === "launch_project_agent_terminal",
    );
    expect(launchCalls).toHaveLength(1);
    const payload = launchCalls[0][1].input;
    expect(payload).toEqual({
      projectId: "repo",
      role: "coder",
      client: "claude",
      host: "app",
      agentId: payload.agentId,
      designHandoff: { workingFolderPath: "C:/repo/.devboule-design/landing" },
    });
    expect(payload.agentId).toMatch(/^coder-\d+-[0-9a-z]{1,5}$/);
    expect(h.current.phase).toBe("done");
    expect(h.current.agentId).toBe("coder-xyz");
    h.unmount();
  });

  it("uses the codex client when the design backend kind is codex", async () => {
    const { args, invoke } = makeArgs({ backendKind: "codex" });
    const h = renderUseHandoff(args);
    act(() => h.current.openHandoff());
    await flush();
    act(() => h.current.dispatch());
    await flush();
    const payload = invoke.mock.calls.find(
      (c) => c[0] === "launch_project_agent_terminal",
    )![1].input;
    expect(payload.client).toBe("codex");
    h.unmount();
  });

  it("guards against double dispatch permanently (second click is a no-op)", async () => {
    const { args, invoke } = makeArgs();
    const h = renderUseHandoff(args);
    act(() => h.current.openHandoff());
    await flush();
    act(() => {
      h.current.dispatch();
      h.current.dispatch();
      h.current.dispatch();
    });
    await flush();
    // Even a post-done re-fire does nothing.
    act(() => h.current.dispatch());
    await flush();
    const launchCalls = invoke.mock.calls.filter(
      (c) => c[0] === "launch_project_agent_terminal",
    );
    expect(launchCalls).toHaveLength(1);
    expect(h.current.canDispatch).toBe(false);
    h.unmount();
  });

  it("does not dispatch without a selected project", async () => {
    const { args, invoke } = makeArgs({
      // No project rootPath matches the folder => no preselection.
      projects: [project("other", "C:/elsewhere")],
    });
    const h = renderUseHandoff(args);
    act(() => h.current.openHandoff());
    await flush();
    expect(h.current.selectedProjectId).toBeNull();
    expect(h.current.canDispatch).toBe(false);
    act(() => h.current.dispatch());
    await flush();
    expect(
      invoke.mock.calls.filter((c) => c[0] === "launch_project_agent_terminal"),
    ).toHaveLength(0);
    expect(h.current.errorStage).toBe("dispatch");
    h.unmount();
  });

  it("un-latches on a failed dispatch so Retry can re-fire", async () => {
    let attempts = 0;
    const invoke = vi.fn(async (cmd: string) => {
      if (cmd === "design_read_design_md") return "# Contract";
      if (cmd === "design_preview_capture") return { path: "preview.png" };
      if (cmd === "launch_project_agent_terminal") {
        attempts += 1;
        if (attempts === 1) throw new Error("claude not found in PATH");
        return { agentId: "coder-2", role: "coder", client: "claude", rootPath: "C:/repo" };
      }
      return {};
    });
    const { args } = makeArgs({
      invoke: invoke as unknown as UseHandoffArgs["invoke"],
    });
    const h = renderUseHandoff(args);
    act(() => h.current.openHandoff());
    await flush();
    act(() => h.current.dispatch());
    await flush();
    expect(h.current.errorStage).toBe("dispatch");
    expect(h.current.canDispatch).toBe(true); // re-armed

    act(() => h.current.dispatch());
    await flush();
    expect(h.current.phase).toBe("done");
    expect(attempts).toBe(2);
    h.unmount();
  });

  it("Open terminal deep-links to the selected project's work tab, then closes", async () => {
    const { args, onOpenTerminal } = makeArgs();
    const h = renderUseHandoff(args);
    act(() => h.current.openHandoff());
    await flush();
    act(() => h.current.dispatch());
    await flush();
    act(() => h.current.openTerminal());
    expect(onOpenTerminal).toHaveBeenCalledWith("repo");
    expect(h.current.open).toBe(false);
    h.unmount();
  });

  it("the scrim is not closable while dispatching", async () => {
    let resolveLaunch!: (v: unknown) => void;
    const invoke = vi.fn(async (cmd: string) => {
      if (cmd === "design_read_design_md") return "# Contract";
      if (cmd === "design_preview_capture") return { path: "preview.png" };
      if (cmd === "launch_project_agent_terminal")
        return new Promise((res) => {
          resolveLaunch = res;
        });
      return {};
    });
    const { args } = makeArgs({
      invoke: invoke as unknown as UseHandoffArgs["invoke"],
    });
    const h = renderUseHandoff(args);
    act(() => h.current.openHandoff());
    await flush();
    act(() => h.current.dispatch());
    await flush();
    expect(h.current.dispatching).toBe(true);
    expect(h.current.closable).toBe(false);
    await act(async () => {
      resolveLaunch({ agentId: "c1", role: "coder", client: "claude", rootPath: "C:/repo" });
      await Promise.resolve();
    });
    expect(h.current.closable).toBe(true);
    h.unmount();
  });
});
