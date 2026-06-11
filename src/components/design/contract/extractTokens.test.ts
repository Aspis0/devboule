// PURE tests for token extraction from Oracle chunk text.

import { describe, it, expect } from "vitest";
import { extractTokensFromChunks } from "./extractTokens";
import { validateTokensDoc } from "../engine/tokens";
import { colorSwatches } from "../shell/format";
import type { DesignContextChunk } from "../generation/grounding";

function chunk(text: string, fileSource = "f", score = 0.9): DesignContextChunk {
  return { fileSource, score, text };
}

// --- fixtures ----------------------------------------------------------------------

const TOKENS_CSS = `
:root {
  --brand: #4f46e5;
  --brand-strong: #4338ca;
  --ink: #0f172a;
  --space-md: 16px;
  --space-lg: 24px;
  --radius-card: 8px;
  --font-sans: "Inter", system-ui, sans-serif;
}
.btn { color: var(--brand); background: #4f46e5; border-radius: 8px; }
.card { background: rgba(15, 23, 42, 0.04); font-family: Inter, sans-serif; }
`;

const TAILWIND_CONFIG = `
module.exports = {
  theme: {
    extend: {
      colors: {
        brand: '#0ea5e9',
        accent: "#f59e0b",
        ink: '#111827',
      },
      fontFamily: {
        sans: ['Inter', 'system-ui'],
        mono: ["JetBrains Mono", "monospace"],
      },
      borderRadius: { card: '12px' },
    },
  },
};
`;

const STYLED_COMPONENTS = `
export const Button = styled.button\`
  color: #ffffff;
  background: #6750a4;
  border-radius: 16px;
  font-family: 'Roboto', sans-serif;
  padding: 12px 20px;
\`;
export const theme = { primary: '#6750a4', text: '#1c1b1f' };
`;

// A binary-ish / garbage chunk with hex that is NOT a design color (a sha and an id).
const GARBAGE = `
commit a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0
<svg id="icon-0f172a" viewBox="0 0 24 24"><path d="M3 3h18v18"/></svg>
const HASH = "deadbeefcafebabe1234567890abcdef";
some prose with the word color but no values at all
`;

// --- tests -------------------------------------------------------------------------

describe("extractTokensFromChunks — tokens.css", () => {
  const doc = extractTokensFromChunks([chunk(TOKENS_CSS, "src/tokens.css")]);

  it("produces a valid DTCG document", () => {
    expect(validateTokensDoc(doc)).toEqual([]);
  });

  it("names colors from CSS custom props", () => {
    const color = doc.color as Record<string, { $value: string }>;
    expect(color.brand.$value).toBe("#4f46e5");
    expect(color["brand-strong"].$value).toBe("#4338ca");
    expect(color.ink.$value).toBe("#0f172a");
  });

  it("captures spacing, radius, and font tokens", () => {
    const spacing = doc.spacing as Record<string, { $value: string }>;
    expect(Object.values(spacing).map((t) => t.$value)).toContain("16px");
    expect(Object.values(spacing).map((t) => t.$value)).toContain("24px");
    const radius = doc.radius as Record<string, { $value: string }>;
    expect(Object.values(radius).map((t) => t.$value)).toContain("8px");
    const typo = doc.typography as Record<string, { $value: string }>;
    expect(Object.values(typo).map((t) => t.$value).join(" ")).toContain("Inter");
  });

  it("color swatches resolve", () => {
    expect(colorSwatches(doc, 3).length).toBeGreaterThan(0);
  });
});

describe("extractTokensFromChunks — tailwind.config.js", () => {
  const doc = extractTokensFromChunks([chunk(TAILWIND_CONFIG, "tailwind.config.js")]);

  it("captures tailwind theme colors with their keys", () => {
    const color = doc.color as Record<string, { $value: string }>;
    const byValue = Object.fromEntries(
      Object.entries(color).map(([k, v]) => [v.$value, k]),
    );
    expect(byValue["#0ea5e9"]).toBe("brand");
    expect(byValue["#f59e0b"]).toBe("accent");
    expect(byValue["#111827"]).toBe("ink");
  });

  it("captures tailwind fontFamily entries and borderRadius", () => {
    const typo = doc.typography as Record<string, { $value: string }>;
    const fams = Object.values(typo).map((t) => t.$value);
    expect(fams).toContain("Inter");
    const radius = doc.radius as Record<string, { $value: string }>;
    expect(Object.values(radius).map((t) => t.$value)).toContain("12px");
  });
});

describe("extractTokensFromChunks — styled-components", () => {
  const doc = extractTokensFromChunks([chunk(STYLED_COMPONENTS, "Button.tsx")]);

  it("extracts colors, radius, and font from a template literal", () => {
    const colorValues = Object.values(
      doc.color as Record<string, { $value: string }>,
    ).map((t) => t.$value);
    expect(colorValues).toContain("#ffffff");
    expect(colorValues).toContain("#6750a4");
    expect(colorValues).toContain("#1c1b1f");
    const radius = doc.radius as Record<string, { $value: string }>;
    expect(Object.values(radius).map((t) => t.$value)).toContain("16px");
    const typo = doc.typography as Record<string, { $value: string }>;
    expect(Object.values(typo).map((t) => t.$value).join(" ")).toContain("Roboto");
  });
});

describe("extractTokensFromChunks — garbage / binary-ish", () => {
  it("returns an empty document when nothing is design-like", () => {
    const doc = extractTokensFromChunks([chunk(GARBAGE, "blob.bin")]);
    expect(doc).toEqual({});
  });

  it("returns empty for empty input", () => {
    expect(extractTokensFromChunks([])).toEqual({});
    expect(extractTokensFromChunks([chunk("")])).toEqual({});
  });
});

describe("extractTokensFromChunks — dedupe / ordering / cap", () => {
  it("dedupes by value (and collapses #fff / #ffffff)", () => {
    const doc = extractTokensFromChunks([
      chunk("a { color: #fff; } b { color: #ffffff; } c { background: #FFFFFF; }"),
    ]);
    const colors = doc.color as Record<string, { $value: string }>;
    const values = Object.values(colors).map((t) => t.$value);
    // All three collapse to a single #ffffff token.
    expect(values.filter((v) => v === "#ffffff").length).toBe(1);
  });

  it("orders colors by frequency desc then first-seen", () => {
    // #aa0000 appears 3x, #00bb00 2x, #0000cc 1x -> that order regardless of position.
    const text = `
      .x { color: #0000cc; }
      .a { color: #aa0000; } .a2 { background: #aa0000; } .a3 { border-color: #aa0000; }
      .b { color: #00bb00; } .b2 { background: #00bb00; }
    `;
    const doc = extractTokensFromChunks([chunk(text)]);
    const order = Object.values(
      doc.color as Record<string, { $value: string }>,
    ).map((t) => t.$value);
    expect(order[0]).toBe("#aa0000");
    expect(order[1]).toBe("#00bb00");
    expect(order[2]).toBe("#0000cc");
  });

  it("caps color count", () => {
    const many = Array.from({ length: 60 }, (_, i) => {
      const hex = (0x100000 + i * 0x010101).toString(16).padStart(6, "0");
      return `.c${i} { color: #${hex}; }`;
    }).join("\n");
    const doc = extractTokensFromChunks([chunk(many)]);
    const count = Object.keys(doc.color as Record<string, unknown>).length;
    expect(count).toBeLessThanOrEqual(24);
  });

  it("is deterministic for the same input", () => {
    const a = extractTokensFromChunks([chunk(TOKENS_CSS)]);
    const b = extractTokensFromChunks([chunk(TOKENS_CSS)]);
    expect(JSON.stringify(a)).toBe(JSON.stringify(b));
  });
});

describe("extractTokensFromChunks — name + line caps", () => {
  it("caps a pathological 200-char CSS var name at 64 chars (Fix 6)", () => {
    const longName = "a".repeat(200);
    const doc = extractTokensFromChunks([chunk(`:root { --${longName}: #4f46e5; }`)]);
    const color = doc.color as Record<string, { $value: string }>;
    const keys = Object.keys(color);
    expect(keys.length).toBe(1);
    expect(keys[0].length).toBeLessThanOrEqual(64);
    // It is still the sanitized prefix of the original name, no trailing dash.
    expect(keys[0]).toBe("a".repeat(64));
    expect(keys[0].endsWith("-")).toBe(false);
    expect(color[keys[0]].$value).toBe("#4f46e5");
  });

  it("returns and respects caps on a 200KB single-line minified input (Fix 9)", () => {
    // One giant line: many tiny color rules concatenated with no newlines.
    const rules = Array.from({ length: 8000 }, (_, i) => {
      const hex = (0x100000 + i * 0x0101).toString(16).slice(-6).padStart(6, "0");
      return `.cccccc${i}{color:#${hex}}`;
    }).join("");
    expect(rules.length).toBeGreaterThan(100_000);
    const doc = extractTokensFromChunks([chunk(rules)]);
    // It completes and still produces a valid, capped document.
    expect(validateTokensDoc(doc)).toEqual([]);
    const count = Object.keys((doc.color as Record<string, unknown>) ?? {}).length;
    expect(count).toBeLessThanOrEqual(24);
  });
});
