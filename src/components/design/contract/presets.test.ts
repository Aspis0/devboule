// PURE tests for the design-contract preset catalog.

import { describe, it, expect } from "vitest";
import { PRESET_CATALOG, PRESETS_VERSION, presetById } from "./presets";
import {
  validateTokensDoc,
  tokenNamesForPrompt,
} from "../engine/tokens";
import { colorSwatches } from "../shell/format";

describe("PRESET_CATALOG", () => {
  it("is versioned and has the three documented presets", () => {
    expect(PRESETS_VERSION).toBeGreaterThanOrEqual(1);
    expect(PRESET_CATALOG.map((p) => p.id)).toEqual([
      "tailwind-defaults",
      "material-ish",
      "minimal-neutral",
    ]);
  });

  it("has unique ids", () => {
    const ids = PRESET_CATALOG.map((p) => p.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("keeps every preset's designMd well under 4 KiB", () => {
    for (const p of PRESET_CATALOG) {
      const bytes = new TextEncoder().encode(p.designMd).length;
      expect(bytes, `${p.id} designMd bytes`).toBeLessThan(4 * 1024);
      expect(p.designMd).toContain("#"); // markdown header + hex values present
    }
  });

  it("every preset's tokens doc is a valid DTCG document", () => {
    for (const p of PRESET_CATALOG) {
      expect(validateTokensDoc(p.tokens), `${p.id} tokens`).toEqual([]);
    }
  });

  it("tokens round-trip through engine utilities (names + swatches)", () => {
    for (const p of PRESET_CATALOG) {
      const names = tokenNamesForPrompt(p.tokens);
      // Each preset has color + spacing + radius + typography token leaves.
      expect(names.some((n) => n.startsWith("color."))).toBe(true);
      expect(names.some((n) => n.startsWith("spacing."))).toBe(true);
      expect(names.some((n) => n.startsWith("radius."))).toBe(true);
      expect(names.some((n) => n.startsWith("typography."))).toBe(true);
      // The swatches resolve to concrete CSS color strings.
      const sw = colorSwatches(p.tokens, 4);
      expect(sw.length).toBe(4);
      for (const c of sw) expect(c).toMatch(/^#|^rgb|^hsl|^oklch/);
    }
  });

  it("presetById looks a preset up and returns undefined for unknown", () => {
    expect(presetById("material-ish")?.name).toBe("Material-ish");
    expect(presetById("nope")).toBeUndefined();
  });
});
