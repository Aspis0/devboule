// @vitest-environment node
import { describe, it, expect } from "vitest";
import { flattenActions } from "./AgentTimelineStrip";
import type { ConsoleActivity } from "./agentConsoleModel";

describe("flattenActions", () => {
  it("returns [] for an empty / activity with no entries", () => {
    expect(flattenActions({})).toEqual([]);
    expect(flattenActions({ entries: [] })).toEqual([]);
  });

  it("ignores coder milestones (they carry no actions)", () => {
    const activity: ConsoleActivity = {
      entries: [{ type: "coder", text: "planned", time: "10:00" }],
    };
    expect(flattenActions(activity)).toEqual([]);
  });

  it("flattens a spawn entry's rounds → actions in order", () => {
    const activity: ConsoleActivity = {
      entries: [
        { type: "coder", text: "spawned mini", time: "10:00" },
        {
          type: "spawn",
          text: "mini",
          time: "10:01",
          mini: {
            model: "mini · sonnet-4",
            scope: ["a.rs"],
            rounds: [
              {
                n: 1,
                actions: [
                  { kind: "read", verb: "Read", target: "a.rs" },
                  { kind: "write", verb: "Write", target: "a.rs", ok: true },
                ],
              },
              {
                n: 2,
                actions: [{ kind: "run", verb: "Run", target: "cargo test", status: "run" }],
              },
            ],
          },
        },
      ],
    };
    const out = flattenActions(activity);
    expect(out.map((a) => a.verb)).toEqual(["Read", "Write", "Run"]);
    expect(out[2].status).toBe("run");
  });
});
