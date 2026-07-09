// @vitest-environment jsdom
//
// D4 (planner-chat demolition): delivery failures, launch guidance and the 90s
// silence watchdog are composer CHROME — an amber banner strip above the composer —
// never fake assistant messages spliced into the transcript. While the banner is
// set it also supersedes the "thinking…" pill (the strip explains why there is no
// reply; a spinning pill next to it would contradict it).

import { describe, it, expect } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import { PlannerChat } from "./PlannerChat";

const html = (props: Partial<Parameters<typeof PlannerChat>[0]>) =>
  renderToStaticMarkup(
    createElement(PlannerChat, {
      messages: [],
      modelLabel: "Orchestrator · test",
      live: false,
      awaitingReply: false,
      onSend: () => {},
      ...props,
    }),
  );

describe("PlannerChat banner (D4 chrome)", () => {
  it("renders the banner strip when set", () => {
    const out = html({ banner: "No reply from the orchestrator in 90s." });
    expect(out).toContain('data-testid="planner-banner"');
    expect(out).toContain("No reply from the orchestrator in 90s.");
  });

  it("renders no strip when the banner is absent", () => {
    expect(html({})).not.toContain("planner-banner");
    expect(html({ banner: null })).not.toContain("planner-banner");
  });

  it("supersedes the thinking pill while set", () => {
    const messages = [{ role: "user" as const, text: "go" }];
    const withPill = html({ messages, live: true });
    const withBanner = html({ messages, live: true, banner: "stalled" });
    // The live thinking affordance shows without the banner…
    expect(withPill).toContain("thinking");
    // …and is suppressed while the banner explains the silence.
    expect(withBanner).not.toContain("thinking");
    expect(withBanner).toContain("stalled");
  });

  it("banner text is chrome, not a transcript bubble", () => {
    const out = html({ banner: "delivery failed" });
    // The empty-transcript hint still shows: the banner added no message rows.
    expect(out).toContain("delivery failed");
    expect(out).toContain("Describe a goal");
  });
});

describe("PlannerChat orchestrator selector (S4)", () => {
  const orcs = [
    { id: "orchestrator", label: "Local" },
    { id: "claude", label: "Claude" },
    { id: "codex", label: "Codex" },
    { id: "openai", label: "OpenAI" },
  ];

  it("renders the segmented control with every label", () => {
    const out = html({
      orchestrators: orcs,
      orchestratorId: "claude",
      onOrchestratorChange: () => {},
    });
    expect(out).toContain("Local");
    expect(out).toContain("Claude");
    expect(out).toContain("Codex");
    expect(out).toContain("OpenAI");
  });

  it("highlights the active entry and pulses it while live", () => {
    const out = html({
      live: true,
      orchestrators: orcs,
      orchestratorId: "claude",
      onOrchestratorChange: () => {},
    });
    // The active entry runs the same pp-pulse animation used by the standalone selector.
    expect(out).toContain("pp-pulse");
  });

  it("omits the control when no orchestrators are supplied", () => {
    const out = html({});
    expect(out).not.toContain(">Local<");
  });
});
