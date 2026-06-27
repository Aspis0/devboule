// TDD — written BEFORE implementation (Phase 4). Tests the `autoResize` prop on ArtifactView.
// All rendering is server-side via `renderToStaticMarkup` (vitest node env — no real DOM).
// The security/trust/dispose logic is already covered by `artifactProtocol.test.ts`; we only
// assert the STATIC render differences that autoResize introduces.
import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { ArtifactView } from "./ArtifactView";

describe("ArtifactView — autoResize prop", () => {
  it("with autoResize=true (default) iframe has a pixel height from minHeight", () => {
    const html = renderToStaticMarkup(
      <ArtifactView artifactId="test-id" minHeight={180} autoResize={true} />,
    );
    // React serialises number CSS values as `<prop>:<n>px`
    expect(html).toContain("height:180px");
    // The wrapper div (first <div> tag) must NOT carry height:100% — that would mean
    // the fixed-frame spread leaked into auto-grow mode.  Extract just the opening tag
    // so we're not sensitive to CSS property ordering or later div attributes.
    const firstDivTag = html.match(/<div[^>]*>/)?.[0] ?? "";
    expect(firstDivTag).not.toContain("height:100%");
  });

  it("with no autoResize prop (default true) same pixel height behaviour", () => {
    const html = renderToStaticMarkup(
      <ArtifactView artifactId="test-id" minHeight={240} />,
    );
    expect(html).toContain("height:240px");
  });

  it("with autoResize=false iframe has height:100% (fill fixed device screen)", () => {
    const html = renderToStaticMarkup(
      <ArtifactView artifactId="test-id" autoResize={false} />,
    );
    // The iframe must fill the containing device screen slot
    expect(html).toContain("height:100%");
    // Wrapper div must also stretch to fill its parent
    expect(html).toContain("height:100%");
    // Must NOT contain a px height on the iframe (that would mean pixel sizing leaked in)
    // The default minHeight is 120; check it's absent from the iframe's style
    expect(html).not.toContain("height:120px");
  });

  it("autoResize=false still renders the sandbox attribute unchanged (security invariant)", () => {
    const html = renderToStaticMarkup(
      <ArtifactView artifactId="test-id" autoResize={false} />,
    );
    expect(html).toContain('sandbox="allow-scripts"');
    // must NOT have allow-same-origin (the opaque-origin invariant)
    expect(html).not.toContain("allow-same-origin");
  });

  it("autoResize=true still renders the sandbox attribute unchanged (security regression guard)", () => {
    const html = renderToStaticMarkup(
      <ArtifactView artifactId="test-id" autoResize={true} />,
    );
    expect(html).toContain('sandbox="allow-scripts"');
    expect(html).not.toContain("allow-same-origin");
  });

  it("error banner is absolutely positioned (overlay) when autoResize=false", () => {
    // Use defaultError to force the banner into the static render without a DOM/event loop.
    const html = renderToStaticMarkup(
      <ArtifactView
        artifactId="test-id"
        autoResize={false}
        defaultError="runtime crash"
      />,
    );
    // The banner must be present
    expect(html).toContain('role="alert"');
    expect(html).toContain("runtime crash");
    // Extract the opening tag of the role=alert element and assert it is absolutely positioned.
    const alertTag = html.match(/<div[^>]*role="alert"[^>]*>/)?.[0] ?? "";
    expect(alertTag).toContain("position:absolute");
    expect(alertTag).toContain("top:0");
    expect(alertTag).toContain("left:0");
    expect(alertTag).toContain("right:0");
    // zIndex must be present so the overlay is actually on top
    expect(alertTag).toContain("z-index:10");
  });

  it("error banner is absolutely positioned (overlay) when autoResize=true", () => {
    // Unified overlay: auto-grow mode also uses absolute positioning so the banner
    // does not contribute to the scroll height of the artifact.
    const html = renderToStaticMarkup(
      <ArtifactView
        artifactId="test-id"
        autoResize={true}
        defaultError="runtime crash"
      />,
    );
    const alertTag = html.match(/<div[^>]*role="alert"[^>]*>/)?.[0] ?? "";
    expect(alertTag).toContain("position:absolute");
  });
});
