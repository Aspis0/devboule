// @vitest-environment jsdom
//
// ChatThread is the shared message-list renderer extracted from PlannerChat: bubbles
// (user right / assistant left), a streaming caret, the "thinking" affordance, the
// "awaiting your reply" pill, and an empty hint. Pure + prop-driven (no IO).

import { describe, it, expect } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import { ChatThread } from "./ChatThread";
import type { PlannerMessage } from "../projects/planner/plannerModel";

const html = (props: Parameters<typeof ChatThread>[0]) =>
  renderToStaticMarkup(createElement(ChatThread, props));

describe("ChatThread", () => {
  it("shows the empty hint when there are no messages", () => {
    const out = html({ messages: [], live: false, awaitingReply: false, emptyHint: "Describe a goal." });
    expect(out).toContain("Describe a goal.");
  });

  it("renders user and assistant message text", () => {
    const messages: PlannerMessage[] = [
      { role: "user", text: "wire the login" },
      { role: "assistant", text: "on it, planning now" },
    ];
    const out = html({ messages, live: true, awaitingReply: false });
    expect(out).toContain("wire the login");
    expect(out).toContain("on it, planning now");
  });

  it("renders a streaming caret for a streaming message", () => {
    const messages: PlannerMessage[] = [{ role: "assistant", text: "thinking", streaming: true }];
    const out = html({ messages, live: true, awaitingReply: false });
    expect(out).toContain("▌");
  });

  it("shows the thinking affordance when live and the last turn is the user's", () => {
    const messages: PlannerMessage[] = [{ role: "user", text: "go" }];
    const out = html({ messages, live: true, awaitingReply: false });
    expect(out.toLowerCase()).toContain("thinking");
  });

  it("shows the awaiting-reply pill when awaitingReply is set", () => {
    const messages: PlannerMessage[] = [{ role: "assistant", text: "which provider?" }];
    const out = html({ messages, live: true, awaitingReply: true });
    expect(out.toLowerCase()).toContain("awaiting");
  });
});
