import {
  AlertTriangle,
  Cloud,
  Server,
  Globe,
  ExternalLink,
  RefreshCw,
  Users,
  Key,
  Database,
  CheckCircle2,
  ShieldCheck,
  Boxes,
  Network,
  Layers3,
  Cpu,
  Bot,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useState } from "react";
import type { AppConfig } from "../../types/config";
import { useAppActions, useAppContext } from "../../context/AppContext";
import { safeOpenExternal } from "../../utils/safeOpenExternal";
import { ALPHA_HIDDEN_PROVIDERS } from "../../lib/alphaHidden";
import { CloudflareView } from "./CloudflareView";
import { ComputeView } from "./ComputeView";
import { BudgetView } from "./BudgetView";
import type {
  ProviderConsoleResourceSummary,
  ProviderHealth,
  ProviderId,
  ProviderServiceSummary,
  ProviderScopeSelection,
} from "../../types/backend";

interface ProvidersViewProps {
  config: AppConfig;
}

const iconMap: Record<string, LucideIcon> = {
  Cloud,
  Server,
  Globe,
};

const serviceIconMap: Record<string, LucideIcon> = {
  Account: ShieldCheck,
  "Developer platform": Layers3,
  "Storage & data": Database,
  "Security & network": Network,
  "AI & observability": Bot,
  Identity: ShieldCheck,
  Compute: Cpu,
  "Compute catalog": Boxes,
  Serverless: Cloud,
  Storage: Database,
  "Network & security": Network,
  "Managed data": Database,
};

const providerMeta: Record<ProviderId, { description: string; icon: string; url: string }> = {
  cloudflare: {
    description: "Cloudflare account, Workers inventory, deployment health, and Worker secret rotation.",
    icon: "Cloud",
    url: "https://dash.cloudflare.com",
  },
  scaleway: {
    description: "Scaleway Devboule project inventory for Instance CPU/GPU and serverless workloads.",
    icon: "Server",
    url: "https://console.scaleway.com",
  },
};

const healthColors = {
  healthy: { dot: "bg-sage", text: "text-sage-dark", label: "Healthy" },
  degraded: { dot: "bg-amber", text: "text-amber-dark", label: "Degraded" },
  down: { dot: "bg-coral", text: "text-coral-dark", label: "Down" },
  error: { dot: "bg-coral", text: "text-coral-dark", label: "Error" },
  missing_token: { dot: "bg-coral", text: "text-coral-dark", label: "Missing Token" },
};

const tokenColors = {
  valid: { bg: "bg-sage/10", text: "text-sage-dark", label: "Valid" },
  expiring: { bg: "bg-amber/10", text: "text-amber-dark", label: "Expiring" },
  expired: { bg: "bg-coral/10", text: "text-coral-dark", label: "Expired" },
  unknown: { bg: "bg-cream-100", text: "text-cream-500", label: "Unknown" },
  missing: { bg: "bg-coral/10", text: "text-coral-dark", label: "Missing" },
  invalid: { bg: "bg-coral/10", text: "text-coral-dark", label: "Invalid" },
  insufficient_scope: { bg: "bg-coral/10", text: "text-coral-dark", label: "Scope" },
  valid_read_only: { bg: "bg-amber/10", text: "text-amber-dark", label: "Read-only" },
  valid_unverified: { bg: "bg-amber/10", text: "text-amber-dark", label: "Unverified" },
};

const serviceStatusColors = {
  live: { bg: "bg-sage/10", text: "text-sage-dark", label: "Live" },
  partial: { bg: "bg-amber/10", text: "text-amber-dark", label: "Partial" },
  ready: { bg: "bg-teal/10", text: "text-teal", label: "Ready" },
  roadmap: { bg: "bg-cream-100", text: "text-cream-500", label: "Roadmap" },
  blocked: { bg: "bg-coral/10", text: "text-coral-dark", label: "Blocked" },
  unknown: { bg: "bg-cream-100", text: "text-cream-500", label: "Unknown" },
};

function serviceIcon(service: ProviderServiceSummary) {
  return serviceIconMap[service.category] || Globe;
}

function resourceTone(resource: ProviderConsoleResourceSummary) {
  const status = resource.status.toLowerCase();
  if (status.includes("running") || status.includes("available") || status.includes("healthy")) {
    return "bg-sage/10 text-sage-dark";
  }
  if (status.includes("error") || status.includes("blocked") || status.includes("failed")) {
    return "bg-coral/10 text-coral-dark";
  }
  return "bg-cream-100 text-cream-500";
}

function isProviderLive(status: string) {
  return status === "healthy" || status === "degraded";
}

function canReadProvider(tokenHealth: string) {
  return tokenHealth === "valid" || tokenHealth === "valid_read_only" || tokenHealth === "valid_unverified";
}

function readinessCopy(
  provider: ProviderId,
  health: ProviderHealth | undefined,
  scope: ProviderScopeSelection | undefined,
  resourceCount: number,
) {
  const hasLiveRead = !!health && isProviderLive(health.status) && canReadProvider(health.tokenHealth);
  const canMutate =
    !!health &&
    isProviderLive(health.status) &&
    health.tokenHealth === "valid" &&
    (provider === "cloudflare" || resourceCount > 0);
  const scopeLabel = scope?.name || scope?.id || "not pinned";
  const tokenLabel =
    tokenColors[health?.tokenHealth as keyof typeof tokenColors]?.label ?? "Unknown";
  const credentialLabel = credentialKindLabel(health?.credentialKind ?? null);

  if (!health) {
    return {
      tone: "blocked",
      title: "Not synced",
      detail: "No live provider snapshot yet.",
      read: "Blocked",
      write: "Blocked",
      scope: scopeLabel,
      token: tokenLabel,
      credential: credentialLabel,
    };
  }

  if (!hasLiveRead) {
    return {
      tone: "blocked",
      title: "Needs credentials",
      detail: health.message || "Save a valid token and sync inventory.",
      read: "Blocked",
      write: "Blocked",
      scope: scopeLabel,
      token: tokenLabel,
      credential: credentialLabel,
    };
  }

  if (!canMutate) {
    const detail =
      provider === "cloudflare"
        ? "Inventory is available, but secret rotation stays locked until a verified custom API token has write scope."
        : "Inventory is available, but VM actions need a valid Devboule project token and live resources.";
    return {
      tone: "limited",
      title: "Read-only ready",
      detail,
      read: "Ready",
      write: "Locked",
      scope: scopeLabel,
      token: tokenLabel,
      credential: credentialLabel,
    };
  }

  return {
    tone: "ready",
    title: provider === "cloudflare" ? "Inventory and write guard ready" : "Inventory and VM actions ready",
    detail:
      provider === "cloudflare"
        ? "Devboule worker inventory is filtered; mutation stays behind explicit guarded actions."
        : "Devboule project inventory can run guarded start, stop, reboot and delete actions.",
    read: "Ready",
    write: "Ready",
    scope: scopeLabel,
    token: tokenLabel,
    credential: credentialLabel,
  };
}

function credentialKindLabel(kind: string | null) {
  switch (kind) {
    case "cloudflare_account_owned_token":
      return "Account-owned";
    case "cloudflare_profile_token":
      return "Profile";
    case "cloudflare_unverified_policy_token":
      return "Policy unknown";
    case "scaleway_project_api_token":
      return "Project token";
    case "scaleway_object_storage":
      return "S3 keys";
    default:
      return "Unknown";
  }
}

const readinessTone = {
  ready: {
    border: "border-sage/20",
    bg: "bg-sage/[0.04]",
    icon: "text-sage-dark",
    badge: "bg-sage/10 text-sage-dark",
  },
  limited: {
    border: "border-amber/25",
    bg: "bg-amber/[0.05]",
    icon: "text-amber-dark",
    badge: "bg-amber/10 text-amber-dark",
  },
  blocked: {
    border: "border-coral/20",
    bg: "bg-coral/[0.04]",
    icon: "text-coral-dark",
    badge: "bg-coral/10 text-coral-dark",
  },
};

type ProvidersTabId = "overview" | "cloudflare" | "scaleway" | "budget";

const PROVIDER_TABS: { id: ProvidersTabId; label: string }[] = [
  { id: "overview", label: "Overview" },
  { id: "cloudflare", label: "Cloudflare" },
  { id: "scaleway", label: "Scaleway / Compute" },
  { id: "budget", label: "Budget" },
];

// Alpha: drop any tabs explicitly hidden from the UI (reversible via the set).
const VISIBLE_PROVIDER_TABS = PROVIDER_TABS.filter(
  (t) => !ALPHA_HIDDEN_PROVIDERS.has(t.id),
);

// Alpha: the overview tab's readiness cards, console-map sub-tabs and the
// selected console provider all hard-code the cloudflare/scaleway pair. Drop
// any provider explicitly hidden from the UI here too (reversible by clearing
// ALPHA_HIDDEN_PROVIDERS). When this ends up empty, the console-map + live
// inventory sections are hidden entirely and the readiness grid renders nothing.
const VISIBLE_CONSOLE_PROVIDERS = (["cloudflare", "scaleway"] as ProviderId[]).filter(
  (id) => !ALPHA_HIDDEN_PROVIDERS.has(id),
);

export function ProvidersView({ config }: ProvidersViewProps) {
  const { cloudSnapshot, syncProviderInventory, isLoading, pendingTab } =
    useAppContext();
  const { consumePendingTab } = useAppActions();
  const [tab, setTab] = useState<ProvidersTabId>("overview");

  // Fallback: if the active tab is (or becomes) hidden, land on the first
  // visible tab so ProvidersView never renders a blank/hidden tab.
  useEffect(() => {
    if (!VISIBLE_PROVIDER_TABS.some((t) => t.id === tab)) {
      setTab(VISIBLE_PROVIDER_TABS[0]?.id ?? "overview");
    }
  }, [tab]);

  // Deep-links (risk flags, jump-search) can request a specific tab via
  // requestView("providers", "cloudflare"). Depend on `pendingTab` (not just the
  // stable callback) so a request that arrives while Providers is ALREADY the
  // active view still re-runs and switches the tab (otherwise the click is dead).
  useEffect(() => {
    const requested = consumePendingTab();
    if (requested && VISIBLE_PROVIDER_TABS.some((t) => t.id === requested)) {
      setTab(requested as ProvidersTabId);
    }
  }, [consumePendingTab, pendingTab]);
  const providerHealth = cloudSnapshot?.providerHealth ?? [];
  const visibleProviderHealth = providerHealth.filter(
    (health) => !ALPHA_HIDDEN_PROVIDERS.has(health.id),
  );
  const providerServices = cloudSnapshot?.providerServices ?? [];
  const consoleResources = cloudSnapshot?.consoleResources ?? [];
  const selectedScopes = cloudSnapshot?.selectedScopes ?? [];
  const [externalError, setExternalError] = useState<string | null>(null);
  const [serviceProvider, setServiceProvider] = useState<ProviderId>(
    VISIBLE_CONSOLE_PROVIDERS[0] ?? "cloudflare",
  );
  const visibleServices = providerServices.filter(
    (service) => service.provider === serviceProvider,
  );
  const visibleResources = consoleResources
    .filter((resource) => resource.provider === serviceProvider)
    .slice(0, 80);
  const liveServiceCount = visibleServices.filter(
    (service) => service.status === "live" || service.status === "partial",
  ).length;

  const openExternal = async (url: string) => {
    setExternalError(null);
    try {
      await safeOpenExternal(url);
    } catch (e) {
      setExternalError(e instanceof Error ? e.message : "External link failed.");
    }
  };

  return (
    <div className="w-full space-y-6">
      <div className="flex w-fit flex-wrap gap-1 rounded-2xl border border-cream-200 bg-white p-1">
        {VISIBLE_PROVIDER_TABS.map((t) => {
          const isActive = tab === t.id;
          return (
            <button
              key={t.id}
              onClick={() => setTab(t.id)}
              className={`rounded-xl px-3 py-1.5 text-[12px] font-semibold transition-colors ${
                isActive
                  ? "bg-terracotta text-white"
                  : "text-cream-500 hover:bg-cream-50 hover:text-cream-700"
              }`}
            >
              {t.label}
            </button>
          );
        })}
      </div>

      {tab === "cloudflare" && <CloudflareView />}
      {tab === "scaleway" && <ComputeView />}
      {tab === "budget" && <BudgetView />}

      {tab === "overview" && (
        <div className="space-y-8 max-w-5xl">
          {externalError && (
            <div className="rounded-2xl border border-coral/20 bg-coral/[0.04] px-4 py-3 text-[12px] font-medium text-coral-dark">
              {externalError}
            </div>
          )}

      <section>
        <div className="mb-4 flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
          <div>
            <h3 className="mb-1 text-[11px] font-semibold uppercase tracking-widest text-cream-500">
              Provider Readiness
            </h3>
            <p className="text-[12px] text-cream-400">
              Live read/write readiness from saved tokens, pinned scope and inventory health
            </p>
          </div>
          <button
            onClick={() => void syncProviderInventory()}
            disabled={isLoading}
            data-help-title="This syncs every configured provider."
            data-help-lines="It reads Cloudflare and Scaleway readiness, scopes, and inventory.|It uses tokens saved in Secrets and should not write provider resources.|Use this after saving or rotating tokens.|If one provider fails, inspect its token and pinned scope."
            className="flex w-fit items-center gap-1.5 rounded-xl border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-medium text-cream-600 transition-colors hover:border-terracotta-200 hover:text-terracotta disabled:opacity-60"
          >
            <RefreshCw className="h-3.5 w-3.5" />
            Sync all
          </button>
        </div>

        <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
          {VISIBLE_CONSOLE_PROVIDERS.map((provider) => {
            const health = providerHealth.find((item) => item.id === provider);
            const scope = selectedScopes.find((item) => item.provider === provider);
            const resourcesForProvider = consoleResources.filter(
              (resource) => resource.provider === provider,
            ).length;
            const state = readinessCopy(provider, health, scope, resourcesForProvider);
            const tone = readinessTone[state.tone as keyof typeof readinessTone];
            const Icon = state.tone === "ready" ? CheckCircle2 : AlertTriangle;

            return (
              <div
                key={provider}
                className={`rounded-2xl border ${tone.border} ${tone.bg} p-4`}
                data-help-title={`${provider === "cloudflare" ? "Cloudflare" : "Scaleway"} readiness says whether this provider is safe to use.`}
                data-help-lines="Readiness combines saved token health, pinned scope, resource counts, and backend inventory status.|For Devboule, do not launch provider-writing agents if write readiness is missing or scope is wrong.|Read-ready is enough for verifiers; write-ready is needed for coders or human mutation actions.|Use Secrets to fix token and scope problems."
              >
                <div className="flex items-start gap-3">
                  <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-white">
                    <Icon className={`h-5 w-5 ${tone.icon}`} />
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <p className="text-[14px] font-semibold text-cream-800">
                        {provider === "cloudflare" ? "Cloudflare" : "Scaleway"}
                      </p>
                      <span className={`rounded-full px-2 py-0.5 text-[10px] font-semibold ${tone.badge}`}>
                        {state.title}
                      </span>
                    </div>
                    <p className="mt-1 text-[12px] leading-5 text-cream-500">
                      {state.detail}
                    </p>
                    <div className="mt-3 grid grid-cols-2 gap-2 text-[11px] sm:grid-cols-5">
                      <div>
                        <p className="text-cream-400">Read</p>
                        <p className="font-semibold text-cream-700">{state.read}</p>
                      </div>
                      <div>
                        <p className="text-cream-400">Write</p>
                        <p className="font-semibold text-cream-700">{state.write}</p>
                      </div>
                      <div className="min-w-0">
                        <p className="text-cream-400">Scope</p>
                        <p className="truncate font-semibold text-cream-700">{state.scope}</p>
                      </div>
                      <div>
                        <p className="text-cream-400">Token</p>
                        <p className="font-semibold text-cream-700">{state.token}</p>
                      </div>
                      <div>
                        <p className="text-cream-400">Kind</p>
                        <p className="font-semibold text-cream-700">{state.credential}</p>
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      </section>

      {VISIBLE_CONSOLE_PROVIDERS.length > 0 && (
      <>
      <section>
        <div className="mb-4 flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
          <div>
            <h3 className="mb-1 text-[11px] font-semibold uppercase tracking-widest text-cream-500">
              Provider Console Map
            </h3>
            <p className="text-[12px] text-cream-400">
              {liveServiceCount} live/partial sections · {visibleServices.length} mapped sections
            </p>
          </div>
          <div className="flex w-fit rounded-2xl border border-cream-200 bg-white p-1">
            {VISIBLE_CONSOLE_PROVIDERS.map((provider) => (
              <button
                key={provider}
                onClick={() => setServiceProvider(provider)}
                data-help-title={`This switches the console map to ${provider}.`}
                data-help-lines="The console map is a planning dashboard for provider surfaces.|Switching tabs only changes the displayed provider.|It does not call provider APIs or write anything.|Use the Cloudflare and Compute pages for deeper operational actions."
                className={`rounded-xl px-3 py-1.5 text-[12px] font-semibold transition-colors ${
                  serviceProvider === provider
                    ? "bg-terracotta text-white"
                    : "text-cream-500 hover:bg-cream-50 hover:text-cream-700"
                }`}
              >
                {providerMeta[provider].url.includes("cloudflare") ? "Cloudflare" : "Scaleway"}
              </button>
            ))}
          </div>
        </div>

        <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
          {visibleServices.map((service) => {
            const Icon = serviceIcon(service);
            const status =
              serviceStatusColors[service.status as keyof typeof serviceStatusColors] ??
              serviceStatusColors.unknown;

            return (
              <div
                key={service.id}
                className="rounded-2xl border border-cream-200 bg-white p-4"
                data-help-title={`${service.name} is a mapped provider console section.`}
                data-help-lines="The console map shows which Cloudflare or Scaleway surfaces the app understands or plans to cover.|For Devboule, this helps decide what UX/backend tool to build next instead of using raw terminal commands.|Live counts mean the app found resources for that service; missing counts can mean missing scope or missing implementation.|Use official docs before adding write permissions."
              >
                <div className="flex items-start gap-3">
                  <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-cream-50">
                    <Icon className="h-5 w-5 text-cream-600" />
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <p className="text-[14px] font-semibold text-cream-800">
                        {service.name}
                      </p>
                      <span className={`rounded-full px-2 py-0.5 text-[10px] font-semibold ${status.bg} ${status.text}`}>
                        {status.label}
                      </span>
                      {service.liveCount > 0 && (
                        <span className="rounded-full bg-cream-100 px-2 py-0.5 text-[10px] font-semibold text-cream-500">
                          {service.liveCount} live
                        </span>
                      )}
                    </div>
                    <p className="mt-1 text-[12px] leading-5 text-cream-500">
                      {service.description}
                    </p>
                    <div className="mt-3 flex flex-wrap gap-1.5">
                      {service.actions.slice(0, 5).map((action) => (
                        <span
                          key={action}
                          className="rounded-lg bg-cream-50 px-2 py-1 text-[10px] font-medium text-cream-500"
                        >
                          {action}
                        </span>
                      ))}
                    </div>
                    <p className="mt-3 text-[11px] leading-5 text-cream-400">
                      {service.coverage} · {service.permission}
                    </p>
                    {service.notes[0] && (
                      <p className="mt-1 text-[11px] leading-5 text-amber-dark">
                        {service.notes[0]}
                      </p>
                    )}
                  </div>
                  <button
                    onClick={() => void openExternal(service.docsUrl)}
                    data-help-title="This opens official provider documentation."
                    data-help-lines="Docs links leave the local app and open the provider website.|They are useful before adding a new dashboard action or permission scope.|Opening docs does not change your cloud account.|Prefer official docs when deciding token permissions."
                    className="shrink-0 rounded-xl border border-cream-200 p-2 text-cream-400 transition-colors hover:border-terracotta-200 hover:text-terracotta"
                    title="Open official docs"
                  >
                    <ExternalLink className="h-4 w-4" />
                  </button>
                </div>
              </div>
            );
          })}
          {visibleServices.length === 0 && (
            <div className="rounded-2xl border border-cream-200 bg-white px-5 py-8 text-center text-[13px] text-cream-400 xl:col-span-2">
              Sync providers to build the console map.
            </div>
          )}
        </div>
      </section>

      <section>
        <div className="mb-3 flex items-center justify-between">
          <div>
            <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
              Live Console Inventory
            </h3>
            <p className="mt-1 text-[12px] text-cream-400">
              {visibleResources.length} resources from current {serviceProvider === "cloudflare" ? "Cloudflare" : "Scaleway"} scope
            </p>
          </div>
        </div>

        <div className="overflow-hidden rounded-2xl border border-cream-200 bg-white">
          <div className="max-h-[520px] overflow-y-auto">
            {visibleResources.map((resource) => (
              <div
                key={resource.id}
                className="grid grid-cols-1 gap-2 border-b border-cream-100 px-4 py-3 last:border-b-0 lg:grid-cols-[minmax(0,1.35fr)_120px_minmax(0,1fr)_96px] lg:items-center lg:gap-3"
                data-help-title={`${resource.name} is a live provider inventory resource.`}
                data-help-lines="This is one resource the backend found in the current provider scope.|For Devboule, use it to verify what exists before building automations, smoke tests, or cleanup tasks.|Inventory is read-only here; operational writes belong on Cloudflare, Compute, or project-linked actions.|If it should not be here, check account/project isolation."
              >
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <p className="truncate text-[13px] font-semibold text-cream-800">
                      {resource.name}
                    </p>
                    <span className={`rounded-full px-2 py-0.5 text-[10px] font-semibold ${resourceTone(resource)}`}>
                      {resource.status}
                    </span>
                  </div>
                  <p className="mt-0.5 line-clamp-1 text-[11px] text-cream-400">
                    {resource.description}
                  </p>
                </div>
                <span className="rounded-lg bg-cream-50 px-2 py-1 text-center text-[10px] font-semibold text-cream-500">
                  {resource.resourceType}
                </span>
                <div className="min-w-0">
                  <p className="truncate text-[11px] text-cream-500">
                    {resource.region ?? resource.serviceId}
                  </p>
                  {resource.metadata[0] && (
                    <p className="mt-0.5 truncate text-[10px] text-cream-400">
                      {resource.metadata[0]}
                    </p>
                  )}
                </div>
                <button
                  onClick={() => void openExternal(resource.docsUrl)}
                  data-help-title="This opens documentation for the resource type."
                  data-help-lines="Use docs to verify API endpoints, scopes, and destructive behavior.|It does not call Cloudflare or Scaleway APIs.|Docs are especially important before adding write actions.|If docs disagree with the app, trust docs and audit the backend."
                  className="w-fit rounded-xl border border-cream-200 px-2.5 py-1.5 text-[11px] font-medium text-cream-500 hover:border-terracotta-200 hover:text-terracotta lg:justify-self-end"
                >
                  Docs
                </button>
              </div>
            ))}
            {visibleResources.length === 0 && (
              <div className="px-5 py-8 text-center text-[13px] text-cream-400">
                No live console resources for this provider yet. Sync with broader read permissions to populate data/IAM sections.
              </div>
            )}
          </div>
        </div>
      </section>
      </>
      )}

      {/* Operational providers */}
      <section>
        <div className="flex items-center justify-between mb-4">
          <div>
            <h3 className="text-[11px] font-semibold text-cream-500 uppercase tracking-widest mb-1">
              API Providers
            </h3>
            <p className="text-[12px] text-cream-400">
              Live status from backend provider inventory
            </p>
          </div>
          <span className="text-[11px] text-cream-400">
            {visibleProviderHealth.length} providers
          </span>
        </div>

        <div className="space-y-3">
          {visibleProviderHealth.map((health) => {
            const providerId = health.id as ProviderId;
            const meta = providerMeta[providerId];
            const consoleUrl = meta?.url;
            const Icon = iconMap[meta?.icon ?? "Globe"] || Globe;
            const hStatus = healthColors[health.status as keyof typeof healthColors] || healthColors.degraded;
            const token = tokenColors[health.tokenHealth as keyof typeof tokenColors] || tokenColors.unknown;

            return (
              <div
                key={health.id}
                className="bg-white rounded-2xl border border-cream-200 p-5 hover:shadow-soft-sm transition-shadow"
                data-help-title={`${health.name} provider status summarizes live access.`}
                data-help-lines="This card tells whether the saved token and provider scope can read inventory and whether writes look safe.|For Devboule, provider status is the gate before letting agents touch Cloudflare or Scaleway.|Read-only can be enough for verifier roles; coder roles need explicit limited write scopes.|If status is degraded, audit the token and pinned scope before using provider tools."
              >
                <div className="flex items-start gap-4">
                  <div className="w-10 h-10 rounded-xl bg-cream-50 flex items-center justify-center shrink-0">
                    <Icon className="w-5 h-5 text-cream-600" />
                  </div>

                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-3 mb-1">
                      <h4 className="text-[14px] font-semibold text-cream-800">
                        {health.name}
                      </h4>
                      <span
                        className={`inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-[10px] font-medium ${
                          hStatus.text
                        } ${hStatus.dot === "bg-sage" ? "bg-sage/10" : hStatus.dot === "bg-coral" ? "bg-coral/10" : "bg-amber/10"}`}
                      >
                        <span className={`w-1.5 h-1.5 rounded-full ${hStatus.dot}`} />
                        {hStatus.label}
                      </span>
                    </div>
                    <p className="text-[12px] text-cream-500">
                      {meta?.description ?? "Live provider inventory."}
                    </p>

                    {/* Operational metadata row */}
                    <div className="flex items-center gap-4 mt-3">
                      <div className="flex items-center gap-1.5 text-[11px] text-cream-400">
                        <RefreshCw className="w-3 h-3" />
                        <span>Last sync: {health.lastSync || "never"}</span>
                      </div>
                      <div className="flex items-center gap-1.5 text-[11px] text-cream-400">
                        <Key className="w-3 h-3" />
                        <span
                          className={`px-1.5 py-0.5 rounded text-[10px] font-medium ${token.bg} ${token.text}`}
                        >
                          Token: {token.label}
                        </span>
                      </div>
                      <div className="flex items-center gap-1.5 text-[11px] text-cream-400">
                        <Users className="w-3 h-3" />
                        <span>{health.resourceCount} resources</span>
                      </div>
                    </div>
                    {health.message && (
                      <p className="mt-2 text-[11px] text-amber-dark">{health.message}</p>
                    )}
                  </div>

                  <div className="flex shrink-0 items-center gap-2">
                    <button
                      onClick={() => void syncProviderInventory(providerId)}
                      disabled={isLoading}
                      data-help-title={`This syncs only ${health.name}.`}
                      data-help-lines="Provider-specific sync reads only this provider's inventory and health.|It uses the saved token and pinned scope for this provider.|It should not run write or delete actions.|Use it after token rotation or when one provider looks stale."
                      className="flex items-center gap-1.5 rounded-xl border border-cream-200 px-3 py-1.5 text-[12px] font-medium text-cream-600 transition-all duration-200 hover:border-terracotta-200 hover:bg-terracotta-50 hover:text-terracotta disabled:opacity-60"
                    >
                      <RefreshCw className="w-3.5 h-3.5" />
                      Sync
                    </button>
                    {consoleUrl && (
                      <button
                        onClick={() => void openExternal(consoleUrl)}
                        data-help-title={`This opens the ${health.name} web console.`}
                        data-help-lines="The provider console is outside Devboule.|Use it to revoke tokens, inspect permissions, or confirm billing directly.|Opening it does not change anything by itself.|For repeatable operations, prefer adding a guarded app action later."
                        className="flex items-center gap-1.5 rounded-xl border border-cream-200 px-3 py-1.5 text-[12px] font-medium text-cream-600 transition-all duration-200 hover:border-terracotta-200 hover:bg-terracotta-50 hover:text-terracotta"
                      >
                        <ExternalLink className="w-3.5 h-3.5" />
                        Console
                      </button>
                    )}
                  </div>
                </div>
              </div>
            );
          })}
          {visibleProviderHealth.length === 0 && (
            <div className="rounded-2xl border border-cream-200 bg-white px-5 py-8 text-center text-[13px] text-cream-400">
              No live provider status yet. Sync after saving Cloudflare and Scaleway tokens.
            </div>
          )}
        </div>
      </section>

      {config.bookmarks.length > 0 && (
        <section>
        <div className="flex items-center justify-between mb-3">
          <h3 className="text-[11px] font-semibold text-cream-500 uppercase tracking-widest">
            Bookmarks
          </h3>
          <span className="text-[11px] text-cream-400">
            {config.bookmarks.length} links
          </span>
        </div>

        <div className="grid grid-cols-1 sm:grid-cols-2 gap-2.5">
          {config.bookmarks.map((bookmark) => {
            const Icon = iconMap[bookmark.icon] || Globe;

            return (
              <button
                key={bookmark.id}
                onClick={() => void openExternal(bookmark.url)}
                data-help-title={`${bookmark.name} opens a saved provider bookmark.`}
                data-help-lines="Bookmarks are quick links to external consoles, docs, or project resources.|For Devboule, use them to verify provider state or documentation before adding dangerous app actions.|Opening a bookmark does not change Cloudflare, Scaleway, or local files.|Do not rely on bookmarks as audit evidence; write important findings into the project."
                className="group flex items-center gap-3 px-4 py-3 bg-white rounded-xl border border-cream-200
                           hover:shadow-soft-xs hover:border-cream-300
                           transition-all duration-200 text-left"
              >
                <Icon className="w-4 h-4 text-cream-500 group-hover:text-terracotta transition-colors" />
                <span className="text-[13px] font-medium text-cream-700 group-hover:text-cream-800 transition-colors truncate">
                  {bookmark.name}
                </span>
                <ExternalLink className="w-3 h-3 text-cream-300 ml-auto opacity-0 group-hover:opacity-100 transition-opacity" />
              </button>
            );
          })}
        </div>
        </section>
      )}
        </div>
      )}
    </div>
  );
}
