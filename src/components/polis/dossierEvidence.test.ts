// Polis — unit tests for pure dossier EVIDENCE extraction + presentational
// contract (evidence stays visible when the Oracle narrative is unavailable).
//
// Pure extraction runs under node; the render pin uses jsdom + static markup.

import { describe, it, expect } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { Building, CityState, Road, UrbanSin } from "../../types/city";
import {
  buildDossierEvidence,
  formatRoleLine,
  EVIDENCE_PEER_CAP,
  NOTHING_IMPORTS_THIS,
  IMPORTS_NOTHING,
  GRAPH_UNAVAILABLE,
  NO_DETECTED_PROBLEMS,
} from "./dossierEvidence";
import { DossierEvidenceSection } from "./dossierEvidenceView";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

function mkBuilding(partial: Partial<Building> & Pick<Building, "fileId" | "filePath">): Building {
  return {
    districtId: "d1",
    purpose: "workshop",
    purposeSource: "extension",
    linesOfCode: 100,
    visualTier: "kalybe",
    coords: { x: 0, y: 0 },
    status: "normal",
    label: partial.filePath.split("/").pop() ?? partial.fileId,
    description: "",
    lastModified: "",
    sins: [],
    notes: [],
    ...partial,
  };
}

function mkRoad(
  from: string,
  to: string,
  weight = 1,
  roadId?: string,
): Road {
  return {
    roadId: roadId ?? `${from}->${to}`,
    from,
    to,
    type: "import",
    style: "lastricata",
    weight,
  };
}

function mkCity(buildings: Building[], roads: Road[]): CityState {
  return {
    version: 1,
    projectName: "test",
    era: "test",
    generatedAt: "",
    gridSize: { w: 10, h: 10 },
    districts: [
      {
        districtId: "d1",
        name: "Commons",
        type: "commons",
        bounds: { x: 0, y: 0, w: 10, h: 10 },
        wallStyle: "none",
        colorAccent: "#ccc",
      },
    ],
    buildings,
    roads,
    agents: [],
    externalServices: [],
    notes: [],
    sins: [],
  };
}

// ---------------------------------------------------------------------------
// Graph: importers / imports
// ---------------------------------------------------------------------------

describe("buildDossierEvidence — import graph", () => {
  it("derives imports (out) and importers (in) with fileId → path resolution", () => {
    const target = mkBuilding({ fileId: "t", filePath: "src/target.ts", label: "target.ts" });
    const dep = mkBuilding({ fileId: "dep", filePath: "src/dep.ts", label: "dep.ts" });
    const consumer = mkBuilding({
      fileId: "c",
      filePath: "src/consumer.ts",
      label: "consumer.ts",
    });
    // consumer imports target; target imports dep
    const city = mkCity(
      [target, dep, consumer],
      [mkRoad("c", "t", 3), mkRoad("t", "dep", 2)],
    );

    const ev = buildDossierEvidence(target, city);
    expect(ev.graph.kind).toBe("ready");
    if (ev.graph.kind !== "ready") return;

    expect(ev.graph.imports.total).toBe(1);
    expect(ev.graph.imports.shown[0]).toEqual({
      fileId: "dep",
      filePath: "src/dep.ts",
      label: "dep.ts",
    });
    expect(ev.graph.imports.emptyMessage).toBeNull();

    expect(ev.graph.importers.total).toBe(1);
    expect(ev.graph.importers.shown[0]).toEqual({
      fileId: "c",
      filePath: "src/consumer.ts",
      label: "consumer.ts",
    });
    expect(ev.graph.importers.emptyMessage).toBeNull();
  });

  it("a file with no edges produces explicit empty messages (not a silent omit)", () => {
    const alone = mkBuilding({ fileId: "a", filePath: "src/alone.ts" });
    const other = mkBuilding({ fileId: "b", filePath: "src/other.ts" });
    // Roads exist elsewhere; alone has zero incident edges.
    const city2 = mkCity([alone, other], [mkRoad("b", "b", 1, "loop")]);

    const ev = buildDossierEvidence(alone, city2);
    expect(ev.graph.kind).toBe("ready");
    if (ev.graph.kind !== "ready") return;

    expect(ev.graph.importers.total).toBe(0);
    expect(ev.graph.importers.emptyMessage).toBe(NOTHING_IMPORTS_THIS);
    expect(ev.graph.imports.total).toBe(0);
    expect(ev.graph.imports.emptyMessage).toBe(IMPORTS_NOTHING);
  });

  it("missing city reports graph unavailable (does not claim 'nothing imports')", () => {
    const b = mkBuilding({ fileId: "a", filePath: "src/a.ts" });
    const ev = buildDossierEvidence(b, null);
    expect(ev.graph).toEqual({
      kind: "unavailable",
      message: GRAPH_UNAVAILABLE,
    });
  });

  it("sorts peers by weight DESC then filePath ASC (deterministic)", () => {
    const t = mkBuilding({ fileId: "t", filePath: "src/t.ts" });
    const a = mkBuilding({ fileId: "a", filePath: "src/a.ts", label: "a.ts" });
    const z = mkBuilding({ fileId: "z", filePath: "src/z.ts", label: "z.ts" });
    const m = mkBuilding({ fileId: "m", filePath: "src/m.ts", label: "m.ts" });
    // weights: z=1, a=5, m=5 — a before m at equal weight by path
    const city = mkCity(
      [t, a, z, m],
      [mkRoad("z", "t", 1), mkRoad("a", "t", 5), mkRoad("m", "t", 5)],
    );
    const ev = buildDossierEvidence(t, city);
    if (ev.graph.kind !== "ready") throw new Error("expected ready");
    expect(ev.graph.importers.shown.map((p) => p.fileId)).toEqual(["a", "m", "z"]);
  });

  it("list capping is exact: moreCount === total - shown.length", () => {
    const t = mkBuilding({ fileId: "t", filePath: "src/t.ts" });
    const peers: Building[] = [];
    const roads: Road[] = [];
    for (let i = 0; i < EVIDENCE_PEER_CAP + 3; i++) {
      const id = `p${String(i).padStart(2, "0")}`;
      peers.push(
        mkBuilding({
          fileId: id,
          filePath: `src/${id}.ts`,
          label: `${id}.ts`,
        }),
      );
      roads.push(mkRoad(id, "t", 10 - i));
    }
    const city = mkCity([t, ...peers], roads);
    const ev = buildDossierEvidence(t, city);
    if (ev.graph.kind !== "ready") throw new Error("expected ready");

    const list = ev.graph.importers;
    expect(list.total).toBe(EVIDENCE_PEER_CAP + 3);
    expect(list.shown).toHaveLength(EVIDENCE_PEER_CAP);
    expect(list.moreCount).toBe(3);
    expect(list.moreCount).toBe(list.total - list.shown.length);
  });
});

// ---------------------------------------------------------------------------
// Sins
// ---------------------------------------------------------------------------

describe("buildDossierEvidence — sins", () => {
  it("groups by severity in deterministic order (inferno → fire → smoke)", () => {
    const sins: UrbanSin[] = [
      {
        sinId: "s1",
        severity: "smoke",
        description: "unused import",
        autoDetectable: true,
      },
      {
        sinId: "s2",
        severity: "inferno",
        description: "hardcoded secret",
        autoDetectable: true,
      },
      {
        sinId: "s3",
        severity: "fire",
        description: "sql injection risk",
        autoDetectable: true,
      },
      {
        sinId: "s4",
        severity: "fire",
        description: "bare except",
        autoDetectable: true,
      },
    ];
    const b = mkBuilding({ fileId: "a", filePath: "src/a.ts", sins });
    const ev = buildDossierEvidence(b, mkCity([b], []));

    expect(ev.sinGroups.map((g) => g.severity)).toEqual([
      "inferno",
      "fire",
      "smoke",
    ]);
    // Within fire: description ASC
    expect(ev.sinGroups[1].items.map((i) => i.description)).toEqual([
      "bare except",
      "sql injection risk",
    ]);
  });

  it("empty sins → empty groups (UI uses NO_DETECTED_PROBLEMS)", () => {
    const b = mkBuilding({ fileId: "a", filePath: "src/a.ts", sins: [] });
    const ev = buildDossierEvidence(b, mkCity([b], []));
    expect(ev.sinGroups).toEqual([]);
    expect(NO_DETECTED_PROBLEMS.length).toBeGreaterThan(0);
  });
});

// ---------------------------------------------------------------------------
// Role line
// ---------------------------------------------------------------------------

describe("formatRoleLine", () => {
  it("includes purpose, tier, and resolved district name", () => {
    const b = mkBuilding({
      fileId: "a",
      filePath: "src/a.ts",
      purpose: "temple",
      visualTier: "megaron",
      districtId: "d1",
    });
    const ev = buildDossierEvidence(b, mkCity([b], []));
    const line = formatRoleLine(ev);
    expect(line).toContain("Temple");
    expect(line).toContain("megaron");
    expect(line).toContain("Commons");
  });
});

// ---------------------------------------------------------------------------
// Evidence section still renders when narrative is unavailable
// ---------------------------------------------------------------------------

describe("DossierEvidenceSection — visible without Oracle narrative", () => {
  it("renders importers empty-copy and role line when narrative is unavailable", () => {
    const alone = mkBuilding({
      fileId: "a",
      filePath: "src/alone.ts",
      purpose: "library",
      visualTier: "oikia",
    });
    const evidence = buildDossierEvidence(alone, mkCity([alone], []));

    // Simulate the dossier panel body: honest Oracle status + evidence.
    const html = renderToStaticMarkup(
      createElement(
        "div",
        null,
        createElement("p", null, "Oracle is indexing…"),
        createElement(DossierEvidenceSection, { evidence }),
      ),
    );

    expect(html).toContain("Oracle is indexing…");
    expect(html).toContain(NOTHING_IMPORTS_THIS);
    expect(html).toContain(IMPORTS_NOTHING);
    expect(html).toContain(NO_DETECTED_PROBLEMS);
    // Role line present
    expect(html).toMatch(/Library|library/i);
    expect(html).toContain("oikia");
  });

  it("renders +N more with the exact remainder", () => {
    const t = mkBuilding({ fileId: "t", filePath: "src/t.ts" });
    const peers: Building[] = [];
    const roads: Road[] = [];
    for (let i = 0; i < EVIDENCE_PEER_CAP + 2; i++) {
      const id = `p${i}`;
      peers.push(mkBuilding({ fileId: id, filePath: `src/${id}.ts` }));
      roads.push(mkRoad(id, "t", 1));
    }
    const evidence = buildDossierEvidence(t, mkCity([t, ...peers], roads));
    const html = renderToStaticMarkup(
      createElement(DossierEvidenceSection, { evidence }),
    );
    expect(html).toContain("+2 more");
  });
});
