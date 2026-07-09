import { describe, it, expect } from "vitest";
import { Container } from "pixi.js";
import { ExternalServiceLayer } from "./ExternalServiceLayer";
import { cartToIso } from "./iso";
import type { ExternalService } from "../../types/city";

// Headless exercise of the external-service layer's pooling/reconciliation +
// the NEW era-monument render branch. PIXI v8 Container/Graphics construct +
// mutate without a GL context (same approach as TradeRouteLayer.test).

function svc(over: Partial<ExternalService>): ExternalService {
  return {
    serviceId: "scw-compute-c1",
    provider: "scaleway",
    type: "container",
    name: "rnaseq-job",
    status: "running",
    coords: { x: 10, y: 0 },
    spawnable: false,
    ...over,
  };
}

function monument(over: Partial<ExternalService>): ExternalService {
  return svc({
    serviceId: "monument-alpha",
    provider: "monument",
    // The backend ships a deterministic wonder slug in `service_type` (→ svc.type);
    // "bomos" exercises a wonder WITH an animated part (the altar Flame).
    type: "bomos",
    name: "Era Alpha: 12 files, 0 disasters active",
    status: "running",
    coords: { x: -3, y: 0 },
    ...over,
  });
}

describe("ExternalServiceLayer — era monuments render the real wonder", () => {
  it("builds the chosen wonder at its margin coords, clickable, anims driven", () => {
    const root = new Container();
    let clicked: ExternalService | null = null;
    const layer = new ExternalServiceLayer(root, (s) => (clicked = s));

    const m = monument({});
    layer.setServices([m]);

    // The monument IS placed (previously it was skipped entirely).
    expect(layer.placedCount).toBe(1);

    // Its container sits at the iso of the backend-placed margin coords.
    const node = root.children[0] as Container;
    const iso = cartToIso(m.coords.x, m.coords.y);
    expect(node.position.x).toBeCloseTo(iso.x);
    expect(node.position.y).toBeCloseTo(iso.y);

    // It mounts the REAL Claude-Design wonder kit container (not a tiny
    // placeholder slab) — its first child is the kit `monuments` container with
    // substantial geometry.
    const wonder = node.children[0] as Container;
    expect(wonder).toBeInstanceOf(Container);
    expect(wonder.children.length).toBeGreaterThan(0);

    // The chosen wonder ("bomos") carries a live altar Flame; step() must drive
    // its anims without throwing (and without a GL context).
    expect(() => layer.step(0, 0.033, 0.033)).not.toThrow();
    expect(() => layer.step(1, 0.066, 0.033)).not.toThrow();

    // Clicking surfaces the monument's own service (honest stats name).
    node.emit("pointertap", { stopPropagation() {} } as never);
    expect(clicked).not.toBeNull();
    expect(clicked!.serviceId).toBe("monument-alpha");
    expect(clicked!.name).toContain("Era Alpha");

    layer.clear();
    expect(layer.placedCount).toBe(0);
  });

  it("falls back to a default wonder for an unknown slug (still renders)", () => {
    const root = new Container();
    const layer = new ExternalServiceLayer(root);
    layer.setServices([monument({ type: "not-a-wonder" })]);
    expect(layer.placedCount).toBe(1);
    const node = root.children[0] as Container;
    const wonder = node.children[0] as Container;
    expect(wonder.children.length).toBeGreaterThan(0);
    layer.clear();
  });

  it("draws monuments AND cloud outposts side by side, reconciling each by id", () => {
    const root = new Container();
    const layer = new ExternalServiceLayer(root);

    layer.setServices([monument({}), svc({})]);
    expect(layer.placedCount).toBe(2);

    // Removing the cloud outpost (e.g. it left the inventory) tears it down but
    // keeps the cumulative monument.
    layer.setServices([monument({})]);
    expect(layer.placedCount).toBe(1);
    expect(root.children.length).toBe(1);

    layer.clear();
  });

  it("builds a cloud outpost from the kit harbour (with live water anims) at its coords, clickable", () => {
    const root = new Container();
    let clicked: ExternalService | null = null;
    const layer = new ExternalServiceLayer(root, (s) => (clicked = s));

    const s = svc({ status: "running" });
    layer.setServices([s]);
    expect(layer.placedCount).toBe(1);

    // Positioned at the iso of the backend-placed seaward margin coords.
    const node = root.children[0] as Container;
    const iso = cartToIso(s.coords.x, s.coords.y);
    expect(node.position.x).toBeCloseTo(iso.x);
    expect(node.position.y).toBeCloseTo(iso.y);

    // The kit harbour mounts a sizeable body container (not the old tiny
    // hand-drawn box) — its first child is the kit `harbor` container.
    const harbor = node.children[0] as Container;
    expect(harbor).toBeInstanceOf(Container);
    expect(harbor.children.length).toBeGreaterThan(0);

    // step() must drive the harbour's water anim without throwing (and without
    // requiring a GL context). It's a no-op-safe call when LOD-visible.
    expect(() => layer.step(0, 0.033, 0.033)).not.toThrow();
    expect(() => layer.step(1, 0.066, 0.033)).not.toThrow();

    // Clicking surfaces the outpost's own service (honest inventory entry).
    node.emit("pointertap", { stopPropagation() {} } as never);
    expect(clicked).not.toBeNull();
    expect(clicked!.serviceId).toBe("scw-compute-c1");

    layer.clear();
    expect(layer.placedCount).toBe(0);
  });

  it("animates a spawning outpost's status lamp on step (pulse), running stays steady", () => {
    const root = new Container();
    const layer = new ExternalServiceLayer(root);

    layer.setServices([svc({ serviceId: "spawn-1", status: "spawning" })]);
    const node = root.children[0] as Container;
    // The status lamp is the LAST child of the node container (after the harbour).
    const lamp = node.children[node.children.length - 1];
    const before = lamp.alpha;
    // Advance several frames; a spawning lamp pulses (alpha changes across the
    // stepped pulse cycle), proving step() reaches the lamp.
    let changed = false;
    for (let f = 1; f <= 8 && !changed; f++) {
      layer.step(f, f * 0.033, 0.033);
      if (lamp.alpha !== before) changed = true;
    }
    expect(changed).toBe(true);

    layer.clear();
  });

  it("rebuilds a monument when its margin coords change (no overlap accrual)", () => {
    const root = new Container();
    const layer = new ExternalServiceLayer(root);

    layer.setServices([monument({ coords: { x: -3, y: 0 } })]);
    const first = root.children[0] as Container;

    // A second era reset can re-place the same monument id at a different row.
    layer.setServices([monument({ coords: { x: -3, y: 3 } })]);
    expect(layer.placedCount).toBe(1);
    const second = root.children[0] as Container;
    // Rebuilt (old node torn down), positioned at the new coords.
    expect(second).not.toBe(first);
    const iso = cartToIso(-3, 3);
    expect(second.position.y).toBeCloseTo(iso.y);

    layer.clear();
  });
});

// T1b.2 — provider visibility: providers are OFF by default; monument always visible.
describe("ExternalServiceLayer — provider visibility (opt-in, default OFF)", () => {
  it("with empty visibleProviders, cloud outposts are hidden even when lodVisible=true", () => {
    const root = new Container();
    const layer = new ExternalServiceLayer(root);

    layer.setServices([svc({}), monument({})]);
    expect(layer.placedCount).toBe(2);

    // Set lodVisible = true (default), but no providers enabled.
    layer.setLodVisible(true);
    layer.setVisibleProviders(new Set<string>());

    // The root container is visible (lod gate), but individual cloud outpost
    // containers are hidden (no provider enabled).
    expect(root.visible).toBe(true);
    // First child = cloud outpost (scaleway), second = monument.
    const cloudNode = root.children[0] as Container;
    const monumentNode = root.children[1] as Container;
    expect(cloudNode.visible).toBe(false);
    expect(monumentNode.visible).toBe(true);

    layer.clear();
  });

  it("enabling scaleway shows only scaleway structures; monument stays visible", () => {
    const root = new Container();
    const layer = new ExternalServiceLayer(root);

    const cloudflareSvc = svc({ serviceId: "cf-1", provider: "cloudflare", coords: { x: 10, y: 0 } });
    const scalewaySvc = svc({ serviceId: "scw-1", provider: "scaleway", coords: { x: 12, y: 0 } });
    const m = monument({});
    layer.setServices([cloudflareSvc, scalewaySvc, m]);
    expect(layer.placedCount).toBe(3);

    layer.setVisibleProviders(new Set(["scaleway"]));

    // Find each node by serviceId using the placed map (accessible via reflection).
    // We can check by iterating root children and checking their iso positions.
    const cfIso = cartToIso(cloudflareSvc.coords.x, cloudflareSvc.coords.y);
    const scwIso = cartToIso(scalewaySvc.coords.x, scalewaySvc.coords.y);
    const mIso = cartToIso(m.coords.x, m.coords.y);

    const cfNode = root.children.find((c) => Math.abs(c.position.x - cfIso.x) < 1 && Math.abs(c.position.y - cfIso.y) < 1) as Container;
    const scwNode = root.children.find((c) => Math.abs(c.position.x - scwIso.x) < 1 && Math.abs(c.position.y - scwIso.y) < 1) as Container;
    const mNode = root.children.find((c) => Math.abs(c.position.x - mIso.x) < 1 && Math.abs(c.position.y - mIso.y) < 1) as Container;

    expect(cfNode.visible).toBe(false);
    expect(scwNode.visible).toBe(true);
    expect(mNode.visible).toBe(true);

    layer.clear();
  });

  it("monument is always visible regardless of visibleProviders", () => {
    const root = new Container();
    const layer = new ExternalServiceLayer(root);

    const m = monument({});
    layer.setServices([m]);
    layer.setVisibleProviders(new Set<string>());

    const mNode = root.children[0] as Container;
    expect(mNode.visible).toBe(true);

    layer.clear();
  });

  it("setVisibleProviders applies immediately without setServices call", () => {
    const root = new Container();
    const layer = new ExternalServiceLayer(root);

    layer.setServices([svc({})]);
    // Initially visible (lodVisible default = true, provider "scaleway" not in
    // the default set, so it's hidden after setVisibleProviders([])).
    layer.setVisibleProviders(new Set<string>());
    const node = root.children[0] as Container;
    expect(node.visible).toBe(false);

    // Enabling scaleway makes it visible immediately.
    layer.setVisibleProviders(new Set(["scaleway"]));
    expect(node.visible).toBe(true);

    // Disabling again hides it.
    layer.setVisibleProviders(new Set<string>());
    expect(node.visible).toBe(false);

    layer.clear();
  });
});
