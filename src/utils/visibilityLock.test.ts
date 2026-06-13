// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { installVisibilityLock } from "./visibilityLock";

function setHidden(hidden: boolean) {
  Object.defineProperty(document, "visibilityState", {
    value: hidden ? "hidden" : "visible",
    configurable: true,
  });
  document.dispatchEvent(new Event("visibilitychange"));
}

describe("installVisibilityLock — grace period before auto-lock", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    setHidden(false);
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("does NOT lock on a brief hide that becomes visible again before the grace", () => {
    const lock = vi.fn();
    const cleanup = installVisibilityLock(lock, 20000);
    setHidden(true);
    vi.advanceTimersByTime(5000); // < grace
    setHidden(false); // came back
    vi.advanceTimersByTime(60000);
    expect(lock).not.toHaveBeenCalled();
    cleanup();
  });

  it("locks once the window stays hidden for the whole grace period", () => {
    const lock = vi.fn();
    const cleanup = installVisibilityLock(lock, 20000);
    setHidden(true);
    vi.advanceTimersByTime(19999);
    expect(lock).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(lock).toHaveBeenCalledTimes(1);
    cleanup();
  });

  it("cleanup removes the listener and cancels a pending lock", () => {
    const lock = vi.fn();
    const cleanup = installVisibilityLock(lock, 20000);
    setHidden(true);
    cleanup();
    vi.advanceTimersByTime(60000);
    expect(lock).not.toHaveBeenCalled();
  });

  it("a second hide after returning re-arms the timer", () => {
    const lock = vi.fn();
    const cleanup = installVisibilityLock(lock, 20000);
    setHidden(true);
    vi.advanceTimersByTime(5000);
    setHidden(false);
    setHidden(true);
    vi.advanceTimersByTime(20000);
    expect(lock).toHaveBeenCalledTimes(1);
    cleanup();
  });
});
