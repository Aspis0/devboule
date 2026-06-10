// @vitest-environment jsdom
//
// Regression tests for the WEBVIEW-ONLY bug where a generated node appeared on
// the canvas but could not be selected or dragged. Root cause: the pointer
// delegation listener was bound ONLY inside the iframe `onLoad` handler, which
// `return`ed early (no retry) when `contentDocument` was null at the load
// instant — while the SEPARATE reinject effect still injected the nodes later.
// Result in WebView2: nodes present, listener never bound, pointerdown ignored.
//
// These tests assert the fix: delegation is bound via the inject/availability
// path (NOT `onLoad`), idempotently, even when `onLoad` fired with a null
// document first; and a pointerdown on an INNER interactive element (a `<button>`
// inside the host) still resolves to the host and starts a drag.
//
// jsdom (v29) lacks PointerEvent, so we synthesize a MouseEvent carrying the
// pointer fields the handlers read — matching useDrag.lifecycle.test.tsx.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { Canvas } from "./Canvas";
import { CANVAS_ROOT_ID, NODE_ID_ATTR } from "./iframeInject";
import type { DesignProject } from "../../types/design";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

function pointer(type: string, x: number, y: number, target?: Element): Event {
  const e = new MouseEvent(type, { clientX: x, clientY: y, bubbles: true });
  Object.defineProperty(e, "pointerId", { value: 1, configurable: true });
  if (target) Object.defineProperty(e, "target", { value: target, configurable: true });
  return e;
}

// A project whose single node's markup is a `<button>` — so a pointerdown on the
// INNER button must still resolve to the host and start a drag.
function buttonProject(): DesignProject {
  return {
    meta: {
      schemaVersion: 1,
      id: "p1",
      name: "Test",
      createdAt: "2026-01-01T00:00:00Z",
      updatedAt: "2026-01-01T00:00:00Z",
      canvas: { w: 1200, h: 800, grid: 8 },
      nodeOrder: ["btn"],
    },
    manifest: {
      schemaVersion: 1,
      nodes: { btn: { x: 40, y: 40, z: 1, w: 160, h: 48, kind: "html" } },
    },
    components: { btn: "<button>Click me</button>" },
  };
}

function mountCanvas(
  project: DesignProject,
  onManifestChange = vi.fn(),
  onSelect = vi.fn(),
): { container: HTMLElement; root: Root; iframe: HTMLIFrameElement } {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let root!: Root;
  act(() => {
    root = createRoot(container);
    root.render(
      createElement(Canvas, { project, onManifestChange, onSelect }),
    );
  });
  const iframe = container.querySelector("iframe") as HTMLIFrameElement;
  return { container, root, iframe };
}

// jsdom does not parse `srcDoc` into a live contentDocument the way a real
// browser does, so we install a controllable fake document the component reaches
// through `iframe.contentDocument`. This lets us model the WebView2 timing:
// contentDocument can be made null first (onLoad-with-null) then become ready.
function installContentDoc(iframe: HTMLIFrameElement): Document {
  const doc = document.implementation.createHTMLDocument("canvas");
  const root = doc.createElement("div");
  root.id = CANVAS_ROOT_ID;
  root.setAttribute("style", "position:relative");
  doc.body.appendChild(root);
  Object.defineProperty(iframe, "contentDocument", {
    value: doc,
    configurable: true,
  });
  return doc;
}

function setContentDocNull(iframe: HTMLIFrameElement): void {
  Object.defineProperty(iframe, "contentDocument", {
    value: null,
    configurable: true,
  });
}

describe("Canvas — pointer delegation binds independent of onLoad timing", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.runOnlyPendingTimers();
    vi.useRealTimers();
    vi.restoreAllMocks();
    document.body.innerHTML = "";
  });

  it("binds the delegation listener via the poll even when onLoad fired with a null document first", () => {
    const onSelect = vi.fn();
    const { iframe, root, container } = mountCanvas(buttonProject(), vi.fn(), onSelect);

    // WebView2 reality: onLoad fires while contentDocument is still null.
    setContentDocNull(iframe);
    act(() => {
      iframe.dispatchEvent(new Event("load"));
    });
    // Nothing bound yet, nothing to select — the early-null path must NOT crash
    // and must NOT have bound a listener.
    expect(onSelect).not.toHaveBeenCalled();

    // The document becomes ready slightly later (srcDoc parse completes).
    const doc = installContentDoc(iframe);

    // The poll (50ms interval) must inject + bind WITHOUT a second onLoad.
    act(() => {
      vi.advanceTimersByTime(60);
    });

    // Node was injected by the availability path.
    const host = doc.querySelector(`[${NODE_ID_ATTR}="btn"]`) as HTMLElement;
    expect(host).toBeTruthy();

    // A pointerdown on the host now reaches the bound delegation listener.
    act(() => {
      doc.dispatchEvent(pointer("pointerdown", 50, 50, host));
    });
    expect(onSelect).toHaveBeenCalledWith("btn");

    act(() => root.unmount());
    container.remove();
  });

  it("starts a drag from a pointerdown on an INNER interactive element (<button>)", () => {
    const onSelect = vi.fn();
    const { iframe, root, container } = mountCanvas(buttonProject(), vi.fn(), onSelect);

    const doc = installContentDoc(iframe);
    act(() => {
      vi.advanceTimersByTime(60);
    });

    const host = doc.querySelector(`[${NODE_ID_ATTR}="btn"]`) as HTMLElement;
    const innerButton = host.querySelector("button") as HTMLButtonElement;
    expect(innerButton).toBeTruthy();

    // pointerdown lands on the inner <button>, NOT the host div. The delegation
    // handler must `closest([data-node-id])` up to the host, select it, and start
    // a drag — proven by a following pointermove mutating the host's style.
    const styleBefore = host.getAttribute("style");
    act(() => {
      doc.dispatchEvent(pointer("pointerdown", 50, 50, innerButton));
    });
    expect(onSelect).toHaveBeenCalledWith("btn");

    act(() => {
      doc.dispatchEvent(pointer("pointermove", 80, 70));
    });
    // The live drag preview mutated the host inline style (move underway).
    expect(host.getAttribute("style")).not.toBe(styleBefore);

    act(() => {
      doc.dispatchEvent(pointer("pointerup", 80, 70));
    });

    act(() => root.unmount());
    container.remove();
  });

  it("binds exactly once on the live document (poll does not stack handlers)", () => {
    const onSelect = vi.fn();
    const { iframe, root, container } = mountCanvas(buttonProject(), vi.fn(), onSelect);

    const doc = installContentDoc(iframe);
    const addSpy = vi.spyOn(doc, "addEventListener");

    // Multiple poll ticks + a redundant onLoad must NOT add a second pointerdown
    // listener (boundDocRef guards re-binding on the same document).
    act(() => {
      vi.advanceTimersByTime(200);
    });
    act(() => {
      iframe.dispatchEvent(new Event("load"));
    });

    const pointerdownBinds = addSpy.mock.calls.filter(
      (c) => c[0] === "pointerdown",
    ).length;
    expect(pointerdownBinds).toBe(1);

    // And a single pointerdown still selects once (handler not duplicated).
    const host = doc.querySelector(`[${NODE_ID_ATTR}="btn"]`) as HTMLElement;
    act(() => {
      doc.dispatchEvent(pointer("pointerdown", 50, 50, host));
    });
    expect(onSelect).toHaveBeenCalledTimes(1);

    act(() => root.unmount());
    container.remove();
  });

  it("clears selection on a pointerdown over empty canvas (no host)", () => {
    const onSelect = vi.fn();
    const { iframe, root, container } = mountCanvas(buttonProject(), vi.fn(), onSelect);

    const doc = installContentDoc(iframe);
    act(() => {
      vi.advanceTimersByTime(60);
    });

    const canvasRoot = doc.getElementById(CANVAS_ROOT_ID) as HTMLElement;
    act(() => {
      doc.dispatchEvent(pointer("pointerdown", 5, 5, canvasRoot));
    });
    expect(onSelect).toHaveBeenCalledWith(null);

    act(() => root.unmount());
    container.remove();
  });
});
