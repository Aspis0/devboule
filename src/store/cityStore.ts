// Zustand store for the Polis map.
//
// Single source of truth for the CityState shown by PolisView.
//
// FOLDER-AGNOSTIC: Polis is no longer pinned to this repo. The user POINTS it at
// any folder via the OS folder picker (PolisView), and that folder becomes the
// city. `loadFolder(path)` calls the backend `generate_city_state` with the
// chosen path. There is NO implicit auto-scan of the Management root on mount —
// the view shows an honest empty state until a folder is picked (or the last
// folder is restored from localStorage).
//
// When not running inside Tauri (browser / dev-harness), there is no OS picker,
// so `load()` falls back to the dev fixture at `/polis-dev-city.json` so the map
// can be developed and visually verified in a plain browser without login.

import { create } from "zustand";
import type { UnlistenFn } from "@tauri-apps/api/event";
import type { CityState, SinRecord, FilterState } from "../types/city";
import { invokeBackendCommand, isTauriRuntime } from "../context/AppContext";

/** localStorage key for the last folder the user mapped (folder-agnostic reload). */
const LAST_FOLDER_KEY = "polis:lastFolder";

/** Tauri event the backend fs-watcher emits with the full new CityState. Must
 *  match `CITY_UPDATED_EVENT` in `src-tauri/src/polis/watcher.rs`. */
const CITY_UPDATED_EVENT = "polis://city-updated";

function readLastFolder(): string | null {
  try {
    const v = window.localStorage.getItem(LAST_FOLDER_KEY);
    return v && v.trim() ? v : null;
  } catch {
    return null;
  }
}

function writeLastFolder(path: string | null): void {
  try {
    if (path) window.localStorage.setItem(LAST_FOLDER_KEY, path);
    else window.localStorage.removeItem(LAST_FOLDER_KEY);
  } catch {
    // Private mode / quota — non-fatal; the folder simply won't persist.
  }
}

/** Last path component (basename) for display, handling both `/` and `\`. */
export function folderBasename(path: string | null): string | null {
  if (!path) return null;
  const parts = path.replace(/[\\/]+$/, "").split(/[\\/]/);
  const base = parts[parts.length - 1];
  return base && base.length > 0 ? base : path;
}

interface CityStoreState {
  cityState: CityState | null;
  /**
   * The most recent LIVE update from the fs-watcher (`polis://city-updated`),
   * distinct from `cityState` so the view can DIFF it onto the renderer in place
   * instead of doing a full reload + recenter. Carries a monotonic `seq` so the
   * view's effect fires on every event (even when two consecutive payloads are
   * structurally equal). Null until the first live event after a load.
   */
  liveCity: { city: CityState; seq: number } | null;
  loading: boolean;
  error: string | null;
  /** True when the data came from the dev fixture, not the live backend. */
  usingFixture: boolean;
  /** The folder currently mapped (absolute path), or null if none chosen yet. */
  selectedFolder: string | null;
  selectedBuildingId: string | null;

  /** Augure sin ledger (P1.4): all records for the mapped project, or [] when
   *  unavailable (browser, no folder, ledger error). Drives the parchment
   *  anomaly section — the visual-layer `Building.sins` is open-sins-only. */
  sinRecords: SinRecord[] | null;
  /** Sin id of an in-flight dispose/fix action. When non-null the matching
   *  row's action buttons are disabled (prevents double-dispatch). */
  sinActionPending: string[];

  /**
   * DEEP-LINK (GAP B): a pending request to focus a specific agent in Polis.
   * `pendingFolder` is the agent's project root to auto-map; `pendingFocusAgentId`
   * is the agent to select/center once the city is loaded. PolisView consumes this
   * on mount / city-ready, then clears it via `consumeFocusRequest`. Either field
   * may be null.
   *
   * NOTE (Phase G): the former writer (the standalone Agents page, via a
   * `requestFocusAgent` setter) was removed when that page was dissolved. The
   * consume path is kept (PolisView still calls `consumeFocusRequest`) so a future
   * agent-focus deep-link can re-add a setter without re-plumbing the consumer;
   * with no current writer these fields simply stay null and the consume no-ops.
   */
  pendingFolder: string | null;
  pendingFocusAgentId: string | null;

  /** Browser/dev-harness only: load the dev fixture. No-op (sets a guidance
   *  error) in the Tauri runtime — use loadFolder() there instead. */
  load: () => Promise<void>;
  /** Map a specific folder. The one entry point for the folder picker. */
  loadFolder: (path: string) => Promise<void>;
  /** Re-scan the currently selected folder (or re-fetch the fixture in browser). */
  refresh: (force?: boolean) => Promise<void>;
  /** Apply a live fs-watcher CityState (stored as `liveCity` for the view to
   *  diff). Also keeps `cityState` current so header counts/era reflect it. */
  applyLiveUpdate: (city: CityState) => void;
  /** Stop the live fs-watcher and drop the event listener (idempotent). */
  stopWatch: () => Promise<void>;
  selectBuilding: (fileId: string | null) => void;

  /**
   * Live AGENT poll (GAP A): start/stop a ~5s poll of `polis_refresh_agents`
   * that re-attaches real agents onto the EXISTING city without re-scanning, so
   * agents move/appear/disappear like the Projects/Agents pages. Tauri-only;
   * pauses when the document is hidden; idempotent; clears cleanly on stop.
   * Driven by PolisView's mount/unmount lifecycle.
   */
  startAgentPoll: () => void;
  stopAgentPoll: () => void;

  /** DEEP-LINK consumer: read + clear the pending focus request (PolisView). The
   *  matching setter was removed in Phase G (no production writer remains); see the
   *  pendingFolder/pendingFocusAgentId note above. */

  consumeFocusRequest: () => { folder: string | null; agentId: string | null };

  /**
   * F2: explicitly ask the Oracle to NAME / DESCRIBE / MERGE the deterministic
   * features into product-level quarters. Tauri-only (no-op guidance in the
   * browser). Fail-closed: when the Oracle is unavailable the backend returns the
   * UNCHANGED deterministic city + an honest status, which we still apply. Returns
   * the honest status string for the UI to surface. Sets `loading` while in
   * flight so the spinner shows and the agent poll pauses.
   */
  reclassifyFeatures: () => Promise<{ changed: boolean; status: string }>;
  /**
   * PRESTIGE / ERA RESET (explicit, guarded, Tauri-only). ARCHIVES the current
   * city to a real on-disk snapshot under `eras/<oldEra>_snapshot.json`, erects a
   * cumulative era monument from the REAL closing-era stats on the landward
   * margin, bumps the persisted era, and CLEARS the in-memory city (honest empty
   * — the next scan repopulates it). Mildly destructive, so the caller must
   * confirm; this method does NOT prompt.
   *
   * Applies the returned (reset) CityState through the SAME live-update/diff path
   * the fs-watcher uses, so the OLD city sequences OUT (buildings rubble away) and
   * the monument appears at its margin — then re-scans the current folder and
   * funnels THAT through the diff path too, so the NEW era's buildings POP IN from
   * the ground. requestSeq-guarded like the other mutations (a newer load/refresh
   * supersedes a slow reset). Returns an honest status string for the UI.
   * No-op guidance in the browser (no backend / no Oracle).
   */
  resetToNewEra: (name: string) => Promise<{ ok: boolean; status: string }>;
  /** Read the per-workspace scan extensions (full available set + enabled subset). */
  getScanExtensions: () => Promise<{ available: string[]; enabled: string[] }>;
  /** Persist the scan extensions for the current folder, then rebuild the city. */
  applyScanExtensions: (extensions: string[]) => Promise<void>;

  /** Load all sin ledger records for the mapped project (Tauri-only).
   *  Called after every full city load; failures silently set records to null. */
  loadSinRecords: () => Promise<void>;
  /** Set a sin disposition (open|ignored). Returns an error string or null.
   *  On success refreshes the ledger + city. */
  disposeSin: (relPath: string, sinId: string, disposition: "open" | "ignored") => Promise<string | null>;
  /** Dispatch a fix directive to the main coder. Returns an error string or null.
   *  On success refreshes the ledger + city. */
  fixSin: (relPath: string, sinId: string) => Promise<string | null>;

  /** P3.2 — Filters panel state. Survives city refresh automatically. */
  filter: FilterState;
  setFilter: (patch: Partial<FilterState>) => void;
  resetFilter: () => void;
}

// ---------------------------------------------------------------------------
// Live fs-watcher plumbing (module-level, Tauri only).
// ---------------------------------------------------------------------------
//
// The backend watcher (polis_start_watch) re-scans on a debounced file change
// and emits `polis://city-updated` with the full new CityState. We start it
// after a successful folder load, listen for the event, and tear both down on
// stop / folder switch so we never double-subscribe or leak a listener.

/** The active event-unlisten fn, or null when not listening. */
let cityUpdatedUnlisten: UnlistenFn | null = null;
/** The folder the watcher is CLAIMED/started on (so a re-load of the same folder
 *  doesn't re-subscribe redundantly; the backend start is itself idempotent).
 *  FIX 2: `startWatchFor` CLAIMS this synchronously (before any await) so a
 *  concurrent `loadFolder`/`refresh` sees the claim and skips, preventing a
 *  double-subscribe race. */
let watchedFolder: string | null = null;
/** FIX 2: monotonic watch-start epoch. Each `startWatchFor` bumps it and
 *  captures its value; if a NEWER start has begun by the time an await resolves,
 *  the superseded start bows out (no double-subscribe, no stale claim stomp). */
let watchEpoch = 0;
/** Monotonic sequence for live updates (forces the view effect to re-run even on
 *  structurally-equal payloads). */
let liveSeq = 0;

// ---------------------------------------------------------------------------
// Live AGENT poll (GAP A): module-level so it survives store-action identity.
// ---------------------------------------------------------------------------
//
// The fs-watcher only fires on FILE changes, so agents that move/appear/
// disappear without touching a file would otherwise stay frozen on the map.
// This poll calls the cheap `polis_refresh_agents` (re-attach onto the existing
// city, no re-scan) every AGENT_POLL_MS and funnels the result through the SAME
// live-update path the watcher uses (applyLiveUpdate -> renderer.applyCityDiff),
// so agents animate via the existing diff. Mirrors the Projects/Agents pages'
// ~5s get_agent_live_state poll, with the same guards.
//
// L3 — WHY A POLL, NOT AN EVENT (deliberate, honest decision):
//   The real agent liveness signal is the `.aspis-agents.json` state file, which
//   the EXTERNAL MCP server process (oracle/server/aspis_mcp.py) rewrites on every
//   `agent_heartbeat`/claim/status change. It lives in the Management `projects/`
//   directory — OUTSIDE the mapped project root the Polis fs-watcher recurses, so
//   the existing `polis://city-updated` watcher does NOT and cannot trivially
//   observe it. Observing it event-driven would require a SEPARATE notify watcher
//   rooted at the projects dir (a new long-lived OS resource with its own start/
//   stop lifecycle racing the folder switch, its own debounce, a new Tauri event,
//   command registration, and a new frontend listener to tear down) — i.e. new
//   plumbing with exactly the leak/race surface we hardened the poll against, for
//   sub-5s gains that aren't meaningful for human-paced heartbeats. We also will
//   NOT tighten below 5s: each refresh re-reads the file under a cross-process
//   file lock contended with the MCP writer and runs the per-entry ledger prune
//   (EnumWindows on Windows), so a tighter cadence buys nothing but contention.
//   DECISION: keep the robust, guarded 5s poll (aligned with AgentsView). A
//   real-time MCP push (the server emitting an "agents changed" signal the app
//   subscribes to) is the right future enhancement and is tracked as such; it is
//   intentionally deferred here per "honest + robust, not must-be-push".

/** Poll cadence: matches the Projects/Agents pages (~5s). See the L3 note above
 *  for why this is a poll and not an event, and why 5s is not tightened. */
const AGENT_POLL_MS = 5_000;
/** Active poll timer id, or null when not polling. */
let agentPollTimer: number | null = null;
/** GLOBAL in-flight guard (L3 FIX): true while a `polis_refresh_agents` request
 *  is on the wire, regardless of which poll epoch issued it. Enforces the HARD
 *  invariant "at most ONE refresh in flight at a time".
 *
 *  OWNERSHIP RULE: ONLY the tick that set it to `true` ever clears it — in its
 *  own `finally`, which always runs (even when the result is dropped on an epoch/
 *  folder mismatch). `stopAgentPoll` MUST NOT clear it: clearing it on stop would
 *  let a restart fire a SECOND concurrent request while the first is still on the
 *  wire (the prior-audit F0 bug). Because only the issuing tick clears it, the
 *  flag is `true` strictly while a real request is in flight — never stranded:
 *  the in-flight request's `finally` clears it, and only then can a restarted
 *  poll proceed. */
let agentPollInFlight = false;
/** Monotonic epoch: bumped on every start/stop so an in-flight refresh whose
 *  poll was stopped/restarted meanwhile is dropped (no stale apply). */
let agentPollEpoch = 0;

/** TEST-ONLY: read the GLOBAL agent-poll in-flight flag directly, so tests can
 *  assert the "never stranded true" invariant without going through indirect
 *  consequences. Pure getter with NO production callers — tree-shaken out of
 *  prod bundles (no side effects, no runtime cost on the prod surface). */
export function __agentPollInFlightForTest(): boolean {
  return agentPollInFlight;
}

/** Subscribe to `polis://city-updated` exactly once. Safe to call repeatedly;
 *  drops any prior listener first so we never double-subscribe. */
async function subscribeCityUpdated(
  onCity: (city: CityState) => void,
): Promise<void> {
  if (!isTauriRuntime()) return;
  // Drop any prior listener before re-subscribing.
  if (cityUpdatedUnlisten) {
    cityUpdatedUnlisten();
    cityUpdatedUnlisten = null;
  }
  const { listen } = await import("@tauri-apps/api/event");
  cityUpdatedUnlisten = await listen<CityState>(CITY_UPDATED_EVENT, (event) => {
    if (event.payload) onCity(event.payload);
  });
}

/** Tear down the live watch PLUMBING (drop the event listener + stop the backend
 *  watcher) WITHOUT touching `watchedFolder`. Shared by `stopWatch` (which then
 *  clears the claim) and `startWatchFor` (which must NOT clear the freshly-claimed
 *  folder when stopping the PREVIOUS watcher). Idempotent / best-effort. */
async function teardownWatchPlumbing(): Promise<void> {
  if (cityUpdatedUnlisten) {
    cityUpdatedUnlisten();
    cityUpdatedUnlisten = null;
  }
  if (!isTauriRuntime()) return;
  try {
    await invokeBackendCommand<void>("polis_stop_watch");
  } catch {
    // Already stopped / unlock lapsed — nothing to clean up beyond the above.
  }
}

/** Start the backend watcher on `path` and (re)subscribe to its events. Guarded
 *  to Tauri; never throws into the load path (the map still works without live
 *  updates, e.g. if unlock lapsed).
 *
 *  BLOCKER B — invariant: there is NEVER a running BACKEND watcher without a
 *  matching live FRONTEND listener. We FULLY tear down any prior watch (drop the
 *  listener AND stop the previous folder's backend watcher) BEFORE we subscribe +
 *  start the new one, so a failed start never orphans a backend watcher.
 *
 *  FIX 2 — double-subscribe race: `stopWatch()` used to null `watchedFolder`
 *  BEFORE the backend `polis_stop_watch` resolved, so two rapid
 *  `loadFolder`/`refresh` calls both saw `watchedFolder === null` and BOTH ran
 *  `startWatchFor` -> double `subscribeCityUpdated` + competing starts, letting a
 *  stale-folder scan land via the listener. The fix makes the claim SYNCHRONOUS
 *  and idempotent:
 *    1. If `watchedFolder === path` already, we're watching/claiming this exact
 *       folder -> return early (a `refresh()` of the same folder never restarts).
 *    2. Otherwise CLAIM `watchedFolder = path` BEFORE any await, so a concurrent
 *       `loadFolder` sees the claim (its `watchedFolder !== trimmed` guard fails)
 *       and skips its own start.
 *    3. Tear down the PREVIOUS watcher's plumbing WITHOUT nulling the fresh claim.
 *    4. A monotonic `watchEpoch` lets a superseded start (one whose folder was
 *       re-claimed by a newer call mid-await) bow out and skip its subscribe/start.
 *    5. On failure, only clear the claim if it STILL equals `path` (never stomp a
 *       newer claim). */
async function startWatchFor(
  path: string,
  onCity: (city: CityState) => void,
): Promise<void> {
  if (!isTauriRuntime()) return;
  // (1) Already watching/claiming this exact folder: idempotent no-op. Keeps a
  // same-folder refresh() from needlessly restarting the watcher.
  if (watchedFolder === path) return;
  // (2) CLAIM the target synchronously, BEFORE any await, so a concurrent
  // loadFolder/refresh sees watchedFolder === path and skips its own start.
  watchedFolder = path;
  // (4) Capture this start's epoch; a newer start bumps it and supersedes us.
  const epoch = ++watchEpoch;
  try {
    // (3) Tear down the PREVIOUS watcher (listener + backend) WITHOUT nulling our
    // fresh claim. After the await a newer start may have begun — bail if so.
    await teardownWatchPlumbing();
    if (epoch !== watchEpoch) return;
    // Subscribe BEFORE starting so we can't miss an immediate emit.
    await subscribeCityUpdated(onCity);
    if (epoch !== watchEpoch) {
      // Superseded mid-subscribe: drop the listener we just attached so a newer
      // start owns the single listener, and bow out.
      if (cityUpdatedUnlisten) {
        cityUpdatedUnlisten();
        cityUpdatedUnlisten = null;
      }
      return;
    }
    // The backend command mirrors generate_city_state's signature: `projectPath`
    // is the folder to watch. Idempotent on the same root.
    await invokeBackendCommand<void>("polis_start_watch", { projectPath: path });
    // (4) A newer start superseded us during polis_start_watch: leave the claim
    // to the newer start (it owns watchedFolder now) and bow out.
    if (epoch !== watchEpoch) return;
  } catch {
    // Watch is best-effort: a failure (unlock lapsed, path vanished) leaves the
    // static map intact. Drop the half-open listener + stop any backend watcher
    // the failed start may have left running, so we never orphan a backend watch
    // with no live listener.
    await teardownWatchPlumbing();
    // (5) Only clear OUR claim — never stomp a newer start's claim.
    if (watchedFolder === path) watchedFolder = null;
  }
}

// Monotonic request id. Each load()/loadFolder()/refresh() bumps it and captures
// the value before its await; a response is applied only if no newer request
// started meanwhile (mirrors the authEpochRef pattern in AppContext). This
// prevents a slow earlier request from clobbering a faster later one's state.
let requestSeq = 0;

/** Scan a folder via the backend. `path` is required and sent explicitly — the
 *  frontend NEVER sends null (which would default the backend to the Management
 *  root); folder-agnostic means the user always chooses the target. */
async function scanFolder(path: string): Promise<CityState> {
  return invokeBackendCommand<CityState>("generate_city_state", {
    projectPath: path,
  });
}

/** DIAGNOSTIC (temporary, Phase 0): the moment a CityState arrives in JS — BEFORE
 *  any PixiJS render — log its composition (building/road counts, total routed road
 *  waypoints, JSON payload size as seen in JS) + the current JS heap to
 *  `%TEMP%/aspis-polis-debug.log` via the `polis_debug_log` Tauri command. This is
 *  the receive-side half of the OOM measurement: if the heap is already huge HERE,
 *  the payload/deserialization is the cost; if it's small here and explodes during
 *  the render, it's the renderer. Fire-and-forget; uses the proven invokeBackendCommand. */
function logCityComposition(
  city: CityState,
  source: string,
  payloadChars: number,
): void {
  try {
    const roads = city.roads ?? [];
    const waypoints = roads.reduce((n, r) => n + (r.path?.length ?? 0), 0);
    let heap = "";
    const mem = (
      performance as unknown as {
        memory?: { usedJSHeapSize: number; jsHeapSizeLimit: number };
      }
    ).memory;
    if (mem) {
      heap =
        ` heap=${Math.round(mem.usedJSHeapSize / 1048576)}MB ` +
        `(limit ${Math.round(mem.jsHeapSizeLimit / 1048576)}MB)`;
    }
    void invokeBackendCommand("polis_debug_log", {
      line:
        `RECEIVE[${source}] buildings=${city.buildings?.length ?? 0} ` +
        `roads=${roads.length} roadWaypoints=${waypoints} ` +
        `agents=${city.agents?.length ?? 0} payloadChars=${payloadChars}${heap}`,
    }).catch(() => {});
  } catch {
    /* never let diagnostics perturb loading */
  }
}

// Signature of the last city applied to state, used by `applyLiveUpdate` to drop
// identical re-deliveries. Computed ignoring volatile fields the backend refreshes
// on every rescan even when nothing changed: the top-level `generatedAt` timestamp
// and each building's `lastModified` mtime (mirrors the backend watcher's
// `city_signature` in watcher.rs — that side ZEROES the values, this side OMITS
// the keys; each layer only ever compares against its own previous signature, so
// the strategies never need to match, but the EXCLUDED FIELD SET must stay in
// sync if it ever grows). This frontend check is the catch-all funnel: the ~5s
// agent poll bypasses the backend watcher's own skip entirely.
//
// Stored as a 32-bit hash (not the JSON string) so the module retains 4 bytes,
// not a ~1.5MB string, for the whole session.
let lastAppliedCitySig: number | null = null;

function citySignature(city: CityState): { sig: number; chars: number } {
  const s = JSON.stringify(city, (key, value) =>
    key === "generatedAt" || key === "lastModified" ? undefined : value,
  );
  // djb2-xor over the serialized city.
  let h = 5381;
  for (let i = 0; i < s.length; i++) {
    h = (Math.imul(h, 33) ^ s.charCodeAt(i)) >>> 0;
  }
  return { sig: h, chars: s.length };
}

/** Browser dev fixture (no Tauri, no OS picker available). */
async function fetchFixture(): Promise<CityState> {
  const resp = await fetch("/polis-dev-city.json");
  if (!resp.ok) {
    throw new Error(
      "Polis map requires the desktop app (or a /polis-dev-city.json fixture for browser preview).",
    );
  }
  return (await resp.json()) as CityState;
}

export const useCityStore = create<CityStoreState>((set, get) => ({
  cityState: null,
  liveCity: null,
  loading: false,
  error: null,
  usingFixture: false,
  // Restore the last-mapped folder so the map reloads on next open (Tauri only).
  selectedFolder: isTauriRuntime() ? readLastFolder() : null,
  selectedBuildingId: null,
  pendingFolder: null,
  pendingFocusAgentId: null,
  sinRecords: null,
  sinActionPending: [],
  filter: { categories: [], minSeverity: null, features: [], pathGlob: "", mode: "ghost" },

  load: async () => {
    if (get().loading) return;
    // In the desktop app there is NO implicit Management scan: if a folder was
    // restored from localStorage, map it; otherwise stay in the empty state and
    // wait for the user to pick a folder.
    if (isTauriRuntime()) {
      const restored = get().selectedFolder;
      if (restored) await get().loadFolder(restored);
      return;
    }
    // Browser / dev-harness path: load the dumped fixture.
    const seq = ++requestSeq;
    set({ loading: true, error: null });
    try {
      const city = await fetchFixture();
      if (seq !== requestSeq) return;
      set({ cityState: city, usingFixture: true, loading: false });
    } catch (e) {
      if (seq !== requestSeq) return;
      set({
        loading: false,
        error:
          e instanceof Error ? e.message : "Failed to generate the Polis map.",
      });
    }
  },

  loadFolder: async (path: string) => {
    const trimmed = path.trim();
    if (!trimmed) return;
    const seq = ++requestSeq;
    // Clear the live-update signature up front: if THIS load's response is later
    // dropped (a newer request won), no stale signature from a previous folder
    // can spuriously suppress the next folder's first live event. A null sig
    // only means "never skip", which is the safe direction.
    lastAppliedCitySig = null;
    // Persist + reflect the selection immediately so the header shows the folder
    // even while the scan is in flight.
    writeLastFolder(trimmed);
    // P3.2 — reset filter when switching to a different folder so old project's
    // features/glob don't ghost the entire new city.
    const prev = get().selectedFolder;
    set({
      loading: true,
      error: null,
      selectedFolder: trimmed,
      ...(prev !== trimmed ? { filter: { categories: [], minSeverity: null, features: [], pathGlob: "", mode: "ghost" as const } } : {}),
    });
    try {
      const city = await scanFolder(trimmed);
      // Drop the response if a newer request started meanwhile.
      if (seq !== requestSeq) return;
      // A fresh full load supersedes any pending live update; clear liveCity so
      // the view's diff effect doesn't replay a stale event over the new scan.
      // Seed the live-update signature with this scan so a watcher/poll event
      // carrying the IDENTICAL city is dropped instead of restarting the build.
      const { sig, chars } = citySignature(city);
      lastAppliedCitySig = sig;
      logCityComposition(city, "scanFolder", chars);
      set({ cityState: city, liveCity: null, usingFixture: false, loading: false });
      // Enrichment: load the augure sin ledger (best-effort, non-blocking).
      if (isTauriRuntime()) void get().loadSinRecords();
      // Start (or re-point) the live fs-watcher on this folder. Best-effort and
      // Tauri-only; never blocks or fails the load. The event handler funnels
      // into applyLiveUpdate (a separate, diff-only path).
      if (isTauriRuntime() && watchedFolder !== trimmed) {
        void startWatchFor(trimmed, (live) => get().applyLiveUpdate(live));
      }
    } catch (e) {
      if (seq !== requestSeq) return;
      set({
        loading: false,
        error:
          e instanceof Error ? e.message : "Failed to map the selected folder.",
      });
    }
  },

  refresh: async (force?: boolean) => {
    if (!force && get().loading) return;
    const folder = get().selectedFolder;
    if (isTauriRuntime()) {
      // Nothing to refresh until a folder is chosen.
      if (!folder) return;
      await get().loadFolder(folder);
      return;
    }
    // Browser: always re-fetch the fixture.
    const seq = ++requestSeq;
    set({ loading: true, error: null });
    try {
      const city = await fetchFixture();
      if (seq !== requestSeq) return;
      set({ cityState: city, usingFixture: true, loading: false });
    } catch (e) {
      if (seq !== requestSeq) return;
      set({
        loading: false,
        error:
          e instanceof Error ? e.message : "Failed to refresh the Polis map.",
      });
    }
  },

  applyLiveUpdate: (city) => {
    // SKIP-IF-UNCHANGED at the single funnel both live paths share — the fs-watcher
    // AND the ~5s agent poll (`polis_refresh_agents`) call this. If the incoming
    // city is IDENTICAL to what we already show (timestamp + per-file mtime
    // excluded — see `citySignature`), do NOT touch state. Re-applying an identical
    // city bumps `liveSeq`, which re-fires the view's diff effect and (worse) keeps
    // CANCELLING the in-flight chunked build before it can finish — the map never
    // settles (stays grey) and the repeated diffs churn the JS heap. Dropping the
    // no-op updates lets the build complete and the map render.
    const { sig, chars } = citySignature(city);
    if (sig === lastAppliedCitySig) {
      // DIAGNOSTIC (temporary, Phase 0): record that a no-op delivery was
      // dropped, so the live verification can SEE the skip working.
      void invokeBackendCommand("polis_debug_log", {
        line: `SKIP[liveUpdate] identical city (sig=${sig})`,
      }).catch(() => {});
      return;
    }
    lastAppliedCitySig = sig;
    logCityComposition(city, "liveUpdate", chars);
    // Store as the LIVE payload (the view diffs this onto the renderer) AND as
    // the current cityState so header counts / era / sidebar stay in sync. The
    // monotonic seq forces the view effect to fire on every event. Drop a fixture
    // flag — a live event only ever comes from the real backend.
    liveSeq += 1;
    set({ cityState: city, liveCity: { city, seq: liveSeq }, usingFixture: false });
  },

  stopWatch: async () => {
    // FIX 2: bump the watch epoch FIRST so any in-flight startWatchFor bows out
    // (it won't subscribe/start, and its catch won't stomp state). Clear the
    // claim synchronously, then tear down the plumbing (drop the listener so no
    // in-flight event re-arms anything, then stop the backend watcher). All
    // idempotent / best-effort.
    watchEpoch += 1;
    watchedFolder = null;
    // No watcher -> no live deliveries to dedupe; drop the signature so a later
    // restart starts clean (and nothing lingers after the view unmounts).
    lastAppliedCitySig = null;
    await teardownWatchPlumbing();
  },

  selectBuilding: (fileId) => set({ selectedBuildingId: fileId }),

  setFilter: (patch) => set((s) => ({ filter: { ...s.filter, ...patch } })),
  resetFilter: () => set({ filter: { categories: [], minSeverity: null, features: [], pathGlob: "", mode: "ghost" } }),

  startAgentPoll: () => {
    // Tauri-only; the browser fixture has no live agents. Idempotent: a second
    // start is a no-op while a poll is already armed.
    if (!isTauriRuntime()) return;
    if (agentPollTimer !== null) return;
    const epoch = ++agentPollEpoch;

    const tick = async () => {
      // Re-check the epoch each tick: a stop()/restart bumps it and this stale
      // closure must bow out (clearing nothing it doesn't own).
      if (epoch !== agentPollEpoch) return;
      // Pause when hidden (saves a backend round-trip while the app is in the
      // background) and skip if a folder is loading or a refresh is in flight.
      // The in-flight guard is GLOBAL (epoch-agnostic): if ANY refresh issued by
      // a prior epoch is still on the wire, we do NOT fire a second one — at most
      // one `polis_refresh_agents` is ever in flight (HARD invariant a).
      const { cityState, loading, selectedFolder } = get();
      if (
        document.visibilityState === "visible" &&
        !loading &&
        !agentPollInFlight &&
        cityState // nothing to refresh until a city is loaded
      ) {
        // Capture the folder this refresh is FOR. `polis_refresh_agents` re-attaches
        // agents onto the backend's last-scanned root (== selectedFolder); if the
        // user switches/loads a different folder while this is in flight, the result
        // is for a STALE city and must be dropped (invariant d).
        const folderAtRequest = selectedFolder;
        // L3 FIX 1 — SAME-folder reload clobber: the folder gate alone is blind to a
        // reload of the SAME folder (loadFolder(sameFolder)/refresh()/reclassify),
        // which writes a NEWER city while this agent refresh is in flight. Capture the
        // module-level `requestSeq` here; those paths each bump it (++requestSeq in
        // load/loadFolder/refresh/reclassifyFeatures), so a mismatch on resolve means a
        // fresher full load landed meanwhile and this agent result is stale -> drop it.
        // The agent poll itself NEVER bumps requestSeq (it applies via applyLiveUpdate),
        // so a quiet poll cycle leaves seqAtRequest === requestSeq and applies normally.
        const seqAtRequest = requestSeq;
        agentPollInFlight = true;
        try {
          const city = await invokeBackendCommand<CityState>(
            "polis_refresh_agents",
          );
          // Apply ONLY if: this poll is still current (epoch unchanged by a
          // stop/restart), the watched folder still matches (no folder switch
          // mid-flight), no fresher full load landed meanwhile (requestSeq
          // unchanged — guards the SAME-folder reload clobber), a city is still
          // loaded, and we actually got one back.
          const cur = get();
          if (
            epoch === agentPollEpoch &&
            cur.selectedFolder === folderAtRequest &&
            requestSeq === seqAtRequest &&
            cur.cityState &&
            city
          ) {
            // Funnel through the SAME live-update path the watcher uses, so the
            // renderer animates the agent diff (agentPresent changes) in place.
            get().applyLiveUpdate(city);
          }
        } catch {
          // Best-effort: no city yet, unlock lapsed, or no buildings -> the
          // backend returns an error; just skip this tick (the static map and
          // any fs-watcher updates remain intact).
        } finally {
          // OWNERSHIP: this tick set the flag, so this tick clears it — ALWAYS,
          // even on an epoch/folder mismatch above. `stopAgentPoll` never clears
          // it, so this is the only place a true->false transition happens for a
          // request that was actually issued. Guarantees the next poll can only
          // fire once this in-flight request has resolved (never two at once),
          // and that the flag is never stranded true after the request settles.
          agentPollInFlight = false;
        }
      }
      // Re-arm only while THIS poll is still current. A stop/restart bumped the
      // epoch -> this stale closure stops scheduling (the new poll owns the timer).
      if (epoch === agentPollEpoch) {
        agentPollTimer = window.setTimeout(() => void tick(), AGENT_POLL_MS);
      }
    };
    agentPollTimer = window.setTimeout(() => void tick(), AGENT_POLL_MS);
  },

  stopAgentPoll: () => {
    // Bump the epoch FIRST so (b) any in-flight refresh's result is DROPPED on
    // resolve (its `epoch === agentPollEpoch` check fails) and (c) the stale
    // tick stops re-arming the timer. Then clear the pending timer. Idempotent.
    //
    // L3 FIX: we deliberately DO NOT clear `agentPollInFlight` here. Clearing it
    // would let a restart (stopAgentPoll -> startAgentPoll) fire a SECOND
    // concurrent `polis_refresh_agents` while the first is still on the wire (the
    // F0-audit bug). The in-flight request's OWN `finally` clears the flag when it
    // settles, so a restarted poll proceeds only once at most one request is live.
    // It is therefore never stranded: the flag is true strictly while a real
    // request is in flight, and that request always clears it.
    agentPollEpoch += 1;
    if (agentPollTimer !== null) {
      window.clearTimeout(agentPollTimer);
      agentPollTimer = null;
    }
  },


  // --- Augure sin ledger (P1.4) ---

  loadSinRecords: async () => {
    if (!isTauriRuntime()) return;
    const folderAtRequest = get().selectedFolder;
    if (!folderAtRequest) { set({ sinRecords: null }); return; }
    // B1: capture the current requestSeq (read only — do NOT bump it)
    const seq = requestSeq;
    try {
      const records = await invokeBackendCommand<SinRecord[]>(
        "polis_list_sins",
        { projectPath: folderAtRequest },
      );
      // B1: bail if folder switched or a newer load superseded us
      if (get().selectedFolder !== folderAtRequest || requestSeq !== seq) return;
      set({ sinRecords: records ?? [] });
    } catch {
      if (get().selectedFolder !== folderAtRequest || requestSeq !== seq) return;
      // Ledger is enrichment — failures never block the parchment.
      set({ sinRecords: null });
    }
  },

  disposeSin: async (relPath, sinId, disposition) => {
    const folder = get().selectedFolder;
    if (!folder) return "No project mapped.";
    // M1: idempotent — skip if this sin is already in-flight
    if (get().sinActionPending.includes(sinId)) return null;
    set({ sinActionPending: [...get().sinActionPending, sinId] });
    try {
      await invokeBackendCommand("polis_dispose_sin", {
        projectPath: folder,
        relPath,
        sinId,
        disposition,
      });
      // B1: re-check folder before reloading — a concurrent loadFolder may have switched
      if (get().selectedFolder !== folder) return null;
      // Refresh the ledger + city so the map reflects the changed disposition.
      void get().loadSinRecords();
      await get().refresh(true);
      return null;
    } catch (e) {
      return e instanceof Error ? e.message : String(e);
    } finally {
      set({ sinActionPending: get().sinActionPending.filter((id) => id !== sinId) });
    }
  },

  fixSin: async (relPath, sinId) => {
    const folder = get().selectedFolder;
    if (!folder) return "No project mapped.";
    // M1: idempotent — skip if this sin is already in-flight
    if (get().sinActionPending.includes(sinId)) return null;
    set({ sinActionPending: [...get().sinActionPending, sinId] });
    try {
      await invokeBackendCommand("polis_fix_sin", {
        projectPath: folder,
        relPath,
        sinId,
      });
      // B1: re-check folder before reloading — a concurrent loadFolder may have switched
      if (get().selectedFolder !== folder) return null;
      // Refresh the ledger + city so the building gains the agent overlay.
      void get().loadSinRecords();
      await get().refresh(true);
      return null;
    } catch (e) {
      return e instanceof Error ? e.message : String(e);
    } finally {
      set({ sinActionPending: get().sinActionPending.filter((id) => id !== sinId) });
    }
  },

  consumeFocusRequest: () => {
    const { pendingFolder, pendingFocusAgentId } = get();
    // Clear immediately so a re-mount / re-render doesn't replay the request.
    if (pendingFolder !== null || pendingFocusAgentId !== null) {
      set({ pendingFolder: null, pendingFocusAgentId: null });
    }
    return { folder: pendingFolder, agentId: pendingFocusAgentId };
  },

  reclassifyFeatures: async () => {
    // Browser harness has no Oracle: degrade with an honest message, no call.
    if (!isTauriRuntime()) {
      return {
        changed: false,
        status: "Re-classify requires the desktop app (Oracle).",
      };
    }
    if (get().loading) {
      return { changed: false, status: "Busy — try again in a moment." };
    }
    const seq = ++requestSeq;
    set({ loading: true, error: null });
    try {
      const res = await invokeBackendCommand<{
        city: CityState;
        changed: boolean;
        status: string;
      }>("polis_reclassify_features");
      // Drop the response if a newer load/refresh/reclassify started meanwhile.
      if (seq !== requestSeq) return { changed: false, status: res.status };
      // Apply the returned city. Clear liveCity so the view's diff effect doesn't
      // replay a stale fs-watcher event over the freshly-classified scan.
      set({
        cityState: res.city,
        liveCity: null,
        usingFixture: false,
        loading: false,
      });
      // Seed the live-update signature with the freshly-classified city (mirror
      // loadFolder). Without this, lastAppliedCitySig still holds the PRE-reclassify
      // city, so the next watcher/poll event carrying THIS city is treated as a
      // change and spuriously re-applies/rebuilds the scene.
      lastAppliedCitySig = citySignature(res.city).sig;
      void get().loadSinRecords();
      return { changed: res.changed, status: res.status };
    } catch (e) {
      if (seq !== requestSeq)
        return { changed: false, status: "Re-classify superseded." };
      const status =
        e instanceof Error ? e.message : "Re-classify failed.";
      set({ loading: false, error: null });
      return { changed: false, status };
    }
  },

  resetToNewEra: async (name: string) => {
    const era = name.trim();
    if (!era) return { ok: false, status: "Era name is empty." };
    // Prestige/era reset needs the backend (it archives to disk + persists meta).
    if (!isTauriRuntime()) {
      return {
        ok: false,
        status: "Starting a new era requires the desktop app.",
      };
    }
    if (get().loading) {
      return { ok: false, status: "Busy — try again in a moment." };
    }
    const folder = get().selectedFolder;
    if (!folder) {
      return { ok: false, status: "Map a folder before starting a new era." };
    }
    // Bump the request id so a slow reset never clobbers a newer load/refresh, and
    // so any in-flight agent poll's result is dropped (it checks requestSeq).
    const seq = ++requestSeq;
    // Stop the live fs-watcher first: the reset clears the city, and we don't want
    // a stale `polis://city-updated` event (from a debounced earlier change) to
    // race the reset apply. The re-scan below restarts the watcher cleanly.
    await get().stopWatch();
    set({ loading: true, error: null });
    try {
      // 1) Archive + reset on the backend. Returns the reset CityState (empty
      //    buildings + the cumulative monument at the landward margin).
      const resetCity = await invokeBackendCommand<CityState>(
        "reset_city_to_new_era",
        { newEraName: era },
      );
      if (seq !== requestSeq)
        return { ok: false, status: "Era reset superseded." };
      // Apply the reset city through the LIVE-UPDATE path so the renderer DIFFS it
      // onto the existing scene: the OLD era's buildings rubble OUT and the new
      // monument appears at its margin (no full teardown/recenter). loading stays
      // true through the re-scan below so the spinner covers the whole transition.
      // FORCE the apply: clear the dedupe signature so applyLiveUpdate cannot skip
      // it. On an empty project the reset city can hash identically to what's shown,
      // and a silent skip would leave the OLD era's scene on screen.
      lastAppliedCitySig = null;
      get().applyLiveUpdate(resetCity);

      // 2) Re-scan the SAME folder to grow the new era's city, and funnel it
      //    through the live-update path too so the fresh buildings POP IN from the
      //    ground (the diff path fires popIn for ADDED buildings). A scan failure
      //    here is non-fatal: the reset already succeeded and is shown; surface an
      //    honest note and leave the monument-only city in place.
      try {
        const city = await scanFolder(folder);
        if (seq !== requestSeq)
          return { ok: true, status: `New era “${era}” begun.` };
        // FORCE the apply too: if the re-scanned city happens to hash identically
        // to the reset city just applied (e.g. an empty project), a silent skip
        // would strand the UI on the reset/monument-only city.
        lastAppliedCitySig = null;
        get().applyLiveUpdate(city);
        set({ loading: false });
        void get().loadSinRecords();
        // Restart the live fs-watcher on the folder (best-effort, Tauri-only).
        if (watchedFolder !== folder) {
          void startWatchFor(folder, (live) => get().applyLiveUpdate(live));
        }
        return { ok: true, status: `New era “${era}” begun.` };
      } catch {
        if (seq !== requestSeq)
          return { ok: true, status: `New era “${era}” begun.` };
        set({ loading: false });
        void get().loadSinRecords();
        return {
          ok: true,
          status: `New era “${era}” begun — map the folder again to grow the new city.`,
        };
      }
    } catch (e) {
      if (seq !== requestSeq)
        return { ok: false, status: "Era reset superseded." };
      const status =
        e instanceof Error ? e.message : "Failed to start the new era.";
      set({ loading: false, error: null });
      return { ok: false, status };
    }
  },

  getScanExtensions: () =>
    invokeBackendCommand<{ available: string[]; enabled: string[] }>(
      "polis_get_scan_extensions",
      { projectPath: get().selectedFolder },
    ),

  applyScanExtensions: async (extensions: string[]) => {
    const folder = get().selectedFolder;
    await invokeBackendCommand<void>("polis_set_scan_extensions", {
      projectPath: folder,
      extensions,
    });
    // Rebuild the city with the new filter (preserves the folder + selection).
    if (folder) await get().loadFolder(folder);
    else await get().refresh();
  },
}));
