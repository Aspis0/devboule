// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import { Container } from "pixi.js";
import { ExternalServiceLayer } from "./ExternalServiceLayer";
import type { ExternalService } from "../../types/city";
import { cartToIso } from "./iso";

function monument(over: Partial<ExternalService> = {}): ExternalService {
  return {
    serviceId: "era-1",
    provider: "monument",
    type: "parthenon",
    name: "Era 1: 10 files, 0 disasters",
    status: "running",
    coords: { x: 0, y: 0 },
    spawnable: false,
    ...over,
  };
}

describe("ExternalServiceLayer — monument-only", () => {
  it("renders a monument and reports placedCount", () => {
    const root = new Container();
    const layer = new ExternalServiceLayer(root);
    layer.setServices([monument()]);
    expect(layer.placedCount).toBe(1);
    expect(root.children.length).toBe(1);
  });

  it("ignores non-monument (legacy cloud) external services", () => {
    const root = new Container();
    const layer = new ExternalServiceLayer(root);
    layer.setServices([
      monument(),
      {
        serviceId: "scw-1",
        provider: "scaleway",
        type: "cpu_vm",
        name: "legacy-vm",
        status: "running",
        coords: { x: 5, y: 0 },
        spawnable: true,
      },
      {
        serviceId: "cf-1",
        provider: "cloudflare",
        type: "worker",
        name: "legacy-worker",
        status: "running",
        coords: { x: 6, y: 0 },
        spawnable: false,
      },
    ]);
    expect(layer.placedCount).toBe(1);
  });

  it("fires onSelect when a monument is clicked", () => {
    const root = new Container();
    let clicked: ExternalService | null = null;
    const layer = new ExternalServiceLayer(root, (s) => {
      clicked = s;
    });
    const m = monument({ serviceId: "era-click" });
    layer.setServices([m]);
    const node = root.children[0] as Container;
    node.emit("pointertap", { stopPropagation: () => {} } as never);
    // `clicked` is mutated inside the onSelect closure, which TS control-flow analysis
    // does not track — re-widen to the declared type before reading.
    expect((clicked as ExternalService | null)?.serviceId).toBe("era-click");
  });

  it("monuments stay visible under LOD on; hide when LOD off", () => {
    const root = new Container();
    const layer = new ExternalServiceLayer(root);
    layer.setServices([monument()]);
    expect(root.children[0].visible).toBe(true);
    layer.setLodVisible(false);
    expect(root.children[0].visible).toBe(false);
    layer.setLodVisible(true);
    expect(root.children[0].visible).toBe(true);
  });

  it("reconciles: remove monument on setServices empty", () => {
    const root = new Container();
    const layer = new ExternalServiceLayer(root);
    layer.setServices([monument()]);
    expect(layer.placedCount).toBe(1);
    layer.setServices([]);
    expect(layer.placedCount).toBe(0);
    expect(root.children.length).toBe(0);
  });

  it("places monument at backend coords iso", () => {
    const root = new Container();
    const layer = new ExternalServiceLayer(root);
    const m = monument({ coords: { x: 4, y: 2 } });
    layer.setServices([m]);
    const iso = cartToIso(4, 2);
    const node = root.children[0] as Container;
    expect(node.x).toBeCloseTo(iso.x, 5);
    expect(node.y).toBeCloseTo(iso.y, 5);
  });

  it("clear tears down all monuments", () => {
    const root = new Container();
    const layer = new ExternalServiceLayer(root);
    layer.setServices([
      monument({ serviceId: "a" }),
      monument({ serviceId: "b", type: "colossus", coords: { x: 1, y: 0 } }),
    ]);
    expect(layer.placedCount).toBe(2);
    layer.clear();
    expect(layer.placedCount).toBe(0);
    expect(root.children.length).toBe(0);
  });
});
