// @vitest-environment jsdom
//
// usePreview unit tests: the openPreview export→open order, the visualCheck
// capture→thumbnail→critique chain, the capture-not-open hint, the
// critique-error surfacing, and the no-concurrent-visualCheck guard. Driven
// through a tiny raw-DOM renderHook (matching the repo's react-dom/client + act
// harness — no testing-library dependency).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, createElement, useEffect } from "react";
import { createRoot, type Root } from "react-dom/client";
import { usePreview, type UsePreview, type UsePreviewDeps } from "./usePreview";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

/** A live handle whose `current` always reflects the hook's latest return value. */
interface HookHandle {
  readonly current: UsePreview;
  unmount: () => void;
}

/** Render the hook and expose a live handle to its latest return value. */
function renderUsePreview(deps: UsePreviewDeps): HookHandle {
  const ref: { value: UsePreview } = { value: null as unknown as UsePreview };
  function Probe() {
    const api = usePreview(deps);
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

function makeDeps(over: Partial<UsePreviewDeps> = {}): {
  deps: UsePreviewDeps;
  invoke: ReturnType<typeof vi.fn>;
  runExport: ReturnType<typeof vi.fn>;
  rememberThumbnail: ReturnType<typeof vi.fn>;
} {
  const invoke = vi.fn(async () => ({}) as unknown);
  const runExport = vi.fn(async () => true);
  const rememberThumbnail = vi.fn();
  const deps: UsePreviewDeps = {
    getFolder: () => "C:/proj",
    tauri: true,
    invoke: invoke as unknown as UsePreviewDeps["invoke"],
    runExport: runExport as unknown as UsePreviewDeps["runExport"],
    rememberThumbnail,
    ...over,
  };
  return { deps, invoke, runExport, rememberThumbnail };
}

describe("usePreview.openPreview", () => {
  beforeEach(() => vi.clearAllMocks());

  it("exports the mode then opens the preview window, in that order", async () => {
    const order: string[] = [];
    const runExport = vi.fn(async () => {
      order.push("export");
      return true;
    });
    const invoke = vi.fn(async (cmd: string) => {
      order.push(cmd);
      return {};
    });
    const { deps } = makeDeps({
      runExport: runExport as unknown as UsePreviewDeps["runExport"],
      invoke: invoke as unknown as UsePreviewDeps["invoke"],
    });
    const h = renderUsePreview(deps);
    await act(async () => {
      await h.current.openPreview("absolute");
    });
    expect(runExport).toHaveBeenCalledWith("absolute");
    expect(invoke).toHaveBeenCalledWith("design_preview_open", {
      workingFolderPath: "C:/proj",
      mode: "absolute",
    });
    expect(order).toEqual(["export", "design_preview_open"]);
    h.unmount();
  });

  it("does not open when no folder is set", async () => {
    const { deps, invoke } = makeDeps({ getFolder: () => "  " });
    const h = renderUsePreview(deps);
    await act(async () => {
      await h.current.openPreview("absolute");
    });
    expect(invoke).not.toHaveBeenCalled();
    h.unmount();
  });

  it("aborts the open (no stale preview) when the export fails", async () => {
    // BLOCKER regression: runExport rejected → resolves false. openPreview MUST NOT
    // invoke design_preview_open (which would show an OLD export). DesignView surfaces
    // the underlying error itself, so the hook just stops.
    const runExport = vi.fn(async () => false);
    const { deps, invoke } = makeDeps({
      runExport: runExport as unknown as UsePreviewDeps["runExport"],
    });
    const h = renderUsePreview(deps);
    await act(async () => {
      await h.current.openPreview("absolute");
    });
    expect(runExport).toHaveBeenCalledWith("absolute");
    expect(invoke).not.toHaveBeenCalledWith(
      "design_preview_open",
      expect.anything(),
    );
    expect(h.current.opening).toBe(false);
    h.unmount();
  });

  it("surfaces an unexpected export throw without opening", async () => {
    // Defense in depth: even if runExport throws (it shouldn't — it returns false), the
    // hook's try/catch surfaces it and never opens the window.
    const invoke = vi.fn(async () => ({}) as unknown);
    const { deps } = makeDeps({
      invoke: invoke as unknown as UsePreviewDeps["invoke"],
      runExport: (async () => {
        throw new Error("Export not found — run Export first");
      }) as unknown as UsePreviewDeps["runExport"],
    });
    const h = renderUsePreview(deps);
    await act(async () => {
      await h.current.openPreview("absolute");
    });
    expect(h.current.error).toContain("Export not found");
    expect(invoke).not.toHaveBeenCalledWith(
      "design_preview_open",
      expect.anything(),
    );
    h.unmount();
  });
});

describe("usePreview.visualCheck", () => {
  beforeEach(() => vi.clearAllMocks());

  it("captures, records the thumbnail, then critiques (happy path)", async () => {
    const calls: string[] = [];
    const invoke = vi.fn(async (cmd: string) => {
      calls.push(cmd);
      if (cmd === "design_preview_capture") return { path: "preview.png", bytes: 100 };
      if (cmd === "design_visual_critique") return { critique: "Contrast is low." };
      return {};
    });
    const { deps, rememberThumbnail } = makeDeps({
      invoke: invoke as unknown as UsePreviewDeps["invoke"],
    });
    const h = renderUsePreview(deps);
    let outcome: Awaited<ReturnType<UsePreview["visualCheck"]>> | null = null;
    await act(async () => {
      outcome = await h.current.visualCheck();
    });
    expect(calls).toEqual(["design_preview_capture", "design_visual_critique"]);
    expect(rememberThumbnail).toHaveBeenCalledWith("C:/proj");
    expect(outcome).toEqual({ kind: "ok", critique: "Contrast is low." });
    expect(h.current.error).toBeNull();
    h.unmount();
  });

  it("forwards a trimmed focus to the critique", async () => {
    const invoke = vi.fn(async (cmd: string) => {
      if (cmd === "design_preview_capture") return { path: "preview.png", bytes: 1 };
      if (cmd === "design_visual_critique") return { critique: "ok" };
      return {};
    });
    const { deps } = makeDeps({
      invoke: invoke as unknown as UsePreviewDeps["invoke"],
    });
    const h = renderUsePreview(deps);
    await act(async () => {
      await h.current.visualCheck("  the hero  ");
    });
    expect(invoke).toHaveBeenCalledWith("design_visual_critique", {
      workingFolderPath: "C:/proj",
      focus: "the hero",
    });
    h.unmount();
  });

  it("surfaces the capture error with an open-first hint and skips critique", async () => {
    const invoke = vi.fn(async (cmd: string) => {
      if (cmd === "design_preview_capture") {
        throw new Error("Preview window is not open");
      }
      return {};
    });
    const { deps, rememberThumbnail } = makeDeps({
      invoke: invoke as unknown as UsePreviewDeps["invoke"],
    });
    const h = renderUsePreview(deps);
    const box: { outcome: Awaited<ReturnType<UsePreview["visualCheck"]>> | null } = {
      outcome: null,
    };
    await act(async () => {
      box.outcome = await h.current.visualCheck();
    });
    const outcome = box.outcome;
    expect(outcome?.kind).toBe("error");
    if (outcome?.kind === "error") {
      expect(outcome.message).toContain("Preview window is not open");
      expect(outcome.message).toContain("open the preview first");
    }
    expect(h.current.error).toContain("open the preview first");
    expect(rememberThumbnail).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalledWith("design_visual_critique", expect.anything());
    h.unmount();
  });

  it("does NOT append the open-first hint to a non-'not open' capture error", async () => {
    // A real capture failure (the window IS open) must be surfaced verbatim — telling the
    // user to "open the preview first" would be misleading.
    const invoke = vi.fn(async (cmd: string) => {
      // Tauri rejects IPC with a plain string, not an Error instance.
      if (cmd === "design_preview_capture") {
        return Promise.reject("preview capture timed out");
      }
      return {};
    });
    const { deps } = makeDeps({
      invoke: invoke as unknown as UsePreviewDeps["invoke"],
    });
    const h = renderUsePreview(deps);
    const box: { outcome: Awaited<ReturnType<UsePreview["visualCheck"]>> | null } = {
      outcome: null,
    };
    await act(async () => {
      box.outcome = await h.current.visualCheck();
    });
    const outcome = box.outcome;
    expect(outcome?.kind).toBe("error");
    if (outcome?.kind === "error") {
      expect(outcome.message).toBe("preview capture timed out");
      expect(outcome.message).not.toContain("open the preview first");
    }
    expect(h.current.error).not.toContain("open the preview first");
    h.unmount();
  });

  it("surfaces a critique (Ollama-unconfigured) backend error verbatim", async () => {
    const invoke = vi.fn(async (cmd: string) => {
      if (cmd === "design_preview_capture") return { path: "preview.png", bytes: 1 };
      if (cmd === "design_visual_critique") {
        // Tauri rejects IPC with a plain string, not an Error instance.
        return Promise.reject("Local AI (Ollama) is not configured for this project.");
      }
      return {};
    });
    const { deps } = makeDeps({
      invoke: invoke as unknown as UsePreviewDeps["invoke"],
    });
    const h = renderUsePreview(deps);
    let outcome: Awaited<ReturnType<UsePreview["visualCheck"]>> | null = null;
    await act(async () => {
      outcome = await h.current.visualCheck();
    });
    expect(outcome).toEqual({
      kind: "error",
      message: "Local AI (Ollama) is not configured for this project.",
    });
    expect(h.current.error).toBe("Local AI (Ollama) is not configured for this project.");
    h.unmount();
  });

  it("beginCheck claims the slot synchronously (second claim refused)", () => {
    const { deps } = makeDeps();
    const h = renderUsePreview(deps);
    // First synchronous claim wins; a second in the same tick is refused.
    let first = false;
    let second = false;
    act(() => {
      first = h.current.beginCheck();
      second = h.current.beginCheck();
    });
    expect(first).toBe(true);
    expect(second).toBe(false);
    h.unmount();
  });

  it("visualCheck adopts a beginCheck claim (no skip) and releases it", async () => {
    const invoke = vi.fn(async (cmd: string) => {
      if (cmd === "design_preview_capture") return { path: "preview.png", bytes: 1 };
      if (cmd === "design_visual_critique") return { critique: "ok" };
      return {};
    });
    const { deps } = makeDeps({
      invoke: invoke as unknown as UsePreviewDeps["invoke"],
    });
    const h = renderUsePreview(deps);
    let claimed = false;
    let outcome: Awaited<ReturnType<UsePreview["visualCheck"]>> | null = null;
    await act(async () => {
      claimed = h.current.beginCheck();
      // visualCheck must ADOPT the claim, not treat it as "already running" and skip.
      outcome = await h.current.visualCheck();
    });
    expect(claimed).toBe(true);
    expect(outcome).toEqual({ kind: "ok", critique: "ok" });
    // Claim released: a subsequent beginCheck succeeds again.
    let again = false;
    act(() => {
      again = h.current.beginCheck();
    });
    expect(again).toBe(true);
    h.unmount();
  });

  it("guards against two concurrent visual checks", async () => {
    let resolveCapture: (v: unknown) => void = () => {};
    const captureGate = new Promise((r) => (resolveCapture = r));
    const invoke = vi.fn(async (cmd: string) => {
      if (cmd === "design_preview_capture") {
        await captureGate;
        return { path: "preview.png", bytes: 1 };
      }
      if (cmd === "design_visual_critique") return { critique: "done" };
      return {};
    });
    const { deps } = makeDeps({
      invoke: invoke as unknown as UsePreviewDeps["invoke"],
    });
    const h = renderUsePreview(deps);
    let first: Promise<Awaited<ReturnType<UsePreview["visualCheck"]>>>;
    let second: Awaited<ReturnType<UsePreview["visualCheck"]>> | null = null;
    await act(async () => {
      first = h.current.visualCheck();
      // Second call while the first is mid-capture must short-circuit (skipped).
      second = await h.current.visualCheck();
    });
    expect(second).toEqual({ kind: "skipped" });
    await act(async () => {
      resolveCapture({});
      await first;
    });
    // Exactly one capture happened (the guarded second never reached the backend).
    const captureCalls = invoke.mock.calls.filter(
      (c) => c[0] === "design_preview_capture",
    );
    expect(captureCalls).toHaveLength(1);
    h.unmount();
  });
});
