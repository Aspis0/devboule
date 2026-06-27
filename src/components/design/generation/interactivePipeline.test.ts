// PURE tests for the interactive-artifact pipeline: extract -> wrap-if-fragment ->
// neutralize remote refs. NO DOMPurify, NO bridge, NO CSP meta in the output.

import { describe, it, expect } from "vitest";
import {
  applyInteractiveGeneration,
  ARTIFACT_CDN_ALLOWLIST,
} from "./interactivePipeline";

describe("ARTIFACT_CDN_ALLOWLIST — parity with the Rust CSP allowlist", () => {
  it("equals exactly the three expected CDN origins (drift guard)", () => {
    // KEEP IN SYNC with src-tauri/src/backend/artifact_protocol.rs's ARTIFACT_CDN_ALLOWLIST
    // (and its Rust twin test). Pinning to the EXACT set means any intentional change trips
    // this test, forcing a conscious update here AND a mirrored edit on the Rust side.
    expect([...ARTIFACT_CDN_ALLOWLIST]).toEqual([
      "https://cdnjs.cloudflare.com",
      "https://cdn.jsdelivr.net",
      "https://unpkg.com",
    ]);
  });
});

describe("applyInteractiveGeneration — wrapping", () => {
  it("wraps a bare fragment in a responsive HTML shell", () => {
    const r = applyInteractiveGeneration("<button>hi</button>");
    expect(r.wrapped).toBe(true);
    expect(r.html).toContain("<!DOCTYPE html>");
    expect(r.html).toContain("<html");
    expect(r.html).toContain(
      'name="viewport" content="width=device-width, initial-scale=1"',
    );
    expect(r.html).toContain("<button>hi</button>");
  });

  it("passes a full document through without double-wrapping", () => {
    const doc =
      '<!DOCTYPE html><html lang="en"><head><title>x</title></head><body><h1>Hi</h1></body></html>';
    const r = applyInteractiveGeneration(doc);
    expect(r.wrapped).toBe(false);
    // exactly one <html occurrence (not nested).
    expect(r.html.match(/<html/gi)?.length).toBe(1);
    expect(r.html).toBe(doc); // no remote refs => byte-identical
  });

  it("treats <HTML> (uppercase) and <html attr> as a full document", () => {
    expect(applyInteractiveGeneration("<HTML><body>x</body></HTML>").wrapped).toBe(false);
    expect(
      applyInteractiveGeneration('<html data-theme="dark"><body>x</body></html>').wrapped,
    ).toBe(false);
  });

  it("strips code fences via extractMarkup before wrapping", () => {
    const r = applyInteractiveGeneration("```html\n<div>x</div>\n```");
    expect(r.html).not.toContain("```");
    expect(r.html).toContain("<div>x</div>");
  });
});

describe("applyInteractiveGeneration — remote reference neutralization", () => {
  it("neutralizes a remote <img src> to data:,", () => {
    const r = applyInteractiveGeneration('<img src="https://evil.example/x.png">');
    expect(r.html).not.toContain("evil.example");
    expect(r.html).toContain('src="data:,"');
    expect(r.neutralizedCount).toBe(1);
    expect(r.warnings.some((w) => w.includes("Neutralized 1 remote"))).toBe(true);
  });

  it("keeps an inline data: URI image", () => {
    const r = applyInteractiveGeneration('<img src="data:image/png;base64,AAAA">');
    expect(r.html).toContain("data:image/png;base64,AAAA");
    expect(r.neutralizedCount).toBe(0);
  });

  it("keeps each allowlisted CDN <script src>", () => {
    for (const cdn of ARTIFACT_CDN_ALLOWLIST) {
      const r = applyInteractiveGeneration(`<script src="${cdn}/lib/x.js"></script>`);
      expect(r.html).toContain(`${cdn}/lib/x.js`);
      expect(r.neutralizedCount).toBe(0);
    }
  });

  it("neutralizes a look-alike host that only PREFIX-matches a CDN", () => {
    const r = applyInteractiveGeneration(
      '<script src="https://cdnjs.cloudflare.com.evil.example/x.js"></script>',
    );
    expect(r.html).not.toContain("evil.example");
    expect(r.neutralizedCount).toBe(1);
  });

  it("neutralizes a CSS url() but keeps a data: url()", () => {
    const remote = applyInteractiveGeneration(
      "<div style=\"background:url(https://evil.example/bg.png)\">x</div>",
    );
    expect(remote.html).not.toContain("evil.example");
    expect(remote.html).toContain("url(data:,)");
    expect(remote.neutralizedCount).toBe(1);

    const inline = applyInteractiveGeneration(
      "<div style=\"background:url(data:image/png;base64,AAAA)\">x</div>",
    );
    expect(inline.html).toContain("url(data:image/png;base64,AAAA)");
    expect(inline.neutralizedCount).toBe(0);
  });

  it("keeps relative, fragment, and query hrefs", () => {
    const r = applyInteractiveGeneration(
      '<a href="#section">a</a><a href="page.html">b</a><a href="?q=1">c</a>',
    );
    expect(r.html).toContain('href="#section"');
    expect(r.html).toContain('href="page.html"');
    expect(r.html).toContain('href="?q=1"');
    expect(r.neutralizedCount).toBe(0);
  });

  it("neutralizes a protocol-relative URL", () => {
    const r = applyInteractiveGeneration('<script src="//evil.example/x.js"></script>');
    expect(r.html).not.toContain("evil.example");
    expect(r.neutralizedCount).toBe(1);
  });

  it("handles single-quoted and unquoted remote src", () => {
    const sq = applyInteractiveGeneration("<img src='https://evil.example/a.png'>");
    expect(sq.html).toContain("src='data:,'");
    expect(sq.neutralizedCount).toBe(1);

    const uq = applyInteractiveGeneration("<img src=https://evil.example/a.png alt=x>");
    expect(uq.html).not.toContain("evil.example");
    expect(uq.html).toContain("src=data:,");
    expect(uq.html).toContain("alt=x"); // the next attribute is untouched
    expect(uq.neutralizedCount).toBe(1);
  });

  it("does not match srcset (CSP-covered, not a src= attribute)", () => {
    const r = applyInteractiveGeneration(
      '<img srcset="https://evil.example/a.png 1x" src="data:,">',
    );
    // srcset is left as-is by this pass (the served img-src data: CSP blocks it).
    expect(r.html).toContain("srcset=");
  });
});

describe("applyInteractiveGeneration — empty/garbage input", () => {
  it("produces a valid empty shell + warning for empty text", () => {
    const r = applyInteractiveGeneration("");
    expect(r.wrapped).toBe(true);
    expect(r.html).toContain("<!DOCTYPE html>");
    expect(r.warnings.some((w) => w.includes("no usable markup"))).toBe(true);
  });

  it("does not throw on a non-string input", () => {
    const r = applyInteractiveGeneration(null);
    expect(r.wrapped).toBe(true);
    expect(r.html).toContain("<!DOCTYPE html>");
    expect(r.warnings.some((w) => w.includes("no usable markup"))).toBe(true);
  });
});

describe("applyInteractiveGeneration — boundary invariants", () => {
  it("never injects a CSP meta or a bridge marker (the serve handler owns both)", () => {
    const r = applyInteractiveGeneration(
      '<!DOCTYPE html><html><body><script>console.log(1)</script></body></html>',
    );
    expect(r.html).not.toContain("Content-Security-Policy");
    expect(r.html).not.toContain("__artifact_bridge");
  });

  it("escapes the shell lang attribute defensively", () => {
    const r = applyInteractiveGeneration("<p>x</p>", { lang: 'en"><script>bad' });
    expect(r.html).not.toContain('lang="en"><script>bad');
    expect(r.html).toContain("&quot;");
  });
});
