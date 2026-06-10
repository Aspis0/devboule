// @vitest-environment jsdom
//
// Lifecycle/regression tests for the drag hook's listener bookkeeping. The pure
// commit math lives in dragMath.test.ts; here we drive `useDrag` through a real
// React render in jsdom and assert the listener add/remove pairing is exact (the
// `detach` dishonest-`[]`-deps fix) and that a finished drag commits once. No
// testing-library dependency — a tiny `react-dom/client` harness keeps this in
// line with the repo's existing dependency-free test approach.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, createElement, useEffect } from "react";
import { createRoot, type Root } from "react-dom/client";
import { useDrag, type DragMode } from "./useDrag";
import { buildShellHtml, CANVAS_ROOT_ID, NODE_ID_ATTR } from "./iframeInject";
import type { DesignManifest, DesignNodePlacement } from "../../types/design";

// Opt this file into React's act(...) environment (silences the warning and makes
// effect flushing deterministic under the manual react-dom/client harness).
(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

function placement(over: Partial<DesignNodePlacement> = {}): DesignNodePlacement {
  return { x: 0, y: 0, z: 1, w: 200, h: "auto", kind: "html", ...over };
}
function manifest(nodes: Record<string, DesignNodePlacement>): DesignManifest {
  return { schemaVersion: 1, nodes };
}

// Minimal pointer-event shim: jsdom (v29) lacks PointerEvent, so synthesize a
// MouseEvent carrying the pointer fields the hook reads. `target` is set so
// `beginDrag`'s `e.target.closest([data-node-id])` resolves the host.
function pointer(
  type: string,
  x: number,
  y: number,
  target?: Element,
): Event {
  const e = new MouseEvent(type, { clientX: x, clientY: y, bubbles: true });
  Object.defineProperty(e, "pointerId", { value: 1, configurable: true });
  if (target) Object.defineProperty(e, "target", { value: target, configurable: true });
  return e;
}

// Harness: mount a component that calls useDrag and exposes its `beginDrag`.
type Harness = {
  beginDrag: (id: string, mode: DragMode, e: PointerEvent) => void;
};

function mountHook(opts: {
  getDoc: () => Document | null;
  getManifest: () => DesignManifest;
  grid: number;
  onCommit: (m: DesignManifest) => void;
}): { handle: Harness; unmount: () => void; container: HTMLElement } {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const handle: Harness = { beginDrag: () => {} };
  let root: Root;

  function Probe() {
    const { beginDrag } = useDrag(opts);
    useEffect(() => {
      handle.beginDrag = beginDrag;
    }, [beginDrag]);
    return null;
  }

  act(() => {
    root = createRoot(container);
    root.render(createElement(Probe));
  });

  return {
    handle,
    container,
    unmount: () => {
      act(() => root.unmount());
      container.remove();
    },
  };
}

describe("useDrag — listener lifecycle (detach removes the exact instances)", () => {
  let canvasDoc: Document;
  let host: HTMLElement;

  beforeEach(() => {
    document.documentElement.innerHTML = buildShellHtml();
    canvasDoc = document;
    const root = canvasDoc.getElementById(CANVAS_ROOT_ID) as HTMLElement;
    host = canvasDoc.createElement("div");
    host.setAttribute(NODE_ID_ATTR, "hero");
    root.appendChild(host);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("removes every listener it added after a completed drag (no leak)", () => {
    const m = manifest({ hero: placement({ x: 0, y: 0 }) });
    const onCommit = vi.fn();
    const { handle, unmount } = mountHook({
      getDoc: () => canvasDoc,
      getManifest: () => m,
      grid: 8,
      onCommit,
    });

    const removed: string[] = [];
    const realRemove = canvasDoc.removeEventListener.bind(canvasDoc);
    vi.spyOn(canvasDoc, "removeEventListener").mockImplementation(
      (type: string, ...rest: unknown[]) => {
        removed.push(type);
        return (realRemove as unknown as (...a: unknown[]) => void)(
          type,
          ...rest,
        );
      },
    );

    act(() => {
      handle.beginDrag("hero", "move", pointer("pointerdown", 5, 5, host) as PointerEvent);
    });
    act(() => {
      canvasDoc.dispatchEvent(pointer("pointermove", 21, 13));
    });
    act(() => {
      canvasDoc.dispatchEvent(pointer("pointerup", 21, 13));
    });

    // pointerup teardown must remove move + up + cancel (the exact set added).
    expect(removed).toContain("pointermove");
    expect(removed).toContain("pointerup");
    expect(removed).toContain("pointercancel");

    // A stray pointermove AFTER the drag finished must NOT mutate the host (proves
    // the move listener was actually detached, not merely re-registered).
    const styleAfterCommit = host.getAttribute("style");
    act(() => {
      canvasDoc.dispatchEvent(pointer("pointermove", 999, 999));
    });
    expect(host.getAttribute("style")).toBe(styleAfterCommit);

    unmount();
  });

  it("commits the moved position exactly once on pointer-up", () => {
    const m = manifest({ hero: placement({ x: 0, y: 0 }) });
    const onCommit = vi.fn();
    const { handle, unmount } = mountHook({
      getDoc: () => canvasDoc,
      getManifest: () => m,
      grid: 8,
      onCommit,
    });

    act(() => {
      handle.beginDrag("hero", "move", pointer("pointerdown", 0, 0, host) as PointerEvent);
    });
    act(() => {
      canvasDoc.dispatchEvent(pointer("pointerup", 16, 16));
    });

    // bringToFront on a single-node manifest is a no-op (same z), so the ONLY
    // commit is the position one — exactly one call with the moved manifest.
    expect(onCommit).toHaveBeenCalledTimes(1);
    const committed = onCommit.mock.calls[0][0] as DesignManifest;
    expect(committed.nodes.hero.x).toBe(16);
    expect(committed.nodes.hero.y).toBe(16);

    unmount();
  });

  it("B1: aborts the commit when the manifest is REPLACED under the drag (moved node)", () => {
    // A generation/self-repair commits a new manifest mid-drag that moves `hero`
    // to a different base. Committing the start-delta against that new base would
    // teleport the node. The guard must abort: NO commit (the in-flight DOM
    // preview is discarded on the next React render).
    let current = manifest({ hero: placement({ x: 0, y: 0 }) });
    const onCommit = vi.fn();
    const { handle, unmount } = mountHook({
      getDoc: () => canvasDoc,
      getManifest: () => current,
      grid: 8,
      onCommit,
    });

    act(() => {
      handle.beginDrag("hero", "move", pointer("pointerdown", 0, 0, host) as PointerEvent);
    });
    // Swap in a manifest where hero sits at a DIFFERENT committed placement.
    current = manifest({ hero: placement({ x: 320, y: 240 }) });
    act(() => {
      canvasDoc.dispatchEvent(pointer("pointerup", 16, 16));
    });

    expect(onCommit).not.toHaveBeenCalled();
    unmount();
  });

  it("B1: aborts the commit when the dragged node is REMOVED mid-drag", () => {
    let current = manifest({ hero: placement({ x: 0, y: 0 }) });
    const onCommit = vi.fn();
    const { handle, unmount } = mountHook({
      getDoc: () => canvasDoc,
      getManifest: () => current,
      grid: 8,
      onCommit,
    });

    act(() => {
      handle.beginDrag("hero", "move", pointer("pointerdown", 0, 0, host) as PointerEvent);
    });
    // The node is gone in the fresh manifest (e.g. a generation removed it).
    current = manifest({ other: placement({ x: 10, y: 10 }) });
    act(() => {
      canvasDoc.dispatchEvent(pointer("pointerup", 16, 16));
    });

    expect(onCommit).not.toHaveBeenCalled();
    unmount();
  });

  it("B1: still commits normally when the manifest is unchanged under the drag", () => {
    // Control: the same node at the same committed base must commit (guard does not
    // over-abort). Two-node manifest where hero is already top so bringToFront is a
    // no-op, leaving exactly the position commit.
    let current = manifest({
      hero: placement({ x: 0, y: 0, z: 2 }),
      other: placement({ x: 50, y: 50, z: 1 }),
    });
    const onCommit = vi.fn();
    const { handle, unmount } = mountHook({
      getDoc: () => canvasDoc,
      getManifest: () => current,
      grid: 8,
      onCommit,
    });

    act(() => {
      handle.beginDrag("hero", "move", pointer("pointerdown", 0, 0, host) as PointerEvent);
    });
    // A NEW reference with the SAME hero placement — a benign re-render, not a swap.
    current = manifest({
      hero: placement({ x: 0, y: 0, z: 2 }),
      other: placement({ x: 50, y: 50, z: 1 }),
    });
    act(() => {
      canvasDoc.dispatchEvent(pointer("pointerup", 16, 16));
    });

    expect(onCommit).toHaveBeenCalledTimes(1);
    const committed = onCommit.mock.calls[0][0] as DesignManifest;
    expect(committed.nodes.hero.x).toBe(16);
    expect(committed.nodes.hero.y).toBe(16);
    unmount();
  });

  it("detaches listeners when unmounted mid-drag (no stale handler fires)", () => {
    const m = manifest({ hero: placement() });
    const onCommit = vi.fn();
    const { handle, unmount } = mountHook({
      getDoc: () => canvasDoc,
      getManifest: () => m,
      grid: 8,
      onCommit,
    });

    act(() => {
      handle.beginDrag("hero", "move", pointer("pointerdown", 0, 0, host) as PointerEvent);
    });
    unmount(); // unmount mid-drag -> effect teardown detaches

    const styleBefore = host.getAttribute("style");
    act(() => {
      canvasDoc.dispatchEvent(pointer("pointermove", 500, 500));
      canvasDoc.dispatchEvent(pointer("pointerup", 500, 500));
    });
    // No listener fired after unmount: host untouched, no commit.
    expect(host.getAttribute("style")).toBe(styleBefore);
    expect(onCommit).not.toHaveBeenCalled();
  });
});
