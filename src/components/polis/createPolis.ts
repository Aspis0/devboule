// Shared PIXI bootstrap for the Polis map.
//
// Creates a PIXI.Application + pixi-viewport Viewport + PolisRenderer mounted
// into a host element, wires pan/zoom, and returns a handle with a `destroy()`
// that tears everything down with NO leaks (ticker callbacks removed, resize
// observer disconnected, app + renderer destroyed).
//
// pixi-viewport@6 declares peer `pixi.js: >=8` and is compatible with the
// installed pixi.js@8 — so we use it directly (drag/pinch/wheel/clampZoom)
// rather than a manual fallback.

import { Application, Container } from "pixi.js";
import { Viewport } from "pixi-viewport";
import type {
  CityState,
  Building,
  Agent,
  ExternalService,
} from "../../types/city";
import { PolisRenderer, type BuildProgress } from "./PolisRenderer";
import type {
  CensorFindingsPayload,
  GemmaStatus,
} from "./censorPresence";

const MIN_ZOOM = 0.15;
const MAX_ZOOM = 3.0;

export interface PolisHandle {
  app: Application;
  viewport: Viewport;
  renderer: PolisRenderer;
  /** Replace the whole scene. The heavy building geometry is built in batches
   *  across requestAnimationFrame (non-blocking); `onProgress` reports the
   *  running building count and a final "done". */
  setCity: (city: CityState, onProgress?: (p: BuildProgress) => void) => void;
  /** Apply a live fs-watcher update IN PLACE (diff onto the scene; no full
   *  rebuild, no camera move, selection preserved). */
  applyDiff: (city: CityState) => void;
  setSelected: (fileId: string | null) => void;
  recenter: () => void;
  /** Polis-P5 — feed a real `censor://findings-updated` event to the Censor
   *  firefighter presence (claims/walks an idle firefighter to the reviewed
   *  building; an empty-`files` event settles it). NOT an agent — never in
   *  `city.agents`. */
  onCensorFindings: (payload: CensorFindingsPayload) => void;
  /** Polis-P5 — update the cached gemma availability; "offline" suppresses (and
   *  releases) the Censor firefighter. */
  setCensorGemmaStatus: (status: GemmaStatus) => void;
  destroy: () => void;
}

export interface CreatePolisOptions {
  onSelectBuilding?: (b: Building | null) => void;
  onHoverBuilding?: (b: Building | null) => void;
  onSelectAgent?: (a: Agent | null) => void;
  /** A trade-route porter (or its road) was clicked — surface the REAL import
   *  edge `from` (importer/consumer) imports `to` (imported/supplier). */
  onSelectConnection?: (from: string, to: string) => void;
  /** A cloud outpost ("harbour" node) was clicked — surface the REAL external
   *  service (provider/type/name/status) in the inspect sidebar. */
  onSelectExternalService?: (service: ExternalService | null) => void;
  background?: number;
}

// DIAGNOSTIC (temporary, Phase 0): same proven file-log channel as cityStore's,
// so a failure inside app.init (WebGL context, canvas) is visible in release.
function polisDebug(line: string): void {
  try {
    void import("../../context/AppContext")
      .then(({ invokeBackendCommand }) =>
        invokeBackendCommand("polis_debug_log", { line }),
      )
      .catch(() => {});
  } catch {
    /* browser harness — ignore */
  }
}

export async function createPolis(
  host: HTMLElement,
  opts: CreatePolisOptions = {},
): Promise<PolisHandle> {
  const app = new Application();
  polisDebug(
    `createPolis: app.init start (host ${host.clientWidth}x${host.clientHeight}, dpr=${window.devicePixelRatio})`,
  );
  await app.init({
    background: opts.background ?? 0xf4f0e6,
    antialias: true,
    resolution: window.devicePixelRatio || 1,
    autoDensity: true,
    resizeTo: host,
    preference: "webgl",
  });
  polisDebug(
    `createPolis: app.init done (renderer=${app.renderer?.name ?? "?"})`,
  );

  // If the component unmounted while init() was awaiting, bail cleanly.
  if (!host.isConnected) {
    app.destroy(true, { children: true });
    throw new Error("Polis host detached before init completed.");
  }

  host.appendChild(app.canvas);
  app.canvas.style.width = "100%";
  app.canvas.style.height = "100%";
  app.canvas.style.display = "block";

  const viewport = new Viewport({
    screenWidth: host.clientWidth || 800,
    screenHeight: host.clientHeight || 600,
    worldWidth: 4000,
    worldHeight: 4000,
    events: app.renderer.events,
    ticker: app.ticker,
  });
  viewport
    .drag()
    .pinch()
    .wheel({ smooth: 3 })
    .decelerate({ friction: 0.92 })
    .clampZoom({ minScale: MIN_ZOOM, maxScale: MAX_ZOOM });

  // The viewport itself is the root container the renderer attaches layers to.
  app.stage.addChild(viewport as unknown as Container);

  const renderer = new PolisRenderer(app, viewport, {
    onSelectBuilding: opts.onSelectBuilding,
    onHoverBuilding: opts.onHoverBuilding,
    onSelectAgent: opts.onSelectAgent,
    onSelectConnection: opts.onSelectConnection,
    onSelectExternalService: opts.onSelectExternalService,
  });

  // Drive per-frame agent animation.
  const animTick = (ticker: { deltaMS: number }) => renderer.update(ticker.deltaMS);
  app.ticker.add(animTick);

  // Keep the viewport screen size in sync with the host element.
  const ro = new ResizeObserver(() => {
    const w = host.clientWidth || 800;
    const h = host.clientHeight || 600;
    viewport.resize(w, h);
  });
  ro.observe(host);

  let destroyed = false;
  const destroy = () => {
    if (destroyed) return;
    destroyed = true;
    ro.disconnect();
    app.ticker.remove(animTick);
    renderer.destroy();
    // Destroy viewport + app. `app.destroy(true, ...)` also removes the canvas.
    try {
      viewport.destroy({ children: true });
    } catch {
      // Viewport may already be detached by app.destroy; ignore.
    }
    app.destroy(true, { children: true, texture: true });
  };

  return {
    app,
    viewport,
    renderer,
    setCity: (city, onProgress) => renderer.setCityState(city, onProgress),
    applyDiff: (city) => renderer.applyCityDiff(city),
    setSelected: (fileId) => renderer.setSelected(fileId),
    recenter: () => renderer.recenter(),
    // Polis-P5 — prod clock is the monotonic performance.now(); the renderer ticks
    // the same clock from its loop so the debounce settles consistently.
    onCensorFindings: (payload) =>
      renderer.onCensorFindings(payload, performance.now()),
    setCensorGemmaStatus: (status) => renderer.setCensorGemmaStatus(status),
    destroy,
  };
}
