// @vitest-environment jsdom
//
// Tests for the floating content-edit toolbar: it renders the element's tag, applies
// a swatch (text vs fill), and fires move/delete/Ask-AI callbacks. jsdom returns
// zero rects (no layout), but the toolbar still positions (top/left numbers) and
// renders, which is all these behavioural assertions need.

import { describe, it, expect, afterEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { ContentToolbar } from "./ContentToolbar";
import type { ColorTokenSwatch } from "../engine/tokens";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const swatches: ColorTokenSwatch[] = [
  { name: "color.brand", value: "#c2410c" },
  { name: "color.ink", value: "#37291a" },
];

function setup(
  props: Partial<React.ComponentProps<typeof ContentToolbar>> = {},
  elHtml = "<h1>Title</h1>",
): {
  container: HTMLElement;
  root: Root;
  el: HTMLElement;
  wrap: HTMLElement;
  callbacks: Record<string, ReturnType<typeof vi.fn>>;
} {
  const wrap = document.createElement("div");
  wrap.className = "canvas-wrap";
  // The edited element lives in the (sanitized) node content; mount it so getBCR works.
  wrap.innerHTML = `<div class="node-content">${elHtml}</div>`;
  document.body.appendChild(wrap);
  const el = wrap.querySelector(".node-content")!.firstElementChild as HTMLElement;

  const container = document.createElement("div");
  document.body.appendChild(container);
  const callbacks = {
    onEditText: vi.fn(),
    onColor: vi.fn(),
    onMove: vi.fn(),
    onRemove: vi.fn(),
    onAskAi: vi.fn(),
  };
  let root!: Root;
  act(() => {
    root = createRoot(container);
    root.render(
      createElement(ContentToolbar, {
        el,
        wrapEl: wrap,
        version: 0,
        swatches,
        ...callbacks,
        ...props,
      }),
    );
  });
  return { container, root, el, wrap, callbacks };
}

describe("ContentToolbar", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    vi.restoreAllMocks();
  });

  it("renders the element tag name", () => {
    const { container, root } = setup({}, "<section>x</section>");
    const tag = container.querySelector(".ce-tag");
    expect(tag?.textContent).toBe("section");
    act(() => root.unmount());
  });

  it("renders the provided token swatches (and falls back when empty)", () => {
    const { container, root } = setup();
    expect(container.querySelectorAll(".ce-sw")).toHaveLength(2);
    act(() => root.unmount());

    const empty = setup({ swatches: [] });
    // Fallback neutral palette has 5 swatches.
    expect(empty.container.querySelectorAll(".ce-sw")).toHaveLength(5);
    act(() => empty.root.unmount());
  });

  it("applies a swatch as TEXT color by default, FILL after toggling", () => {
    const { container, root, callbacks } = setup();
    const sw = container.querySelectorAll<HTMLButtonElement>(".ce-sw");
    act(() => sw[0].dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(callbacks.onColor).toHaveBeenLastCalledWith("#c2410c", "text");

    // Toggle to fill (the 2nd mode button), then apply.
    const modeButtons = container.querySelectorAll<HTMLButtonElement>(".tb");
    // tb order: [edit-text, text-mode, fill-mode, up, down, trash, ask-ai]
    act(() =>
      modeButtons[2].dispatchEvent(new MouseEvent("click", { bubbles: true })),
    );
    act(() => sw[1].dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(callbacks.onColor).toHaveBeenLastCalledWith("#37291a", "fill");
    act(() => root.unmount());
  });

  it("disables Edit-text when the element has no direct text", () => {
    const { container, root, callbacks } = setup({}, "<div><span>x</span></div>");
    const editText = container.querySelectorAll<HTMLButtonElement>(".tb")[0];
    expect(editText.disabled).toBe(true);
    act(() => editText.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(callbacks.onEditText).not.toHaveBeenCalled();
    act(() => root.unmount());
  });

  it("fires onMove up/down and onRemove", () => {
    // Element has both prev and next siblings so up/down are enabled.
    const wrap = document.createElement("div");
    wrap.innerHTML =
      '<div class="node-content"><p>a</p><p id="mid">b</p><p>c</p></div>';
    document.body.appendChild(wrap);
    const el = wrap.querySelector("#mid") as HTMLElement;
    const container = document.createElement("div");
    document.body.appendChild(container);
    const onMove = vi.fn();
    const onRemove = vi.fn();
    let root!: Root;
    act(() => {
      root = createRoot(container);
      root.render(
        createElement(ContentToolbar, {
          el,
          wrapEl: wrap,
          version: 0,
          swatches,
          onEditText: vi.fn(),
          onColor: vi.fn(),
          onMove,
          onRemove,
          onAskAi: vi.fn(),
        }),
      );
    });
    const tb = container.querySelectorAll<HTMLButtonElement>(".tb");
    // [edit, text, fill, up, down, trash, ask-ai]
    act(() => tb[3].dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(onMove).toHaveBeenCalledWith("up");
    act(() => tb[4].dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(onMove).toHaveBeenCalledWith("down");
    act(() => tb[5].dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(onRemove).toHaveBeenCalledTimes(1);
    act(() => root.unmount());
  });

  it("fires onAskAi from the accent button", () => {
    const { container, root, callbacks } = setup();
    const tb = container.querySelectorAll<HTMLButtonElement>(".tb");
    const askAi = tb[tb.length - 1];
    act(() => askAi.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(callbacks.onAskAi).toHaveBeenCalledTimes(1);
    act(() => root.unmount());
  });
});
