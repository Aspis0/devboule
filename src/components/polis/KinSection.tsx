// Polis P6.3 — store-connected "Kin buildings" section (lazy-loaded).
//
// Separated from InspectSidebar so the cityStore import is deferred (avoids
// triggering module-level `isTauriRuntime()` in node test environments).
//
// Fetches kin results lazily on building select using the existing sidebar
// pattern: epoch-guard against stale answers, session cache per fileId, silent
// failure. Backend command `polis_get_kin` is cache-only and fast, so this is
// cheap to call.

import { useState, useEffect, useRef } from "react";
import { Users } from "lucide-react";
import type { Building } from "../../types/city";
import { useCityStore } from "../../store/cityStore";
import { invokeBackendCommand, isTauriRuntime } from "../../context/AppContext";
import { topKin, kinBarWidth, type KinWire } from "./kinModel";

// ---------------------------------------------------------------------------
// Session-scoped kin cache (per fileId). Survives building switches within a
// session so re-clicking a building shows its kin instantly. Module scope (not
// React state) keeps it stable across renders / remounts.
// ---------------------------------------------------------------------------

type KinState =
  | { kind: "loading" }
  | { kind: "ok"; entries: KinWire[] }
  | { kind: "unavailable" };

const kinCache = new Map<string, KinState>();

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function KinSection({
  building,
  city,
  onSelectBuilding,
}: {
  building: Building;
  city: import("../../types/city").CityState | null;
  onSelectBuilding: (b: Building) => void;
}) {
  const fileId = building.fileId;
  const selectedFolder = useCityStore((s) => s.selectedFolder);
  const [state, setState] = useState<KinState>(
    () => kinCache.get(fileId) ?? { kind: "loading" },
  );
  // Monotonic epoch: only the latest request may write state. Bumped on every
  // fileId change and on unmount, so a slow in-flight call can never setState
  // after the building switched or the component left.
  const epochRef = useRef(0);

  useEffect(() => {
    const epoch = ++epochRef.current;

    // Restore from session cache if available and terminal.
    const cached = kinCache.get(fileId);
    if (cached && cached.kind !== "loading") {
      setState(cached);
      return;
    }

    // Browser harness (or no Tauri): skip the call, show muted message.
    if (!isTauriRuntime()) {
      const next: KinState = { kind: "unavailable" };
      kinCache.set(fileId, next);
      setState(next);
      return;
    }

    setState({ kind: "loading" });
    let cancelled = false;

    void (async () => {
      try {
        const result = await invokeBackendCommand<KinWire[]>(
          "polis_get_kin",
          { projectPath: selectedFolder, relPath: building.filePath },
        );
        if (cancelled || epoch !== epochRef.current) return;
        const kin = Array.isArray(result) ? result : [];
        const next: KinState =
          kin.length > 0
            ? { kind: "ok", entries: kin }
            : { kind: "unavailable" };
        kinCache.set(fileId, next);
        setState(next);
      } catch {
        if (cancelled || epoch !== epochRef.current) return;
        const next: KinState = { kind: "unavailable" };
        kinCache.set(fileId, next);
        setState(next);
      }
    })();

    return () => {
      cancelled = true;
      epochRef.current++;
    };
  }, [fileId, building.filePath, selectedFolder]);

  // Fail-open: render nothing when the backend has nothing (Oracle absent /
  // cache empty ⇒ section absent entirely).
  if (state.kind !== "ok" || state.entries.length === 0) return null;

  const top = topKin(state.entries);

  // Pre-index buildings by filePath for click → navigate lookups.
  const buildingsByPath = city
    ? new Map(city.buildings.map((b) => [b.filePath, b]))
    : new Map<string, Building>();

  return (
    <section className="mt-4">
      <h4 className="mb-2 flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-cream-400">
        <Users className="h-3.5 w-3.5" /> Kin buildings
      </h4>
      <ul className="space-y-0.5">
        {top.map((k) => {
          const target = buildingsByPath.get(k.relPath);
          const hasTarget = target !== undefined;
          // Derive display name from the relPath basename.
          const baseName = k.relPath.split("/").pop() ?? k.relPath;
          const barPct = kinBarWidth(k.score);

          return (
            <li key={k.relPath}>
              <button
                onClick={hasTarget ? () => onSelectBuilding(target) : undefined}
                disabled={!hasTarget}
                title={
                  hasTarget
                    ? `${k.relPath}  ·  score ${k.score.toFixed(2)}`
                    : `${k.relPath}  ·  not on the map`
                }
                className={`flex w-full items-center gap-2 rounded-md px-1.5 py-1 text-left transition-colors ${
                  hasTarget
                    ? "hover:bg-terracotta-50"
                    : "cursor-default opacity-50"
                }`}
              >
                <span
                  className="min-w-0 flex-1 truncate font-mono text-[12px] text-cream-600"
                >
                  {baseName}
                </span>
                {/* Thin score bar */}
                <span className="inline-flex h-1.5 w-16 shrink-0 overflow-hidden rounded-full bg-cream-200">
                  <span
                    className="h-full rounded-full bg-terracotta-400"
                    style={{ width: `${barPct}%` }}
                  />
                </span>
                <span className="w-8 shrink-0 text-right text-[11px] text-cream-400">
                  {k.score.toFixed(2)}
                </span>
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
