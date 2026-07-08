// Unit tests for the framework-free console model + its pure helpers (node env).

import { describe, it, expect } from "vitest";
import {
  type Action,
  type ConsoleActivity,
  type ConsoleEntry,
  type ThinkingEntry,
  actionHasDetail,
  actionStatus,
  consoleRunCount,
  isEmptyActivity,
} from "./agentConsoleModel";

describe("isEmptyActivity", () => {
  it("treats null/undefined as empty", () => {
    expect(isEmptyActivity(null)).toBe(true);
    expect(isEmptyActivity(undefined)).toBe(true);
  });

  it("honors the explicit empty flag", () => {
    expect(isEmptyActivity({ empty: true })).toBe(true);
  });

  it("treats a missing/zero entries list as empty", () => {
    expect(isEmptyActivity({})).toBe(true);
    expect(isEmptyActivity({ entries: [] })).toBe(true);
  });

  it("is non-empty when there is at least one entry", () => {
    const a: ConsoleActivity = {
      entries: [{ type: "coder", text: "claimed", time: "00:00" }],
    };
    expect(isEmptyActivity(a)).toBe(false);
  });

  it("is empty when empty:true even if entries exist (explicit override)", () => {
    const a: ConsoleActivity = {
      empty: true,
      entries: [{ type: "coder", text: "x", time: "00:00" }],
    };
    expect(isEmptyActivity(a)).toBe(true);
  });
});

describe("consoleRunCount", () => {
  it("returns 0 when not running and no count", () => {
    expect(consoleRunCount(undefined)).toBe(0);
    expect(consoleRunCount({})).toBe(0);
    expect(consoleRunCount({ running: false })).toBe(0);
  });

  it("floors to 1 when running with an absent/invalid count (mock's ||1)", () => {
    expect(consoleRunCount({ running: true })).toBe(1);
    expect(consoleRunCount({ running: true, runCount: 0 })).toBe(1);
    expect(consoleRunCount({ running: true, runCount: -3 })).toBe(1);
    expect(consoleRunCount({ running: true, runCount: NaN })).toBe(1);
  });

  it("returns the positive integer count when running", () => {
    expect(consoleRunCount({ running: true, runCount: 2 })).toBe(2);
    expect(consoleRunCount({ running: true, runCount: 4.9 })).toBe(4);
  });

  it("ignores a stray count when NOT running (pill only shows while running)", () => {
    // The tab pill is gated on `running`, so a resting state never surfaces a count.
    expect(consoleRunCount({ runCount: 3 })).toBe(0);
    expect(consoleRunCount({ running: false, runCount: 3 })).toBe(0);
  });
});

describe("actionHasDetail", () => {
  it("is true with a non-empty diff", () => {
    const a: Action = {
      kind: "write",
      verb: "Write",
      diff: [{ t: "add", s: "x" }],
    };
    expect(actionHasDetail(a)).toBe(true);
  });

  it("is true with a non-empty output", () => {
    expect(
      actionHasDetail({ kind: "read", verb: "Read", output: "42 lines" }),
    ).toBe(true);
  });

  it("is false with neither (a static row)", () => {
    expect(actionHasDetail({ kind: "run", verb: "Run", status: "run" })).toBe(
      false,
    );
    expect(
      actionHasDetail({ kind: "write", verb: "Write", diff: [] }),
    ).toBe(false);
    expect(
      actionHasDetail({ kind: "read", verb: "Read", output: "" }),
    ).toBe(false);
  });
});

describe("actionStatus", () => {
  it("running wins over everything", () => {
    expect(
      actionStatus({ kind: "run", verb: "Run", status: "run", ok: true }),
    ).toEqual({ kind: "run", label: "running" });
  });

  it("ok===false is a fail", () => {
    expect(actionStatus({ kind: "write", verb: "Write", ok: false })).toEqual({
      kind: "fail",
      label: "fail",
    });
  });

  it("default / ok===true is ok", () => {
    expect(actionStatus({ kind: "read", verb: "Read" })).toEqual({
      kind: "ok",
      label: "ok",
    });
    expect(actionStatus({ kind: "read", verb: "Read", ok: true })).toEqual({
      kind: "ok",
      label: "ok",
    });
  });
});

describe("ThinkingEntry (console fidelity)", () => {
  it("is part of the ConsoleEntry union and serializes to type 'thinking'", () => {
    const entry: ThinkingEntry = {
      type: "thinking",
      text: "let me reason about this step\nsecond line",
      time: "10:01:02",
    };
    // Narrowing check: a ThinkingEntry must be assignable to the ConsoleEntry union.
    const asUnion: ConsoleEntry = entry;
    expect(asUnion.type).toBe("thinking");
    expect(asUnion).toEqual(entry);
  });

  it("is preserved when passed through an entries list (not filtered out)", () => {
    const entries: ConsoleEntry[] = [
      { type: "coder", text: "claimed", time: "10:00:00" },
      { type: "thinking", text: "hmm", time: "10:00:01" },
    ];
    // The console filters `question` entries out; `thinking` must survive that filter.
    const kept = entries.filter((e) => e.type !== "question");
    expect(kept.map((e) => e.type)).toEqual(["coder", "thinking"]);
  });
});
