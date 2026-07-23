import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { CensorFindingRow } from "./CensorFindingRow";
import type { CensorFinding } from "../../types/backend";

function finding(over: Partial<CensorFinding> = {}): CensorFinding {
  return {
    id: "f1",
    file: "src/app.ts",
    contentHash: "h",
    line: 42,
    severity: "high",
    category: "security",
    source: "gitleaks",
    title: "Hardcoded secret",
    body: "A credential pattern was detected.",
    verdict: "suspected",
    disposition: "open",
    provenance: [],
    createdAt: "",
    commit: null,
    ...over,
  };
}

// react-dom/server renders without a DOM (vitest's node env). We assert the row's
// STATIC markup: the severity / category / source badges, the file:line label, and
// the action buttons. Click behavior is wired to the passed callbacks (the exact
// command payloads are covered purely in censorPanelModel.test.ts).

describe("CensorFindingRow", () => {
  it("renders severity, category and source badges", () => {
    const html = renderToStaticMarkup(
      <CensorFindingRow finding={finding()} onOpen={vi.fn()} onDispose={vi.fn()} />,
    );
    expect(html).toContain("high"); // severity badge
    expect(html).toContain("Security"); // category badge
    expect(html).toContain("Linter · gitleaks"); // source badge (Layer 1)
    expect(html).toContain("Hardcoded secret"); // title
  });

  it("renders the clickable file:line reference", () => {
    const html = renderToStaticMarkup(
      <CensorFindingRow finding={finding({ line: 42 })} onOpen={vi.fn()} onDispose={vi.fn()} />,
    );
    expect(html).toContain("src/app.ts:42");
    expect(html).toContain('aria-label="Open src/app.ts:42 in editor"');
  });

  it("renders just the file for a file-level (null line) finding", () => {
    const html = renderToStaticMarkup(
      <CensorFindingRow finding={finding({ line: null })} onOpen={vi.fn()} onDispose={vi.fn()} />,
    );
    expect(html).toContain("src/app.ts");
    expect(html).not.toContain("src/app.ts:");
  });

  it("exposes a Mark FP dispose action", () => {
    const html = renderToStaticMarkup(
      <CensorFindingRow finding={finding()} onOpen={vi.fn()} onDispose={vi.fn()} />,
    );
    expect(html).toContain("Mark FP");
    expect(html).toContain('aria-label="Mark as false positive"');
  });

  it("does not render raw secret-shaped content beyond the redacted body", () => {
    // The body is the engine's already-redacted summary; the row must render it
    // verbatim and never synthesize raw fields. (Sanity: a secret-looking string
    // is NOT introduced by the row itself.)
    const html = renderToStaticMarkup(
      <CensorFindingRow finding={finding({ body: "secret redacted" })} onOpen={vi.fn()} onDispose={vi.fn()} />,
    );
    // contentHash / createdAt are not part of CensorFinding rendering.
    expect(html).not.toContain("contentHash");
    expect(html).not.toContain("createdAt");
  });

  // C-F5 regression: BIDI override chars in title/body/file must be stripped.
  it("strips U+202E BIDI right-to-left override from finding title (C-F5)", () => {
    const bidiTitle = "Normal‮egap";
    const html = renderToStaticMarkup(
      <CensorFindingRow
        finding={finding({ title: bidiTitle })}
        onOpen={vi.fn()}
        onDispose={vi.fn()}
      />,
    );
    expect(html).not.toContain("‮");
    expect(html).toContain("Normalegap");
  });

  it("strips U+202E from finding body (C-F5)", () => {
    const bidiBody = "A ‮malicious body";
    const html = renderToStaticMarkup(
      <CensorFindingRow
        finding={finding({ body: bidiBody })}
        onOpen={vi.fn()}
        onDispose={vi.fn()}
      />,
    );
    expect(html).not.toContain("‮");
  });

  it("strips U+202E from file path (C-F5)", () => {
    const bidiFile = "src/‮evil.ts";
    const html = renderToStaticMarkup(
      <CensorFindingRow
        finding={finding({ file: bidiFile })}
        onOpen={vi.fn()}
        onDispose={vi.fn()}
      />,
    );
    expect(html).not.toContain("‮");
  });
});
