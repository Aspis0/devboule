// @vitest-environment jsdom
//
// CensorStrip is the sober bottom inspection row: per-file CLEAN (sage) / DIRTY (coral · N)
// status, summarised from the Censor findings. Pure + prop-driven; data-* hooks for tests.

import { describe, it, expect } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import { CensorStrip } from "./CensorStrip";
import { buildCensorStrip } from "./censorStripModel";
import type { CensorFinding } from "../../types/backend";

const f = (file: string, disposition: CensorFinding["disposition"]): CensorFinding =>
  ({
    id: `${file}-${disposition}`, file, contentHash: "h", line: null, severity: "low",
    category: "style", source: "gemma", title: "t", body: "b", verdict: "suspected",
    disposition, provenance: [], createdAt: "x",
  }) as unknown as CensorFinding;

const html = (props: Parameters<typeof CensorStrip>[0]) =>
  renderToStaticMarkup(createElement(CensorStrip, props));

describe("CensorStrip", () => {
  it("renders the CENSOR label", () => {
    const out = html({ model: buildCensorStrip([]) });
    expect(out).toContain("CENSOR");
  });

  it("shows a dirty file with its open-finding count", () => {
    const out = html({
      model: buildCensorStrip([f("src/auth/login.ts", "open"), f("src/auth/login.ts", "open")]),
    });
    expect(out).toContain("login.ts");
    expect(out).toMatch(/data-censor-file="src\/auth\/login\.ts"[^>]*data-censor-status="dirty"|data-censor-status="dirty"[^>]*data-censor-file="src\/auth\/login\.ts"/);
    expect(out).toContain("2");
  });

  it("shows a clean file as clean", () => {
    const out = html({ model: buildCensorStrip([f("src/a.ts", "fixed")]) });
    expect(out).toMatch(/data-censor-file="src\/a\.ts"[^>]*data-censor-status="clean"|data-censor-status="clean"[^>]*data-censor-file="src\/a\.ts"/);
  });

  it("shows a calm all-clean state when there are no findings", () => {
    const out = html({ model: buildCensorStrip([]) });
    expect(out.toLowerCase()).toContain("clean");
  });
});
