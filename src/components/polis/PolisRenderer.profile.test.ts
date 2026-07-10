// B2c — the renderer APPLIES the hardware render profile's LOD thresholds (headless
// PIXI v8). Proves the LOD pass + the attach-on-demand label gate read the
// PER-INSTANCE profile bands, NOT the historical rich constants:
//   - a LEAN renderer (lodLabelsIn 0.85) does NOT create a label at zoom 0.7 (which
//     a RICH renderer at 0.62 would) but DOES at zoom 0.9.
//   - the ambient walker cap is the profile's maxAmbientWalkers (via
//     desiredAmbientCount), so a lean/minimal box renders a smaller crowd.
//
// Strategy mirrors PolisRenderer.sprite.test.ts: Object.create the renderer, seed
// the LOD fields from a chosen profile tier, run the real createBuildingNode +
// updateCulling.

import { describe, it, expect, vi } from "vitest";

const { Container, Texture, TextStyle, Rectangle } = await import("pixi.js");
const { PolisRenderer } = await import("./PolisRenderer");
const { BuildingTextureAtlas } = await import("./buildingAtlas");
const { RENDER_PROFILES } = await import("./renderProfile");
const { desiredAmbientCount } = await import("./AmbientLayer");
import type { Building } from "../../types/city";

const growthFxStub = { cancelTransition: vi.fn(), popIn: vi.fn(), dust: vi.fn() };

function mkBuilding(over: Partial<Building> = {}): Building {
  return {
    fileId: "f1",
    filePath: "src/a.ts",
    districtId: "d1",
    purpose: "house",
    purposeSource: "grounded",
    linesOfCode: 100,
    visualTier: "synoikia",
    coords: { x: 0, y: 0 },
    status: "idle",
    label: "a.ts",
    description: "",
    lastModified: "2026-01-01",
    sins: [],
    notes: [],
    ...over,
  } as Building;
}

type AnyRec = Record<string, unknown>;

function makeRenderer(tier: "rich" | "lean" | "minimal", scale: number) {
  const profile = RENDER_PROFILES[tier];
  const fake = Object.create(
    PolisRenderer.prototype,
  ) as InstanceType<typeof PolisRenderer>;
  const set = (k: string, v: unknown) =>
    ((fake as unknown as AnyRec)[k] = v);

  set("layers", { shadows: new Container(), buildings: new Container(), ui: new Container() });
  set("viewport", { scale: { x: scale } });
  set("app", { renderer: { generateTexture: () => new Texture() } });
  set("buildingAtlas", new BuildingTextureAtlas(1));
  set("buildingNodes", new Map());
  set("fileIdByPath", new Map());
  set("animatedNodes", []);
  set("chunks", new Map());
  set("growthFx", growthFxStub);
  set("callbacks", {});
  set("labelStyle", new TextStyle({ fontFamily: "sans-serif", fontSize: 12 }));
  // The profile + seeded LOD bands (what the real constructor does).
  set("profile", profile);
  set("lodLabelsIn", profile.lodLabelsIn);
  set("lodLabelsOut", profile.lodLabelsOut);
  set("lodDetails", profile.lodDetails);
  set("lodAgents", profile.lodAgents);
  return fake;
}

const create = (f: unknown, b: Building) =>
  (f as Record<string, (...a: unknown[]) => unknown>)["createBuildingNode"].call(
    f,
    b,
  ) as { label: unknown };

/** Run ONE updateCulling pass at `scale` (wires the fields it sweeps). */
function driveCulling(fake: unknown, scale: number): void {
  const r = fake as AnyRec;
  const lodStub = { setLodVisible: () => {} };
  r["destroyed"] = false;
  r["cullDirty"] = true;
  r["viewBounds"] = new Rectangle(0, 0, 1, 1);
  r["terrainChunks"] = [];
  r["roadMinorLayer"] = null;
  r["agentLayer"] = lodStub;
  r["ambientLayer"] = lodStub;
  r["tradeRouteLayer"] = lodStub;
  r["roadHitLayer"] = lodStub;
  r["externalLayer"] = lodStub;
  r["viewport"] = {
    scale: { x: scale },
    left: -1e6,
    top: -1e6,
    worldScreenWidth: 2e6,
    worldScreenHeight: 2e6,
  };
  r["lastScale"] = -10;
  (r as Record<string, (...a: unknown[]) => unknown>)["updateCulling"].call(r);
}

describe("B2c renderer applies the profile LOD thresholds (not the rich constants)", () => {
  it("a LEAN renderer creates NO label at zoom 0.7 (rich would at 0.62)", () => {
    // Build zoomed out so no label is seeded, then cull at 0.7 — below lean's
    // lodLabelsIn (0.85) so a label must NOT appear (a rich 0.62 gate WOULD show one).
    const fake = makeRenderer("lean", 0.3);
    const node = create(fake, mkBuilding()) as { label: unknown };
    expect(node.label).toBeNull();
    driveCulling(fake, 0.7);
    expect(node.label).toBeNull(); // lean: still hidden at 0.7
  });

  it("a LEAN renderer DOES create the label once zoomed past its lodLabelsIn (0.9)", () => {
    const fake = makeRenderer("lean", 0.3);
    const node = create(fake, mkBuilding()) as { label: unknown };
    expect(node.label).toBeNull();
    driveCulling(fake, 0.9); // >= lean lodLabelsIn (0.85)
    expect(node.label).not.toBeNull();
  });

  it("a RICH renderer creates the label at 0.7 (its lodLabelsIn is 0.62) — contrast", () => {
    const fake = makeRenderer("rich", 0.3);
    const node = create(fake, mkBuilding()) as { label: unknown };
    expect(node.label).toBeNull();
    driveCulling(fake, 0.7);
    expect(node.label).not.toBeNull(); // rich: visible at 0.7
  });
});

describe("B2c ambient walker cap is the profile's maxAmbientWalkers", () => {
  it("desiredAmbientCount is capped by the profile (lean < rich for a big city)", () => {
    const big = 500; // a city with many road nodes
    const rich = desiredAmbientCount(big, RENDER_PROFILES.rich.maxAmbientWalkers);
    const lean = desiredAmbientCount(big, RENDER_PROFILES.lean.maxAmbientWalkers);
    const minimal = desiredAmbientCount(
      big,
      RENDER_PROFILES.minimal.maxAmbientWalkers,
    );
    expect(rich).toBe(RENDER_PROFILES.rich.maxAmbientWalkers);
    expect(lean).toBe(RENDER_PROFILES.lean.maxAmbientWalkers);
    expect(minimal).toBe(RENDER_PROFILES.minimal.maxAmbientWalkers);
    expect(lean).toBeLessThan(rich);
    expect(minimal).toBeLessThan(lean);
  });
});
