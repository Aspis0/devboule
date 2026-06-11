// Regression test for the mid-build shadow-orphan fix in PolisRenderer.createBuildingNode.
//
// SPRITE-SHEET UPDATE: the per-building shadow is now a Sprite (textured from the
// atlas), parented to layers.shadows. The INVARIANT is unchanged: if any step AFTER
// `this.layers.shadows.addChild(shadowSprite)` throws, the shadow SPRITE must be
// removed and destroyed before re-throwing, so no orphan accumulates on
// layers.shadows. The building must also NOT appear in buildingNodes or fileIdByPath
// (those are written after the shadow parent, inside the try).
//
// Strategy: use Object.create(PolisRenderer.prototype) to get an instance without
// running the constructor, then set up the minimal fields createBuildingNode (a
// private method, accessed via cast-to-any) accesses, INCLUDING a fake renderer for
// the atlas's generateTexture + a real BuildingTextureAtlas. Mock `buildBuildingParts`
// to return a real Graphics shadow + a real Container static body. Mock
// `worstSinSeverity` to throw — it is called AFTER the shadow sprite is parented
// (inside attachBuildingDynamics) and is a deterministic throw point.

import { describe, it, expect, vi, beforeEach } from "vitest";

// --------------------------------------------------------------------------
// Mocks — declared BEFORE module imports so vitest hoists them correctly.
// --------------------------------------------------------------------------

// Mock buildBuildingParts to return a minimal BuiltParts with real PIXI objects.
// The actual geometry is irrelevant for this test; we only care about the shadow
// sprite's parent lifecycle. We import Graphics/Container AFTER the mock below.
vi.mock("./buildings", () => ({
  buildBuildingParts: vi.fn(),
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

const { Container, Graphics, Texture, TextStyle } = await import("pixi.js");
const { PolisRenderer } = await import("./PolisRenderer");
const { buildBuildingParts } = await import("./buildings");
const { worstSinSeverity } = await import("./diffCity");
const { BuildingTextureAtlas } = await import("./buildingAtlas");

const buildBuildingSpy = buildBuildingParts as ReturnType<typeof vi.fn>;
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

  // viewport.scale.x used for LOD checks on pennant/disaster/investigation/label.
  (fake as unknown as Record<string, unknown>)["viewport"] = {
    scale: { x: 1 },
  };

  // app.renderer.generateTexture — the atlas calls this for the static body +
  // shadow. A headless stub returns a fresh empty Texture (no GPU needed).
  (fake as unknown as Record<string, unknown>)["app"] = {
    renderer: {
      generateTexture: () => new Texture(),
    },
  };

  // A real atlas (its keying/caching/destroy logic is what we exercise); it uses
  // the fake renderer above. dpr 1 for a stable resolution.
  (fake as unknown as Record<string, unknown>)["buildingAtlas"] =
    new BuildingTextureAtlas(1);

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
  let staticBody: InstanceType<typeof Container>;

  beforeEach(() => {
    // Fresh real PIXI objects for each test so parent-state is clean. The mock
    // returns a NEW pair per call (createBuildingNode is called once per test, but
    // the atlas consumes/destroys these on the first miss).
    buildBuildingSpy.mockImplementation(() => {
      shadowGraphics = new Graphics();
      staticBody = new Container();
      return {
        staticBody,
        shadow: shadowGraphics,
        anims: [],
        pennant: null,
        hw: 32,
        depth: 40,
        foot: [2, 2] as [number, number],
      };
    });

    // Default: no disaster (worstSinSeverity returns null → no throw).
    worstSinSeveritySpy.mockReturnValue(null);
  });

  it("SUCCESS PATH: a shadow SPRITE is parented to layers.shadows after a clean build", () => {
    const { fake, shadows } = makeFakeRenderer();
    const b = mkBuilding();

    const node = (fake as unknown as Record<string, (...args: unknown[]) => unknown>)["createBuildingNode"].call(fake, b) as {
      shadow: unknown;
    };

    // Exactly ONE child on the shadows layer: the building's shadow Sprite (the
    // source shadow Graphics was consumed + destroyed by the atlas on the miss).
    expect(shadows.children).toHaveLength(1);
    expect(shadows.children[0]).toBe(node.shadow);
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

  it("THROW PATH: the shadow SPRITE is destroyed (not just removed)", () => {
    worstSinSeveritySpy.mockImplementation(() => {
      throw new Error("simulated mid-build failure");
    });

    const { fake, shadows } = makeFakeRenderer();
    const b = mkBuilding();

    // Capture the shadow Sprite the moment it is parented (it is detached again on
    // the throw path, so we can't read it off the layer afterwards).
    let shadowSprite: { destroyed: boolean } | null = null;
    const realAdd = shadows.addChild.bind(shadows);
    shadows.addChild = ((...kids: unknown[]) => {
      shadowSprite = kids[0] as { destroyed: boolean };
      return realAdd(...(kids as Parameters<typeof realAdd>));
    }) as typeof shadows.addChild;

    expect(() =>
      (fake as unknown as Record<string, (...args: unknown[]) => unknown>)["createBuildingNode"].call(fake, b),
    ).toThrow();

    // PIXI display objects set `.destroyed = true` after destroy().
    expect(shadowSprite).not.toBeNull();
    expect(shadowSprite!.destroyed).toBe(true);
    // And no orphan remains on the layer.
    expect(shadows.children).toHaveLength(0);
  });
});

describe("PolisRenderer.createBuildingNode — atlas.get throw cleanup (FIX 4)", () => {
  beforeEach(() => {
    worstSinSeveritySpy.mockReturnValue(null);
  });

  it("generateTexture throws → the freshly-built static body, shadow AND pennant are destroyed with no orphan on layers", () => {
    const pennant = new Graphics();
    const body = new Container();
    const shadow = new Graphics();
    buildBuildingSpy.mockImplementation(() => ({
      staticBody: body,
      shadow,
      anims: [],
      pennant, // a provider pennant exists → must be destroyed on the throw path
      hw: 32,
      depth: 40,
      foot: [2, 2] as [number, number],
    }));

    const { fake, shadows } = makeFakeRenderer();
    // Make the ATLAS step throw: generateTexture blows up on the cache MISS, AFTER
    // our build closure ran (so staticBody/shadow/pennant exist) but BEFORE the
    // atlas destroys them — exactly the leak FIX 4 closes.
    (fake as unknown as { app: { renderer: { generateTexture: () => unknown } } }).app.renderer.generateTexture =
      () => {
        throw new Error("simulated GPU/generateTexture failure");
      };

    const b = mkBuilding();
    expect(() =>
      (fake as unknown as Record<string, (...args: unknown[]) => unknown>)["createBuildingNode"].call(fake, b),
    ).toThrow("simulated GPU/generateTexture failure");

    // FIX 4: all three freshly-built parts destroyed (no retained Graphics tree).
    expect(body.destroyed).toBe(true);
    expect(shadow.destroyed).toBe(true);
    expect(pennant.destroyed).toBe(true);

    // The throw is BEFORE makeShadowSprite, so no shadow Sprite was ever parented:
    // the shadows layer stays empty (no orphan) and no node was registered.
    expect(shadows.children).toHaveLength(0);
    expect(
      (fake as unknown as { buildingNodes: Map<string, unknown> }).buildingNodes.has("f-test"),
    ).toBe(false);
  });
});

describe("PolisRenderer.createBuildingNode — atlas cache HIT disposal (second building)", () => {
  beforeEach(() => {
    worstSinSeveritySpy.mockReturnValue(null);
  });

  it("a SECOND building of the same variant destroys its freshly-built static body + shadow (atlas HIT returns the shared texture)", () => {
    // Each build() call yields a FRESH pair; we capture the second building's pair.
    const bodies: InstanceType<typeof Container>[] = [];
    const shadowsBuilt: InstanceType<typeof Graphics>[] = [];
    buildBuildingSpy.mockImplementation(() => {
      const sBody = new Container();
      const sShadow = new Graphics();
      bodies.push(sBody);
      shadowsBuilt.push(sShadow);
      return {
        staticBody: sBody,
        shadow: sShadow,
        anims: [],
        pennant: null,
        hw: 32,
        depth: 40,
        foot: [2, 2] as [number, number],
      };
    });

    const { fake } = makeFakeRenderer();
    const call = (b: unknown) =>
      (fake as unknown as Record<string, (...args: unknown[]) => unknown>)[
        "createBuildingNode"
      ].call(fake, b);

    // First building: cache MISS → the atlas consumes + destroys bodies[0]/shadows[0].
    call(mkBuilding({ fileId: "f1", filePath: "src/a.ts" }));
    // Same purpose+tier → same variant key. Second building: cache HIT.
    call(mkBuilding({ fileId: "f2", filePath: "src/b.ts" }));

    // The existing atlas test covers the MISS disposal (bodies[0]); this asserts the
    // HIT branch in the CALLER destroys the second building's freshly-built parts so
    // the heavy static Graphics is not retained or orphaned.
    expect(bodies).toHaveLength(2);
    expect(bodies[1].destroyed).toBe(true);
    expect(shadowsBuilt[1].destroyed).toBe(true);
  });
});
