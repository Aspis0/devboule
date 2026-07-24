// renderProfile.ts — hardware-adaptive tier policy (Phase B2c). PURE unit tests:
//   - profileFor truth table: discrete RTX-4050-like → rich; integrated 8GB shared
//     low-vram → lean; tiny (low core / low vram) → minimal; null → middle (lean).
//   - thresholds MONOTONIC: minimal >= lean >= rich for every LOD threshold.
//   - the safe-default contract: unprobed / NaN / unknown inputs never land on the
//     most demanding tier.

import { describe, it, expect } from "vitest";
import {
  profileFor,
  RENDER_PROFILES,
  type HardwareInfo,
  type RenderTier,
} from "./renderProfile";

function hw(over: Partial<HardwareInfo> = {}): HardwareInfo {
  return {
    cpuCores: 16,
    ramTotalGb: 32,
    ramAvailableGb: 16,
    gpuName: "NVIDIA GeForce RTX 4050",
    vramGb: 6,
    gpuKind: "discrete",
    ...over,
  };
}

describe("profileFor — tier truth table", () => {
  it("a discrete card with >=4GB VRAM and >=8 cores → rich (this box: RTX 4050, 22 cores)", () => {
    // The dev box: 22 cores, RTX 4050, ~5.78GB VRAM, discrete.
    const p = profileFor(
      hw({ cpuCores: 22, gpuName: "NVIDIA GeForce RTX 4050 Laptop GPU", vramGb: 5.78 }),
    );
    expect(p.tier).toBe("rich");
    expect(p.preloadRing).toBe(2);
    expect(p.atlasResolutionCap).toBe(2);
    expect(p.buildingVariantSaltMax).toBe(4);
    expect(p.antialias).toBe(true);
  });

  it("an integrated GPU with shared/low VRAM → lean", () => {
    const p = profileFor(
      hw({ gpuKind: "integrated", gpuName: "Intel UHD Graphics 770", vramGb: null, cpuCores: 12 }),
    );
    expect(p.tier).toBe("lean");
    expect(p.preloadRing).toBe(1);
    expect(p.atlasResolutionCap).toBe(1);
    expect(p.buildingVariantSaltMax).toBe(2);
    expect(p.antialias).toBe(false);
  });

  it("a discrete card with <4GB VRAM (but above the minimal floor) → lean", () => {
    const p = profileFor(hw({ gpuKind: "discrete", vramGb: 2, cpuCores: 12 }));
    expect(p.tier).toBe("lean");
  });

  it("a capable CPU but UNKNOWN GPU → lean (never rich without a discrete read)", () => {
    const p = profileFor(
      hw({ gpuKind: "unknown", gpuName: "unknown", vramGb: null, cpuCores: 16 }),
    );
    expect(p.tier).toBe("lean");
  });

  it("a tiny box (<=4 cores) → minimal regardless of GPU", () => {
    const p = profileFor(hw({ cpuCores: 4 }));
    expect(p.tier).toBe("minimal");
    expect(p.preloadRing).toBe(0);
    expect(p.buildingVariantSaltMax).toBe(1);
    expect(p.maxAmbientWalkers).toBeLessThanOrEqual(RENDER_PROFILES.lean.maxAmbientWalkers);
  });

  it("a tiny known VRAM (<1.5GB) → minimal", () => {
    const p = profileFor(hw({ gpuKind: "discrete", vramGb: 1, cpuCores: 16 }));
    expect(p.tier).toBe("minimal");
  });

  it("the floor wins over rich: a discrete 8GB card on a 4-core CPU → minimal", () => {
    const p = profileFor(hw({ gpuKind: "discrete", vramGb: 8, cpuCores: 4 }));
    expect(p.tier).toBe("minimal");
  });
});

describe("profileFor — safe default (null / unprobed)", () => {
  it("null hardware → the MIDDLE tier (lean), never rich and never minimal", () => {
    const p = profileFor(null);
    expect(p.tier).toBe("lean");
    expect(p).toBe(RENDER_PROFILES.lean);
  });

  it("NaN core count → not rich (degrades to the floor or middle, never the richest)", () => {
    const p = profileFor(hw({ cpuCores: Number.NaN }));
    expect(p.tier).not.toBe("rich");
  });

  it("a NaN/garbage VRAM is treated as no-dedicated (lean), not as tiny-VRAM minimal", () => {
    const p = profileFor(
      hw({ gpuKind: "integrated", vramGb: Number.NaN, cpuCores: 12 }),
    );
    expect(p.tier).toBe("lean");
  });
});

describe("LOD thresholds are MONOTONIC across tiers (minimal >= lean >= rich)", () => {
  const order: RenderTier[] = ["rich", "lean", "minimal"];
  const keys = ["lodLabelsIn", "lodLabelsOut", "lodDetails", "lodAgents"] as const;

  for (const key of keys) {
    it(`${key} is non-decreasing rich → lean → minimal`, () => {
      for (let i = 1; i < order.length; i++) {
        const lower = RENDER_PROFILES[order[i - 1]][key];
        const higher = RENDER_PROFILES[order[i]][key];
        expect(higher).toBeGreaterThanOrEqual(lower);
      }
    });
  }

  it("every tier keeps a non-empty label dead-band (lodLabelsOut < lodLabelsIn)", () => {
    for (const t of order) {
      const p = RENDER_PROFILES[t];
      expect(p.lodLabelsOut).toBeLessThan(p.lodLabelsIn);
    }
  });

  it("preloadRing is non-increasing rich → lean → minimal (weaker = less preload)", () => {
    expect(RENDER_PROFILES.rich.preloadRing).toBeGreaterThanOrEqual(
      RENDER_PROFILES.lean.preloadRing,
    );
    expect(RENDER_PROFILES.lean.preloadRing).toBeGreaterThanOrEqual(
      RENDER_PROFILES.minimal.preloadRing,
    );
  });

  it("maxAmbientWalkers is non-increasing rich → lean → minimal", () => {
    expect(RENDER_PROFILES.rich.maxAmbientWalkers).toBeGreaterThanOrEqual(
      RENDER_PROFILES.lean.maxAmbientWalkers,
    );
    expect(RENDER_PROFILES.lean.maxAmbientWalkers).toBeGreaterThanOrEqual(
      RENDER_PROFILES.minimal.maxAmbientWalkers,
    );
  });
});

describe("profileFor — Apple Silicon unified-memory RICH classification", () => {
  it("M1 Max (10 cores, 64GB, integrated) → rich", () => {
    const p = profileFor(
      hw({
        cpuCores: 10,
        ramTotalGb: 64,
        ramAvailableGb: 40,
        gpuName: "Apple M1 Max",
        vramGb: null,
        gpuKind: "integrated",
      }),
    );
    expect(p.tier).toBe("rich");
  });

  it("M4 Pro (12 cores, 48GB, integrated) → rich", () => {
    const p = profileFor(
      hw({
        cpuCores: 12,
        ramTotalGb: 48,
        gpuName: "Apple M4 Pro",
        vramGb: null,
        gpuKind: "integrated",
      }),
    );
    expect(p.tier).toBe("rich");
  });

  it("M1 (8 cores, 16GB, integrated) → lean (RAM below 32)", () => {
    const p = profileFor(
      hw({
        cpuCores: 8,
        ramTotalGb: 16,
        gpuName: "Apple M1",
        vramGb: null,
        gpuKind: "integrated",
      }),
    );
    expect(p.tier).toBe("lean");
  });

  it("M2 (4 cores, 32GB, integrated) → minimal (core floor wins)", () => {
    const p = profileFor(
      hw({
        cpuCores: 4,
        ramTotalGb: 32,
        gpuName: "Apple M2",
        vramGb: null,
        gpuKind: "integrated",
      }),
    );
    expect(p.tier).toBe("minimal");
  });

  it("Intel UHD (16 cores, 64GB, integrated) → lean (not Apple)", () => {
    const p = profileFor(
      hw({
        cpuCores: 16,
        ramTotalGb: 64,
        gpuName: "Intel(R) UHD Graphics 770",
        vramGb: null,
        gpuKind: "integrated",
      }),
    );
    expect(p.tier).toBe("lean");
  });

  it("M3 (10 cores, 64GB, gpuKind unknown) → lean (kind must be integrated)", () => {
    const p = profileFor(
      hw({
        cpuCores: 10,
        ramTotalGb: 64,
        gpuName: "Apple M3",
        vramGb: null,
        gpuKind: "unknown",
      }),
    );
    expect(p.tier).toBe("lean");
  });

  it("Fake Apple M1 Max (not prefix-matched) → lean (regex anchor ^)", () => {
    const p = profileFor(
      hw({
        cpuCores: 10,
        ramTotalGb: 64,
        gpuName: "Fake Apple M1 Max",
        vramGb: null,
        gpuKind: "integrated",
      }),
    );
    expect(p.tier).toBe("lean");
  });

  it("M1 Max (10 cores, Infinity RAM, integrated) → lean (RAM sanitisation)", () => {
    const p = profileFor(
      hw({
        cpuCores: 10,
        ramTotalGb: Infinity,
        gpuName: "Apple M1 Max",
        vramGb: null,
        gpuKind: "integrated",
      }),
    );
    expect(p.tier).toBe("lean");
  });
});
