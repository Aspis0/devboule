import {
  AlertTriangle,
  BrainCircuit,
  ClipboardCheck,
  CreditCard,
  Database,
  ExternalLink,
  HardDrive,
  Layers3,
  Network,
  Plus,
  RefreshCw,
  Search,
  ServerCog,
  Sparkles,
  Boxes,
  Table2,
  Play,
  Save,
  Trash2,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { invokeBackendCommand, useAppContext } from "../../context/AppContext";
import { toOracleError } from "../../utils/oracleError";
import type {
  CloudflareBilling,
  CloudflareEnvDryRunResult,
  CloudflareSmokeDryRunResult,
  CloudflareWorkerSettings,
  CloudflareWorkerSummary,
  CloudflareAiGatewaySettings,
  CloudflareAiGatewaySettingsPatch,
  CloudflareAutoragReindexResult,
  CloudflareKvKey,
  CloudflareKvValue,
  CloudflareD1QueryResult,
  CloudflareR2Config,
  OracleAnswer,
  OracleError,
  ProviderConsoleResourceSummary,
  ProviderHealth,
  ProjectDetail,
  ProjectSummary,
  SecretRotationResult,
} from "../../types/backend";

// Resource-type selector that replaces the old tab union. Each entry maps a
// short UI key to the exact backend `resourceType` strings emitted by
// sync_provider_inventory for Cloudflare (verified against
// src-tauri/src/backend/providers.rs). "workers" and "billing" are special
// views handled separately from the generic inventory two-pane.
type ResourceType =
  | "workers"
  | "r2"
  | "durable-objects"
  | "kv"
  | "d1"
  | "queues"
  | "vectorize"
  | "ai-gateway"
  | "ai-search"
  | "billing";

const resourceTabs: {
  id: ResourceType;
  label: string;
  icon: typeof Boxes;
  // Backend resourceType strings this tab selects (empty for special views).
  types: string[];
}[] = [
  { id: "workers", label: "Workers", icon: ServerCog, types: ["Worker"] },
  { id: "r2", label: "R2", icon: HardDrive, types: ["R2 Bucket"] },
  {
    id: "durable-objects",
    label: "Durable Objects",
    icon: Boxes,
    types: ["Durable Object Namespace"],
  },
  { id: "kv", label: "KV", icon: Layers3, types: ["KV Namespace"] },
  { id: "d1", label: "D1", icon: Table2, types: ["D1 Database"] },
  { id: "queues", label: "Queues", icon: Network, types: ["Queue"] },
  {
    id: "vectorize",
    label: "Vectorize",
    icon: Database,
    types: ["Vectorize Index"],
  },
  {
    id: "ai-gateway",
    label: "AI Gateway",
    icon: Sparkles,
    types: ["AI Gateway"],
  },
  {
    id: "ai-search",
    label: "AI Search",
    icon: BrainCircuit,
    types: ["AI Search Namespace", "AI Search Instance"],
  },
  { id: "billing", label: "Billing", icon: CreditCard, types: [] },
];

function credentialKindLabel(kind: string | null | undefined) {
  switch (kind) {
    case "cloudflare_account_owned_token":
      return "Account-owned token";
    case "cloudflare_profile_token":
      return "Profile token";
    case "cloudflare_unverified_policy_token":
      return "Policy unknown";
    default:
      return "Unknown token";
  }
}

function rotationDisabledReason(health: ProviderHealth | undefined) {
  if (!health) return "Sync Cloudflare before rotating Worker secrets.";
  if (health.tokenHealth === "valid") return "";
  // valid_unverified means Cloudflare did not expose policy details, so write
  // cannot be proven up front. Allow the attempt: the rotate call surfaces a loud
  // Cloudflare rejection if the token is truly under-scoped.
  if (health.tokenHealth === "valid_unverified") return "";
  if (health.tokenHealth === "valid_read_only") {
    return "Current Cloudflare token is read-only. Secret rotation needs Workers Scripts Write.";
  }
  return health.message || "Cloudflare token is not ready for mutation.";
}

function resourceGroupLabel(key: string) {
  switch (key) {
    case "cf-storage-data":
      return "Storage and Data";
    case "cf-ai-observability":
      return "AI Search and Observability";
    case "cf-developer-platform":
      return "Developer Platform";
    case "cf-workers-pages":
      return "Workers and Pages";
    case "cf-security-network":
      return "Security and Network";
    case "cf-account-iam":
      return "Account and IAM";
    default:
      return key;
  }
}

function resourceApiHint(resource: ProviderConsoleResourceSummary) {
  const type = resource.resourceType.toLowerCase();
  if (type.includes("worker")) return "GET /accounts/{account_id}/workers/scripts/{script_name}";
  if (type.includes("r2")) return "GET /accounts/{account_id}/r2/buckets";
  if (type.includes("kv")) return "GET /accounts/{account_id}/storage/kv/namespaces";
  if (type.includes("d1")) return "GET /accounts/{account_id}/d1/database";
  if (type.includes("queue")) return "GET /accounts/{account_id}/queues";
  if (type.includes("vectorize")) return "GET /accounts/{account_id}/vectorize/v2/indexes";
  if (type.includes("durable")) return "GET /accounts/{account_id}/workers/durable_objects/namespaces";
  if (type.includes("gateway")) return "GET /accounts/{account_id}/ai-gateway/gateways";
  if (type.includes("ai search")) return "GET /accounts/{account_id}/ai-search/namespaces";
  if (type.includes("zone")) return "GET /zones";
  if (type.includes("token")) return "GET /accounts/{account_id}/tokens/verify";
  return "GET /accounts/{account_id}/...";
}

function smokeEvidenceText(result: CloudflareSmokeDryRunResult) {
  const scope = result.selectedScope?.name || result.selectedScope?.id || "not pinned";
  const rotation = result.canRotateWorkerSecret ? "would be allowed" : `blocked: ${result.blockedReason ?? "write not proven"}`;
  return [
    "Cloudflare smoke dry run",
    `Status: ${result.status}`,
    `Scope: ${scope}`,
    `Token kind: ${credentialKindLabel(result.credentialKind)}`,
    `Token health: ${result.tokenHealth}`,
    `Workers/resources read: ${result.resourceCount}`,
    `Secret rotation: ${rotation}`,
    "API equivalent:",
    ...result.apiEquivalent.map((line) => `- ${line}`),
    ...(result.risks.length ? ["Risks:", ...result.risks.map((risk) => `- ${risk}`)] : []),
  ].join("\n");
}

function secretRotationEvidenceText(result: SecretRotationResult, worker: CloudflareWorkerSummary) {
  return [
    "Cloudflare Worker secret rotation",
    `Worker: ${result.workerName}`,
    `Binding: ${result.secretName}`,
    `Account: ${worker.accountName || result.accountId}`,
    `Rotated at: ${result.rotatedAt}`,
    "API equivalent:",
    `- PUT /accounts/${result.accountId}/workers/scripts/${result.workerName}/secrets/${result.secretName}`,
    "Secret value was not stored, logged or attached to this project note.",
  ].join("\n");
}

function envWriteEvidenceText(
  worker: CloudflareWorkerSummary,
  varName: string,
  result: { writtenAt: string; message: string },
) {
  return [
    "Cloudflare Worker plain-text env update",
    `Worker: ${worker.name}`,
    `Variable: ${varName}`,
    `Account: ${worker.accountName || worker.accountId}`,
    `Written at: ${result.writtenAt}`,
    `Result: ${result.message}`,
    "API equivalent:",
    `- PATCH /accounts/${worker.accountId}/workers/scripts/${worker.name}/settings`,
    "Plain-text vars only. Existing secrets and other bindings were preserved.",
  ].join("\n");
}

function aiGatewayUpdateEvidenceText(
  gatewayId: string,
  changedFields: string[],
  result: { message: string | null },
) {
  return [
    "Cloudflare AI Gateway settings updated",
    `Gateway: ${gatewayId}`,
    `Fields: ${changedFields.length ? changedFields.join(", ") : "none"}`,
    `Result: ${result.message || "settings saved"}`,
    "API equivalent:",
    "- PUT /accounts/{account_id}/ai-gateway/gateways/{gateway_id}",
    "Only configuration toggles/limits were changed. No prompt, completion or secret values are stored in this note.",
  ].join("\n");
}

function autoragReindexEvidenceText(
  instanceId: string,
  result: CloudflareAutoragReindexResult,
) {
  return [
    "Cloudflare AI Search reindex triggered",
    `Instance: ${instanceId}`,
    `Job: ${result.jobId || "queued"}`,
    `Triggered at: ${result.triggeredAt}`,
    `Result: ${result.message}`,
    "API equivalent:",
    "- POST /accounts/{account_id}/autorag/rags/{rag_id}/sync",
    "Reindex is an async trigger only. No source data or content is stored in this note.",
  ].join("\n");
}

function workerDriftFlags(worker: CloudflareWorkerSummary) {
  const flags = [];
  if (worker.status !== "healthy") flags.push(`deployment status: ${worker.status}`);
  if (!worker.lastDeploy) flags.push("latest deployment timestamp missing");
  if (!worker.compatibilityDate) flags.push("compatibility date missing");
  if (worker.routes.length === 0) flags.push("no route metadata reported");
  if (worker.handlers.length === 0) flags.push("handler metadata missing");
  return flags;
}

export function CloudflareView() {
  const {
    cloudSnapshot,
    syncProviderInventory,
    runCloudflareSmokeDryRun,
    rotateCloudflareWorkerSecret,
    fetchCloudflareWorkerSettings,
    cloudflareEnvDryRun,
    cloudflareSetWorkerEnv,
    fetchCloudflareBilling,
    fetchCloudflareAiGatewaySettings,
    setCloudflareAiGatewaySettings,
    cloudflareAutoragReindex,
    fetchCloudflareKvKeys,
    fetchCloudflareKvValue,
    setCloudflareKvValue,
    deleteCloudflareKvValue,
    cloudflareD1Query,
    fetchCloudflareR2Config,
    setCloudflareR2Lifecycle,
    setCloudflareR2Cors,
    askOracle,
    isLoading,
  } = useAppContext();
  const [resourceType, setResourceType] = useState<ResourceType>("workers");
  const [resourceSearch, setResourceSearch] = useState("");
  const [selectedResourceId, setSelectedResourceId] = useState<string | null>(null);
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  // Audit-target project. Defaults empty, then seeds to the first active project
  // once the list loads (see the list_projects effect). The Phase-G dissolution of
  // the standalone Agents page removed the sole writer of the former
  // `aspis:selectedProjectId` sessionStorage handoff, so there is no cross-view
  // seed to read anymore.
  const [auditProjectId, setAuditProjectId] = useState("");
  const [auditMessage, setAuditMessage] = useState<string | null>(null);
  const [selectedWorkerId, setSelectedWorkerId] = useState<string | null>(null);
  const [workerSecretName, setWorkerSecretName] = useState("");
  const [workerSecretValue, setWorkerSecretValue] = useState("");
  const [workerActionMessage, setWorkerActionMessage] = useState<string | null>(null);

  // Oracle explanation for the selected worker (mirrors WorkersTable's guard).
  const [oracleAnswer, setOracleAnswer] = useState<OracleAnswer | null>(null);
  const [oracleError, setOracleError] = useState<OracleError | null>(null);
  const [oracleLoading, setOracleLoading] = useState(false);
  const oracleRequestId = useRef(0);

  // Worker settings (env vars + secret names + other bindings).
  const [settings, setSettings] = useState<CloudflareWorkerSettings | null>(null);
  const [settingsLoading, setSettingsLoading] = useState(false);
  const settingsRequestId = useRef(0);

  // Per-variable draft values + per-variable dry-run results. Keyed by var name.
  // A draft only becomes applicable after a dry-run for THAT exact draft value.
  const [envDrafts, setEnvDrafts] = useState<Record<string, string>>({});
  const [envDryRuns, setEnvDryRuns] = useState<Record<string, CloudflareEnvDryRunResult>>({});
  // The draft value that the stored dry-run was computed for, so editing the
  // input again re-disables Apply until a fresh dry-run is run.
  const [envDryRunValue, setEnvDryRunValue] = useState<Record<string, string>>({});

  // New (added) env var.
  const [newEnvName, setNewEnvName] = useState("");
  const [newEnvValue, setNewEnvValue] = useState("");
  const [newEnvDryRun, setNewEnvDryRun] = useState<CloudflareEnvDryRunResult | null>(null);
  const [newEnvDryRunFor, setNewEnvDryRunFor] = useState<{ name: string; value: string } | null>(null);

  // Per-worker smoke dry run.
  const [smoke, setSmoke] = useState<CloudflareSmokeDryRunResult | null>(null);

  // Billing (lazy on first selection of the billing tab).
  const [billing, setBilling] = useState<CloudflareBilling | null>(null);
  const [billingLoading, setBillingLoading] = useState(false);
  const [billingLoaded, setBillingLoaded] = useState(false);

  const health = cloudSnapshot?.providerHealth.find((item) => item.id === "cloudflare");
  const scope = cloudSnapshot?.selectedScopes.find((item) => item.provider === "cloudflare");
  const workers = cloudSnapshot?.workers ?? [];
  // Memoized so a new array identity each render does not defeat the
  // filteredResources useMemo below (it depends on `resources`).
  const consoleResources = cloudSnapshot?.consoleResources;
  const resources = useMemo(
    () => consoleResources?.filter((item) => item.provider === "cloudflare") ?? [],
    [consoleResources],
  );
  const canRotateSecrets =
    (health?.tokenHealth === "valid" || health?.tokenHealth === "valid_unverified") &&
    scope !== undefined;
  const actionProject = projects.find((project) => project.id === auditProjectId) ?? null;
  const actionProjectRequiredReason = actionProject
    ? ""
    : "Select an active project before running Cloudflare smoke or write actions.";
  const rotateReason = actionProjectRequiredReason || rotationDisabledReason(health);
  const canRotateWithProject = canRotateSecrets && Boolean(actionProject);

  const activeTabTypes = useMemo(
    () => resourceTabs.find((tab) => tab.id === resourceType)?.types ?? [],
    [resourceType],
  );

  const filteredResources = useMemo(() => {
    const query = resourceSearch.trim().toLowerCase();
    return resources
      .filter((resource) => activeTabTypes.includes(resource.resourceType))
      .filter((resource) => {
        if (!query) return true;
        return [
          resource.name,
          resource.resourceType,
          resource.status,
          resource.description,
          resource.region ?? "",
          ...resource.metadata,
        ]
          .join(" ")
          .toLowerCase()
          .includes(query);
      })
      .sort((a, b) => a.resourceType.localeCompare(b.resourceType) || a.name.localeCompare(b.name));
  }, [resources, activeTabTypes, resourceSearch]);
  const selectedResource = useMemo(
    () => filteredResources.find((resource) => resource.id === selectedResourceId) ?? filteredResources[0] ?? null,
    [filteredResources, selectedResourceId],
  );
  const selectedWorker = useMemo(
    () => workers.find((worker) => worker.id === selectedWorkerId) ?? workers[0] ?? null,
    [workers, selectedWorkerId],
  );
  const selectedWorkerDrift = useMemo(
    () => (selectedWorker ? workerDriftFlags(selectedWorker) : []),
    [selectedWorker],
  );

  // Reset the selected resource whenever the active resource tab changes.
  // Otherwise a `selectedResourceId` from the previous tab survives and
  // `selectedResource` silently falls back to `filteredResources[0]` of the
  // NEW type — rendering a detail panel (KvPanel, R2Panel, ...) bound to the
  // wrong resource's id and letting a per-type command target it. The worker
  // selection (`selectedWorkerId`) is a separate state and is unaffected.
  useEffect(() => {
    setSelectedResourceId(null);
  }, [resourceType]);

  useEffect(() => {
    void invokeBackendCommand<ProjectSummary[]>("list_projects")
      .then((items) => {
        const active = items.filter((project) => project.status === "active");
        setProjects(active);
        setAuditProjectId((current) => {
          if (active.some((project) => project.id === current)) return current;
          return active[0]?.id ?? "";
        });
      })
      .catch(() => setProjects([]));
  }, []);

  // Load Oracle explanation + Worker settings whenever the selected worker
  // changes (while the Workers tab is active). Request-id guards keep a slow
  // response from clobbering a newer selection (mirrors WorkersTable).
  const selectedWorkerKey = selectedWorker?.id ?? null;
  useEffect(() => {
    if (resourceType !== "workers" || !selectedWorker) {
      return;
    }
    const worker = selectedWorker;

    const oracleId = oracleRequestId.current + 1;
    oracleRequestId.current = oracleId;
    setOracleAnswer(null);
    setOracleError(null);
    setOracleLoading(true);
    void askOracle(worker.oracleQuery || worker.name, 4)
      .then((answer) => {
        if (oracleRequestId.current === oracleId) setOracleAnswer(answer);
      })
      .catch((e) => {
        if (oracleRequestId.current === oracleId) {
          setOracleAnswer(null);
          setOracleError(toOracleError(e));
        }
      })
      .finally(() => {
        if (oracleRequestId.current === oracleId) setOracleLoading(false);
      });

    const settingsId = settingsRequestId.current + 1;
    settingsRequestId.current = settingsId;
    setSettings(null);
    setSettingsLoading(true);
    setEnvDrafts({});
    setEnvDryRuns({});
    setEnvDryRunValue({});
    setNewEnvName("");
    setNewEnvValue("");
    setNewEnvDryRun(null);
    setNewEnvDryRunFor(null);
    setSmoke(null);
    setWorkerActionMessage(null);
    // Pasted secret must not linger across worker switches.
    setWorkerSecretName("");
    setWorkerSecretValue("");
    void fetchCloudflareWorkerSettings(worker.name)
      .then((result) => {
        if (settingsRequestId.current !== settingsId) return;
        setSettings(result);
        if (result?.readable) {
          const drafts: Record<string, string> = {};
          for (const binding of result.plainText) drafts[binding.name] = binding.text ?? "";
          setEnvDrafts(drafts);
        }
      })
      .finally(() => {
        if (settingsRequestId.current === settingsId) setSettingsLoading(false);
      });

    // Discard in-flight Oracle + settings responses on unmount / worker switch.
    // Bumping .current makes both pending handlers' request-id checks fail; the
    // next effect run reads .current + 1, so the new selection still loads.
    return () => {
      oracleRequestId.current += 1;
      settingsRequestId.current += 1;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [resourceType, selectedWorkerKey, askOracle, fetchCloudflareWorkerSettings]);

  // Lazily fetch billing the first time the billing tab is opened. The
  // in-flight guard (billingLoading) prevents a refetch storm; billingLoaded is
  // only latched on a SUCCESSFUL load so a failed fetch can be retried by
  // re-opening the tab within the same mount.
  useEffect(() => {
    if (resourceType !== "billing" || billingLoaded || billingLoading) return;
    setBillingLoading(true);
    void fetchCloudflareBilling()
      .then((result) => {
        setBilling(result);
        if (result != null) setBillingLoaded(true);
      })
      .finally(() => setBillingLoading(false));
  }, [resourceType, billingLoaded, billingLoading, fetchCloudflareBilling]);

  // Drop any pasted secret when the user leaves the Workers tab, so a private
  // value never lingers in JS state on a non-Workers view.
  useEffect(() => {
    if (resourceType !== "workers") {
      setWorkerSecretName("");
      setWorkerSecretValue("");
    }
  }, [resourceType]);

  const appendProjectEvidence = async (text: string, successMessage: string) => {
    if (!auditProjectId) {
      setAuditMessage("Select an active project before running Cloudflare actions.");
      return false;
    }
    try {
      const project = await invokeBackendCommand<ProjectDetail>("get_project", {
        projectId: auditProjectId,
      });
      await invokeBackendCommand<ProjectDetail>("append_project_note", {
        projectId: auditProjectId,
        note: {
          text,
          source: "cloudflare",
          expectedRevision: project.revision,
        },
      });
      setAuditMessage(successMessage);
      return true;
    } catch (e) {
      setAuditMessage(e instanceof Error ? e.message : "Cloudflare evidence could not be attached to project.");
      return false;
    }
  };

  const reloadSettings = async (workerName: string) => {
    const settingsId = settingsRequestId.current + 1;
    settingsRequestId.current = settingsId;
    setSettingsLoading(true);
    try {
      const result = await fetchCloudflareWorkerSettings(workerName);
      // Guard every state write: a worker switch (or unmount) bumps the id and
      // this reload must not clobber the newer selection's state.
      if (settingsRequestId.current !== settingsId) return;
      setSettings(result);
      if (result?.readable) {
        const drafts: Record<string, string> = {};
        for (const binding of result.plainText) drafts[binding.name] = binding.text ?? "";
        setEnvDrafts(drafts);
      } else {
        // null (load failed) or unreadable: keep state coherent, no stale drafts.
        setEnvDrafts({});
      }
    } finally {
      if (settingsRequestId.current === settingsId) setSettingsLoading(false);
    }
  };

  const runEnvDryRun = async (varName: string, value: string) => {
    if (!selectedWorker) return;
    const result = await cloudflareEnvDryRun(selectedWorker.name, varName, value);
    if (!result) return;
    setEnvDryRuns((prev) => ({ ...prev, [varName]: result }));
    setEnvDryRunValue((prev) => ({ ...prev, [varName]: value }));
  };

  const applyEnv = async (varName: string, value: string) => {
    const worker = selectedWorker;
    if (!worker) return;
    const result = await cloudflareSetWorkerEnv(worker.name, varName, value);
    if (!result) return;
    await appendProjectEvidence(
      envWriteEvidenceText(worker, varName, result),
      "Worker env update evidence attached to the selected project.",
    );
    setWorkerActionMessage(result.message || `${varName} written at ${result.writtenAt}`);
    setEnvDryRuns((prev) => {
      const next = { ...prev };
      delete next[varName];
      return next;
    });
    setEnvDryRunValue((prev) => {
      const next = { ...prev };
      delete next[varName];
      return next;
    });
    // Only reload if the user is still on the worker we wrote to. If they
    // switched, the worker-switch effect already owns loading the new settings.
    if (selectedWorker?.name === worker.name) await reloadSettings(worker.name);
  };

  const runNewEnvDryRun = async () => {
    if (!selectedWorker) return;
    const name = newEnvName.trim();
    if (!name) return;
    const result = await cloudflareEnvDryRun(selectedWorker.name, name, newEnvValue);
    if (!result) return;
    setNewEnvDryRun(result);
    setNewEnvDryRunFor({ name, value: newEnvValue });
  };

  const applyNewEnv = async () => {
    const worker = selectedWorker;
    if (!worker) return;
    // Snapshot name + value at the top so an input edit mid-write cannot make
    // the written value diverge from what evidence/message report.
    const name = newEnvName.trim();
    const value = newEnvValue;
    if (!name) return;
    const result = await cloudflareSetWorkerEnv(worker.name, name, value);
    if (!result) return;
    await appendProjectEvidence(
      envWriteEvidenceText(worker, name, result),
      "Worker env addition evidence attached to the selected project.",
    );
    setWorkerActionMessage(result.message || `${name} written at ${result.writtenAt}`);
    setNewEnvName("");
    setNewEnvValue("");
    setNewEnvDryRun(null);
    setNewEnvDryRunFor(null);
    // Only reload if the user is still on the worker we wrote to.
    if (selectedWorker?.name === worker.name) await reloadSettings(worker.name);
  };

  const runWorkerSmoke = async () => {
    if (!actionProject) {
      setAuditMessage("Select an active project before running Cloudflare smoke checks.");
      return;
    }
    const result = await runCloudflareSmokeDryRun();
    if (!result) return;
    setSmoke(result);
    await appendProjectEvidence(smokeEvidenceText(result), "Dry run evidence attached to the selected project.");
  };

  const rotateSelectedWorkerSecret = async () => {
    const worker = selectedWorker;
    if (!worker || !canRotateWithProject) return;
    const result = await rotateCloudflareWorkerSecret(
      worker.accountId,
      worker.name,
      workerSecretName,
      workerSecretValue,
    );
    if (!result) return;
    await appendProjectEvidence(
      secretRotationEvidenceText(result, worker),
      "Secret rotation evidence attached to the selected project.",
    );
    setWorkerSecretName("");
    setWorkerSecretValue("");
    setWorkerActionMessage(`${result.secretName} rotated at ${result.rotatedAt}`);
    // Only reload if the user is still on the worker we rotated against.
    if (selectedWorker?.name === worker.name) await reloadSettings(worker.name);
  };

  return (
    <div className="max-w-6xl space-y-5">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
        <div>
          <h2 className="text-[22px] font-semibold text-cream-900">Cloudflare Console</h2>
          <p className="mt-1 text-[12px] text-cream-500">
            Inspect Workers and account resources, preview env changes, and run guarded mutations with project evidence.
          </p>
        </div>
        <div className="flex flex-col gap-2 sm:items-end">
          <select
            value={auditProjectId}
            data-help-title="This chooses the project that receives evidence."
            data-help-lines="A project is the local work notebook for one job.|Cloudflare smoke tests, env writes and secret rotations should write evidence there.|Selecting a project does not change Cloudflare by itself.|If no project is selected, guarded write actions stay blocked."
            onChange={(event) => setAuditProjectId(event.target.value)}
            className="w-full rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-700 outline-none focus:border-terracotta/30 sm:w-72"
          >
            <option value="">Select action project</option>
            {projects.map((project) => (
              <option key={project.id} value={project.id}>
                {project.title}
              </option>
            ))}
          </select>
          <button
            onClick={() => void syncProviderInventory("cloudflare")}
            disabled={isLoading}
            data-help-title="This asks Cloudflare what exists right now."
            data-help-lines="Sync is a live read of Workers, routes, deployments, R2, KV, D1, queues, and account resources.|It uses the Cloudflare dashboard token saved in Windows vault.|It should not write, deploy, or rotate anything.|If the token is expired or scoped wrong, the page will show a blocked or limited state."
            className="flex w-fit items-center gap-1.5 rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:border-terracotta-200 hover:text-terracotta disabled:opacity-60"
          >
            <RefreshCw className="h-3.5 w-3.5" />
            Sync Cloudflare
          </button>
        </div>
      </div>

      <div className="flex w-fit max-w-full flex-wrap gap-1 overflow-x-auto rounded-2xl border border-cream-200 bg-white p-1">
        {resourceTabs.map((tab) => {
          const Icon = tab.icon;
          const active = resourceType === tab.id;
          const count = tab.types.length
            ? resources.filter((r) => tab.types.includes(r.resourceType)).length
            : null;
          return (
            <button
              key={tab.id}
              onClick={() => setResourceType(tab.id)}
              data-help-title={`This opens the Cloudflare ${tab.label} resources.`}
              data-help-lines="The selector only changes which Cloudflare resource type you inspect on screen.|Workers has env, secret and smoke actions; other types are inspect-only for now; Billing is account-level.|No provider write happens just by switching here.|Counts reflect the last sync of the pinned account scope."
              className={`flex items-center gap-2 rounded-xl px-3 py-2 text-[12px] font-semibold transition-colors ${
                active
                  ? "bg-terracotta text-white"
                  : "text-cream-500 hover:bg-cream-50 hover:text-cream-800"
              }`}
            >
              <Icon className="h-3.5 w-3.5" />
              {tab.label}
              {count !== null && (
                <span
                  className={`rounded-full px-1.5 py-0.5 text-[10px] font-semibold ${
                    active ? "bg-white/20 text-white" : "bg-cream-50 text-cream-500"
                  }`}
                >
                  {count}
                </span>
              )}
            </button>
          );
        })}
      </div>

      {auditMessage && (
        <p className="rounded-xl bg-cream-50 px-3 py-2 text-[11px] font-semibold text-cream-600">
          {auditMessage}
        </p>
      )}

      {resourceType === "workers" && (
        <div className="grid grid-cols-1 gap-4 xl:grid-cols-[minmax(0,0.9fr)_minmax(360px,1.1fr)] xl:items-start">
          <div className="space-y-4 xl:sticky xl:top-0 xl:self-start xl:max-h-[calc(100vh-7rem)] xl:overflow-y-auto">
            <section className="rounded-2xl border border-cream-200 bg-white p-5">
              <div className="mb-4 flex items-center justify-between gap-3">
                <div>
                  <h3 className="text-[15px] font-semibold text-cream-900">Workers</h3>
                  <p className="mt-1 text-[12px] text-cream-500">
                    Scripts, routes, runtime and deployment state.
                  </p>
                </div>
                <span className="rounded-full bg-cream-50 px-2 py-1 text-[10px] font-semibold text-cream-500">
                  {workers.length}
                </span>
              </div>

              <div className="space-y-2">
                {workers.map((worker) => {
                  const active = selectedWorker?.id === worker.id;
                  const drift = workerDriftFlags(worker);
                  return (
                    <button
                      key={worker.id}
                      type="button"
                      onClick={() => {
                        setSelectedWorkerId(worker.id);
                        setWorkerActionMessage(null);
                      }}
                      data-help-title="A Worker is Cloudflare code running at the edge."
                      data-help-lines="Selecting a Worker opens its Oracle link, settings, env, secrets and smoke actions.|Routes show where traffic enters the Worker.|Deploy and compatibility fields help spot drift before a change.|Env writes and secret rotation stay blocked unless token and project checks pass."
                      className={`w-full rounded-xl border px-3 py-3 text-left transition-colors ${
                        active
                          ? "border-terracotta/25 bg-terracotta/[0.05]"
                          : "border-cream-100 bg-white hover:bg-cream-50"
                      }`}
                    >
                      <div className="flex items-start justify-between gap-3">
                        <div className="min-w-0">
                          <p className="truncate font-mono text-[13px] font-semibold text-cream-800">
                            {worker.name}
                          </p>
                          <p className="mt-0.5 truncate text-[11px] text-cream-400">
                            {worker.routes[0] || worker.purpose}
                          </p>
                        </div>
                        <span
                          className={`rounded-full px-2 py-0.5 text-[10px] font-semibold ${
                            worker.status === "healthy"
                              ? "bg-sage/10 text-sage-dark"
                              : worker.status === "degraded"
                                ? "bg-amber/[0.12] text-amber-dark"
                                : "bg-coral/[0.08] text-coral-dark"
                          }`}
                        >
                          {worker.status}
                        </span>
                      </div>
                      <div className="mt-3 grid grid-cols-3 gap-2">
                        <Metric label="Deploy" value={worker.lastDeploy || "unknown"} />
                        <Metric label="Compat" value={worker.compatibilityDate || "unknown"} />
                        <Metric label="Drift" value={drift.length ? String(drift.length) : "none"} />
                      </div>
                    </button>
                  );
                })}
                {workers.length === 0 && (
                  <p className="rounded-xl bg-cream-50 px-3 py-8 text-center text-[13px] text-cream-400">
                    No Workers synced. Sync Cloudflare with a Workers read token.
                  </p>
                )}
              </div>
            </section>

            {selectedWorker && (
              <section className="rounded-2xl border border-cream-200 bg-white p-5">
                <div>
                  <div className="flex flex-wrap items-center gap-2">
                    <p className="font-mono text-[16px] font-semibold text-cream-900">
                      {selectedWorker.name}
                    </p>
                    <span className="rounded-full bg-cream-50 px-2 py-0.5 text-[10px] font-semibold text-cream-500">
                      {selectedWorker.accountName || selectedWorker.accountId}
                    </span>
                  </div>
                  <p className="mt-2 text-[12px] leading-5 text-cream-600">
                    {selectedWorker.purpose}
                  </p>
                </div>

                <div className="mt-4 grid grid-cols-2 gap-3 md:grid-cols-4">
                  <Metric label="Status" value={selectedWorker.status} />
                  <Metric label="Usage" value={selectedWorker.usageModel || "unknown"} />
                  <Metric label="Deploy" value={selectedWorker.lastDeploy || "unknown"} />
                  <Metric label="Compat" value={selectedWorker.compatibilityDate || "unknown"} />
                </div>

                <div className="mt-4 grid grid-cols-1 gap-4 lg:grid-cols-2">
                  <DetailList
                    title="Routes"
                    items={selectedWorker.routes}
                    empty="No route metadata reported."
                  />
                  <DetailList
                    title="Runtime"
                    items={[
                      ...(selectedWorker.handlers.length ? selectedWorker.handlers : ["fetch"]),
                      ...selectedWorker.compatibilityFlags.map((flag) => `flag: ${flag}`),
                    ]}
                    empty="No runtime metadata reported."
                  />
                </div>

                <div className="mt-4">
                  <p className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
                    Drift / Missing Metadata
                  </p>
                  <div className="space-y-2">
                    {selectedWorkerDrift.map((flag) => (
                      <p key={flag} className="rounded-xl bg-amber/[0.08] px-3 py-2 text-[11px] font-semibold text-amber-dark">
                        {flag}
                      </p>
                    ))}
                    {selectedWorkerDrift.length === 0 && (
                      <p className="rounded-xl bg-sage/10 px-3 py-2 text-[11px] font-semibold text-sage-dark">
                        No obvious drift in the synced Worker metadata.
                      </p>
                    )}
                  </div>
                </div>

                <div
                  className="mt-5 rounded-2xl border border-cream-100 bg-cream-50 p-4"
                  data-help-title="Oracle links the live Worker to local architecture chunks."
                  data-help-lines="Oracle is a read path that connects this Cloudflare Worker to indexed code and notes.|Use it to understand ownership and intent before changing env or secrets.|It does not change Cloudflare.|If results are weak, refresh the Oracle index and provider inventory."
                >
                  <div className="mb-2 flex items-center gap-2">
                    <BrainCircuit className="h-3.5 w-3.5 text-teal" />
                    <h4 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
                      Oracle explanation
                    </h4>
                  </div>
                  {oracleLoading ? (
                    <p className="text-[12px] text-cream-400">Asking Oracle about this Worker...</p>
                  ) : oracleError ? (
                    <div className="rounded-xl border border-coral/30 bg-coral/5 px-3 py-2">
                      <div className="flex items-start gap-2">
                        <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-coral-dark" />
                        <div className="min-w-0">
                          <p className="text-[12px] font-semibold leading-5 text-coral-dark">
                            {oracleError.message}
                          </p>
                          {oracleError.remediation && (
                            <p className="mt-1 text-[11px] leading-5 text-cream-500">
                              {oracleError.remediation}
                            </p>
                          )}
                        </div>
                      </div>
                    </div>
                  ) : oracleAnswer ? (
                    <div className="space-y-2">
                      <p className="text-[12px] leading-5 text-cream-600">
                        {oracleAnswer.summary || oracleAnswer.answer}
                      </p>
                      {oracleAnswer.results.map((result) => (
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
              </section>
            )}
          </div>

          <section className="rounded-2xl border border-cream-200 bg-white p-5 xl:sticky xl:top-0 xl:self-start xl:max-h-[calc(100vh-7rem)] xl:overflow-y-auto">
            <h3 className="mb-3 text-[11px] font-semibold uppercase tracking-widest text-cream-500">
              Worker Settings
            </h3>
            {!selectedWorker ? (
              <p className="text-[13px] text-cream-400">Select a Worker.</p>
            ) : settingsLoading ? (
              <p className="text-[13px] text-cream-400">Loading Worker settings...</p>
            ) : !settings ? (
              <p className="text-[13px] text-cream-400">
                Worker settings could not be loaded. Sync Cloudflare and retry.
              </p>
            ) : !settings.readable ? (
              <p className="rounded-xl bg-amber/[0.08] px-3 py-2 text-[12px] font-semibold text-amber-dark">
                {settings.message || "Worker settings are not readable with the current token scope."}
              </p>
            ) : (
              <div className="space-y-6">
                {/* Env vars */}
                <div>
                  <p className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
                    Plain-text env vars
                  </p>
                  {!canRotateWithProject && (
                    <p className="mb-2 rounded-xl bg-amber/[0.08] px-3 py-2 text-[11px] font-semibold text-amber-dark">
                      {rotateReason}
                    </p>
                  )}
                  <div className="space-y-3">
                    {settings.plainText.map((binding) => {
                      const draft = envDrafts[binding.name] ?? "";
                      const dry = envDryRuns[binding.name];
                      const dryValue = envDryRunValue[binding.name];
                      const dryFresh = dry !== undefined && dryValue === draft;
                      const changed = draft !== (binding.text ?? "");
                      return (
                        <div key={binding.name} className="rounded-xl border border-cream-100 bg-cream-50 p-3">
                          <div className="flex items-center justify-between gap-2">
                            <p className="truncate font-mono text-[12px] font-semibold text-cream-800">
                              {binding.name}
                            </p>
                            <span className="shrink-0 rounded-lg bg-white px-2 py-0.5 text-[10px] font-semibold text-cream-500">
                              {binding.bindingType}
                            </span>
                          </div>
                          <input
                            value={draft}
                            onChange={(event) =>
                              setEnvDrafts((prev) => ({ ...prev, [binding.name]: event.target.value }))
                            }
                            disabled={isLoading}
                            spellCheck={false}
                            data-help-title="This is the plain-text value for this Worker variable."
                            data-help-lines="Editing here is a local draft and does not save to Cloudflare.|Preview change runs a dry run that shows the exact before and after.|Apply stays blocked until a dry run matches the current draft.|Do not put secrets in plain-text vars; use the Secrets section instead."
                            className="mt-2 w-full rounded-xl border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] text-cream-800 outline-none focus:border-terracotta/30 disabled:opacity-60"
                          />
                          <div className="mt-2 flex flex-wrap gap-2">
                            <button
                              type="button"
                              onClick={() => void runEnvDryRun(binding.name, draft)}
                              disabled={isLoading || !changed}
                              data-help-title="This previews the env change without writing."
                              data-help-lines="A dry run asks the backend exactly what the PATCH would do.|It shows changed values, preserved secrets and other bindings, and risks.|It does not modify Cloudflare.|Run it again after editing to refresh the preview."
                              className="rounded-xl border border-cream-200 bg-white px-3 py-1.5 text-[11px] font-semibold text-cream-600 hover:border-terracotta-200 hover:text-terracotta disabled:opacity-50"
                            >
                              Preview change
                            </button>
                            <button
                              type="button"
                              onClick={() => void applyEnv(binding.name, draft)}
                              disabled={isLoading || !canRotateWithProject || !dryFresh}
                              title={!canRotateWithProject ? rotateReason : !dryFresh ? "Preview the current value first." : undefined}
                              data-help-title="This writes the previewed plain-text env value to Cloudflare."
                              data-help-lines="This is a real PATCH of the Worker settings, not a dry run.|It only changes plain-text vars; secrets and other bindings are preserved.|It requires Workers Scripts Write and an action project for evidence.|Apply stays blocked until a dry run matches the current draft."
                              className="rounded-xl bg-terracotta px-3 py-1.5 text-[11px] font-semibold text-white disabled:opacity-50"
                            >
                              Apply
                            </button>
                          </div>
                          {dryFresh && <EnvDryRunResult result={dry} />}
                        </div>
                      );
                    })}
                    {settings.plainText.length === 0 && (
                      <p className="rounded-xl bg-cream-50 px-3 py-2 text-[11px] text-cream-400">
                        No plain-text env vars on this Worker.
                      </p>
                    )}
                  </div>

                  {/* Add new env var */}
                  <div className="mt-3 rounded-xl border border-dashed border-cream-200 bg-white p-3">
                    <div className="mb-2 flex items-center gap-2">
                      <Plus className="h-3.5 w-3.5 text-terracotta" />
                      <p className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
                        Add env var
                      </p>
                    </div>
                    <div className="grid grid-cols-1 gap-2 md:grid-cols-2">
                      <input
                        value={newEnvName}
                        onChange={(event) => {
                          setNewEnvName(event.target.value);
                          setNewEnvDryRun(null);
                          setNewEnvDryRunFor(null);
                        }}
                        disabled={isLoading}
                        placeholder="NEW_VAR_NAME"
                        spellCheck={false}
                        data-help-title="This is the name of a new plain-text Worker variable."
                        data-help-lines="The name is what the Worker code reads, for example FEATURE_FLAG.|Adding here is a draft until you preview and apply.|Use the Secrets section for private values, not this field.|Existing bindings with the same name would be overwritten on apply."
                        className="rounded-xl border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] text-cream-800 outline-none focus:border-terracotta/30 disabled:opacity-60"
                      />
                      <input
                        value={newEnvValue}
                        onChange={(event) => {
                          setNewEnvValue(event.target.value);
                          setNewEnvDryRun(null);
                          setNewEnvDryRunFor(null);
                        }}
                        disabled={isLoading}
                        placeholder="value"
                        spellCheck={false}
                        data-help-title="This is the plain-text value for the new variable."
                        data-help-lines="Plain-text values are visible in the Worker settings; do not store secrets here.|This is a draft until you preview and apply.|Preview shows the exact before and after.|Apply requires a matching dry run, write scope and an action project."
                        className="rounded-xl border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] text-cream-800 outline-none focus:border-terracotta/30 disabled:opacity-60"
                      />
                    </div>
                    <div className="mt-2 flex flex-wrap gap-2">
                      <button
                        type="button"
                        onClick={() => void runNewEnvDryRun()}
                        disabled={isLoading || !newEnvName.trim()}
                        className="rounded-xl border border-cream-200 bg-white px-3 py-1.5 text-[11px] font-semibold text-cream-600 hover:border-terracotta-200 hover:text-terracotta disabled:opacity-50"
                      >
                        Preview change
                      </button>
                      <button
                        type="button"
                        onClick={() => void applyNewEnv()}
                        disabled={
                          isLoading ||
                          !canRotateWithProject ||
                          !newEnvDryRunFor ||
                          newEnvDryRunFor.name !== newEnvName.trim() ||
                          newEnvDryRunFor.value !== newEnvValue
                        }
                        title={!canRotateWithProject ? rotateReason : undefined}
                        className="rounded-xl bg-terracotta px-3 py-1.5 text-[11px] font-semibold text-white disabled:opacity-50"
                      >
                        Apply
                      </button>
                    </div>
                    {newEnvDryRun &&
                      newEnvDryRunFor &&
                      newEnvDryRunFor.name === newEnvName.trim() &&
                      newEnvDryRunFor.value === newEnvValue && (
                        <EnvDryRunResult result={newEnvDryRun} />
                      )}
                  </div>
                </div>

                {/* Secrets */}
                <div>
                  <p className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
                    Secrets
                  </p>
                  <p className="mb-2 text-[11px] leading-5 text-cream-400">
                    Secret values are write-only — Cloudflare never returns them.
                  </p>
                  <div className="space-y-2">
                    {settings.secrets.map((secret) => (
                      <div
                        key={secret.name}
                        className="flex items-center justify-between gap-2 rounded-xl bg-cream-50 px-3 py-2"
                      >
                        <p className="truncate font-mono text-[12px] font-semibold text-cream-800">
                          {secret.name}
                        </p>
                        <span className="shrink-0 rounded-lg bg-white px-2 py-0.5 text-[10px] font-semibold text-cream-500">
                          {secret.bindingType}
                        </span>
                      </div>
                    ))}
                    {settings.secrets.length === 0 && (
                      <p className="rounded-xl bg-cream-50 px-3 py-2 text-[11px] text-cream-400">
                        No secret bindings reported on this Worker.
                      </p>
                    )}
                  </div>

                  <div className="mt-3 rounded-2xl border border-cream-100 bg-cream-50 p-4">
                    <div className="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
                      <div>
                        <h4 className="text-[13px] font-semibold text-cream-900">
                          Guarded secret rotation
                        </h4>
                        <p className="mt-1 text-[11px] leading-5 text-cream-500">
                          Requires Workers Scripts Write and an active action project. The secret value is never stored in project notes.
                        </p>
                      </div>
                      <span className={`w-fit rounded-full px-2 py-0.5 text-[10px] font-semibold ${
                        canRotateWithProject ? "bg-sage/10 text-sage-dark" : "bg-amber/[0.12] text-amber-dark"
                      }`}>
                        {canRotateWithProject ? "ready" : "blocked"}
                      </span>
                    </div>
                    {!canRotateWithProject && (
                      <p className="mt-3 rounded-xl bg-white px-3 py-2 text-[11px] font-semibold text-amber-dark">
                        {rotateReason}
                      </p>
                    )}
                    <div className="mt-3 grid grid-cols-1 gap-2 md:grid-cols-[minmax(0,0.7fr)_minmax(0,1fr)_auto]">
                      <input
                        value={workerSecretName}
                        onChange={(event) => setWorkerSecretName(event.target.value)}
                        placeholder="SECRET_BINDING"
                        data-help-title="A secret name is the Worker variable name."
                        data-help-lines="A secret is a private value a Worker reads without exposing it in code.|This field is the binding name, for example API_TOKEN or DATABASE_URL.|Do not paste the secret value here; this field is only the name.|The actual value is sent only to Cloudflare during the guarded rotation call."
                        spellCheck={false}
                        className="rounded-xl border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] text-cream-800 outline-none focus:border-terracotta/30"
                      />
                      <input
                        type="password"
                        value={workerSecretValue}
                        onChange={(event) => setWorkerSecretValue(event.target.value)}
                        placeholder="new secret value"
                        data-help-title="This is the new private secret value."
                        data-help-lines="A secret value is like a password for the Worker.|The app sends it to Cloudflare and does not store it in project notes.|Use a fresh value when rotating a leaked or expired key.|The Rotate button stays blocked until token scope and action project are ready."
                        autoComplete="off"
                        spellCheck={false}
                        className="rounded-xl border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] text-cream-800 outline-none focus:border-terracotta/30"
                      />
                      <button
                        type="button"
                        onClick={() => void rotateSelectedWorkerSecret()}
                        disabled={
                          isLoading ||
                          !canRotateWithProject ||
                          !workerSecretName.trim() ||
                          !workerSecretValue.trim()
                        }
                        data-help-title="This rotates one Cloudflare Worker secret."
                        data-help-lines="Rotate means replace the private value Cloudflare gives to the Worker.|It requires Workers Scripts Write and an action project for evidence.|The secret value is never written to Markdown, logs, or Oracle chunks.|If the token is temporary, assume it can expire and rotate/save a new profile when needed."
                        className="rounded-xl bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-50"
                      >
                        Rotate
                      </button>
                    </div>
                  </div>
                </div>

                {/* Smoke / Dry run */}
                <div>
                  <div className="flex items-center justify-between gap-3">
                    <div>
                      <p className="text-[10px] font-semibold uppercase tracking-widest text-cream-400">
                        Smoke / Dry run
                      </p>
                      <p className="mt-1 text-[11px] leading-5 text-cream-500">
                        Live read checks plus a simulated secret-rotation path. No Cloudflare write.
                      </p>
                    </div>
                    <button
                      onClick={() => void runWorkerSmoke()}
                      disabled={isLoading || !actionProject}
                      data-help-title="A dry smoke checks the path without changing secrets."
                      data-help-lines="It reads live Cloudflare state and simulates the dangerous part.|It prints the API equivalent, token kind, and whether secret rotation would be allowed.|It writes evidence to the selected project when successful.|Use this before real rotation or env writes."
                      className="flex w-fit shrink-0 items-center gap-1.5 rounded-xl bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-60"
                    >
                      <ClipboardCheck className="h-3.5 w-3.5" />
                      Run dry smoke
                    </button>
                  </div>
                  {smoke && (
                    <div className="mt-3 grid grid-cols-1 gap-4 lg:grid-cols-[0.9fr_1.1fr]">
                      <div className="rounded-xl bg-cream-50 p-4">
                        <h4 className="mb-3 text-[11px] font-semibold uppercase tracking-widest text-cream-500">
                          Result
                        </h4>
                        <div className="space-y-3 text-[12px]">
                          <Metric label="Status" value={smoke.status} />
                          <Metric label="Token kind" value={credentialKindLabel(smoke.credentialKind)} />
                          <Metric label="Workers read" value={String(smoke.resourceCount)} />
                          <Metric label="Secret rotation" value={smoke.canRotateWorkerSecret ? "would be allowed" : "blocked"} />
                          {smoke.blockedReason && (
                            <p className="rounded-xl bg-amber/[0.08] px-3 py-2 text-[11px] leading-5 text-amber-dark">
                              {smoke.blockedReason}
                            </p>
                          )}
                          <p className="text-[12px] leading-5 text-cream-600">{smoke.message}</p>
                        </div>
                      </div>
                      <div className="rounded-xl border border-cream-100 p-4">
                        <h4 className="mb-3 text-[11px] font-semibold uppercase tracking-widest text-cream-500">
                          API Equivalent
                        </h4>
                        <div className="space-y-2">
                          {smoke.apiEquivalent.map((line) => (
                            <code
                              key={line}
                              className="block rounded-lg bg-cream-50 px-3 py-2 text-[11px] text-cream-700"
                            >
                              {line}
                            </code>
                          ))}
                        </div>
                        {smoke.risks[0] && (
                          <div className="mt-4 space-y-1">
                            {smoke.risks.slice(0, 3).map((risk) => (
                              <p key={risk} className="text-[11px] leading-5 text-amber-dark">
                                {risk}
                              </p>
                            ))}
                          </div>
                        )}
                      </div>
                    </div>
                  )}
                </div>

                {workerActionMessage && (
                  <p className="rounded-xl bg-sage/10 px-3 py-2 text-[11px] font-semibold text-sage-dark">
                    {workerActionMessage}
                  </p>
                )}
              </div>
            )}
          </section>
        </div>
      )}

      {resourceType !== "workers" && resourceType !== "billing" && (
        <div className="grid grid-cols-1 gap-4 xl:grid-cols-[minmax(0,1.35fr)_minmax(320px,0.65fr)]">
          <section className="rounded-2xl border border-cream-200 bg-white p-5">
            <div className="mb-4 flex flex-col gap-3 lg:flex-row lg:items-end lg:justify-between">
              <div>
                <h3 className="text-[15px] font-semibold text-cream-900">
                  {resourceTabs.find((t) => t.id === resourceType)?.label} inventory
                </h3>
                <p className="mt-1 text-[12px] text-cream-500">
                  Live inventory from the pinned Aspis Bio account scope. Inspect-only.
                </p>
              </div>
              <label className="flex items-center gap-2 rounded-xl border border-cream-200 bg-white px-3 py-2 sm:w-72">
                <Search className="h-3.5 w-3.5 shrink-0 text-cream-400" />
                <input
                  value={resourceSearch}
                  onChange={(event) => setResourceSearch(event.target.value)}
                  placeholder="Search resources"
                  data-help-title="This searches the live Cloudflare inventory for this type."
                  data-help-lines="Search only filters resources already loaded by sync.|It does not call Cloudflare while you type.|Switch the selector to inspect a different resource type.|If results are wrong, refresh the inventory first."
                  className="min-w-0 flex-1 bg-transparent text-[12px] font-medium text-cream-800 outline-none placeholder:text-cream-400"
                />
              </label>
            </div>

            <div className="overflow-hidden rounded-xl border border-cream-100">
              {filteredResources.map((resource) => {
                const active = selectedResource?.id === resource.id;
                return (
                  <button
                    key={resource.id}
                    type="button"
                    onClick={() => setSelectedResourceId(resource.id)}
                    data-help-title={`${resource.name} is a Cloudflare ${resource.resourceType} resource.`}
                    data-help-lines="Selecting a resource opens its details and API equivalent on the right.|For Aspis Bio, use this to understand which resources exist before adding app actions.|Selection is read-only and does not modify Cloudflare.|If a resource looks wrong, check the pinned account scope and run sync again."
                    className={`grid w-full grid-cols-1 gap-1 border-b border-cream-50 px-3 py-2 text-left last:border-b-0 md:grid-cols-[minmax(0,1.25fr)_130px_minmax(0,0.85fr)] md:items-center ${
                      active ? "bg-terracotta/[0.06]" : "bg-white hover:bg-cream-50"
                    }`}
                  >
                    <div className="min-w-0">
                      <p className="truncate text-[12px] font-semibold text-cream-800">{resource.name}</p>
                      <p className="mt-0.5 truncate text-[11px] text-cream-400">{resource.description}</p>
                    </div>
                    <span className="w-fit rounded-lg bg-cream-50 px-2 py-1 text-[10px] font-semibold text-cream-500">
                      {resource.resourceType}
                    </span>
                    <div className="min-w-0 md:text-right">
                      <p className="truncate text-[11px] font-semibold text-cream-700">{resource.status}</p>
                      <p className="mt-0.5 truncate text-[10px] text-cream-400">
                        {resource.region || resource.updatedAt || "global"}
                      </p>
                    </div>
                  </button>
                );
              })}
              {filteredResources.length === 0 && (
                <p className="px-3 py-8 text-center text-[13px] text-cream-400">
                  No resources of this type. Sync Cloudflare or clear the search.
                </p>
              )}
            </div>
          </section>

          <section className="rounded-2xl border border-cream-200 bg-white p-5">
            <h3 className="mb-3 text-[11px] font-semibold uppercase tracking-widest text-cream-500">
              Resource Detail
            </h3>
            {selectedResource ? (
              <div className="space-y-4">
                <div>
                  <p className="text-[15px] font-semibold text-cream-900">{selectedResource.name}</p>
                  <p className="mt-1 text-[12px] leading-5 text-cream-500">
                    {selectedResource.description}
                  </p>
                </div>
                <div className="grid grid-cols-2 gap-3">
                  <Metric label="Service" value={resourceGroupLabel(selectedResource.serviceId)} />
                  <Metric label="Type" value={selectedResource.resourceType} />
                  <Metric label="Status" value={selectedResource.status} />
                  <Metric label="Region" value={selectedResource.region || "global"} />
                </div>
                <div>
                  <p className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
                    API Equivalent
                  </p>
                  <code className="block rounded-xl bg-cream-50 px-3 py-2 text-[11px] text-cream-700">
                    {resourceApiHint(selectedResource)}
                  </code>
                </div>
                <div>
                  <p className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
                    Metadata
                  </p>
                  <div className="space-y-2">
                    {selectedResource.metadata.map((line) => (
                      <p key={line} className="rounded-xl bg-cream-50 px-3 py-2 text-[11px] leading-5 text-cream-600">
                        {line}
                      </p>
                    ))}
                    {selectedResource.metadata.length === 0 && (
                      <p className="text-[12px] text-cream-400">No metadata returned by Cloudflare.</p>
                    )}
                  </div>
                </div>
                {selectedResource.docsUrl && (
                  <a
                    href={selectedResource.docsUrl}
                    target="_blank"
                    rel="noreferrer"
                    data-help-title="This opens the Cloudflare documentation for this resource type."
                    data-help-lines="The link points to the Cloudflare API/docs page for this resource type.|It opens in your default browser, outside the app.|It is read-only context, not a console action.|Per-type actions will arrive in a later phase."
                    className="inline-flex items-center gap-1.5 rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:border-terracotta-200 hover:text-terracotta"
                  >
                    <ExternalLink className="h-3.5 w-3.5" />
                    Open in Cloudflare console
                  </a>
                )}
                {/* Per-type safe-edit action panel below the inspect info.
                    Keyed by resource id so a resource switch fully remounts the
                    panel — local drafts/loaded state never leak across A→B. */}
                <ResourceActionPanel
                  key={selectedResource.id}
                  resourceType={resourceType}
                  resource={selectedResource}
                  canMutate={canRotateWithProject}
                  mutateReason={rotateReason}
                  isLoading={isLoading}
                  appendProjectEvidence={appendProjectEvidence}
                  fetchCloudflareAiGatewaySettings={fetchCloudflareAiGatewaySettings}
                  setCloudflareAiGatewaySettings={setCloudflareAiGatewaySettings}
                  cloudflareAutoragReindex={cloudflareAutoragReindex}
                  fetchCloudflareKvKeys={fetchCloudflareKvKeys}
                  fetchCloudflareKvValue={fetchCloudflareKvValue}
                  setCloudflareKvValue={setCloudflareKvValue}
                  deleteCloudflareKvValue={deleteCloudflareKvValue}
                  cloudflareD1Query={cloudflareD1Query}
                  fetchCloudflareR2Config={fetchCloudflareR2Config}
                  setCloudflareR2Lifecycle={setCloudflareR2Lifecycle}
                  setCloudflareR2Cors={setCloudflareR2Cors}
                />
              </div>
            ) : (
              <p className="text-[13px] text-cream-400">Select a Cloudflare resource.</p>
            )}
          </section>
        </div>
      )}

      {resourceType === "billing" && (
        <section className="rounded-2xl border border-cream-200 bg-white p-5">
          <div className="mb-4 flex items-center gap-2">
            <CreditCard className="h-4 w-4 text-terracotta" />
            <h3 className="text-[15px] font-semibold text-cream-900">Account billing</h3>
          </div>
          <p
            className="mb-4 rounded-xl bg-cream-50 px-3 py-2 text-[11px] leading-5 text-cream-500"
            data-help-title="Cloudflare billing is account-level only."
            data-help-lines="Cloudflare does not expose per-Worker euro cost through its API.|This page shows account plans and invoices, not per-resource spend.|Use it to confirm subscription level and recent charges.|For per-project cost attribution, track usage signals separately."
          >
            Per-worker € cost is not available from the Cloudflare API — this is account-level billing only.
          </p>

          {billingLoading ? (
            <p className="text-[13px] text-cream-400">Loading billing...</p>
          ) : !billing ? (
            <p className="text-[13px] text-cream-400">
              Billing could not be loaded. Sync Cloudflare and reopen this tab.
            </p>
          ) : !billing.readable ? (
            <p className="rounded-xl bg-amber/[0.08] px-3 py-2 text-[12px] font-semibold text-amber-dark">
              {billing.message || "Billing is not readable with the current token scope."}
            </p>
          ) : (
            <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
              <div>
                <p className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
                  Plans
                </p>
                <div className="space-y-2">
                  {billing.plans.map((plan, index) => (
                    <div key={plan.id ?? index} className="rounded-xl bg-cream-50 px-3 py-3">
                      <div className="flex items-center justify-between gap-2">
                        <p className="truncate text-[12px] font-semibold text-cream-800">
                          {plan.name || "Plan"}
                        </p>
                        <span className="shrink-0 rounded-lg bg-white px-2 py-0.5 text-[11px] font-semibold text-cream-600">
                          {plan.price !== null
                            ? `${plan.price} ${plan.currency ?? ""}${plan.frequency ? ` / ${plan.frequency}` : ""}`
                            : "—"}
                        </span>
                      </div>
                      {plan.componentSummary && (
                        <p className="mt-1 text-[11px] leading-5 text-cream-500">{plan.componentSummary}</p>
                      )}
                    </div>
                  ))}
                  {billing.plans.length === 0 && (
                    <p className="rounded-xl bg-cream-50 px-3 py-2 text-[11px] text-cream-400">
                      No plans reported.
                    </p>
                  )}
                </div>
              </div>
              <div>
                <p className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
                  Invoices
                </p>
                <div className="space-y-2">
                  {billing.invoices.map((invoice, index) => (
                    <div key={invoice.id ?? index} className="rounded-xl bg-cream-50 px-3 py-3">
                      <div className="flex items-center justify-between gap-2">
                        <p className="truncate text-[12px] font-semibold text-cream-800">
                          {invoice.occurredAt || "unknown date"}
                        </p>
                        <span className="shrink-0 rounded-lg bg-white px-2 py-0.5 text-[11px] font-semibold text-cream-600">
                          {invoice.amount !== null ? `${invoice.amount} ${invoice.currency ?? ""}` : "—"}
                        </span>
                      </div>
                      <p className="mt-1 text-[11px] text-cream-500">
                        {[invoice.kind, invoice.status].filter(Boolean).join(" / ") || "—"}
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
          )}
        </section>
      )}
    </div>
  );
}

function EnvDryRunResult({ result }: { result: CloudflareEnvDryRunResult }) {
  return (
    <div className="mt-3 grid grid-cols-1 gap-3 lg:grid-cols-2">
      <div className="rounded-xl bg-cream-50 p-3">
        <h5 className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
          Changes
        </h5>
        <div className="space-y-2">
          {result.changes.map((change) => (
            <div key={`${change.name}-${change.kind}`} className="rounded-lg bg-white px-3 py-2">
              <p className="truncate font-mono text-[11px] font-semibold text-cream-800">{change.name}</p>
              <p className="mt-0.5 truncate text-[10px] text-cream-400">
                {change.before ?? "(none)"} → {change.after}
              </p>
            </div>
          ))}
          {result.changes.length === 0 && (
            <p className="text-[11px] text-cream-400">No effective change.</p>
          )}
        </div>
        {(result.preservedSecrets.length > 0 || result.preservedOther.length > 0) && (
          <p className="mt-2 text-[10px] leading-4 text-cream-500">
            Preserved: {result.preservedSecrets.length} secret(s), {result.preservedOther.length} other binding(s).
          </p>
        )}
      </div>
      <div className="rounded-xl border border-cream-100 p-3">
        <h5 className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
          API Equivalent
        </h5>
        <div className="space-y-2">
          {result.apiEquivalent.map((line) => (
            <code key={line} className="block rounded-lg bg-cream-50 px-3 py-2 text-[11px] text-cream-700">
              {line}
            </code>
          ))}
        </div>
        {result.risks.length > 0 && (
          <div className="mt-3 space-y-1">
            {result.risks.map((risk) => (
              <p key={risk} className="text-[11px] leading-5 text-amber-dark">
                {risk}
              </p>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div
      className="min-w-0"
      data-help-title={`${label} is a Cloudflare readiness fact.`}
      data-help-lines="These facts summarize the currently synced Cloudflare account, Worker, resource, token, or write state.|For Aspis Bio, use them to verify scope and readiness before smoke tests, env writes, rotations, or agent launches.|A metric can be stale after token or account changes.|Run Sync Cloudflare when facts do not match the provider console."
    >
      <p className="text-[10px] font-semibold uppercase tracking-widest text-cream-400">{label}</p>
      <p className="mt-1 truncate text-[13px] font-semibold text-cream-800">{value}</p>
    </div>
  );
}

function DetailList({
  title,
  items,
  empty,
}: {
  title: string;
  items: string[];
  empty: string;
}) {
  return (
    <div
      data-help-title={`${title} explains part of the selected Worker.`}
      data-help-lines="Worker details show how traffic reaches edge code and which runtime metadata is known.|For Aspis Bio, use routes, runtime, and drift warnings before changing env or secrets.|Missing metadata is a warning to inspect the Worker directly in Cloudflare.|This list is read-only."
    >
      <p className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
        {title}
      </p>
      <div className="space-y-2">
        {items.map((item) => (
          <p key={item} className="rounded-xl bg-cream-50 px-3 py-2 text-[11px] leading-5 text-cream-600">
            {item}
          </p>
        ))}
        {items.length === 0 && (
          <p className="rounded-xl bg-cream-50 px-3 py-2 text-[11px] text-cream-400">
            {empty}
          </p>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Per-type safe-edit action panels (Phase 4 frontend).
//
// Each console resource id is `cloudflare:{serviceId}:{resourceType}:{rawId}`
// (see make_cloudflare_console_resource in providers.rs). The backend action
// commands want the bare CF resource id (gateway/instance/namespace/database)
// or the R2 bucket name — recover it by stripping the known prefix, which is
// robust even if the raw id itself contains a colon.
// ---------------------------------------------------------------------------

function cloudflareResourceRawId(resource: ProviderConsoleResourceSummary): string {
  const prefix = `cloudflare:${resource.serviceId}:${resource.resourceType}:`;
  if (resource.id.startsWith(prefix)) {
    return resource.id.slice(prefix.length);
  }
  // Fallback when the exact prefix did not match (e.g. resourceType drift):
  // the id is `cloudflare:{serviceId}:{resourceType}:{rawId}`, so recover the
  // raw id as everything AFTER the 3rd colon. This preserves a rawId that
  // itself contains colons, unlike a lastIndexOf-based last-segment grab.
  let colonsSeen = 0;
  for (let i = 0; i < resource.id.length; i += 1) {
    if (resource.id[i] === ":") {
      colonsSeen += 1;
      if (colonsSeen === 3) {
        return resource.id.slice(i + 1);
      }
    }
  }
  return resource.name;
}

interface ResourceActionPanelProps {
  resourceType: ResourceType;
  resource: ProviderConsoleResourceSummary;
  canMutate: boolean;
  mutateReason: string;
  isLoading: boolean;
  appendProjectEvidence: (text: string, successMessage: string) => Promise<boolean>;
  fetchCloudflareAiGatewaySettings: (
    gatewayId: string,
  ) => Promise<CloudflareAiGatewaySettings | null>;
  setCloudflareAiGatewaySettings: (
    gatewayId: string,
    patch: CloudflareAiGatewaySettingsPatch,
  ) => Promise<CloudflareAiGatewaySettings | null>;
  cloudflareAutoragReindex: (
    instanceId: string,
  ) => Promise<CloudflareAutoragReindexResult | null>;
  fetchCloudflareKvKeys: (
    namespaceId: string,
    prefix?: string,
  ) => Promise<{ keys: CloudflareKvKey[]; listComplete: boolean } | null>;
  fetchCloudflareKvValue: (
    namespaceId: string,
    key: string,
  ) => Promise<CloudflareKvValue | null>;
  setCloudflareKvValue: (
    namespaceId: string,
    key: string,
    value: string,
  ) => Promise<{ message: string; writtenAt: string } | null>;
  deleteCloudflareKvValue: (
    namespaceId: string,
    key: string,
    confirmKey: string,
  ) => Promise<{ message: string; writtenAt: string } | null>;
  cloudflareD1Query: (
    databaseId: string,
    sql: string,
    confirm: boolean,
  ) => Promise<CloudflareD1QueryResult | null>;
  fetchCloudflareR2Config: (bucket: string) => Promise<CloudflareR2Config | null>;
  setCloudflareR2Lifecycle: (
    bucket: string,
    rules: unknown,
  ) => Promise<{ message: string; writtenAt: string } | null>;
  setCloudflareR2Cors: (
    bucket: string,
    rules: unknown,
  ) => Promise<{ message: string; writtenAt: string } | null>;
}

function ResourceActionPanel(props: ResourceActionPanelProps) {
  const { resourceType, resource } = props;
  const rawId = cloudflareResourceRawId(resource);
  switch (resourceType) {
    case "ai-gateway":
      return <AiGatewayPanel {...props} gatewayId={rawId} />;
    case "ai-search":
      return <AutoRagPanel {...props} instanceId={rawId} />;
    case "kv":
      return <KvPanel {...props} namespaceId={rawId} />;
    case "d1":
      return <D1Panel {...props} databaseId={rawId} />;
    case "r2":
      // The backend write/read commands key R2 by bucket NAME.
      return <R2Panel {...props} bucket={resource.name} />;
    default:
      return (
        <p className="rounded-xl border border-dashed border-cream-200 px-3 py-2 text-[11px] text-cream-400">
          This resource type is inspect-only — no safe-edit actions are exposed.
        </p>
      );
  }
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

// --- 1. AI Gateway ---------------------------------------------------------

const RATE_LIMIT_TECHNIQUES = ["fixed", "sliding"] as const;

function AiGatewayPanel({
  gatewayId,
  canMutate,
  mutateReason,
  isLoading,
  appendProjectEvidence,
  fetchCloudflareAiGatewaySettings,
  setCloudflareAiGatewaySettings,
}: ResourceActionPanelProps & { gatewayId: string }) {
  const [loaded, setLoaded] = useState<CloudflareAiGatewaySettings | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const requestId = useRef(0);

  // Per-field drafts. Numbers are kept as strings so an empty field is
  // representable and never coerces to 0; null fields render as empty.
  const [cacheTtl, setCacheTtl] = useState("");
  const [cacheInvalidate, setCacheInvalidate] = useState(false);
  const [collectLogs, setCollectLogs] = useState(false);
  const [rlInterval, setRlInterval] = useState("");
  const [rlLimit, setRlLimit] = useState("");
  const [rlTechnique, setRlTechnique] = useState("");

  const hydrate = (s: CloudflareAiGatewaySettings) => {
    setCacheTtl(s.cacheTtl == null ? "" : String(s.cacheTtl));
    setCacheInvalidate(Boolean(s.cacheInvalidateOnUpdate));
    setCollectLogs(Boolean(s.collectLogs));
    setRlInterval(s.rateLimitingInterval == null ? "" : String(s.rateLimitingInterval));
    setRlLimit(s.rateLimitingLimit == null ? "" : String(s.rateLimitingLimit));
    setRlTechnique(s.rateLimitingTechnique ?? "");
  };

  useEffect(() => {
    const id = requestId.current + 1;
    requestId.current = id;
    setLoading(true);
    setLoaded(null);
    setMessage(null);
    void fetchCloudflareAiGatewaySettings(gatewayId)
      .then((s) => {
        if (requestId.current !== id) return;
        setLoaded(s);
        if (s?.readable) hydrate(s);
      })
      .finally(() => {
        if (requestId.current === id) setLoading(false);
      });
    return () => {
      requestId.current += 1;
    };
  }, [gatewayId, fetchCloudflareAiGatewaySettings]);

  const numOrNull = (raw: string): number | null => {
    const trimmed = raw.trim();
    if (!trimmed) return null;
    const n = Number(trimmed);
    return Number.isFinite(n) ? n : null;
  };

  // Build a patch containing ONLY the fields the user actually changed.
  const buildPatch = (s: CloudflareAiGatewaySettings): CloudflareAiGatewaySettingsPatch => {
    const patch: CloudflareAiGatewaySettingsPatch = {};
    const draftTtl = numOrNull(cacheTtl);
    if (draftTtl !== (s.cacheTtl ?? null)) patch.cacheTtl = draftTtl;
    if (cacheInvalidate !== Boolean(s.cacheInvalidateOnUpdate))
      patch.cacheInvalidateOnUpdate = cacheInvalidate;
    if (collectLogs !== Boolean(s.collectLogs)) patch.collectLogs = collectLogs;
    const draftInterval = numOrNull(rlInterval);
    if (draftInterval !== (s.rateLimitingInterval ?? null))
      patch.rateLimitingInterval = draftInterval;
    const draftLimit = numOrNull(rlLimit);
    if (draftLimit !== (s.rateLimitingLimit ?? null)) patch.rateLimitingLimit = draftLimit;
    const draftTechnique = rlTechnique.trim() ? rlTechnique.trim() : null;
    if (draftTechnique !== (s.rateLimitingTechnique ?? null))
      patch.rateLimitingTechnique = draftTechnique;
    return patch;
  };

  const dirtyCount = loaded?.readable ? Object.keys(buildPatch(loaded)).length : 0;

  const save = async () => {
    if (!loaded?.readable || !canMutate) return;
    const patch = buildPatch(loaded);
    const changedFields = Object.keys(patch);
    if (changedFields.length === 0) return;
    const id = requestId.current + 1;
    requestId.current = id;
    setSaving(true);
    setMessage(null);
    try {
      const updated = await setCloudflareAiGatewaySettings(gatewayId, patch);
      if (requestId.current !== id) return;
      if (updated) {
        setLoaded(updated);
        if (updated.readable) hydrate(updated);
        setMessage(updated.message || "AI Gateway settings saved.");
        // Record an audit note on the active action project (field NAMES only,
        // never the values — some toggles relate to logging prompt content).
        await appendProjectEvidence(
          aiGatewayUpdateEvidenceText(gatewayId, changedFields, updated),
          "AI Gateway update evidence attached to the selected project.",
        );
      }
    } finally {
      if (requestId.current === id) setSaving(false);
    }
  };

  return (
    <PanelShell
      title="AI Gateway settings"
      subtitle="Caching, request logging and rate limiting for this gateway. Editing is a local draft until you save."
    >
      {loading ? (
        <p className="text-[12px] text-cream-400">Loading gateway settings...</p>
      ) : !loaded ? (
        <p className="text-[12px] text-cream-400">
          Gateway settings could not be loaded. Sync Cloudflare and reselect.
        </p>
      ) : !loaded.readable ? (
        <p className="rounded-xl bg-amber/[0.08] px-3 py-2 text-[12px] font-semibold text-amber-dark">
          {loaded.message || "Gateway settings are not readable with the current token scope."}
        </p>
      ) : (
        <div className="space-y-3">
          {!canMutate && <MutateBlockedNote reason={mutateReason} />}
          <NumberField
            label="Cache TTL (seconds)"
            value={cacheTtl}
            onChange={setCacheTtl}
            disabled={isLoading || saving}
            placeholder="disabled"
            help="How long Cloudflare caches identical AI responses. Empty disables caching."
          />
          <ToggleField
            label="Invalidate cache on update"
            checked={cacheInvalidate}
            onChange={setCacheInvalidate}
            disabled={isLoading || saving}
            help="Drop cached responses when the upstream model or config changes."
          />
          <ToggleField
            label="Collect logs"
            checked={collectLogs}
            onChange={setCollectLogs}
            disabled={isLoading || saving}
            help="Store request/response logs in Cloudflare. Turning this on persists prompt and completion content in Cloudflare."
          />
          <NumberField
            label="Rate limit interval (seconds)"
            value={rlInterval}
            onChange={setRlInterval}
            disabled={isLoading || saving}
            placeholder="disabled"
            help="Length of each rate-limit window. Empty disables rate limiting."
          />
          <NumberField
            label="Rate limit (requests / interval)"
            value={rlLimit}
            onChange={setRlLimit}
            disabled={isLoading || saving}
            placeholder="disabled"
            help="Maximum requests allowed in each interval window."
          />
          <SelectField
            label="Rate limit technique"
            value={rlTechnique}
            onChange={setRlTechnique}
            disabled={isLoading || saving}
            options={[
              { value: "", label: "unset" },
              ...RATE_LIMIT_TECHNIQUES.map((t) => ({ value: t, label: t })),
            ]}
            help="fixed counts per discrete window; sliding smooths bursts across the window edge."
          />
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={() => void save()}
              disabled={isLoading || saving || !canMutate || dirtyCount === 0}
              title={!canMutate ? mutateReason : dirtyCount === 0 ? "No changes to save." : undefined}
              className="flex items-center gap-1.5 rounded-xl bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-50"
            >
              <Save className="h-3.5 w-3.5" />
              Save settings
            </button>
            {dirtyCount > 0 && (
              <span className="text-[11px] font-semibold text-cream-500">
                {dirtyCount} field(s) changed
              </span>
            )}
          </div>
          {message && (
            <p className="rounded-xl bg-sage/10 px-3 py-2 text-[11px] font-semibold text-sage-dark">
              {message}
            </p>
          )}
        </div>
      )}
    </PanelShell>
  );
}

// --- 2. AI Search / AutoRAG ------------------------------------------------

function AutoRagPanel({
  instanceId,
  resource,
  canMutate,
  mutateReason,
  isLoading,
  appendProjectEvidence,
  cloudflareAutoragReindex,
}: ResourceActionPanelProps & { instanceId: string }) {
  const [confirming, setConfirming] = useState(false);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<CloudflareAutoragReindexResult | null>(null);
  const requestId = useRef(0);

  // Reindex is an INSTANCE-only action. The "ai-search" tab also groups
  // "AI Search Namespace" resources, which have no reindex command — selecting
  // one must be inspect-only, never offering (or calling) the reindex action.
  const isInstance = resource.resourceType === "AI Search Instance";

  const trigger = async () => {
    if (!canMutate || !isInstance) return;
    const id = requestId.current + 1;
    requestId.current = id;
    setRunning(true);
    setConfirming(false);
    try {
      const r = await cloudflareAutoragReindex(instanceId);
      if (requestId.current !== id) return;
      if (r) {
        setResult(r);
        // Record an audit note on the active action project. Instance id +
        // async job id only — no source data or content is included.
        await appendProjectEvidence(
          autoragReindexEvidenceText(instanceId, r),
          "AI Search reindex evidence attached to the selected project.",
        );
      }
    } finally {
      if (requestId.current === id) setRunning(false);
    }
  };

  // Keyed to instanceId so a reused panel (without remount) also discards any
  // in-flight response that belongs to the previous instance.
  useEffect(() => {
    return () => {
      requestId.current += 1;
    };
  }, [instanceId]);

  if (!isInstance) {
    return (
      <PanelShell
        title="Reindex / Sync"
        subtitle="AI Search namespaces are inspect-only here."
      >
        <p className="rounded-xl border border-dashed border-cream-200 px-3 py-2 text-[11px] text-cream-500">
          Reindex applies to AI Search instances, not namespaces. Select an
          instance to trigger a sync.
        </p>
      </PanelShell>
    );
  }

  return (
    <PanelShell
      title="Reindex / Sync"
      subtitle="Trigger a full reindex of this AI Search (AutoRAG) instance. It runs asynchronously on Cloudflare."
    >
      {!canMutate && <MutateBlockedNote reason={mutateReason} />}
      {!confirming ? (
        <button
          type="button"
          onClick={() => setConfirming(true)}
          disabled={isLoading || running || !canMutate}
          title={!canMutate ? mutateReason : undefined}
          className="flex items-center gap-1.5 rounded-xl bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-50"
        >
          <RefreshCw className="h-3.5 w-3.5" />
          Reindex / Sync
        </button>
      ) : (
        <div className="rounded-xl border border-cream-100 bg-white p-3">
          <p className="text-[12px] font-semibold text-cream-800">Trigger reindex?</p>
          <p className="mt-1 text-[11px] leading-5 text-cream-500">
            This re-scans the source data and rebuilds the index. It is a trigger, not destructive, and runs in the background.
          </p>
          <div className="mt-3 flex items-center gap-2">
            <button
              type="button"
              onClick={() => void trigger()}
              disabled={isLoading || running}
              className="rounded-xl bg-terracotta px-3 py-1.5 text-[11px] font-semibold text-white disabled:opacity-50"
            >
              Trigger reindex
            </button>
            <button
              type="button"
              onClick={() => setConfirming(false)}
              disabled={running}
              className="rounded-xl border border-cream-200 bg-white px-3 py-1.5 text-[11px] font-semibold text-cream-600 hover:border-terracotta-200 hover:text-terracotta disabled:opacity-50"
            >
              Cancel
            </button>
          </div>
        </div>
      )}
      {result && (
        <div className="mt-3 rounded-xl bg-white px-3 py-2">
          <Metric label="Job" value={result.jobId || "queued"} />
          <p className="mt-2 text-[11px] leading-5 text-cream-600">{result.message}</p>
          <p className="mt-1 text-[10px] text-cream-400">
            Triggered at {result.triggeredAt}. Reindex runs asynchronously — check Cloudflare for progress.
          </p>
        </div>
      )}
    </PanelShell>
  );
}

// --- 3. KV -----------------------------------------------------------------

function KvPanel({
  namespaceId,
  canMutate,
  mutateReason,
  isLoading,
  fetchCloudflareKvKeys,
  fetchCloudflareKvValue,
  setCloudflareKvValue,
  deleteCloudflareKvValue,
}: ResourceActionPanelProps & { namespaceId: string }) {
  const [prefix, setPrefix] = useState("");
  const [keys, setKeys] = useState<CloudflareKvKey[]>([]);
  const [listComplete, setListComplete] = useState(true);
  const [keysLoading, setKeysLoading] = useState(false);
  const [keysLoaded, setKeysLoaded] = useState(false);

  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [valueLoading, setValueLoading] = useState(false);
  const [valueDraft, setValueDraft] = useState("");
  const [truncated, setTruncated] = useState(false);
  // A value fetch that returns null (backend error / out of scope) must NOT be
  // presented as an empty editable string — saving it would overwrite the real
  // stored value with "". Track the load outcome and gate Save on it.
  const [valueLoaded, setValueLoaded] = useState(false);
  const [valueLoadError, setValueLoadError] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  // Type-the-exact-key confirm for the destructive delete.
  const [deleteConfirm, setDeleteConfirm] = useState("");
  const [deleting, setDeleting] = useState(false);
  const [saving, setSaving] = useState(false);

  const keysRequestId = useRef(0);
  const valueRequestId = useRef(0);
  // Delete owns a SEPARATE request id from selectKey/saveValue. Sharing
  // valueRequestId meant selecting another key while a delete was in flight
  // bumped the id and cancelled the delete's success state — the deleted key
  // lingered with no confirmation. The delete now guards only on its own id.
  const deleteRequestId = useRef(0);
  // Mirror of selectedKey so async delete-success handling can tell whether the
  // user is still on the deleted key WITHOUT a stale closure or a setter inside
  // another setter's updater.
  const selectedKeyRef = useRef<string | null>(null);
  selectedKeyRef.current = selectedKey;

  // Reset everything if the namespace remounts (handled by key) — also clean
  // up in-flight guards on unmount.
  useEffect(() => {
    return () => {
      keysRequestId.current += 1;
      valueRequestId.current += 1;
      deleteRequestId.current += 1;
    };
  }, []);

  const loadKeys = async () => {
    const id = keysRequestId.current + 1;
    keysRequestId.current = id;
    setKeysLoading(true);
    setMessage(null);
    try {
      const page = await fetchCloudflareKvKeys(namespaceId, prefix.trim() || undefined);
      if (keysRequestId.current !== id) return;
      if (page) {
        setKeys(page.keys);
        setListComplete(page.listComplete);
        setKeysLoaded(true);
      }
    } finally {
      if (keysRequestId.current === id) setKeysLoading(false);
    }
  };

  const selectKey = async (key: string) => {
    setSelectedKey(key);
    setDeleteConfirm("");
    setMessage(null);
    const id = valueRequestId.current + 1;
    valueRequestId.current = id;
    setValueLoading(true);
    setValueDraft("");
    setTruncated(false);
    setValueLoaded(false);
    setValueLoadError(false);
    try {
      const v = await fetchCloudflareKvValue(namespaceId, key);
      if (valueRequestId.current !== id) return;
      if (v) {
        setValueDraft(v.value);
        setTruncated(v.truncated);
        setValueLoaded(true);
      } else {
        // Backend error / out of scope: never present an empty editable value.
        setValueLoadError(true);
      }
    } finally {
      if (valueRequestId.current === id) setValueLoading(false);
    }
  };

  const saveValue = async () => {
    if (!selectedKey || !canMutate || truncated || !valueLoaded) return;
    const id = valueRequestId.current + 1;
    valueRequestId.current = id;
    setSaving(true);
    setMessage(null);
    try {
      const r = await setCloudflareKvValue(namespaceId, selectedKey, valueDraft);
      if (valueRequestId.current !== id) return;
      if (r) setMessage(r.message || `Value written at ${r.writtenAt}`);
    } finally {
      if (valueRequestId.current === id) setSaving(false);
    }
  };

  const deleteKey = async () => {
    if (!selectedKey || !canMutate || deleteConfirm !== selectedKey) return;
    const deletedKey = selectedKey;
    // Delete owns its own request id so navigating to another key (which bumps
    // valueRequestId) cannot cancel this delete's success handling.
    const id = deleteRequestId.current + 1;
    deleteRequestId.current = id;
    setDeleting(true);
    setMessage(null);
    try {
      const r = await deleteCloudflareKvValue(namespaceId, deletedKey, deleteConfirm);
      if (deleteRequestId.current !== id) return;
      if (r) {
        // Always reflect the removal and show a confirmation, even if the user
        // navigated to another key while the delete was in flight.
        setMessage(r.message || `Key "${deletedKey}" deleted at ${r.writtenAt}`);
        setKeys((prev) => prev.filter((k) => k.name !== deletedKey));
        // Only collapse the value editor if the user is STILL on the deleted
        // key; if they navigated to another key, selectKey already reset these
        // and we must not clobber the new key's draft/confirm.
        if (selectedKeyRef.current === deletedKey) {
          setSelectedKey(null);
          setValueDraft("");
          setDeleteConfirm("");
          setValueLoaded(false);
          setValueLoadError(false);
        }
      }
    } finally {
      if (deleteRequestId.current === id) setDeleting(false);
    }
  };

  return (
    <PanelShell
      title="KV key browser"
      subtitle="Search keys, inspect and edit values. Values are not logged. Writes and deletes require an active action project."
    >
      {!canMutate && <MutateBlockedNote reason={mutateReason} />}
      <div className="flex items-center gap-2">
        <label className="flex flex-1 items-center gap-2 rounded-xl border border-cream-200 bg-white px-3 py-2">
          <Search className="h-3.5 w-3.5 shrink-0 text-cream-400" />
          <input
            value={prefix}
            onChange={(event) => setPrefix(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void loadKeys();
            }}
            placeholder="key prefix (optional)"
            spellCheck={false}
            className="min-w-0 flex-1 bg-transparent font-mono text-[12px] text-cream-800 outline-none placeholder:text-cream-400"
          />
        </label>
        <button
          type="button"
          onClick={() => void loadKeys()}
          disabled={isLoading || keysLoading}
          className="rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:border-terracotta-200 hover:text-terracotta disabled:opacity-50"
        >
          {keysLoading ? "Loading..." : "List keys"}
        </button>
      </div>

      {keysLoaded && (
        <div className="mt-3 max-h-48 space-y-1 overflow-y-auto rounded-xl border border-cream-100 bg-white p-1">
          {keys.map((k) => (
            <button
              key={k.name}
              type="button"
              onClick={() => void selectKey(k.name)}
              className={`block w-full truncate rounded-lg px-2 py-1.5 text-left font-mono text-[11px] ${
                selectedKey === k.name
                  ? "bg-terracotta/[0.08] text-cream-900"
                  : "text-cream-700 hover:bg-cream-50"
              }`}
            >
              {k.name}
            </button>
          ))}
          {keys.length === 0 && (
            <p className="px-2 py-3 text-center text-[11px] text-cream-400">
              No keys match this prefix.
            </p>
          )}
          {!listComplete && (
            <p className="px-2 py-1 text-[10px] text-cream-400">
              List truncated — narrow the prefix to see more keys.
            </p>
          )}
        </div>
      )}

      {selectedKey && (
        <div className="mt-3 rounded-xl border border-cream-100 bg-white p-3">
          <p className="truncate font-mono text-[12px] font-semibold text-cream-800">
            {selectedKey}
          </p>
          {valueLoading ? (
            <p className="mt-2 text-[12px] text-cream-400">Loading value...</p>
          ) : (
            <>
              {valueLoadError && (
                <p className="mt-2 rounded-xl bg-coral/5 px-3 py-2 text-[11px] font-semibold text-coral-dark">
                  Value could not be loaded (backend error or insufficient token
                  scope). Editing is disabled so an empty value is never written
                  over the stored one. Reselect the key to retry.
                </p>
              )}
              {truncated && (
                <p className="mt-2 rounded-xl bg-amber/[0.08] px-3 py-2 text-[11px] font-semibold text-amber-dark">
                  Value exceeds 64 KiB and was truncated for display. Editing is disabled to avoid corrupting the stored value.
                </p>
              )}
              {!valueLoadError && (
                <>
                  <textarea
                    value={valueDraft}
                    onChange={(event) => setValueDraft(event.target.value)}
                    disabled={isLoading || saving || truncated || !valueLoaded}
                    spellCheck={false}
                    rows={5}
                    className="mt-2 w-full rounded-xl border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] text-cream-800 outline-none focus:border-terracotta/30 disabled:opacity-60"
                  />
                  <div className="mt-2 flex flex-wrap items-center gap-2">
                    <button
                      type="button"
                      onClick={() => void saveValue()}
                      disabled={isLoading || saving || !canMutate || truncated || !valueLoaded}
                      title={
                        !canMutate
                          ? mutateReason
                          : truncated
                            ? "Truncated value cannot be saved."
                            : !valueLoaded
                              ? "Value did not load — nothing safe to save."
                              : undefined
                      }
                      className="flex items-center gap-1.5 rounded-xl bg-terracotta px-3 py-1.5 text-[11px] font-semibold text-white disabled:opacity-50"
                    >
                      <Save className="h-3.5 w-3.5" />
                      Save value
                    </button>
                  </div>
                </>
              )}

              {/* Destructive delete — type the exact key to confirm. */}
              <div className="mt-3 rounded-xl border border-coral/30 bg-coral/5 p-3">
                <p className="text-[11px] font-semibold text-coral-dark">Delete key</p>
                <p className="mt-1 text-[11px] leading-5 text-cream-500">
                  Type the exact key name to confirm. This permanently removes the key and its value.
                </p>
                <input
                  value={deleteConfirm}
                  onChange={(event) => setDeleteConfirm(event.target.value)}
                  disabled={isLoading || deleting || !canMutate}
                  placeholder={selectedKey}
                  spellCheck={false}
                  className="mt-2 w-full rounded-xl border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] text-cream-800 outline-none focus:border-coral/40 disabled:opacity-60"
                />
                <button
                  type="button"
                  onClick={() => void deleteKey()}
                  disabled={
                    isLoading || deleting || !canMutate || deleteConfirm !== selectedKey
                  }
                  title={
                    !canMutate
                      ? mutateReason
                      : deleteConfirm !== selectedKey
                        ? "Type the exact key name to enable delete."
                        : undefined
                  }
                  className="mt-2 flex items-center gap-1.5 rounded-xl bg-coral px-3 py-1.5 text-[11px] font-semibold text-white disabled:opacity-50"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                  Delete key
                </button>
              </div>
            </>
          )}
        </div>
      )}

      {message && (
        <p className="mt-3 rounded-xl bg-sage/10 px-3 py-2 text-[11px] font-semibold text-sage-dark">
          {message}
        </p>
      )}
    </PanelShell>
  );
}

// --- 4. D1 -----------------------------------------------------------------

function D1Panel({
  databaseId,
  canMutate,
  mutateReason,
  isLoading,
  cloudflareD1Query,
}: ResourceActionPanelProps & { databaseId: string }) {
  const [sql, setSql] = useState("");
  const [result, setResult] = useState<CloudflareD1QueryResult | null>(null);
  const [running, setRunning] = useState(false);
  // SQL the pending confirmation belongs to, so editing the textarea after a
  // write probe re-hides the confirm button (no confirming a stale statement).
  const [pendingSql, setPendingSql] = useState<string | null>(null);
  const requestId = useRef(0);

  useEffect(() => {
    return () => {
      requestId.current += 1;
    };
  }, []);

  const run = async (confirm: boolean) => {
    const sqlSnapshot = sql;
    if (!sqlSnapshot.trim()) return;
    // A confirmed (mutating) run requires the action-project gate.
    if (confirm && !canMutate) return;
    const id = requestId.current + 1;
    requestId.current = id;
    setRunning(true);
    try {
      const r = await cloudflareD1Query(databaseId, sqlSnapshot, confirm);
      if (requestId.current !== id) return;
      if (r) {
        setResult(r);
        setPendingSql(r.requiresConfirmation ? sqlSnapshot : null);
      }
    } finally {
      if (requestId.current === id) setRunning(false);
    }
  };

  const needsConfirm =
    result?.requiresConfirmation === true && pendingSql !== null && pendingSql === sql;

  return (
    <PanelShell
      title="D1 query"
      subtitle="Run SQL against this database. Reads execute immediately; writes require an explicit confirm step. Row data is not logged."
    >
      <textarea
        value={sql}
        onChange={(event) => {
          setSql(event.target.value);
          // Invalidate any pending write confirmation for the old SQL.
          if (pendingSql !== null && event.target.value !== pendingSql) {
            setPendingSql(null);
          }
        }}
        disabled={isLoading || running}
        spellCheck={false}
        rows={4}
        placeholder="SELECT * FROM ... LIMIT 50"
        className="w-full rounded-xl border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] text-cream-800 outline-none focus:border-terracotta/30 disabled:opacity-60"
      />
      <div className="mt-2 flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={() => void run(false)}
          disabled={isLoading || running || !sql.trim()}
          className="flex items-center gap-1.5 rounded-xl bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-50"
        >
          <Play className="h-3.5 w-3.5" />
          Run
        </button>
      </div>

      {needsConfirm && (
        <div className="mt-3 rounded-xl border border-amber/40 bg-amber/[0.08] p-3">
          <div className="flex items-start gap-2">
            <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-amber-dark" />
            <div className="min-w-0">
              <p className="text-[12px] font-semibold text-amber-dark">
                This statement modifies data
              </p>
              <p className="mt-1 text-[11px] leading-5 text-cream-600">
                {result?.message ||
                  "The statement was detected as a write and was NOT executed. Confirm to run it."}
              </p>
            </div>
          </div>
          {!canMutate && <p className="mt-2 text-[11px] font-semibold text-amber-dark">{mutateReason}</p>}
          <button
            type="button"
            onClick={() => void run(true)}
            disabled={isLoading || running || !canMutate || !needsConfirm}
            title={!canMutate ? mutateReason : undefined}
            className="mt-2 rounded-xl bg-coral px-3 py-1.5 text-[11px] font-semibold text-white disabled:opacity-50"
          >
            Run anyway / Confirm
          </button>
        </div>
      )}

      {result && result.executed && (
        <div className="mt-3 space-y-2">
          <div className="flex flex-wrap gap-3">
            <Metric label="Rows" value={String(result.rowCount)} />
            {result.rowsRead != null && <Metric label="Rows read" value={String(result.rowsRead)} />}
            {result.rowsWritten != null && (
              <Metric label="Rows written" value={String(result.rowsWritten)} />
            )}
          </div>
          {result.columns.length > 0 ? (
            <div className="overflow-x-auto rounded-xl border border-cream-100">
              <table className="w-full border-collapse text-left text-[11px]">
                <thead>
                  <tr className="bg-cream-50">
                    {result.columns.map((col) => (
                      <th
                        key={col}
                        className="border-b border-cream-100 px-2 py-1.5 font-mono font-semibold text-cream-600"
                      >
                        {col}
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {result.rows.map((row, rIdx) => (
                    <tr key={rIdx} className="odd:bg-white even:bg-cream-50/50">
                      {row.map((cell, cIdx) => (
                        <td
                          key={cIdx}
                          className="border-b border-cream-50 px-2 py-1.5 font-mono text-cream-700"
                        >
                          {cell}
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            <p className="rounded-xl bg-cream-50 px-3 py-2 text-[11px] text-cream-500">
              {result.message || "Statement executed. No rows returned."}
            </p>
          )}
          {result.truncated && (
            <p className="text-[10px] text-cream-400">
              Result set truncated for display.
            </p>
          )}
        </div>
      )}
    </PanelShell>
  );
}

// --- 5. R2 -----------------------------------------------------------------

function R2Panel({
  bucket,
  canMutate,
  mutateReason,
  isLoading,
  fetchCloudflareR2Config,
  setCloudflareR2Lifecycle,
  setCloudflareR2Cors,
}: ResourceActionPanelProps & { bucket: string }) {
  const [config, setConfig] = useState<CloudflareR2Config | null>(null);
  const [loading, setLoading] = useState(true);
  const [lifecycleDraft, setLifecycleDraft] = useState("");
  const [corsDraft, setCorsDraft] = useState("");
  const [lifecycleError, setLifecycleError] = useState<string | null>(null);
  const [corsError, setCorsError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [savingLifecycle, setSavingLifecycle] = useState(false);
  const [savingCors, setSavingCors] = useState(false);
  // Independent request-id refs. The lifecycle and CORS saves MUST NOT share a
  // ref: clicking "Save lifecycle" then "Save CORS" before the first resolves
  // would otherwise bump the shared ref, making the lifecycle call's `finally`
  // see a stale id and skip `setSavingLifecycle(false)` — stranding that button
  // disabled forever. The load effect also gets its own ref.
  const loadRequestId = useRef(0);
  const lifecycleRequestId = useRef(0);
  const corsRequestId = useRef(0);

  const hydrate = (c: CloudflareR2Config) => {
    setLifecycleDraft(
      c.lifecycleReadable ? JSON.stringify(c.lifecycleRules ?? [], null, 2) : "",
    );
    setCorsDraft(c.corsReadable ? JSON.stringify(c.corsRules ?? [], null, 2) : "");
  };

  useEffect(() => {
    const id = loadRequestId.current + 1;
    loadRequestId.current = id;
    setLoading(true);
    setConfig(null);
    setMessage(null);
    void fetchCloudflareR2Config(bucket)
      .then((c) => {
        if (loadRequestId.current !== id) return;
        setConfig(c);
        if (c) hydrate(c);
      })
      .finally(() => {
        if (loadRequestId.current === id) setLoading(false);
      });
    return () => {
      // Invalidate any in-flight load and saves on unmount / bucket change so
      // their late resolutions can't write into a stale/unmounted panel.
      loadRequestId.current += 1;
      lifecycleRequestId.current += 1;
      corsRequestId.current += 1;
    };
  }, [bucket, fetchCloudflareR2Config]);

  const saveTarget = async (
    target: "lifecycle" | "cors",
    draft: string,
    setErr: (m: string | null) => void,
    setSaving: (b: boolean) => void,
    requestId: { current: number },
    save: (bucket: string, rules: unknown) => Promise<{ message: string; writtenAt: string } | null>,
  ) => {
    if (!canMutate) return;
    let parsed: unknown;
    try {
      parsed = JSON.parse(draft);
    } catch {
      setErr("Draft is not valid JSON. Fix it before saving.");
      return;
    }
    setErr(null);
    const id = requestId.current + 1;
    requestId.current = id;
    setSaving(true);
    setMessage(null);
    try {
      const r = await save(bucket, parsed);
      if (requestId.current !== id) return;
      if (r) {
        setMessage(r.message || `${target} written at ${r.writtenAt}`);
        // Re-fetch the canonical, normalized rules so the drafts reflect what
        // Cloudflare actually stored (it may reorder/normalize the JSON).
        const fresh = await fetchCloudflareR2Config(bucket);
        if (requestId.current !== id) return;
        if (fresh) {
          setConfig(fresh);
          hydrate(fresh);
        }
      }
    } finally {
      if (requestId.current === id) setSaving(false);
    }
  };

  return (
    <PanelShell
      title="R2 lifecycle & CORS"
      subtitle="Edit the bucket lifecycle and CORS rules as JSON. Object browsing is not available here. Writes require an active action project."
    >
      {loading ? (
        <p className="text-[12px] text-cream-400">Loading bucket config...</p>
      ) : !config ? (
        <p className="text-[12px] text-cream-400">
          Bucket config could not be loaded. Sync Cloudflare and reselect.
        </p>
      ) : (
        <div className="space-y-4">
          {!canMutate && <MutateBlockedNote reason={mutateReason} />}

          {/* Lifecycle */}
          <div>
            <p className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
              Lifecycle rules
            </p>
            {!config.lifecycleReadable ? (
              <p className="rounded-xl bg-amber/[0.08] px-3 py-2 text-[11px] font-semibold text-amber-dark">
                Lifecycle rules are not readable with the current token scope.
              </p>
            ) : (
              <>
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
                  <p className="mt-1 text-[11px] font-semibold text-coral-dark">{lifecycleError}</p>
                )}
                <button
                  type="button"
                  onClick={() =>
                    void saveTarget(
                      "lifecycle",
                      lifecycleDraft,
                      setLifecycleError,
                      setSavingLifecycle,
                      lifecycleRequestId,
                      setCloudflareR2Lifecycle,
                    )
                  }
                  disabled={isLoading || savingLifecycle || !canMutate}
                  title={!canMutate ? mutateReason : undefined}
                  className="mt-2 flex items-center gap-1.5 rounded-xl bg-terracotta px-3 py-1.5 text-[11px] font-semibold text-white disabled:opacity-50"
                >
                  <Save className="h-3.5 w-3.5" />
                  Save lifecycle
                </button>
              </>
            )}
          </div>

          {/* CORS */}
          <div>
            <p className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
              CORS rules
            </p>
            {!config.corsReadable ? (
              <p className="rounded-xl bg-amber/[0.08] px-3 py-2 text-[11px] font-semibold text-amber-dark">
                CORS rules are not readable with the current token scope.
              </p>
            ) : (
              <>
                <textarea
                  value={corsDraft}
                  onChange={(event) => {
                    setCorsDraft(event.target.value);
                    setCorsError(null);
                  }}
                  disabled={isLoading || savingCors}
                  spellCheck={false}
                  rows={6}
                  className="w-full rounded-xl border border-cream-200 bg-white px-3 py-2 font-mono text-[11px] text-cream-800 outline-none focus:border-terracotta/30 disabled:opacity-60"
                />
                {corsError && (
                  <p className="mt-1 text-[11px] font-semibold text-coral-dark">{corsError}</p>
                )}
                <button
                  type="button"
                  onClick={() =>
                    void saveTarget(
                      "cors",
                      corsDraft,
                      setCorsError,
                      setSavingCors,
                      corsRequestId,
                      setCloudflareR2Cors,
                    )
                  }
                  disabled={isLoading || savingCors || !canMutate}
                  title={!canMutate ? mutateReason : undefined}
                  className="mt-2 flex items-center gap-1.5 rounded-xl bg-terracotta px-3 py-1.5 text-[11px] font-semibold text-white disabled:opacity-50"
                >
                  <Save className="h-3.5 w-3.5" />
                  Save CORS
                </button>
              </>
            )}
          </div>

          {message && (
            <p className="rounded-xl bg-sage/10 px-3 py-2 text-[11px] font-semibold text-sage-dark">
              {message}
            </p>
          )}
        </div>
      )}
    </PanelShell>
  );
}

// --- shared field controls -------------------------------------------------

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
  disabled: boolean;
  placeholder?: string;
  help: string;
}) {
  return (
    <div data-help-title={label} data-help-lines={help}>
      <p className="mb-1 text-[11px] font-semibold text-cream-600">{label}</p>
      <input
        type="number"
        value={value}
        onChange={(event) => onChange(event.target.value)}
        disabled={disabled}
        placeholder={placeholder}
        className="w-full rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] text-cream-800 outline-none focus:border-terracotta/30 disabled:opacity-60"
      />
    </div>
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
  disabled: boolean;
  help: string;
}) {
  return (
    <label
      className="flex items-center justify-between gap-3 rounded-xl border border-cream-100 bg-white px-3 py-2"
      data-help-title={label}
      data-help-lines={help}
    >
      <span className="text-[11px] font-semibold text-cream-600">{label}</span>
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

function SelectField({
  label,
  value,
  onChange,
  disabled,
  options,
  help,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  disabled: boolean;
  options: { value: string; label: string }[];
  help: string;
}) {
  return (
    <div data-help-title={label} data-help-lines={help}>
      <p className="mb-1 text-[11px] font-semibold text-cream-600">{label}</p>
      <select
        value={value}
        onChange={(event) => onChange(event.target.value)}
        disabled={disabled}
        className="w-full rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-700 outline-none focus:border-terracotta/30 disabled:opacity-60"
      >
        {options.map((opt) => (
          <option key={opt.value} value={opt.value}>
            {opt.label}
          </option>
        ))}
      </select>
    </div>
  );
}
