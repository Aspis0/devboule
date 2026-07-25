// PolisView — the React view that hosts the Polis isometric map.
//
// Mounts a PIXI.Application (via createPolis) into a div, loads the CityState
// from the zustand store on mount, and renders loading / error / empty states,
// the InspectSidebar, a small agent roster, and a header bar (building + agent
// counts, refresh, recenter, and an immersive/fullscreen toggle that hides the
// app chrome). PIXI is fully torn down on unmount — no leaks.

import { useEffect, useRef, useState, useCallback, useMemo } from "react";
import {
  RefreshCw,
  Crosshair,
  Maximize2,
  Minimize2,
  Castle,
  Bot,
  MapPinOff,
  FolderOpen,
  Sparkles,
  Trophy,
  X,
} from "lucide-react";
import type { Building, Agent, CityState } from "../../types/city";
import { agentTypeLabel } from "../../types/city";
import {
  useCityStore,
  folderBasename,
  folderLoadErrorSurface,
} from "../../store/cityStore";
import {
  isTauriRuntime,
  useAppContext,
  invokeBackendCommand,
} from "../../context/AppContext";
import {
  CENSOR_FINDINGS_UPDATED_EVENT,
  type CensorFindingsUpdatedPayload,
  type CensorStatus,
} from "../../types/backend";
import { agentColor } from "./palette";
import { createPolis, type PolisHandle } from "./createPolis";
import { InspectSidebar, type InspectSubject } from "./InspectSidebar";
import { planResourceSites, type ResourceSite } from "./resources";
import { PolisBottomBar } from "./PolisBottomBar";
import { findBuildingByCitation } from "./findBuildingByCitation";
import { computeFilterSets } from "./filterModel";

// DIAGNOSTIC (temporary, Phase 0): fire-and-forget a line to
// `%TEMP%/aspis-polis-debug.log` via the proven invokeBackendCommand path, so we
// can see whether PIXI init / the build effect ever run in the release build
// (DevTools unavailable; renderer-side logs never appeared).
function polisDebug(line: string): void {
  try {
    void invokeBackendCommand("polis_debug_log", { line }).catch(() => {});
  } catch {
    /* browser harness — ignore */
  }
}

/** Roster secondary label: strip OpenRouter-style provider prefix, truncate. */
function shortModelLabel(model: string, max = 18): string {
  const trimmed = model.trim();
  if (!trimmed) return "";
  const bare = (
    trimmed.includes("/") ? (trimmed.split("/").pop() ?? trimmed) : trimmed
  ).trim();
  if (!bare) return "";
  if (bare.length <= max) return bare;
  if (max <= 1) return "…";
  return `${bare.slice(0, max - 1)}…`;
}

export function PolisView() {
  const hostRef = useRef<HTMLDivElement>(null);
  const polisFocusedRef = useRef(false);
  const handleRef = useRef<PolisHandle | null>(null);
  const [ready, setReady] = useState(false);
  // The inspect subject is either a BUILDING, an AGENT, or nothing.
  const [selected, setSelected] = useState<InspectSubject>(null);
  // A selected resource site (quarry/mine) — shown in the inspect sidebar.
  const [selectedResource, setSelectedResource] = useState<ResourceSite | null>(null);

  /** Set the primary inspect subject (building/agent/connection/external) and
   *  ALWAYS clear selectedResource. Every path that sets `selected` must go
   *  through here to prevent stale resource cards from resurfacing. */
  const selectPrimary = useCallback((subject: InspectSubject) => {
    setSelected(subject);
    setSelectedResource(null);
  }, []);

  const [immersive, setImmersive] = useState(false);
  // Progress of the renderer's NON-BLOCKING chunked build. The backend scan
  // (`loading`) finishes before the renderer starts placing buildings in batches
  // across requestAnimationFrame; `building` covers that second phase so the
  // "Generating the Polis…" overlay stays up (with a count) while the UI thread
  // keeps breathing — instead of a multi-minute freeze. Null when idle/done.
  const [build, setBuild] = useState<{ done: number; total: number } | null>(
    null,
  );

  const cityState = useCityStore((s) => s.cityState);
  const liveCity = useCityStore((s) => s.liveCity);
  const loading = useCityStore((s) => s.loading);
  const error = useCityStore((s) => s.error);
  const usingFixture = useCityStore((s) => s.usingFixture);
  const selectedFolder = useCityStore((s) => s.selectedFolder);
  const load = useCityStore((s) => s.load);
  const loadFolder = useCityStore((s) => s.loadFolder);
  const refresh = useCityStore((s) => s.refresh);
  const stopWatch = useCityStore((s) => s.stopWatch);
  const startAgentPoll = useCityStore((s) => s.startAgentPoll);
  const stopAgentPoll = useCityStore((s) => s.stopAgentPoll);
  const consumeFocusRequest = useCityStore((s) => s.consumeFocusRequest);
  const reclassifyFeatures = useCityStore((s) => s.reclassifyFeatures);
  const resetToNewEra = useCityStore((s) => s.resetToNewEra);

  // F2 re-classify transient status (honest toast shown after the action).
  const [reclassifyStatus, setReclassifyStatus] = useState<string | null>(null);
  const reclassifyTimer = useRef<number | null>(null);

  // Prestige / "New era" guarded dialog + transient status toast.
  const [eraDialogOpen, setEraDialogOpen] = useState(false);
  const [eraStatus, setEraStatus] = useState<string | null>(null);
  const eraStatusTimer = useRef<number | null>(null);

  // The desktop app has an OS folder picker; the browser dev-harness does not.
  const desktop = isTauriRuntime();

  // The app-wide workspace folder (picked once via the first-run banner /
  // Oracle ▸ Indexing). Polis uses it as its DEFAULT folder when the user
  // hasn't pointed Polis at a folder of its own. Loads async with prefs.
  const { oracleIndexPreferences } = useAppContext();
  const workspaceRoot = oracleIndexPreferences?.indexRoot ?? null;

  // A pending deep-link agent to select once the city is ready (GAP B). Set when
  // PolisView mounts with a pending focus request from the Agents/Projects page;
  // consumed by the city-ready effect below and then cleared.
  const pendingFocusAgentRef = useRef<string | null>(null);

  // On mount: opening Polis must be INSTANT and never trigger a heavy scan on a
  // plain nav click. We ONLY load when there is an EXPLICIT intent:
  //   - a DEEP-LINK focus request from the Agents/Projects page (map that
  //     agent's folder + remember the agent to select) — that is a user intent;
  //   - the BROWSER dev-harness (no OS picker), where load() fetches the fixture.
  // In the plain desktop case we do ZERO scanning here and land on the empty
  // state; the user maps a folder via the explicit "Map workspace" / "Open
  // folder" buttons (or refresh). We still start the live AGENT poll (GAP A) —
  // it self-guards on `cityState` and is a no-op until a city exists — and stop
  // it + the fs-watcher on unmount so we don't leak a subscription or keep the
  // backend thread running.
  useEffect(() => {
    const { folder, agentId } = consumeFocusRequest();
    pendingFocusAgentRef.current = agentId;
    if (desktop) {
      // Desktop: only a deep-link folder is an explicit load intent. A plain
      // open does NOT scan (no implicit last-folder/workspace scan) — the empty
      // state is shown until the user picks a folder.
      if (folder && folder.trim()) {
        // loadFolder also (re)starts the fs-watcher; the city-ready effect then
        // selects the agent.
        void loadFolder(folder);
      }
    } else {
      // Browser dev-harness: load the dev fixture so the map is viewable.
      void load();
    }
    // Start the live agent poll (Tauri-only, self-guarded, visibility-aware).
    startAgentPoll();
    return () => {
      stopAgentPoll();
      void stopWatch();
    };
  }, [
    load,
    loadFolder,
    stopWatch,
    startAgentPoll,
    stopAgentPoll,
    consumeFocusRequest,
    desktop,
  ]);

  // Open the OS "select folder" dialog and map the chosen folder. Desktop only.
  const handleOpenFolder = useCallback(async () => {
    if (!desktop) return;
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const picked = await open({
        directory: true,
        multiple: false,
        title: "Choose a folder to map its city",
      });
      // `open` returns string | string[] | null. With multiple:false it's a
      // single string (or null if the user cancelled).
      if (typeof picked === "string" && picked.trim()) {
        void loadFolder(picked);
      }
    } catch {
      // Dialog plugin unavailable or user dismissed — no-op.
    }
  }, [desktop, loadFolder]);

  // Mount PIXI. createPolis is async, so guard against unmount-during-init.
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    let cancelled = false;
    let handle: PolisHandle | null = null;

    void createPolis(host, {
      onSelectBuilding: (b) =>
        selectPrimary(b ? { kind: "building", building: b } : null),
      onSelectAgent: (a) =>
        selectPrimary(a ? { kind: "agent", agent: a } : null),
      onSelectConnection: (from, to) =>
        selectPrimary({ kind: "connection", from, to }),
      onSelectExternalService: (s) =>
        selectPrimary(s ? { kind: "externalService", service: s } : null),
      onSelectResource: (r) => {
        setSelectedResource(r);
        if (r) setSelected(null);
      },
    })
      .then((h) => {
        if (cancelled) {
          polisDebug("PIXI init OK but component unmounted — destroying");
          h.destroy();
          return;
        }
        polisDebug("PIXI init OK — renderer ready");
        handle = h;
        handleRef.current = h;
        setReady(true);
      })
      .catch((e) => {
        // Host detached before init finished, or WebGL unavailable. The swallow
        // here made a failed init INVISIBLE (grey map, no logs) — record it.
        polisDebug(
          `PIXI INIT FAILED: ${e instanceof Error ? `${e.message}\n${e.stack ?? ""}` : String(e)}`,
        );
      });

    return () => {
      cancelled = true;
      handleRef.current = null;
      setReady(false);
      handle?.destroy();
    };
  }, []);

  // FULL BUILD path: a new scan (initial load, folder switch, refresh) replaces
  // the whole scene and recenters. A LIVE update also writes `cityState` (so the
  // header stays in sync), but it is the SAME object as `liveCity.city`; we skip
  // the full rebuild for that object and let the diff effect below handle it in
  // place (no recenter, selection preserved). The ref records which object the
  // diff path has claimed so this effect never double-renders it.
  const liveRenderedRef = useRef<CityState | null>(null);
  useEffect(() => {
    if (!ready || !cityState) return;
    // If this cityState object came from a live update, the diff effect owns it.
    if (cityState === liveCity?.city) return;
    let cancelled = false;
    // Mark the build in progress immediately so the overlay shows even before
    // the first batch frame lands. The renderer reports per-batch progress and a
    // final "done"; ignore late callbacks if this effect was superseded.
    setBuild({ done: 0, total: cityState.buildings.length });
    polisDebug(
      `BUILD EFFECT fires: buildings=${cityState.buildings.length} handle=${handleRef.current ? "yes" : "NULL"}`,
    );
    handleRef.current?.setCity(cityState, (p) => {
      if (cancelled) return;
      setBuild(p.phase === "done" ? null : { done: p.done, total: p.total });
    });
    // FIX 4: do NOT pre-claim the current `liveCity` object here. Pre-claiming
    // could capture a NEWER liveCity object that THIS build did not render (the
    // build effect reads `cityState`, which may lag a fresher `liveCity` that
    // arrived in the same render batch), permanently suppressing that newer
    // object's diff. Clear the claim instead: if the diff effect then re-applies
    // the SAME object the full build already rendered, it is a harmless no-op —
    // the renderer's `applyCityDiff` early-returns when `next === lastCity`
    // (FIX 4, PolisRenderer), and the full build set `lastCity` to this object.
    // A genuinely newer object still diffs correctly.
    liveRenderedRef.current = null;
    setSelected(null);
    setSelectedResource(null);
    return () => {
      // A new cityState (or unmount) supersedes this build; stop reacting to its
      // progress. The renderer's own buildToken aborts the rAF batch loop.
      cancelled = true;
    };
  }, [ready, cityState, liveCity]);

  // LIVE UPDATE path: the fs-watcher emitted a new CityState. Diff it onto the
  // existing scene IN PLACE — no full rebuild, no camera move, selection kept.
  // Keyed on `liveCity.seq` so it fires on every event (even equal payloads).
  useEffect(() => {
    if (!ready || !liveCity) return;
    // Guard: don't re-diff the same object twice.
    if (liveRenderedRef.current === liveCity.city) return;
    liveRenderedRef.current = liveCity.city;
    handleRef.current?.applyDiff(liveCity.city);
    // Selection is preserved by the renderer; if the selected building was
    // removed, reconcile the React-side popup so it doesn't show a stale subject.
    setSelected((prev) => {
      if (prev?.kind === "building") {
        const stillThere = liveCity.city.buildings.some(
          (b) => b.fileId === prev.building.fileId,
        );
        return stillThere ? prev : null;
      }
      if (prev?.kind === "connection") {
        // Drop a trade-route popup if EITHER endpoint left the map (the edge no
        // longer exists). The ConnectionPopup degrades gracefully too, but
        // closing keeps the inspector honest.
        const ids = new Set(liveCity.city.buildings.map((b) => b.fileId));
        return ids.has(prev.from) && ids.has(prev.to) ? prev : null;
      }
      if (prev?.kind === "externalService") {
        // Refresh the inspected cloud service from the fresh inventory (its status
        // may have flipped) or drop the popup if the resource vanished — never show
        // a stale/ghost service.
        const fresh = liveCity.city.externalServices?.find(
          (s) => s.serviceId === prev.service.serviceId,
        );
        return fresh ? { kind: "externalService", service: fresh } : null;
      }
      return prev;
    });
    // Reconcile selectedResource: check whether the selected site's district
    // still qualifies (census ≥ threshold, bounds unchanged). Cheap guard:
    // compare the district's census + bounds directly; only re-plan when they
    // differ (T9 — skip expensive planResourceSites on every diff).
    setSelectedResource((prev) => {
      if (!prev) return null;
      const district = liveCity.city.districts.find(
        (d) => d.districtId === prev.districtId,
      );
      if (!district) return null; // district removed
      const c = district.assetCensus ?? { images: 0, fonts: 0, media: 0 };
      const total = c.images + c.fonts + c.media;
      if (total < 8) return null; // census dropped below threshold
      const b = district.bounds;
      const bChanged =
        b.x !== prev.gx || b.y !== prev.gy || // rough: bounds changed
        c.images !== prev.census.images ||
        c.fonts !== prev.census.fonts ||
        c.media !== prev.census.media;
      if (!bChanged) return prev; // unchanged — keep as-is
      // Census or bounds changed — refresh by re-planning.
      const updatedSites = planResourceSites(liveCity.city);
      return updatedSites.find((s) => s.id === prev.id) ?? null;
    });
  }, [ready, liveCity]);

  // DEEP-LINK focus (GAP B): once a city containing the target agent is loaded,
  // select that agent (its building gets ringed via the selection effect) and
  // recenter so it is in view. Runs whenever cityState changes while a focus
  // request is pending; clears the pending ref once the agent is matched. If the
  // agent isn't in this city yet (off-map / not resolved), the request stays
  // pending so a later live/agent-poll update can still match it.
  useEffect(() => {
    if (!ready || !cityState) return;
    const wantId = pendingFocusAgentRef.current;
    if (!wantId) return;
    const agent = cityState.agents.find((a) => a.agentId === wantId);
    if (agent) {
      pendingFocusAgentRef.current = null;
      selectPrimary({ kind: "agent", agent });
      handleRef.current?.recenter();
    }
  }, [ready, cityState]);

  // Polis-P5 — Censor firefighter feed. Subscribe to the REAL
  // `censor://findings-updated` event (desktop only) and drive the renderer's
  // Censor presence: a naming event walks the firefighter to the reviewed
  // building (lighting its water arc); an empty-`files` event settles it. We also
  // refresh the cached gemma availability (the firefighter is suppressed when the
  // engine is offline) — once on mount and again on every event, since the probe
  // is set lazily on the first watch start. Censor is an ENGINE, not an agent: it
  // never touches `city.agents`. Cleaned up on unmount (no listener leak).
  useEffect(() => {
    if (!ready || !desktop) return;
    let cancelled = false;
    let unlisten: (() => void) | null = null;

    // Read + push the cached gemma availability for `projectId` (root from the
    // selected folder). A failed read (untrusted root / no watch yet) is benign —
    // we leave the presence's last-known status untouched.
    const refreshGemma = async (projectId: string) => {
      const root = selectedFolder;
      if (!root || !root.trim()) return;
      try {
        const status = await invokeBackendCommand<CensorStatus>("censor_status", {
          root,
          projectId,
        });
        if (cancelled) return;
        handleRef.current?.setCensorGemmaStatus(status.gemmaStatus);
      } catch {
        // Untrusted/no-watch/board-level read — keep the last-known status.
      }
    };

    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      const dispose = await listen<CensorFindingsUpdatedPayload>(
        CENSOR_FINDINGS_UPDATED_EVENT,
        async (event) => {
          const payload = event.payload;
          if (!payload || typeof payload.projectId !== "string") return;
          // Refresh gemma FIRST and AWAIT it (it gates whether the firefighter
          // shows): refreshGemma pushes the fresh status into the presence via
          // setCensorGemmaStatus, so by the time we feed the event the handle's
          // CensorPresence sees the current gemma status — preventing a claim
          // while gemma is actually offline. refreshGemma swallows its own probe
          // errors, so on probe failure we proceed with the last-known status
          // (acceptable) rather than dropping the event. Re-check the cancelled/
          // ref guards after the await (the effect may have torn down meanwhile).
          await refreshGemma(payload.projectId);
          if (cancelled) return;
          handleRef.current?.onCensorFindings(payload);
        },
      );
      // #2 — disposed-flag + capture pattern (the standard Tauri async-listen fix):
      // `listen()` is async, so the component can unmount DURING the await above. If
      // it did, the cleanup below already ran with `unlisten` still null (no-op) — so
      // we MUST tear the just-resolved subscription down HERE or it leaks for the
      // session. The `cancelled` flag IS the disposed flag: if set, dispose now and
      // never store the token; otherwise store it for the cleanup to call. (The
      // window between this line and the await resolving is synchronous, so no
      // interleaved unmount can slip a leak through.)
      if (cancelled) {
        dispose();
        return;
      }
      unlisten = dispose;
    })();

    return () => {
      // Sets the disposed flag (so an in-flight `listen()` self-disposes when it
      // resolves) AND tears down an already-stored subscription. Either path tears
      // the listener down exactly once — no leak whether unmount races the await or
      // happens after it resolved.
      cancelled = true;
      unlisten?.();
    };
  }, [ready, desktop, selectedFolder]);

  // Reflect selection into the renderer (selection ring). For a building, ring
  // it directly; for an agent, ring the building it is working on (if any).
  useEffect(() => {
    let ringId: string | null = null;
    if (selected?.kind === "building") {
      ringId = selected.building.fileId;
    } else if (selected?.kind === "agent") {
      ringId = selected.agent.currentFileId ?? null;
    } else if (selected?.kind === "connection") {
      // Ring the importer (consumer) end of the trade route for click feedback.
      ringId = selected.from;
    }
    handleRef.current?.setSelected(ringId);
  }, [selected]);

  // P3.2 — single memoized FilterSets computation; used by the renderer effect
  // and passed to PolisBottomBar for the panel footer (no double compute).
  const filterState = useCityStore((s) => s.filter);
  const sinRecords = useCityStore((s) => s.sinRecords);
  const filterSets = useMemo(
    () => computeFilterSets(cityState, sinRecords, filterState),
    [cityState, sinRecords, filterState],
  );
  useEffect(() => {
    if (!ready) return;
    handleRef.current?.setFilter(filterSets);
    // F7 — reconcile React-side selection against the active filter.
    // If the selected building is HIDDEN (mode hide), close the sidebar.
    if (filterSets && filterSets.mode === "hide") {
      setSelected((prev) => {
        if (prev?.kind === "building") {
          return filterSets.ghostedFileIds.has(prev.building.fileId) ? null : prev;
        }
        return prev;
      });
      // Also clear any resource selection when a filter hides content.
      setSelectedResource(null);
    }
  }, [ready, filterSets]);

  // Aesthetic district-walls visibility (user display pref — pure visibility).
  const showWalls = useCityStore((s) => s.showWalls);
  useEffect(() => {
    if (!ready) return;
    handleRef.current?.setShowWalls(showWalls);
  }, [ready, showWalls]);

  const handleRefresh = useCallback(() => {
    void refresh();
  }, [refresh]);

  const handleRecenter = useCallback(() => {
    handleRef.current?.recenter();
  }, []);

  // F2: explicitly re-classify features with the Oracle. Shows the honest status
  // (success OR fail-closed "kept deterministic labels") as a transient toast. The
  // store sets `loading` while in flight, so the existing spinner + overlay cover
  // the regenerate.
  const handleReclassify = useCallback(() => {
    void (async () => {
      const res = await reclassifyFeatures();
      setReclassifyStatus(res.status);
      if (reclassifyTimer.current !== null)
        window.clearTimeout(reclassifyTimer.current);
      reclassifyTimer.current = window.setTimeout(
        () => setReclassifyStatus(null),
        5000,
      );
    })();
  }, [reclassifyFeatures]);

  // Clear the re-classify toast timer on unmount (no stray timeout firing after).
  useEffect(() => {
    return () => {
      if (reclassifyTimer.current !== null)
        window.clearTimeout(reclassifyTimer.current);
    };
  }, []);

  // Prestige / "New era": archive the current city + reset to a freshly-named
  // era. Confirmed via the EraDialog (the user types the new era name) so it is
  // never accidental. Shows the honest status as a transient toast; the store
  // sets `loading` while the archive + re-scan run, so the existing spinner +
  // overlay cover the era transition (old city sequences out, new pops in).
  const handleStartEra = useCallback(
    (name: string) => {
      setEraDialogOpen(false);
      void (async () => {
        const res = await resetToNewEra(name);
        setEraStatus(res.status);
        if (eraStatusTimer.current !== null)
          window.clearTimeout(eraStatusTimer.current);
        eraStatusTimer.current = window.setTimeout(
          () => setEraStatus(null),
          5000,
        );
      })();
    },
    [resetToNewEra],
  );

  // Clear the era-status toast timer on unmount (no stray timeout firing after).
  useEffect(() => {
    return () => {
      if (eraStatusTimer.current !== null)
        window.clearTimeout(eraStatusTimer.current);
    };
  }, []);

  // Select a building from the popup (import navigation) or elsewhere: switch the
  // popup to it and tell the renderer to ring it. (Recenter is intentionally
  // omitted to avoid a jarring camera jump on every import click; the ring marks
  // the target. Use the Recenter button to refit.)


  // Oracle citation focus: resolve a citation's fileSource to the matching
  // Polis building (suffix-match aware), ring it in the renderer and select it.
  // No-op if the file is not in the current map.
  const handleFocusFile = useCallback(
    (fileSource: string) => {
      if (!cityState) return;
      const building = findBuildingByCitation(cityState, fileSource);
      if (!building) return;
      selectPrimary({ kind: "building", building });
      handleRef.current?.recenter();
    },
    [cityState, selectPrimary],
  );

  // Select a building from the popup (import navigation) or elsewhere.
  const selectBuilding = useCallback((b: Building) => {
    selectPrimary({ kind: "building", building: b });
  }, [selectPrimary]);

  // Select an agent (from the roster).
  const selectAgent = useCallback((a: Agent) => {
    selectPrimary({ kind: "agent", agent: a });
  }, [selectPrimary]);

  const buildings = cityState?.buildings ?? [];
  const agents = cityState?.agents ?? [];
  const roads = cityState?.roads ?? [];
  const folderName = folderBasename(selectedFolder);

  // Honest empty state: desktop app with nothing loaded and not currently
  // loading. We DELIBERATELY ignore a `selectedFolder` restored from
  // localStorage here — since opening Polis no longer auto-scans, a remembered
  // folder with no city must still show the prompt (otherwise the user faces a
  // blank map with no way to map). The remembered folder only pre-fills the
  // "Map workspace" target / header label; mapping stays an explicit click.
  const showEmptyState = desktop && !cityState && !loading && !error;

  // Agents that are NOT on the map (currentFileId null or unresolved, or augur).
  const buildingIds = new Set(buildings.map((b) => b.fileId));
  const offMapAgents = agents.filter(
    (a) =>
      a.type === "augur" ||
      a.currentFileId === null ||
      !buildingIds.has(a.currentFileId),
  );
  const onMapAgents = agents.filter(
    (a) =>
      a.type !== "augur" &&
      a.currentFileId !== null &&
      buildingIds.has(a.currentFileId),
  );
  // WARNING 1: O(1) off-map lookup in the roster render loop. `offMapAgents.includes(a)`
  // inside `agents.map(...)` was O(N²); a Set of off-map agent ids makes the
  // per-row check O(1).
  const offMapAgentIds = new Set(offMapAgents.map((a) => a.agentId));

  return (
    <div
      className={
        immersive
          ? "fixed inset-0 z-50 flex flex-col bg-cream-100"
          : "relative flex h-full min-h-[480px] flex-col gap-3"
      }
    >
      {/* Header bar */}
      <div className="z-10 flex shrink-0 flex-wrap items-center justify-between gap-3 rounded-3xl border border-cream-200 bg-white px-4 py-2.5 shadow-soft-sm">
        <div className="flex items-center gap-3">
          <div className="flex h-8 w-8 items-center justify-center rounded-2xl bg-terracotta-100">
            <Castle className="h-4.5 w-4.5 text-terracotta-600" />
          </div>
          <div>
            <h3 className="text-[13px] font-semibold text-cream-800">
              Polis {cityState?.era ? `· ${cityState.era}` : ""}
            </h3>
            <p className="text-[11px] text-cream-400">
              {folderName ? (
                <span
                  className="font-medium text-cream-500"
                  title={selectedFolder ?? undefined}
                >
                  {folderName} ·{" "}
                </span>
              ) : null}
              {!immersive && (
                <>
                  {buildings.length.toLocaleString()} buildings ·{" "}
                  {onMapAgents.length} on map · {agents.length} agents
                  {usingFixture ? " · fixture" : ""}
                </>
              )}
            </p>
          </div>
        </div>
        <div className="flex items-center gap-1.5">
          {desktop && (
            <HeaderBtn
              onClick={() => void handleOpenFolder()}
              label={selectedFolder ? "Change folder" : "Open folder"}
              icon={<FolderOpen className="h-4 w-4" />}
            />
          )}
          <HeaderBtn
            onClick={handleRecenter}
            label="Recenter"
            icon={<Crosshair className="h-4 w-4" />}
          />
          {desktop && cityState && buildings.length > 0 && (
            <HeaderBtn
              onClick={handleReclassify}
              label="Re-classify"
              spinning={loading}
              disabled={loading}
              title="Ask the Oracle to name, describe, and merge the city's quarters"
              icon={<Sparkles className="h-4 w-4" />}
            />
          )}
          {desktop && cityState && buildings.length > 0 && (
            <HeaderBtn
              onClick={() => setEraDialogOpen(true)}
              label="New era"
              disabled={loading}
              title="Archive this city to a snapshot and begin a fresh era (the old city is erected as a monument)"
              icon={<Trophy className="h-4 w-4" />}
            />
          )}
          <HeaderBtn
            onClick={handleRefresh}
            label="Refresh"
            spinning={loading}
            icon={<RefreshCw className="h-4 w-4" />}
          />
          <HeaderBtn
            onClick={() => setImmersive((v) => !v)}
            label={immersive ? "Exit" : "Immersive"}
            icon={
              immersive ? (
                <Minimize2 className="h-4 w-4" />
              ) : (
                <Maximize2 className="h-4 w-4" />
              )
            }
          />
        </div>
      </div>

      {/* Map + overlays */}
      <div className="relative min-h-0 flex-1 overflow-hidden rounded-3xl border border-cream-200 bg-cream-50 shadow-soft">
        <div
          ref={hostRef}
          tabIndex={-1}
          onFocus={() => { polisFocusedRef.current = true; }}
          onBlur={() => { polisFocusedRef.current = false; }}
          className="absolute inset-0 outline-none"
        />

        {/* Empty state — no folder mapped yet (desktop only). Workspace-aware:
            if the app has a workspace folder, the primary action MAPS it; the
            "Open folder" override is always available. Mapping requires a
            registered project root (or the management root) — not Oracle indexing. */}
        {showEmptyState && (
          <Overlay>
            <div className="max-w-[420px] rounded-2xl border border-cream-200 bg-white px-6 py-6 text-center shadow-soft">
              <div className="mx-auto mb-3 flex h-12 w-12 items-center justify-center rounded-2xl bg-terracotta-100">
                <FolderOpen className="h-6 w-6 text-terracotta-600" />
              </div>
              {workspaceRoot ? (
                <>
                  <p className="text-[14px] font-semibold text-cream-800">
                    Map your workspace folder
                  </p>
                  <p className="mt-1 text-[12px] text-cream-500">
                    Polis can map your workspace folder
                    {folderBasename(workspaceRoot)
                      ? ` (${folderBasename(workspaceRoot)})`
                      : ""}{" "}
                    into a city from the files on disk, or you can open a
                    different project folder here. Oracle indexing is not required.
                  </p>
                  <div className="mt-4 flex flex-wrap items-center justify-center gap-2">
                    <button
                      onClick={() => void loadFolder(workspaceRoot)}
                      title={workspaceRoot}
                      className="inline-flex items-center gap-2 rounded-xl bg-terracotta px-4 py-2 text-[13px] font-medium text-white hover:bg-terracotta-500"
                    >
                      <FolderOpen className="h-4 w-4" />
                      Map workspace
                    </button>
                    <button
                      onClick={() => void handleOpenFolder()}
                      className="inline-flex items-center gap-2 rounded-xl border border-cream-200 px-4 py-2 text-[13px] font-medium text-cream-600 hover:bg-cream-100 hover:text-cream-800"
                    >
                      Open folder…
                    </button>
                  </div>
                </>
              ) : (
                <>
                  <p className="text-[14px] font-semibold text-cream-800">
                    Choose a folder to map its city
                  </p>
                  <p className="mt-1 text-[12px] text-cream-500">
                    Open a folder that belongs to one of your projects. Polis
                    builds the city from the files on disk — no indexing step
                    needed.
                  </p>
                  <button
                    onClick={() => void handleOpenFolder()}
                    className="mt-4 inline-flex items-center gap-2 rounded-xl bg-terracotta px-4 py-2 text-[13px] font-medium text-white hover:bg-terracotta-500"
                  >
                    <FolderOpen className="h-4 w-4" />
                    Open folder…
                  </button>
                </>
              )}
            </div>
          </Overlay>
        )}

        {/* Loading overlay — covers BOTH the backend scan (`loading`, before any
            cityState arrives) AND the renderer's non-blocking chunked build
            (`build`, after cityState arrives, while batches are placed across
            frames). The build phase shows a live count so a large city reads as
            steady progress, not a freeze. */}
        {((loading && !cityState) || build) && (
          <Overlay>
            <div className="flex items-center gap-2 text-[13px] text-cream-500">
              <div className="h-4 w-4 animate-spin rounded-full border-2 border-cream-300 border-t-terracotta" />
              {build && build.total > 0
                ? `Generating the Polis… ${build.done.toLocaleString()} / ${build.total.toLocaleString()} buildings`
                : "Generating the Polis…"}
            </div>
          </Overlay>
        )}

        {/* Error: blocking overlay only when there is no city yet. When a city is
            already on screen, a failed re-map/switch must not wipe the map —
            surface a non-destructive banner so the real reason is still visible. */}
        {folderLoadErrorSurface(error, !!cityState) === "blocking" && (
          <Overlay>
            <div className="max-w-[440px] rounded-2xl border border-coral/20 bg-white px-5 py-4 text-center shadow-sm">
              <p className="text-[13px] font-semibold text-coral-dark">
                Polis unavailable
              </p>
              <p className="mt-1 text-[12px] text-cream-500">{error}</p>
              <button
                onClick={handleRefresh}
                className="mt-3 rounded-xl bg-terracotta px-3 py-1.5 text-[12px] font-medium text-white hover:bg-terracotta-500"
              >
                Retry
              </button>
            </div>
          </Overlay>
        )}
        {folderLoadErrorSurface(error, !!cityState) === "banner" && (
          <div className="pointer-events-none absolute left-1/2 top-3 z-30 -translate-x-1/2">
            <div className="pointer-events-auto flex max-w-[min(480px,92vw)] items-start gap-2 rounded-xl border border-coral/30 bg-white/95 px-3 py-2 text-[12px] text-cream-700 shadow-soft-md backdrop-blur">
              <div className="min-w-0 flex-1">
                <p className="font-semibold text-coral-dark">Could not map folder</p>
                <p className="mt-0.5 text-cream-500">{error}</p>
              </div>
              <button
                onClick={handleRefresh}
                className="shrink-0 rounded-lg bg-terracotta px-2.5 py-1 text-[11px] font-medium text-white hover:bg-terracotta-500"
              >
                Retry
              </button>
            </div>
          </div>
        )}

        {/* Empty overlay (loaded, but no buildings) */}
        {cityState && buildings.length === 0 && !loading && (
          <Overlay>
            <p className="text-[13px] text-cream-500">
              The backend returned no buildings to map.
            </p>
          </Overlay>
        )}

        {/* Agent roster (off-map agents are honest about being off-map) */}
        {agents.length > 0 && (
          <div className="pointer-events-auto absolute left-3 top-3 z-10 w-[212px] rounded-2xl border border-cream-200 bg-white/95 p-3 shadow-soft-sm backdrop-blur">
            <h4 className="mb-1 flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-cream-400">
              <Bot className="h-3.5 w-3.5" /> Real agents
            </h4>
            {/* Honest distinction (L3): this roster lists ONLY real AI agents
                (city.agents), each marked on the map by a gold arrow + glow. The
                figures strolling the streets are DECORATIVE townsfolk (scenery)
                rendered by the AmbientLayer — never real agents, never in this
                list, never glowing. Stated here so the roster can't be mistaken
                for including them; the Guide panel explains the cue in full. */}
            <p className="mb-2 text-[10px] leading-4 text-cream-400">
              Real AI agents only (gold arrow on the map). Strolling townsfolk are
              decorative scenery.
            </p>
            <ul className="space-y-1">
              {agents.map((a) => {
                const off = offMapAgentIds.has(a.agentId);
                const isSel =
                  selected?.kind === "agent" && selected.agent.agentId === a.agentId;
                const modelLabel = a.model ? shortModelLabel(a.model) : "";
                return (
                  <li key={a.agentId}>
                    <button
                      onClick={() => selectAgent(a)}
                      title="Inspect this agent"
                      className={`flex w-full items-center gap-2 rounded-lg px-1.5 py-1 text-left transition-colors ${
                        isSel
                          ? "bg-terracotta-100 ring-1 ring-terracotta-200"
                          : "hover:bg-cream-100"
                      }`}
                    >
                      <span
                        className="h-2.5 w-2.5 shrink-0 rounded-full"
                        style={{
                          backgroundColor: `#${agentColor(a.type, a.color)
                            .toString(16)
                            .padStart(6, "0")}`,
                        }}
                      />
                      <div className="min-w-0 flex-1">
                        <p className="truncate text-[12px] font-medium text-cream-700">
                          {agentTypeLabel(a.type)}
                          {modelLabel ? (
                            <span className="ml-1 font-normal text-cream-400">
                              {modelLabel}
                            </span>
                          ) : null}
                        </p>
                        {a.currentTask && (
                          <p className="truncate text-[11px] text-cream-400">
                            {a.currentTask}
                          </p>
                        )}
                      </div>
                      {off && (
                        <span title="Off map — working in a folder not currently mapped. Map that folder to see this agent on the map.">
                          <MapPinOff className="h-3.5 w-3.5 shrink-0 text-cream-300" />
                        </span>
                      )}
                    </button>
                  </li>
                );
              })}
            </ul>

            {/* OFF-MAP summary (GAP B): be explicit that some agents are working
                in folders the current map doesn't cover, and how to see them. */}
            {offMapAgents.length > 0 && (
              <div className="mt-2 border-t border-cream-200 pt-2">
                <p className="flex items-start gap-1.5 text-[10px] leading-4 text-cream-400">
                  <MapPinOff className="mt-0.5 h-3 w-3 shrink-0" />
                  <span>
                    {offMapAgents.length} agent
                    {offMapAgents.length === 1 ? "" : "s"} working in other
                    folders. Map that folder to see them on the map.
                  </span>
                </p>
              </div>
            )}
          </div>
        )}

        {/* F2 re-classify status toast (honest: success OR fail-closed). */}
        {reclassifyStatus && (
          <div className="pointer-events-none absolute left-1/2 top-3 z-30 -translate-x-1/2">
            <div className="pointer-events-auto flex items-center gap-2 rounded-xl border border-terracotta-200 bg-white/95 px-3 py-2 text-[12px] text-cream-700 shadow-soft-md backdrop-blur">
              <Sparkles className="h-3.5 w-3.5 shrink-0 text-terracotta-500" />
              <span>{reclassifyStatus}</span>
            </div>
          </div>
        )}

        {/* Prestige / new-era status toast (honest: success OR error). Offset
            below the re-classify toast so both can show without overlapping. */}
        {eraStatus && (
          <div className="pointer-events-none absolute left-1/2 top-14 z-30 -translate-x-1/2">
            <div className="pointer-events-auto flex items-center gap-2 rounded-xl border border-terracotta-200 bg-white/95 px-3 py-2 text-[12px] text-cream-700 shadow-soft-md backdrop-blur">
              <Trophy className="h-3.5 w-3.5 shrink-0 text-terracotta-500" />
              <span>{eraStatus}</span>
            </div>
          </div>
        )}

        {/* Prestige / "New era" guarded confirm dialog (Tauri-only path; the
            button only renders on desktop). The user types the new era name —
            an explicit, non-accidental confirmation of a mildly destructive act
            (the city is archived to a snapshot and reset). */}
        {eraDialogOpen && (
          <EraDialog
            currentEra={cityState?.era ?? null}
            onConfirm={handleStartEra}
            onClose={() => setEraDialogOpen(false)}
          />
        )}

        {/* Inspect sidebar */}
        <InspectSidebar
          subject={selected ?? (selectedResource ? { kind: "resource", site: selectedResource } : null)}
          city={cityState}
          onClose={() => selectPrimary(null)}
          onSelectBuilding={selectBuilding}
        />

        {/* Videogame-style bottom control bar (Guide + Legend + File types…).
            Lives inside the map area; its wrapper is pointer-events-none so
            pan/zoom pass through everywhere except over the bar + its panels.
            Shown whenever a city is loaded — including the mapped-but-empty
            case (0 buildings), so File types stays reachable to undo a filter
            that emptied the city (F69). Pre-mapping empty state is unchanged. */}
        {cityState && (
          <PolisBottomBar
            buildingCount={buildings.length}
            roadCount={roads.length}
            agentCount={agents.length}
            onFocusFile={handleFocusFile}
            onSelectBuilding={selectBuilding}
            handleRef={handleRef}
            viewportReady={ready}
            immersive={immersive}
            polisFocusedRef={polisFocusedRef}
            filterSets={filterSets}
          />
        )}
      </div>
    </div>
  );
}

function HeaderBtn({
  onClick,
  label,
  icon,
  spinning,
  disabled,
  title,
}: {
  onClick: () => void;
  label: string;
  icon: React.ReactNode;
  spinning?: boolean;
  disabled?: boolean;
  title?: string;
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      title={title}
      className={`flex items-center gap-1.5 rounded-xl border border-cream-200 px-2.5 py-1.5 text-[12px] font-medium transition-colors ${
        disabled
          ? "cursor-not-allowed text-cream-300"
          : "text-cream-600 hover:bg-cream-100 hover:text-cream-800"
      }`}
    >
      <span className={spinning ? "animate-spin" : ""}>{icon}</span>
      <span className="hidden sm:inline">{label}</span>
    </button>
  );
}

function Overlay({ children }: { children: React.ReactNode }) {
  return (
    <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
      <div className="pointer-events-auto">{children}</div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// EraDialog — the guarded "begin a new era" confirmation. The user must TYPE a
// non-empty era name to confirm (so the mildly-destructive archive+reset is
// never accidental). On-brand (cream / terracotta), submits on Enter, closes on
// Escape / scrim click / Cancel.
// ---------------------------------------------------------------------------

function EraDialog({
  currentEra,
  onConfirm,
  onClose,
}: {
  currentEra: string | null;
  onConfirm: (name: string) => void;
  onClose: () => void;
}) {
  const [name, setName] = useState("");
  const trimmed = name.trim();
  const canConfirm = trimmed.length > 0;

  // Close on Escape (keyboard parity with the scrim click).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="pointer-events-auto absolute inset-0 z-40 flex items-center justify-center p-4">
      {/* Scrim — click to cancel. */}
      <button
        className="absolute inset-0 bg-cream-800/30 backdrop-blur-[1px]"
        onClick={onClose}
        aria-label="Cancel new era"
      />
      <div className="relative z-10 w-[420px] max-w-full overflow-hidden rounded-2xl border-2 border-terracotta-300 bg-cream-50 shadow-soft-lg">
        <div className="flex items-center gap-3 border-b-2 border-terracotta-200 bg-gradient-to-b from-terracotta-50 to-cream-50 p-4">
          <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl border border-terracotta-300 bg-terracotta-100">
            <Trophy className="h-5 w-5 text-terracotta-600" />
          </div>
          <div className="min-w-0 flex-1">
            <h3 className="text-[15px] font-semibold text-cream-800">
              Begin a new era
            </h3>
            <p className="text-[12px] text-cream-500">
              {currentEra
                ? `Archive “${currentEra}” and start fresh.`
                : "Archive this city and start fresh."}
            </p>
          </div>
          <button
            onClick={onClose}
            className="rounded-full p-1.5 text-cream-400 hover:bg-cream-200 hover:text-cream-700"
            aria-label="Cancel new era"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="p-5">
          <p className="mb-3 text-[12.5px] leading-5 text-cream-600">
            This saves the current city to a snapshot on disk (an{" "}
            <code className="rounded bg-cream-100 px-1 py-0.5 text-[11px]">
              eras/
            </code>{" "}
            archive), erects the closing era as a monument on the map, then resets
            the city. Re-scanning grows the new era. This cannot be undone in the
            app.
          </p>
          <label className="mb-1 block text-[11px] font-semibold uppercase tracking-wider text-cream-500">
            New era name
          </label>
          <input
            autoFocus
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && canConfirm) onConfirm(trimmed);
            }}
            placeholder="e.g. Beta"
            className="w-full rounded-xl border border-cream-200 bg-white px-3 py-2 text-[13px] text-cream-800 outline-none focus:border-terracotta-300 focus:ring-2 focus:ring-terracotta-100"
          />
          <div className="mt-4 flex items-center justify-end gap-2">
            <button
              onClick={onClose}
              className="rounded-xl border border-cream-200 px-4 py-2 text-[13px] font-medium text-cream-600 hover:bg-cream-100 hover:text-cream-800"
            >
              Cancel
            </button>
            <button
              onClick={() => canConfirm && onConfirm(trimmed)}
              disabled={!canConfirm}
              className={`inline-flex items-center gap-2 rounded-xl px-4 py-2 text-[13px] font-medium text-white transition-colors ${
                canConfirm
                  ? "bg-terracotta hover:bg-terracotta-500"
                  : "cursor-not-allowed bg-cream-300"
              }`}
            >
              <Trophy className="h-4 w-4" />
              Begin era
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

export default PolisView;
