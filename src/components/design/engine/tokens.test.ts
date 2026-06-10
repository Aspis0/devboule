import { describe, it, expect } from "vitest";
import {
  EMPTY_TOKENS,
  isDtcgToken,
  isValidTokensDoc,
  resolveToken,
  tokenNamesForPrompt,
  validateTokensDoc,
  type DtcgDocument,
} from "./tokens";

const sample: DtcgDocument = {
  color: {
    $type: "color",
    brand: { $value: "#c2410c", $type: "color" },
    surface: { $value: "#fff7ed", $type: "color" },
  },
  spacing: {
    md: { $value: "8px", $type: "dimension" },
    lg: { $value: "16px", $type: "dimension" },
  },
  typography: {
    sans: { $value: "Inter, sans-serif", $type: "fontFamily" },
  },
};

describe("validateTokensDoc", () => {
  it("accepts a well-formed DTCG document", () => {
    expect(validateTokensDoc(sample)).toEqual([]);
    expect(isValidTokensDoc(sample)).toBe(true);
  });

  it("accepts the empty document", () => {
    expect(validateTokensDoc(EMPTY_TOKENS)).toEqual([]);
  });

  it("rejects a non-object root", () => {
    expect(validateTokensDoc(null).length).toBeGreaterThan(0);
    expect(validateTokensDoc("nope").length).toBeGreaterThan(0);
    expect(validateTokensDoc(42).length).toBeGreaterThan(0);
    expect(validateTokensDoc([]).length).toBeGreaterThan(0);
  });

  it("rejects a non-object/non-metadata leaf", () => {
    const bad = { color: { brand: "#c2410c" } }; // a bare string, not a token object
    const problems = validateTokensDoc(bad);
    expect(problems.length).toBe(1);
    expect(problems[0]).toContain("color.brand");
  });

  it("rejects a non-string $type on a token", () => {
    const bad = { color: { brand: { $value: "#000", $type: 7 } } };
    const problems = validateTokensDoc(bad);
    expect(problems.some((p) => p.includes("$type"))).toBe(true);
  });

  it("rejects a non-string $type on a group", () => {
    const bad = { color: { $type: 9, brand: { $value: "#000" } } };
    expect(validateTokensDoc(bad).some((p) => p.includes("$type"))).toBe(true);
  });

  it("flags excessive nesting rather than recursing without bound", () => {
    // Build a deeply nested group beyond the depth cap.
    let node: Record<string, unknown> = { $value: "#000", $type: "color" };
    for (let i = 0; i < 40; i++) node = { g: node };
    expect(validateTokensDoc(node).some((p) => p.includes("deeply"))).toBe(true);
  });
});

describe("isDtcgToken", () => {
  it("detects token leaves by $value presence", () => {
    expect(isDtcgToken({ $value: "#000" })).toBe(true);
    expect(isDtcgToken({ brand: { $value: "#000" } })).toBe(false);
    expect(isDtcgToken(null)).toBe(false);
    expect(isDtcgToken("x")).toBe(false);
  });
});

describe("tokenNamesForPrompt", () => {
  it("extracts dotted token names in stable sorted order", () => {
    expect(tokenNamesForPrompt(sample)).toEqual([
      "color.brand",
      "color.surface",
      "spacing.lg",
      "spacing.md",
      "typography.sans",
    ]);
  });

  it("returns an empty list for an invalid / empty document", () => {
    expect(tokenNamesForPrompt(EMPTY_TOKENS)).toEqual([]);
    expect(tokenNamesForPrompt(null)).toEqual([]);
    expect(tokenNamesForPrompt("nope")).toEqual([]);
  });

  it("caps the number of names it returns", () => {
    const big: Record<string, unknown> = {};
    for (let i = 0; i < 200; i++) {
      big[`t${String(i).padStart(3, "0")}`] = { $value: "#000", $type: "color" };
    }
    const names = tokenNamesForPrompt(big);
    expect(names.length).toBeLessThanOrEqual(80);
  });

  it("ignores $-metadata keys (does not treat them as tokens)", () => {
    const doc = { color: { $type: "color", brand: { $value: "#000" } } };
    expect(tokenNamesForPrompt(doc)).toEqual(["color.brand"]);
  });
});

describe("resolveToken", () => {
  it("resolves a {group.token} reference to its $value", () => {
    expect(resolveToken("{color.brand}", sample)).toBe("#c2410c");
    expect(resolveToken("{spacing.md}", sample)).toBe("8px");
  });

  it("returns the input unchanged for a non-reference or missing path", () => {
    expect(resolveToken("#fff", sample)).toBe("#fff");
    expect(resolveToken("{color.missing}", sample)).toBe("{color.missing}");
    expect(resolveToken("{nope}", sample)).toBe("{nope}");
  });

  it("returns the input when the path points at a group, not a token", () => {
    expect(resolveToken("{color}", sample)).toBe("{color}");
  });

  it("stringifies a finite numeric $value", () => {
    const doc: DtcgDocument = {
      fontWeight: { bold: { $value: 700, $type: "fontWeight" } },
    } as unknown as DtcgDocument;
    expect(resolveToken("{fontWeight.bold}", doc)).toBe("700");
  });

  it("WARNING 6: never coerces a composite/null/non-finite $value — returns the original ref", () => {
    const doc: DtcgDocument = {
      shadow: {
        // Composite DTCG value (object) — must NOT become "[object Object]".
        sm: {
          $value: { color: "#000", offsetX: "0", offsetY: "1px", blur: "2px" },
          $type: "shadow",
        },
        nul: { $value: null },
        nan: { $value: Number.NaN },
        arr: { $value: ["a", "b"] },
        bool: { $value: true },
      },
    } as unknown as DtcgDocument;
    expect(resolveToken("{shadow.sm}", doc)).toBe("{shadow.sm}");
    expect(resolveToken("{shadow.nul}", doc)).toBe("{shadow.nul}");
    expect(resolveToken("{shadow.nan}", doc)).toBe("{shadow.nan}");
    expect(resolveToken("{shadow.arr}", doc)).toBe("{shadow.arr}");
    expect(resolveToken("{shadow.bool}", doc)).toBe("{shadow.bool}");
  });
});
