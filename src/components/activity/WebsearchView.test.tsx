// @vitest-environment jsdom
//
// WebsearchView is the shared websearch renderer extracted from StageWebsearch: the
// live-page carousel (left) + distilled findings (right), and the calm idle state when
// nothing is running. Pure + prop-driven (no IO; header/mode toggle stays in StageWebsearch).

import { describe, it, expect } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import { WebsearchView } from "./WebsearchView";
import type { StagePage, StageFinding } from "../projects/planner/plannerModel";

const html = (props: Parameters<typeof WebsearchView>[0]) =>
  renderToStaticMarkup(createElement(WebsearchView, props));

describe("WebsearchView", () => {
  it("shows the calm idle state when not live and no pages", () => {
    const out = html({ pages: [], findings: [], isAuto: true, live: false });
    expect(out).toContain("searching the web right now");
  });

  // F25: parent used to pass live=!!orchestratorAgentId, which kept the
  // skeleton forever even with zero search events.
  it("stays idle when live=true but pages and findings are empty (F25)", () => {
    const out = html({ pages: [], findings: [], isAuto: true, live: true });
    expect(out).toContain("searching the web right now");
    expect(out).not.toContain("READING LIVE PAGES");
  });

  it("renders a page hostname when a live page is present", () => {
    const pages: StagePage[] = [{ url: "https://docs.rs/tokio", title: "Tokio", summary: "" }];
    const out = html({ pages, findings: [], isAuto: true, live: true });
    expect(out).toContain("docs.rs");
  });

  it("renders findings text and a linked task marker", () => {
    const findings: StageFinding[] = [
      { text: "use a bounded channel", task: 3 },
      { text: "no unbounded growth" },
    ];
    const out = html({ pages: [], findings, isAuto: false, live: true });
    expect(out).toContain("use a bounded channel");
    expect(out).toContain("no unbounded growth");
    expect(out).toContain("task 3");
  });
});
