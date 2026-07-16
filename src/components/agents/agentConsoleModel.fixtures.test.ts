// Fixture-based tests for the console model: prove the frontend models consume
// the REAL backend snapshot (rig/fixtures/console-activity.json) correctly.

import { describe, it, expect } from "vitest";
import {
  isEmptyActivity,
  consoleRunCount,
  type ConsoleActivity,
  type ConsoleEntry,
} from "./agentConsoleModel";
import { chatMessagesWithMilestones } from "../projects/planner/plannerModel";
// Direct JSON import — vitest resolves across the repo; tsconfig resolveJsonModule:true
import fixture from "../../../rig/fixtures/console-activity.json";

const activity = fixture as ConsoleActivity;

describe("agentConsoleModel against console-activity fixture", () => {
  it("isEmptyActivity is false — the fixture has entries", () => {
    expect(isEmptyActivity(activity)).toBe(false);
  });

  it("consoleRunCount returns 0 — running:false, runCount:0 (pill never shows)", () => {
    // The fixture recorded a RESTING snapshot: running=false, runCount=0.
    // The model returns 0 when not running; the tab pill is gated on `running`.
    expect(consoleRunCount(activity)).toBe(0);
  });

  it("entry KINDS in timeline order exactly match the recorded snapshot", () => {
    const kinds = (activity.entries ?? []).map((e) => e.type);
    expect(kinds).toEqual([
      "chat",       // user
      "chat",       // assistant
      "coder",      // dot — 🔧 Calling `write`
      "coder",      // args line
      "coder",      // sage — ✅ `write` → wrote
      "coder",      // sage — ⚑ Censor review started
      "thinking",
      "webSearch",
      "banner",
    ]);
  });

  it("chatMessagesWithMilestones: 2 chat bubbles + 4 milestone rows, thinking/webSearch/banner dropped", () => {
    // chatMessagesWithMilestones keeps chat (user/assistant, NOT plan) + coder/spawn as
    // milestones. thinking/webSearch/banner are dropped.
    const messages = chatMessagesWithMilestones(activity.entries);
    expect(messages).toHaveLength(6);
    expect(messages[0]).toEqual({ role: "user", text: "write a hello function" });
    expect(messages[1]).toEqual({
      role: "assistant",
      text: "I'll write a hello function",
    });
    expect(messages[2]).toEqual({
      role: "milestone",
      text: "🔧 Calling `write`",
    });
    expect(messages[3]).toEqual({
      role: "milestone",
      text: '  args: {"path":"src/hello.rs","content":"pub fn hello() -> &\'static str { \\"hello\\" }"}',
    });
    expect(messages[4]).toEqual({
      role: "milestone",
      text: "✅ `write` → wrote src/hello.rs",
    });
    expect(messages[5]).toEqual({
      role: "milestone",
      text: "⚑ Censor review started for: src/hello.rs",
    });
  });

  it("role:'plan' chat filtering — inject a plan entry, prove it is excluded from chat bubbles", () => {
    // The fixture has no plan entry. We construct a DIFFERENT input containing one
    // to verify the filter — NOT re-deriving the fixture's own output.
    const entriesWithPlan: ConsoleEntry[] = [
      ...(activity.entries ?? []),
      {
        type: "chat",
        role: "plan",
        text: '{"title":"My Plan","steps":[]}',
        time: "",
      },
    ];
    const messages = chatMessagesWithMilestones(entriesWithPlan);
    const planMessages = messages.filter((m) => m.text.includes("My Plan"));
    expect(planMessages).toHaveLength(0);
  });

  it("webSearch entry carries real URLs from the fixture", () => {
    const ws = activity.entries?.find(
      (e) => e.type === "webSearch",
    );
    expect(ws).toBeDefined();
    if (ws && ws.type === "webSearch") {
      expect(ws.query).toBe("rust hello function");
      expect(ws.pages).toHaveLength(2);
      expect(ws.pages[0].url).toBe(
        "https://docs.rust-lang.org/std/primitive.fn.html",
      );
      expect(ws.pages[1].title).toBe("Hello World in Rust");
    }
  });

  it("banner entry text matches the fixture", () => {
    const banner = activity.entries?.find((e) => e.type === "banner");
    expect(banner).toBeDefined();
    if (banner && banner.type === "banner") {
      expect(banner.text).toBe("Sidecar error [sidecar]: network timeout");
    }
  });
});
