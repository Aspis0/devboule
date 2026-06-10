import { describe, it, expect } from "vitest";
import {
  createHistory,
  push,
  undo,
  redo,
  MAX_HISTORY,
  type History,
} from "./history";

describe("history — empty state", () => {
  it("a fresh history can neither undo nor redo", () => {
    const h = createHistory<number>();
    expect(h.canUndo).toBe(false);
    expect(h.canRedo).toBe(false);
    expect(undo(h, 1)).toBeNull();
    expect(redo(h, 1)).toBeNull();
  });
});

describe("history — push", () => {
  it("push records a snapshot and enables undo", () => {
    const h = push(createHistory<number>(), 1);
    expect(h.canUndo).toBe(true);
    expect(h.past).toEqual([1]);
    expect(h.future).toEqual([]);
  });

  it("push does not mutate the input history", () => {
    const h0 = createHistory<number>();
    const before = JSON.stringify(h0);
    push(h0, 1);
    expect(JSON.stringify(h0)).toBe(before);
  });

  it("a push clears the redo (future) branch", () => {
    // push(1) -> current becomes 2; undo back to 1 (future now has 2); push(9)
    // must drop the redo branch so 2 is no longer redoable.
    let h = push(createHistory<number>(), 1); // past:[1]
    const u = undo(h, 2)!; // value 1, past:[], future:[2]
    h = u.history;
    expect(h.canRedo).toBe(true);
    h = push(h, 9); // a new edit: future cleared
    expect(h.canRedo).toBe(false);
    expect(h.future).toEqual([]);
    expect(h.past).toEqual([9]);
  });
});

describe("history — undo / redo round-trip", () => {
  it("undo returns the previous snapshot and moves current into future", () => {
    const h = push(createHistory<number>(), 10); // past:[10]
    const u = undo(h, 20)!; // current is 20
    expect(u.value).toBe(10);
    expect(u.history.past).toEqual([]);
    expect(u.history.future).toEqual([20]);
    expect(u.history.canUndo).toBe(false);
    expect(u.history.canRedo).toBe(true);
  });

  it("redo restores the undone snapshot and pushes current back to past", () => {
    const h = push(createHistory<number>(), 10);
    const u = undo(h, 20)!; // value 10, future:[20]
    const r = redo(u.history, u.value)!; // current is now 10 (u.value)
    expect(r.value).toBe(20);
    expect(r.history.past).toEqual([10]);
    expect(r.history.future).toEqual([]);
    expect(r.history.canUndo).toBe(true);
    expect(r.history.canRedo).toBe(false);
  });

  it("undo/redo neither mutate their input history", () => {
    const h = push(createHistory<number>(), 1);
    const beforeUndo = JSON.stringify(h);
    const u = undo(h, 2)!;
    expect(JSON.stringify(h)).toBe(beforeUndo); // input untouched
    const beforeRedo = JSON.stringify(u.history);
    redo(u.history, u.value);
    expect(JSON.stringify(u.history)).toBe(beforeRedo);
  });

  it("a multi-step sequence undoes in LIFO order", () => {
    // Build current=3 with past=[1,2] (snapshots of the two prior states).
    let h: History<number> = createHistory<number>();
    h = push(h, 1); // edit from 1 -> 2
    h = push(h, 2); // edit from 2 -> 3, current is 3
    const u1 = undo(h, 3)!; // back to 2
    expect(u1.value).toBe(2);
    const u2 = undo(u1.history, u1.value)!; // back to 1
    expect(u2.value).toBe(1);
    expect(u2.history.canUndo).toBe(false);
  });
});

describe("history — cap (MAX_HISTORY)", () => {
  it("evicts the OLDEST entry past the cap", () => {
    let h = createHistory<number>();
    for (let i = 0; i < MAX_HISTORY + 5; i++) h = push(h, i);
    expect(h.past.length).toBe(MAX_HISTORY);
    // The first 5 (0..4) were evicted; the oldest retained is index 5.
    expect(h.past[0]).toBe(5);
    expect(h.past[h.past.length - 1]).toBe(MAX_HISTORY + 4);
  });

  it("MAX_HISTORY is 60", () => {
    expect(MAX_HISTORY).toBe(60);
  });
});
