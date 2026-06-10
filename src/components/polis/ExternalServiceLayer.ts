// ExternalServiceLayer — renders the project's LIVE cloud resources as small
// "harbour / cloud-outpost" structures at the seaward margin of the map ("the
// city meets the cloud / a harbour to the sea").
//
// HONESTY RULE: every node mirrors a REAL entry in `city.externalServices`,
// which the Rust backend populates ONLY from the already-synced provider
// inventory (Scaleway/Cloudflare) PLUS the era MONUMENTS (provider "monument") —
// prestige wonders derived from REAL archived-era stats, never invented. No node
// is fabricated; an empty list draws nothing. The backend places each entry
// deterministically OUTSIDE the building grid (cloud outposts on the seaward/east
// margin, monuments on the landward/west margin), so this layer simply maps
// `coords` → iso and draws: a cloud outpost via `createService`, an era monument
// (no status lamp) via `createMonument`.
//
// These are NOT buildings, NOT agents, NOT files: they live on their own layer,
// carry no file identity, and never participate in the road/agent graphs. They
// ARE clickable to surface an inspect card (provider / type / name / status) —
// inspect-only, honest, no secret (the backend never put one on the wire).
//
// VISUAL — ERA MONUMENT: an era marker renders one of the 12 Claude-Design
// "Meraviglie" (wonders) from the procedural `kitcd/monuments.ts` builders (NOT a
// placeholder). The backend picks the wonder deterministically by era and ships
// the slug in `service_type` (frontend `svc.type`); `createMonument` looks up
// `MONUMENTS[slug]`, mounts its `{container,anims}` at the margin iso, and drives
// its anims (Flag / Flame / Beacon / Water) off the step clock. The marker's data
// (the honest era stats in `name`) is unchanged — the wonder is purely the visual.
//
// VISUAL — CLOUD OUTPOST: reuses the CLAUDE DESIGN procedural kit's `harbor`
// builder (the same kit the real file-buildings use via `buildBuilding`) — a
// quay + harbour-master house + piers + animated WATER. We build it at a fixed
// level (HARBOR_LEVEL) since a cloud resource has no "size tier". On top we add:
//   - the F4 provider PENNANT (the same `buildPennant` livery flag the
//     file-buildings fly) so the tech provider reads at a glance, and
//   - a small functional STATUS LAMP overlay:
//       running  → lit (steady warm glow)
//       stopped  → dim (low, cool)
//       error    → red (steady alarm)
//       spawning → pulsing (stepped pulse, like the agent glow)
//
// PERFORMANCE / LIFECYCLE: the harbour geometry + pennant + lamp are built ONCE
// per service in setServices. The step clock drives the kit `anims` (the Water
// ripple) the SAME way PolisRenderer drives building anims — `anim.update(t, dt)`
// off the shared step clock, VISIBLE-only, and mutates the "spawning" lamp alpha;
// no other per-step allocation, no geometry rebuild for steady states. Pooled by
// serviceId and reconciled on each setServices call; every node is torn down with
// `removeFromParent()` + `destroy({ children:true })` (the L1/max-recall teardown
// discipline) — the kit container and its anim part-nodes are children of the
// node container, so they are freed with it (no separate anim disposal). LOD-gated
// like the trade-route porters (hidden when zoomed out).

import { Container, Graphics } from "pixi.js";
import type { ExternalService } from "../../types/city";
import { cartToIso, type IsoPoint } from "./iso";
import { PALETTE } from "./palette";
import { steppedPulse } from "./effects";
import { BUILDERS } from "./kitcd/buildings";
import type { AnimInstance } from "./kitcd/anims";
import { MONUMENTS } from "./kitcd/monuments";
import { buildPennant } from "./buildings";

// Fallback wonder when an era marker carries an unknown/empty slug (defensive:
// the backend always emits a known MONUMENT_META slug). The Parthenon is the
// first wonder in canonical order — a sensible, recognizable default.
const DEFAULT_WONDER = "parthenon";

// Cloud resources have no "size tier"; a fixed mid level gives a substantial but
// not monumental harbour (level 2 → a 3x3 quay with a pier + crane + water).
const HARBOR_LEVEL = 2;

// Status lamp colors (derived from PALETTE — palette discipline). Running uses
// the warm gold accent; error a terracotta-dark red; stopped/spawning reuse the
// gold at varying intensity (alpha carries the state, not a fresh hue).
const LAMP = {
  running: PALETTE.goldAccent,
  stopped: PALETTE.stoneDark,
  error: PALETTE.terracottaDark,
  spawning: PALETTE.goldAccent,
} as const;

// Steady lamp alphas per non-pulsing state.
const LAMP_ALPHA = {
  running: 0.95,
  stopped: 0.3,
  error: 0.9,
} as const;

// Spawning: a stepped pulse (mirrors the agent glow cadence) so a provisioning
// resource visibly "breathes" while it comes up.
const SPAWN_PULSE = [0.35, 0.6, 0.95, 0.6] as const;

interface PlacedService {
  service: ExternalService;
  /** The whole outpost container (kit harbour + pennant + lamp). */
  container: Container;
  /** The status lamp Graphics — its `alpha` is the only thing the step mutates. */
  lamp: Graphics;
  /** Normalized status, captured at build time (drives the step animation). */
  status: string;
  /**
   * The kit harbour's live anim instances (the Water ripple) — driven by the
   * step clock via `update(t, dt)`. Empty for monuments (no live geometry). The
   * anim part-nodes are children of `container`, so they are freed with it; this
   * array only holds references for the step driver, not for separate disposal.
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
   * Reconcile the placed outposts against the city's external services, keyed by
   * serviceId. NEW → build; CHANGED (status/type/name/coords differ) → rebuild;
   * REMOVED → tear down. Era monuments (provider "monument") take the wonder
   * build branch (`createMonument`); cloud outposts take `createService`. Both are
   * pooled, reconciled, and torn down through this same map.
   */
  setServices(services: ExternalService[]): void {
    const seen = new Set<string>();

    for (const svc of services) {
      seen.add(svc.serviceId);

      const existing = this.placed.get(svc.serviceId);
      if (existing && !serviceChanged(existing.service, svc)) {
        // Unchanged → leave the prebuilt node untouched.
        continue;
      }
      if (existing) {
        // CHANGED → tear down and rebuild (cheap; there are few services).
        this.destroyService(existing);
        this.placed.delete(svc.serviceId);
      }
      // Era monuments (provider "monument") are prestige wonders, not cloud
      // outposts — built by a different branch (no status lamp / livery), but
      // pooled + reconciled + torn down through the SAME map so they place at
      // their margin coords and clean up with the rest.
      this.placed.set(
        svc.serviceId,
        svc.provider === "monument"
          ? this.createMonument(svc)
          : this.createService(svc),
      );
    }

    // REMOVED → tear down.
    for (const [id, p] of this.placed) {
      if (!seen.has(id)) {
        this.destroyService(p);
        this.placed.delete(id);
      }
    }
  }

  /** Number of outposts actually drawn. */
  get placedCount(): number {
    return this.placed.size;
  }

  /** LOD gate: hide the whole layer when zoomed out (mirrors trade routes). */
  setLodVisible(visible: boolean): void {
    this.lodVisible = visible;
    this.root.visible = visible;
  }

  /**
   * Advance one STEP (shared 30fps clock). Two things happen here, both
   * allocation-free in steady state:
   *   1. the kit harbour's anims (the Water ripple) are advanced via
   *      `update(t, dt)` — the SAME convention PolisRenderer uses to drive
   *      building anims (`t` = running total seconds, `dt` = clamped step
   *      seconds). Monuments carry no anims, so their loop is a no-op.
   *   2. the pulsing "spawning" lamp alpha is stepped. Steady lamps
   *      (running/stopped/error) keep their fixed alpha from build time and are
   *      intentionally NOT rewritten — nothing else mutates them, so a per-step
   *      write would be pure churn.
   * The whole layer is skipped when LOD-hidden (no anim churn off-screen).
   */
  step(frame: number, t: number, dt: number): void {
    if (!this.lodVisible) return;
    for (const p of this.placed.values()) {
      // Drive the harbour water (and any future kit anim). Alloc-free: each
      // anim clears+redraws its own small Graphics, inherent to the kit art.
      const anims = p.anims;
      // Defensive: a single misbehaving kit anim must not freeze every OTHER
      // service's animation for the frame (one try/catch around the inner loop).
      try {
        for (let i = 0; i < anims.length; i++) anims[i].update(t, dt);
      } catch {
        // swallow — a bad anim frame is cosmetic, never fatal to the layer
      }
      if (p.status === "spawning") {
        p.lamp.alpha = steppedPulse(frame, SPAWN_PULSE, 2);
      }
    }
  }

  /** Tear down every outpost (used by the renderer's clearScene + destroy). */
  clear(): void {
    for (const p of this.placed.values()) this.destroyService(p);
    this.placed.clear();
  }

  // -------------------------------------------------------------------------

  /**
   * Build a CLOUD OUTPOST node from the CLAUDE DESIGN kit `harbor` builder (the
   * same procedural kit the real file-buildings use via `buildBuilding`), at a
   * fixed mid level (a cloud resource has no size tier). The kit container is the
   * node body; on top we add the F4 provider PENNANT (the livery flag the
   * file-buildings fly) and a functional STATUS LAMP. The harbour's live `anims`
   * (the Water ripple) are stored on the record and driven by `step()`. Placed at
   * the backend's seaward margin coords; clickable to surface the inspect card.
   */
  private createService(svc: ExternalService): PlacedService {
    const iso: IsoPoint = cartToIso(svc.coords.x, svc.coords.y);
    const container = new Container();
    container.position.set(iso.x, iso.y);
    container.eventMode = "static";
    container.cursor = "pointer";

    const status = normalizeStatus(svc.status);

    // --- Body: the CLAUDE DESIGN kit `harbor` builder (same kit the real
    // file-buildings use). A cloud resource has no size tier, so we build a
    // fixed mid level. `built.container` becomes the node body; the kit anchors
    // front-bottom at local (0,0), so positioning the node container at the iso
    // point lines the harbour up exactly as a real building would. The harbour's
    // live `anims` (the Water ripple) are returned to the step driver. ---
    const built = BUILDERS.harbor(HARBOR_LEVEL, { outline: false });
    const harbor = built.container;
    container.addChild(harbor);

    // Silhouette top (most-negative y) of the kit harbour, in local px — used to
    // plant the pennant just above the roof and the status lamp above that.
    const topY = built.container.getLocalBounds().y;

    // --- Provider livery: the F4 PENNANT (the same livery flag the real
    // file-buildings fly), planted on the harbour at the iso center column.
    // `null` (parented onto nothing) when the provider has no known livery, so
    // a provider-less outpost simply flies no flag — never a fabricated cue. ---
    const pennant = buildPennant(svc.provider, topY);
    if (pennant) harbor.addChild(pennant);

    // --- Status lamp: a small glowing dot above the harbour silhouette. Its
    // alpha is the only thing the step mutates (the spawning pulse). ---
    const lampY = topY - 6;
    const lamp = new Graphics();
    const lampColor = LAMP[status as keyof typeof LAMP] ?? LAMP.stopped;
    lamp.circle(0, lampY, 2.6).fill({ color: lampColor });
    // A soft halo so a "lit" lamp reads at a glance.
    lamp.circle(0, lampY, 4.5).fill({ color: lampColor, alpha: 0.35 });
    lamp.alpha =
      status === "spawning"
        ? SPAWN_PULSE[0]
        : (LAMP_ALPHA[status as keyof typeof LAMP_ALPHA] ?? LAMP_ALPHA.stopped);
    container.addChild(lamp);

    // Click → surface the inspect card (provider/type/name/status). The handler
    // stops propagation so the viewport background tap doesn't deselect it.
    container.on("pointertap", (e) => {
      e.stopPropagation();
      this.onSelect?.(svc);
    });

    container.visible = this.lodVisible;
    this.root.addChild(container);

    return { service: svc, container, lamp, status, anims: built.anims };
  }

  /**
   * Build an ERA MONUMENT marker — one of the 12 Claude-Design "Meraviglie"
   * (wonders), rendered from the procedural `kitcd/monuments.ts` builders (the
   * same primitive language the file-buildings use). The wonder is chosen by the
   * backend (deterministic era → wonder cycle) and carried in `svc.type`
   * (`service_type`); we look up `MONUMENTS[slug]` and fall back to a sensible
   * default for an unknown/empty slug. The kit container anchors front-bottom at
   * local (0,0), so positioning the node container at the iso point lines the
   * wonder up exactly as a real building would (same as the cloud harbour).
   *
   * Placed at the monument's margin coords (the backend puts it on the LANDWARD
   * edge, outside the grid + clear of the seaward cloud harbour). Its animated
   * parts (Flag for the Horologion vane, Flame for the Bōmos altar, Beacon for the
   * Kolossos torch, Water for the Zeus pool / Kolossos harbour) are returned to
   * the step driver and advanced via `update(t, dt)` — the SAME convention the
   * harbour Water uses; a wonder with no anim parts simply has an empty loop.
   *
   * NO status lamp (a wonder has no live state) — it carries an invisible
   * placeholder lamp so the pooled `PlacedService` shape stays uniform and the
   * step loop (which only touches "spawning" lamps) never animates it. Clickable →
   * surfaces the honest stats card (`name` = "Era X: N files, …" from the backend;
   * the inspect UI can title it with the wonder name from MONUMENT_META[slug]).
   */
  private createMonument(svc: ExternalService): PlacedService {
    const iso: IsoPoint = cartToIso(svc.coords.x, svc.coords.y);
    const container = new Container();
    container.position.set(iso.x, iso.y);
    container.eventMode = "static";
    container.cursor = "pointer";

    // Resolve the wonder builder from the backend-chosen slug (svc.type =
    // service_type). Unknown/empty → DEFAULT_WONDER (defensive; the backend always
    // emits a known MONUMENT_META slug).
    const slug =
      svc.type && MONUMENTS[svc.type] ? svc.type : DEFAULT_WONDER;
    const built = MONUMENTS[slug]({ outline: false });
    const wonder = built.container;
    container.addChild(wonder);

    // Invisible placeholder lamp so the pooled record shape is uniform; never
    // animated (status is not "spawning") and never drawn visibly.
    const lamp = new Graphics();
    lamp.visible = false;
    container.addChild(lamp);

    container.on("pointertap", (e) => {
      e.stopPropagation();
      this.onSelect?.(svc);
    });

    container.visible = this.lodVisible;
    this.root.addChild(container);

    // A wonder has no live status; record a stable non-pulsing marker so the step
    // loop's `status === "spawning"` check never animates the lamp. Its kit `anims`
    // (Flag / Flame / Beacon / Water) ARE driven by the step loop. The anim
    // part-nodes are children of `wonder` (→ of `container`), so they are freed
    // with the container on teardown — no separate anim disposal, no leak.
    return { service: svc, container, lamp, status: "monument", anims: built.anims };
  }

  private destroyService(p: PlacedService): void {
    // Drop the click handler refs, detach, and destroy the whole subtree (the kit
    // harbour container — including its Water anim part-nodes — the pennant and
    // the lamp are all children) — no separate anim disposal, no leak.
    p.container.removeAllListeners();
    p.container.removeFromParent();
    p.container.destroy({ children: true });
  }
}

/** Normalize an open-union status string to the 4 known states (defensive: the
 *  backend already emits these, but an unknown value degrades to "stopped"). */
function normalizeStatus(status: string): string {
  switch (status) {
    case "running":
    case "stopped":
    case "spawning":
    case "error":
      return status;
    default:
      return "stopped";
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
