// ExternalServiceLayer — renders era MONUMENTS only (`provider === "monument"`).
//
// HONESTY RULE: every node mirrors a REAL entry in `city.externalServices` with
// `provider === "monument"`. Prestige wonders are derived from REAL archived-era
// stats (backend era reset), never invented. Cloud-provider outposts (CF/SCW)
// were removed from the app; legacy JSON entries with other providers are
// filtered out and not drawn. An empty monument list draws nothing.
//
// The backend places each monument deterministically on the landward margin.
// This layer maps `coords` → iso and draws via `createMonument`.
//
// These are NOT buildings, NOT agents, NOT files: they live on their own layer,
// carry no file identity, and never participate in the road/agent graphs. They
// ARE clickable to surface an inspect card (honest era stats in `name`) —
// inspect-only, no secret on the wire.
//
// VISUAL: one of the 12 Claude-Design "Meraviglie" from `kitcd/monuments.ts`.
// The backend ships the wonder slug in `service_type` (frontend `svc.type`);
// `createMonument` looks up `MONUMENTS[slug]` and drives Flag/Flame/Beacon/Water
// anims off the step clock.
//
// PERFORMANCE: geometry is built ONCE per service in setServices. The step clock
// drives kit anims (VISIBLE-only). Pooled by serviceId; teardown uses
// `removeFromParent()` + `destroy({ children:true })`. LOD-gated when zoomed out.

import { Container, Graphics } from "pixi.js";
import type { ExternalService } from "../../types/city";
import { cartToIso, type IsoPoint } from "./iso";
import type { AnimInstance } from "./kitcd/anims";
import { MONUMENTS } from "./kitcd/monuments";

// Fallback wonder when an era marker carries an unknown/empty slug (defensive:
// the backend always emits a known MONUMENT_META slug). The Parthenon is the
// first wonder in canonical order — a sensible, recognizable default.
const DEFAULT_WONDER = "parthenon";

interface PlacedService {
  service: ExternalService;
  /** The whole monument container. */
  container: Container;
  /**
   * Invisible placeholder lamp so the pooled record shape stays uniform with
   * the historical PlacedService type (step only animated "spawning" lamps;
   * monuments never spawn).
   */
  lamp: Graphics;
  /** Stable non-pulsing marker for monuments. */
  status: string;
  /**
   * Kit wonder live anim instances (Flag / Flame / Beacon / Water) — driven by
   * the step clock via `update(t, dt)`. Part-nodes are children of `container`.
   */
  anims: AnimInstance[];
}

export class ExternalServiceLayer {
  private root: Container;
  private placed = new Map<string, PlacedService>();
  private lodVisible = true;
  private onSelect?: (service: ExternalService | null) => void;

  constructor(
    root: Container,
    onSelect?: (service: ExternalService | null) => void,
  ) {
    this.root = root;
    this.onSelect = onSelect;
  }

  /**
   * Reconcile placed monuments against the city's external services, keyed by
   * serviceId. Only `provider === "monument"` entries are drawn; all others are
   * ignored (legacy cloud outposts in saved CityState).
   * NEW → build; CHANGED → rebuild; REMOVED → tear down.
   */
  setServices(services: ExternalService[]): void {
    const seen = new Set<string>();

    for (const svc of services) {
      // Monument-only: skip legacy cloud outposts if present in old JSON.
      if (svc.provider !== "monument") continue;

      seen.add(svc.serviceId);

      const existing = this.placed.get(svc.serviceId);
      if (existing && !serviceChanged(existing.service, svc)) {
        continue;
      }
      if (existing) {
        this.destroyService(existing);
        this.placed.delete(svc.serviceId);
      }
      this.placed.set(svc.serviceId, this.createMonument(svc));
    }

    for (const [id, p] of this.placed) {
      if (!seen.has(id)) {
        this.destroyService(p);
        this.placed.delete(id);
      }
    }

    this.applyLodVisibility();
  }

  /** Number of monuments actually drawn. */
  get placedCount(): number {
    return this.placed.size;
  }

  /** LOD gate: hide the whole layer when zoomed out. */
  setLodVisible(visible: boolean): void {
    this.lodVisible = visible;
    this.applyLodVisibility();
  }

  private applyLodVisibility(): void {
    const lod = this.lodVisible;
    for (const p of this.placed.values()) {
      p.container.visible = lod;
    }
    this.root.visible = lod;
  }

  /**
   * Advance one STEP (shared 30fps clock). Drives kit wonder anims via
   * `update(t, dt)`. Skipped when LOD-hidden.
   */
  step(frame: number, t: number, dt: number): void {
    if (!this.lodVisible) return;
    void frame;
    for (const p of this.placed.values()) {
      const anims = p.anims;
      try {
        for (let i = 0; i < anims.length; i++) anims[i].update(t, dt);
      } catch {
        // swallow — a bad anim frame is cosmetic, never fatal to the layer
      }
    }
  }

  /** Tear down every monument (used by the renderer's clearScene + destroy). */
  clear(): void {
    for (const p of this.placed.values()) this.destroyService(p);
    this.placed.clear();
  }

  // -------------------------------------------------------------------------

  /**
   * Build an ERA MONUMENT marker — one of the 12 Claude-Design "Meraviglie".
   * Wonder slug comes from `svc.type` (backend `service_type`).
   */
  private createMonument(svc: ExternalService): PlacedService {
    const iso: IsoPoint = cartToIso(svc.coords.x, svc.coords.y);
    const container = new Container();
    container.position.set(iso.x, iso.y);
    container.eventMode = "static";
    container.cursor = "pointer";

    const slug =
      svc.type && MONUMENTS[svc.type] ? svc.type : DEFAULT_WONDER;
    const built = MONUMENTS[slug]({ outline: false });
    const wonder = built.container;
    container.addChild(wonder);

    // Invisible placeholder lamp (never drawn / never animated).
    const lamp = new Graphics();
    lamp.visible = false;
    container.addChild(lamp);

    container.on("pointertap", (e) => {
      e.stopPropagation();
      this.onSelect?.(svc);
    });

    container.visible = this.lodVisible;
    this.root.addChild(container);

    return {
      service: svc,
      container,
      lamp,
      status: "monument",
      anims: built.anims,
    };
  }

  private destroyService(p: PlacedService): void {
    p.container.removeAllListeners();
    p.container.removeFromParent();
    p.container.destroy({ children: true });
  }
}

/** True when a visual input changed and the node must be rebuilt. */
function serviceChanged(a: ExternalService, b: ExternalService): boolean {
  return (
    a.status !== b.status ||
    a.type !== b.type ||
    a.provider !== b.provider ||
    a.name !== b.name ||
    a.coords.x !== b.coords.x ||
    a.coords.y !== b.coords.y
  );
}
