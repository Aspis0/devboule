import {
  Server,
  Cpu,
  Zap,
  Activity,
  AlertTriangle,
  Boxes,
  HardDrive,
  FolderArchive,
  Database,
  Sparkles,
  CreditCard,
  ExternalLink,
  Plus,
  Trash2,
  Save,
  Rocket,
  ClipboardCheck,
  BrainCircuit,
  Search,
} from "lucide-react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useAppContext } from "../../context/AppContext";
import { toOracleError } from "../../utils/oracleError";
import type {
  OracleAnswer,
  OracleError,
  ScalewayBilling,
  ScalewayInstanceCreateRequest,
  ScalewayInstanceDryRunResult,
  ScalewayOfferSummary,
  ScalewayResourceAction,
  ScalewayResourceSummary,
  ScalewayStorageSummary,
} from "../../types/backend";
import { scalewayActionChoices } from "../../utils/scalewayActions";
import {
  ScalewayActionConfirm,
  type PendingScalewayAction,
} from "../compute/ScalewayActionConfirm";

// Resource-type selector. Each entry maps a short UI key to the exact backend
// strings emitted by sync_provider_inventory for Scaleway (verified against
// src-tauri/src/backend/providers.rs `into_summary` impls):
//   - compute (ScalewayResourceSummary.resourceType): "GPU", "CPU VM",
//     "Serverless" (BOTH functions and containers), "Serverless SQL",
//     "Generative API Model".
//   - storage (ScalewayStorageSummary.storageType): "Object Bucket",
//     "Block Storage 5K", "Block Storage 15K", "Block Snapshot", "File System".
// "serverless-functions"/"serverless-containers" BOTH select compute
// resourceType "Serverless" and are SPLIT client-side by the `runtime`
// heuristic (containers carry a runtime starting with "container"; functions
// carry a language runtime like "node20", or null). "billing" and "generic"
// are special views handled outside the standard inventory two-pane.
type ScwResourceType =
  | "gpu"
  | "cpu-vm"
  | "serverless-functions"
  | "serverless-containers"
  | "serverless-sql"
  | "generative"
  | "object-storage"
  | "block-storage"
  | "file-storage"
  | "billing"
  | "generic";

// Which snapshot collection a tab reads from.
type Domain = "compute" | "storage" | "special";

const resourceTabs: {
  id: ScwResourceType;
  label: string;
  icon: typeof Server;
  domain: Domain;
  // Backend string(s) this tab selects (resourceType for compute,
  // storageType for storage). Empty for special/heuristic tabs.
  types: string[];
}[] = [
  { id: "gpu", label: "GPU", icon: Server, domain: "compute", types: ["GPU"] },
  { id: "cpu-vm", label: "CPU VM", icon: Cpu, domain: "compute", types: ["CPU VM"] },
  {
    id: "serverless-functions",
    label: "Functions",
    icon: Zap,
    domain: "compute",
    types: ["Serverless"],
  },
  {
    id: "serverless-containers",
    label: "Containers",
    icon: Boxes,
    domain: "compute",
    types: ["Serverless"],
  },
  {
    id: "serverless-sql",
    label: "Serverless SQL",
    icon: Database,
    domain: "compute",
    types: ["Serverless SQL"],
  },
  {
    id: "generative",
    label: "Generative",
    icon: Sparkles,
    domain: "compute",
    types: ["Generative API Model"],
  },
  {
    id: "object-storage",
    label: "Object Storage",
    icon: FolderArchive,
    domain: "storage",
    types: ["Object Bucket"],
  },
  {
    id: "block-storage",
    label: "Block Storage",
    icon: HardDrive,
    domain: "storage",
    // The backend never emits a bare "Block Volume"/"Block Storage" string;
    // volumes are tagged by IOPS class. Snapshots are a sibling block type.
    types: ["Block Storage 5K", "Block Storage 15K", "Block Snapshot"],
  },
  {
    id: "file-storage",
    label: "File Storage",
    icon: HardDrive,
    domain: "storage",
    types: ["File System"],
  },
  { id: "billing", label: "Billing", icon: CreditCard, domain: "special", types: [] },
  { id: "generic", label: "Generic", icon: Boxes, domain: "special", types: [] },
];

// Compute resourceType strings that DO have a dedicated tab. Anything else
// (e.g. a future Scaleway product) falls into the Generic inventory.
const COVERED_COMPUTE_TYPES = new Set([
  "GPU",
  "CPU VM",
  "Serverless",
  "Serverless SQL",
  "Generative API Model",
]);
const COVERED_STORAGE_TYPES = new Set([
  "Object Bucket",
  "Block Storage 5K",
  "Block Storage 15K",
  "Block Snapshot",
  "File System",
]);

// Classify a "Serverless" compute resource exactly as the backend does
// (commands.rs scaleway_serverless_kind, the source of truth that the delete
// commands enforce): a CONTAINER has runtime "container" or "container/<proto>";
// a FUNCTION has any other non-empty runtime (e.g. "node20"); a missing/blank
// runtime is AMBIGUOUS and the backend REFUSES to delete it. The frontend MUST
// match so a resource never appears under the tab whose delete command will
// reject it. Ambiguous resources surface under Functions (default), where the
// delete simply errors loudly rather than hitting the wrong endpoint.
function isServerlessContainer(resource: ScalewayResourceSummary) {
  const runtime = (resource.runtime ?? "").trim();
  return runtime === "container" || runtime.startsWith("container/");
}

const stateColors = {
  running: { dot: "bg-sage", text: "text-sage-dark", label: "Running" },
  available: { dot: "bg-teal", text: "text-teal-dark", label: "Available" },
  stopped: { dot: "bg-cream-400", text: "text-cream-500", label: "Stopped" },
  provisioning: { dot: "bg-amber", text: "text-amber-dark", label: "Provisioning" },
  error: { dot: "bg-coral", text: "text-coral-dark", label: "Error" },
  unknown: { dot: "bg-cream-300", text: "text-cream-500", label: "Unknown" },
};

const stateRank: Record<string, number> = {
  running: 0,
  available: 1,
  provisioning: 2,
  stopped: 3,
  error: 4,
  unknown: 5,
};

function scaleLabel(resource: ScalewayResourceSummary) {
  if (resource.minScale == null && resource.maxScale == null) {
    return null;
  }
  return `scale ${resource.minScale ?? 0}-${resource.maxScale ?? "?"}`;
}

function timelineLabel(resource: ScalewayResourceSummary | ScalewayStorageSummary) {
  if (resource.updatedAt) {
    return `updated ${resource.updatedAt}`;
  }
  if (resource.createdAt) {
    return `created ${resource.createdAt}`;
  }
  return "no timestamp";
}

function planLabel(resource: ScalewayResourceSummary) {
  const scale = scaleLabel(resource);
  if (resource.commercialType && scale) {
    return `${resource.commercialType} / ${scale}`;
  }
  return resource.commercialType || scale || resource.runtime || "serverless";
}

function serverlessMetadata(resource: ScalewayResourceSummary) {
  return [
    resource.runtime,
    resource.privacy,
    resource.domainName,
    scaleLabel(resource),
  ]
    .filter(Boolean)
    .join(" / ");
}

function formatMemory(gb: number) {
  if (!Number.isFinite(gb) || gb <= 0) return "-";
  return `${gb >= 10 ? Math.round(gb) : gb.toFixed(1)} GB`;
}

function formatPrice(offer: ScalewayOfferSummary) {
  if (offer.hourlyPriceEur != null) {
    return `€${offer.hourlyPriceEur.toFixed(4)}/h`;
  }
  if (offer.monthlyPriceEur != null) {
    return `€${offer.monthlyPriceEur.toFixed(2)}/mo`;
  }
  return "price n/a";
}

function offerRank(offer: ScalewayOfferSummary) {
  const availabilityRank = offer.availability === "available" ? 0 : 1;
  return availabilityRank * 10_000 - offer.gpuCount * 100 - offer.vcpus;
}

function formatEur(value: number | null, suffix: string) {
  if (value == null) return "n/a";
  return `€${value.toFixed(value >= 100 ? 2 : 4)}${suffix}`;
}

// Build the "Open in Scaleway console" deep link for a resource family. Falls
// back to the project dashboard when the family is unknown. Pure string-only.
function scalewayConsoleUrl(family: string): string {
  switch (family) {
    case "GPU":
    case "CPU VM":
      return "https://console.scaleway.com/instance/servers";
    case "Serverless":
      return "https://console.scaleway.com/functions/namespaces";
    case "Serverless SQL":
      return "https://console.scaleway.com/serverless-db/databases";
    case "Generative API Model":
      return "https://console.scaleway.com/generative-api/models";
    case "Object Bucket":
      return "https://console.scaleway.com/object-storage/buckets";
    case "Block Storage 5K":
    case "Block Storage 15K":
    case "Block Snapshot":
      return "https://console.scaleway.com/block-storage/volumes";
    case "File System":
      return "https://console.scaleway.com/file-storage/filesystems";
    default:
      return "https://console.scaleway.com/";
  }
}

// Defensive: Scaleway Serverless SQL uses IAM auth, so its endpoint DSN is
// expected to be `postgresql://user@host:port/db` with NO embedded password.
// But we NEVER render a `user:password@` credential pair even if a future API
// shape or a different product ever returns one — redact the secret before it
// can land in a screenshot, screen-share, or shoulder-surf. Privacy fail-closed.
function redactDsnPassword(dsn: string): string {
  // Match the userinfo segment `scheme://user:password@host` and drop only the
  // `:password` part, keeping the username and the rest of the DSN intact. The
  // password class is `[^@\s]+` (NOT excluding `/`) so a password containing a
  // slash is still fully redacted — `@` cannot appear unencoded inside userinfo,
  // so it remains the reliable terminator.
  return dsn.replace(/(:\/\/[^/@:\s]+):[^@\s]+@/g, "$1:••••••@");
}

export function ComputeView() {
  const {
    cloudSnapshot,
    syncProviderInventory,
    performScalewayResourceAction,
    scalewayInstanceCreateDryRun,
    createScalewayInstance,
    createScalewayBlockVolume,
    resizeScalewayBlockVolume,
    createScalewayBlockSnapshot,
    deleteScalewayBlockStorage,
    createScalewayFilesystem,
    deleteScalewayFilesystem,
    createScalewayObjectBucket,
    deleteScalewayObjectBucket,
    setScalewayObjectBucketLifecycle,
    createScalewaySqlDatabase,
    deleteScalewaySqlDatabase,
    createScalewayFunction,
    deleteScalewayFunction,
    createScalewayContainer,
    deleteScalewayContainer,
    fetchScalewayBilling,
    askOracle,
    isLoading,
  } = useAppContext();

  const [resourceType, setResourceType] = useState<ScwResourceType>("gpu");
  const [actionMessage, setActionMessage] = useState<string | null>(null);
  const [pendingAction, setPendingAction] = useState<PendingScalewayAction | null>(null);
  const [selectedResourceId, setSelectedResourceId] = useState<string | null>(null);

  // Billing is lazy: fetched on first open of the billing tab, latched only on
  // a SUCCESSFUL load so a failed fetch can be retried by re-opening the tab.
  const [billing, setBilling] = useState<ScalewayBilling | null>(null);
  const [billingLoading, setBillingLoading] = useState(false);
  const [billingLoaded, setBillingLoaded] = useState(false);

  const selectedTab = useMemo(
    () => resourceTabs.find((tab) => tab.id === resourceType) ?? resourceTabs[0],
    [resourceType],
  );
  const activeTabTypes = useMemo(() => selectedTab.types, [selectedTab]);

  const computeResources = useMemo(
    () => cloudSnapshot?.compute ?? [],
    [cloudSnapshot?.compute],
  );
  const storageResources = useMemo(
    () => cloudSnapshot?.storage ?? [],
    [cloudSnapshot?.storage],
  );
  const scalewayOffers = useMemo(
    () => cloudSnapshot?.scalewayOffers ?? [],
    [cloudSnapshot?.scalewayOffers],
  );
  const liveScalewayEvents = useMemo(
    () =>
      (cloudSnapshot?.activity ?? [])
        .filter((entry) => entry.source === "Scaleway")
        .slice(0, 4),
    [cloudSnapshot?.activity],
  );

  // The pinned Scaleway project id (mutating creates HARD-FAIL unless the
  // request project id equals this). Creates are gated on it; deletes are not.
  const scalewayScope = useMemo(
    () =>
      cloudSnapshot?.selectedScopes.find((scope) => scope.provider === "scaleway") ??
      null,
    [cloudSnapshot?.selectedScopes],
  );
  const pinnedProjectId = scalewayScope?.id ?? "";
  const actionProjectRequiredReason = pinnedProjectId
    ? ""
    : "Pin a Scaleway project scope (Secrets) before creating resources.";

  // Per-tab compute count helper used for the count badges.
  const computeCount = (types: string[], filter?: (r: ScalewayResourceSummary) => boolean) =>
    computeResources.filter(
      (r) => types.includes(r.resourceType) && (filter ? filter(r) : true),
    ).length;
  const storageCount = (types: string[]) =>
    storageResources.filter((r) => types.includes(r.storageType)).length;
  const tabCount = (tab: (typeof resourceTabs)[number]): number | null => {
    if (tab.id === "serverless-functions") {
      return computeCount(["Serverless"], (r) => !isServerlessContainer(r));
    }
    if (tab.id === "serverless-containers") {
      return computeCount(["Serverless"], (r) => isServerlessContainer(r));
    }
    if (tab.id === "generic") {
      return (
        computeResources.filter((r) => !COVERED_COMPUTE_TYPES.has(r.resourceType)).length +
        storageResources.filter((r) => !COVERED_STORAGE_TYPES.has(r.storageType)).length
      );
    }
    if (tab.domain === "compute") return computeCount(tab.types);
    if (tab.domain === "storage") return storageCount(tab.types);
    return null;
  };

  // Filtered compute resources for the active compute tab (GPU/CPU lifecycle
  // table + serverless split).
  const computeFiltered = useMemo(() => {
    let rows = computeResources.filter((r) => activeTabTypes.includes(r.resourceType));
    if (resourceType === "serverless-functions") {
      rows = rows.filter((r) => !isServerlessContainer(r));
    } else if (resourceType === "serverless-containers") {
      rows = rows.filter((r) => isServerlessContainer(r));
    }
    return rows.sort(
      (a, b) =>
        (stateRank[a.state] ?? 9) - (stateRank[b.state] ?? 9) ||
        a.name.localeCompare(b.name),
    );
  }, [computeResources, activeTabTypes, resourceType]);

  // Filtered storage resources for the active storage tab.
  const storageFiltered = useMemo(
    () =>
      storageResources
        .filter((r) => activeTabTypes.includes(r.storageType))
        .sort(
          (a, b) =>
            (stateRank[a.state] ?? 9) - (stateRank[b.state] ?? 9) ||
            a.name.localeCompare(b.name),
        ),
    [storageResources, activeTabTypes],
  );

  // Generic inventory: everything not covered by a deep tab, projected to a
  // common shape (id/name/family/region/state).
  const genericRows = useMemo(() => {
    const compute = computeResources
      .filter((r) => !COVERED_COMPUTE_TYPES.has(r.resourceType))
      .map((r) => ({
        id: r.id,
        name: r.name,
        family: r.resourceType,
        region: r.region,
        state: r.state,
        domain: "compute" as const,
      }));
    const storage = storageResources
      .filter((r) => !COVERED_STORAGE_TYPES.has(r.storageType))
      .map((r) => ({
        id: r.id,
        name: r.name,
        family: r.storageType,
        region: r.region,
        state: r.state,
        domain: "storage" as const,
      }));
    return [...compute, ...storage].sort(
      (a, b) => a.family.localeCompare(b.family) || a.name.localeCompare(b.name),
    );
  }, [computeResources, storageResources]);

  // For GPU/CPU summary cards + offer catalog (unchanged behavior).
  const runningCount = useMemo(
    () => computeFiltered.filter((r) => r.state === "running").length,
    [computeFiltered],
  );
  const idleRiskItems = useMemo(
    () => computeFiltered.filter((r) => r.idleCostRisk),
    [computeFiltered],
  );
  const runningPlanCount = useMemo(
    () =>
      new Set(
        computeFiltered
          .filter((r) => r.state === "running")
          .map((r) => r.commercialType || r.resourceType),
      ).size,
    [computeFiltered],
  );
  const visibleOffers = useMemo(
    () =>
      scalewayOffers
        .filter((offer) => activeTabTypes.includes(offer.category))
        .sort(
          (a, b) => offerRank(a) - offerRank(b) || a.name.localeCompare(b.name),
        )
        .slice(0, 24),
    [scalewayOffers, activeTabTypes],
  );
  const availableOfferCount = useMemo(
    () =>
      visibleOffers.filter((offer) => offer.availability === "available").length,
    [visibleOffers],
  );

  const isComputeLifecycleTab = resourceType === "gpu" || resourceType === "cpu-vm";
  const isStorageTab = selectedTab.domain === "storage";

  // The currently-selected storage resource (object/block/file detail pane).
  const selectedStorage = useMemo(
    () =>
      storageFiltered.find((r) => r.id === selectedResourceId) ??
      storageFiltered[0] ??
      null,
    [storageFiltered, selectedResourceId],
  );
  // The currently-selected serverless/sql/generative resource (detail pane for
  // the non-lifecycle compute tabs).
  const selectedCompute = useMemo(
    () =>
      computeFiltered.find((r) => r.id === selectedResourceId) ??
      computeFiltered[0] ??
      null,
    [computeFiltered, selectedResourceId],
  );

  // Reset the selected resource whenever the active tab changes — otherwise a
  // selectedResourceId from the previous tab survives and the detail pane
  // silently falls back to [0] of the NEW type, binding a panel/command to the
  // wrong resource (the Cloudflare cross-tab fix).
  useEffect(() => {
    setSelectedResourceId(null);
    setActionMessage(null);
  }, [resourceType]);

  // On lock (cloudSnapshot cleared) drop the latched billing so a re-unlock —
  // possibly under a different scope — forces a fresh fetch instead of showing
  // stale pre-lock amounts. billingLoaded/billing are local state that
  // clearSensitiveState does not reach, so reset them here.
  useEffect(() => {
    if (cloudSnapshot === null) {
      setBilling(null);
      setBillingLoaded(false);
    }
  }, [cloudSnapshot]);

  // Lazy billing fetch (latch only on success; in-flight guard prevents storms).
  // The `cloudSnapshot === null` guard is load-bearing: while locked, the fetch
  // returns null (no latch), and the lock-reset effect above keeps billingLoaded
  // false — without this guard the pair would spin (billingLoading toggles →
  // effect re-fires) flooding the bridge with failing calls.
  useEffect(() => {
    if (
      resourceType !== "billing" ||
      billingLoaded ||
      billingLoading ||
      cloudSnapshot === null
    )
      return;
    setBillingLoading(true);
    void fetchScalewayBilling()
      .then((result) => {
        setBilling(result);
        if (result != null) setBillingLoaded(true);
      })
      .finally(() => setBillingLoading(false));
  }, [resourceType, billingLoaded, billingLoading, cloudSnapshot, fetchScalewayBilling]);

  const runAction = async (
    resource: ScalewayResourceSummary,
    action: ScalewayResourceAction,
    confirmResourceName: string | null,
  ) => {
    setActionMessage(null);
    setPendingAction(null);
    const result = await performScalewayResourceAction(
      resource.id,
      action,
      confirmResourceName,
    );
    if (result) {
      setActionMessage(result.message);
    }
  };

  // Shared sink for panel actions: a panel returns its ScalewayActionResult and
  // we surface the message in the activity banner.
  const reportResult = (message: string | null) => {
    if (message) setActionMessage(message);
  };

  return (
    <div className="space-y-5 max-w-5xl">
      {/* Resource-type selector bar */}
      <div className="flex w-fit max-w-full flex-wrap items-center gap-1 overflow-x-auto rounded-2xl border border-cream-200 bg-white p-1">
        {resourceTabs.map((tab) => {
          const Icon = tab.icon;
          const isActive = resourceType === tab.id;
          const count = tabCount(tab);
          return (
            <button
              key={tab.id}
              onClick={() => setResourceType(tab.id)}
              data-help-title={`This shows Scaleway ${tab.label} resources.`}
              data-help-lines="Tabs only filter the compute page; switching does not call Scaleway.|GPU and CPU VM resources can cost money while running.|Storage, serverless, and SQL tabs expose guarded create/delete actions.|Always sync before terminating, deleting, or creating resources."
              className={`flex items-center gap-2 px-3 py-2 rounded-xl text-[12px] font-medium transition-all duration-200 ${
                isActive
                  ? "bg-terracotta text-white shadow-soft-xs"
                  : "text-cream-500 hover:text-cream-700 hover:bg-cream-50"
              }`}
            >
              <Icon className="w-3.5 h-3.5" />
              {tab.label}
              {count !== null && (
                <span
                  className={`text-[10px] font-semibold px-1.5 py-0.5 rounded-full ${
                    isActive ? "bg-white/20" : "bg-cream-100"
                  }`}
                >
                  {count}
                </span>
              )}
            </button>
          );
        })}
      </div>

      {actionProjectRequiredReason && (
        <p className="rounded-xl bg-amber/[0.08] px-3 py-2 text-[11px] font-semibold text-amber-dark">
          {actionProjectRequiredReason}
        </p>
      )}

      {/* GPU/CPU spawnable offer catalog (unchanged). */}
      {isComputeLifecycleTab && (
        <div className="rounded-2xl border border-cream-200 bg-white p-4">
          <div className="mb-3 flex items-center justify-between gap-4">
            <div className="flex items-center gap-3">
              <div className="flex h-9 w-9 items-center justify-center rounded-xl bg-terracotta-50">
                <Boxes className="h-4.5 w-4.5 text-terracotta" />
              </div>
              <div>
                <p className="text-[11px] font-semibold uppercase tracking-wider text-cream-400">
                  Spawnable {selectedTab.label} catalog
                </p>
                <p className="text-[12px] text-cream-500">
                  {availableOfferCount} available · {visibleOffers.length} shown
                </p>
              </div>
            </div>
            <span className="text-[10px] font-medium text-cream-400">
              Scaleway product API
            </span>
          </div>

          <div className="grid grid-cols-1 gap-2 lg:grid-cols-2">
            {visibleOffers.map((offer) => (
              <div
                key={offer.id}
                className="flex items-center justify-between gap-3 rounded-xl border border-cream-100 px-3 py-2.5"
                data-help-title={`${offer.name} is a spawnable Scaleway offer.`}
                data-help-lines="The catalog shows what Scaleway says can be created in this zone or product family.|For Aspis Bio, use it before deciding GPU/CPU capacity for indexing, pipelines, or analysis jobs.|Catalog visibility does not create a VM by itself.|Before any spawn action, check price, zone, GPU type, RAM, and project scope."
              >
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <p className="font-mono text-[12px] font-semibold text-cream-800">
                      {offer.name}
                    </p>
                    <span
                      className={`rounded-full px-2 py-0.5 text-[10px] font-semibold ${
                        offer.availability === "available"
                          ? "bg-sage/10 text-sage-dark"
                          : "bg-amber/10 text-amber-dark"
                      }`}
                    >
                      {offer.availability}
                    </span>
                  </div>
                  <p className="mt-1 text-[11px] text-cream-500">
                    {offer.zone} · {offer.vcpus} vCPU · {formatMemory(offer.memoryGb)}
                    {offer.gpuCount > 0
                      ? ` · ${offer.gpuLabel ?? `${offer.gpuCount} GPU`}`
                      : ""}
                  </p>
                  <div className="mt-1 flex flex-wrap gap-1">
                    {offer.tags.slice(0, 4).map((tag) => (
                      <span
                        key={tag}
                        className="rounded bg-cream-50 px-1.5 py-0.5 text-[9px] font-medium text-cream-400"
                      >
                        {tag}
                      </span>
                    ))}
                  </div>
                </div>
                <span className="shrink-0 text-right font-mono text-[11px] font-semibold text-cream-600">
                  {formatPrice(offer)}
                </span>
              </div>
            ))}
            {visibleOffers.length === 0 && (
              <div className="rounded-xl bg-cream-50 px-3 py-3 text-[12px] text-cream-400 lg:col-span-2">
                Product catalog unavailable for this tab.
              </div>
            )}
          </div>
        </div>
      )}

      {/* GPU/CPU summary cards (unchanged). */}
      {isComputeLifecycleTab && (
        <div className="grid grid-cols-1 sm:grid-cols-3 gap-4">
          <SummaryCard
            icon={<Activity className="w-4.5 h-4.5 text-sage" />}
            iconBg="bg-sage/10"
            label="Active"
            value={`${runningCount} / ${computeFiltered.length}`}
            helpTitle="Active shows running resources in the current compute tab."
            helpLines="Active resources are likely costing money or serving live work.|For Aspis Bio, running GPU/CPU VMs should have a current task, owner, or project note.|If active count is unexpected, inspect rows below before terminating.|Sync first because old inventory can hide already-stopped machines."
          />
          <SummaryCard
            icon={<Activity className="w-4.5 h-4.5 text-terracotta" />}
            iconBg="bg-terracotta-50"
            label="Running Plans"
            value={String(runningPlanCount)}
            helpTitle="Running Plans groups active compute by machine plan."
            helpLines="This helps show whether Aspis Bio has one type of VM running or several different plans.|Mixed plans can mean old experiments, wrong project resources, or different workloads.|Use this before cleanup so you do not delete the wrong machine family.|It is a summary, not a substitute for row-level inspection."
          />
          <SummaryCard
            icon={
              idleRiskItems.length > 0 ? (
                <AlertTriangle className="w-4.5 h-4.5 text-coral" />
              ) : (
                <Activity className="w-4.5 h-4.5 text-sage" />
              )
            }
            iconBg={idleRiskItems.length > 0 ? "bg-coral/10" : "bg-sage/10"}
            label="Idle Cost Risk"
            value={
              idleRiskItems.length > 0 ? `${idleRiskItems.length} flagged` : "None"
            }
            valueClass={idleRiskItems.length > 0 ? "text-coral" : "text-sage"}
            helpTitle="Idle Cost Risk flags compute that may be wasting money."
            helpLines="Idle risk is the fastest warning for expensive Scaleway mistakes.|For Aspis Bio, idle GPU/CPU VMs should be stopped or deleted after checking disks, outputs, and project evidence.|A zero risk count does not guarantee billing is zero.|Use Budget and provider billing for final confirmation."
          />
        </div>
      )}

      {/* Live activity + sync (shown on every tab so sync is always reachable). */}
      <div className="bg-white rounded-2xl border border-cream-200 p-4">
        <div className="flex items-center justify-between gap-4">
          <p className="text-[11px] text-cream-400 uppercase tracking-wider font-semibold">
            Live Scaleway Activity
          </p>
          <button
            onClick={() => void syncProviderInventory("scaleway")}
            disabled={isLoading}
            data-help-title="This syncs Scaleway live inventory."
            data-help-lines="Sync reads the pinned Aspis Bio Scaleway project.|It updates GPU, CPU VM, serverless, SQL, storage, and activity data.|It should not start, stop, terminate, or delete anything.|If results look wrong, check Secrets for the Scaleway project id and token scope."
            className="shrink-0 px-3 py-2 rounded-xl border border-cream-200 bg-white text-[12px] font-medium text-cream-600 hover:border-terracotta-200 hover:text-terracotta disabled:opacity-60"
          >
            {isLoading ? "Syncing..." : "Sync"}
          </button>
        </div>

        {actionMessage && (
          <div className="mt-3 rounded-xl border border-sage/20 bg-sage/10 px-3 py-2 text-[12px] font-medium text-sage-dark">
            {actionMessage}
          </div>
        )}

        <div className="mt-3 grid gap-2">
          {liveScalewayEvents.map((event) => (
            <div
              key={event.id}
              className="flex items-center justify-between gap-3 rounded-xl bg-cream-50 px-3 py-2"
            >
              <span className="min-w-0 text-[12px] font-medium leading-5 text-cream-700 line-clamp-2">
                {event.message}
              </span>
              <span className="shrink-0 text-[10px] font-mono text-cream-400">
                {event.timestamp}
              </span>
            </div>
          ))}
          {liveScalewayEvents.length === 0 && (
            <div className="rounded-xl bg-cream-50 px-3 py-3 text-[12px] text-cream-400">
              No Scaleway activity.
            </div>
          )}
        </div>
      </div>

      {/* GPU/CPU lifecycle table + create form. */}
      {isComputeLifecycleTab && (
        <div className="space-y-4">
          <InstanceLifecycleTable
            rows={computeFiltered}
            tabLabel={selectedTab.label}
            isLoading={isLoading}
            onPick={(resource, choice) => setPendingAction({ resource, choice })}
          />
          <InstanceCreatePanel
            key={resourceType}
            tabLabel={selectedTab.label}
            offers={visibleOffers}
            pinnedProjectId={pinnedProjectId}
            blockedReason={actionProjectRequiredReason}
            isLoading={isLoading}
            scalewayInstanceCreateDryRun={scalewayInstanceCreateDryRun}
            createScalewayInstance={createScalewayInstance}
            onResult={reportResult}
          />
        </div>
      )}

      {/* Serverless Functions / Containers detail + create/delete. */}
      {(resourceType === "serverless-functions" ||
        resourceType === "serverless-containers") && (
        <ComputeTwoPane
          rows={computeFiltered}
          selected={selectedCompute}
          onSelect={setSelectedResourceId}
          tabLabel={selectedTab.label}
          renderMeta={(r) => serverlessMetadata(r) || planLabel(r)}
          askOracle={askOracle}
          detail={
            selectedCompute ? (
              <ServerlessPanel
                key={selectedCompute.id}
                resource={selectedCompute}
                kind={
                  resourceType === "serverless-containers" ? "container" : "function"
                }
                pinnedProjectId={pinnedProjectId}
                blockedReason={actionProjectRequiredReason}
                isLoading={isLoading}
                onDeploy={(resource) => {
                  const choice = scalewayActionChoices(resource).find(
                    (c) => c.action === "deploy",
                  );
                  if (choice) setPendingAction({ resource, choice });
                }}
                createScalewayFunction={createScalewayFunction}
                createScalewayContainer={createScalewayContainer}
                deleteScalewayFunction={deleteScalewayFunction}
                deleteScalewayContainer={deleteScalewayContainer}
                onResult={reportResult}
              />
            ) : null
          }
        />
      )}

      {/* Serverless SQL detail + create/delete + DSN. */}
      {resourceType === "serverless-sql" && (
        <ComputeTwoPane
          rows={computeFiltered}
          selected={selectedCompute}
          onSelect={setSelectedResourceId}
          tabLabel={selectedTab.label}
          renderMeta={(r) => r.commercialType || scaleLabel(r) || "serverless-sql"}
          askOracle={askOracle}
          detail={
            <SqlPanel
              key={selectedCompute?.id ?? "none"}
              resource={selectedCompute}
              pinnedProjectId={pinnedProjectId}
              blockedReason={actionProjectRequiredReason}
              isLoading={isLoading}
              createScalewaySqlDatabase={createScalewaySqlDatabase}
              deleteScalewaySqlDatabase={deleteScalewaySqlDatabase}
              onResult={reportResult}
            />
          }
        />
      )}

      {/* Generative API Model — inspect only. */}
      {resourceType === "generative" && (
        <ComputeTwoPane
          rows={computeFiltered}
          selected={selectedCompute}
          onSelect={setSelectedResourceId}
          tabLabel={selectedTab.label}
          renderMeta={(r) => r.runtime || r.purpose}
          askOracle={askOracle}
          detail={
            selectedCompute ? (
              <GenericInspectPanel
                title="Generative API Model"
                subtitle="Inspect-only. Generative models are served from fr-par and are not project-scoped; manage them in the Scaleway console."
                family="Generative API Model"
              />
            ) : null
          }
        />
      )}

      {/* Object / Block / File storage two-pane. */}
      {isStorageTab && (
        <StorageTwoPane
          rows={storageFiltered}
          selected={selectedStorage}
          onSelect={setSelectedResourceId}
          tabLabel={selectedTab.label}
          detail={
            // isStorageTab guarantees one of the three storage tabs; each panel
            // renders its create-only form when no resource is selected.
            resourceType === "object-storage" ? (
                <ObjectStoragePanel
                  key={selectedStorage?.id ?? "none"}
                  resource={selectedStorage}
                  pinnedProjectId={pinnedProjectId}
                  blockedReason={actionProjectRequiredReason}
                  isLoading={isLoading}
                  createScalewayObjectBucket={createScalewayObjectBucket}
                  deleteScalewayObjectBucket={deleteScalewayObjectBucket}
                  setScalewayObjectBucketLifecycle={setScalewayObjectBucketLifecycle}
                  onResult={reportResult}
                />
              ) : resourceType === "block-storage" ? (
                <BlockStoragePanel
                  key={selectedStorage?.id ?? "none"}
                  resource={selectedStorage}
                  pinnedProjectId={pinnedProjectId}
                  blockedReason={actionProjectRequiredReason}
                  isLoading={isLoading}
                  createScalewayBlockVolume={createScalewayBlockVolume}
                  resizeScalewayBlockVolume={resizeScalewayBlockVolume}
                  createScalewayBlockSnapshot={createScalewayBlockSnapshot}
                  deleteScalewayBlockStorage={deleteScalewayBlockStorage}
                  onResult={reportResult}
                />
              ) : (
                <FileStoragePanel
                  key={selectedStorage?.id ?? "none"}
                  resource={selectedStorage}
                  pinnedProjectId={pinnedProjectId}
                  blockedReason={actionProjectRequiredReason}
                  isLoading={isLoading}
                  createScalewayFilesystem={createScalewayFilesystem}
                  deleteScalewayFilesystem={deleteScalewayFilesystem}
                  onResult={reportResult}
                />
              )
          }
        />
      )}

      {/* Generic inventory (uncovered families). */}
      {resourceType === "generic" && (
        <GenericInventory rows={genericRows} />
      )}

      {/* Billing (lazy). */}
      {resourceType === "billing" && (
        <BillingView billing={billing} loading={billingLoading} />
      )}

      <ScalewayActionConfirm
        pending={pendingAction}
        isLoading={isLoading}
        onCancel={() => setPendingAction(null)}
        onConfirm={(resource, action, confirmResourceName) =>
          void runAction(resource, action, confirmResourceName)
        }
      />
    </div>
  );
}

// ===========================================================================
// Shared primitives
// ===========================================================================

function SummaryCard({
  icon,
  iconBg,
  label,
  value,
  valueClass,
  helpTitle,
  helpLines,
}: {
  icon: ReactNode;
  iconBg: string;
  label: string;
  value: string;
  valueClass?: string;
  helpTitle: string;
  helpLines: string;
}) {
  return (
    <div
      className="flex items-center gap-3 p-4 bg-white rounded-2xl border border-cream-200"
      data-help-title={helpTitle}
      data-help-lines={helpLines}
    >
      <div className={`w-9 h-9 rounded-xl flex items-center justify-center ${iconBg}`}>
        {icon}
      </div>
      <div>
        <p className="text-[11px] text-cream-400 uppercase tracking-wider font-semibold">
          {label}
        </p>
        <p
          className={`text-lg font-semibold tabular-nums ${
            valueClass ?? "text-cream-800"
          }`}
        >
          {value}
        </p>
      </div>
    </div>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0">
      <p className="text-[10px] font-semibold uppercase tracking-widest text-cream-400">
        {label}
      </p>
      <p className="mt-1 truncate text-[13px] font-semibold text-cream-800">{value}</p>
    </div>
  );
}

function PanelShell({
  title,
  subtitle,
  children,
}: {
  title: string;
  subtitle: string;
  children: ReactNode;
}) {
  return (
    <div className="rounded-2xl border border-cream-100 bg-cream-50 p-4">
      <div className="mb-3">
        <h4 className="text-[13px] font-semibold text-cream-900">{title}</h4>
        <p className="mt-1 text-[11px] leading-5 text-cream-500">{subtitle}</p>
      </div>
      {children}
    </div>
  );
}

function MutateBlockedNote({ reason }: { reason: string }) {
  return (
    <p className="mb-3 rounded-xl bg-amber/[0.08] px-3 py-2 text-[11px] font-semibold text-amber-dark">
      {reason}
    </p>
  );
}

function ConsoleLink({ family }: { family: string }) {
  return (
    <a
      href={scalewayConsoleUrl(family)}
      target="_blank"
      rel="noreferrer"
      data-help-title="This opens the Scaleway console for this resource family."
      data-help-lines="The link points to the Scaleway dashboard page for this family.|It opens in your default browser, outside the app.|It is read-only navigation, not a console action.|Use it for actions the app does not expose."
      className="inline-flex items-center gap-1.5 rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:border-terracotta-200 hover:text-terracotta"
    >
      <ExternalLink className="h-3.5 w-3.5" />
      Open in Scaleway console
    </a>
  );
}

function TextField({
  label,
  value,
  onChange,
  disabled,
  placeholder,
  mono,
  help,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  disabled?: boolean;
  placeholder?: string;
  mono?: boolean;
  help?: string;
}) {
  return (
    <label className="block">
      <span className="text-[10px] font-semibold uppercase tracking-widest text-cream-400">
        {label}
      </span>
      <input
        value={value}
        onChange={(event) => onChange(event.target.value)}
        disabled={disabled}
        placeholder={placeholder}
        spellCheck={false}
        title={help}
        className={`mt-1 w-full rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] text-cream-800 outline-none focus:border-terracotta/30 disabled:opacity-60 ${
          mono ? "font-mono" : ""
        }`}
      />
    </label>
  );
}

function NumberField({
  label,
  value,
  onChange,
  disabled,
  placeholder,
  help,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  disabled?: boolean;
  placeholder?: string;
  help?: string;
}) {
  return (
    <label className="block">
      <span className="text-[10px] font-semibold uppercase tracking-widest text-cream-400">
        {label}
      </span>
      <input
        value={value}
        inputMode="numeric"
        onChange={(event) => onChange(event.target.value.replace(/[^0-9]/g, ""))}
        disabled={disabled}
        placeholder={placeholder}
        spellCheck={false}
        title={help}
        className="mt-1 w-full rounded-xl border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] text-cream-800 outline-none focus:border-terracotta/30 disabled:opacity-60"
      />
    </label>
  );
}

function ToggleField({
  label,
  checked,
  onChange,
  disabled,
  help,
}: {
  label: string;
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  help?: string;
}) {
  return (
    <label
      className="flex items-center justify-between gap-3 rounded-xl border border-cream-100 bg-white px-3 py-2"
      title={help}
    >
      <span className="text-[11px] font-semibold text-cream-700">{label}</span>
      <input
        type="checkbox"
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
        disabled={disabled}
        className="h-4 w-4 accent-terracotta disabled:opacity-60"
      />
    </label>
  );
}

// A type-the-name confirm block reused by every delete panel (mirrors the
// ScalewayActionConfirm contract but inline, since deletes here act on the
// detail-pane resource rather than a row choice).
function DeleteConfirm({
  resourceName,
  label,
  hint,
  canMutate,
  isLoading,
  busy,
  onDelete,
}: {
  resourceName: string;
  label: string;
  hint: string;
  canMutate: boolean;
  isLoading: boolean;
  busy: boolean;
  onDelete: () => void;
}) {
  const [typed, setTyped] = useState("");
  const ok = typed === resourceName;
  return (
    <div className="rounded-xl border border-coral/30 bg-coral/5 p-3">
      <p className="text-[11px] font-semibold text-coral-dark">{label}</p>
      <p className="mt-1 text-[11px] leading-5 text-cream-500">{hint}</p>
      <input
        value={typed}
        onChange={(event) => setTyped(event.target.value)}
        disabled={isLoading || busy || !canMutate}
        placeholder={resourceName}
        spellCheck={false}
        data-help-title="This confirmation protects a destructive Scaleway delete."
        data-help-lines="Type the exact resource name so accidental clicks do not run.|Delete is a real provider mutation, not a dry run.|Check region, project, and resource id before confirming.|If unsure, cancel and run sync first."
        className="mt-2 w-full rounded-xl border border-coral/20 bg-white px-3 py-2 font-mono text-[12px] text-cream-800 outline-none focus:border-coral/50 disabled:opacity-60"
      />
      <button
        type="button"
        onClick={onDelete}
        disabled={isLoading || busy || !canMutate || !ok}
        title={!ok ? "Type the exact resource name to enable delete." : undefined}
        className="mt-2 flex items-center gap-1.5 rounded-xl bg-coral px-3 py-1.5 text-[11px] font-semibold text-white disabled:opacity-50"
      >
        <Trash2 className="h-3.5 w-3.5" />
        {busy ? "Deleting..." : label}
      </button>
    </div>
  );
}

// ===========================================================================
// GPU / CPU lifecycle table
// ===========================================================================

function InstanceLifecycleTable({
  rows,
  tabLabel,
  isLoading,
  onPick,
}: {
  rows: ScalewayResourceSummary[];
  tabLabel: string;
  isLoading: boolean;
  onPick: (
    resource: ScalewayResourceSummary,
    choice: ReturnType<typeof scalewayActionChoices>[number],
  ) => void;
}) {
  return (
    <div className="bg-white rounded-2xl border border-cream-200 overflow-hidden">
      <div className="overflow-x-auto">
        <table className="w-full text-left">
          <thead>
            <tr className="border-b border-cream-100">
              <th className="px-5 py-3 text-[10px] font-semibold text-cream-400 uppercase tracking-wider">
                Name
              </th>
              <th className="px-4 py-3 text-[10px] font-semibold text-cream-400 uppercase tracking-wider">
                Region
              </th>
              <th className="px-4 py-3 text-[10px] font-semibold text-cream-400 uppercase tracking-wider">
                State
              </th>
              <th className="px-4 py-3 text-[10px] font-semibold text-cream-400 uppercase tracking-wider text-right">
                Plan
              </th>
              <th className="px-4 py-3 text-[10px] font-semibold text-cream-400 uppercase tracking-wider text-right">
                Resource ID
              </th>
              <th className="px-4 py-3 text-[10px] font-semibold text-cream-400 uppercase tracking-wider text-right">
                Risk
              </th>
              <th className="px-5 py-3 text-[10px] font-semibold text-cream-400 uppercase tracking-wider text-right">
                Ops
              </th>
            </tr>
          </thead>
          <tbody className="divide-y divide-cream-50">
            {rows.map((r) => {
              const st =
                stateColors[r.state as keyof typeof stateColors] || stateColors.unknown;
              const actions = scalewayActionChoices(r);
              return (
                <tr
                  key={r.id}
                  className={`hover:bg-cream-50/50 transition-colors ${
                    r.idleCostRisk ? "bg-coral/[0.03]" : ""
                  }`}
                  data-help-title={`${r.name} is a Scaleway ${r.resourceType} resource.`}
                  data-help-lines="This row is the live inventory entry for one compute resource.|For Aspis Bio, check project name, region, state, plan, public IP, tags, and idle risk before any lifecycle action.|Rows can represent expensive GPU/CPU VMs.|If this does not belong to Aspis Bio, verify the pinned Scaleway project scope in Secrets."
                >
                  <td className="px-5 py-3">
                    <div className="flex items-center gap-2">
                      {r.idleCostRisk && (
                        <AlertTriangle className="w-3.5 h-3.5 text-coral shrink-0" />
                      )}
                      <span className="text-[13px] font-mono font-medium text-cream-800">
                        {r.name}
                      </span>
                    </div>
                    <p className="mt-0.5 max-w-[340px] truncate text-[11px] text-cream-400">
                      {r.purpose}
                    </p>
                    <div className="mt-1 truncate text-[10px] font-mono text-cream-300">
                      {[
                        r.projectName || "Project unknown",
                        r.purposeSource,
                        timelineLabel(r),
                        r.image,
                        r.publicIp,
                        r.tags.slice(0, 2).join(" / "),
                      ]
                        .filter(Boolean)
                        .join(" / ")}
                    </div>
                  </td>
                  <td className="px-4 py-3 text-[12px] text-cream-500 font-mono">
                    {r.region}
                  </td>
                  <td className="px-4 py-3">
                    <span className="inline-flex items-center gap-1.5">
                      <span className={`w-1.5 h-1.5 rounded-full ${st.dot}`} />
                      <span className={`text-[11px] font-medium ${st.text}`}>
                        {st.label}
                      </span>
                    </span>
                  </td>
                  <td className="px-4 py-3 text-right">
                    <span className="text-[12px] font-mono text-cream-500">
                      {planLabel(r)}
                    </span>
                  </td>
                  <td className="px-4 py-3 text-right">
                    <div className="text-[13px] font-mono text-cream-700 tabular-nums">
                      {r.id.slice(0, 12)}
                    </div>
                    <div className="mt-0.5 max-w-[170px] truncate text-[10px] font-mono text-cream-300">
                      {timelineLabel(r)}
                    </div>
                  </td>
                  <td className="px-4 py-3 text-right">
                    <span
                      className={`text-[13px] font-mono tabular-nums ${
                        r.idleCostRisk ? "text-coral font-medium" : "text-cream-700"
                      }`}
                    >
                      {r.idleCostRisk ? "idle" : "clear"}
                    </span>
                  </td>
                  <td className="px-5 py-3 text-right">
                    <div className="flex items-center justify-end gap-1.5">
                      {actions.map((choice) => (
                        <button
                          key={choice.action}
                          onClick={() => onPick(r, choice)}
                          disabled={isLoading}
                          data-help-title={`${choice.label} is a Scaleway VM lifecycle action.`}
                          data-help-lines="Lifecycle actions can change cost and availability.|Terminate/delete are destructive and run only after the typed-name confirmation.|The confirmation dialog shows the exact resource name before execution.|Verifier agents should only read this state."
                          className={`rounded-lg border px-2.5 py-1.5 text-[11px] font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-50 ${
                            choice.tone === "critical"
                              ? "border-coral bg-coral/10 text-coral-dark hover:bg-coral/15"
                              : choice.tone === "danger"
                                ? "border-coral/20 text-coral-dark hover:bg-coral/10"
                                : choice.tone === "primary"
                                  ? "border-sage/20 text-sage-dark hover:bg-sage/10"
                                  : "border-cream-200 text-cream-600 hover:border-terracotta-200 hover:text-terracotta"
                          }`}
                        >
                          {choice.label}
                        </button>
                      ))}
                      {actions.length === 0 && (
                        <span className="text-[11px] text-cream-300">Unavailable</span>
                      )}
                    </div>
                  </td>
                </tr>
              );
            })}
            {rows.length === 0 && (
              <tr>
                <td
                  colSpan={7}
                  className="px-5 py-8 text-center text-[13px] text-cream-400"
                >
                  No {tabLabel.toLowerCase()} resources configured.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}

// ===========================================================================
// Instance create (dry-run → preview → confirm)
// ===========================================================================

function InstanceCreatePanel({
  tabLabel,
  offers,
  pinnedProjectId,
  blockedReason,
  isLoading,
  scalewayInstanceCreateDryRun,
  createScalewayInstance,
  onResult,
}: {
  tabLabel: string;
  offers: ScalewayOfferSummary[];
  pinnedProjectId: string;
  blockedReason: string;
  isLoading: boolean;
  scalewayInstanceCreateDryRun: (
    request: ScalewayInstanceCreateRequest,
  ) => Promise<ScalewayInstanceDryRunResult | null>;
  createScalewayInstance: (
    request: ScalewayInstanceCreateRequest,
  ) => Promise<{ message: string } | null>;
  onResult: (message: string | null) => void;
}) {
  const [name, setName] = useState("");
  const [zone, setZone] = useState("");
  const [commercialType, setCommercialType] = useState("");
  const [image, setImage] = useState("");
  const [dynamicIp, setDynamicIp] = useState(true);
  const [tags, setTags] = useState("");

  const [dryRun, setDryRun] = useState<ScalewayInstanceDryRunResult | null>(null);
  // The exact request the stored dry-run was computed for; editing any field
  // re-disables Confirm until a fresh dry-run runs for the new request.
  const [dryRunFor, setDryRunFor] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const requestId = useRef(0);

  useEffect(() => {
    return () => {
      requestId.current += 1;
    };
  }, []);

  const canMutate = Boolean(pinnedProjectId);
  const offerZones = useMemo(
    () => Array.from(new Set(offers.map((o) => o.zone))).sort(),
    [offers],
  );
  const offerTypes = useMemo(
    () => Array.from(new Set(offers.map((o) => o.name))).sort(),
    [offers],
  );

  const buildRequest = (): ScalewayInstanceCreateRequest | null => {
    const trimmedName = name.trim();
    const trimmedZone = zone.trim();
    const trimmedType = commercialType.trim();
    const trimmedImage = image.trim();
    if (!trimmedName || !trimmedZone || !trimmedType || !trimmedImage) return null;
    return {
      name: trimmedName,
      zone: trimmedZone,
      commercialType: trimmedType,
      image: trimmedImage,
      projectId: pinnedProjectId,
      dynamicIpRequired: dynamicIp,
      tags: tags
        .split(",")
        .map((t) => t.trim())
        .filter(Boolean),
    };
  };

  // A stable fingerprint of the request for the dry-run freshness check.
  const requestKey = (req: ScalewayInstanceCreateRequest) =>
    JSON.stringify(req);

  const currentRequest = buildRequest();
  const dryRunFresh =
    dryRun !== null &&
    dryRunFor !== null &&
    currentRequest !== null &&
    dryRunFor === requestKey(currentRequest);

  const runDryRun = async () => {
    const req = buildRequest();
    if (!req) return;
    const id = requestId.current + 1;
    requestId.current = id;
    const result = await scalewayInstanceCreateDryRun(req);
    if (requestId.current !== id) return;
    if (result) {
      setDryRun(result);
      setDryRunFor(requestKey(req));
    }
  };

  const confirmCreate = async () => {
    const req = buildRequest();
    if (!req || !canMutate || !dryRunFresh) return;
    const id = requestId.current + 1;
    requestId.current = id;
    setRunning(true);
    try {
      const result = await createScalewayInstance(req);
      if (requestId.current !== id) return;
      if (result) {
        onResult(result.message);
        setName("");
        setImage("");
        setTags("");
        setDryRun(null);
        setDryRunFor(null);
      }
    } finally {
      if (requestId.current === id) setRunning(false);
    }
  };

  return (
    <PanelShell
      title={`Create ${tabLabel} instance`}
      subtitle="Fill the form, run a dry run to preview the exact request body and estimated cost, then confirm. Confirm stays blocked until a dry run matches the current form."
    >
      {!canMutate && <MutateBlockedNote reason={blockedReason} />}
      <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
        <TextField
          label="Name"
          value={name}
          onChange={(v) => {
            setName(v);
          }}
          disabled={isLoading}
          placeholder="my-instance"
          mono
        />
        <label className="block">
          <span className="text-[10px] font-semibold uppercase tracking-widest text-cream-400">
            Zone
          </span>
          <input
            list="scw-instance-zones"
            value={zone}
            onChange={(event) => setZone(event.target.value)}
            disabled={isLoading}
            placeholder="fr-par-1"
            spellCheck={false}
            className="mt-1 w-full rounded-xl border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] text-cream-800 outline-none focus:border-terracotta/30 disabled:opacity-60"
          />
          <datalist id="scw-instance-zones">
            {offerZones.map((z) => (
              <option key={z} value={z} />
            ))}
          </datalist>
        </label>
        <label className="block">
          <span className="text-[10px] font-semibold uppercase tracking-widest text-cream-400">
            Commercial type
          </span>
          <input
            list="scw-instance-types"
            value={commercialType}
            onChange={(event) => setCommercialType(event.target.value)}
            disabled={isLoading}
            placeholder="GP1-S"
            spellCheck={false}
            className="mt-1 w-full rounded-xl border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] text-cream-800 outline-none focus:border-terracotta/30 disabled:opacity-60"
          />
          <datalist id="scw-instance-types">
            {offerTypes.map((t) => (
              <option key={t} value={t} />
            ))}
          </datalist>
        </label>
        <TextField
          label="Image (UUID)"
          value={image}
          onChange={setImage}
          disabled={isLoading}
          placeholder="image uuid"
          mono
          help="The Scaleway image UUID to boot from."
        />
        <TextField
          label="Tags (comma-separated)"
          value={tags}
          onChange={setTags}
          disabled={isLoading}
          placeholder="aspis, gpu"
        />
        <ToggleField
          label="Request dynamic public IP"
          checked={dynamicIp}
          onChange={setDynamicIp}
          disabled={isLoading}
          help="Allocates a dynamic public IPv4 for the instance."
        />
      </div>

      <div className="mt-3 flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={() => void runDryRun()}
          disabled={isLoading || running || currentRequest === null}
          className="flex items-center gap-1.5 rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:border-terracotta-200 hover:text-terracotta disabled:opacity-50"
        >
          <ClipboardCheck className="h-3.5 w-3.5" />
          Dry run
        </button>
        <button
          type="button"
          onClick={() => void confirmCreate()}
          disabled={isLoading || running || !canMutate || !dryRunFresh}
          title={
            !canMutate
              ? blockedReason
              : !dryRunFresh
                ? "Run a dry run for the current form first."
                : undefined
          }
          className="flex items-center gap-1.5 rounded-xl bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-50"
        >
          <Plus className="h-3.5 w-3.5" />
          {running ? "Creating..." : "Confirm create"}
        </button>
      </div>

      {dryRunFresh && dryRun && (
        <div className="mt-3 grid grid-cols-1 gap-3 lg:grid-cols-2">
          <div className="rounded-xl bg-white p-3">
            <h5 className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
              Estimated cost
            </h5>
            <div className="flex flex-wrap gap-4">
              <Metric label="Hourly" value={formatEur(dryRun.estimatedHourlyEur, "/h")} />
              <Metric
                label="Monthly"
                value={formatEur(dryRun.estimatedMonthlyEur, "/mo")}
              />
            </div>
            {dryRun.risks.length > 0 && (
              <div className="mt-3 space-y-1">
                {dryRun.risks.map((risk) => (
                  <p key={risk} className="text-[11px] leading-5 text-amber-dark">
                    {risk}
                  </p>
                ))}
              </div>
            )}
          </div>
          <div className="rounded-xl border border-cream-100 bg-white p-3">
            <h5 className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
              Request body preview
            </h5>
            <pre className="max-h-48 overflow-auto rounded-lg bg-cream-50 px-3 py-2 text-[10px] leading-4 text-cream-700">
              {dryRun.bodyPreview}
            </pre>
          </div>
        </div>
      )}
    </PanelShell>
  );
}

// ===========================================================================
// Generic two-pane shells (compute / storage)
// ===========================================================================

function OraclePane({
  oracleQuery,
  fallbackLabel,
  askOracle,
}: {
  oracleQuery: string;
  fallbackLabel: string;
  askOracle: (query: string, limit?: number) => Promise<OracleAnswer>;
}) {
  const [answer, setAnswer] = useState<OracleAnswer | null>(null);
  const [error, setError] = useState<OracleError | null>(null);
  const [loading, setLoading] = useState(false);
  const requestId = useRef(0);

  useEffect(() => {
    const id = requestId.current + 1;
    requestId.current = id;
    setAnswer(null);
    setError(null);
    setLoading(true);
    void askOracle(oracleQuery || fallbackLabel, 4)
      .then((a) => {
        if (requestId.current === id) setAnswer(a);
      })
      .catch((e) => {
        if (requestId.current === id) {
          setAnswer(null);
          setError(toOracleError(e));
        }
      })
      .finally(() => {
        if (requestId.current === id) setLoading(false);
      });
    return () => {
      requestId.current += 1;
    };
  }, [oracleQuery, fallbackLabel, askOracle]);

  return (
    <div
      className="rounded-2xl border border-cream-100 bg-cream-50 p-4"
      data-help-title="Oracle links the live resource to local architecture chunks."
      data-help-lines="Oracle is a read path that connects this Scaleway resource to indexed code and notes.|Use it to understand ownership and intent before changing or deleting.|It does not change Scaleway.|If results are weak, refresh the Oracle index and provider inventory."
    >
      <div className="mb-2 flex items-center gap-2">
        <BrainCircuit className="h-3.5 w-3.5 text-teal" />
        <h4 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          Oracle explanation
        </h4>
      </div>
      {loading ? (
        <p className="text-[12px] text-cream-400">Asking Oracle about this resource...</p>
      ) : error ? (
        <div className="rounded-xl border border-coral/30 bg-coral/5 px-3 py-2">
          <div className="flex items-start gap-2">
            <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-coral-dark" />
            <div className="min-w-0">
              <p className="text-[12px] font-semibold leading-5 text-coral-dark">
                {error.message}
              </p>
              {error.remediation && (
                <p className="mt-1 text-[11px] leading-5 text-cream-500">
                  {error.remediation}
                </p>
              )}
            </div>
          </div>
        </div>
      ) : answer ? (
        <div className="space-y-2">
          <p className="text-[12px] leading-5 text-cream-600">
            {answer.summary || answer.answer}
          </p>
          {answer.results.map((result) => (
            <div key={result.id} className="rounded-xl bg-white px-3 py-2">
              <p className="truncate text-[12px] font-medium text-cream-800">
                {result.label}
              </p>
              <p className="truncate font-mono text-[10px] text-cream-400">
                {result.fileSource}
              </p>
            </div>
          ))}
        </div>
      ) : (
        <p className="text-[12px] text-cream-400">No Oracle match yet.</p>
      )}
    </div>
  );
}

function ComputeTwoPane({
  rows,
  selected,
  onSelect,
  tabLabel,
  renderMeta,
  detail,
  askOracle,
}: {
  rows: ScalewayResourceSummary[];
  selected: ScalewayResourceSummary | null;
  onSelect: (id: string) => void;
  tabLabel: string;
  renderMeta: (r: ScalewayResourceSummary) => string;
  detail: ReactNode;
  askOracle: (query: string, limit?: number) => Promise<OracleAnswer>;
}) {
  return (
    <div className="grid grid-cols-1 gap-4 xl:grid-cols-[minmax(0,1.1fr)_minmax(320px,0.9fr)] xl:items-start">
      <section className="rounded-2xl border border-cream-200 bg-white p-5 xl:sticky xl:top-0 xl:self-start xl:max-h-[calc(100vh-7rem)] xl:overflow-y-auto">
        <h3 className="mb-3 text-[15px] font-semibold text-cream-900">
          {tabLabel} inventory
        </h3>
        <div className="overflow-hidden rounded-xl border border-cream-100">
          {rows.map((r) => {
            const active = selected?.id === r.id;
            const st =
              stateColors[r.state as keyof typeof stateColors] || stateColors.unknown;
            return (
              <button
                key={r.id}
                type="button"
                onClick={() => onSelect(r.id)}
                className={`grid w-full grid-cols-[minmax(0,1fr)_90px] items-center gap-2 border-b border-cream-50 px-3 py-2 text-left last:border-b-0 ${
                  active ? "bg-terracotta/[0.06]" : "bg-white hover:bg-cream-50"
                }`}
              >
                <div className="min-w-0">
                  <p className="truncate font-mono text-[12px] font-semibold text-cream-800">
                    {r.name}
                  </p>
                  <p className="mt-0.5 truncate text-[11px] text-cream-400">
                    {renderMeta(r)}
                  </p>
                </div>
                <span className="inline-flex items-center justify-end gap-1.5">
                  <span className={`w-1.5 h-1.5 rounded-full ${st.dot}`} />
                  <span className={`text-[11px] font-medium ${st.text}`}>
                    {st.label}
                  </span>
                </span>
              </button>
            );
          })}
          {rows.length === 0 && (
            <p className="px-3 py-8 text-center text-[13px] text-cream-400">
              No {tabLabel.toLowerCase()} resources. Sync Scaleway.
            </p>
          )}
        </div>
      </section>

      <section className="space-y-4 rounded-2xl border border-cream-200 bg-white p-5 xl:sticky xl:top-0 xl:self-start xl:max-h-[calc(100vh-7rem)] xl:overflow-y-auto">
        <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          Resource detail
        </h3>
        {selected ? (
          <>
            <div>
              <p className="font-mono text-[15px] font-semibold text-cream-900">
                {selected.name}
              </p>
              <p className="mt-1 text-[12px] leading-5 text-cream-500">
                {selected.purpose}
              </p>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <Metric label="Type" value={selected.resourceType} />
              <Metric label="Region" value={selected.region} />
              <Metric label="State" value={selected.state} />
              <Metric label="Project" value={selected.projectName || "unknown"} />
            </div>
            <OraclePane
              oracleQuery={selected.oracleQuery}
              fallbackLabel={selected.name}
              askOracle={askOracle}
            />
          </>
        ) : (
          <p className="text-[13px] text-cream-400">No resource selected.</p>
        )}
        {detail}
      </section>
    </div>
  );
}

function StorageTwoPane({
  rows,
  selected,
  onSelect,
  tabLabel,
  detail,
}: {
  rows: ScalewayStorageSummary[];
  selected: ScalewayStorageSummary | null;
  onSelect: (id: string) => void;
  tabLabel: string;
  detail: ReactNode;
}) {
  return (
    <div className="grid grid-cols-1 gap-4 xl:grid-cols-[minmax(0,1.1fr)_minmax(320px,0.9fr)] xl:items-start">
      <section className="rounded-2xl border border-cream-200 bg-white p-5 xl:sticky xl:top-0 xl:self-start xl:max-h-[calc(100vh-7rem)] xl:overflow-y-auto">
        <h3 className="mb-3 text-[15px] font-semibold text-cream-900">
          {tabLabel} inventory
        </h3>
        <div className="overflow-hidden rounded-xl border border-cream-100">
          {rows.map((r) => {
            const active = selected?.id === r.id;
            const st =
              stateColors[r.state as keyof typeof stateColors] || stateColors.unknown;
            return (
              <button
                key={r.id}
                type="button"
                onClick={() => onSelect(r.id)}
                className={`grid w-full grid-cols-[minmax(0,1fr)_auto] items-center gap-2 border-b border-cream-50 px-3 py-2 text-left last:border-b-0 ${
                  active ? "bg-terracotta/[0.06]" : "bg-white hover:bg-cream-50"
                }`}
              >
                <div className="min-w-0">
                  <p className="truncate font-mono text-[12px] font-semibold text-cream-800">
                    {r.name}
                  </p>
                  <p className="mt-0.5 truncate text-[11px] text-cream-400">
                    {r.storageType} · {r.region} · {r.pricingLabel}
                  </p>
                </div>
                <span className="inline-flex items-center justify-end gap-1.5">
                  <span className={`w-1.5 h-1.5 rounded-full ${st.dot}`} />
                  <span className={`text-[11px] font-medium ${st.text}`}>
                    {st.label}
                  </span>
                </span>
              </button>
            );
          })}
          {rows.length === 0 && (
            <p className="px-3 py-8 text-center text-[13px] text-cream-400">
              No {tabLabel.toLowerCase()} resources. Sync Scaleway or create one.
            </p>
          )}
        </div>
      </section>

      <section className="space-y-4 rounded-2xl border border-cream-200 bg-white p-5 xl:sticky xl:top-0 xl:self-start xl:max-h-[calc(100vh-7rem)] xl:overflow-y-auto">
        <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          Resource detail
        </h3>
        {selected && (
          <>
            <div>
              <p className="font-mono text-[15px] font-semibold text-cream-900">
                {selected.name}
              </p>
              <p className="mt-1 text-[12px] leading-5 text-cream-500">
                {selected.pricingNote}
              </p>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <Metric label="Type" value={selected.storageType} />
              <Metric label="Region" value={selected.region} />
              <Metric label="Size" value={`${selected.sizeGb} GB`} />
              <Metric
                label="Est. / month"
                value={formatEur(selected.estimatedEurMonth, "")}
              />
            </div>
          </>
        )}
        {detail}
      </section>
    </div>
  );
}

// ===========================================================================
// Storage panels
// ===========================================================================

function ObjectStoragePanel({
  resource,
  pinnedProjectId,
  blockedReason,
  isLoading,
  createScalewayObjectBucket,
  deleteScalewayObjectBucket,
  setScalewayObjectBucketLifecycle,
  onResult,
}: {
  resource: ScalewayStorageSummary | null;
  pinnedProjectId: string;
  blockedReason: string;
  isLoading: boolean;
  createScalewayObjectBucket: (request: {
    name: string;
    region: string;
    projectId: string;
  }) => Promise<{ message: string } | null>;
  deleteScalewayObjectBucket: (
    resourceId: string,
    confirmResourceName?: string | null,
  ) => Promise<{ message: string } | null>;
  setScalewayObjectBucketLifecycle: (
    resourceId: string,
    rules: unknown,
  ) => Promise<{ message: string } | null>;
  onResult: (message: string | null) => void;
}) {
  const canMutate = Boolean(pinnedProjectId);
  const [bucketName, setBucketName] = useState("");
  const [region, setRegion] = useState(resource?.region || "fr-par");
  const [creating, setCreating] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [lifecycleDraft, setLifecycleDraft] = useState('[\n  { "id": "expire-old", "prefix": "", "enabled": true, "expirationDays": 30 }\n]');
  const [lifecycleError, setLifecycleError] = useState<string | null>(null);
  const [savingLifecycle, setSavingLifecycle] = useState(false);
  const requestId = useRef(0);

  useEffect(() => {
    return () => {
      requestId.current += 1;
    };
  }, []);

  const create = async () => {
    if (!canMutate || !bucketName.trim() || !region.trim()) return;
    const id = requestId.current + 1;
    requestId.current = id;
    setCreating(true);
    try {
      const r = await createScalewayObjectBucket({
        name: bucketName.trim(),
        region: region.trim(),
        projectId: pinnedProjectId,
      });
      if (requestId.current !== id) return;
      if (r) {
        onResult(r.message);
        setBucketName("");
      }
    } finally {
      if (requestId.current === id) setCreating(false);
    }
  };

  const remove = async () => {
    if (!resource || !canMutate) return;
    const id = requestId.current + 1;
    requestId.current = id;
    setDeleting(true);
    try {
      const r = await deleteScalewayObjectBucket(resource.id, resource.name);
      if (requestId.current !== id) return;
      if (r) onResult(r.message);
    } finally {
      if (requestId.current === id) setDeleting(false);
    }
  };

  const saveLifecycle = async () => {
    if (!resource || !canMutate) return;
    let parsed: unknown;
    try {
      parsed = JSON.parse(lifecycleDraft);
    } catch {
      setLifecycleError("Draft is not valid JSON. Fix it before saving.");
      return;
    }
    setLifecycleError(null);
    const id = requestId.current + 1;
    requestId.current = id;
    setSavingLifecycle(true);
    try {
      const r = await setScalewayObjectBucketLifecycle(resource.id, parsed);
      if (requestId.current !== id) return;
      if (r) onResult(r.message);
    } finally {
      if (requestId.current === id) setSavingLifecycle(false);
    }
  };

  return (
    <PanelShell
      title="Object Storage"
      subtitle="Create a bucket, edit its expire-by-age lifecycle rules, or delete it. Bucket names must follow S3 rules (lowercase, 3-63 chars, no underscores)."
    >
      {!canMutate && <MutateBlockedNote reason={blockedReason} />}

      <div className="space-y-3">
        <p className="text-[10px] font-semibold uppercase tracking-widest text-cream-400">
          Create bucket
        </p>
        <TextField
          label="Bucket name (S3 lowercase)"
          value={bucketName}
          onChange={(v) => setBucketName(v.toLowerCase())}
          disabled={isLoading}
          placeholder="aspis-bio-data"
          mono
          help="Lowercase letters, digits, and hyphens only; 3-63 characters."
        />
        <TextField
          label="Region"
          value={region}
          onChange={setRegion}
          disabled={isLoading}
          placeholder="fr-par"
          mono
        />
        <button
          type="button"
          onClick={() => void create()}
          disabled={isLoading || creating || !canMutate || !bucketName.trim()}
          title={!canMutate ? blockedReason : undefined}
          className="flex items-center gap-1.5 rounded-xl bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-50"
        >
          <Plus className="h-3.5 w-3.5" />
          {creating ? "Creating..." : "Create bucket"}
        </button>
      </div>

      {resource && (
        <div className="mt-4 space-y-3 border-t border-cream-200 pt-4">
          <p className="text-[10px] font-semibold uppercase tracking-widest text-cream-400">
            Lifecycle rules (expire by age)
          </p>
          <p className="text-[11px] leading-5 text-cream-500">
            Each rule expires objects under a prefix after N days. Edit the JSON and
            save. This overwrites the bucket lifecycle configuration.
          </p>
          <textarea
            value={lifecycleDraft}
            onChange={(event) => {
              setLifecycleDraft(event.target.value);
              setLifecycleError(null);
            }}
            disabled={isLoading || savingLifecycle}
            spellCheck={false}
            rows={6}
            className="w-full rounded-xl border border-cream-200 bg-white px-3 py-2 font-mono text-[11px] text-cream-800 outline-none focus:border-terracotta/30 disabled:opacity-60"
          />
          {lifecycleError && (
            <p className="text-[11px] font-semibold text-coral-dark">{lifecycleError}</p>
          )}
          <button
            type="button"
            onClick={() => void saveLifecycle()}
            disabled={isLoading || savingLifecycle || !canMutate}
            title={!canMutate ? blockedReason : undefined}
            className="flex items-center gap-1.5 rounded-xl bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-50"
          >
            <Save className="h-3.5 w-3.5" />
            {savingLifecycle ? "Saving..." : "Save lifecycle"}
          </button>

          <DeleteConfirm
            resourceName={resource.name}
            label="Delete bucket"
            hint="Type the exact bucket name to confirm. The bucket must be EMPTY — Scaleway refuses to delete a bucket that still has objects."
            canMutate={canMutate}
            isLoading={isLoading}
            busy={deleting}
            onDelete={() => void remove()}
          />
        </div>
      )}

      <div className="mt-4">
        <ConsoleLink family="Object Bucket" />
      </div>
    </PanelShell>
  );
}

function BlockStoragePanel({
  resource,
  pinnedProjectId,
  blockedReason,
  isLoading,
  createScalewayBlockVolume,
  resizeScalewayBlockVolume,
  createScalewayBlockSnapshot,
  deleteScalewayBlockStorage,
  onResult,
}: {
  resource: ScalewayStorageSummary | null;
  pinnedProjectId: string;
  blockedReason: string;
  isLoading: boolean;
  createScalewayBlockVolume: (request: {
    name: string;
    zone: string;
    projectId: string;
    sizeGib: number;
    perfIops: number;
  }) => Promise<{ message: string } | null>;
  resizeScalewayBlockVolume: (
    resourceId: string,
    newSizeGib: number,
  ) => Promise<{ message: string } | null>;
  createScalewayBlockSnapshot: (
    volumeId: string,
    name: string,
  ) => Promise<{ message: string } | null>;
  deleteScalewayBlockStorage: (
    resourceId: string,
    confirmResourceName?: string | null,
  ) => Promise<{ message: string } | null>;
  onResult: (message: string | null) => void;
}) {
  const canMutate = Boolean(pinnedProjectId);
  const isVolume = (resource?.storageType ?? "").startsWith("Block Storage");
  const [name, setName] = useState("");
  const [zone, setZone] = useState(resource?.region || "fr-par-1");
  const [sizeGib, setSizeGib] = useState("10");
  const [iops, setIops] = useState("5000");
  const [resizeGib, setResizeGib] = useState(
    resource ? String(resource.sizeGb) : "",
  );
  const [snapshotName, setSnapshotName] = useState("");
  const [busy, setBusy] = useState<null | "create" | "resize" | "snapshot" | "delete">(
    null,
  );
  const requestId = useRef(0);

  useEffect(() => {
    return () => {
      requestId.current += 1;
    };
  }, []);

  const run = async (
    kind: "create" | "resize" | "snapshot" | "delete",
    fn: () => Promise<{ message: string } | null>,
  ) => {
    const id = requestId.current + 1;
    requestId.current = id;
    setBusy(kind);
    try {
      const r = await fn();
      if (requestId.current !== id) return;
      if (r) onResult(r.message);
    } finally {
      if (requestId.current === id) setBusy(null);
    }
  };

  return (
    <PanelShell
      title="Block Storage"
      subtitle="Create a volume (5K or 15K IOPS), grow an existing volume, snapshot it, or delete it. Volumes cannot be shrunk — the backend refuses a smaller size."
    >
      {!canMutate && <MutateBlockedNote reason={blockedReason} />}

      <div className="space-y-3">
        <p className="text-[10px] font-semibold uppercase tracking-widest text-cream-400">
          Create volume
        </p>
        <TextField
          label="Volume name"
          value={name}
          onChange={setName}
          disabled={isLoading}
          placeholder="data-volume"
          mono
        />
        <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
          <TextField
            label="Zone"
            value={zone}
            onChange={setZone}
            disabled={isLoading}
            placeholder="fr-par-1"
            mono
          />
          <NumberField
            label="Size (GB)"
            value={sizeGib}
            onChange={setSizeGib}
            disabled={isLoading}
            placeholder="10"
          />
          <label className="block">
            <span className="text-[10px] font-semibold uppercase tracking-widest text-cream-400">
              IOPS class
            </span>
            <select
              value={iops}
              onChange={(event) => setIops(event.target.value)}
              disabled={isLoading}
              className="mt-1 w-full rounded-xl border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] text-cream-800 outline-none focus:border-terracotta/30 disabled:opacity-60"
            >
              <option value="5000">5000 (Block Storage 5K)</option>
              <option value="15000">15000 (Block Storage 15K)</option>
            </select>
          </label>
        </div>
        <button
          type="button"
          onClick={() =>
            void run("create", () =>
              createScalewayBlockVolume({
                name: name.trim(),
                zone: zone.trim(),
                projectId: pinnedProjectId,
                sizeGib: Number(sizeGib),
                perfIops: Number(iops),
              }),
            )
          }
          disabled={
            isLoading ||
            busy !== null ||
            !canMutate ||
            !name.trim() ||
            !zone.trim() ||
            !sizeGib
          }
          title={!canMutate ? blockedReason : undefined}
          className="flex items-center gap-1.5 rounded-xl bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-50"
        >
          <Plus className="h-3.5 w-3.5" />
          {busy === "create" ? "Creating..." : "Create volume"}
        </button>
      </div>

      {resource && isVolume && (
        <div className="mt-4 space-y-3 border-t border-cream-200 pt-4">
          <p className="text-[10px] font-semibold uppercase tracking-widest text-cream-400">
            Resize volume (grow only)
          </p>
          <p className="text-[11px] leading-5 text-cream-500">
            Current size: {resource.sizeGb} GB. The new size must be greater than or
            equal to the current size; shrinking is rejected.
          </p>
          <div className="flex items-end gap-2">
            <div className="flex-1">
              <NumberField
                label="New size (GB)"
                value={resizeGib}
                onChange={setResizeGib}
                disabled={isLoading}
                placeholder={String(resource.sizeGb)}
              />
            </div>
            <button
              type="button"
              onClick={() =>
                void run("resize", () =>
                  resizeScalewayBlockVolume(resource.id, Number(resizeGib)),
                )
              }
              disabled={
                isLoading ||
                busy !== null ||
                !canMutate ||
                !resizeGib ||
                Number(resizeGib) < resource.sizeGb
              }
              title={
                !canMutate
                  ? blockedReason
                  : Number(resizeGib) < resource.sizeGb
                    ? "New size cannot be smaller than current size."
                    : undefined
              }
              className="flex items-center gap-1.5 rounded-xl bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-50"
            >
              {busy === "resize" ? "Resizing..." : "Resize"}
            </button>
          </div>

          <p className="mt-2 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
            Snapshot volume
          </p>
          <div className="flex items-end gap-2">
            <div className="flex-1">
              <TextField
                label="Snapshot name"
                value={snapshotName}
                onChange={setSnapshotName}
                disabled={isLoading}
                placeholder="snap-2026-06"
                mono
              />
            </div>
            <button
              type="button"
              onClick={() =>
                void run("snapshot", () =>
                  createScalewayBlockSnapshot(resource.id, snapshotName.trim()),
                )
              }
              disabled={
                isLoading || busy !== null || !canMutate || !snapshotName.trim()
              }
              title={!canMutate ? blockedReason : undefined}
              className="flex items-center gap-1.5 rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:border-terracotta-200 hover:text-terracotta disabled:opacity-50"
            >
              {busy === "snapshot" ? "Snapshotting..." : "Snapshot"}
            </button>
          </div>
        </div>
      )}

      {resource && (
        <div className="mt-4">
          <DeleteConfirm
            resourceName={resource.name}
            label={`Delete ${resource.storageType === "Block Snapshot" ? "snapshot" : "volume"}`}
            hint="Type the exact resource name to confirm. A volume still attached to an Instance cannot be deleted — detach it first."
            canMutate={canMutate}
            isLoading={isLoading}
            busy={busy === "delete"}
            onDelete={() =>
              void run("delete", () =>
                deleteScalewayBlockStorage(resource.id, resource.name),
              )
            }
          />
        </div>
      )}

      <div className="mt-4">
        <ConsoleLink family="Block Storage 5K" />
      </div>
    </PanelShell>
  );
}

function FileStoragePanel({
  resource,
  pinnedProjectId,
  blockedReason,
  isLoading,
  createScalewayFilesystem,
  deleteScalewayFilesystem,
  onResult,
}: {
  resource: ScalewayStorageSummary | null;
  pinnedProjectId: string;
  blockedReason: string;
  isLoading: boolean;
  createScalewayFilesystem: (request: {
    name: string;
    region: string;
    projectId: string;
    sizeGib: number;
  }) => Promise<{ message: string } | null>;
  deleteScalewayFilesystem: (
    resourceId: string,
    confirmResourceName?: string | null,
  ) => Promise<{ message: string } | null>;
  onResult: (message: string | null) => void;
}) {
  const canMutate = Boolean(pinnedProjectId);
  const [name, setName] = useState("");
  const [region, setRegion] = useState(resource?.region || "fr-par");
  const [sizeGib, setSizeGib] = useState("100");
  const [busy, setBusy] = useState<null | "create" | "delete">(null);
  const requestId = useRef(0);

  useEffect(() => {
    return () => {
      requestId.current += 1;
    };
  }, []);

  const run = async (
    kind: "create" | "delete",
    fn: () => Promise<{ message: string } | null>,
  ) => {
    const id = requestId.current + 1;
    requestId.current = id;
    setBusy(kind);
    try {
      const r = await fn();
      if (requestId.current !== id) return;
      if (r) onResult(r.message);
    } finally {
      if (requestId.current === id) setBusy(null);
    }
  };

  return (
    <PanelShell
      title="File Storage"
      subtitle="Create a managed filesystem (region-scoped) or delete one."
    >
      {!canMutate && <MutateBlockedNote reason={blockedReason} />}

      <div className="space-y-3">
        <p className="text-[10px] font-semibold uppercase tracking-widest text-cream-400">
          Create filesystem
        </p>
        <TextField
          label="Filesystem name"
          value={name}
          onChange={setName}
          disabled={isLoading}
          placeholder="shared-fs"
          mono
        />
        <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
          <TextField
            label="Region"
            value={region}
            onChange={setRegion}
            disabled={isLoading}
            placeholder="fr-par"
            mono
          />
          <NumberField
            label="Size (GB)"
            value={sizeGib}
            onChange={setSizeGib}
            disabled={isLoading}
            placeholder="100"
          />
        </div>
        <button
          type="button"
          onClick={() =>
            void run("create", () =>
              createScalewayFilesystem({
                name: name.trim(),
                region: region.trim(),
                projectId: pinnedProjectId,
                sizeGib: Number(sizeGib),
              }),
            )
          }
          disabled={
            isLoading ||
            busy !== null ||
            !canMutate ||
            !name.trim() ||
            !region.trim() ||
            !sizeGib
          }
          title={!canMutate ? blockedReason : undefined}
          className="flex items-center gap-1.5 rounded-xl bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-50"
        >
          <Plus className="h-3.5 w-3.5" />
          {busy === "create" ? "Creating..." : "Create filesystem"}
        </button>
      </div>

      {resource && (
        <div className="mt-4">
          <DeleteConfirm
            resourceName={resource.name}
            label="Delete filesystem"
            hint="Type the exact filesystem name to confirm. This permanently removes the filesystem and its data."
            canMutate={canMutate}
            isLoading={isLoading}
            busy={busy === "delete"}
            onDelete={() =>
              void run("delete", () =>
                deleteScalewayFilesystem(resource.id, resource.name),
              )
            }
          />
        </div>
      )}

      <div className="mt-4">
        <ConsoleLink family="File System" />
      </div>
    </PanelShell>
  );
}

// ===========================================================================
// Serverless SQL panel
// ===========================================================================

function SqlPanel({
  resource,
  pinnedProjectId,
  blockedReason,
  isLoading,
  createScalewaySqlDatabase,
  deleteScalewaySqlDatabase,
  onResult,
}: {
  resource: ScalewayResourceSummary | null;
  pinnedProjectId: string;
  blockedReason: string;
  isLoading: boolean;
  createScalewaySqlDatabase: (request: {
    name: string;
    region: string;
    projectId: string;
    cpuMin: number;
    cpuMax: number;
  }) => Promise<{ message: string } | null>;
  deleteScalewaySqlDatabase: (
    resourceId: string,
    confirmResourceName?: string | null,
  ) => Promise<{ message: string } | null>;
  onResult: (message: string | null) => void;
}) {
  const canMutate = Boolean(pinnedProjectId);
  const [name, setName] = useState("");
  const [region, setRegion] = useState(resource?.region || "fr-par");
  const [cpuMin, setCpuMin] = useState("0");
  const [cpuMax, setCpuMax] = useState("8");
  const [busy, setBusy] = useState<null | "create" | "delete">(null);
  const requestId = useRef(0);

  useEffect(() => {
    return () => {
      requestId.current += 1;
    };
  }, []);

  const run = async (
    kind: "create" | "delete",
    fn: () => Promise<{ message: string } | null>,
  ) => {
    const id = requestId.current + 1;
    requestId.current = id;
    setBusy(kind);
    try {
      const r = await fn();
      if (requestId.current !== id) return;
      if (r) onResult(r.message);
    } finally {
      if (requestId.current === id) setBusy(null);
    }
  };

  return (
    <PanelShell
      title="Serverless SQL"
      subtitle="Create an autoscaling PostgreSQL database or delete one. There is no in-app query — the endpoint is a raw psql DSN; connect with a Postgres client."
    >
      {!canMutate && <MutateBlockedNote reason={blockedReason} />}

      {resource?.endpoint && (
        <div className="mb-4 rounded-xl border border-cream-100 bg-white p-3">
          <p className="text-[10px] font-semibold uppercase tracking-widest text-cream-400">
            Connection endpoint (DSN)
          </p>
          <p
            className="mt-1 break-all rounded-lg bg-cream-50 px-3 py-2 font-mono text-[11px] text-cream-700"
            data-help-title="This is the raw PostgreSQL DSN for this Serverless SQL database."
            data-help-lines="Connect with a Postgres client such as psql; the app does not run queries here.|The DSN identifies the database host and name; any password is redacted here.|Treat it as sensitive — do not paste it into shared logs.|Use the Scaleway console to rotate credentials."
          >
            {redactDsnPassword(resource.endpoint)}
          </p>
          <p className="mt-2 text-[11px] leading-5 text-cream-500">
            Connect with{" "}
            <code className="rounded bg-cream-50 px-1 py-0.5 font-mono text-[10px] text-cream-700">
              psql "&lt;dsn&gt;"
            </code>{" "}
            using your database credentials.
          </p>
          <div className="mt-2">
            <ConsoleLink family="Serverless SQL" />
          </div>
        </div>
      )}

      <div className="space-y-3">
        <p className="text-[10px] font-semibold uppercase tracking-widest text-cream-400">
          Create database
        </p>
        <TextField
          label="Database name"
          value={name}
          onChange={setName}
          disabled={isLoading}
          placeholder="aspis-db"
          mono
        />
        <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
          <TextField
            label="Region"
            value={region}
            onChange={setRegion}
            disabled={isLoading}
            placeholder="fr-par"
            mono
          />
          <NumberField
            label="CPU min"
            value={cpuMin}
            onChange={setCpuMin}
            disabled={isLoading}
            placeholder="0"
          />
          <NumberField
            label="CPU max"
            value={cpuMax}
            onChange={setCpuMax}
            disabled={isLoading}
            placeholder="8"
          />
        </div>
        <button
          type="button"
          onClick={() =>
            void run("create", () =>
              createScalewaySqlDatabase({
                name: name.trim(),
                region: region.trim(),
                projectId: pinnedProjectId,
                cpuMin: Number(cpuMin),
                cpuMax: Number(cpuMax),
              }),
            )
          }
          disabled={
            isLoading ||
            busy !== null ||
            !canMutate ||
            !name.trim() ||
            !region.trim() ||
            !cpuMax ||
            Number(cpuMax) === 0 ||
            Number(cpuMin) > Number(cpuMax)
          }
          title={!canMutate ? blockedReason : undefined}
          className="flex items-center gap-1.5 rounded-xl bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-50"
        >
          <Plus className="h-3.5 w-3.5" />
          {busy === "create" ? "Creating..." : "Create database"}
        </button>
      </div>

      {resource && resource.resourceType === "Serverless SQL" && (
        <div className="mt-4">
          <DeleteConfirm
            resourceName={resource.name}
            label="Delete database"
            hint="Type the exact database name to confirm. This permanently removes the database and its data."
            canMutate={canMutate}
            isLoading={isLoading}
            busy={busy === "delete"}
            onDelete={() =>
              void run("delete", () =>
                deleteScalewaySqlDatabase(resource.id, resource.name),
              )
            }
          />
        </div>
      )}
    </PanelShell>
  );
}

// ===========================================================================
// Serverless Functions / Containers panel
// ===========================================================================

function ServerlessPanel({
  resource,
  kind,
  pinnedProjectId,
  blockedReason,
  isLoading,
  createScalewayFunction,
  createScalewayContainer,
  deleteScalewayFunction,
  deleteScalewayContainer,
  onDeploy,
  onResult,
}: {
  resource: ScalewayResourceSummary;
  kind: "function" | "container";
  pinnedProjectId: string;
  blockedReason: string;
  isLoading: boolean;
  createScalewayFunction: (request: {
    name: string;
    region: string;
    projectId: string;
    namespaceId?: string;
    namespaceName?: string;
    runtime: string;
    memoryLimit?: number;
    minScale?: number;
    maxScale?: number;
  }) => Promise<{ message: string } | null>;
  createScalewayContainer: (request: {
    name: string;
    region: string;
    projectId: string;
    namespaceId?: string;
    namespaceName?: string;
    registryImage: string;
    memoryLimit?: number;
    minScale?: number;
    maxScale?: number;
  }) => Promise<{ message: string } | null>;
  deleteScalewayFunction: (
    resourceId: string,
    confirmResourceName?: string | null,
  ) => Promise<{ message: string } | null>;
  deleteScalewayContainer: (
    resourceId: string,
    confirmResourceName?: string | null,
  ) => Promise<{ message: string } | null>;
  onDeploy: (resource: ScalewayResourceSummary) => void;
  onResult: (message: string | null) => void;
}) {
  const canMutate = Boolean(pinnedProjectId);
  const [name, setName] = useState("");
  const [region, setRegion] = useState(resource.region || "fr-par");
  const [namespaceName, setNamespaceName] = useState("");
  const [runtime, setRuntime] = useState("");
  const [registryImage, setRegistryImage] = useState("");
  const [memory, setMemory] = useState("");
  const [minScale, setMinScale] = useState("");
  const [maxScale, setMaxScale] = useState("");
  const [busy, setBusy] = useState<null | "create" | "delete">(null);
  const requestId = useRef(0);

  useEffect(() => {
    return () => {
      requestId.current += 1;
    };
  }, []);

  const run = async (
    state: "create" | "delete",
    fn: () => Promise<{ message: string } | null>,
  ) => {
    const id = requestId.current + 1;
    requestId.current = id;
    setBusy(state);
    try {
      const r = await fn();
      if (requestId.current !== id) return;
      if (r) onResult(r.message);
    } finally {
      if (requestId.current === id) setBusy(null);
    }
  };

  const optNum = (raw: string): number | undefined => {
    const n = Number(raw.trim());
    return raw.trim() && Number.isFinite(n) ? n : undefined;
  };

  const create = () => {
    if (kind === "container") {
      return run("create", () =>
        createScalewayContainer({
          name: name.trim(),
          region: region.trim(),
          projectId: pinnedProjectId,
          namespaceName: namespaceName.trim() || undefined,
          registryImage: registryImage.trim(),
          memoryLimit: optNum(memory),
          minScale: optNum(minScale),
          maxScale: optNum(maxScale),
        }),
      );
    }
    return run("create", () =>
      createScalewayFunction({
        name: name.trim(),
        region: region.trim(),
        projectId: pinnedProjectId,
        namespaceName: namespaceName.trim() || undefined,
        runtime: runtime.trim(),
        memoryLimit: optNum(memory),
        minScale: optNum(minScale),
        maxScale: optNum(maxScale),
      }),
    );
  };

  const createDisabled =
    isLoading ||
    busy !== null ||
    !canMutate ||
    !name.trim() ||
    !region.trim() ||
    (kind === "function" ? !runtime.trim() : !registryImage.trim());

  return (
    <PanelShell
      title={kind === "container" ? "Serverless Container" : "Serverless Function"}
      subtitle={
        kind === "container"
          ? "Create a container from an existing registry image, deploy it, or delete it. The image is referenced, not built."
          : "Create a function resource, deploy it, or delete it. Uploading the function code is a separate deploy step."
      }
    >
      {!canMutate && <MutateBlockedNote reason={blockedReason} />}

      {/* Deploy reuses the existing lifecycle action (performScalewayResourceAction)
          via the page-level confirm dialog. */}
      {resource.availableActions.includes("deploy") && (
        <div className="mb-4">
          <p className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
            Deploy
          </p>
          <p className="mb-2 text-[11px] leading-5 text-cream-500">
            Deploy redeploys this {kind} from its configured source/image.
          </p>
          <button
            type="button"
            onClick={() => onDeploy(resource)}
            disabled={isLoading || busy !== null}
            data-help-title={`Deploy redeploys the ${kind}.`}
            data-help-lines="Deploy is a non-destructive lifecycle action.|It redeploys the configured source or image for this resource.|It runs through the standard Scaleway confirmation dialog.|Verifier agents should only read this state."
            className="flex items-center gap-1.5 rounded-xl border border-sage/20 px-3 py-2 text-[12px] font-semibold text-sage-dark hover:bg-sage/10 disabled:opacity-50"
          >
            <Rocket className="h-3.5 w-3.5" />
            Deploy {kind}
          </button>
        </div>
      )}

      <div className="space-y-3">
        <p className="text-[10px] font-semibold uppercase tracking-widest text-cream-400">
          Create {kind}
        </p>
        <TextField
          label={`${kind === "container" ? "Container" : "Function"} name`}
          value={name}
          onChange={setName}
          disabled={isLoading}
          placeholder={kind === "container" ? "my-container" : "my-function"}
          mono
        />
        <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
          <TextField
            label="Region"
            value={region}
            onChange={setRegion}
            disabled={isLoading}
            placeholder="fr-par"
            mono
          />
          <TextField
            label="Namespace name (optional)"
            value={namespaceName}
            onChange={setNamespaceName}
            disabled={isLoading}
            placeholder="created if absent"
            mono
            help="If omitted, a namespace is created from the resource name."
          />
        </div>
        {kind === "function" ? (
          <TextField
            label="Runtime"
            value={runtime}
            onChange={setRuntime}
            disabled={isLoading}
            placeholder="node20"
            mono
            help="A supported Scaleway language runtime, e.g. node20, python311."
          />
        ) : (
          <TextField
            label="Registry image"
            value={registryImage}
            onChange={setRegistryImage}
            disabled={isLoading}
            placeholder="rg.fr-par.scw.cloud/ns/image:tag"
            mono
            help="An existing image in your Scaleway container registry."
          />
        )}
        <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
          <NumberField
            label="Memory (MB)"
            value={memory}
            onChange={setMemory}
            disabled={isLoading}
            placeholder="optional"
          />
          <NumberField
            label="Min scale"
            value={minScale}
            onChange={setMinScale}
            disabled={isLoading}
            placeholder="optional"
          />
          <NumberField
            label="Max scale"
            value={maxScale}
            onChange={setMaxScale}
            disabled={isLoading}
            placeholder="optional"
          />
        </div>
        <button
          type="button"
          onClick={() => void create()}
          disabled={createDisabled}
          title={!canMutate ? blockedReason : undefined}
          className="flex items-center gap-1.5 rounded-xl bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-50"
        >
          <Plus className="h-3.5 w-3.5" />
          {busy === "create" ? "Creating..." : `Create ${kind}`}
        </button>
      </div>

      <div className="mt-4">
        <DeleteConfirm
          resourceName={resource.name}
          label={`Delete ${kind}`}
          hint={`Type the exact ${kind} name to confirm. This permanently removes the ${kind}.`}
          canMutate={canMutate}
          isLoading={isLoading}
          busy={busy === "delete"}
          onDelete={() =>
            void run("delete", () =>
              kind === "container"
                ? deleteScalewayContainer(resource.id, resource.name)
                : deleteScalewayFunction(resource.id, resource.name),
            )
          }
        />
      </div>

      <div className="mt-4">
        <ConsoleLink family="Serverless" />
      </div>
    </PanelShell>
  );
}

// ===========================================================================
// Generic inspect / inventory + Billing
// ===========================================================================

function GenericInspectPanel({
  title,
  subtitle,
  family,
}: {
  title: string;
  subtitle: string;
  family: string;
}) {
  return (
    <PanelShell title={title} subtitle={subtitle}>
      <ConsoleLink family={family} />
    </PanelShell>
  );
}

function GenericInventory({
  rows,
}: {
  rows: {
    id: string;
    name: string;
    family: string;
    region: string;
    state: string;
    domain: "compute" | "storage";
  }[];
}) {
  const [search, setSearch] = useState("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return rows;
    return rows.filter((r) =>
      [r.name, r.family, r.region, r.state].join(" ").toLowerCase().includes(q),
    );
  }, [rows, search]);
  const selected =
    filtered.find((r) => r.id === selectedId) ?? filtered[0] ?? null;

  return (
    <div className="grid grid-cols-1 gap-4 xl:grid-cols-[minmax(0,1.35fr)_minmax(320px,0.65fr)]">
      <section className="rounded-2xl border border-cream-200 bg-white p-5">
        <div className="mb-4 flex flex-col gap-3 lg:flex-row lg:items-end lg:justify-between">
          <div>
            <h3 className="text-[15px] font-semibold text-cream-900">
              Generic inventory
            </h3>
            <p className="mt-1 text-[12px] text-cream-500">
              Scaleway resource families without a dedicated tab. Inspect-only.
            </p>
          </div>
          <label className="flex items-center gap-2 rounded-xl border border-cream-200 bg-white px-3 py-2 sm:w-72">
            <Search className="h-3.5 w-3.5 shrink-0 text-cream-400" />
            <input
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder="Search resources"
              className="min-w-0 flex-1 bg-transparent text-[12px] font-medium text-cream-800 outline-none placeholder:text-cream-400"
            />
          </label>
        </div>
        <div className="overflow-hidden rounded-xl border border-cream-100">
          {filtered.map((r) => {
            const active = selected?.id === r.id;
            return (
              <button
                key={r.id}
                type="button"
                onClick={() => setSelectedId(r.id)}
                className={`grid w-full grid-cols-[minmax(0,1.25fr)_130px_minmax(0,0.7fr)] items-center gap-1 border-b border-cream-50 px-3 py-2 text-left last:border-b-0 ${
                  active ? "bg-terracotta/[0.06]" : "bg-white hover:bg-cream-50"
                }`}
              >
                <p className="truncate text-[12px] font-semibold text-cream-800">
                  {r.name}
                </p>
                <span className="w-fit rounded-lg bg-cream-50 px-2 py-1 text-[10px] font-semibold text-cream-500">
                  {r.family}
                </span>
                <p className="truncate text-right text-[11px] font-semibold text-cream-700">
                  {r.state} · {r.region}
                </p>
              </button>
            );
          })}
          {filtered.length === 0 && (
            <p className="px-3 py-8 text-center text-[13px] text-cream-400">
              No uncovered resource families. Sync Scaleway or clear the search.
            </p>
          )}
        </div>
      </section>

      <section className="rounded-2xl border border-cream-200 bg-white p-5">
        <h3 className="mb-3 text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          Resource detail
        </h3>
        {selected ? (
          <div className="space-y-4">
            <div>
              <p className="font-mono text-[15px] font-semibold text-cream-900">
                {selected.name}
              </p>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <Metric label="Family" value={selected.family} />
              <Metric label="Region" value={selected.region} />
              <Metric label="State" value={selected.state} />
              <Metric label="Domain" value={selected.domain} />
            </div>
            <ConsoleLink family={selected.family} />
          </div>
        ) : (
          <p className="text-[13px] text-cream-400">Select a resource.</p>
        )}
      </section>
    </div>
  );
}

function BillingView({
  billing,
  loading,
}: {
  billing: ScalewayBilling | null;
  loading: boolean;
}) {
  return (
    <section className="rounded-2xl border border-cream-200 bg-white p-5">
      <div className="mb-4 flex items-center gap-2">
        <CreditCard className="h-4 w-4 text-terracotta" />
        <h3 className="text-[15px] font-semibold text-cream-900">Account billing</h3>
      </div>
      <p
        className="mb-4 rounded-xl bg-cream-50 px-3 py-2 text-[11px] leading-5 text-cream-500"
        data-help-title="Scaleway billing is account-level consumption and invoices."
        data-help-lines="This shows live untaxed consumption by category plus issued invoices.|Costs here are real euro amounts billed by Scaleway.|Use it to confirm spend before and after running GPU/CPU jobs.|Re-open the tab to refresh after a sync."
      >
        Live untaxed consumption and invoices for the Scaleway account.
      </p>

      {loading ? (
        <p className="text-[13px] text-cream-400">Loading billing...</p>
      ) : !billing ? (
        <p className="text-[13px] text-cream-400">
          Billing could not be loaded. Sync Scaleway and reopen this tab.
        </p>
      ) : !billing.readable ? (
        <p className="rounded-xl bg-amber/[0.08] px-3 py-2 text-[12px] font-semibold text-amber-dark">
          {billing.message || "Billing is not readable with the current token scope."}
        </p>
      ) : (
        <div className="space-y-5">
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
            <Metric
              label="Total untaxed"
              value={formatEur(billing.totalUntaxed, "")}
            />
            <Metric
              label="Total discount"
              value={formatEur(billing.totalDiscount, "")}
            />
            <Metric label="Updated" value={billing.updatedAt || "unknown"} />
          </div>

          <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
            <div>
              <p className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
                Consumptions
              </p>
              <div className="space-y-2">
                {billing.consumptions.map((c, index) => (
                  <div
                    key={`${c.category ?? "cat"}-${index}`}
                    className="flex items-center justify-between gap-2 rounded-xl bg-cream-50 px-3 py-3"
                  >
                    <p className="truncate text-[12px] font-semibold text-cream-800">
                      {c.category || "Uncategorized"}
                    </p>
                    <span className="shrink-0 rounded-lg bg-white px-2 py-0.5 text-[11px] font-semibold text-cream-600">
                      {c.valueUntaxed != null
                        ? `${c.valueUntaxed} ${c.currency ?? ""}`
                        : "—"}
                    </span>
                  </div>
                ))}
                {billing.consumptions.length === 0 && (
                  <p className="rounded-xl bg-cream-50 px-3 py-2 text-[11px] text-cream-400">
                    No consumption reported.
                  </p>
                )}
              </div>
            </div>

            <div>
              <p className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
                Invoices
              </p>
              <div className="space-y-2">
                {billing.invoices.map((inv, index) => (
                  <div
                    key={inv.id ?? index}
                    className="rounded-xl bg-cream-50 px-3 py-3"
                  >
                    <div className="flex items-center justify-between gap-2">
                      <p className="truncate text-[12px] font-semibold text-cream-800">
                        {inv.issuedAt || inv.startDate || "unknown date"}
                      </p>
                      <span className="shrink-0 rounded-lg bg-white px-2 py-0.5 text-[11px] font-semibold text-cream-600">
                        {inv.totalTaxed != null
                          ? `${inv.totalTaxed} ${inv.currency ?? ""}`
                          : inv.totalUntaxed != null
                            ? `${inv.totalUntaxed} ${inv.currency ?? ""}`
                            : "—"}
                      </span>
                    </div>
                    <p className="mt-1 text-[11px] text-cream-500">
                      {[
                        inv.startDate && inv.stopDate
                          ? `${inv.startDate} → ${inv.stopDate}`
                          : null,
                        inv.state,
                      ]
                        .filter(Boolean)
                        .join(" · ") || "—"}
                    </p>
                  </div>
                ))}
                {billing.invoices.length === 0 && (
                  <p className="rounded-xl bg-cream-50 px-3 py-2 text-[11px] text-cream-400">
                    No invoices reported.
                  </p>
                )}
              </div>
            </div>
          </div>
        </div>
      )}
    </section>
  );
}
