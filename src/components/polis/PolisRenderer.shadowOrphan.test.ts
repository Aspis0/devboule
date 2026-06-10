// Regression test for the mid-build shadow-orphan fix in PolisRenderer.createBuildingNode.
//
// INVARIANT: if any step AFTER `this.layers.shadows.addChild(built.shadow)` throws,
// the shadow Graphics must be removed and destroyed before re-throwing, so no orphan
// accumulates on layers.shadows. The building must also NOT appear in buildingNodes
// or fileIdByPath (those are written after the shadow parent, inside the try).
//
// Strategy: use Object.create(PolisRenderer.prototype) to get an instance without
// running the constructor, then set up the minimal fields that createBuildingNode
// (a private method, accessed via cast-to-any) accesses. Mock `buildBuilding` to
// return a real Graphics shadow + a real Container display. Mock `worstSinSeverity`
// to throw — it is called AFTER the shadow is parented and is a deterministic
// throw point that requires no b.sins-related special-casing beyond sins: [].

import { describe, it, expect, vi, beforeEach } from "vitest";

// --------------------------------------------------------------------------
// Mocks — declared BEFORE module imports so vitest hoists them correctly.
// --------------------------------------------------------------------------

// Mock buildBuilding to return a minimal BuiltBuilding with real PIXI objects.
// The actual geometry is irrelevant for this test; we only care about the shadow's
// parent lifecycle. We import Graphics/Container AFTER the mock declaration below.
vi.mock("./buildings", () => ({
  buildBuilding: vi.fn(),
}));

// Mock worstSinSeverity to throw on demand; controlled per-test via
// worstSinSeveritySpy.mockImplementation().
vi.mock("./diffCity", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./diffCity")>();
  return {
    ...actual,
    worstSinSeverity: vi.fn(() => null), // default: no disaster, no throw
    buildingChanged: actual.buildingChanged,
  };
});

// --------------------------------------------------------------------------
// Imports AFTER mocks
// --------------------------------------------------------------------------

const { Container, Graphics, TextStyle } = await import("pixi.js");
const { PolisRenderer } = await import("./PolisRenderer");
const { buildBuilding } = await import("./buildings");
const { worstSinSeverity } = await import("./diffCity");

const buildBuildingSpy = buildBuilding as ReturnType<typeof vi.fn>;
const worstSinSeveritySpy = worstSinSeverity as ReturnType<typeof vi.fn>;

// --------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------

/** Minimal Building for the test — sins is empty (no disaster overlay). */
function mkBuilding(overrides: Partial<{
  fileId: string;
  filePath: string;
  agentPresent: string | undefined;
  suspectOfCardId: string | undefined;
}> = {}) {
  return {
    fileId: overrides.fileId ?? "f-test",
    filePath: overrides.filePath ?? "src/test.ts",
    districtId: "d1",
    purpose: "source",
    purposeSource: "grounded",
    linesOfCode: 100,
    visualTier: "tier1",
    coords: { x: 0, y: 0 },
    status: "idle",
    label: "test.ts",
    description: "",
    lastModified: "2026-01-01",
    agentPresent: overrides.agentPresent,
    suspectOfCardId: overrides.suspectOfCardId,
    sins: [],
    notes: [],
  };
}

/**
 * Construct a fake PolisRenderer that skips the constructor but has all the
 * fields that createBuildingNode reads. Returns the fake renderer and its
 * shadows container so the test can count orphan children.
 */
function makeFakeRenderer() {
  const shadows = new Container();
  const buildings = new Container();
  const ui = new Container();

  const fake = Object.create(PolisRenderer.prototype) as InstanceType<typeof PolisRenderer>;

  // The layers object createBuildingNode reads.
  (fake as unknown as Record<string, unknown>)["layers"] = {
    shadows,
    buildings,
    ui,
  };

  // viewport.scale.x used for LOD checks on pennant/disaster/investigation.
  (fake as unknown as Record<string, unknown>)["viewport"] = {
    scale: { x: 1 },
  };

  // callbacks — empty, no-op.
  (fake as unknown as Record<string, unknown>)["callbacks"] = {};

  // labelStyle — TextStyle is constructed in the real constructor; provide a
  // minimal instance so `new Text({ text, style })` doesn't blow up.
  (fake as unknown as Record<string, unknown>)["labelStyle"] = new TextStyle({
    fontFamily: "sans-serif",
    fontSize: 12,
  });

  // State maps.
  (fake as unknown as Record<string, unknown>)["buildingNodes"] = new Map();
  (fake as unknown as Record<string, unknown>)["fileIdByPath"] = new Map();
  (fake as unknown as Record<string, unknown>)["animatedNodes"] = [];
  (fake as unknown as Record<string, unknown>)["chunks"] = new Map();

  return { fake, shadows };
}

// --------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------

describe("PolisRenderer.createBuildingNode — shadow-orphan cleanup on mid-build throw", () => {
  let shadowGraphics: InstanceType<typeof Graphics>;
  let displayContainer: InstanceType<typeof Container>;

  beforeEach(() => {
    // Fresh real PIXI objects for each test so parent-state is clean.
    shadowGraphics = new Graphics();
    displayContainer = new Container();

    buildBuildingSpy.mockReturnValue({
      shadow: shadowGraphics,
      display: displayContainer,
      anims: [],
      pennant: null,
      hw: 32,
      depth: 40,
      foot: [2, 2] as [number, number],
    });

    // Default: no disaster (worstSinSeverity returns null → no throw).
    worstSinSeveritySpy.mockReturnValue(null);
  });

  it("SUCCESS PATH: shadow is parented to layers.shadows after a clean build", () => {
    const { fake, shadows } = makeFakeRenderer();
    const b = mkBuilding();

    const node = (fake as unknown as Record<string, (...args: unknown[]) => unknown>)["createBuildingNode"].call(fake, b);

    expect(shadows.children).toHaveLength(1);
    expect(shadows.children[0]).toBe(shadowGraphics);
    expect((fake as unknown as { buildingNodes: Map<string, unknown> }).buildingNodes.has("f-test")).toBe(true);
    expect(node).toBeDefined();
  });

  it("THROW PATH: shadow is removed from layers.shadows and destroyed when post-shadow setup throws", () => {
    // Make worstSinSeverity (called mid-setup, after shadow is parented) throw.
    worstSinSeveritySpy.mockImplementation(() => {
      throw new Error("simulated mid-build failure");
    });

    const { fake, shadows } = makeFakeRenderer();
    const b = mkBuilding();

    // createBuildingNode must re-throw so the outer handler (runBatch) can log+skip.
    expect(() =>
      (fake as unknown as Record<string, (...args: unknown[]) => unknown>)["createBuildingNode"].call(fake, b),
    ).toThrow("simulated mid-build failure");

    // INVARIANT: no orphan on the shadows layer.
    expect(shadows.children).toHaveLength(0);

    // INVARIANT: no partial entry in buildingNodes or fileIdByPath.
    expect(
      (fake as unknown as { buildingNodes: Map<string, unknown> }).buildingNodes.has("f-test"),
    ).toBe(false);
    expect(
      (fake as unknown as { fileIdByPath: Map<string, unknown> }).fileIdByPath.has(
        "src/test.ts",
      ),
    ).toBe(false);
  });

  it("THROW PATH: shadow Graphics is destroyed (not just removed)", () => {
    worstSinSeveritySpy.mockImplementation(() => {
      throw new Error("simulated mid-build failure");
    });

    const { fake } = makeFakeRenderer();
    const b = mkBuilding();

    expect(() =>
      (fake as unknown as Record<string, (...args: unknown[]) => unknown>)["createBuildingNode"].call(fake, b),
    ).toThrow();

    // PIXI Graphics sets `.destroyed = true` after destroy().
    expect(shadowGraphics.destroyed).toBe(true);
  });
});
