// @vitest-environment jsdom
//
// Toast tests: renders when a message is set, auto-dismisses after the duration via
// onDismiss, and clears its timer on unmount / message change (no leaked timer).

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { Toast } from "./Toast";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

function mount(msg: string | null, onDismiss: () => void) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let root!: Root;
  act(() => {
    root = createRoot(container);
    root.render(createElement(Toast, { msg, onDismiss }));
  });
  const rerender = (m: string | null) =>
    act(() => root.render(createElement(Toast, { msg: m, onDismiss })));
  return { container, root, rerender };
}

describe("Toast", () => {
  it("renders nothing when msg is null", () => {
    const { container } = mount(null, vi.fn());
    expect(container.querySelector(".toast")).toBeNull();
  });

  it("renders the message when set", () => {
    const { container } = mount("Saved to working folder", vi.fn());
    expect(container.querySelector(".toast")?.textContent).toContain(
      "Saved to working folder",
    );
  });

  it("auto-dismisses after 2400ms by default", () => {
    const onDismiss = vi.fn();
    mount("Exported", onDismiss);
    expect(onDismiss).not.toHaveBeenCalled();
    act(() => vi.advanceTimersByTime(2399));
    expect(onDismiss).not.toHaveBeenCalled();
    act(() => vi.advanceTimersByTime(1));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it("clears the timer on unmount (no late dismiss)", () => {
    const onDismiss = vi.fn();
    const { root } = mount("Exported", onDismiss);
    act(() => root.unmount());
    act(() => vi.advanceTimersByTime(5000));
    expect(onDismiss).not.toHaveBeenCalled();
  });

  it("restarts the countdown when the message changes", () => {
    const onDismiss = vi.fn();
    const { rerender } = mount("first", onDismiss);
    act(() => vi.advanceTimersByTime(2000));
    rerender("second"); // resets the 2400ms timer
    act(() => vi.advanceTimersByTime(2000));
    expect(onDismiss).not.toHaveBeenCalled(); // only 2000ms since the reset
    act(() => vi.advanceTimersByTime(400));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });
});
