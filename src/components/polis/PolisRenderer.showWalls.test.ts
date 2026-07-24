// Aesthetic showWalls pref: setShowWalls gates districtWallsLayer.visible
// together with the existing LOD_WALLS zoom condition. Pure visibility —
// no rebuild. Headless Object.create harness (ambientSync-style).

import { describe, it, expect, vi } from "vitest";
import { LOD_WALLS } from "./lod";

const { PolisRenderer } = await import("./PolisRenderer");

type AnyRec = Record<string, unknown>;

function makeHarness(scale: number) {
  const fake = Object.create(
    PolisRenderer.prototype,
  ) as InstanceType<typeof PolisRenderer>;
  const set = (k: string, v: unknown) => {
    (fake as unknown as AnyRec)[k] = v;
  };

  const layer = { visible: true };
  set("showWalls", true);
  set("districtWallsLayer", layer);
  set("viewport", { scale: { x: scale } });

  return { fake, layer };
}

describe("PolisRenderer setShowWalls", () => {
  it("hides walls when showWalls is false even above LOD_WALLS", () => {
    const { fake, layer } = makeHarness(LOD_WALLS + 0.5);
    expect(layer.visible).toBe(true);

    fake.setShowWalls(false);

    expect(layer.visible).toBe(false);
  });

  it("shows walls when showWalls is true and scale >= LOD_WALLS", () => {
    const { fake, layer } = makeHarness(LOD_WALLS + 0.1);
    layer.visible = false;
    // Seed private flag false so setShowWalls is not a no-op.
    (fake as unknown as AnyRec).showWalls = false;

    fake.setShowWalls(true);

    expect(layer.visible).toBe(true);
  });

  it("keeps walls hidden when zoom is below LOD_WALLS even if showWalls true", () => {
    const { fake, layer } = makeHarness(LOD_WALLS - 0.05);
    (fake as unknown as AnyRec).showWalls = false;
    layer.visible = true;

    fake.setShowWalls(true);

    expect(layer.visible).toBe(false);
  });

  it("is a no-op when value is unchanged (does not thrash visibility)", () => {
    const { fake, layer } = makeHarness(LOD_WALLS + 1);
    // Force a wrong layer state; no-op must NOT re-apply.
    layer.visible = false;
    const applySpy = vi.spyOn(
      fake as unknown as { applyWallsVisibility: () => void },
      "applyWallsVisibility",
    );

    fake.setShowWalls(true); // already true

    expect(applySpy).not.toHaveBeenCalled();
    expect(layer.visible).toBe(false); // untouched
    applySpy.mockRestore();
  });
});
