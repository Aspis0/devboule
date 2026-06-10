// Self-contained spec for the pure Polis city-diff (`diffCity.ts`).
//
// The project has NO JS test runner wired (package.json `scripts` is only
// dev/build/preview/tauri; no vitest/jest, and importing one would break the
// `tsc --noEmit` build gate). So this spec is a ZERO-DEPENDENCY, self-asserting
// module: it exports `runDiffCitySpec()` which throws on any failed assertion.
// It type-checks as part of the normal build (the functions under test are
// pure) and can be run from a scratch script or a future runner without change.
//
// Contract verified:
//   - a tier change   → that fileId in `changed`
//   - a new file      → `added`
//   - a deleted file  → `removed`
//   - an unchanged    → none
//   - coord/purpose/provider/status/agent changes → `changed`; LOC-only → not.
//   - sins: a WORST-severity change → `changed`; cosmetic reorder or adding a
//     lesser sin below the current worst → NOT a rebuild.

import { diffBuildings, buildingChanged } from "./diffCity";
import type { Building } from "../../types/city";

function mkBuilding(overrides: Partial<Building> = {}): Building {
  return {
    fileId: "fid-1",
    filePath: "src/a.ts",
    districtId: "core",
    purpose: "house",
    purposeSource: "default",
    linesOfCode: 10,
    visualTier: "kalybe",
    coords: { x: 5, y: 5 },
    status: "normal",
    label: "a.ts",
    description: "",
    lastModified: "",
    sins: [],
    notes: [],
    ...overrides,
  };
}

function assert(cond: boolean, msg: string): void {
  if (!cond) throw new Error(`diffCity spec failed: ${msg}`);
}

function eq(a: readonly string[], b: readonly string[], msg: string): void {
  assert(a.length === b.length && a.every((v, i) => v === b[i]), `${msg} (got [${a}], want [${b}])`);
}

/** Runs every diff assertion. Throws on the first failure; returns silently on
 *  success. Pure — no IO, no globals. */
export function runDiffCitySpec(): void {
  // tier change → `changed`
  {
    const d = diffBuildings(
      [mkBuilding({ visualTier: "kalybe" })],
      [mkBuilding({ visualTier: "megaron" })],
    );
    eq(d.changed, ["fid-1"], "tier change → changed");
    eq(d.added, [], "tier change → no added");
    eq(d.removed, [], "tier change → no removed");
  }

  // new file → `added`
  {
    const d = diffBuildings(
      [mkBuilding({ fileId: "fid-1" })],
      [mkBuilding({ fileId: "fid-1" }), mkBuilding({ fileId: "fid-2" })],
    );
    eq(d.added, ["fid-2"], "new file → added");
    eq(d.changed, [], "new file → no changed");
    eq(d.removed, [], "new file → no removed");
  }

  // deleted file → `removed`
  {
    const d = diffBuildings(
      [mkBuilding({ fileId: "fid-1" }), mkBuilding({ fileId: "fid-2" })],
      [mkBuilding({ fileId: "fid-1" })],
    );
    eq(d.removed, ["fid-2"], "deleted file → removed");
    eq(d.added, [], "deleted file → no added");
    eq(d.changed, [], "deleted file → no changed");
  }

  // unchanged → none
  {
    const d = diffBuildings([mkBuilding()], [mkBuilding()]);
    eq(d.added, [], "unchanged → no added");
    eq(d.changed, [], "unchanged → no changed");
    eq(d.removed, [], "unchanged → no removed");
  }

  // field-level change detection
  assert(buildingChanged(mkBuilding(), mkBuilding({ coords: { x: 6, y: 5 } })), "coord change");
  assert(buildingChanged(mkBuilding(), mkBuilding({ purpose: "temple" })), "purpose change");
  assert(buildingChanged(mkBuilding(), mkBuilding({ status: "active" })), "status change");
  assert(buildingChanged(mkBuilding(), mkBuilding({ agentPresent: "ag-1" })), "agent change");
  assert(buildingChanged(mkBuilding(), mkBuilding({ provider: "cloudflare" })), "provider change");
  // a NEW sin (none → fire) raises the worst-severity rank → rebuild.
  assert(
    buildingChanged(
      mkBuilding(),
      mkBuilding({
        sins: [{ sinId: "s1", severity: "fire", description: "", autoDetectable: true }],
      }),
    ),
    "sin severity change (none → fire)",
  );
  // worsening the worst sin (smoke → inferno) → rebuild.
  assert(
    buildingChanged(
      mkBuilding({ sins: [{ sinId: "s1", severity: "smoke", description: "", autoDetectable: true }] }),
      mkBuilding({ sins: [{ sinId: "s1", severity: "inferno", description: "", autoDetectable: true }] }),
    ),
    "sin severity change (smoke → inferno)",
  );
  // cosmetic REORDER of equal-worst sins must NOT rebuild (worst rank unchanged).
  assert(
    !buildingChanged(
      mkBuilding({
        sins: [
          { sinId: "s1", severity: "fire", description: "", autoDetectable: true },
          { sinId: "s2", severity: "smoke", description: "", autoDetectable: true },
        ],
      }),
      mkBuilding({
        sins: [
          { sinId: "s2", severity: "smoke", description: "", autoDetectable: true },
          { sinId: "s1", severity: "fire", description: "", autoDetectable: true },
        ],
      }),
    ),
    "sin reorder (worst unchanged) is not a rebuild",
  );
  // adding a LESSER sin below the current worst must NOT rebuild.
  assert(
    !buildingChanged(
      mkBuilding({ sins: [{ sinId: "s1", severity: "fire", description: "", autoDetectable: true }] }),
      mkBuilding({
        sins: [
          { sinId: "s1", severity: "fire", description: "", autoDetectable: true },
          { sinId: "s2", severity: "smoke", description: "", autoDetectable: true },
        ],
      }),
    ),
    "adding a lesser sin below worst is not a rebuild",
  );
  // LOC / description / label do not affect the silhouette → NOT a rebuild.
  assert(
    !buildingChanged(mkBuilding(), mkBuilding({ linesOfCode: 999, description: "x", label: "z" })),
    "non-visual change is not a rebuild",
  );
}
