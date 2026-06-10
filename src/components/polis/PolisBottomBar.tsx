// PolisBottomBar — a videogame-style control bar anchored to the bottom of the
// Polis map (like a city-builder's control bar), plus the panels its boxes open.
//
// On-brand (cream / terracotta, rounded, soft shadow). It floats ABOVE the PIXI
// canvas. POINTER-EVENTS DISCIPLINE: the outer wrapper is `pointer-events-none`
// so drag/pan/zoom pass straight through to the canvas everywhere EXCEPT over the
// bar's own pixels (the bar and any open panel re-enable `pointer-events-auto`).
// This keeps the map fully interactive around the bar.
//
// The bar is trivially extensible: it renders from a `BAR_ITEMS` array of
// {id, label, icon, ...}. Today it has two REAL boxes:
//   - Guide   → opens a full HELP PANEL explaining the whole map in plain
//               language for a non-technical user (buildings, sizes, types +
//               Greek names + icons, roads, agents, districts/colors, burning
//               buildings, and the controls).
//   - Legend  → toggles a compact purpose-color legend overlay.
// Both reflect real concepts; no placeholder buttons that do nothing.

import { useState, useEffect } from "react";
import {
  X,
  HelpCircle,
  Map as MapIcon,
  Building2,
  Ruler,
  Shapes,
  Route,
  Bot,
  LandPlot,
  Flame,
  Flag,
  Trophy,
  MousePointerClick,
  Scroll,
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
  FileCode,
  Cloud,
  type LucideIcon,
} from "lucide-react";
import { OracleAskPanel } from "./OracleAskPanel";
import { purposeLabel } from "../../types/city";
import { getProfile, PALETTE, PROVIDER_LIVERY } from "./palette";
import { useCityStore } from "../../store/cityStore";

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

// The headline purposes shown in the legend + help (ordered for reading).
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

// Fallback swatch for the (registry-impossible) case of a degenerate 0 color, so
// an unknown slug can never paint a pure-black legend chip. getProfile already
// resolves unknown slugs to the `unknown` profile (a real color), so this only
// guards a hypothetical zero — it stays a cheap, honest default (PALETTE cream).
const SWATCH_FALLBACK = `#${PALETTE.cream.toString(16).padStart(6, "0")}`;

function purposeSwatch(slug: string): string {
  const profile = getProfile(slug);
  if (!profile.colorTop) return SWATCH_FALLBACK;
  return `#${profile.colorTop.toString(16).padStart(6, "0")}`;
}

// TECH LIVERY (F4): provider → pennant accent (hex), mirroring the renderer's
// PROVIDER_LIVERY map so the legend swatch matches the on-map pennant exactly.
function providerSwatch(slug: string): string {
  return `#${(PROVIDER_LIVERY[slug] ?? 0).toString(16).padStart(6, "0")}`;
}

// The provider liveries shown in the legend (label + machine slug).
const LEGEND_PROVIDERS: { slug: string; label: string }[] = [
  { slug: "cloudflare", label: "Cloudflare" },
  { slug: "scaleway", label: "Scaleway" },
];

// ---------------------------------------------------------------------------
// Bar items registry — render-from-data so future boxes are a one-line add.
// ---------------------------------------------------------------------------

type PanelId = "guide" | "legend" | "filetypes" | "oracle";

interface BarItem {
  id: PanelId;
  label: string;
  icon: LucideIcon;
  title: string;
}

const BAR_ITEMS: BarItem[] = [
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
];

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function PolisBottomBar({
  buildingCount,
  roadCount,
  agentCount,
  onFocusFile,
}: {
  buildingCount: number;
  roadCount: number;
  agentCount: number;
  /** Called when the user clicks a citation chip in the Oracle panel. */
  onFocusFile?: (fileSource: string) => void;
}) {
  // Which panel is open (a toggle-group: clicking the active item closes it).
  const [open, setOpen] = useState<PanelId | null>(null);

  const toggle = (id: PanelId) => setOpen((prev) => (prev === id ? null : id));

  const handleFocusFile = (fileSource: string) => {
    onFocusFile?.(fileSource);
  };

  return (
    // Full-area, click-through layer. Only the bar + panels capture events.
    <div className="pointer-events-none absolute inset-0 z-20">
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

      {/* The bar itself, anchored bottom-center. */}
      <div className="absolute inset-x-0 bottom-3 flex justify-center">
        <div className="pointer-events-auto flex items-center gap-1.5 rounded-2xl border border-cream-200 bg-white/95 p-1.5 shadow-soft-md backdrop-blur">
          {BAR_ITEMS.map((item) => {
            const Icon = item.icon;
            const active = open === item.id;
            return (
              <button
                key={item.id}
                onClick={() => toggle(item.id)}
                title={item.title}
                className={`flex items-center gap-2 rounded-xl px-3 py-2 text-[12px] font-medium transition-colors ${
                  active
                    ? "bg-terracotta text-white"
                    : "text-cream-600 hover:bg-cream-100 hover:text-cream-800"
                }`}
              >
                <Icon className="h-4 w-4" />
                <span>{item.label}</span>
              </button>
            );
          })}
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// File-types panel — choose which file extensions become buildings in THIS
// workspace. Reads available/enabled from the backend, lets the user toggle,
// and on Apply persists + rebuilds the city (all via the store).
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
    <div className="pointer-events-auto absolute bottom-16 left-1/2 w-[340px] max-w-[92vw] -translate-x-1/2 rounded-2xl border border-cream-200 bg-white/97 p-3 shadow-soft-lg backdrop-blur">
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
// Legend overlay — a compact purpose-color legend (real palette colors).
// ---------------------------------------------------------------------------

function LegendOverlay({ onClose }: { onClose: () => void }) {
  return (
    <div className="pointer-events-auto absolute bottom-16 left-1/2 w-[300px] max-w-[92vw] -translate-x-1/2 rounded-2xl border border-cream-200 bg-white/97 p-3 shadow-soft-lg backdrop-blur">
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
// Help panel — explains the whole map in plain language for a non-technical
// reader. Sections with icons, not a wall of text.
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
            on a busy avenue and you’ll see{" "}
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
            “urban sins”): smoke = minor, fire = real, an inferno = serious. Open
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

export default PolisBottomBar;
