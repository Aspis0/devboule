// @vitest-environment jsdom
//
// Tests for the surviving THIN DOM-parse layer (the Path-B iframe shell + inject
// half was retired with the direct-DOM canvas). Correctness logic lives in the
// pure engine; here we verify the DOMParser -> ParsedNode wrapper used by the
// generation pipeline.

import { describe, it, expect } from "vitest";
import {
  parseTopLevelNodes,
  parseTopLevelNodesWithMarkup,
  NODE_ID_ATTR,
} from "./iframeInject";

describe("parseTopLevelNodes", () => {
  it("parses top-level elements with their data-node-id and structure", () => {
    const nodes = parseTopLevelNodes(
      '<section data-node-id="hero"><h1>Hi</h1></section><button data-node-id="cta">Go</button>',
    );
    expect(nodes).toHaveLength(2);
    expect(nodes[0].tag).toBe("section");
    expect(nodes[0].dataNodeId).toBe("hero");
    expect(nodes[0].children[0].tag).toBe("h1");
    expect(nodes[1].tag).toBe("button");
    expect(nodes[1].dataNodeId).toBe("cta");
  });

  it("returns [] for empty markup", () => {
    expect(parseTopLevelNodes("")).toEqual([]);
  });
});

describe("parseTopLevelNodesWithMarkup", () => {
  it("returns each top-level element's verbatim outer markup alongside its shape", () => {
    const parsed = parseTopLevelNodesWithMarkup(
      '<section data-node-id="hero"><h1>Hi</h1></section>',
    );
    expect(parsed).toHaveLength(1);
    expect(parsed[0].node.tag).toBe("section");
    expect(parsed[0].node.dataNodeId).toBe("hero");
    expect(parsed[0].markup).toContain(`${NODE_ID_ATTR}="hero"`);
    expect(parsed[0].markup).toContain("<h1>Hi</h1>");
  });
});
