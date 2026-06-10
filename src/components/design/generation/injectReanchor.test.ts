// @vitest-environment jsdom
//
// Proves the WIRING: `applyGeneration`'s re-anchored project is exactly what the
// direct-DOM canvas renders from (manifest placement + per-id component markup), so
// the deterministic re-anchoring drives the canvas. A regeneration that drops/
// renames ids must keep survivor placement at its original id, mint stable ids for
// genuinely new nodes, and drop removed ones — with no duplicates.
//
// (Formerly asserted via the retired Path-B `injectNodes` DOM path; the canvas now
// renders directly from the project, so these assertions read the project the
// pipeline returns — the single source of truth the canvas maps to host divs.)

import { describe, it, expect } from "vitest";
import { applyGeneration } from "./pipeline";
import type { DesignProject } from "../../../types/design";

type Project = DesignProject;

function seededProject(): Project {
  return {
    meta: {
      schemaVersion: 1,
      id: "p",
      name: "t",
      createdAt: "1970-01-01T00:00:00Z",
      updatedAt: "1970-01-01T00:00:00Z",
      canvas: { w: 1440, h: 1024, grid: 8 },
      nodeOrder: ["hero", "cta"],
    },
    manifest: {
      schemaVersion: 1,
      nodes: {
        hero: { x: 500, y: 300, z: 9, w: 420, h: "auto", kind: "html" },
        cta: { x: 80, y: 80, z: 2, w: 160, h: "auto", kind: "html" },
      },
    },
    components: {
      hero: '<section data-node-id="hero"><h1>Hero</h1></section>',
      cta: '<button data-node-id="cta">Go</button>',
    },
  };
}

const HERO_SHAPE = {
  tag: "section",
  dataNodeId: "hero",
  attrs: { "data-node-id": "hero" },
  children: [{ tag: "h1", attrs: {}, children: [], text: "Hero" }],
  text: "Hero",
};
const CTA_SHAPE = {
  tag: "button",
  dataNodeId: "cta",
  attrs: { "data-node-id": "cta" },
  children: [],
  text: "Go",
};

describe("reanchored generation output feeds the canvas project", () => {
  it("keeps survivor id + placement across an id-dropping regen", () => {
    // Regenerate with ids DROPPED but same structure: pipeline re-anchors them.
    const { project } = applyGeneration(
      seededProject(),
      "<section><h1>Hero v2</h1></section><button>Go v2</button>",
      { prevShapes: { hero: HERO_SHAPE, cta: CTA_SHAPE } },
    );

    // Survivor placement preserved at its ORIGINAL id (not a fresh minted one).
    expect(project.manifest.nodes.hero).toMatchObject({ x: 500, y: 300 });
    expect(project.components.hero).toContain("Hero v2");

    // Exactly the two survivors — no fresh-id ghost, no duplicates.
    expect(Object.keys(project.manifest.nodes).sort()).toEqual(["cta", "hero"]);
  });

  it("is stable: re-running the same generation keeps exactly the two nodes", () => {
    const markup =
      '<section data-node-id="hero"><h1>Hero</h1></section><button data-node-id="cta">Go</button>';
    const first = applyGeneration(seededProject(), markup, {
      prevShapes: { hero: HERO_SHAPE, cta: CTA_SHAPE },
    }).project;
    const second = applyGeneration(first, markup, {
      prevShapes: { hero: HERO_SHAPE, cta: CTA_SHAPE },
    }).project;
    expect(Object.keys(second.manifest.nodes).sort()).toEqual(["cta", "hero"]);
  });

  it("mints a new node and drops a removed one", () => {
    // hero survives, cta removed, a new footer added.
    const { project, newIds } = applyGeneration(
      seededProject(),
      '<section data-node-id="hero"><h1>Hero</h1></section><footer>New</footer>',
      { prevShapes: { hero: HERO_SHAPE, cta: CTA_SHAPE } },
    );

    expect(project.manifest.nodes.hero).toBeTruthy();
    expect(project.manifest.nodes.cta).toBeUndefined();
    expect(newIds).toHaveLength(1);
    expect(project.manifest.nodes[newIds[0]]).toBeTruthy();
    expect(Object.keys(project.manifest.nodes)).toHaveLength(2);
  });
});
