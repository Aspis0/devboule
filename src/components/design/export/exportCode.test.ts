import { describe, it, expect } from "vitest";
import { exportCode } from "./exportCode";
import type { DesignProject } from "../../../types/design";

function project(): DesignProject {
  return {
    meta: {
      schemaVersion: 1,
      id: "p1",
      name: "Landing",
      createdAt: "1970-01-01T00:00:00Z",
      updatedAt: "1970-01-01T00:00:00Z",
      canvas: { w: 1440, h: 1024, grid: 8 },
      nodeOrder: ["hero", "cta"],
    },
    manifest: {
      schemaVersion: 1,
      nodes: {
        hero: { x: 80, y: 80, z: 1, w: 420, h: "auto", kind: "html" },
        cta: { x: 80, y: 260, z: 2, w: 160, h: 48, kind: "html" },
      },
    },
    components: {
      hero: '<section data-node-id="hero"><h1>Hi</h1></section>',
      cta: '<button data-node-id="cta">Go</button>',
    },
  };
}

describe("exportCode — absolute mode", () => {
  it("places each node at its manifest rect", () => {
    const html = exportCode(project(), "absolute");
    expect(html).toContain("position:absolute");
    expect(html).toContain("left:80px");
    expect(html).toContain("top:80px");
    expect(html).toContain("z-index:1");
    expect(html).toContain("width:420px");
    // numeric height for cta, auto for hero
    expect(html).toContain("height:auto");
    expect(html).toContain("height:48px");
  });

  it("sizes the canvas container to the project canvas", () => {
    const html = exportCode(project(), "absolute");
    expect(html).toContain("position:relative");
    expect(html).toContain("width:1440px");
    expect(html).toContain("height:1024px");
  });

  it("produces a standalone document", () => {
    const html = exportCode(project(), "absolute");
    expect(html.startsWith("<!doctype html>")).toBe(true);
    expect(html).toContain("<title>Landing</title>");
    expect(html.trimEnd().endsWith("</html>")).toBe(true);
  });

  it("matches the absolute snapshot", () => {
    expect(exportCode(project(), "absolute")).toMatchSnapshot();
  });
});

describe("exportCode — flow mode", () => {
  it("respects nodeOrder and drops positioning", () => {
    const html = exportCode(project(), "flow");
    expect(html).toContain("flex-direction:column");
    expect(html).not.toContain("position:absolute");
    // hero appears before cta (nodeOrder)
    expect(html.indexOf("data-node-id=\"hero\"")).toBeLessThan(
      html.indexOf("data-node-id=\"cta\""),
    );
    // width still preserved
    expect(html).toContain("width:420px");
  });

  it("matches the flow snapshot", () => {
    expect(exportCode(project(), "flow")).toMatchSnapshot();
  });
});

describe("exportCode — edge cases", () => {
  it("skips a manifest node with no stored markup", () => {
    const p = project();
    delete p.components.cta;
    const html = exportCode(p, "absolute");
    expect(html).toContain("data-node-id=\"hero\"");
    expect(html).not.toContain("data-node-id=\"cta\"");
  });

  it("appends manifest-only ids missing from nodeOrder deterministically", () => {
    const p = project();
    p.meta.nodeOrder = ["hero"]; // cta only in manifest
    const html = exportCode(p, "flow");
    expect(html).toContain("data-node-id=\"cta\"");
    expect(html.indexOf("hero")).toBeLessThan(html.indexOf("cta"));
  });

  it("does not introduce unsanitized content — inlines stored markup verbatim", () => {
    const p = project();
    // Stored markup is already sanitized upstream; the exporter must not add a
    // <script> or alter the markup. Use benign markup and assert it is verbatim.
    p.components.hero = '<section data-node-id="hero"><p>safe &amp; sound</p></section>';
    const html = exportCode(p, "absolute");
    expect(html).toContain('<p>safe &amp; sound</p>');
    // No script tag introduced anywhere.
    expect(html).not.toContain("<script");
  });

  it("escapes a malicious project title in the attribute/text context", () => {
    const p = project();
    p.meta.name = '"><script>alert(1)</script>';
    const html = exportCode(p, "flow");
    expect(html).not.toContain("<script>alert(1)</script>");
    expect(html).toContain("&lt;script&gt;");
  });
});
