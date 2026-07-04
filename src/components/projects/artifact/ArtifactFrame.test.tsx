// TDD — written BEFORE implementation (Phase 4). Tests the `ArtifactFrame` presentational
// wrapper. All rendering via `renderToStaticMarkup` (vitest node env — no real DOM).
import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { ArtifactFrame } from "./ArtifactFrame";

// A minimal sentinel child so we can verify children are always rendered.
const CHILD_SENTINEL = <span data-testid="child-content">artifact-here</span>;
const CHILD_TEXT = "artifact-here";

describe("ArtifactFrame — bezel markup per kind", () => {
  it("android renders the Google Pixel 6 Pro bezel class", () => {
    const html = renderToStaticMarkup(
      <ArtifactFrame kind="android">{CHILD_SENTINEL}</ArtifactFrame>,
    );
    expect(html).toContain("device-google-pixel-6-pro");
    expect(html).toContain("device-frame");
    expect(html).toContain("device-screen");
    // children are placed inside the screen slot
    expect(html).toContain(CHILD_TEXT);
    // must NOT contain iOS class
    expect(html).not.toContain("device-iphone-14-pro");
  });

  it("ios renders the iPhone 14 Pro bezel class", () => {
    const html = renderToStaticMarkup(
      <ArtifactFrame kind="ios">{CHILD_SENTINEL}</ArtifactFrame>,
    );
    expect(html).toContain("device-iphone-14-pro");
    expect(html).toContain("device-frame");
    expect(html).toContain("device-screen");
    expect(html).toContain(CHILD_TEXT);
    // must NOT contain Android class
    expect(html).not.toContain("device-google-pixel-6-pro");
  });

  it("web renders the browser chrome wrapper class", () => {
    const html = renderToStaticMarkup(
      <ArtifactFrame kind="web">{CHILD_SENTINEL}</ArtifactFrame>,
    );
    expect(html).toContain("app-frame");
    expect(html).toContain(CHILD_TEXT);
    // must NOT contain phone device classes
    expect(html).not.toContain("device-iphone-14-pro");
    expect(html).not.toContain("device-google-pixel-6-pro");
  });

  it("component renders children bare — no bezel, no chrome class", () => {
    const html = renderToStaticMarkup(
      <ArtifactFrame kind="component">{CHILD_SENTINEL}</ArtifactFrame>,
    );
    expect(html).toContain(CHILD_TEXT);
    // must NOT contain any device or chrome class
    expect(html).not.toContain("device-iphone-14-pro");
    expect(html).not.toContain("device-google-pixel-6-pro");
    expect(html).not.toContain("app-frame");
    expect(html).not.toContain("device-frame");
  });

  it("all kinds render children (children never dropped)", () => {
    for (const kind of ["android", "ios", "web", "component"] as const) {
      const html = renderToStaticMarkup(
        <ArtifactFrame kind={kind}>{CHILD_SENTINEL}</ArtifactFrame>,
      );
      expect(html, `kind=${kind} must render children`).toContain(CHILD_TEXT);
    }
  });
});

describe("ArtifactFrame — viewport prop (SSR scale is 1.0 — containerWidth=0 before measurement)", () => {
  it("renders without crashing for each viewport value", () => {
    for (const viewport of ["mobile", "tablet", "desktop"] as const) {
      expect(() =>
        renderToStaticMarkup(
          <ArtifactFrame kind="android" viewport={viewport}>{CHILD_SENTINEL}</ArtifactFrame>,
        ),
      ).not.toThrow();
    }
  });

  it("android with mobile viewport still renders Pixel bezel class", () => {
    const html = renderToStaticMarkup(
      <ArtifactFrame kind="android" viewport="mobile">{CHILD_SENTINEL}</ArtifactFrame>,
    );
    expect(html).toContain("device-google-pixel-6-pro");
  });
});
