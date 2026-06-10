// @vitest-environment jsdom
//
// Adversarial XSS corpus for the single sanitization chokepoint. This repo's
// default vitest env is `node`; this file opts into jsdom (DOMPurify needs a DOM)
// via the docblock above. Every payload must be neutralized; legitimate markup
// and the `data-node-id` marker must survive.

import { describe, it, expect } from "vitest";
import { sanitizeNodeMarkup } from "./sanitize";

function clean(html: string): string {
  return sanitizeNodeMarkup(html).toLowerCase();
}

describe("sanitizeNodeMarkup — neutralizes XSS", () => {
  it("strips <script> tags entirely", () => {
    const out = clean("<div>hi</div><script>alert(1)</script>");
    expect(out).not.toContain("<script");
    expect(out).not.toContain("alert(1)");
  });

  it("strips on* event handlers (img onerror)", () => {
    const out = clean('<img src="x" onerror="alert(1)">');
    expect(out).not.toContain("onerror");
    expect(out).not.toContain("alert(1)");
  });

  it("strips inline onclick handlers", () => {
    const out = clean('<button onclick="steal()">go</button>');
    expect(out).not.toContain("onclick");
    expect(out).not.toContain("steal()");
  });

  it("removes <script> nested inside <svg>", () => {
    const out = clean("<svg><script>alert(1)</script><rect/></svg>");
    expect(out).not.toContain("<script");
    expect(out).not.toContain("alert(1)");
  });

  it("removes <foreignObject> from SVG (HTML/script smuggling)", () => {
    const out = clean(
      "<svg><foreignObject><script>alert(1)</script></foreignObject></svg>",
    );
    expect(out).not.toContain("foreignobject");
    expect(out).not.toContain("<script");
  });

  it("neutralizes javascript: hrefs", () => {
    const out = clean('<a href="javascript:alert(1)">x</a>');
    expect(out).not.toContain("javascript:");
  });

  it("neutralizes data:text/html anchors (mutation-XSS vector)", () => {
    const out = clean(
      '<a href="data:text/html,<script>alert(1)</script>">x</a>',
    );
    expect(out).not.toContain("data:text/html");
    expect(out).not.toContain("<script");
  });

  it("strips <iframe>", () => {
    const out = clean('<iframe src="https://evil.example"></iframe>');
    expect(out).not.toContain("<iframe");
  });

  it("strips <style> (CSS exfil / injection)", () => {
    const out = clean("<style>body{background:url(https://evil/x)}</style><p>ok</p>");
    expect(out).not.toContain("<style");
    expect(out).not.toContain("evil");
  });

  it("strips <object> and <embed>", () => {
    const out = clean('<object data="x.swf"></object><embed src="x.swf">');
    expect(out).not.toContain("<object");
    expect(out).not.toContain("<embed");
  });

  it("strips <link> and <base> (document-head hijack)", () => {
    const out = clean('<link rel="stylesheet" href="https://evil/x.css"><base href="https://evil/">');
    expect(out).not.toContain("<link");
    expect(out).not.toContain("<base");
  });

  it("strips <meta> (meta-refresh redirect)", () => {
    const out = clean('<meta http-equiv="refresh" content="0;url=https://evil">');
    expect(out).not.toContain("<meta");
  });

  it("neutralizes a known mutation-XSS payload", () => {
    // Classic mXSS: a malformed comment/markup that re-parses into a script in
    // some sinks. After sanitize there must be no executable script element.
    const out = clean('<svg></p><style><a id="</style><img src=1 onerror=alert(1)>">');
    expect(out).not.toContain("onerror");
    expect(out).not.toContain("alert(1)");
    expect(out).not.toContain("<style");
  });

  it("strips svg use xlink:href javascript vectors", () => {
    const out = clean('<svg><use xlink:href="javascript:alert(1)"></use></svg>');
    expect(out).not.toContain("javascript:");
    expect(out).not.toContain("xlink:href=\"javascript");
  });

  it("strips data-* exfil attributes other than data-node-id", () => {
    const out = clean('<div data-secret="token" data-node-id="hero">x</div>');
    expect(out).not.toContain("data-secret");
    expect(out).toContain('data-node-id="hero"');
  });

  it("drops a malformed (non-charset) data-node-id value", () => {
    const out = clean('<div data-node-id="../escape">x</div>');
    expect(out).not.toContain("data-node-id");
    const out2 = clean('<div data-node-id="Has Spaces">x</div>');
    expect(out2).not.toContain("data-node-id");
  });

  it("neutralizes javascript: url() inside inline style", () => {
    const out = clean('<div style="background:url(javascript:alert(1))">x</div>');
    expect(out).not.toContain("javascript:");
    expect(out).not.toContain("alert(1)");
    expect(out).toContain("url(about:blank)");
  });

  it("neutralizes an external beacon url() inside inline style", () => {
    // Policy (W1): ONLY data:image/ and #fragment urls survive. An external
    // https beacon to a third-party host is an exfil vector we cannot distinguish
    // from a legit asset at this layer, so https is NO LONGER allowed either.
    const out = clean(
      '<div style="background:url(https://evil.example/beacon.png)">x</div>',
    );
    expect(out).toContain("url(about:blank)");
    expect(out).not.toContain("https://evil.example");
    const httpOut = clean('<div style="background:url(http://evil.example/x.png)">x</div>');
    expect(httpOut).toContain("url(about:blank)");
    expect(httpOut).not.toContain("http://evil.example");
    // Protocol-relative (//host) is also neutralized.
    const pr = clean('<div style="background:url(//evil.example/x.png)">x</div>');
    expect(pr).toContain("url(about:blank)");
  });

  it("keeps a data:image/ url() inside inline style (legit inline asset)", () => {
    const out = clean(
      '<div style="background:url(data:image/png;base64,iVBORw0kggg==)">x</div>',
    );
    expect(out).toContain("url(data:image/png;base64,ivborw0kggg==)");
    expect(out).not.toContain("about:blank");
  });

  it("B3: neutralizes a javascript: href on an SVG <a> element", () => {
    const out = clean('<svg><a href="javascript:alert(1)"><rect/></a></svg>');
    expect(out).not.toContain("javascript:");
    expect(out).not.toContain('href="javascript');
  });

  it("B3: neutralizes a javascript: href on an HTML <a> element", () => {
    const out = clean('<a href="javascript:alert(1)">x</a>');
    expect(out).not.toContain("javascript:");
    expect(out).not.toContain('href="javascript');
  });

  it("removes position:fixed / position:sticky overlay CSS", () => {
    const fixed = clean('<div style="position:fixed;inset:0;z-index:99999">x</div>');
    expect(fixed).not.toContain("position:fixed");
    expect(fixed).not.toContain("position: fixed");
    const sticky = clean('<div style="position:sticky;top:0">x</div>');
    expect(sticky).not.toContain("position:sticky");
  });

  it("W7: strips position:fixed/sticky even with an !important qualifier", () => {
    const fixed = clean(
      '<div style="position: fixed !important; inset:0; z-index:99999">x</div>',
    );
    expect(fixed).not.toContain("position: fixed");
    expect(fixed).not.toContain("position:fixed");
    // the !important declaration must not leave a dangling `position` anywhere
    expect(fixed).not.toMatch(/position\s*:\s*fixed/);
    const sticky = clean('<div style="position:sticky !important;top:0">x</div>');
    expect(sticky).not.toMatch(/position\s*:\s*sticky/);
  });

  it("W7: still allows position:absolute (neutralized by host containment, not stripped)", () => {
    const out = clean('<div style="position:absolute;top:-9999px;left:-9999px">x</div>');
    // absolute is NOT stripped at the sanitize layer — the host .node-card
    // (position:relative; overflow:hidden) clips it (B2). It must remain in the CSS.
    expect(out).toContain("position:absolute");
  });

  it("kills CSS expression() (legacy IE dynamic-property XSS)", () => {
    const out = clean('<div style="width:expression(alert(1))">x</div>');
    expect(out).not.toContain("expression(");
  });

  it("preserves legitimate inline layout CSS untouched", () => {
    const out = clean('<div style="display:flex;gap:8px;color:#333">x</div>');
    expect(out).toContain("display:flex");
    expect(out).toContain("gap:8px");
    expect(out).toContain("color:#333");
  });

  it("keeps a same-document fragment url() (e.g. svg clip-path)", () => {
    const out = clean('<div style="clip-path:url(#clip)">x</div>');
    expect(out).toContain("url(#clip)");
    expect(out).not.toContain("about:blank");
  });
});

describe("sanitizeNodeMarkup — direct-DOM isolation (class/id stripping)", () => {
  it("strips the class attribute from HTML elements (no app/Tailwind selector leak)", () => {
    const out = clean('<div class="card flex gap-2">Hello</div>');
    expect(out).not.toContain("class=");
    expect(out).not.toContain("card");
    expect(out).toContain("hello"); // content (lowercased by clean) survives
  });

  it("strips the id attribute from HTML elements (no global id clobbering)", () => {
    const out = clean('<section id="root"><h1>Hi</h1></section>');
    expect(out).not.toContain('id="root"');
    expect(out).not.toContain("id=");
    expect(out).toContain("<h1>");
  });

  it("strips class even when combined with a kept data-node-id", () => {
    const out = clean('<div class="card" data-node-id="hero">x</div>');
    expect(out).not.toContain('class="card"');
    // data-node-id is a data-* attr, NOT the `id` attr — it must survive intact.
    expect(out).toContain('data-node-id="hero"');
  });

  it("strips id even inside an SVG subtree (DECISION: SVG internal id refs unsupported)", () => {
    // DECISION (documented in sanitize.ts): `id` is stripped EVERYWHERE, including
    // SVG. DOMPurify's SANITIZE_NAMED_PROPS already rewrites `id` to
    // `user-content-…` without rewriting the matching `url(#…)` ref, so SVG
    // internal refs are already severed — keeping the id would buy nothing. The
    // SVG shapes themselves still render; only the id-referenced fill is dropped.
    const out = clean(
      '<svg viewBox="0 0 10 10"><defs><linearGradient id="g1"><stop offset="0%" stop-color="#fff"/></linearGradient></defs><rect width="10" height="10" fill="url(#g1)"/></svg>',
    );
    expect(out).not.toContain('id="g1"');
    expect(out).not.toContain("user-content-g1");
    expect(out).toContain("<svg"); // the SVG itself still renders
    expect(out).toContain("<rect");
  });

  it("M3: strips UPPERCASE CLASS/ID on SVG elements (case-sensitive attr names)", () => {
    // SVG attribute names are case-sensitive in the DOM, so CLASS/ID are distinct
    // from class/id; a case-sensitive hasAttribute check would let them survive.
    const out = clean('<svg><rect CLASS="x" ID="y" data-node-id="hero"/></svg>');
    expect(out).not.toContain("class=");
    expect(out).not.toContain('"x"');
    expect(out).not.toContain('"y"'); // the id VALUE is gone
    // no standalone `id=` attribute (data-node-id= legitimately contains the
    // substring "id=", so match a word boundary instead of a bare substring)
    expect(out).not.toMatch(/(?:^|[\s"])id=/);
    expect(out).not.toContain("user-content");
    // a legitimate data-node-id on the same element is untouched
    expect(out).toContain('data-node-id="hero"');
  });

  it("M3: strips lowercase class/id on SVG too (regression — unchanged behavior)", () => {
    const out = clean('<svg><rect class="a" id="b"/></svg>');
    expect(out).not.toContain("class=");
    expect(out).not.toContain("id=");
    expect(out).not.toContain("user-content");
  });
});

describe("sanitizeNodeMarkup — preserves legitimate markup", () => {
  it("keeps a normal div's text content (class is intentionally stripped)", () => {
    const out = sanitizeNodeMarkup('<div class="card">Hello</div>');
    expect(out).toContain("Hello");
    // class is removed for direct-DOM isolation; only the element + text remain.
    expect(out.toLowerCase()).not.toContain("class=");
  });

  it("KEEPS data-node-id (the placement marker)", () => {
    const out = sanitizeNodeMarkup('<section data-node-id="hero"><h1>Hi</h1></section>');
    expect(out.toLowerCase()).toContain('data-node-id="hero"');
    expect(out.toLowerCase()).toContain("<h1>");
  });

  it("keeps safe https links", () => {
    const out = sanitizeNodeMarkup('<a href="https://example.com">x</a>');
    expect(out.toLowerCase()).toContain("https://example.com");
  });

  it("keeps inline style attributes (used for intra-component layout)", () => {
    const out = sanitizeNodeMarkup('<div style="display:flex;gap:8px">x</div>');
    expect(out.toLowerCase()).toContain("display:flex");
  });

  it("keeps legitimate SVG shapes", () => {
    const out = sanitizeNodeMarkup('<svg viewBox="0 0 10 10"><rect width="10" height="10"/></svg>');
    expect(out.toLowerCase()).toContain("<svg");
    expect(out.toLowerCase()).toContain("<rect");
  });

  it("M2: drops a REMOTE href on an SVG <image> (exfil/beacon resource fetch)", () => {
    // A remote http(s) href on <image> passes the LINK policy but is a resource
    // beacon — it must be dropped. The <image> element itself may remain, but with
    // no remote href to fetch.
    const out = clean(
      '<svg><image href="https://evil.example/x.png" width="10" height="10"/></svg>',
    );
    expect(out).not.toContain("evil.example");
    expect(out).not.toContain("https://");
  });

  it("M2: keeps a data:image/ href on an SVG <image> (legit inline asset)", () => {
    const out = clean(
      '<svg><image href="data:image/png;base64,iVBORw0KGgg==" width="10" height="10"/></svg>',
    );
    expect(out).toContain("data:image/png");
  });
});

describe("sanitizeNodeMarkup — total / defensive", () => {
  it("returns '' for non-string input", () => {
    expect(sanitizeNodeMarkup(null)).toBe("");
    expect(sanitizeNodeMarkup(undefined)).toBe("");
    expect(sanitizeNodeMarkup(42 as unknown)).toBe("");
  });

  it("returns '' for an empty string", () => {
    expect(sanitizeNodeMarkup("")).toBe("");
  });
});
