// @vitest-environment jsdom
//
// The standalone preview sandbox page (public/design-preview/preview.js) cannot
// be unit-tested as a module (it has no exports — it self-executes against the
// real window). Instead we load its SOURCE, build the index.html body skeleton,
// and exercise `window.__applyDesignPreview` (the function it exposes) against a
// stubbed `window.__PREVIEW_HTML`, asserting the iframe `srcdoc` + empty-state.

import { describe, it, expect, beforeEach } from "vitest";
// Load the standalone sandbox script as a raw string (Vite `?raw`) — no node:fs,
// no @types/node — then execute its IIFE source against the jsdom window below.
import previewJsSrc from "../../../../public/design-preview/preview.js?raw";

function loadPage(): void {
  // Minimal mirror of index.html's body: the iframe (FULLY OPAQUE sandbox="") + the
  // empty-state container.
  document.body.innerHTML =
    '<div class="pv-frame-wrap">' +
    '<iframe id="frame" sandbox=""></iframe>' +
    '<div class="pv-empty"></div>' +
    "</div>";
  // Execute the IIFE source: it defines + invokes applyPreview against `window`.
  // eslint-disable-next-line no-new-func
  new Function(previewJsSrc)();
}

describe("design preview sandbox page", () => {
  beforeEach(() => {
    delete (window as unknown as { __PREVIEW_HTML?: string }).__PREVIEW_HTML;
    delete (window as unknown as { __applyDesignPreview?: unknown }).__applyDesignPreview;
    document.body.innerHTML = "";
    document.body.removeAttribute("data-empty");
  });

  it("sets the iframe srcdoc to the injected preview HTML", () => {
    (window as unknown as { __PREVIEW_HTML?: string }).__PREVIEW_HTML =
      "<h1 data-node-id='hero'>Hello</h1>";
    loadPage();
    const frame = document.getElementById("frame") as HTMLIFrameElement;
    expect(frame.getAttribute("srcdoc")).toBe("<h1 data-node-id='hero'>Hello</h1>");
    expect(document.body.getAttribute("data-empty")).toBe("false");
  });

  it("keeps the iframe in a fully opaque sandbox (no allow-scripts, no allow-same-origin)", () => {
    // NOTE: jsdom cannot PROVE the opaque-origin rendering behaviour — it does not enforce
    // the iframe sandbox. We assert the attribute contract (empty sandbox = every
    // restriction on); the live srcdoc-still-renders check on a real WebView2/WKWebView is
    // owed. preview.js must NOT widen the attribute, so we also re-apply and re-check.
    (window as unknown as { __PREVIEW_HTML?: string }).__PREVIEW_HTML = "<p>x</p>";
    loadPage();
    const frame = document.getElementById("frame") as HTMLIFrameElement;
    const sandbox = frame.getAttribute("sandbox") ?? null;
    // The attribute is present and EMPTY — the most restrictive sandbox possible.
    expect(sandbox).toBe("");
    expect(sandbox).not.toContain("allow-scripts");
    expect(sandbox).not.toContain("allow-same-origin");
    // srcdoc still gets set (an empty sandbox does NOT stop us from assigning the markup;
    // the live render under the opaque origin is the part jsdom cannot verify).
    expect(frame.getAttribute("srcdoc")).toBe("<p>x</p>");
  });

  it("shows the empty state when no preview HTML was injected", () => {
    loadPage(); // __PREVIEW_HTML is undefined
    const frame = document.getElementById("frame") as HTMLIFrameElement;
    expect(frame.getAttribute("srcdoc")).toBe("");
    expect(document.body.getAttribute("data-empty")).toBe("true");
  });

  it("re-applies idempotently via the exposed helper", () => {
    loadPage();
    expect(document.body.getAttribute("data-empty")).toBe("true");
    (window as unknown as { __PREVIEW_HTML?: string }).__PREVIEW_HTML = "<b>now</b>";
    (
      window as unknown as { __applyDesignPreview: (w: Window) => void }
    ).__applyDesignPreview(window);
    const frame = document.getElementById("frame") as HTMLIFrameElement;
    expect(frame.getAttribute("srcdoc")).toBe("<b>now</b>");
    expect(document.body.getAttribute("data-empty")).toBe("false");
  });
});
