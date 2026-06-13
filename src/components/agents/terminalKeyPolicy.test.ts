import { describe, it, expect, vi } from "vitest";
import { terminalKeyPolicy } from "./terminalKeyPolicy";

// #16 review (BLOCKER): with the grid interactive, a raw Ctrl+C must NOT emit ETX
// straight to the PTY — that bypasses the two-step SIGINT guard. terminalKeyPolicy
// routes Ctrl+C to the arm/confirm callback and swallows the key; every other key
// passes through so the grid stays typeable.
describe("terminalKeyPolicy (#16 interactive grid, Ctrl+C guard)", () => {
  it("lets ordinary keys through (returns true, no Ctrl+C callback)", () => {
    const onCtrlC = vi.fn();
    expect(
      terminalKeyPolicy({ type: "keydown", ctrlKey: false, key: "a" }, onCtrlC),
    ).toBe(true);
    expect(onCtrlC).not.toHaveBeenCalled();
  });

  it("swallows Ctrl+C and arms the two-step guard on keydown", () => {
    const onCtrlC = vi.fn();
    expect(
      terminalKeyPolicy({ type: "keydown", ctrlKey: true, key: "c" }, onCtrlC),
    ).toBe(false);
    expect(onCtrlC).toHaveBeenCalledTimes(1);
  });

  it("swallows Ctrl+C on keyup WITHOUT re-arming (no double fire)", () => {
    const onCtrlC = vi.fn();
    expect(
      terminalKeyPolicy({ type: "keyup", ctrlKey: true, key: "c" }, onCtrlC),
    ).toBe(false);
    expect(onCtrlC).not.toHaveBeenCalled();
  });

  it("does not intercept Cmd+C (copy: metaKey, not ctrlKey) — passes through", () => {
    const onCtrlC = vi.fn();
    expect(
      terminalKeyPolicy({ type: "keydown", ctrlKey: false, key: "c" }, onCtrlC),
    ).toBe(true);
    expect(onCtrlC).not.toHaveBeenCalled();
  });

  it("is robust when no Ctrl+C handler is supplied", () => {
    expect(terminalKeyPolicy({ type: "keydown", ctrlKey: true, key: "c" })).toBe(
      false,
    );
  });
});
