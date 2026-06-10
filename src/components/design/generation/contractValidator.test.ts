// @vitest-environment jsdom
//
// Tier-1 contract validator + auto-fixer (Phase 2.5 STEP A). Uses the REAL jsdom
// DOMParser so foster-parenting and inline-style behavior match the runtime.

import { describe, it, expect } from "vitest";
import {
  validateNodeMarkup,
  autoFixNodeMarkup,
  type ViolationCode,
} from "./contractValidator";

function codes(violations: { code: ViolationCode }[]): ViolationCode[] {
  return violations.map((v) => v.code);
}

describe("validateNodeMarkup — clean markup passes", () => {
  it("a legit single-element node has no violations", () => {
    const r = validateNodeMarkup(
      '<section data-node-id="hero" style="display:flex;gap:8px;padding:16px;color:#111"><h1>Hi</h1></section>',
    );
    expect(r.violations).toEqual([]);
    expect(r.rootTag).toBe("section");
    expect(r.fosterParented).toBe(false);
  });

  it("an inner position:absolute does NOT trigger a root violation", () => {
    const r = validateNodeMarkup(
      '<div style="display:grid"><span style="position:absolute;top:0">badge</span></div>',
    );
    expect(codes(r.violations)).not.toContain("POSITIONAL_CSS_ON_ROOT");
    expect(r.violations).toEqual([]);
  });

  it("an svg root passes clean", () => {
    const r = validateNodeMarkup('<svg viewBox="0 0 10 10"><rect/></svg>');
    expect(r.violations).toEqual([]);
    expect(r.rootTag).toBe("svg");
  });
});

describe("validateNodeMarkup — positional CSS on root (Finding 8)", () => {
  const positionalCases: Array<[string, string]> = [
    ["position:absolute", '<div style="position:absolute;color:red">x</div>'],
    ["position:fixed", '<div style="position:fixed">x</div>'],
    ["position:sticky", '<div style="position:sticky">x</div>'],
    [
      "position:fixed !important",
      '<div style="position:fixed !important">x</div>',
    ],
    ["position:ABSOLUTE (case-insensitive)", '<div style="position:ABSOLUTE">x</div>'],
    ["top", '<div style="top:10px">x</div>'],
    ["left", '<div style="left:10px">x</div>'],
    ["right", '<div style="right:10px">x</div>'],
    ["bottom", '<div style="bottom:10px">x</div>'],
    ["float", '<div style="float:left">x</div>'],
    ["z-index", '<div style="z-index:99">x</div>'],
    ["inset", '<div style="inset:0">x</div>'],
    ["outer margin", '<div style="margin:8px">x</div>'],
    ["margin-top", '<div style="margin-top:8px">x</div>'],
    ["margin-inline-start", '<div style="margin-inline-start:8px">x</div>'],
  ];
  for (const [name, markup] of positionalCases) {
    it(`flags ${name} on the root`, () => {
      const r = validateNodeMarkup(markup);
      expect(codes(r.violations)).toContain("POSITIONAL_CSS_ON_ROOT");
    });
  }

  it("does NOT flag padding/gap (kept inner layout) on the root", () => {
    const r = validateNodeMarkup('<div style="padding:8px;gap:4px">x</div>');
    expect(codes(r.violations)).not.toContain("POSITIONAL_CSS_ON_ROOT");
  });

  // BLOCKER 1: position:relative/static on the ROOT is a legit positioning CONTEXT
  // for absolutely-positioned CHILDREN — it does NOT take the root out of flow, so
  // it must NOT be flagged (stripping it would make those children escape).
  it("does NOT flag position:relative on the root (positioning context)", () => {
    const r = validateNodeMarkup('<div style="position:relative;padding:8px">x</div>');
    expect(codes(r.violations)).not.toContain("POSITIONAL_CSS_ON_ROOT");
  });

  it("does NOT flag position:static on the root", () => {
    const r = validateNodeMarkup('<div style="position:static">x</div>');
    expect(codes(r.violations)).not.toContain("POSITIONAL_CSS_ON_ROOT");
  });
});

describe("validateNodeMarkup — foster-parented root (Finding 9)", () => {
  const fosterTags = [
    "tr",
    "td",
    "th",
    "thead",
    "tbody",
    "tfoot",
    "col",
    "colgroup",
    "caption",
    "option",
    "optgroup",
  ];
  for (const tag of fosterTags) {
    it(`detects <${tag}> as a foster-parented root`, () => {
      const r = validateNodeMarkup(`<${tag}>x</${tag}>`);
      expect(r.fosterParented).toBe(true);
      expect(codes(r.violations)).toContain("FOSTER_PARENTED_ROOT");
      expect(r.rootTag).toBeNull();
    });
  }

  it("a <table> wrapping a <tr> is NOT foster-parented (valid root)", () => {
    const r = validateNodeMarkup("<table><tr><td>x</td></tr></table>");
    expect(r.fosterParented).toBe(false);
    expect(r.rootTag).toBe("table");
  });
});

describe("validateNodeMarkup — empty / multiple / script", () => {
  it("flags EMPTY for an empty string", () => {
    expect(codes(validateNodeMarkup("").violations)).toEqual(["EMPTY"]);
  });
  it("flags EMPTY for whitespace", () => {
    expect(codes(validateNodeMarkup("   \n  ").violations)).toEqual(["EMPTY"]);
  });
  it("flags EMPTY for prose with no element", () => {
    expect(codes(validateNodeMarkup("just some text").violations)).toContain(
      "EMPTY",
    );
  });
  it("flags MULTIPLE_TOP_LEVEL for two siblings", () => {
    const r = validateNodeMarkup("<div>a</div><section>b</section>");
    expect(codes(r.violations)).toContain("MULTIPLE_TOP_LEVEL");
    expect(r.rootTag).toBe("div");
  });
  it("flags SCRIPT_OR_HANDLER for an inline <script>", () => {
    const r = validateNodeMarkup("<div><script>alert(1)</script></div>");
    expect(codes(r.violations)).toContain("SCRIPT_OR_HANDLER");
  });
  it("flags SCRIPT_OR_HANDLER for an on* handler", () => {
    const r = validateNodeMarkup('<img onerror="x()" src="y">');
    expect(codes(r.violations)).toContain("SCRIPT_OR_HANDLER");
  });
  it("flags SCRIPT_OR_HANDLER for a javascript: href", () => {
    const r = validateNodeMarkup('<a href="javascript:alert(1)">x</a>');
    expect(codes(r.violations)).toContain("SCRIPT_OR_HANDLER");
  });
});

describe("autoFixNodeMarkup — Finding 8 root strip (descendants preserved)", () => {
  it("strips position/top/left from the root, keeps other root CSS", () => {
    const r = autoFixNodeMarkup(
      '<div style="position:absolute;top:10px;left:5px;color:red;padding:8px">x</div>',
    );
    expect(codes(r.fixed)).toContain("POSITIONAL_CSS_ON_ROOT");
    expect(r.remaining).toEqual([]);
    expect(r.markup).not.toContain("position");
    expect(r.markup).not.toContain("top:");
    expect(r.markup).not.toContain("left:");
    expect(r.markup).toContain("color:red");
    expect(r.markup).toContain("padding:8px");
  });

  it("strips outer margin + float + z-index from the root", () => {
    const r = autoFixNodeMarkup(
      '<div style="margin:8px;float:left;z-index:9;display:flex">x</div>',
    );
    expect(r.markup).not.toMatch(/margin/);
    expect(r.markup).not.toContain("float");
    expect(r.markup).not.toContain("z-index");
    expect(r.markup).toContain("display:flex");
  });

  it("PRESERVES an inner element's position:absolute (root-only strip)", () => {
    const r = autoFixNodeMarkup(
      '<div style="position:absolute;display:grid"><span style="position:absolute;top:0">b</span></div>',
    );
    // Root position gone (a trailing ';' is appended on rewrite — harmless),
    // inner position intact.
    expect(r.markup).toContain('style="display:grid;"');
    expect(r.markup).not.toMatch(/<div[^>]*position/);
    expect(r.markup).toContain('<span style="position:absolute;top:0">');
  });

  it("removes the style attribute entirely when only positional props existed", () => {
    const r = autoFixNodeMarkup('<div style="position:absolute;top:0">x</div>');
    expect(r.markup).toBe("<div>x</div>");
  });

  it("leaves clean markup untouched (no fixes, no remaining)", () => {
    const clean = '<section data-node-id="hero" style="padding:8px"><h1>Hi</h1></section>';
    const r = autoFixNodeMarkup(clean);
    expect(r.fixed).toEqual([]);
    expect(r.remaining).toEqual([]);
    expect(r.markup).toBe(clean);
    expect(r.usable).toBe(true);
    expect(r.collapsedSiblings).toBe(false);
  });
});

describe("autoFixNodeMarkup — BLOCKER 1: position:relative/static kept on root", () => {
  it("KEEPS position:relative on the root and leaves an inner position:absolute untouched", () => {
    const r = autoFixNodeMarkup(
      '<div style="position:relative;padding:16px"><span style="position:absolute;top:0">badge</span></div>',
    );
    // No fix applied — relative is legit, the inner absolute child relies on it.
    expect(codes(r.fixed)).not.toContain("POSITIONAL_CSS_ON_ROOT");
    expect(r.markup).toContain("position:relative");
    expect(r.markup).toContain("padding:16px");
    expect(r.markup).toContain('<span style="position:absolute;top:0">badge</span>');
    expect(r.usable).toBe(true);
  });

  it("strips position:absolute;top:0 from the root (still dangerous)", () => {
    const r = autoFixNodeMarkup('<div style="position:absolute;top:0">x</div>');
    expect(codes(r.fixed)).toContain("POSITIONAL_CSS_ON_ROOT");
    expect(r.markup).toBe("<div>x</div>");
  });

  it("strips position:fixed !important from the root (tolerates !important)", () => {
    const r = autoFixNodeMarkup(
      '<div style="position:fixed !important;color:red">x</div>',
    );
    expect(codes(r.fixed)).toContain("POSITIONAL_CSS_ON_ROOT");
    expect(r.markup).not.toMatch(/position/);
    expect(r.markup).toContain("color:red");
  });
});

describe("autoFixNodeMarkup — BLOCKER 2: url/quote-aware declaration parsing", () => {
  it("PRESERVES a base64 data-URL background while stripping position:absolute", () => {
    const r = autoFixNodeMarkup(
      '<div style="background:url(data:image/png;base64,AAA);padding:16px;position:absolute">x</div>',
    );
    expect(codes(r.fixed)).toContain("POSITIONAL_CSS_ON_ROOT");
    // The data-URL (incl. the ;base64 segment) survives intact.
    expect(r.markup).toContain("background:url(data:image/png;base64,AAA)");
    expect(r.markup).toContain("padding:16px");
    // The dangerous position is gone.
    expect(r.markup).not.toMatch(/position\s*:/);
  });

  it('preserves a quoted value containing a semicolon (content:";") through a re-serialize', () => {
    // HTML attribute uses &quot; so the inner double-quotes survive into the style
    // value as `content:";"` (a quoted value whose `;` must NOT split the decl). A
    // position:absolute forces a STRIP -> re-serialize, proving the quoted `;` is
    // not treated as a declaration boundary (it would otherwise vanish/corrupt).
    const r = autoFixNodeMarkup(
      '<div style="content:&quot;;&quot;;padding:8px;position:absolute">x</div>',
    );
    expect(codes(r.fixed)).toContain("POSITIONAL_CSS_ON_ROOT");
    expect(r.markup).toContain("padding:8px");
    // The quoted content survives as a single declaration (serialized with the
    // double-quotes re-encoded as &quot; by the DOM serializer).
    expect(r.markup).toMatch(/content:&quot;;&quot;/);
    expect(r.markup).not.toMatch(/position\s*:/);
    expect(r.usable).toBe(true);
  });

  it("leaves an un-parseable style (unterminated url) UNTOUCHED rather than corrupting it", () => {
    const broken = '<div style="background:url(data:image/png;base64,AAA">x</div>';
    const r = autoFixNodeMarkup(broken);
    // Conservative fallback: no positional fix claimed, style preserved verbatim.
    expect(codes(r.fixed)).not.toContain("POSITIONAL_CSS_ON_ROOT");
    expect(r.markup).toContain("background:url(data:image/png;base64,AAA");
  });
});

describe("autoFixNodeMarkup — usable / collapsedSiblings signals", () => {
  it("marks a foster-parented root as NOT usable", () => {
    const r = autoFixNodeMarkup("<tr><td>x</td></tr>");
    expect(r.usable).toBe(false);
    expect(codes(r.remaining)).toEqual(["FOSTER_PARENTED_ROOT"]);
  });

  it("marks an empty root as NOT usable", () => {
    expect(autoFixNodeMarkup("").usable).toBe(false);
    expect(autoFixNodeMarkup("no elements").usable).toBe(false);
  });

  it("sets collapsedSiblings when multiple top-level elements are present", () => {
    const r = autoFixNodeMarkup("<div>a</div><section>b</section>");
    expect(r.usable).toBe(true);
    expect(r.collapsedSiblings).toBe(true);
    expect(r.markup).toBe("<div>a</div>");
  });
});

describe("autoFixNodeMarkup — multiple top-level collapse", () => {
  it("keeps the first element and reports MULTIPLE_TOP_LEVEL fixed", () => {
    const r = autoFixNodeMarkup("<div>first</div><section>second</section>");
    expect(codes(r.fixed)).toContain("MULTIPLE_TOP_LEVEL");
    expect(r.remaining).toEqual([]);
    expect(r.markup).toBe("<div>first</div>");
  });
});

describe("autoFixNodeMarkup — unfixable (Finding 9 + empty)", () => {
  it("marks a <tr> root as remaining FOSTER_PARENTED_ROOT, markup unchanged", () => {
    const r = autoFixNodeMarkup("<tr><td>x</td></tr>");
    expect(r.fixed).toEqual([]);
    expect(codes(r.remaining)).toEqual(["FOSTER_PARENTED_ROOT"]);
    expect(r.markup).toBe("<tr><td>x</td></tr>");
  });

  it("marks an empty string as remaining EMPTY", () => {
    const r = autoFixNodeMarkup("");
    expect(codes(r.remaining)).toEqual(["EMPTY"]);
  });

  it("marks prose-only as remaining EMPTY", () => {
    const r = autoFixNodeMarkup("no elements here");
    expect(codes(r.remaining)).toEqual(["EMPTY"]);
  });
});

describe("autoFixNodeMarkup — combined / determinism", () => {
  it("strips root positional AND collapses multiple in one pass", () => {
    const r = autoFixNodeMarkup(
      '<div style="position:absolute;color:red">a</div><div>b</div>',
    );
    const fixedCodes = codes(r.fixed);
    expect(fixedCodes).toContain("MULTIPLE_TOP_LEVEL");
    expect(fixedCodes).toContain("POSITIONAL_CSS_ON_ROOT");
    expect(r.markup).toBe('<div style="color:red;">a</div>');
  });

  it("is deterministic — identical input yields identical output", () => {
    const input =
      '<div style="position:absolute;top:1px;margin:2px;color:#fff">x</div>';
    expect(autoFixNodeMarkup(input)).toEqual(autoFixNodeMarkup(input));
  });
});
