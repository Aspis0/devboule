// InspectSidebar — a serious, Caesar-III-style RICH INSPECT POPUP for the Polis
// map.
//
// A right-side slide-in panel evoking late-90s city-builder info windows
// (Caesar III / Pharaoh / Zeus) via FRAMING — a titled header bar with the
// building's purpose icon, a double/ornamented border with corner accents, a
// parchment-tone (cream) background, a stat grid with small lucide icons, clear
// section dividers, and a clear action footer — but ON-BRAND with the app's
// cream / ivory / terracotta / stone Tailwind palette (no clashing skeuomorphic
// texture).
//
// It handles TWO subjects:
//   - BUILDING (a source file): purpose icon + filename + "English (Greek)"
//     purpose label + a CONFIDENCE badge (grounded → verified dot; guess → "?").
//     A LAZY, on-demand Oracle "what it does" blurb (epoch-guarded + session
//     cached + gracefully unavailable). A stat grid (LOC + tier, district, last
//     modified, repo-relative path with Copy). CONNECTIONS derived from the road
//     graph: Imports (out) + Imported by (in), counted, sorted by weight, top-6
//     each with a "+N more" expand into a contained scroll region; every entry
//     is clickable and re-selects that building. ISSUES (sins) with severity
//     tone. Agent present. Notes. An "Open in editor" footer.
//   - AGENT (citizen): the agent id + type (English (Greek)), status, current
//     task, and the building it is working on (clickable to select / open).
//
// HONESTY: only real backend fields are shown — nothing is invented. The Oracle
// call uses the real `ask_oracle` command (OracleAnswer.answer ?? summary); the
// editor buttons call the gated, path-validated `polis_open_in_editor` command.
// Both are Tauri-only and degrade to a muted message in the browser harness.

import React, { useMemo, useState, useCallback, useRef, useEffect, Suspense } from "react";
import {
  X,
  Copy,
  Check,
  FileCode,
  ArrowDownToLine,
  ArrowUpFromLine,
  HelpCircle,
  Search,
  Bot,
  Sparkles,
  ShieldCheck as VerifiedCheck,
  // Subject / stat icons
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
  Mountain,
  Share2,
  Building2,
  Home,
  Code2,
  MapPin,
  Clock,
  FolderTree,
  Network,
  ShieldCheck,
  Eye,
  Activity,
  ExternalLink,
  Flag,
  ArrowRight,
  BookOpen,
  Cloud,
  Server,
  Cpu,
  Database,
  Box,
  BrainCircuit,
  Globe,
  Trophy,
  type LucideIcon,
} from "lucide-react";
import type {
  Building,
  CityState,
  Agent,
  ExternalService,
} from "../../types/city";
import type { ResourceSite } from "./resources";
const AnomalySection = React.lazy(() => import("./AnomalySection"));
const KinSection = React.lazy(() => import("./KinSection"));
import type { OracleAnswer } from "../../types/backend";
import { purposeLabel, agentTypeLabel } from "../../types/city";
import {
  invokeBackendCommand,
  isTauriRuntime,
  useAppContext,
} from "../../context/AppContext";
import {
  browserOracleMessage,
  emptyOracleResultMessage,
  indexingUnavailableMessage,
  oracleFailureMessage,
  shouldDeferOracleAsk,
} from "./oraclePanelMessages";
import { buildDossierEvidence } from "./dossierEvidence";
import { DossierEvidenceSection } from "./dossierEvidenceView";
import { isAppleHost } from "../../lib/platform";
import { getProfile } from "./palette";
import { MONUMENT_META } from "./kitcd/monuments";

// ---------------------------------------------------------------------------
// Subject discriminated union
// ---------------------------------------------------------------------------

export type InspectSubject =
  | { kind: "building"; building: Building }
  | { kind: "agent"; agent: Agent }
  // A TRADE-ROUTE connection: a REAL import edge surfaced by clicking a merchant
  // porter (or its road) on the map. `from` is the importer (consumer), `to` is
  // the imported dependency (supplier) — the same orientation the road graph and
  // the building Connections section use. Resolved to real Buildings here.
  | { kind: "connection"; from: string; to: string }
  // An EXTERNAL SERVICE: era monument (or legacy cloud outpost from old JSON) (from the
  // synced provider inventory) surfaced by clicking its harbour/outpost node at
  // the map margin. Inspect-only (provider/type/name/status) — no secret, no
  // spawn action in this phase.
  | { kind: "externalService"; service: ExternalService }
  // A RESOURCE SITE (quarry / mine): a clickable sprite outside a district whose
  // static-asset census meets the threshold. Compact info card.
  | { kind: "resource"; site: ResourceSite }
  | null;

interface InspectSidebarProps {
  subject: InspectSubject;
  city: CityState | null;
  onClose: () => void;
  /** Navigate the popup + map to another building (connection navigation). */
  onSelectBuilding: (building: Building) => void;
}

// Purpose sources that are GUESSES rather than grounded verdicts.
const GUESS_SOURCES = new Set(["heuristic", "default"]);


// Purpose slug -> lucide icon (per the design brief).
const PURPOSE_ICON: Record<string, LucideIcon> = {
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
  townhall: Building2,
  house: Home,
};

function purposeIcon(slug: string): LucideIcon {
  return PURPOSE_ICON[slug] ?? FileCode;
}

// Purpose slug -> a small swatch color (reuse the renderer's profile palette so
// the chip dot matches the actual building body color on the map). Returns a CSS
// hex string. Falls back to the "unknown" profile.
function purposeDotColor(slug: string): string {
  const profile = getProfile(slug);
  return `#${profile.colorTop.toString(16).padStart(6, "0")}`;
}

// Agent type slug -> lucide icon.
const AGENT_ICON: Record<string, LucideIcon> = {
  orchestrator: Network,
  coder: Hammer,
  verifier: ShieldCheck,
  augur: Eye,
};

function agentIcon(slug: string): LucideIcon {
  return AGENT_ICON[slug] ?? Bot;
}

// Visual-tier slug -> humanized label.
const TIER_LABELS: Record<string, string> = {
  kalybe: "Kalybe (hut)",
  oikia: "Oikia (house)",
  synoikia: "Synoikia (tenement)",
  megaron: "Megaron (hall)",
  mnemeion: "Mnemeion (monument)",
};

function tierLabel(slug: string): string {
  return TIER_LABELS[slug] ?? titleCase(slug);
}

// District-type slug -> humanized label.
const DISTRICT_TYPE_LABELS: Record<string, string> = {
  commons: "Commons",
  feature: "Feature",
  external: "External",
};

function districtTypeLabel(slug: string): string {
  return DISTRICT_TYPE_LABELS[slug] ?? titleCase(slug);
}

function titleCase(slug: string): string {
  return slug
    .split(/[_-]/)
    .filter((w) => w.length > 0)
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join(" ");
}

function humanizeStatus(slug: string): string {
  return titleCase(slug);
}

/** Human label for a provider/external slug (monuments use "monument"). */
function providerLabel(slug: string): string {
  if (slug === "monument") return "Monument";
  return titleCase(slug);
}

// Editors offered in the action footer. Slugs match the backend allowlist
// (notepad | vscode | cursor | explorer) — platform-stable contract. Labels
// are OS-aware so macOS doesn't advertise Windows-only binaries (F68).
function isWindowsHost(): boolean {
  if (typeof navigator === "undefined") return false;
  const haystack =
    `${navigator.platform ?? ""} ${navigator.userAgent ?? ""}`.toLowerCase();
  return haystack.includes("win");
}

function nativeEditorLabels(): { notepad: string; explorer: string } {
  if (isAppleHost()) {
    return { notepad: "TextEdit", explorer: "Reveal in Finder" };
  }
  if (isWindowsHost()) {
    return { notepad: "Notepad", explorer: "Reveal in Explorer" };
  }
  return { notepad: "Text editor", explorer: "Reveal in file manager" };
}

const NATIVE_EDITOR_LABELS = nativeEditorLabels();

const EDITORS: { slug: string; label: string; icon: LucideIcon }[] = [
  { slug: "notepad", label: NATIVE_EDITOR_LABELS.notepad, icon: FileCode },
  { slug: "vscode", label: "VS Code", icon: Code2 },
  { slug: "cursor", label: "Cursor", icon: Code2 },
  { slug: "explorer", label: NATIVE_EDITOR_LABELS.explorer, icon: FolderTree },
];

// How many connections to show before collapsing the rest behind "+N more".
const CONNECTIONS_PREVIEW = 6;

// ---------------------------------------------------------------------------
// Session-scoped Oracle cache (per fileId). Survives building switches within a
// session so re-clicking a building shows its description instantly. Module
// scope (not React state) keeps it stable across renders / remounts.
// ---------------------------------------------------------------------------

type OracleState =
  | { kind: "loading" }
  | { kind: "ok"; text: string }
  // `message` is the honest reason (indexing, typed OracleError, empty, browser…).
  // Never cache pure "indexing" rows so a finished job can re-ask on next open.
  | { kind: "unavailable"; message: string; transient?: boolean };

const oracleCache = new Map<string, OracleState>();

// ---------------------------------------------------------------------------
// 4b — "More details" narrative DOSSIER. A deeper, product-level Oracle
// explanation persisted PER FILE by the backend and regenerated ONLY when the
// file's content changed. Lazy: only fetched/generated on an explicit click.
//
// Backend contract (Tauri commands, both gated + path-validated):
//   - polis_get_dossier(filePath) -> { text: string | null, stale: boolean }
//     PURE disk read (no Oracle). `text` = cached dossier; `stale` = no dossier
//     OR the file changed since it was generated.
//   - polis_generate_dossier(filePath) -> { text: string | null, available: boolean }
//     Calls the gated, retrieval-backed Oracle with a deep prompt; on success
//     persists + returns the fresh text; on ANY failure makes no write and
//     returns the cached text (if any) with available=false (fail-closed).
//
// Session cache per fileId mirrors `oracleCache` so re-opening a building shows
// its dossier instantly; an epoch guard (per building switch) prevents a slow
// in-flight generate from landing in the wrong pergamena.
// ---------------------------------------------------------------------------

interface DossierStatus {
  text: string | null;
  stale: boolean;
}

interface DossierResult {
  text: string | null;
  available: boolean;
}

type DossierState =
  // Initial: not opened yet — show the "More details" button.
  | { kind: "idle" }
  // Fetching the persisted status (fast disk read).
  | { kind: "checking" }
  // Generating via the Oracle; `cached` is any existing text shown meanwhile.
  | { kind: "generating"; cached: string | null }
  // Have text (possibly being refreshed if `refreshing`).
  | { kind: "ok"; text: string }
  // No text + generation failed/unavailable (fail-closed). Honest `message`.
  | { kind: "unavailable"; message: string; transient?: boolean };

const dossierCache = new Map<string, DossierState>();

// PURE decision: given the persisted dossier status, what should the explicit
// "More details" open do? `serveCached` -> show the cached text instantly with no
// Oracle call (fresh cached dossier). Otherwise `generate` -> kick off the Oracle
// generate, passing whatever cached text exists (shown while it runs, kept on
// failure). Exported so the lazy/stale rule is unit-testable without a DOM.
export function decideDossierOpen(status: DossierStatus): {
  action: "serveCached" | "generate";
  cached: string | null;
} {
  const cached = (status.text ?? "").trim();
  if (cached.length > 0 && !status.stale) {
    return { action: "serveCached", cached };
  }
  return { action: "generate", cached: cached.length > 0 ? cached : null };
}

// PURE decision: given the generate result + the cached text shown meanwhile, what
// final state should we land in? Fail-closed: an unavailable result keeps any
// cached text ("ok" with cached) rather than discarding it; only a genuinely empty
// result with no cache becomes "unavailable". `failureMessage` is the honest
// reason when there is nothing to show. Exported for unit tests.
export function decideDossierResult(
  res: DossierResult | null,
  cached: string | null,
  failureMessage?: string,
): DossierState {
  const text = (res?.text ?? "").trim();
  if (res?.available && text.length > 0) return { kind: "ok", text };
  if (text.length > 0) return { kind: "ok", text };
  const keep = (cached ?? "").trim();
  if (keep.length > 0) return { kind: "ok", text: keep };
  return {
    kind: "unavailable",
    message:
      failureMessage?.trim() ||
      emptyOracleResultMessage("dossier", false),
  };
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function InspectSidebar({
  subject,
  city,
  onClose,
  onSelectBuilding,
}: InspectSidebarProps) {
  const open = subject !== null;

  return (
    <div
      className={`pointer-events-none absolute inset-y-0 right-0 z-20 flex w-[372px] max-w-[92vw] transform transition-transform duration-300 ease-out ${
        open ? "translate-x-0" : "translate-x-full"
      }`}
      aria-hidden={!open}
    >
      {subject?.kind === "building" && (
        <BuildingPopup
          building={subject.building}
          city={city}
          onClose={onClose}
          onSelectBuilding={onSelectBuilding}
        />
      )}
      {subject?.kind === "agent" && (
        <AgentPopup
          agent={subject.agent}
          city={city}
          onClose={onClose}
          onSelectBuilding={onSelectBuilding}
        />
      )}
      {subject?.kind === "connection" && (
        <ConnectionPopup
          from={subject.from}
          to={subject.to}
          city={city}
          onClose={onClose}
          onSelectBuilding={onSelectBuilding}
        />
      )}
      {subject?.kind === "externalService" && (
        <ExternalServicePopup service={subject.service} onClose={onClose} />
      )}
      {subject?.kind === "resource" && (
        <ResourceCard site={subject.site} onClose={onClose} />
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Shared frame — the "city-builder info window" chrome
// ---------------------------------------------------------------------------

function PopupFrame({
  icon: Icon,
  title,
  subtitle,
  confidence,
  onClose,
  children,
  footer,
}: {
  icon: LucideIcon;
  title: string;
  subtitle: string;
  confidence?: React.ReactNode;
  onClose: () => void;
  children: React.ReactNode;
  footer?: React.ReactNode;
}) {
  return (
    <div className="pointer-events-auto relative m-3 flex h-[calc(100%-1.5rem)] w-full flex-col overflow-hidden rounded-2xl border-2 border-terracotta-300 bg-cream-50 shadow-soft-lg">
      {/* Inner ornamented border (the "double border" era cue). */}
      <div className="pointer-events-none absolute inset-1.5 z-10 rounded-xl border border-terracotta-100" />
      {/* Corner accents. */}
      <CornerAccents />

      {/* Titled header bar. */}
      <div className="relative flex items-start gap-3 border-b-2 border-terracotta-200 bg-gradient-to-b from-terracotta-50 to-cream-50 p-4">
        <div className="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-terracotta-300 bg-terracotta-100 shadow-soft-xs">
          <Icon className="h-5.5 w-5.5 text-terracotta-600" />
        </div>
        <div className="min-w-0 flex-1">
          <h3
            className="truncate text-[14px] font-semibold text-cream-800"
            title={title}
          >
            {title}
          </h3>
          <div className="mt-0.5 flex items-center gap-1.5">
            <p className="truncate text-[12px] text-cream-500" title={subtitle}>
              {subtitle}
            </p>
            {confidence}
          </div>
        </div>
        <button
          onClick={onClose}
          className="rounded-full p-1.5 text-cream-400 transition-colors hover:bg-cream-200 hover:text-cream-700"
          aria-label="Close inspector"
        >
          <X className="h-4 w-4" />
        </button>
      </div>

      {/* Scrollable body. */}
      <div className="relative flex-1 overflow-y-auto p-4 text-[13px]">
        {children}
      </div>

      {/* Action footer. */}
      {footer && (
        <div className="relative space-y-2 border-t-2 border-terracotta-200 bg-cream-100/70 p-4">
          {footer}
        </div>
      )}
    </div>
  );
}

function CornerAccents() {
  const base = "pointer-events-none absolute z-10 h-3 w-3 border-terracotta-400";
  return (
    <>
      <span className={`${base} left-2 top-2 rounded-tl border-l-2 border-t-2`} />
      <span className={`${base} right-2 top-2 rounded-tr border-r-2 border-t-2`} />
      <span
        className={`${base} bottom-2 left-2 rounded-bl border-b-2 border-l-2`}
      />
      <span
        className={`${base} bottom-2 right-2 rounded-br border-b-2 border-r-2`}
      />
    </>
  );
}

// Confidence badge for a building's purpose classification.
function ConfidenceBadge({ source }: { source: string }) {
  const isGuess = GUESS_SOURCES.has(source);
  if (isGuess) {
    return (
      <span
        title={`Classification is a guess (${source}) — not a grounded verdict.`}
        className="inline-flex shrink-0 items-center gap-0.5 rounded-full border border-cream-300 bg-cream-100 px-1.5 py-0.5 text-[10px] font-semibold text-cream-500"
      >
        <HelpCircle className="h-3 w-3" />?
      </span>
    );
  }
  return (
    <span
      title={`Verified from a grounded signal (${source}).`}
      className="inline-flex shrink-0 items-center gap-0.5 rounded-full border border-sage/30 bg-sage/10 px-1.5 py-0.5 text-[10px] font-semibold text-sage-dark"
    >
      <VerifiedCheck className="h-3 w-3" />
    </span>
  );
}

// ---------------------------------------------------------------------------
// Building popup
// ---------------------------------------------------------------------------

function BuildingPopup({
  building,
  city,
  onClose,
  onSelectBuilding,
}: {
  building: Building;
  city: CityState | null;
  onClose: () => void;
  onSelectBuilding: (b: Building) => void;
}) {
  // Imports / imported-by from the road graph (from = importer side). Each entry
  // carries the resolved Building + the road weight (for sorting).
  const { imports, importedBy } = useMemo(() => {
    if (!city)
      return {
        imports: [] as ConnEntry[],
        importedBy: [] as ConnEntry[],
      };
    const byId = new Map(city.buildings.map((b) => [b.fileId, b]));
    const imp: ConnEntry[] = [];
    const impBy: ConnEntry[] = [];
    for (const road of city.roads) {
      if (road.from === building.fileId) {
        const t = byId.get(road.to);
        if (t) imp.push({ building: t, weight: road.weight });
      } else if (road.to === building.fileId) {
        const s = byId.get(road.from);
        if (s) impBy.push({ building: s, weight: road.weight });
      }
    }
    // Sort by weight DESC, then label ASC for a stable, meaningful order.
    const sorter = (a: ConnEntry, b: ConnEntry) =>
      b.weight - a.weight || a.building.label.localeCompare(b.building.label);
    imp.sort(sorter);
    impBy.sort(sorter);
    return { imports: imp, importedBy: impBy };
  }, [building, city]);

  const district = useMemo(() => {
    if (!city) return null;
    return (
      city.districts.find((d) => d.districtId === building.districtId) ?? null
    );
  }, [building, city]);

  const presentAgent = useMemo(() => {
    if (!building.agentPresent || !city) return null;
    return city.agents.find((a) => a.agentId === building.agentPresent) ?? null;
  }, [building, city]);

  // The building's FEATURE (product/domain quarter) from the F1/F2 registry. Its
  // label/description are the Oracle's after an F2 re-classify, else the
  // deterministic F1 label. `featureSource === "oracle"` means the Oracle named it.
  const feature = useMemo(() => {
    if (!city?.features || !building.featureId) return null;
    return city.features.find((f) => f.id === building.featureId) ?? null;
  }, [city, building.featureId]);

  const Icon = purposeIcon(building.purpose);

  const districtValue = district
    ? `${district.name} · ${districtTypeLabel(district.type)}`
    : building.districtId;

  // Quarter provenance: "named by Oracle" when the feature was Oracle-classified
  // (F2), else "by structure" (the deterministic F1 assignment).
  const namedByOracle = building.featureSource === "oracle";

  return (
    <PopupFrame
      icon={Icon}
      title={building.label}
      subtitle={purposeLabel(building.purpose)}
      confidence={<ConfidenceBadge source={building.purposeSource} />}
      onClose={onClose}
      footer={<EditorActions filePath={building.filePath} />}
    >
      {/* WHAT IT DOES — lazy Oracle blurb. */}
      <OracleBlurb building={building} />

      {/* QUARTER — the building's feature/product area + provenance (F1/F2). */}
      {feature && (
        <section className="mt-3 rounded-xl border border-cream-200 bg-white px-3 py-2.5">
          <div className="flex items-center gap-2">
            <span
              className="h-3 w-3 shrink-0 rounded-sm ring-1 ring-black/5"
              style={{ backgroundColor: feature.colorAccent }}
            />
            <div className="min-w-0 flex-1">
              <p className="truncate text-[13px] font-medium text-cream-700">
                Quarter: {feature.label}
              </p>
              <p className="text-[11px] text-cream-400">
                {namedByOracle ? "named by Oracle" : "by structure"}
              </p>
            </div>
            {namedByOracle && (
              <span
                title="This quarter was named by the Oracle (re-classify)."
                className="inline-flex shrink-0 items-center gap-0.5 rounded-full border border-terracotta-200 bg-terracotta-50 px-1.5 py-0.5 text-[10px] font-semibold text-terracotta-500"
              >
                <Sparkles className="h-3 w-3" />
              </span>
            )}
          </div>
          {feature.description.trim().length > 0 && (
            <p className="mt-1.5 text-[12px] leading-5 text-cream-600">
              {feature.description}
            </p>
          )}
        </section>
      )}

      {/* Stat grid */}
      <div className="mt-1 grid grid-cols-1 gap-1.5">
        <Stat
          icon={Code2}
          label="Lines of code"
          value={`${building.linesOfCode.toLocaleString()} · ${tierLabel(
            building.visualTier,
          )}`}
        />
        <Stat icon={MapPin} label="District" value={districtValue} />
        {building.provider && (
          <Stat
            icon={Flag}
            label="Provider"
            value={providerLabel(building.provider)}
          />
        )}
        {building.lastModified && (
          <Stat
            icon={Clock}
            label="Last modified"
            value={formatRelative(building.lastModified)}
          />
        )}
      </div>

      {/* File path (monospace) with a Copy-path button */}
      <PathRow filePath={building.filePath} />

      {/* CONNECTIONS — import graph. Counts in the section header; top-6 each;
          "+N more" expands into a contained scroll region; entries navigate. */}
      <section className="mt-4">
        <h4 className="mb-2 flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-cream-400">
          <Share2 className="h-3.5 w-3.5" /> Connections
          <span className="ml-auto font-normal normal-case tracking-normal text-cream-400">
            Imports {imports.length} · Imported by {importedBy.length}
          </span>
        </h4>
        <ConnGroup
          icon={<ArrowDownToLine className="h-3.5 w-3.5" />}
          title="Imports (out)"
          entries={imports}
          onSelect={onSelectBuilding}
        />
        <div className="mt-2">
          <ConnGroup
            icon={<ArrowUpFromLine className="h-3.5 w-3.5" />}
            title="Imported by (in)"
            entries={importedBy}
            onSelect={onSelectBuilding}
          />
        </div>
      </section>


      {/* KIN BUILDINGS (P6.3 — semantic similarity, lazy-loaded) */}
      <Suspense fallback={null}>
        <KinSection
          building={building}
          city={city}
          onSelectBuilding={onSelectBuilding}
        />
      </Suspense>
      {/* UNDER INVESTIGATION (bug-investigation P3) — Oracle SUSPECTS this file for
          an open bug card. HONEST framing: a suspect is a GUESS, not a confirmed
          problem, so this sits in its OWN section, ABOVE and visually separate from
          the confirmed "Issues" (sins) list, with a distinct indigo tone and "may
          be" wording — it must never read as a confirmed disaster. */}
      {building.suspectOfCardId && (
        <section className="mt-4">
          <SectionTitle icon={<Search className="h-3.5 w-3.5" />}>
            Under investigation
          </SectionTitle>
          <div className="rounded-xl border border-teal/30 bg-teal/10 px-3 py-2">
            {/* The id is project-qualified ("<project>/<card>", see
                gather_open_bug_suspects) so it is unambiguous across projects. */}
            <p className="text-[13px] font-medium text-teal-dark">
              Oracle suspect for bug card{" "}
              <span className="font-mono">{building.suspectOfCardId}</span>
            </p>
            <p className="mt-0.5 text-[12px] italic text-cream-600">
              A guess from Oracle localization — this file may be involved in the
              bug, not a confirmed issue.
            </p>
          </div>
        </section>
      )}

      {/* ISSUES (augure sin ledger — P1.4, lazy-loaded) */}
      <Suspense fallback={null}>
        <AnomalySection
          buildingFilePath={building.filePath}
          buildingSins={building.sins}
        />
      </Suspense>

      {/* Agent present */}
      {presentAgent && (
        <section className="mt-4">
          <SectionTitle icon={<Bot className="h-3.5 w-3.5" />}>
            Agent present
          </SectionTitle>
          <div className="rounded-xl border border-cream-200 bg-white px-3 py-2">
            <p className="text-[13px] font-medium text-cream-700">
              An agent is working here
            </p>
            <p className="mt-0.5 text-[12px] text-cream-500">
              {agentTypeLabel(presentAgent.type)} · {presentAgent.agentId}
            </p>
            {presentAgent.currentTask && (
              <p className="mt-0.5 text-[12px] italic text-cream-500">
                “{presentAgent.currentTask}”
              </p>
            )}
          </div>
        </section>
      )}

      {/* Notes */}
      {building.notes.length > 0 && (
        <section className="mt-4">
          <SectionTitle icon={<FileCode className="h-3.5 w-3.5" />}>
            Notes
          </SectionTitle>
          <ul className="space-y-1">
            {building.notes.map((n, i) => (
              <li
                key={i}
                className="rounded-lg bg-cream-100 px-2.5 py-1.5 text-[12px] text-cream-500"
              >
                {n}
              </li>
            ))}
          </ul>
        </section>
      )}

      {/* MORE DETAILS — narrative dossier (Oracle) + client-side evidence. */}
      <MoreDetails building={building} city={city} />
    </PopupFrame>
  );
}

// ---------------------------------------------------------------------------
// WHAT IT DOES — lazy, on-demand Oracle blurb.
//
// On building select, asks the real `ask_oracle` command "<path>: what does this
// file do?". Shows a spinner while awaited; renders OracleAnswer.answer ?? summary
// clamped to a few lines with a "more" expand. Graceful: any failure / empty /
// non-Tauri shows a muted "unavailable" line and never blocks the rest of the
// panel. An epoch guard prevents a stale answer from a previous building landing
// in this render; results are cached per fileId for the session.
// ---------------------------------------------------------------------------

function OracleBlurb({ building }: { building: Building }) {
  const fileId = building.fileId;
  // Index status is already polled app-wide — use it to say "indexing" and to
  // skip a doomed ask_oracle while the embedder is contended.
  const { oracleIndexStatus } = useAppContext();
  const indexStatusRef = useRef(oracleIndexStatus);
  indexStatusRef.current = oracleIndexStatus;
  // Boolean only in the effect deps so progress-tick object identity does not
  // cancel an in-flight ask; transitioning true→false re-runs to fetch.
  const isIndexing = shouldDeferOracleAsk(oracleIndexStatus);
  const [state, setState] = useState<OracleState>(
    () => oracleCache.get(fileId) ?? { kind: "loading" },
  );
  const [expanded, setExpanded] = useState(false);
  // Monotonic epoch: only the latest request may write state. Bumped on every
  // fileId change and on unmount, so a slow in-flight call can never setState
  // after the building switched or the component left.
  const epochRef = useRef(0);

  useEffect(() => {
    const epoch = ++epochRef.current;
    setExpanded(false);

    const cached = oracleCache.get(fileId);
    // Skip transient "indexing" cache hits so a finished job can re-ask.
    if (
      cached &&
      cached.kind !== "loading" &&
      !(cached.kind === "unavailable" && cached.transient)
    ) {
      setState(cached);
      return;
    }

    // Browser harness (or no Tauri): skip the call, show muted message.
    if (!isTauriRuntime()) {
      const next: OracleState = {
        kind: "unavailable",
        message: browserOracleMessage("blurb"),
      };
      oracleCache.set(fileId, next);
      setState(next);
      return;
    }

    const statusNow = indexStatusRef.current;
    // While an index job is queued/running, say so instead of waiting ~90s for
    // a ServerUnavailable timeout. Do not cache: status will change.
    if (shouldDeferOracleAsk(statusNow)) {
      const next: OracleState = {
        kind: "unavailable",
        message: indexingUnavailableMessage(statusNow, "blurb"),
        transient: true,
      };
      setState(next);
      return;
    }

    setState({ kind: "loading" });
    let cancelled = false;

    void (async () => {
      try {
        const ans = await invokeBackendCommand<OracleAnswer>("ask_oracle", {
          query: `${building.filePath}: what does this file do?`,
          limit: 4,
        });
        if (cancelled || epoch !== epochRef.current) return;
        const text = (ans?.answer ?? ans?.summary ?? "").trim();
        const next: OracleState =
          text.length > 0 && !ans?.notFound
            ? { kind: "ok", text }
            : {
                kind: "unavailable",
                message: emptyOracleResultMessage("blurb", !!ans?.notFound),
              };
        oracleCache.set(fileId, next);
        setState(next);
      } catch (e) {
        if (cancelled || epoch !== epochRef.current) return;
        // Re-check indexing on failure: a job may have started mid-flight, or
        // the timeout was embedder contention we already know about.
        const status = indexStatusRef.current;
        const indexing = shouldDeferOracleAsk(status);
        const next: OracleState = {
          kind: "unavailable",
          message: oracleFailureMessage(e, "blurb", {
            indexing,
            indexStatus: status,
          }),
          transient: indexing,
        };
        if (!indexing) oracleCache.set(fileId, next);
        setState(next);
      }
    })();

    return () => {
      cancelled = true;
      // Bump the epoch so any still-pending promise is ignored on resolve.
      epochRef.current++;
    };
  }, [fileId, building.filePath, isIndexing]);

  const CLAMP = 220;

  return (
    <section className="rounded-xl border border-terracotta-200 bg-white px-3 py-2.5">
      <h4 className="mb-1.5 flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-terracotta-500">
        <Sparkles className="h-3.5 w-3.5" /> What it does
      </h4>
      {state.kind === "loading" && (
        <div className="flex items-center gap-2 text-[12px] text-cream-400">
          <span className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-cream-300 border-t-terracotta" />
          Analyzing…
        </div>
      )}
      {state.kind === "unavailable" && (
        <p className="text-[12px] italic text-cream-400">{state.message}</p>
      )}
      {state.kind === "ok" && (
        <div>
          <p className="whitespace-pre-line text-[12.5px] leading-5 text-cream-600">
            {expanded || state.text.length <= CLAMP
              ? state.text
              : `${state.text.slice(0, CLAMP).trimEnd()}…`}
          </p>
          {state.text.length > CLAMP && (
            <button
              onClick={() => setExpanded((v) => !v)}
              className="mt-1 text-[11px] font-semibold text-terracotta-500 hover:text-terracotta-600"
            >
              {expanded ? "Show less" : "More"}
            </button>
          )}
        </div>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------
// MORE DETAILS — the dossier is TWO genuinely different halves:
//
//   1. NARRATIVE — persisted Oracle prose (4b), lazy on explicit click. Same
//      get/generate/fail-closed/session-cache/epoch path as before. Prompt and
//      secret-redaction behaviour are unchanged.
//   2. EVIDENCE — pure client extraction from the loaded city (roads, sins,
//      purpose/tier/district). Needs no Oracle call, so it still paints when
//      the narrative is unavailable/indexing — the panel is never empty prose
//      with no facts underneath.
// ---------------------------------------------------------------------------

function MoreDetails({
  building,
  city,
}: {
  building: Building;
  city: CityState | null;
}) {
  const fileId = building.fileId;
  const filePath = building.filePath;
  const { oracleIndexStatus } = useAppContext();
  const indexStatusRef = useRef(oracleIndexStatus);
  indexStatusRef.current = oracleIndexStatus;
  const [state, setState] = useState<DossierState>(
    () => dossierCache.get(fileId) ?? { kind: "idle" },
  );
  // Evidence is free: recompute from city + building whenever either changes.
  const evidence = useMemo(
    () => buildDossierEvidence(building, city),
    [building, city],
  );
  // Monotonic epoch: only the latest interaction may write state. Bumped on
  // every building switch and on unmount so a slow in-flight generate can never
  // setState into the wrong (or unmounted) pergamena.
  const epochRef = useRef(0);
  // Mirror of the latest `state`, read SYNCHRONOUSLY in `onOpen` (FIX 4: an
  // idempotency guard that can't see a stale closure) without making the
  // `onOpen` callback depend on `state` (which would re-create it every render).
  const stateRef = useRef(state);
  stateRef.current = state;

  // Persist a state transition, guarded by the captured epoch so a stale async
  // resolution is dropped. Only TERMINAL states (ok / unavailable / idle) are
  // written to the session cache — never the transient `checking`/`generating`
  // states (or indexing-unavailable). Otherwise switching away mid-generate and
  // back would restore a spinner with no in-flight promise behind it.
  const commit = useCallback(
    (epoch: number, next: DossierState) => {
      if (epoch !== epochRef.current) return;
      if (
        next.kind === "ok" ||
        next.kind === "idle" ||
        (next.kind === "unavailable" && !next.transient)
      ) {
        dossierCache.set(fileId, next);
      }
      setState(next);
    },
    [fileId],
  );

  // Run a background generate (after an explicit open). `cached` is shown while
  // it runs. Fail-closed: keep cached text on `available=false`.
  const runGenerate = useCallback(
    async (epoch: number, cached: string | null) => {
      const statusNow = indexStatusRef.current;
      // Prefer truth over a doomed generate while the embedder is contended.
      if (shouldDeferOracleAsk(statusNow)) {
        if (cached && cached.trim().length > 0) {
          // Keep any cached prose; surface indexing as a soft note via generating
          // is wrong — show ok + user can re-open later. Prefer unavailable only
          // when there is nothing else to read.
          commit(epoch, { kind: "ok", text: cached });
          return;
        }
        commit(epoch, {
          kind: "unavailable",
          message: indexingUnavailableMessage(statusNow, "dossier"),
          transient: true,
        });
        return;
      }

      commit(epoch, { kind: "generating", cached });
      try {
        const res = await invokeBackendCommand<DossierResult>(
          "polis_generate_dossier",
          { filePath },
        );
        commit(epoch, decideDossierResult(res, cached));
      } catch (e) {
        // Fail-closed: keep any cached text, else honest typed reason.
        const status = indexStatusRef.current;
        const indexing = shouldDeferOracleAsk(status);
        const msg = oracleFailureMessage(e, "dossier", {
          indexing,
          indexStatus: status,
        });
        const decided = decideDossierResult(null, cached, msg);
        if (decided.kind === "unavailable" && indexing) {
          commit(epoch, { ...decided, transient: true });
        } else {
          commit(epoch, decided);
        }
      }
    },
    [commit, filePath],
  );

  // Core open/refresh: ask the backend for the persisted dossier status (a fast,
  // pure disk read — never the Oracle), then serve cached text instantly when
  // fresh, or generate when stale/absent (showing any cached text meanwhile).
  //
  // `showCheckingSpinner` distinguishes the two callers:
  //   - EXPLICIT open from idle/unavailable (true): show a "Loading…" spinner
  //     while the status read is in flight (there is nothing else to show).
  //   - BACKGROUND re-check after restoring a cached `ok` on building re-open
  //     (false): the cached prose is ALREADY on screen, so we stay silent unless
  //     the file changed (FIX 3) — only then do we transition to a refresh.
  // Always epoch-guarded; the backend read is the only awaited step before the
  // first guard, so a building switch mid-read drops the result (no stale write,
  // no setState-after-unmount).
  const checkAndServe = useCallback(
    async (epoch: number, showCheckingSpinner: boolean) => {
      // Browser harness: no backend — honest muted message (explicit open only;
      // a background re-check simply leaves the cached text untouched).
      if (!isTauriRuntime()) {
        if (showCheckingSpinner) {
          commit(epoch, {
            kind: "unavailable",
            message: browserOracleMessage("dossier"),
          });
        }
        return;
      }

      if (showCheckingSpinner) commit(epoch, { kind: "checking" });
      try {
        const status = await invokeBackendCommand<DossierStatus>(
          "polis_get_dossier",
          { filePath },
        );
        if (epoch !== epochRef.current) return;
        const decision = decideDossierOpen(status ?? { text: null, stale: true });
        if (decision.action === "serveCached" && decision.cached) {
          // Fresh cached dossier — show instantly, no Oracle call. (For the
          // background path this re-affirms the already-shown text.)
          commit(epoch, { kind: "ok", text: decision.cached });
          return;
        }
        // Stale or absent -> generate (showing cached text + "Updating…" hint).
        void runGenerate(epoch, decision.cached);
      } catch (e) {
        // A failed status read on an explicit open is honest-unavailable; on a
        // silent background re-check we keep whatever text is already shown.
        if (showCheckingSpinner) {
          commit(epoch, {
            kind: "unavailable",
            message: oracleFailureMessage(e, "dossier"),
          });
        }
      }
    },
    [commit, filePath, runGenerate],
  );

  // The explicit "More details" / "Try again" click.
  // FIX 4: idempotent — a second rapid click while a check/generate is already in
  // flight (or text is shown) is a no-op. The guard reads `stateRef` SYNCHRONOUSLY
  // (no await before it), so two clicks in the same frame can't both pass: only
  // `idle`/`unavailable` may start a new request.
  const onOpen = useCallback(() => {
    const current = stateRef.current.kind;
    if (current !== "idle" && current !== "unavailable") return;
    void checkAndServe(epochRef.current, true);
  }, [checkAndServe]);

  // On building switch / mount: restore this file's cached dossier state (or
  // idle), invalidate any in-flight request from the previous building, and —
  // FIX 3 — if we restored a terminal `ok` from the SESSION cache, run a silent
  // background staleness re-check. The cached prose shows instantly; if the file
  // was edited after that text was generated, `polis_get_dossier` reports
  // `stale=true` and we transition to a refresh (cached text + "Updating…" ->
  // fresh text), exactly like the explicit-open stale path. Without this a file
  // edited after a successful generate would show the OLD dossier until restart.
  useEffect(() => {
    epochRef.current++;
    const epoch = epochRef.current;
    const restored = dossierCache.get(fileId) ?? { kind: "idle" };
    setState(restored);
    if (restored.kind === "ok") {
      // Silent re-check (no spinner — the cached text is already visible).
      void checkAndServe(epoch, false);
    }
    return () => {
      // Invalidate in-flight work when leaving this building / unmounting.
      epochRef.current++;
    };
  }, [fileId, checkAndServe]);

  // Evidence rides with every open state (checking / generating / ok /
  // unavailable) so an indexing Oracle never leaves the dossier body empty.
  const showEvidence = state.kind !== "idle";

  return (
    <section className="mt-4">
      <SectionTitle icon={<BookOpen className="h-3.5 w-3.5" />}>
        More details
      </SectionTitle>

      {state.kind === "idle" && (
        <button
          onClick={() => void onOpen()}
          className="flex w-full items-center justify-center gap-2 rounded-xl border border-terracotta-300 bg-white px-3 py-2.5 text-[12.5px] font-medium text-terracotta-600 transition-colors hover:bg-terracotta-50"
        >
          <BookOpen className="h-4 w-4" />
          Read the full dossier
        </button>
      )}

      {state.kind === "checking" && (
        <div className="rounded-xl border border-cream-200 bg-white px-3 py-2.5">
          <div className="flex items-center gap-2 text-[12px] text-cream-400">
            <span className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-cream-300 border-t-terracotta" />
            Loading…
          </div>
          {showEvidence && <DossierEvidenceSection evidence={evidence} />}
        </div>
      )}

      {state.kind === "generating" && (
        <div className="rounded-xl border border-terracotta-200 bg-cream-50 px-3 py-2.5">
          {state.cached && state.cached.length > 0 ? (
            <>
              <DossierProse text={state.cached} />
              <p className="mt-2 flex items-center gap-1.5 text-[11px] italic text-cream-400">
                <span className="h-3 w-3 animate-spin rounded-full border-2 border-cream-300 border-t-terracotta" />
                Updating…
              </p>
            </>
          ) : (
            <div className="flex items-center gap-2 text-[12px] text-cream-400">
              <span className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-cream-300 border-t-terracotta" />
              Consulting the Oracle…
            </div>
          )}
          {showEvidence && <DossierEvidenceSection evidence={evidence} />}
        </div>
      )}

      {state.kind === "ok" && (
        <div className="rounded-xl border border-terracotta-200 bg-cream-50 px-3 py-2.5">
          <DossierProse text={state.text} />
          {showEvidence && <DossierEvidenceSection evidence={evidence} />}
        </div>
      )}

      {state.kind === "unavailable" && (
        <div className="rounded-xl border border-cream-200 bg-white px-3 py-2.5">
          <p className="text-[12px] italic text-cream-400">{state.message}</p>
          <button
            onClick={() => void onOpen()}
            className="mt-1.5 text-[11px] font-semibold text-terracotta-500 hover:text-terracotta-600"
          >
            Try again
          </button>
          {showEvidence && <DossierEvidenceSection evidence={evidence} />}
        </div>
      )}
    </section>
  );
}

// The narrative dossier prose — a scrollable, on-brand reading section. Long
// dossiers scroll inside a bounded max-height so the panel never blows out.
function DossierProse({ text }: { text: string }) {
  return (
    <div className="max-h-64 overflow-y-auto pr-1">
      <p className="whitespace-pre-line text-[12.5px] leading-[1.55] text-cream-700">
        {text}
      </p>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Agent popup
// ---------------------------------------------------------------------------

function AgentPopup({
  agent,
  city,
  onClose,
  onSelectBuilding,
}: {
  agent: Agent;
  city: CityState | null;
  onClose: () => void;
  onSelectBuilding: (b: Building) => void;
}) {
  const workingOn = useMemo(() => {
    if (!agent.currentFileId || !city) return null;
    return city.buildings.find((b) => b.fileId === agent.currentFileId) ?? null;
  }, [agent, city]);

  const Icon = agentIcon(agent.type);

  return (
    <PopupFrame
      icon={Icon}
      title={agent.agentId}
      subtitle={agentTypeLabel(agent.type)}
      onClose={onClose}
      footer={
        workingOn ? <EditorActions filePath={workingOn.filePath} /> : undefined
      }
    >
      <div className="grid grid-cols-1 gap-1.5">
        <Stat
          icon={Activity}
          label="Status"
          value={humanizeStatus(agent.status)}
        />
        {agent.currentTask && (
          <Stat icon={Hammer} label="Current task" value={agent.currentTask} />
        )}
        {agent.lastIntervention && (
          <Stat
            icon={Clock}
            label="Last action"
            value={formatRelative(agent.lastIntervention)}
          />
        )}
      </div>

      {/* Working on -> the building / file */}
      <section className="mt-4">
        <SectionTitle icon={<MapPin className="h-3.5 w-3.5" />}>
          Working on
        </SectionTitle>
        {workingOn ? (
          <button
            onClick={() => onSelectBuilding(workingOn)}
            className="flex w-full items-center gap-2 rounded-xl border border-cream-200 bg-white px-3 py-2 text-left transition-colors hover:border-terracotta-300 hover:bg-terracotta-50"
          >
            <span
              className="h-2.5 w-2.5 shrink-0 rounded-full ring-1 ring-black/5"
              style={{ backgroundColor: purposeDotColor(workingOn.purpose) }}
            />
            <div className="min-w-0 flex-1">
              <p className="truncate text-[13px] font-medium text-cream-700">
                {workingOn.label}
              </p>
              <p className="truncate font-mono text-[11px] text-cream-400">
                {workingOn.filePath}
              </p>
            </div>
            <ExternalLink className="h-3.5 w-3.5 shrink-0 text-cream-400" />
          </button>
        ) : (
          <p className="rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] text-cream-500">
            Off map — no resolved file for this agent.
          </p>
        )}
      </section>
    </PopupFrame>
  );
}

// ---------------------------------------------------------------------------
// Connection popup — a TRADE ROUTE (real import edge) surfaced from a porter.
//
// Shows the REAL relationship "<from file> imports <to file>" tied to the two
// buildings' real `filePath`s (never fabricated). Both ends are clickable and
// re-select that building on the map (reusing the existing connection-chip
// navigation). The footer offers "Open in editor" for the importer file. If
// either endpoint isn't in the current city (a stale click during a live diff)
// we degrade to an honest "no longer on the map" message rather than invent one.
// ---------------------------------------------------------------------------

function ConnectionPopup({
  from,
  to,
  city,
  onClose,
  onSelectBuilding,
}: {
  from: string;
  to: string;
  city: CityState | null;
  onClose: () => void;
  onSelectBuilding: (b: Building) => void;
}) {
  const { consumer, supplier, weight, provenance, consumerDistrict, supplierDistrict } = useMemo(() => {
    if (!city)
      return {
        consumer: null as Building | null,
        supplier: null as Building | null,
        weight: 0,
        provenance: null as "ast" | "regex" | "semantic" | null,
        consumerDistrict: null as string | null,
        supplierDistrict: null as string | null,
      };
    const byId = new Map(city.buildings.map((b) => [b.fileId, b]));
    const districtById = new Map(city.districts.map((d) => [d.districtId, d]));
    // The road's weight + provenance (for the "imports" strength), matched by endpoints.
    let w = 0;
    let prov: "ast" | "regex" | "semantic" | null = null;
    for (const r of city.roads) {
      if (r.from === from && r.to === to) {
        w = r.weight;
        prov = r.provenance ?? null;
        break;
      }
    }
    const c = byId.get(from) ?? null;
    const s = byId.get(to) ?? null;
    return {
      consumer: c,
      supplier: s,
      weight: w,
      provenance: prov,
      consumerDistrict: c ? (districtById.get(c.districtId)?.name ?? null) : null,
      supplierDistrict: s ? (districtById.get(s.districtId)?.name ?? null) : null,
    };
    // Depend on the buildings/roads arrays specifically — NOT the whole `city`
    // reference, which changes on every live agent/sin/status diff and would
    // needlessly rebuild the byId Map while the popup is open. The memo only
    // reads `city.buildings` (endpoint resolution) and `city.roads` (weight).
  }, [city?.buildings, city?.roads, city?.districts, from, to]);

  return (
    <PopupFrame
      icon={Share2}
      title="Trade route"
      subtitle="Import dependency"
      onClose={onClose}
      footer={
        consumer ? <EditorActions filePath={consumer.filePath} /> : undefined
      }
    >
      {consumer && supplier ? (
        <>
          <section className="rounded-xl border border-terracotta-200 bg-white px-3 py-2.5">
            <h4 className="mb-2 flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-terracotta-500">
              <ArrowRight className="h-3.5 w-3.5" /> Relationship
            </h4>
            <p className="text-[12.5px] leading-5 text-cream-600">
              Connects{' '}<span className="font-semibold text-cream-800">{consumer.label}</span>
              {consumerDistrict && (
                <> (<span className="text-[11px] text-cream-400">district {consumerDistrict}</span>)</>
              )}{' to '}<span className="font-semibold text-cream-800">{supplier.label}</span>
              {supplierDistrict && (
                <> (<span className="text-[11px] text-cream-400">district {supplierDistrict}</span>)</>
              )}
            </p>
            {weight > 0 && (
              <p className="mt-1 text-[11px] text-cream-400">
                Route weight {weight} — porters per road scale with this.
              </p>
            )}
            {provenance && (
              <p className="mt-0.5 text-[11px] text-cream-400">
                Provenance: {provenance}
              </p>
            )}
          </section>

          {/* Both ends — clickable to select that building (real file paths). */}
          <section className="mt-3">
            <SectionTitle icon={<ArrowDownToLine className="h-3.5 w-3.5" />}>
              Importer (consumer)
            </SectionTitle>
            <ConnTarget building={consumer} onSelect={onSelectBuilding} />
          </section>
          <section className="mt-3">
            <SectionTitle icon={<ArrowUpFromLine className="h-3.5 w-3.5" />}>
              Imported (supplier)
            </SectionTitle>
            <ConnTarget building={supplier} onSelect={onSelectBuilding} />
          </section>
        </>
      ) : (
        <p className="rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] text-cream-500">
          One end of this trade route is no longer on the map.
        </p>
      )}
    </PopupFrame>
  );
}

// A full-width clickable building target (path + label) reused by the
// connection popup. Navigates to that building, exactly like the AgentPopup's
// "Working on" target and the connection chips.
function ConnTarget({
  building,
  onSelect,
}: {
  building: Building;
  onSelect: (b: Building) => void;
}) {
  return (
    <button
      onClick={() => onSelect(building)}
      className="flex w-full items-center gap-2 rounded-xl border border-cream-200 bg-white px-3 py-2 text-left transition-colors hover:border-terracotta-300 hover:bg-terracotta-50"
    >
      <span
        className="h-2.5 w-2.5 shrink-0 rounded-full ring-1 ring-black/5"
        style={{ backgroundColor: purposeDotColor(building.purpose) }}
      />
      <div className="min-w-0 flex-1">
        <p className="truncate text-[13px] font-medium text-cream-700">
          {building.label}
        </p>
        <p className="truncate font-mono text-[11px] text-cream-400">
          {building.filePath}
        </p>
      </div>
      <ExternalLink className="h-3.5 w-3.5 shrink-0 text-cream-400" />
    </button>
  );
}

// ---------------------------------------------------------------------------
// External service popup — era monument (or legacy outpost) surfaced
// from its harbour/outpost node at the map margin.
//
// HONESTY: every field comes straight from `city.externalServices`, which the
// backend populates ONLY from the already-synced provider inventory. There is no
// secret/endpoint on the wire (the mapper copies safe display fields only), so
// there is nothing here to leak. Inspect-only: no spawn/stop action in this phase
// (`spawnable` is shown for transparency but no action button is offered yet).
// ---------------------------------------------------------------------------

// External-service TYPE slug -> human label.
const EXTERNAL_TYPE_LABELS: Record<string, string> = {
  container: "Container",
  gpu_vm: "GPU VM",
  cpu_vm: "CPU VM",
  object_store: "Object store",
  llm_api: "LLM API",
  worker: "Worker",
};

function externalTypeLabel(slug: string): string {
  return EXTERNAL_TYPE_LABELS[slug] ?? titleCase(slug);
}

// External-service TYPE slug -> lucide icon.
const EXTERNAL_TYPE_ICON: Record<string, LucideIcon> = {
  container: Box,
  gpu_vm: Cpu,
  cpu_vm: Server,
  object_store: Database,
  llm_api: BrainCircuit,
  worker: Globe,
};

function externalTypeIcon(slug: string): LucideIcon {
  return EXTERNAL_TYPE_ICON[slug] ?? Cloud;
}

// External-service STATUS slug -> tone classes + label. Mirrors the SIN_TONE
// approach (on-brand cream/sage/amber/coral), so a "running" reads calm-positive,
// "error" alarm, "stopped" muted, "spawning" in-progress.
const EXTERNAL_STATUS_TONE: Record<string, string> = {
  running: "text-sage-dark bg-sage/10 border-sage/30",
  spawning: "text-amber-dark bg-amber/10 border-amber/30",
  stopped: "text-cream-500 bg-cream-100 border-cream-300",
  error: "text-coral-dark bg-coral/10 border-coral/40",
};

function externalStatusTone(slug: string): string {
  return EXTERNAL_STATUS_TONE[slug] ?? EXTERNAL_STATUS_TONE.stopped;
}

function ExternalServicePopup({
  service,
  onClose,
}: {
  service: ExternalService;
  onClose: () => void;
}) {
  // ERA MONUMENT: one of the 12 Claude-Design "Meraviglie" (wonders) — a prestige
  // marker derived from the REAL closing-era stats (file count + active disasters
  // at era close), NOT a cloud resource. It carries no status lamp / spawnable
  // semantics, so it gets its own honest card. The full `service.name` already
  // reads e.g. "Era Alpha: 12 files, 0 disasters active"; `service.type` carries
  // the wonder slug, so the subtitle names which wonder marks this era.
  if (service.provider === "monument") {
    const wonder = MONUMENT_META.info[service.type];
    const subtitle = wonder
      ? `Era monument · ${wonder.name}`
      : "Era monument";
    return (
      <PopupFrame
        icon={Trophy}
        title={service.name}
        subtitle={subtitle}
        onClose={onClose}
      >
        <section className="rounded-xl border border-terracotta-200 bg-white px-3 py-2.5">
          <h4 className="mb-1.5 flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-terracotta-500">
            <Trophy className="h-3.5 w-3.5" /> Prestige
          </h4>
          <p className="text-[12px] leading-5 text-cream-600">
            A monument to a closing era of this city. Its inscription is real:
            the file count and the disasters still burning when the era ended,
            captured from the archived city. The full city was saved to a
            snapshot on disk when this era began.
          </p>
        </section>
        <p className="mt-3 text-[11px] italic leading-5 text-cream-400">
          Monuments are cumulative — each new era adds one along the landward
          margin, recording what the city was.
        </p>
      </PopupFrame>
    );
  }

  // Legacy non-monument external service (pre-removal cloud outpost JSON).
  // Not rendered on the map; inspect only if somehow selected.
  const Icon = externalTypeIcon(service.type);
  const tone = externalStatusTone(service.status);

  return (
    <PopupFrame
      icon={Icon}
      title={service.name}
      subtitle={`${providerLabel(service.provider)} · ${externalTypeLabel(
        service.type,
      )}`}
      onClose={onClose}
    >
      <section className="rounded-xl border border-terracotta-200 bg-white px-3 py-2.5">
        <h4 className="mb-1.5 flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-terracotta-500">
          Legacy external service
        </h4>
        <p className="text-[12px] leading-5 text-cream-600">
          This entry comes from an older city snapshot. Cloud-provider outposts
          are no longer managed in Devboule; only era monuments are active.
        </p>
      </section>

      {/* Status — a clear tone badge mirroring the map’s status lamp. */}
      <div className="mt-3">
        <span
          className={`inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-[12px] font-semibold ${tone}`}
        >
          <Activity className="h-3.5 w-3.5" />
          {humanizeStatus(service.status)}
        </span>
      </div>

      {/* Stat grid — provider, type, status, spawnable (all safe display data). */}
      <div className="mt-3 grid grid-cols-1 gap-1.5">
        <Stat
          icon={Flag}
          label="Provider"
          value={providerLabel(service.provider)}
        />
        <Stat icon={Icon} label="Type" value={externalTypeLabel(service.type)} />
        <Stat
          icon={Activity}
          label="Status"
          value={humanizeStatus(service.status)}
        />
      </div>

      <p className="mt-3 text-[11px] italic leading-5 text-cream-400">
        Inspect-only. Spawning and stopping cloud resources from the map is not
        wired yet.
      </p>
    </PopupFrame>
  );
}

// ---------------------------------------------------------------------------
// Resource site card — compact info for a quarry / mine.
// ---------------------------------------------------------------------------

function ResourceCard({
  site,
  onClose,
}: {
  site: ResourceSite;
  onClose: () => void;
}) {
  const total = site.census.images + site.census.fonts + site.census.media;
  const isQuarry = site.kind === "quarry";
  const Icon = isQuarry ? Hammer : Mountain;
  const title = isQuarry ? `Quarry of ${site.districtLabel}` : `Mine of ${site.districtLabel}`;

  // Build the census breakdown list (omit zero groups).
  const groups: string[] = [];
  if (site.census.images > 0) groups.push(`${site.census.images} images`);
  if (site.census.fonts > 0) groups.push(`${site.census.fonts} fonts`);
  if (site.census.media > 0) groups.push(`${site.census.media} media`);

  return (
    <PopupFrame
      icon={Icon}
      title={title}
      subtitle={isQuarry ? "Stone deposit" : "Mountain mine"}
      onClose={onClose}
    >
      <section className="rounded-xl border border-terracotta-200 bg-white px-3 py-2.5">
        <h4 className="mb-1.5 flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-terracotta-500">
          <Hammer className="h-3.5 w-3.5" /> Asset census
        </h4>
        <p className="text-[12px] leading-5 text-cream-600">
          This district's folders hold {total.toLocaleString()} static asset{total === 1 ? "" : "s"}.
        </p>
        {groups.length > 0 && (
          <p className="mt-1.5 text-[11px] font-medium text-cream-500">
            {groups.join(" · ")}
          </p>
        )}
      </section>

      <div className="mt-2 grid grid-cols-1 gap-1.5">
        <Stat icon={MapPin} label="District" value={site.districtLabel} />
      </div>
    </PopupFrame>
  );
}

// ---------------------------------------------------------------------------
// Editor actions (footer) — the "Open file in:" buttons + Copy path
// ---------------------------------------------------------------------------

type ActionState =
  | { kind: "idle" }
  | { kind: "ok"; slug: string }
  | { kind: "err"; slug: string; msg: string };

function EditorActions({ filePath }: { filePath: string }) {
  const inTauri = isTauriRuntime();
  const [state, setState] = useState<ActionState>({ kind: "idle" });
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | null>(null);
  const copyTimer = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
      if (copyTimer.current !== null) window.clearTimeout(copyTimer.current);
    };
  }, []);

  // Reset transient state when the inspected file changes.
  useEffect(() => {
    setState({ kind: "idle" });
    setCopied(false);
  }, [filePath]);

  const openIn = useCallback(
    async (slug: string) => {
      try {
        await invokeBackendCommand<void>("polis_open_in_editor", {
          relativePath: filePath,
          editor: slug,
        });
        setState({ kind: "ok", slug });
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        setState({ kind: "err", slug, msg });
      }
      if (timer.current !== null) window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setState({ kind: "idle" }), 2400);
    },
    [filePath],
  );

  const copyPath = useCallback(async () => {
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(filePath);
      } else {
        const ta = document.createElement("textarea");
        ta.value = filePath;
        ta.style.position = "fixed";
        ta.style.opacity = "0";
        document.body.appendChild(ta);
        ta.select();
        document.execCommand("copy");
        document.body.removeChild(ta);
      }
      setCopied(true);
      if (copyTimer.current !== null) window.clearTimeout(copyTimer.current);
      copyTimer.current = window.setTimeout(() => setCopied(false), 1800);
    } catch {
      // Clipboard can fail in locked-down contexts; ignore.
    }
  }, [filePath]);

  return (
    <div className="space-y-2">
      {inTauri && (
        <>
          <p className="text-[11px] font-semibold uppercase tracking-wider text-cream-400">
            Open file in
          </p>
          <div className="grid grid-cols-2 gap-1.5">
            {EDITORS.map(({ slug, label, icon: BtnIcon }) => {
              const isOk = state.kind === "ok" && state.slug === slug;
              const isErr = state.kind === "err" && state.slug === slug;
              return (
                <button
                  key={slug}
                  onClick={() => void openIn(slug)}
                  title={isErr ? state.msg : `Open in ${label}`}
                  className={`flex items-center justify-center gap-1.5 rounded-xl border px-2.5 py-2 text-[12px] font-medium transition-colors ${
                    isOk
                      ? "border-sage/40 bg-sage/10 text-sage-dark"
                      : isErr
                        ? "border-coral/40 bg-coral/10 text-coral-dark"
                        : "border-cream-300 bg-white text-cream-600 hover:bg-cream-100 hover:text-cream-800"
                  }`}
                >
                  {isOk ? (
                    <Check className="h-3.5 w-3.5 shrink-0" />
                  ) : isErr ? (
                    <X className="h-3.5 w-3.5 shrink-0" />
                  ) : (
                    <BtnIcon className="h-3.5 w-3.5 shrink-0" />
                  )}
                  <span className="truncate">
                    {isErr ? "Couldn't open" : label}
                  </span>
                </button>
              );
            })}
          </div>
        </>
      )}

      <button
        onClick={() => void copyPath()}
        className="flex w-full items-center justify-center gap-2 rounded-xl bg-terracotta px-4 py-2.5 text-[13px] font-medium text-white transition-colors hover:bg-terracotta-500"
      >
        {copied ? (
          <>
            <Check className="h-4 w-4" /> Path copied
          </>
        ) : (
          <>
            <Copy className="h-4 w-4" /> Copy path
          </>
        )}
      </button>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Small building blocks
// ---------------------------------------------------------------------------

interface ConnEntry {
  building: Building;
  weight: number;
}

function Stat({
  icon: Icon,
  label,
  value,
}: {
  icon: LucideIcon;
  label: string;
  value: string;
}) {
  return (
    <div className="flex items-center gap-2.5 rounded-lg bg-white px-2.5 py-1.5">
      <Icon className="h-4 w-4 shrink-0 text-cream-400" />
      <span className="shrink-0 text-[11px] font-semibold uppercase tracking-wide text-cream-400">
        {label}
      </span>
      <span
        className="ml-auto min-w-0 truncate text-right text-[12px] font-medium text-cream-700"
        title={value}
      >
        {value}
      </span>
    </div>
  );
}

function PathRow({ filePath }: { filePath: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | null>(null);

  useEffect(() => {
    setCopied(false);
    return () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
    };
  }, [filePath]);

  const copy = useCallback(async () => {
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(filePath);
        setCopied(true);
        if (timer.current !== null) window.clearTimeout(timer.current);
        timer.current = window.setTimeout(() => setCopied(false), 1600);
      }
    } catch {
      // ignore clipboard failures
    }
  }, [filePath]);

  return (
    <div className="mt-3 rounded-lg border border-cream-200 bg-white px-2.5 py-2">
      <div className="mb-1 flex items-center gap-1.5">
        <FolderTree className="h-3.5 w-3.5 text-cream-400" />
        <span className="text-[11px] font-semibold uppercase tracking-wide text-cream-400">
          File path
        </span>
        <button
          onClick={() => void copy()}
          title="Copy path"
          className="ml-auto inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[10px] font-semibold text-cream-400 transition-colors hover:bg-cream-100 hover:text-terracotta-500"
        >
          {copied ? (
            <>
              <Check className="h-3 w-3" /> Copied
            </>
          ) : (
            <>
              <Copy className="h-3 w-3" /> Copy
            </>
          )}
        </button>
      </div>
      <p className="break-all font-mono text-[11px] leading-relaxed text-cream-600">
        {filePath}
      </p>
    </div>
  );
}

function SectionTitle({
  icon,
  children,
}: {
  icon: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <h4 className="mb-2 flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-cream-400">
      {icon} {children}
    </h4>
  );
}

// A connection group (Imports out / Imported by in). Shows the top
// CONNECTIONS_PREVIEW entries; if there are more, a "+N more" button reveals the
// full list inside a contained, scrollable max-height region so a dense graph
// (50+ edges) never blows out the panel. Every entry navigates to that building.
function ConnGroup({
  icon,
  title,
  entries,
  onSelect,
}: {
  icon: React.ReactNode;
  title: string;
  entries: ConnEntry[];
  onSelect: (b: Building) => void;
}) {
  const [expanded, setExpanded] = useState(false);

  // Collapse the expansion when the building changes (entries identity changes).
  useEffect(() => {
    setExpanded(false);
  }, [entries]);

  const hasMore = entries.length > CONNECTIONS_PREVIEW;
  const preview = entries.slice(0, CONNECTIONS_PREVIEW);
  const rest = entries.slice(CONNECTIONS_PREVIEW);

  return (
    <div>
      <p className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-cream-500">
        {icon} {title}
        <span className="text-cream-400">· {entries.length}</span>
      </p>
      {entries.length === 0 ? (
        <p className="px-1.5 text-[12px] italic text-cream-400">— none —</p>
      ) : (
        <>
          <ul className="space-y-0.5">
            {preview.map((e) => (
              <ConnChip key={e.building.fileId} entry={e} onSelect={onSelect} />
            ))}
          </ul>
          {hasMore && !expanded && (
            <button
              onClick={() => setExpanded(true)}
              className="mt-1 px-1.5 text-[11px] font-semibold text-terracotta-500 hover:text-terracotta-600"
            >
              +{rest.length} more
            </button>
          )}
          {hasMore && expanded && (
            <>
              <ul className="mt-0.5 max-h-44 space-y-0.5 overflow-y-auto rounded-lg border border-cream-200 bg-white/60 p-1">
                {rest.map((e) => (
                  <ConnChip
                    key={e.building.fileId}
                    entry={e}
                    onSelect={onSelect}
                  />
                ))}
              </ul>
              <button
                onClick={() => setExpanded(false)}
                className="mt-1 px-1.5 text-[11px] font-semibold text-terracotta-500 hover:text-terracotta-600"
              >
                Show less
              </button>
            </>
          )}
        </>
      )}
    </div>
  );
}

// One compact, clickable connection chip: a purpose-color dot + filename only,
// so a dense list stays readable. Clicking selects that building on the map.
function ConnChip({
  entry,
  onSelect,
}: {
  entry: ConnEntry;
  onSelect: (b: Building) => void;
}) {
  const b = entry.building;
  return (
    <li>
      <button
        onClick={() => onSelect(b)}
        title={`${b.filePath}  ·  weight ${entry.weight}`}
        className="flex w-full items-center gap-2 rounded-md px-1.5 py-1 text-left transition-colors hover:bg-terracotta-50"
      >
        <span
          className="h-2 w-2 shrink-0 rounded-full ring-1 ring-black/5"
          style={{ backgroundColor: purposeDotColor(b.purpose) }}
        />
        <span className="min-w-0 flex-1 truncate font-mono text-[12px] text-cream-600">
          {b.label}
        </span>
        <ExternalLink className="h-3 w-3 shrink-0 text-cream-300" />
      </button>
    </li>
  );
}

// Format an ISO date robustly as a relative ("3 days ago") or absolute fallback.
function formatRelative(iso: string): string {
  const d = new Date(iso);
  const ms = d.getTime();
  if (Number.isNaN(ms)) return iso;
  const diff = Date.now() - ms;
  if (diff < 0) return formatAbsolute(d);
  const sec = Math.floor(diff / 1000);
  const min = Math.floor(sec / 60);
  const hr = Math.floor(min / 60);
  const day = Math.floor(hr / 24);
  if (sec < 60) return "just now";
  if (min < 60) return `${min} minute${min === 1 ? "" : "s"} ago`;
  if (hr < 24) return `${hr} hour${hr === 1 ? "" : "s"} ago`;
  if (day < 30) return `${day} day${day === 1 ? "" : "s"} ago`;
  return formatAbsolute(d);
}

function formatAbsolute(d: Date): string {
  return d.toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}
