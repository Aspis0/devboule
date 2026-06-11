// @vitest-environment jsdom
//
// CE-mode + Spot Edit integration at the CANVAS level:
//   - double-click a node enters CE (.node-host.content), clicking an inner element
//     selects it (ce-sel) and shows the toolbar, a swatch recolors it, exiting (click
//     outside / Esc) fires onNodeMarkupCommit with the serialized (helper-stripped)
//     markup carrying the recolor;
//   - a generation/undo that removes the CE node aborts CE SILENTLY (no commit);
//   - the Spot Edit tool sets data-tool="ai" and a region drag → Analyze calls
//     onRegionAnalyze with world-coord polygon points.

import { describe, it, expect, afterEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { DesignCanvas } from "./DesignCanvas";
import type { DesignProject } from "../../../types/design";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

function pointer(type: string, x: number, y: number, target?: Element): Event {
  const e = new MouseEvent(type, { clientX: x, clientY: y, bubbles: true });
  Object.defineProperty(e, "pointerId", { value: 1, configurable: true });
  if (target) Object.defineProperty(e, "target", { value: target, configurable: true });
  return e;
}

function baseProject(over?: Partial<DesignProject>): DesignProject {
  return {
    meta: {
      schemaVersion: 1,
      id: "p",
      name: "T",
      createdAt: "1970-01-01T00:00:00Z",
      updatedAt: "1970-01-01T00:00:00Z",
      canvas: { w: 1440, h: 1024, grid: 8 },
      nodeOrder: ["hero"],
    },
    manifest: {
      schemaVersion: 1,
      nodes: { hero: { x: 40, y: 40, z: 1, w: 320, h: "auto", kind: "html" } },
    },
    components: {
      hero: '<section data-node-id="hero"><h1>Hi</h1><p>Body</p></section>',
    },
    ...over,
  };
}

function mount(
  project: DesignProject,
  props: Partial<React.ComponentProps<typeof DesignCanvas>> = {},
): { container: HTMLElement; root: Root; rerender: (p: DesignProject) => void } {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let root!: Root;
  const render = (p: DesignProject) =>
    act(() => {
      root.render(
        createElement(DesignCanvas, {
          project: p,
          onManifestChange: vi.fn(),
          onSelect: vi.fn(),
          ...props,
        }),
      );
    });
  act(() => {
    root = createRoot(container);
  });
  render(project);
  return { container, root, rerender: render };
}

describe("DesignCanvas — content-edit mode", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    vi.restoreAllMocks();
  });

  it("dblclick node → select inner el → recolor via swatch → exit fires onNodeMarkupCommit with the serialized recolor", () => {
    const onNodeMarkupCommit = vi.fn();
    const onBeginChange = vi.fn();
    const tokens = {
      color: { brand: { $value: "#c2410c", $type: "color" } },
    };
    const { container } = mount(baseProject(), {
      onNodeMarkupCommit,
      onBeginChange,
      tokens,
      selectedId: "hero",
    });
    const host = container.querySelector('[data-node-id="hero"]') as HTMLElement;

    // Enter CE: double-click the node host.
    act(() => host.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })));
    expect(host.className).toContain("content");

    // Click the inner <h1> to select it (the click handler reads e.target).
    const h1 = host.querySelector("h1") as HTMLElement;
    act(() => h1.dispatchEvent(pointer("click", 50, 50, h1)));
    expect(h1.classList.contains("ce-sel")).toBe(true);

    // The toolbar is mounted with a swatch; click it (text mode default).
    const sw = container.querySelector(".ce-sw") as HTMLButtonElement;
    expect(sw).toBeTruthy();
    act(() => sw.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    // The live element now carries the inline color.
    expect(h1.style.color).toBeTruthy();

    // Exit CE by pressing Esc — commits the change.
    act(() =>
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })),
    );
    expect(onNodeMarkupCommit).toHaveBeenCalledTimes(1);
    const [nodeId, raw] = onNodeMarkupCommit.mock.calls[0];
    expect(nodeId).toBe("hero");
    // Serialized markup carries the recolor and NO helper classes/attrs.
    expect(raw).toContain("color");
    expect(raw).not.toContain("ce-sel");
    expect(raw).not.toContain("contenteditable");
    // History was snapshotted before the commit (via the canvas) — onBeginChange is
    // the parent's hook; the canvas commit goes through onNodeMarkupCommit, and the
    // PARENT pushes history. At the canvas layer we just assert the commit fired.
  });

  it("does NOT commit when nothing changed (pure click then Esc)", () => {
    const onNodeMarkupCommit = vi.fn();
    const { container } = mount(baseProject(), { onNodeMarkupCommit });
    const host = container.querySelector('[data-node-id="hero"]') as HTMLElement;
    act(() => host.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })));
    const h1 = host.querySelector("h1") as HTMLElement;
    act(() => h1.dispatchEvent(pointer("click", 50, 50, h1)));
    act(() =>
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })),
    );
    expect(onNodeMarkupCommit).not.toHaveBeenCalled();
  });

  it("Delete removes the selected inner element (commit reflects the removal)", () => {
    const onNodeMarkupCommit = vi.fn();
    const { container } = mount(baseProject(), { onNodeMarkupCommit });
    const host = container.querySelector('[data-node-id="hero"]') as HTMLElement;
    act(() => host.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })));
    const p = host.querySelector("p") as HTMLElement;
    act(() => p.dispatchEvent(pointer("click", 50, 50, p)));
    act(() =>
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Delete" })),
    );
    act(() =>
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })),
    );
    expect(onNodeMarkupCommit).toHaveBeenCalledTimes(1);
    const raw = onNodeMarkupCommit.mock.calls[0][1] as string;
    expect(raw).not.toContain("Body"); // the <p>Body</p> was removed
    expect(raw).toContain("Hi"); // the <h1> survives
  });

  it("aborts CE SILENTLY when the node vanishes (generation/undo) — no commit", () => {
    const onNodeMarkupCommit = vi.fn();
    const { container, rerender } = mount(baseProject(), { onNodeMarkupCommit });
    const host = container.querySelector('[data-node-id="hero"]') as HTMLElement;
    act(() => host.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })));
    const h1 = host.querySelector("h1") as HTMLElement;
    act(() => h1.dispatchEvent(pointer("click", 50, 50, h1)));
    // The node is removed by a "generation" (manifest no longer has it).
    rerender(
      baseProject({
        meta: { ...baseProject().meta, nodeOrder: [] },
        manifest: { schemaVersion: 1, nodes: {} },
        components: {},
      }),
    );
    // CE exited silently; no commit fired.
    expect(onNodeMarkupCommit).not.toHaveBeenCalled();
    expect(container.querySelector(".node-host.content")).toBeNull();
  });

  it("aborts CE SILENTLY when the node's stored markup changes underneath (generation on the SAME node) — no commit", () => {
    const onNodeMarkupCommit = vi.fn();
    const { container, rerender } = mount(baseProject(), { onNodeMarkupCommit });
    const host = container.querySelector('[data-node-id="hero"]') as HTMLElement;
    act(() => host.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })));
    const h1 = host.querySelector("h1") as HTMLElement;
    act(() => h1.dispatchEvent(pointer("click", 50, 50, h1)));
    // Make a live DOM edit (recolor) so a normal exit WOULD commit — proving the abort
    // is what suppresses the commit, not "nothing changed".
    const sw = container.querySelector(".ce-sw") as HTMLButtonElement;
    if (sw) act(() => sw.dispatchEvent(new MouseEvent("click", { bubbles: true })));

    // A generation lands on the SAME node: same id stays in the manifest, but the stored
    // component markup is replaced. The external content wins; CE exits silently.
    rerender(
      baseProject({
        components: {
          hero: '<section data-node-id="hero"><h1>Regenerated</h1></section>',
        },
      }),
    );
    expect(onNodeMarkupCommit).not.toHaveBeenCalled();
    expect(container.querySelector(".node-host.content")).toBeNull();
  });
});

describe("DesignCanvas — Spot Edit tool", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    vi.restoreAllMocks();
  });

  it("selecting Spot Edit sets data-tool=ai; region drag + Analyze calls onRegionAnalyze with world points", () => {
    const onRegionAnalyze = vi.fn();
    const { container } = mount(baseProject(), { onRegionAnalyze });
    // Click the Spot Edit tool button (the 2nd tool-pill button).
    const toolButtons = container.querySelectorAll<HTMLButtonElement>(
      ".tool-pill button",
    );
    expect(toolButtons).toHaveLength(2);
    act(() =>
      toolButtons[1].dispatchEvent(new MouseEvent("click", { bubbles: true })),
    );
    const viewport = container.querySelector(".canvas-viewport") as HTMLElement;
    expect(viewport.getAttribute("data-tool")).toBe("ai");

    // Draw a region: pointerdown on the viewport, move, up. (getBCR is 0 in jsdom, so
    // world coords == screen coords / zoom; we just need a >24px box.)
    act(() => viewport.dispatchEvent(pointer("pointerdown", 100, 100, viewport)));
    act(() => window.dispatchEvent(pointer("pointermove", 400, 400)));
    act(() => window.dispatchEvent(pointer("pointerup", 400, 400)));

    // The prompt bar appears; click Analyze.
    const analyze = container.querySelector(".ai-bar .go") as HTMLButtonElement;
    expect(analyze).toBeTruthy();
    act(() => analyze.dispatchEvent(new MouseEvent("click", { bubbles: true })));

    expect(onRegionAnalyze).toHaveBeenCalledTimes(1);
    const [pts, prompt] = onRegionAnalyze.mock.calls[0];
    expect(Array.isArray(pts)).toBe(true);
    expect(pts.length).toBeGreaterThanOrEqual(3);
    expect(prompt).toBe(""); // empty prompt -> auto-detect (handled by the parent)
  });

  it("pointercancel during a region draw ends the gesture (a later move does not resize it)", () => {
    const onRegionAnalyze = vi.fn();
    const { container } = mount(baseProject(), { onRegionAnalyze });
    const toolButtons = container.querySelectorAll<HTMLButtonElement>(
      ".tool-pill button",
    );
    act(() =>
      toolButtons[1].dispatchEvent(new MouseEvent("click", { bubbles: true })),
    );
    const viewport = container.querySelector(".canvas-viewport") as HTMLElement;

    // Begin a draw, grow the box, then OS-cancel the gesture (pointercancel).
    act(() => viewport.dispatchEvent(pointer("pointerdown", 100, 100, viewport)));
    act(() => window.dispatchEvent(pointer("pointermove", 400, 400)));
    act(() => window.dispatchEvent(pointer("pointercancel", 400, 400)));

    // The region remains (a real-sized box was drawn), and the bar is shown.
    const bar = container.querySelector(".ai-bar") as HTMLElement;
    expect(bar).toBeTruthy();
    const outlineBefore =
      container.querySelector(".ai-outline polygon")?.getAttribute("points") ?? "";

    // The drag listeners were torn down by pointercancel: a later move must NOT reshape.
    act(() => window.dispatchEvent(pointer("pointermove", 900, 900)));
    const outlineAfter =
      container.querySelector(".ai-outline polygon")?.getAttribute("points") ?? "";
    expect(outlineAfter).toBe(outlineBefore);
  });
});
