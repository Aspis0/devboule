// @vitest-environment jsdom
//
// Proves the WIRING: the LIVE inject path (injectNodes) renders the project that
// `applyGeneration` produced, so the deterministic re-anchoring actually drives
// the canvas. A regeneration that drops/renames ids must keep survivor host
// elements at their original id + placement (id stability across the canvas),
// and injection must stay idempotent (no duplicate host divs).

import { describe, it, expect } from "vitest";
import { applyGeneration } from "./pipeline";
import {
  buildShellHtml,
  injectNodes,
  CANVAS_ROOT_ID,
  NODE_ID_ATTR,
} from "../iframeInject";
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

describe("injectNodes reflects reanchored generation output", () => {
  it("keeps survivor host id + placement across an id-dropping regen", () => {
    document.documentElement.innerHTML = buildShellHtml();
    const root = document.getElementById(CANVAS_ROOT_ID) as HTMLElement;

    // Regenerate with ids DROPPED but same structure: pipeline re-anchors them.
    const { project } = applyGeneration(
      seededProject(),
      "<section><h1>Hero v2</h1></section><button>Go v2</button>",
      { prevShapes: { hero: HERO_SHAPE, cta: CTA_SHAPE } },
    );

    // The live inject path consumes the re-anchored project.
    injectNodes(document, project);

    const heroHost = root.querySelector(
      `:scope > [${NODE_ID_ATTR}="hero"]`,
    ) as HTMLElement;
    expect(heroHost).toBeTruthy(); // re-anchored, not a fresh minted id
    expect(heroHost.getAttribute("style")).toContain("left:500px");
    expect(heroHost.getAttribute("style")).toContain("top:300px");
    expect(heroHost.innerHTML).toContain("Hero v2");

    // Exactly the two survivor hosts — no fresh-id ghost, no duplicates.
    const hosts = root.querySelectorAll(`:scope > [${NODE_ID_ATTR}]`);
    expect(hosts).toHaveLength(2);
  });

  it("is idempotent: re-injecting the same generated project does not duplicate hosts", () => {
    document.documentElement.innerHTML = buildShellHtml();
    const root = document.getElementById(CANVAS_ROOT_ID) as HTMLElement;

    const { project } = applyGeneration(
      seededProject(),
      '<section data-node-id="hero"><h1>Hero</h1></section><button data-node-id="cta">Go</button>',
      { prevShapes: { hero: HERO_SHAPE, cta: CTA_SHAPE } },
    );
    injectNodes(document, project);
    injectNodes(document, project);
    expect(root.querySelectorAll(`:scope > [${NODE_ID_ATTR}]`)).toHaveLength(2);
  });

  it("renders a newly minted node and drops a removed one", () => {
    document.documentElement.innerHTML = buildShellHtml();
    const root = document.getElementById(CANVAS_ROOT_ID) as HTMLElement;

    // hero survives, cta removed, a new footer added.
    const { project, newIds } = applyGeneration(
      seededProject(),
      '<section data-node-id="hero"><h1>Hero</h1></section><footer>New</footer>',
      { prevShapes: { hero: HERO_SHAPE, cta: CTA_SHAPE } },
    );
    injectNodes(document, project);

    expect(
      root.querySelector(`:scope > [${NODE_ID_ATTR}="hero"]`),
    ).toBeTruthy();
    expect(root.querySelector(`:scope > [${NODE_ID_ATTR}="cta"]`)).toBeNull();
    expect(newIds).toHaveLength(1);
    expect(
      root.querySelector(`:scope > [${NODE_ID_ATTR}="${newIds[0]}"]`),
    ).toBeTruthy();
    expect(root.querySelectorAll(`:scope > [${NODE_ID_ATTR}]`)).toHaveLength(2);
  });
});
