import { describe, it, expect } from "vitest";
import { PolisRenderer } from "./PolisRenderer";

// Polis-P5 (F4 fix) — PolisRenderer.normalizeRelPath must MIRROR the Rust
// `censor::ledger::normalize_rel_path` (the source of the `findings-updated`
// `files` entries): backslashes → forward slashes, then COLLAPSE repeated `//`
// into a single `/`, strip leading `./`, and trim surrounding `/`. The same
// normalizer keys both the `fileIdByPath` index (built from `building.filePath`)
// and the lookup of an incoming event relPath, so an orchestrator path like
// `src//foo.ts` still resolves the building instead of being silently skipped.
//
// Tested headlessly against the static method — no PIXI instance / WebGL.

const norm = PolisRenderer.normalizeRelPath;

describe("PolisRenderer.normalizeRelPath", () => {
  it("collapses a doubled slash (parity with ledger.rs)", () => {
    expect(norm("src//foo.ts")).toBe("src/foo.ts");
  });

  it("collapses runs of 3+ slashes", () => {
    expect(norm("src///deep////foo.ts")).toBe("src/deep/foo.ts");
  });

  it("strips leading ./ then collapses //", () => {
    expect(norm("./a//b.ts")).toBe("a/b.ts");
  });

  it("normalizes backslashes and collapses // together", () => {
    expect(norm("a\\\\b//c.ts")).toBe("a/b/c.ts");
  });

  it("leaves an already-canonical path untouched", () => {
    expect(norm("src/foo.ts")).toBe("src/foo.ts");
  });

  it("trims surrounding slashes", () => {
    expect(norm("/src/foo.ts/")).toBe("src/foo.ts");
  });

  it("INDEX ↔ LOOKUP parity: a `src/foo.ts` index resolves a `src//foo.ts` event", () => {
    // Mimic the renderer: the index is keyed by normalize(building.filePath),
    // the lookup normalizes the incoming event relPath. Both must agree.
    const index = new Map<string, string>();
    index.set(norm("src/foo.ts"), "file-1");

    // Orchestrator/ledger could emit the path with a stray doubled slash.
    const resolved = index.get(norm("src//foo.ts"));
    expect(resolved).toBe("file-1");
  });

  it("INDEX ↔ LOOKUP parity: a `src//foo.ts` index resolves a `src/foo.ts` event", () => {
    const index = new Map<string, string>();
    index.set(norm("src//foo.ts"), "file-1");
    expect(index.get(norm("src/foo.ts"))).toBe("file-1");
  });
});
