// PolisBottomBar — a videogame-style control bar anchored to the bottom of the
// Polis map (like a city-builder's control bar), plus the panels its boxes open.
//
// P3.1 — grown into the Command Deck with three visual clusters:
//   Status (buildings, agents, open sins) | Panels (existing + Anomalies + Filters placeholder) | Zoom (−/fit/+/%)
//
// On-brand (cream / terracotta, rounded, soft shadow). It floats ABOVE the PIXI
// canvas. POINTER-EVENTS DISCIPLINE: the outer wrapper is `pointer-events-none`
// so drag/pan/zoom pass straight through to the canvas everywhere EXCEPT over
// the bar's own pixels (the bar and any open panel re-enable `pointer-events-auto`).

import { useState, useEffect, useCallback, useRef, useMemo, memo } from "react";
import {
  X,
  HelpCircle,
  Map as MapIcon,
  Flame,
  AlertTriangle,
  Scroll,
  Minus,
  Plus,
  Maximize2,
  FileCode,
  Ruler,
  Shapes,
  Route,
  LandPlot,
  Flag,
  Trophy,
  MousePointerClick,
  Cloud,
  Bug,
  SlidersHorizontal,
  // Purpose icons (mirror InspectSidebar's mapping for consistency)
  Landmark,
  Castle,
  TowerControl,
  Store,
  Library,
  Navigation,
  Anchor,
  Warehouse,
  Droplets,
  Drama,
  Hammer,
  Share2,
  Home,
  Building2,
  Bot,
  type LucideIcon,
} from "lucide-react";
import { OracleAskPanel } from "./OracleAskPanel";
import { purposeLabel } from "../../types/city";
import { getProfile, PALETTE, PROVIDER_LIVERY } from "./palette";
import { useCityStore } from "../../store/cityStore";
import {
  buildAnomaliesPanelModel,
  type AnomalyRow,
} from "./anomaliesPanelModel";
import type { PolisHandle } from "./createPolis";
import type { Building, SinRecord } from "../../types/city";
import { normalizeRelPath } from "./anomalyLedgerModel";

// ---------------------------------------------------------------------------
// Purpose legend data (the "main purposes" + their Greek names + icons + the
// real building swatch color from the renderer palette).
// ---------------------------------------------------------------------------

const PURPOSE_ICON: Record<string, LucideIcon> = {
  townhall: Building2,
  temple: Landmark,
  fortress: Castle,
  tower: TowerControl,
  market: Store,
  library: Library,
  lighthouse: Navigation,
  harbor: Anchor,
  warehouse: Warehouse,
  baths: Droplets,
  theater: Drama,
  workshop: Hammer,
  conduit: Share2,
  house: Home,
};

const LEGEND_PURPOSES = [
  "townhall",
  "temple",
  "library",
  "market",
  "workshop",
  "fortress",
  "tower",
  "warehouse",
  "conduit",
  "house",
] as const;

const SWATCH_FALLBACK = `#${PALETTE.cream.toString(16).padStart(6, "0")}`;

function purposeSwatch(slug: string): string {
  const profile = getProfile(slug);
  if (!profile.colorTop) return SWATCH_FALLBACK;
  return `#${profile.colorTop.toString(16).padStart(6, "0")}`;
}

function providerSwatch(slug: string): string {
  return `#${(PROVIDER_LIVERY[slug] ?? 0).toString(16).padStart(6, "0")}`;
}

const LEGEND_PROVIDERS: { slug: string; label: string }[] = [
  { slug: "cloudflare", label: "Cloudflare" },
  { slug: "scaleway", label: "Scaleway" },
];

// ---------------------------------------------------------------------------
// Severity helpers
// ---------------------------------------------------------------------------

const SEVERITY_GLYPH: Record<string, string> = {
  smoke: "💨",
  fire: "🔥",
  inferno: "🌋",
};

const SEVERITY_TONE: Record<string, string> = {
  smoke: "text-cream-600",
  fire: "text-amber-dark",
  inferno: "text-coral-dark",
};

function worstSeverityColor(sinRecords: SinRecord[]): string {
  let worst = "smoke";
  for (const s of sinRecords) {
    if (s.severity === "inferno") return SEVERITY_TONE.inferno;
    if (s.severity === "fire") worst = "fire";
  }
  return SEVERITY_TONE[worst] ?? SEVERITY_TONE.smoke;
}

// ---------------------------------------------------------------------------
// Panel items registry
// ---------------------------------------------------------------------------

type PanelId = "guide" | "legend" | "filetypes" | "oracle" | "anomalies" | "filters";

interface PanelItem {
  id: PanelId;
  label: string;
  icon: LucideIcon;
  title: string;
}

const PANEL_ITEMS: PanelItem[] = [
  { id: "guide", label: "Guide", icon: HelpCircle, title: "What is this map?" },
  { id: "legend", label: "Legend", icon: MapIcon, title: "Building color legend" },
  {
    id: "filetypes",
    label: "File types",
    icon: FileCode,
    title: "Choose which file types become buildings",
  },
  {
    id: "oracle",
    label: "Oracle",
    icon: Scroll,
    title: "Ask Oracle about your codebase",
  },
  {
    id: "anomalies",
    label: "Anomalies",
    icon: Bug,
    title: "Open and ignored sins across the project",
  },
];

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

function PolisBottomBarInner({
  buildingCount,
  roadCount,
  agentCount,
  onFocusFile,
  onSelectBuilding,
  handleRef,
  viewportReady,
  immersive,
  polisFocusedRef,
  filterSets,
}: {
  buildingCount: number;
  roadCount: number;
  agentCount: number;
  onFocusFile?: (fileSource: string) => void;
  onSelectBuilding?: (b: Building) => void;
  handleRef: React.RefObject<PolisHandle | null>;
  viewportReady: boolean;
  immersive: boolean;
  polisFocusedRef: React.RefObject<boolean>;
  filterSets: import("./filterModel").FilterSets | null;
}) {
  const [open, setOpen] = useState<PanelId | null>(null);
  const toggle = (id: PanelId) => setOpen((prev) => (prev === id ? null : id));

  const handleFocusFile = (fileSource: string) => {
    onFocusFile?.(fileSource);
  };

  // --- Status cluster selectors (narrow, memoized) ---
  const sinRecords = useCityStore((s) => s.sinRecords);
  const { openSinCount, sinColor } = useMemo(() => {
    const open = sinRecords?.filter((r) => r.disposition === "open") ?? [];
    return { openSinCount: open.length, sinColor: worstSeverityColor(open) };
  }, [sinRecords]);

  const filterState = useCityStore((s) => s.filter);
  const filterActiveDot = useMemo(() => {
    return (
      filterState.categories.length > 0 ||
      filterState.minSeverity !== null ||
      filterState.features.length > 0 ||
      filterState.pathGlob !== ""
    );
  }, [filterState]);

  // --- Zoom cluster state ---
  const [zoomPct, setZoomPct] = useState(100);
  const rafRef = useRef<number | null>(null);

  useEffect(() => {
    const vp = handleRef.current?.viewport;
    if (!vp) return;

    // Sync initial zoom
    setZoomPct(Math.round(vp.scale.x * 100));

    const onFrame = () => {
      const pct = Math.round(vp.scale.x * 100);
      setZoomPct((prev) => (prev === pct ? prev : pct));
    };

    const onMoved = () => {
      if (rafRef.current === null) {
        rafRef.current = requestAnimationFrame(() => {
          rafRef.current = null;
          onFrame();
        });
      }
    };

    const onZoomed = () => {
      if (rafRef.current === null) {
        rafRef.current = requestAnimationFrame(() => {
          rafRef.current = null;
          onFrame();
        });
      }
    };

    vp.on("moved", onMoved);
    vp.on("zoomed", onZoomed);
    return () => {
      vp.off("moved", onMoved);
      vp.off("zoomed", onZoomed);
      if (rafRef.current !== null) cancelAnimationFrame(rafRef.current);
    };
  }, [handleRef, viewportReady]);

  const zoomIn = useCallback(() => {
    const vp = handleRef.current?.viewport;
    if (!vp || (vp as any).animating) return;
    const target = Math.min(vp.scale.x * 1.25, 3.0);
    vp.animate({ scale: target, time: 250, ease: "easeInOutSine" });
    setZoomPct(Math.round(target * 100));
  }, [handleRef]);

  const zoomOut = useCallback(() => {
    const vp = handleRef.current?.viewport;
    if (!vp || (vp as any).animating) return;
    const target = Math.max(vp.scale.x * 0.8, 0.15);
    vp.animate({ scale: target, time: 250, ease: "easeInOutSine" });
    setZoomPct(Math.round(target * 100));
  }, [handleRef]);

  const zoomFit = useCallback(() => {
    handleRef.current?.recenter();
  }, [handleRef]);

  // --- Keyboard zoom (+/-/0) only while Polis container is focused ---
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!polisFocusedRef.current) return;
      // Don't steal from inputs
      if (
        e.target instanceof HTMLInputElement ||
        e.target instanceof HTMLTextAreaElement
      )
        return;
      if (e.key === "+" || e.key === "=") {
        e.preventDefault();
        zoomIn();
      } else if (e.key === "-") {
        e.preventDefault();
        zoomOut();
      } else if (e.key === "0") {
        e.preventDefault();
        zoomFit();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [zoomIn, zoomOut, zoomFit, polisFocusedRef]);

  return (
    <div className="pointer-events-none absolute inset-0 z-20">
      {/* --- Panels --- */}
      {open === "guide" && (
        <HelpPanel
          buildingCount={buildingCount}
          roadCount={roadCount}
          agentCount={agentCount}
          onClose={() => setOpen(null)}
        />
      )}
      {open === "legend" && <LegendOverlay onClose={() => setOpen(null)} />}
      {open === "filetypes" && <FileTypesPanel onClose={() => setOpen(null)} />}
      {open === "oracle" && (
        <OracleAskPanel
          onFocusFile={handleFocusFile}
          onClose={() => setOpen(null)}
        />
      )}
      {open === "anomalies" && (
        <AnomaliesPanel
          onSelectBuilding={onSelectBuilding}
          handleRef={handleRef}
          onClose={() => setOpen(null)}
          filterSets={filterSets}
        />
      )}
      {open === "filters" && (
        <FiltersPanel onClose={() => setOpen(null)} filterSets={filterSets} />
      )}

      {/* --- The Deck bar (three clusters) --- */}
      <div className="absolute inset-x-0 bottom-3 flex justify-center">
        <div className="pointer-events-auto flex items-center gap-1.5 rounded-2xl border border-cream-200 bg-white/95 p-1.5 shadow-soft-md backdrop-blur">

          {/* STATUS cluster — shown ONLY in immersive mode (windowed shows counts in header) */}
          {immersive && (
            <div className="flex items-center gap-3 rounded-xl border-r border-cream-200 pr-3 text-[12px]">
              <span className="flex items-center gap-1 font-medium text-cream-600" title="Buildings">
                <Building2 className="h-3.5 w-3.5" />
                {buildingCount.toLocaleString()}
              </span>
              <span className="flex items-center gap-1 font-medium text-cream-600" title="Live agents">
                <Bot className="h-3.5 w-3.5" />
                {agentCount}
              </span>
              <span
                className={`flex items-center gap-1 font-medium ${sinColor}`}
                title="Open sins"
              >
                <Flame className="h-3.5 w-3.5" />
                {openSinCount}
              </span>
            </div>
          )}

          {/* PANELS cluster */}
          <div className="flex items-center gap-0.5">
            {PANEL_ITEMS.map((item) => {
              const Icon = item.icon;
              const active = open === item.id;
              const badge =
                item.id === "anomalies" && openSinCount > 0 ? openSinCount : null;
              return (
                <button
                  key={item.id}
                  onClick={() => toggle(item.id)}
                  title={item.title}
                  className={`relative flex items-center gap-1.5 rounded-xl px-2.5 py-1.5 text-[11px] font-medium transition-colors ${
                    active
                      ? "bg-terracotta text-white"
                      : "text-cream-600 hover:bg-cream-100 hover:text-cream-800"
                  }`}
                >
                  <Icon className="h-3.5 w-3.5" />
                  <span className="hidden sm:inline">{item.label}</span>
                  {badge != null && (
                    <span className="absolute -right-1 -top-1 flex h-4 min-w-[16px] items-center justify-center rounded-full bg-coral px-1 text-[9px] font-bold text-white">
                      {badge}
                    </span>
                  )}
                </button>
              );
            })}

            {/* Filters — active when any axis is non-default */}
            <button
              onClick={() => toggle('filters')}
              title="Filter buildings by anomaly, severity, quarter, or path"
              className={`relative flex items-center gap-1.5 rounded-xl px-2.5 py-1.5 text-[11px] font-medium transition-colors ${
                open === 'filters'
                  ? 'bg-terracotta text-white'
                  : 'text-cream-600 hover:bg-cream-100 hover:text-cream-800'
              }`}
            >
              <SlidersHorizontal className="h-3.5 w-3.5" />
              <span className="hidden sm:inline">Filters</span>
              {filterActiveDot && (
                <span className="absolute right-1 top-0.5 h-2 w-2 rounded-full bg-coral" />
              )}
            </button>
          </div>

          {/* ZOOM cluster */}
          <div className="flex items-center gap-0.5 rounded-xl border-l border-cream-200 pl-3">
            <button
              onClick={zoomOut}
              title="Zoom out (−)"
              className="flex h-6 w-6 items-center justify-center rounded-lg text-cream-500 transition-colors hover:bg-cream-100 hover:text-cream-800"
            >
              <Minus className="h-3.5 w-3.5" />
            </button>
            <button
              onClick={zoomFit}
              title="Fit to city (0)"
              className="flex h-6 w-6 items-center justify-center rounded-lg text-cream-500 transition-colors hover:bg-cream-100 hover:text-cream-800"
            >
              <Maximize2 className="h-3.5 w-3.5" />
            </button>
            <button
              onClick={zoomIn}
              title="Zoom in (+)"
              className="flex h-6 w-6 items-center justify-center rounded-lg text-cream-500 transition-colors hover:bg-cream-100 hover:text-cream-800"
            >
              <Plus className="h-3.5 w-3.5" />
            </button>
            <span className="ml-1 min-w-[36px] text-center text-[11px] font-medium text-cream-500 tabular-nums">
              {zoomPct}%
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Anomalies panel — project-wide Open / Ignored tabs with flyTo
// ---------------------------------------------------------------------------

function AnomaliesPanel({
  onSelectBuilding,
  handleRef,
  onClose,
  filterSets,
}: {
  onSelectBuilding?: (b: Building) => void;
  handleRef: React.RefObject<PolisHandle | null>;
  onClose: () => void;
  filterSets: import("./filterModel").FilterSets | null;
}) {
  const [tab, setTab] = useState<"open" | "ignored">("open");
  const sinRecords = useCityStore((s) => s.sinRecords);
  const sinActionPending = useCityStore((s) => s.sinActionPending);
  const disposeSin = useCityStore((s) => s.disposeSin);
  const buildings = useCityStore((s) => s.cityState?.buildings);
  const [now] = useState(() => Date.now());
  // F9 — inline feedback when a row click is blocked by the active filter.
  const [blockedFileId, setBlockedFileId] = useState<string | null>(null);

  const buildingFileIds = useMemo(() => {
    if (!buildings) return new Map<string, string>();
    const m = new Map<string, string>();
    for (const b of buildings) {
      m.set(normalizeRelPath(b.filePath), b.fileId);
    }
    return m;
  }, [buildings]);

  const model = useMemo(
    () => buildAnomaliesPanelModel(sinRecords, buildingFileIds, now),
    [sinRecords, buildingFileIds],
  );

  const rows = tab === "open" ? model.open : model.ignored;

  const handleClick = useCallback(
    (row: AnomalyRow) => {
      if (!row.fileId || !buildings) return;
      const building = buildings.find(
        (b) => b.fileId === row.fileId,
      );
      if (!building) return;
      // F9 — if the target is hidden by the active filter, skip flyTo+select
      // and show a transient inline note on the row.
      if (filterSets?.mode === "hide" && filterSets.ghostedFileIds.has(row.fileId)) {
        setBlockedFileId(row.fileId);
        return;
      }
      setBlockedFileId(null);
      handleRef.current?.flyTo(row.fileId);
      onSelectBuilding?.(building);
    },
    [buildings, handleRef, onSelectBuilding, filterSets],
  );

  const handleUnignore = useCallback(
    async (row: AnomalyRow) => {
      await disposeSin(row.relPath, row.sin.id, "open");
    },
    [disposeSin],
  );

  return (
    <div className="pointer-events-auto absolute bottom-16 left-1/2 w-[420px] max-w-[92vw] -translate-x-1/2 rounded-2xl border border-cream-200 bg-white/95 shadow-soft-lg backdrop-blur">
      {/* Header */}
      <div className="flex items-center justify-between border-b border-cream-100 px-3 py-2">
        <h4 className="flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-cream-500">
          <Bug className="h-3.5 w-3.5" /> Anomalies
        </h4>
        <button
          onClick={onClose}
          className="rounded-full p-1 text-cream-400 hover:bg-cream-100 hover:text-cream-700"
          aria-label="Close anomalies"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>

      {/* Tabs */}
      <div role="tablist" className="flex border-b border-cream-100">
        <button
          onClick={() => setTab("open")}
          role="tab"
          aria-selected={tab === "open"}
          className={`flex-1 px-3 py-1.5 text-[11px] font-medium transition-colors ${
            tab === "open"
              ? "border-b-2 border-terracotta text-terracotta"
              : "text-cream-400 hover:text-cream-600"
          }`}
        >
          Open ({model.openCount})
        </button>
        <button
          onClick={() => setTab("ignored")}
          role="tab"
          aria-selected={tab === "ignored"}
          className={`flex-1 px-3 py-1.5 text-[11px] font-medium transition-colors ${
            tab === "ignored"
              ? "border-b-2 border-terracotta text-terracotta"
              : "text-cream-400 hover:text-cream-600"
          }`}
        >
          Ignored ({model.ignored.length})
        </button>
      </div>

      {/* Rows */}
      <div className="max-h-[260px] overflow-y-auto p-2">
        {rows.length === 0 ? (
          <p className="py-6 text-center text-[12px] italic text-cream-400">
            {tab === "open"
              ? "The augurs find the city untroubled."
              : "No ignored sins."}
          </p>
        ) : (
          <ul className="space-y-1">
            {rows.map((row) => (
              <AnomalyRowItem
                key={`${row.sin.id}-${row.sin.disposition}`}
                row={row}
                tab={tab}
                onClick={handleClick}
                onUnignore={handleUnignore}
                pending={sinActionPending.includes(row.sin.id)}
                filterBlocked={row.fileId !== null && row.fileId === blockedFileId}
              />
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

const AnomalyRowItem = memo(function AnomalyRowItem({
  row,
  tab,
  onClick,
  onUnignore,
  pending,
  filterBlocked,
}: {
  row: AnomalyRow;
  tab: "open" | "ignored";
  onClick: (row: AnomalyRow) => void;
  onUnignore: (row: AnomalyRow) => void;
  pending: boolean;
  filterBlocked?: boolean;
}) {
  const SinIcon = row.sin.severity === "smoke" ? AlertTriangle : Flame;
  const glyph = SEVERITY_GLYPH[row.sin.severity] ?? "💨";
  const canClick = row.fileId !== null;

  return (
    <li>
      <div
        className={`flex items-start gap-2 rounded-xl border px-2.5 py-2 text-[11px] transition-colors ${
          tab === "ignored" ? "opacity-60" : ""
        } ${canClick ? "cursor-pointer hover:border-terracotta-300 hover:bg-terracotta-50" : ""}`}
        onClick={canClick ? () => onClick(row) : undefined}
        role={canClick ? "button" : undefined}
        tabIndex={canClick ? 0 : undefined}
        onKeyDown={
          canClick
            ? (e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  onClick(row);
                }
              }
            : undefined
        }
      >
        {/* Glyph + severity pip */}
        <span className="mt-0.5 text-[13px]">{glyph}</span>
        <SinIcon className="mt-0.5 h-3 w-3 shrink-0 text-cream-400" />

        {/* Rule + location + age */}
        <div className="min-w-0 flex-1">
          <span className="rounded bg-cream-200 px-1 py-0.5 font-mono text-[9px] text-cream-600">
            {row.sin.ruleId}
          </span>
          <span className="ml-1 font-mono text-[10px] text-cream-500">
            {row.relPath}
            {row.sin.line != null ? `:${row.sin.line}` : ""}
          </span>
          <span className="ml-1 text-[10px] text-cream-400">{row.age}</span>
        </div>

        {/* Ignored tab: Un-ignore button */}
        {tab === "ignored" && (
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              onUnignore(row);
            }}
            disabled={pending}
            title="Un-ignore"
            className="shrink-0 rounded px-1.5 py-0.5 text-[10px] text-cream-600 transition hover:bg-cream-200 disabled:opacity-50"
          >
            {pending ? "…" : "Un-ignore"}
          </button>
        )}
      </div>
      {/* F9 — inline note when filter blocks navigation */}
      {filterBlocked && (
        <p className="mt-1 pl-6 text-[10px] italic text-amber-dark">
          hidden by the active filter
        </p>
      )}
    </li>
  );
});

// ---------------------------------------------------------------------------
// File-types panel
// ---------------------------------------------------------------------------

function FileTypesPanel({ onClose }: { onClose: () => void }) {
  const getScanExtensions = useCityStore((s) => s.getScanExtensions);
  const applyScanExtensions = useCityStore((s) => s.applyScanExtensions);
  const [available, setAvailable] = useState<string[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    getScanExtensions()
      .then((r) => {
        if (!alive) return;
        setAvailable(r.available);
        setSelected(new Set(r.enabled));
        setLoading(false);
      })
      .catch((e) => {
        if (!alive) return;
        setErr(e instanceof Error ? e.message : String(e));
        setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [getScanExtensions]);

  const toggle = (ext: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(ext)) next.delete(ext);
      else next.add(ext);
      return next;
    });

  const disabled = selected.size === 0 || saving || loading;

  const apply = async () => {
    if (disabled) return;
    setSaving(true);
    setErr(null);
    try {
      await applyScanExtensions([...selected]);
      onClose();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
      setSaving(false);
    }
  };

  return (
    <div className="pointer-events-auto absolute bottom-16 left-1/2 w-[340px] max-w-[92vw] -translate-x-1/2 rounded-2xl border border-cream-200 bg-white/95 p-3 shadow-soft-lg backdrop-blur">
      <div className="mb-2 flex items-center justify-between">
        <h4 className="flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-cream-500">
          <FileCode className="h-3.5 w-3.5" /> File types
        </h4>
        <button
          onClick={onClose}
          className="rounded-full p-1 text-cream-400 hover:bg-cream-100 hover:text-cream-700"
          aria-label="Close file types"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>
      <p className="mb-2 text-[11px] leading-4 text-cream-500">
        Which file extensions become buildings in this folder. Saved per folder;
        applying rebuilds the city.
      </p>
      {loading ? (
        <p className="py-3 text-center text-[12px] text-cream-400">Loading…</p>
      ) : (
        <>
          <div className="flex flex-wrap gap-1.5">
            {available.map((ext) => {
              const on = selected.has(ext);
              return (
                <button
                  key={ext}
                  onClick={() => toggle(ext)}
                  className={`rounded-lg border px-2 py-1 font-mono text-[11px] transition-colors ${
                    on
                      ? "border-terracotta bg-terracotta text-white"
                      : "border-cream-200 bg-cream-50 text-cream-500 hover:bg-cream-100"
                  }`}
                >
                  .{ext}
                </button>
              );
            })}
          </div>
          {err && <p className="mt-2 text-[11px] text-coral-dark">{err}</p>}
          <div className="mt-3 flex items-center justify-between border-t border-cream-100 pt-2">
            <span className="text-[11px] text-cream-400">
              {selected.size} selected
            </span>
            <button
              onClick={() => void apply()}
              disabled={disabled}
              className={`rounded-xl px-3 py-1.5 text-[12px] font-medium text-white transition-colors ${
                disabled
                  ? "cursor-not-allowed bg-cream-300"
                  : "bg-terracotta hover:bg-terracotta-500"
              }`}
            >
              {saving ? "Rebuilding…" : "Apply & rebuild"}
            </button>
          </div>
        </>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Legend overlay
// ---------------------------------------------------------------------------

function LegendOverlay({ onClose }: { onClose: () => void }) {
  return (
    <div className="pointer-events-auto absolute bottom-16 left-1/2 w-[300px] max-w-[92vw] -translate-x-1/2 rounded-2xl border border-cream-200 bg-white/95 p-3 shadow-soft-lg backdrop-blur">
      <div className="mb-2 flex items-center justify-between">
        <h4 className="flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-cream-500">
          <MapIcon className="h-3.5 w-3.5" /> Building legend
        </h4>
        <button
          onClick={onClose}
          className="rounded-full p-1 text-cream-400 hover:bg-cream-100 hover:text-cream-700"
          aria-label="Close legend"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>
      <ul className="grid grid-cols-2 gap-x-3 gap-y-1.5">
        {LEGEND_PURPOSES.map((slug) => {
          const Icon = PURPOSE_ICON[slug] ?? FileCode;
          return (
            <li key={slug} className="flex items-center gap-2">
              <span
                className="h-3 w-3 shrink-0 rounded-sm ring-1 ring-black/5"
                style={{ backgroundColor: purposeSwatch(slug) }}
              />
              <Icon className="h-3.5 w-3.5 shrink-0 text-cream-500" />
              <span className="min-w-0 truncate text-[11px] text-cream-600">
                {purposeLabel(slug)}
              </span>
            </li>
          );
        })}
      </ul>

      {/* TECH LIVERY — provider pennants. A small flag on a building's roof
          marks files tied to a cloud provider (its color = the provider). */}
      <div className="mt-3 border-t border-cream-100 pt-2">
        <h5 className="mb-1.5 flex items-center gap-1.5 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
          <Flag className="h-3 w-3" /> Provider pennants
        </h5>
        <ul className="grid grid-cols-2 gap-x-3 gap-y-1.5">
          {LEGEND_PROVIDERS.map((p) => (
            <li key={p.slug} className="flex items-center gap-2">
              <span
                className="h-3 w-3 shrink-0 rounded-sm ring-1 ring-black/5"
                style={{ backgroundColor: providerSwatch(p.slug) }}
              />
              <span className="min-w-0 truncate text-[11px] text-cream-600">
                {p.label}
              </span>
            </li>
          ))}
        </ul>
      </div>

      {/* CLOUD HARBOUR — external services. Small outposts at the map's seaward
          margin mirror your LIVE Scaleway/Cloudflare resources; a status lamp
          shows whether each is running, stopped, starting, or in error. */}
      <div className="mt-3 border-t border-cream-100 pt-2">
        <h5 className="mb-1.5 flex items-center gap-1.5 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
          <Cloud className="h-3 w-3" /> Cloud harbour
        </h5>
        <ul className="grid grid-cols-2 gap-x-3 gap-y-1.5">
          {LEGEND_PROVIDERS.map((p) => (
            <li key={p.slug} className="flex items-center gap-2">
              <span
                className="h-3 w-3 shrink-0 rounded-sm ring-1 ring-black/5"
                style={{ backgroundColor: providerSwatch(p.slug) }}
              />
              <span className="min-w-0 truncate text-[11px] text-cream-600">
                {p.label}
              </span>
            </li>
          ))}
        </ul>
        <ul className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1">
          {[
            { label: "Running", cls: "bg-sage" },
            { label: "Starting", cls: "bg-amber" },
            { label: "Stopped", cls: "bg-cream-400" },
            { label: "Error", cls: "bg-coral" },
          ].map((s) => (
            <li key={s.label} className="flex items-center gap-1.5">
              <span className={`h-2 w-2 shrink-0 rounded-full ${s.cls}`} />
              <span className="text-[10px] text-cream-500">{s.label}</span>
            </li>
          ))}
        </ul>
      </div>

      {/* ERA MONUMENTS — prestige arches on the LANDWARD margin. Each marks a
          closing era; its inscription is real (file count + disasters still
          burning at era close). Cumulative: a new era adds another arch. */}
      <div className="mt-3 border-t border-cream-100 pt-2">
        <h5 className="mb-1.5 flex items-center gap-1.5 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
          <Trophy className="h-3 w-3" /> Era monuments
        </h5>
        <p className="text-[10px] leading-4 text-cream-500">
          Triumphal arches at the landward edge mark each past era. The
          inscription is real — the file count and disasters still burning when
          that era ended. Starting a new era archives the city to a snapshot and
          adds a monument.
        </p>
      </div>

      <p className="mt-2 border-t border-cream-100 pt-2 text-[10px] leading-4 text-cream-400">
        The shape/type of a building shows what its file does. Bigger building =
        more lines of code. A small flag on the roof marks the cloud provider, and
        outposts at the shoreline are your live cloud resources.
      </p>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Help panel
// ---------------------------------------------------------------------------

function HelpPanel({
  buildingCount,
  roadCount,
  agentCount,
  onClose,
}: {
  buildingCount: number;
  roadCount: number;
  agentCount: number;
  onClose: () => void;
}) {
  return (
    <div className="pointer-events-auto absolute inset-0 flex items-center justify-center p-4">
      {/* Scrim — click to close. */}
      <button
        className="absolute inset-0 bg-cream-800/30 backdrop-blur-[1px]"
        onClick={onClose}
        aria-label="Close guide"
      />
      <div className="relative z-10 flex max-h-full w-[560px] max-w-full flex-col overflow-hidden rounded-2xl border-2 border-terracotta-300 bg-cream-50 shadow-soft-lg">
        {/* Header */}
        <div className="flex items-center gap-3 border-b-2 border-terracotta-200 bg-gradient-to-b from-terracotta-50 to-cream-50 p-4">
          <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl border border-terracotta-300 bg-terracotta-100">
            <MapIcon className="h-5 w-5 text-terracotta-600" />
          </div>
          <div className="min-w-0 flex-1">
            <h3 className="text-[15px] font-semibold text-cream-800">
              Reading the city
            </h3>
            <p className="text-[12px] text-cream-500">
              Your code, drawn as a town. Here is what everything means.
            </p>
          </div>
          <button
            onClick={onClose}
            className="rounded-full p-1.5 text-cream-400 hover:bg-cream-200 hover:text-cream-700"
            aria-label="Close guide"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        {/* Body */}
        <div className="space-y-4 overflow-y-auto p-5 text-[13px] leading-6 text-cream-600">
          <HelpSection
            icon={Building2}
            title="Buildings are files"
            count={`${buildingCount.toLocaleString()} buildings`}
          >
            Every building is one file in your project. Click any building to open
            an info panel with what the file does, where it lives, its size, and
            everything it connects to.
          </HelpSection>

          <HelpSection icon={Ruler} title="Bigger means longer">
            A building's size reflects how many lines of code its file has — from a
            small <em>Hut</em> up to a towering <em>Monument</em>. A big building is
            a long, heavy file; a tiny one is a short file.
          </HelpSection>

          <HelpSection icon={Shapes} title="Shape = what the file does">
            The building's TYPE (its shape and icon) tells you the file's job. The
            main types:
            <ul className="mt-2 grid grid-cols-1 gap-1.5 sm:grid-cols-2">
              {LEGEND_PURPOSES.map((slug) => {
                const Icon = PURPOSE_ICON[slug] ?? FileCode;
                return (
                  <li key={slug} className="flex items-center gap-2">
                    <span
                      className="h-3 w-3 shrink-0 rounded-sm ring-1 ring-black/5"
                      style={{ backgroundColor: purposeSwatch(slug) }}
                    />
                    <Icon className="h-3.5 w-3.5 shrink-0 text-cream-500" />
                    <span className="text-[12px] text-cream-600">
                      {purposeLabel(slug)}
                    </span>
                  </li>
                );
              })}
            </ul>
            <span className="mt-1 block text-[12px] text-cream-400">
              Each name is shown in English with its Greek city-builder name in
              brackets.
            </span>
          </HelpSection>

          <HelpSection
            icon={Route}
            title="Roads are import connections"
            count={`${roadCount.toLocaleString()} roads`}
          >
            A road between two buildings means one file uses (imports) the other.
            Busy, widely-used files get thick cobbled avenues; one-off links are
            faint paths. Roads reveal which files everything depends on. Zoom in
            on a busy avenue and you'll see{" "}
            <strong className="text-cream-700">merchant porters</strong> (figures
            carrying a goods sack) walking the road from the imported file to the
            file that uses it — the busiest dependencies carry the most porters.
            Unlike the decorative townsfolk, a porter is real data: click one to
            see exactly which file imports which.
          </HelpSection>

          <HelpSection
            icon={Bot}
            title="Agents vs. townsfolk"
            count={`${agentCount.toLocaleString()} agents`}
          >
            Real AI agents working on your code are marked with a small{" "}
            <strong className="text-cream-700">gold arrow</strong> above their
            head and a glow on the building they are editing — click one to see
            its role, status, and task. The other figures wandering the streets
            are <strong className="text-cream-700">decorative townsfolk</strong>:
            scenery to make the city feel alive. They have no arrow, are not
            clickable, and never represent a real agent.
          </HelpSection>

          <HelpSection icon={Bot} title="A living city">
            The town breathes on its own: townsfolk stroll the busiest avenues and
            mill around the market and town hall, and a slow{" "}
            <strong className="text-cream-700">day cycle</strong> warms the light
            from midday toward a golden evening and back. This is ambience only —
            it never reflects real activity. Watch the gold arrows for that.
          </HelpSection>

          <HelpSection icon={LandPlot} title="Districts group related code">
            Tinted zones are districts — clusters of related files (for example a
            web/worker area or a core area). Each district is labelled and shaded
            with its own accent color.
          </HelpSection>

          <HelpSection icon={Flame} title="Burning buildings have issues">
            Smoke or flames mean the file has detected problems (we call them
            "urban sins"): smoke = minor, fire = real, an inferno = serious. Open
            the building to read each issue in plain language.
          </HelpSection>

          <HelpSection icon={MousePointerClick} title="Controls">
            <ul className="space-y-1">
              <li>
                <strong className="text-cream-700">Drag</strong> the map to pan
                around.
              </li>
              <li>
                <strong className="text-cream-700">Scroll / wheel</strong> to zoom
                in and out.
              </li>
              <li>
                <strong className="text-cream-700">Click</strong> a building or
                agent to inspect it; click empty ground to close the panel.
              </li>
            </ul>
          </HelpSection>
        </div>
      </div>
    </div>
  );
}

function HelpSection({
  icon: Icon,
  title,
  count,
  children,
}: {
  icon: LucideIcon;
  title: string;
  count?: string;
  children: React.ReactNode;
}) {
  return (
    <section className="rounded-xl border border-cream-200 bg-white p-3">
      <div className="mb-1.5 flex items-center gap-2">
        <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-terracotta-100">
          <Icon className="h-4 w-4 text-terracotta-600" />
        </span>
        <h4 className="flex-1 text-[13px] font-semibold text-cream-800">
          {title}
        </h4>
        {count && (
          <span className="shrink-0 rounded-full bg-cream-100 px-2 py-0.5 text-[10px] font-semibold text-cream-500">
            {count}
          </span>
        )}
      </div>
      <div className="text-[12.5px] leading-5 text-cream-600">{children}</div>
    </section>
  );
}


export const PolisBottomBar = memo(PolisBottomBarInner);
export default PolisBottomBar;

// ---------------------------------------------------------------------------
// Filters panel (P3.2)
// ---------------------------------------------------------------------------

const ANOMALY_RULE_IDS = [
  "secret",
  "dep-cycle",
  "todo-density",
  "dead-export",
  "env-missing",
  "complexity",
  "god-file",
  "test-gap",
  "clone",
] as const;

const RULE_GLYPH: Record<string, string> = {
  "secret": "\u{1F512}",
  "dep-cycle": "\u{1F504}",
  "todo-density": "\u{1F4DD}",
  "dead-export": "\u{1F480}",
  "env-missing": "\u{1F527}",
  "complexity": "\u{1F300}",
  "god-file": "\u{1F3DB}",
  "test-gap": "\u{1F573}",
  "clone": "\u{1F46F}",
};

const RULE_LABEL: Record<string, string> = {
  "secret": "Secrets",
  "dep-cycle": "Dep cycle",
  "todo-density": "TODO density",
  "dead-export": "Dead export",
  "env-missing": "Env missing",
  "complexity": "Complexity",
  "god-file": "God file",
  "test-gap": "Test gap",
  "clone": "Clones",
};

const SEVERITY_OPTIONS = [
  { key: null, label: "All" },
  { key: "fire" as const, label: "\u{2265}\u{46}\u{69}\u{72}\u{65}" },
  { key: "inferno" as const, label: "\u{2265}\u{49}\u{6E}\u{66}\u{65}\u{72}\u{6E}\u{6F}" },
] as const;

function useDebouncedValue<T>(value: T, delay: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setDebounced(value), delay);
    return () => clearTimeout(timer);
  }, [value, delay]);
  return debounced;
}

function FiltersPanel({ onClose, filterSets }: { onClose: () => void; filterSets: import("./filterModel").FilterSets | null }) {
  const filter = useCityStore((s) => s.filter);
  const setFilter = useCityStore((s) => s.setFilter);
  const resetFilterAction = useCityStore((s) => s.resetFilter);
  const sinRecords = useCityStore((s) => s.sinRecords);
  const cityState = useCityStore((s) => s.cityState);

  // Per-category open count
  const catCounts = useMemo(() => {
    const open = sinRecords?.filter((r) => r.disposition === "open") ?? [];
    const counts: Record<string, number> = {};
    for (const r of open) {
      counts[r.ruleId] = (counts[r.ruleId] ?? 0) + 1;
    }
    return counts;
  }, [sinRecords]);

  // District/feature names
  const features = useMemo(() => {
    const set = new Map<string, string>();
    if (cityState?.features) {
      for (const f of cityState.features) {
        set.set(f.id, f.label);
      }
    }
    // Also collect featureIds from buildings
    for (const b of cityState?.buildings ?? []) {
      if (b.featureId && !set.has(b.featureId)) {
        set.set(b.featureId, b.featureId);
      }
    }
    return [...set.entries()];
  }, [cityState]);

  // Path glob local state with debounce
  const [pathInput, setPathInput] = useState(filter.pathGlob);
  const debouncedPath = useDebouncedValue(pathInput, 300);

  // Sync debounced path to store
  useEffect(() => {
    if (debouncedPath !== filter.pathGlob) {
      setFilter({ pathGlob: debouncedPath });
    }
  }, [debouncedPath, filter.pathGlob, setFilter]);

  // Sync store → local on reset
  useEffect(() => {
    setPathInput(filter.pathGlob);
  }, [filter.pathGlob]);

  const toggleCategory = (ruleId: string) => {
    const cats = filter.categories;
    const next = cats.includes(ruleId)
      ? cats.filter((c) => c !== ruleId)
      : [...cats, ruleId];
    setFilter({ categories: next });
  };

  const toggleFeature = (id: string) => {
    const feats = filter.features;
    const next = feats.includes(id)
      ? feats.filter((f) => f !== id)
      : [...feats, id];
    setFilter({ features: next });
  };

  return (
    <div className="pointer-events-auto absolute bottom-16 left-1/2 w-[400px] max-w-[92vw] -translate-x-1/2 rounded-2xl border border-cream-200 bg-white/95 shadow-soft-lg backdrop-blur">
      {/* Header */}
      <div className="flex items-center justify-between border-b border-cream-100 px-3 py-2">
        <h4 className="flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-cream-500">
          <SlidersHorizontal className="h-3.5 w-3.5" /> Filters
        </h4>
        <button
          onClick={onClose}
          className="rounded-full p-1 text-cream-400 hover:bg-cream-100 hover:text-cream-700"
          aria-label="Close filters"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>

      <div className="max-h-[340px] space-y-3 overflow-y-auto p-3">
        {/* 1. Anomaly categories */}
        <fieldset>
          <legend className="mb-1.5 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
            Anomaly categories (hide effects)
          </legend>
          <div className="flex flex-wrap gap-1">
            {ANOMALY_RULE_IDS.map((ruleId) => {
              const active = filter.categories.includes(ruleId);
              const count = catCounts[ruleId] ?? 0;
              const glyph = RULE_GLYPH[ruleId] ?? "\u{1F538}";
              const label = RULE_LABEL[ruleId] ?? ruleId;
              return (
                <button
                  key={ruleId}
                  onClick={() => toggleCategory(ruleId)}
                  title={`${label}: ${count} open`}
                  className={`flex items-center gap-1 rounded-lg border px-2 py-1 text-[10px] font-medium transition-colors ${
                    active
                      ? "border-terracotta bg-terracotta text-white"
                      : "border-cream-200 bg-cream-50 text-cream-500 hover:bg-cream-100"
                  }`}
                >
                  <span className="text-[11px]">{glyph}</span>
                  <span>{label}</span>
                  {count > 0 && (
                    <span className="ml-0.5 rounded bg-white/20 px-1 text-[9px]">
                      {count}
                    </span>
                  )}
                </button>
              );
            })}
          </div>
        </fieldset>

        {/* 2. Severity floor */}
        <fieldset>
          <legend className="mb-1.5 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
            Severity floor
          </legend>
          <div className="flex gap-0.5">
            {SEVERITY_OPTIONS.map((opt) => {
              const active = filter.minSeverity === opt.key;
              return (
                <button
                  key={opt.label}
                  onClick={() => setFilter({ minSeverity: opt.key })}
                  className={`flex-1 rounded-lg px-2 py-1 text-[11px] font-medium transition-colors ${
                    active
                      ? "bg-terracotta text-white"
                      : "bg-cream-50 text-cream-500 hover:bg-cream-100"
                  }`}
                >
                  {opt.label}
                </button>
              );
            })}
          </div>
        </fieldset>

        {/* 3. Quarters (features) */}
        {features.length > 0 && (
          <fieldset>
            <legend className="mb-1.5 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
              Quarters (keep only selected)
            </legend>
            <div className="flex flex-wrap gap-1">
              {features.map(([id, label]) => {
                const active = filter.features.includes(id);
                return (
                  <button
                    key={id}
                    onClick={() => toggleFeature(id)}
                    className={`rounded-lg border px-2 py-1 text-[10px] font-medium transition-colors ${
                      active
                        ? "border-terracotta bg-terracotta text-white"
                        : "border-cream-200 bg-cream-50 text-cream-500 hover:bg-cream-100"
                    }`}
                  >
                    {label}
                  </button>
                );
              })}
            </div>
          </fieldset>
        )}

        {/* 4. Path glob */}
        <fieldset>
          <legend className="mb-1.5 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
            Path glob
          </legend>
          <input
            type="text"
            value={pathInput}
            onChange={(e) => setPathInput(e.target.value)}
            placeholder="e.g. src/components/* or polis"
            className="w-full rounded-xl border border-cream-200 bg-white px-3 py-1.5 text-[11px] text-cream-800 outline-none focus:border-terracotta-300 focus:ring-1 focus:ring-terracotta-100"
          />
        </fieldset>

        {/* 5. File types fold-in SKIP — TODO: merge standalone File Types panel here */}
        <p className="text-[10px] italic text-cream-400">
          {/* TODO: fold in the standalone File Types panel controls here */}
        </p>

        {/* 6. Footer */}
        <div className="flex items-center justify-between border-t border-cream-100 pt-2">
          <button
            onClick={resetFilterAction}
            className="rounded-lg px-2 py-1 text-[11px] font-medium text-cream-500 transition-colors hover:bg-cream-100 hover:text-cream-700"
          >
            Reset all
          </button>
          <span className="text-[10px] text-cream-400">
            {filterSets
              ? `shows ${filterSets.shownBuildings} of ${filterSets.totalBuildings} buildings, ${filterSets.shownAnomalies} of ${filterSets.totalAnomalies} anomalies`
              : "…"}
          </span>
        </div>
      </div>
    </div>
  );
}
