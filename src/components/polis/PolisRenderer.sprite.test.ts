// SPRITE-SHEET BUILDINGS — PolisRenderer node contract (headless PIXI v8).
//
// Proves the sprite-sheet migration of the building node:
//   - NODE SHAPE: a building node's `container` is a Container whose child 0 is a
//     body Sprite carrying the shared variant texture; it is interactive (eventMode
//     "static", carries the __fileId hit metadata) and its shadow is a Sprite.
//   - SHARED TEXTURE: two buildings of the SAME (purpose, level) share ONE texture
//     object; a different tier → a different texture (atlas keyed correctly).
//   - OVERLAY LIFECYCLE: an in-place update that ADDS a sin attaches a disaster
//     overlay; one that CLEARS it detaches the overlay — on the SAME node object
//     (identity preserved, no rebuild) and SAME body Sprite (texture swapped).
//   - DIFF TIER SWAP: updateBuildingNodeInPlace on a tier change swaps the body
//     texture WITHOUT replacing the node or the Sprite (object identity holds).
//   - REMOVED: destroyBuildingNode tears the node down but does NOT destroy the
//     SHARED variant texture (the atlas owns it; a sibling may still use it).
//
// Strategy mirrors PolisRenderer.shadowOrphan.test.ts: Object.create the renderer
// (skip the constructor), wire the minimal fields, use the REAL buildBuildingParts
// + a real BuildingTextureAtlas backed by a fake renderer that returns stub
// textures (the vitest env is "node" — no GPU). worstSinSeverity is the real one.

import { describe, it, expect, vi, beforeEach } from "vitest";

const { Container, Sprite, Texture, TextStyle, Rectangle } = await import("pixi.js");
const { PolisRenderer } = await import("./PolisRenderer");
const { BuildingTextureAtlas } = await import("./buildingAtlas");
import type { Building } from "../../types/city";

// A growthFx stub — the diff path isn't exercised here, but createBuildingNode /
// destroyBuildingNode reference growthFx via cancelTransition. Provide no-ops.
const growthFxStub = {
  cancelTransition: vi.fn(),
  popIn: vi.fn(),
  dust: vi.fn(),
};

function mkBuilding(over: Partial<Building> = {}): Building {
  return {
    fileId: "f1",
    filePath: "src/a.ts",
    districtId: "d1",
    purpose: "house",
    purposeSource: "grounded",
    linesOfCode: 100,
    visualTier: "synoikia", // tierRank → 2
    coords: { x: 0, y: 0 },
    status: "idle",
    label: "a.ts",
    description: "",
    lastModified: "2026-01-01",
    agentPresent: undefined,
    suspectOfCardId: undefined,
    sins: [],
    notes: [],
    ...over,
  } as Building;
}

function makeFakeRenderer() {
  const shadows = new Container();
  const buildings = new Container();
  const ui = new Container();
  const fake = Object.create(PolisRenderer.prototype) as InstanceType<typeof PolisRenderer>;
  const set = (k: string, v: unknown) =>
    ((fake as unknown as Record<string, unknown>)[k] = v);

  set("layers", { shadows, buildings, ui });
  set("viewport", { scale: { x: 1 } });
  set("app", { renderer: { generateTexture: () => new Texture() } });
  set("buildingAtlas", new BuildingTextureAtlas(1));
  set("buildingNodes", new Map());
  set("fileIdByPath", new Map());
  set("animatedNodes", []);
  set("chunks", new Map());
  set("growthFx", growthFxStub);
  set("callbacks", {});
  set("labelStyle", new TextStyle({ fontFamily: "sans-serif", fontSize: 12 }));
  return { fake, shadows, buildings };
}

// Private-method shims (the methods are private; call via cast).
type AnyRenderer = Record<string, (...a: unknown[]) => unknown>;
const create = (f: unknown, b: Building) =>
  (f as AnyRenderer)["createBuildingNode"].call(f, b) as {
    container: InstanceType<typeof Container>;
    bodySprite: InstanceType<typeof Sprite>;
    shadow: InstanceType<typeof Sprite>;
    disaster: unknown;
    kitAnims: unknown[];
    building: Building;
  };
const update = (f: unknown, node: unknown, b: Building) =>
  (f as AnyRenderer)["updateBuildingNodeInPlace"].call(f, node, b);
const destroy = (f: unknown, node: unknown) =>
  (f as AnyRenderer)["destroyBuildingNode"].call(f, node);

// Wire the extra fields updateCulling() reads (it sweeps chunks/terrain + the
// layer LOD stubs), then run ONE cull pass at `scale`. Used by the LOD-hysteresis
// test to drive zoom across the label dead-band. lastScale starts far from any
// scale we use so the first pass always reacts; a no-op layer stub satisfies the
// agent/ambient/trade/external setLodVisible calls.
function driveCulling(fake: unknown, scale: number): void {
  const r = fake as Record<string, unknown>;
  const lodStub = { setLodVisible: () => {} };
  r["destroyed"] = false;
  r["cullDirty"] = true;
  r["viewBounds"] = new Rectangle(0, 0, 1, 1);
  r["terrainChunks"] = [];
  r["roadMinorLayer"] = null;
  r["agentLayer"] = lodStub;
  r["ambientLayer"] = lodStub;
  r["tradeRouteLayer"] = lodStub;
  r["externalLayer"] = lodStub;
  // A very large viewport so every chunk intersects (no node is culled out).
  r["viewport"] = {
    scale: { x: scale },
    left: -1e6,
    top: -1e6,
    worldScreenWidth: 2e6,
    worldScreenHeight: 2e6,
  };
  // Force the LOD branch to fire on every call (scale step may be < 0.02 between
  // dead-band probes; we want each probe evaluated, not gated out).
  r["lastScale"] = -10;
  (r as AnyRenderer)["updateCulling"].call(r);
}

beforeEach(() => {
  growthFxStub.cancelTransition.mockClear();
});

describe("SPRITE node shape", () => {
  it("container is a Container; child 0 is a body Sprite with the variant texture; shadow is a Sprite", () => {
    const { fake, shadows } = makeFakeRenderer();
    const node = create(fake, mkBuilding());

    expect(node.container).toBeInstanceOf(Container);
    expect(node.container).not.toBeInstanceOf(Sprite); // root is a plain Container
    expect(node.bodySprite).toBeInstanceOf(Sprite);
    expect(node.container.children[0]).toBe(node.bodySprite);
    expect(node.bodySprite.texture).toBeInstanceOf(Texture);
    expect(node.shadow).toBeInstanceOf(Sprite);
    // The shadow sprite is parented to the shadows layer.
    expect(shadows.children).toContain(node.shadow);
  });

  it("the node container is interactive and carries the __fileId hit metadata", () => {
    const { fake } = makeFakeRenderer();
    const node = create(fake, mkBuilding({ fileId: "fX" }));
    expect(node.container.eventMode).toBe("static");
    expect((node.container as unknown as { __fileId: string }).__fileId).toBe("fX");
  });
});

describe("SHARED variant texture", () => {
  it("two buildings of the same (purpose, level) share ONE body texture object", () => {
    const { fake } = makeFakeRenderer();
    const a = create(fake, mkBuilding({ fileId: "f1", coords: { x: 0, y: 0 } }));
    const b = create(fake, mkBuilding({ fileId: "f2", coords: { x: 2, y: 0 } }));
    expect(b.bodySprite.texture).toBe(a.bodySprite.texture);
    expect(b.shadow.texture).toBe(a.shadow.texture);
  });

  it("a different tier yields a DIFFERENT body texture", () => {
    const { fake } = makeFakeRenderer();
    const a = create(fake, mkBuilding({ fileId: "f1", visualTier: "synoikia" }));
    const c = create(fake, mkBuilding({ fileId: "f2", coords: { x: 2, y: 0 }, visualTier: "megaron" }));
    expect(c.bodySprite.texture).not.toBe(a.bodySprite.texture);
  });
});

describe("OVERLAY lifecycle on in-place update", () => {
  it("adding a sin ATTACHES a disaster overlay; clearing it DETACHES — same node, same Sprite", () => {
    const { fake } = makeFakeRenderer();
    const clean = mkBuilding({ sins: [] });
    const node = create(fake, clean);
    expect(node.disaster).toBeNull();

    const bodyBefore = node.bodySprite; // identity must be preserved
    const containerBefore = node.container;

    // Same coords, now WITH a sin → in-place update attaches the disaster overlay.
    const dirty = mkBuilding({
      sins: [
        {
          sinId: "s1",
          severity: "fire",
          description: "high complexity",
          autoDetectable: true,
        },
      ],
    });
    update(fake, node, dirty);
    expect(node.disaster).not.toBeNull();
    expect(node.kitAnims.length).toBeGreaterThan(0);
    // Node + body Sprite identity preserved (NOT a rebuild).
    expect(node.container).toBe(containerBefore);
    expect(node.bodySprite).toBe(bodyBefore);

    // Clear the sin → the disaster overlay detaches.
    update(fake, node, mkBuilding({ sins: [] }));
    expect(node.disaster).toBeNull();
  });
});

describe("DIFF tier change = texture swap, no node rebuild", () => {
  it("updateBuildingNodeInPlace swaps the body texture without replacing node/Sprite", () => {
    const { fake } = makeFakeRenderer();
    const node = create(fake, mkBuilding({ visualTier: "synoikia" }));
    const texBefore = node.bodySprite.texture;
    const containerBefore = node.container;
    const bodyBefore = node.bodySprite;

    update(fake, node, mkBuilding({ visualTier: "megaron" }));

    expect(node.bodySprite.texture).not.toBe(texBefore); // swapped
    expect(node.container).toBe(containerBefore); // SAME node container
    expect(node.bodySprite).toBe(bodyBefore); // SAME body Sprite
    expect(node.building.visualTier).toBe("megaron");
  });
});

describe("LABEL is attach-on-demand (LOD-gated, lazy)", () => {
  it("no label Text is created when zoomed out below LOD_LABELS", () => {
    const { fake } = makeFakeRenderer();
    (fake as unknown as { viewport: { scale: { x: number } } }).viewport.scale.x = 0.3; // < LOD_LABELS (0.6)
    const node = create(fake, mkBuilding()) as unknown as { label: unknown };
    expect(node.label).toBeNull();
  });

  it("a label Text IS created (topmost child) when zoomed in at/above LOD_LABELS", () => {
    const { fake } = makeFakeRenderer();
    (fake as unknown as { viewport: { scale: { x: number } } }).viewport.scale.x = 1; // >= LOD_LABELS
    const node = create(fake, mkBuilding()) as unknown as {
      label: { text: string } | null;
      container: InstanceType<typeof Container>;
    };
    expect(node.label).not.toBeNull();
    // Topmost child (legible over the body + overlays).
    const kids = node.container.children;
    expect(kids[kids.length - 1]).toBe(node.label);
  });
});

describe("PENNANT (provider livery) is added exactly once, below label + overlays", () => {
  it("a building WITH a provider has its pennant once, body is child 0, pennant below label and disaster", () => {
    const { fake } = makeFakeRenderer();
    // A provider (cloudflare → livery) AND a sin (→ disaster overlay) AND a label
    // (scale 1 ≥ LOD_LABELS) so we can assert the full z-order in one node.
    const node = create(
      fake,
      mkBuilding({
        provider: "cloudflare",
        sins: [
          {
            sinId: "s1",
            severity: "fire",
            description: "x",
            autoDetectable: true,
          },
        ],
      }),
    ) as unknown as {
      container: InstanceType<typeof Container>;
      bodySprite: InstanceType<typeof Sprite>;
      pennant: InstanceType<typeof Container> | null;
      label: InstanceType<typeof Container> | null;
      disaster: { node: InstanceType<typeof Container> } | null;
    };

    expect(node.pennant).not.toBeNull();
    const pennant = node.pennant as InstanceType<typeof Container>;
    const kids = node.container.children;

    // Pennant appears EXACTLY ONCE (the FIX 2 regression: a double addChild would
    // re-parent it, leaving one entry but ABOVE the overlays — so we assert both
    // the single occurrence AND the index relations below).
    const pennantCount = kids.filter((c) => c === pennant).length;
    expect(pennantCount).toBe(1);

    // Body sprite is child 0.
    expect(kids[0]).toBe(node.bodySprite);

    const iPennant = kids.indexOf(pennant);
    const iDisaster = kids.indexOf(
      (node.disaster as { node: InstanceType<typeof Container> }).node,
    );
    const iLabel = kids.indexOf(node.label as InstanceType<typeof Container>);

    // Z-ORDER: body(0) < pennant < disaster < label.
    expect(iPennant).toBeGreaterThan(0);
    expect(iPennant).toBeLessThan(iDisaster); // pennant BELOW the disaster overlay
    expect(iPennant).toBeLessThan(iLabel); // pennant BELOW the label
    expect(iDisaster).toBeLessThan(iLabel); // disaster BELOW the label
  });
});

describe("STALE building regression — handlers read node.building at fire time", () => {
  it("an in-place update changing the Building delivers the NEW Building to onSelectBuilding", () => {
    const { fake } = makeFakeRenderer();
    let selected: Building | null = null;
    (fake as unknown as { callbacks: { onSelectBuilding: (b: Building | null) => void } }).callbacks =
      { onSelectBuilding: (b) => (selected = b) };

    const before = mkBuilding({ fileId: "f1", visualTier: "synoikia", sins: [] });
    const node = create(fake, before) as unknown as {
      container: InstanceType<typeof Container> & {
        emit: (ev: string, e: unknown) => void;
      };
      building: Building;
    };

    // In-place update with a CHANGED building (new tier + a sin). The container +
    // its pointer listeners are preserved by updateBuildingNodeInPlace.
    const after = mkBuilding({
      fileId: "f1",
      visualTier: "megaron",
      sins: [
        { sinId: "s1", severity: "fire", description: "x", autoDetectable: true },
      ],
    });
    update(fake, node, after);

    // Fire the tap — the handler must resolve node.building (NOW `after`), not the
    // stale closure over `before`.
    node.container.emit("pointertap", { stopPropagation() {} });
    expect(selected).not.toBeNull();
    expect((selected as unknown as Building).visualTier).toBe("megaron");
    expect((selected as unknown as Building).sins.length).toBe(1);
    expect(selected).toBe(after); // exact NEW object, never the old snapshot
  });
});

describe("LABEL LOD hysteresis (dead-band)", () => {
  it("just below IN keeps no label; >= IN creates; between OUT and IN holds; < OUT destroys", () => {
    const { fake } = makeFakeRenderer();
    // Create the node zoomed OUT so attachBuildingDynamics seeds NO label.
    (fake as unknown as { viewport: { scale: { x: number } } }).viewport.scale.x = 0.3;
    const node = create(fake, mkBuilding()) as unknown as { label: unknown };
    expect(node.label).toBeNull();

    // 0.61 — just BELOW LOD_LABELS_IN (0.62): no create, no label yet.
    driveCulling(fake, 0.61);
    expect(node.label).toBeNull();

    // 0.62 — at LOD_LABELS_IN: label created.
    driveCulling(fake, 0.62);
    expect(node.label).not.toBeNull();
    const heldLabel = node.label;

    // 0.59 — in the dead-band (OUT 0.58 ≤ scale < IN 0.62): HELD, same object.
    driveCulling(fake, 0.59);
    expect(node.label).toBe(heldLabel);

    // 0.57 — below LOD_LABELS_OUT (0.58): destroyed + detached.
    driveCulling(fake, 0.57);
    expect(node.label).toBeNull();
  });
});

describe("REMOVED building keeps the shared texture", () => {
  it("destroyBuildingNode destroys the node but NOT the shared variant texture", () => {
    const { fake } = makeFakeRenderer();
    // Two siblings of the same variant share the texture.
    const a = create(fake, mkBuilding({ fileId: "f1", coords: { x: 0, y: 0 } }));
    const b = create(fake, mkBuilding({ fileId: "f2", coords: { x: 2, y: 0 } }));
    const sharedTex = a.bodySprite.texture;
    const sharedShadow = a.shadow.texture;

    destroy(fake, a);

    // The removed node's container is destroyed...
    expect(a.container.destroyed).toBe(true);
    // ...but the SHARED texture survives (sibling b still uses it, and the atlas
    // owns it). texture:false on the sprite destroy keeps it alive.
    expect(sharedTex.destroyed).toBe(false);
    expect(sharedShadow.destroyed).toBe(false);
    expect(b.bodySprite.texture).toBe(sharedTex);
    expect(b.bodySprite.texture.destroyed).toBe(false);
  });
});
