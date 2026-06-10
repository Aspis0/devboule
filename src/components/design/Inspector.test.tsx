// @vitest-environment jsdom
//
// Tests for the node Inspector float card: arrange buttons drive the manifestOps
// z-order through onManifestChange, radius/elevation patch the placement, and
// Duplicate clones placement + markup through onProjectChange.

import { describe, it, expect, afterEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { Inspector } from "./Inspector";
import type { DesignManifest, DesignProject } from "../../types/design";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

function project(): DesignProject {
  return {
    meta: {
      schemaVersion: 1,
      id: "p",
      name: "T",
      createdAt: "1970-01-01T00:00:00Z",
      updatedAt: "1970-01-01T00:00:00Z",
      canvas: { w: 1440, h: 1024, grid: 8 },
      nodeOrder: ["a", "b", "c"],
    },
    manifest: {
      schemaVersion: 1,
      nodes: {
        a: { x: 0, y: 0, z: 1, w: 300, h: "auto", kind: "html", name: "Alpha" },
        b: { x: 0, y: 0, z: 2, w: 300, h: "auto", kind: "html", name: "Beta" },
        c: { x: 0, y: 0, z: 3, w: 300, h: "auto", kind: "html", name: "Gamma" },
      },
    },
    components: { a: "<p>a</p>", b: "<p>b</p>", c: "<p>c</p>" },
  };
}

function mount(
  selectedId: string,
  onManifestChange = vi.fn(),
  onProjectChange = vi.fn(),
  onSelect = vi.fn(),
): { container: HTMLElement; root: Root } {
  const wrap = document.createElement("div");
  wrap.className = "canvas-wrap";
  document.body.appendChild(wrap);
  let root!: Root;
  act(() => {
    root = createRoot(wrap);
    root.render(
      createElement(Inspector, {
        project: project(),
        selectedId,
        onManifestChange,
        onProjectChange,
        onSelect,
      }),
    );
  });
  return { container: wrap, root };
}

function clickByTitle(container: HTMLElement, title: string) {
  const btn = container.querySelector(`button[title^="${title}"]`) as HTMLButtonElement;
  expect(btn).toBeTruthy();
  act(() => btn.dispatchEvent(new MouseEvent("click", { bubbles: true })));
  return btn;
}

describe("Inspector — Arrange (z-order via manifestOps)", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    vi.restoreAllMocks();
  });

  it("Bring to front sets the selected node's z above the current max", () => {
    const onManifestChange = vi.fn();
    const { container, root } = mount("a", onManifestChange);
    clickByTitle(container, "Bring to front");
    expect(onManifestChange).toHaveBeenCalledTimes(1);
    const m = onManifestChange.mock.calls[0][0] as DesignManifest;
    expect(m.nodes.a.z).toBeGreaterThan(3);
    act(() => root.unmount());
  });

  it("Move forward swaps z with the next-higher neighbour", () => {
    const onManifestChange = vi.fn();
    const { container, root } = mount("a", onManifestChange);
    clickByTitle(container, "Move forward");
    const m = onManifestChange.mock.calls[0][0] as DesignManifest;
    // a (z=1) swaps with b (z=2): a->2, b->1.
    expect(m.nodes.a.z).toBe(2);
    expect(m.nodes.b.z).toBe(1);
    act(() => root.unmount());
  });

  it("Send to back / Move backward are DISABLED for the bottom node", () => {
    const { container, root } = mount("a");
    const back = container.querySelector(
      'button[title^="Send to back"]',
    ) as HTMLButtonElement;
    const backward = container.querySelector(
      'button[title^="Move backward"]',
    ) as HTMLButtonElement;
    expect(back.disabled).toBe(true);
    expect(backward.disabled).toBe(true);
    act(() => root.unmount());
  });

  it("Bring to front / Move forward are DISABLED for the top node", () => {
    const { container, root } = mount("c");
    const front = container.querySelector(
      'button[title^="Bring to front"]',
    ) as HTMLButtonElement;
    const forward = container.querySelector(
      'button[title^="Move forward"]',
    ) as HTMLButtonElement;
    expect(front.disabled).toBe(true);
    expect(forward.disabled).toBe(true);
    act(() => root.unmount());
  });
});

describe("Inspector — Corners / Elevation patch placement", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    vi.restoreAllMocks();
  });

  it("a radius token writes `radius`", () => {
    const onManifestChange = vi.fn();
    const { container, root } = mount("a", onManifestChange);
    const radBtn = container.querySelector(
      'button[title="radius.lg · 22px"]',
    ) as HTMLButtonElement;
    act(() => radBtn.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    const m = onManifestChange.mock.calls[0][0] as DesignManifest;
    expect(m.nodes.a.radius).toBe(22);
    act(() => root.unmount());
  });

  it("Flat writes `flat: true`", () => {
    const onManifestChange = vi.fn();
    const { container, root } = mount("a", onManifestChange);
    const flatBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent === "Flat",
    ) as HTMLButtonElement;
    act(() => flatBtn.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    const m = onManifestChange.mock.calls[0][0] as DesignManifest;
    expect(m.nodes.a.flat).toBe(true);
    act(() => root.unmount());
  });
});

// M4+W3: NumField commits ONE logical edit (on blur / Enter), parses with Number,
// ignores non-numeric input, and reverts on Esc — NOT one onChange per keystroke.
function numInput(container: HTMLElement, label: string): HTMLInputElement {
  const fields = Array.from(container.querySelectorAll(".numf"));
  const field = fields.find(
    (f) => f.querySelector("label")?.textContent === label,
  );
  const input = field?.querySelector("input") as HTMLInputElement;
  expect(input).toBeTruthy();
  return input;
}

function type(input: HTMLInputElement, value: string) {
  act(() => {
    // React tracks the value via the native setter; set it then dispatch input.
    const setter = Object.getOwnPropertyDescriptor(
      window.HTMLInputElement.prototype,
      "value",
    )?.set;
    setter?.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

describe("Inspector — NumField commits one edit (M4+W3)", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    vi.restoreAllMocks();
  });

  it("does NOT fire onChange while typing; commits ONCE on blur", () => {
    const onManifestChange = vi.fn();
    const { container, root } = mount("a", onManifestChange);
    const x = numInput(container, "X");
    type(x, "1");
    type(x, "12");
    type(x, "120");
    // No commit yet — typing must not patch the manifest per keystroke.
    expect(onManifestChange).not.toHaveBeenCalled();
    act(() => x.dispatchEvent(new FocusEvent("focusout", { bubbles: true })));
    expect(onManifestChange).toHaveBeenCalledTimes(1);
    const m = onManifestChange.mock.calls[0][0] as DesignManifest;
    expect(m.nodes.a.x).toBe(120);
    act(() => root.unmount());
  });

  it("blur with non-numeric/empty draft commits NOTHING (no NaN/0 patch)", () => {
    const onManifestChange = vi.fn();
    const { container, root } = mount("a", onManifestChange);
    const x = numInput(container, "X");
    // A `type=number` input coerces "abc" to "" — the empty-draft guard must stop
    // it committing 0 (Number("") === 0).
    type(x, "abc");
    act(() => x.dispatchEvent(new FocusEvent("focusout", { bubbles: true })));
    expect(onManifestChange).not.toHaveBeenCalled();
    act(() => root.unmount());
  });

  it("Enter commits the parsed value", () => {
    const onManifestChange = vi.fn();
    const { container, root } = mount("a", onManifestChange);
    const y = numInput(container, "Y");
    type(y, "44");
    act(() =>
      y.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
      ),
    );
    expect(onManifestChange).toHaveBeenCalledTimes(1);
    const m = onManifestChange.mock.calls[0][0] as DesignManifest;
    expect(m.nodes.a.y).toBe(44);
    act(() => root.unmount());
  });

  it("Esc reverts the draft and commits nothing", () => {
    const onManifestChange = vi.fn();
    const { container, root } = mount("a", onManifestChange);
    const x = numInput(container, "X");
    type(x, "999");
    act(() =>
      x.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      ),
    );
    expect(onManifestChange).not.toHaveBeenCalled();
    // draft reverted to the original committed value (0).
    expect(x.value).toBe("0");
    act(() => root.unmount());
  });

  it("W field clamps below MIN_W via the parent (commit passes raw, parent clamps)", () => {
    const onManifestChange = vi.fn();
    const { container, root } = mount("a", onManifestChange);
    const w = numInput(container, "W");
    type(w, "10");
    act(() => w.dispatchEvent(new FocusEvent("focusout", { bubbles: true })));
    expect(onManifestChange).toHaveBeenCalledTimes(1);
    const m = onManifestChange.mock.calls[0][0] as DesignManifest;
    // The parent's onChange applies Math.max(MIN_W, v) = max(240, 10) = 240.
    expect(m.nodes.a.w).toBe(240);
    act(() => root.unmount());
  });
});

describe("Inspector — Duplicate clones placement + markup via onProjectChange", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    vi.restoreAllMocks();
  });

  it("adds a new node offset by +32 with the highest z and copied markup", () => {
    const onProjectChange = vi.fn();
    const onSelect = vi.fn();
    const { container, root } = mount("a", vi.fn(), onProjectChange, onSelect);
    const dup = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.includes("Duplicate"),
    ) as HTMLButtonElement;
    act(() => dup.dispatchEvent(new MouseEvent("click", { bubbles: true })));

    expect(onProjectChange).toHaveBeenCalledTimes(1);
    const next = onProjectChange.mock.calls[0][0] as DesignProject;
    const newIds = Object.keys(next.manifest.nodes).filter(
      (id) => !["a", "b", "c"].includes(id),
    );
    expect(newIds).toHaveLength(1);
    const copy = next.manifest.nodes[newIds[0]];
    expect(copy.x).toBe(32); // 0 + 32
    expect(copy.y).toBe(32);
    expect(copy.z).toBe(4); // max(1,2,3)+1
    expect(next.components[newIds[0]]).toBe("<p>a</p>");
    expect(next.meta.nodeOrder).toContain(newIds[0]);
    expect(onSelect).toHaveBeenCalledWith(newIds[0]);
    act(() => root.unmount());
  });
});
