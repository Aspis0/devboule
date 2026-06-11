// @vitest-environment jsdom
//
// Tests for the direct-DOM design canvas: sanitized markup rendering, hidden-node
// skipping, selection ring, and a drag that commits ONCE through the manifest path
// with snapped values + a bring-to-front z-bump. jsdom (v29) lacks PointerEvent, so
// we synthesize a MouseEvent carrying the pointer fields the handlers read (same
// idiom the retired Canvas.binding.test used).

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
    components: { hero: "<section><h1>Hi</h1></section>" },
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
  act(() => {
    root = createRoot(container);
    root.render(
      createElement(DesignCanvas, {
        project,
        onManifestChange: vi.fn(),
        onSelect: vi.fn(),
        ...props,
      }),
    );
  });
  const rerender = (p: DesignProject) =>
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
  return { container, root, rerender };
}

describe("DesignCanvas — rendering", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    vi.restoreAllMocks();
  });

  it("renders node markup through the sanitize chokepoint (strips onerror/class/id)", () => {
    const project = baseProject({
      components: {
        hero:
          '<section id="evil" class="leak"><img src="x" onerror="alert(1)">' +
          "<p>safe</p></section>",
      },
    });
    const { container, root } = mount(project);
    const content = container.querySelector(".node-content") as HTMLElement;
    expect(content).toBeTruthy();
    const html = content.innerHTML.toLowerCase();
    expect(html).toContain("safe");
    expect(html).not.toContain("onerror");
    expect(html).not.toContain("alert(1)");
    expect(html).not.toContain('id="evil"');
    expect(html).not.toContain("leak"); // class stripped
    act(() => root.unmount());
  });

  it("B2: renders an inner position:absolute node inside the containing .node-card", () => {
    // The CSS containment (.node-card { position:relative; overflow:hidden }) is what
    // actually CLIPS an absolutely-positioned overlay. jsdom does no layout, so we
    // assert the STRUCTURE that makes the CSS effective: the absolute content lives
    // INSIDE the .node-card (the positioned containing block), not hoisted out of it,
    // and the position:absolute is preserved (it is NOT stripped — see sanitize W7).
    const project = baseProject({
      components: {
        hero:
          '<div style="position:absolute;top:-9999px;left:-9999px">x</div>',
      },
    });
    const { container, root } = mount(project);
    const card = container.querySelector(".node-card") as HTMLElement;
    expect(card).toBeTruthy();
    const inner = card.querySelector(".node-content > div") as HTMLElement;
    expect(inner).toBeTruthy(); // the absolute div is nested inside the card
    expect(inner.getAttribute("style")).toContain("position:absolute");
    act(() => root.unmount());
  });

  it("V3: a nested data-node-id in markup is NOT a top-level host (no measurement clobber)", () => {
    // Node "b"'s markup embeds the data-node-id of SIBLING "a". The sanitize chokepoint
    // re-allows data-node-id when its value matches the id charset, so the nested attribute
    // survives into the DOM. The measuredH effect must scope to `:scope > [data-node-id]`
    // (the top-level .node-host divs only) so the nested attribute can never clobber "a"'s
    // measurement with the inner element's height.
    const project = baseProject({
      meta: {
        ...baseProject().meta,
        nodeOrder: ["a", "b"],
      },
      manifest: {
        schemaVersion: 1,
        nodes: {
          a: { x: 0, y: 0, z: 1, w: 200, h: "auto", kind: "html" },
          b: { x: 0, y: 300, z: 2, w: 200, h: "auto", kind: "html" },
        },
      },
      components: {
        a: "<p>node a</p>",
        // Hostile markup: embeds a sibling's data-node-id.
        b: '<div data-node-id="a"><p>nested impostor</p></div>',
      },
    });
    const { container, root } = mount(project);
    const world = container.querySelector(".canvas-world") as HTMLElement;
    expect(world).toBeTruthy();

    // The nested attribute survives sanitization (proves the hazard is real)…
    const nested = container.querySelector(
      '.node-content [data-node-id="a"]',
    ) as HTMLElement;
    expect(nested).toBeTruthy();

    // …but the SCOPED selector the measure loop uses sees ONLY the two top-level hosts,
    // each exactly once — so "a" maps to the host div, never the nested impostor.
    const scoped = world.querySelectorAll<HTMLElement>(":scope > [data-node-id]");
    const ids = Array.from(scoped).map((el) => el.getAttribute("data-node-id"));
    expect(ids).toEqual(["a", "b"]);
    // The matched "a" element is the host (a direct child of world), not the nested div.
    const hostA = Array.from(scoped).find(
      (el) => el.getAttribute("data-node-id") === "a",
    ) as HTMLElement;
    expect(hostA.parentElement).toBe(world);
    expect(hostA.classList.contains("node-host")).toBe(true);
    expect(hostA).not.toBe(nested);
    act(() => root.unmount());
  });

  it("does NOT render a hidden node", () => {
    const project = baseProject({
      meta: {
        ...baseProject().meta,
        nodeOrder: ["hero", "ghost"],
      },
      manifest: {
        schemaVersion: 1,
        nodes: {
          hero: { x: 0, y: 0, z: 1, w: 200, h: "auto", kind: "html" },
          ghost: { x: 0, y: 0, z: 2, w: 200, h: "auto", kind: "html", hidden: true },
        },
      },
      components: { hero: "<p>seen</p>", ghost: "<p>HIDDEN_MARK</p>" },
    });
    const { container, root } = mount(project);
    expect(container.querySelector('[data-node-id="hero"]')).toBeTruthy();
    expect(container.querySelector('[data-node-id="ghost"]')).toBeNull();
    expect(container.innerHTML).not.toContain("HIDDEN_MARK");
    act(() => root.unmount());
  });

  it("shows the selection ring class on the selected host", () => {
    const project = baseProject();
    const { container, root } = mount(project, { selectedId: "hero" });
    const host = container.querySelector('[data-node-id="hero"]') as HTMLElement;
    expect(host.className).toContain("sel");
    // The ring element is present (CSS shows it only under .sel).
    expect(host.querySelector(".node-ring")).toBeTruthy();
    act(() => root.unmount());
  });

  it("renders the empty-state card when there are no nodes", () => {
    const project = baseProject({
      meta: { ...baseProject().meta, nodeOrder: [] },
      manifest: { schemaVersion: 1, nodes: {} },
      components: {},
    });
    const { container, root } = mount(project);
    expect(container.querySelector(".canvas-empty")).toBeTruthy();
    act(() => root.unmount());
  });
});

describe("DesignCanvas — drag commit", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    vi.restoreAllMocks();
  });

  it("commits ONCE on pointer-up with grid-snapped position + bring-to-front", () => {
    const onManifestChange = vi.fn();
    const onBeginChange = vi.fn();
    const project = baseProject({
      meta: { ...baseProject().meta, nodeOrder: ["hero", "other"] },
      manifest: {
        schemaVersion: 1,
        nodes: {
          hero: { x: 40, y: 40, z: 1, w: 320, h: "auto", kind: "html" },
          other: { x: 600, y: 600, z: 5, w: 200, h: "auto", kind: "html" },
        },
      },
      components: { hero: "<p>h</p>", other: "<p>o</p>" },
    });
    const { container, root } = mount(project, {
      onManifestChange,
      onBeginChange,
      selectedId: "hero",
    });
    const host = container.querySelector('[data-node-id="hero"]') as HTMLElement;

    // pointerdown on the host begins a move (zoom defaults to 0.85).
    act(() => host.dispatchEvent(pointer("pointerdown", 100, 100, host)));
    // Move far enough to clear the 1px "moved" threshold; commit divides by zoom.
    act(() => window.dispatchEvent(pointer("pointermove", 200, 100)));
    act(() => window.dispatchEvent(pointer("pointerup", 200, 100)));

    expect(onManifestChange).toHaveBeenCalledTimes(1);
    expect(onBeginChange).toHaveBeenCalledTimes(1);
    const next = onManifestChange.mock.calls[0][0];
    // x moved by (200-100)/0.85 ≈ 117.6, +40 = 157.6, grid-snapped to a multiple of 8.
    expect(next.nodes.hero.x % 8).toBe(0);
    expect(next.nodes.hero.x).toBeGreaterThan(40);
    // Bring-to-front folded in: z is now above the previous max (5).
    expect(next.nodes.hero.z).toBeGreaterThan(5);
    act(() => root.unmount());
  });

  it("does NOT commit when the pointer never moves past the click threshold", () => {
    const onManifestChange = vi.fn();
    const project = baseProject();
    const { container, root } = mount(project, {
      onManifestChange,
      selectedId: "hero",
    });
    const host = container.querySelector('[data-node-id="hero"]') as HTMLElement;
    act(() => host.dispatchEvent(pointer("pointerdown", 100, 100, host)));
    act(() => window.dispatchEvent(pointer("pointerup", 100, 100)));
    expect(onManifestChange).not.toHaveBeenCalled();
    act(() => root.unmount());
  });
});
