import { describe, it, expect } from "vitest";
import { buildCensorStrip } from "./censorStripModel";
import type { CensorFinding } from "../../types/backend";

let seq = 0;
const f = (p: Partial<CensorFinding> & { file: string; disposition: CensorFinding["disposition"] }): CensorFinding =>
  ({
    id: `${p.file}-${p.disposition}-${seq++}`, file: p.file, contentHash: "h", line: null,
    severity: "low", category: "style", source: "gemma", title: "t", body: "b",
    verdict: "suspected", disposition: p.disposition, provenance: [], createdAt: "x",
  }) as unknown as CensorFinding;

describe("buildCensorStrip", () => {
  it("returns an empty model for no findings", () => {
    const m = buildCensorStrip([]);
    expect(m.items).toEqual([]);
    expect(m.dirtyFiles).toBe(0);
    expect(m.cleanFiles).toBe(0);
    expect(m.openFindings).toBe(0);
  });

  it("marks a file with an open finding as dirty with its open count", () => {
    const m = buildCensorStrip([
      f({ file: "src/auth/login.ts", disposition: "open" }),
      f({ file: "src/auth/login.ts", disposition: "open" }),
    ]);
    expect(m.items).toHaveLength(1);
    expect(m.items[0]).toMatchObject({ file: "src/auth/login.ts", status: "dirty", openCount: 2 });
    expect(m.dirtyFiles).toBe(1);
    expect(m.openFindings).toBe(2);
  });

  it("marks a file with only resolved findings as clean", () => {
    const m = buildCensorStrip([
      f({ file: "src/a.ts", disposition: "fixed" }),
      f({ file: "src/a.ts", disposition: "fp" }),
      f({ file: "src/a.ts", disposition: "wontfix" }),
    ]);
    expect(m.items[0]).toMatchObject({ file: "src/a.ts", status: "clean", openCount: 0 });
    expect(m.cleanFiles).toBe(1);
    expect(m.dirtyFiles).toBe(0);
    expect(m.openFindings).toBe(0);
  });

  it("maps an empty file path to a placeholder, never a blank entry", () => {
    const m = buildCensorStrip([f({ file: "", disposition: "open" })]);
    expect(m.items).toHaveLength(1);
    expect(m.items[0].file).toBe("(unknown file)");
    expect(m.items[0].status).toBe("dirty");
  });

  it("sorts dirty files before clean files, then by path", () => {
    const m = buildCensorStrip([
      f({ file: "z/clean.ts", disposition: "fixed" }),
      f({ file: "b/dirty.ts", disposition: "open" }),
      f({ file: "a/dirty.ts", disposition: "open" }),
    ]);
    expect(m.items.map((i) => i.file)).toEqual(["a/dirty.ts", "b/dirty.ts", "z/clean.ts"]);
    expect(m.items.map((i) => i.status)).toEqual(["dirty", "dirty", "clean"]);
  });
});
