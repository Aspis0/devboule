import { useRef, useState } from "react";
import {
  KeyRound,
  RotateCw,
  ClipboardCheck,
  Fingerprint,
  Clock,
  AlertTriangle,
  CheckCircle2,
  ShieldCheck,
  Trash2,
  type LucideIcon,
} from "lucide-react";
import { useAppContext } from "../../context/AppContext";
import type { ProviderConnectionAudit, ProviderId, SecretStatus } from "../../types/backend";
import { GithubProviderCard } from "./GithubProviderCard";

const statusConfig: Record<
  string,
  { icon: LucideIcon; bg: string; text: string; label: string; dot: string }
> = {
  configured: {
    icon: CheckCircle2,
    bg: "bg-sage/10",
    text: "text-sage-dark",
    label: "Configured",
    dot: "bg-sage",
  },
  missing: {
    icon: AlertTriangle,
    bg: "bg-coral/10",
    text: "text-coral-dark",
    label: "Missing",
    dot: "bg-coral",
  },
  stale: {
    icon: Clock,
    bg: "bg-amber/10",
    text: "text-amber-dark",
    label: "Stale",
    dot: "bg-amber",
  },
  rotation_due: {
    icon: RotateCw,
    bg: "bg-amber/10",
    text: "text-amber-dark",
    label: "Rotation Due",
    dot: "bg-amber",
  },
  error: {
    icon: AlertTriangle,
    bg: "bg-coral/10",
    text: "text-coral-dark",
    label: "Error",
    dot: "bg-coral",
  },
};

const providerLabels: Record<string, string> = {
  cloudflare: "Cloudflare",
  scaleway: "Scaleway",
};

const providerHelp: Record<ProviderId, string> = {
  cloudflare:
    "Use an account-owned API token for production. User/profile tokens are fine for local agents; Wrangler OAuth can read inventory, but secret rotation needs Workers Scripts Write.",
  scaleway: "API token for Devboule VM/serverless inventory plus start/stop/reboot/delete actions.",
};

const providerRequirements: Record<ProviderId, string[]> = {
  cloudflare: [
    "Production app token should be account-owned, not tied to a personal profile.",
    "Account Read plus Workers Scripts Read for inventory.",
    "Workers Scripts Write only when rotating Worker secrets.",
    "AI Search Edit and AI Search Run for AI Search/AutoRAG checks.",
  ],
  scaleway: [
    "Project pin must be the Devboule project, not the default launcher project.",
    "Instance and Serverless read/write for VM start, stop and delete.",
    "Separate Object Storage access + secret keypair for live bucket inventory.",
  ],
};

const providerOrder: ProviderId[] = ["cloudflare", "scaleway"];

function auditTone(audit: ProviderConnectionAudit) {
  if (audit.status === "healthy") {
    return {
      border: "border-sage/20",
      bg: "bg-sage/10",
      icon: "text-sage-dark",
      label: "Connection audit passed",
    };
  }
  if (audit.status === "degraded") {
    return {
      border: "border-amber/25",
      bg: "bg-amber/10",
      icon: "text-amber-dark",
      label: "Connection audit has warnings",
    };
  }
  return {
    border: "border-coral/20",
    bg: "bg-coral/[0.04]",
    icon: "text-coral",
    label: "Connection audit failed",
  };
}

function sortStatuses(statuses: SecretStatus[]) {
  return [...statuses].sort(
    (a, b) => providerOrder.indexOf(a.provider) - providerOrder.indexOf(b.provider),
  );
}

export function SecretsView() {
  const {
    secretStatuses,
    providerScopeStatuses,
    scalewayObjectAccessKeyStatus,
    scalewayObjectSecretKeyStatus,
    refreshSecretStatuses,
    refreshProviderScopeStatuses,
    saveScalewayObjectAccessKey,
    deleteScalewayObjectAccessKey,
    saveScalewayObjectSecretKey,
    deleteScalewayObjectSecretKey,
    saveProviderScope,
    deleteProviderScope,
    auditProviderConnection,
    auditSavedProviderConnection,
    saveProviderToken,
    deleteProviderToken,
    syncProviderInventory,
    isLoading,
  } = useAppContext();
  const [draftTokens, setDraftTokens] = useState<Record<string, string>>({});
  const [draftScopes, setDraftScopes] = useState<Record<string, string>>({});
  const [objectAccessKeyDraft, setObjectAccessKeyDraft] = useState("");
  const [objectSecretKeyDraft, setObjectSecretKeyDraft] = useState("");
  const [connectionAudits, setConnectionAudits] = useState<Record<string, ProviderConnectionAudit>>({});
  const [pendingDelete, setPendingDelete] = useState<ProviderId | null>(null);
  const [pendingObjectAccessKeyDelete, setPendingObjectAccessKeyDelete] = useState(false);
  const [pendingObjectSecretKeyDelete, setPendingObjectSecretKeyDelete] = useState(false);
  const auditRequestSeq = useRef<Record<string, number>>({});
  const sortedStatuses = sortStatuses(secretStatuses);
  const dueCount = sortedStatuses.filter(
    (s) => s.status === "rotation_due" || s.status === "stale",
  ).length;

  const clearConnectionAudit = (provider: ProviderId) => {
    auditRequestSeq.current[provider] = (auditRequestSeq.current[provider] ?? 0) + 1;
    setConnectionAudits((prev) => {
      const next = { ...prev };
      delete next[provider];
      return next;
    });
  };

  const handleDraftChange = (provider: ProviderId, value: string) => {
    clearConnectionAudit(provider);
    setDraftTokens((prev) => ({ ...prev, [provider]: value }));
  };

  const handleScopeDraftChange = (provider: ProviderId, value: string) => {
    clearConnectionAudit(provider);
    setDraftScopes((prev) => ({ ...prev, [provider]: value }));
  };

  const handleSaveScope = async (provider: ProviderId) => {
    const pinnedId = draftScopes[provider]?.trim();
    if (!pinnedId) return;
    const status = await saveProviderScope(provider, pinnedId);
    if (!status?.configured) return;
    clearConnectionAudit(provider);
    setDraftScopes((prev) => ({ ...prev, [provider]: "" }));
    await syncProviderInventory(provider);
  };

  const handleDeleteScope = async (provider: ProviderId) => {
    const status = await deleteProviderScope(provider);
    if (status) {
      clearConnectionAudit(provider);
      await syncProviderInventory(provider);
    }
  };

  const handleConnectionAudit = async (
    provider: ProviderId,
    token: string,
    pinnedId: string | null,
    useSavedToken: boolean,
  ) => {
    const cleaned = token.trim();
    if (!cleaned && !useSavedToken) {
      clearConnectionAudit(provider);
      await refreshSecretStatuses();
      return;
    }
    const requestId = (auditRequestSeq.current[provider] ?? 0) + 1;
    auditRequestSeq.current[provider] = requestId;
    const audit = cleaned
      ? await auditProviderConnection(provider, cleaned, pinnedId)
      : await auditSavedProviderConnection(provider, pinnedId);
    if (!audit) return;
    if (auditRequestSeq.current[provider] !== requestId) return;
    setConnectionAudits((prev) => ({ ...prev, [provider]: audit }));
  };

  const handleSave = async (provider: ProviderId) => {
    const token = draftTokens[provider]?.trim();
    if (!token) return;
    const pinnedId = draftScopes[provider]?.trim() || null;
    const status = await saveProviderToken(provider, token, pinnedId);
    if (!status?.configured) return;
    clearConnectionAudit(provider);
    setDraftTokens((prev) => ({ ...prev, [provider]: "" }));
    if (pinnedId) {
      setDraftScopes((prev) => ({ ...prev, [provider]: "" }));
      await refreshProviderScopeStatuses();
    }
    await syncProviderInventory(provider);
  };

  const handleDelete = async (provider: ProviderId) => {
    if (pendingDelete !== provider) {
      setPendingDelete(provider);
      return;
    }
    await deleteProviderToken(provider);
    clearConnectionAudit(provider);
    setPendingDelete(null);
  };

  const maybeApplyObjectKeypairPaste = (value: string) => {
    const parts = value.split(/[\s,;:=]+/).map((part) => part.trim()).filter(Boolean);
    const accessKey = parts.find((part) => /^SCW[A-Za-z0-9]{8,}$/.test(part));
    const secretKey = parts.find(
      (part) =>
        /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(part) ||
        (part.length >= 24 && part !== accessKey && !part.startsWith("SCW")),
    );
    if (accessKey && secretKey) {
      setObjectAccessKeyDraft(accessKey);
      setObjectSecretKeyDraft(secretKey);
      return true;
    }
    return false;
  };

  const handleSaveObjectAccessKey = async () => {
    const accessKey = objectAccessKeyDraft.trim();
    if (!accessKey) return;
    const status = await saveScalewayObjectAccessKey(accessKey);
    if (!status?.configured) return;
    setObjectAccessKeyDraft("");
    setPendingObjectAccessKeyDelete(false);
    await syncProviderInventory("scaleway");
  };

  const handleSaveObjectSecretKey = async () => {
    const secretKey = objectSecretKeyDraft.trim();
    if (!secretKey) return;
    const status = await saveScalewayObjectSecretKey(secretKey);
    if (!status?.configured) return;
    setObjectSecretKeyDraft("");
    setPendingObjectSecretKeyDelete(false);
    await syncProviderInventory("scaleway");
  };

  const handleDeleteObjectAccessKey = async () => {
    if (!pendingObjectAccessKeyDelete) {
      setPendingObjectAccessKeyDelete(true);
      return;
    }
    await deleteScalewayObjectAccessKey();
    setPendingObjectAccessKeyDelete(false);
  };

  const handleDeleteObjectSecretKey = async () => {
    if (!pendingObjectSecretKeyDelete) {
      setPendingObjectSecretKeyDelete(true);
      return;
    }
    await deleteScalewayObjectSecretKey();
    setPendingObjectSecretKeyDelete(false);
  };

  return (
    <div className="max-w-4xl space-y-6">
      {/* Windows Hello banner */}
      <div className="flex items-center gap-4 p-5 bg-white rounded-2xl border border-cream-200">
        <div className="w-10 h-10 rounded-xl bg-terracotta-50 flex items-center justify-center shrink-0">
          <Fingerprint className="w-5 h-5 text-terracotta" />
        </div>
        <div>
          <p className="text-[14px] font-medium text-cream-800">
            Protected by Windows Hello
          </p>
          <p className="text-[12px] text-cream-400">
            All secret values are encrypted at rest and only accessible after
            biometric verification. Token values are never displayed.
          </p>
        </div>
      </div>

      {/* Secrets list */}
      <div className="bg-white rounded-2xl border border-cream-200 divide-y divide-cream-100 overflow-hidden">
        {sortedStatuses.map((secret) => {
          const cfg = statusConfig[secret.status] || statusConfig.error;
          const StatusIcon = cfg.icon;
          const provider = secret.provider as ProviderId;
          const draft = draftTokens[provider] ?? "";
          const scope = providerScopeStatuses.find((item) => item.provider === provider);
          const scopeDraft = draftScopes[provider] ?? "";
          const audit = connectionAudits[provider];
          const auditPinnedId = scopeDraft.trim() || scope?.pinnedId || null;
          const auditStyle = audit ? auditTone(audit) : null;

          return (
            <div
              key={secret.provider}
              className="px-5 py-4"
              data-help-title={`${providerLabels[provider] || provider} credentials control provider access.`}
              data-help-lines="This row is where the app stores and audits provider access for one cloud service.|For Devboule, the important checks are token validity, pinned account/project scope, and whether the token is read-only or write-capable.|Raw token values should never be copied into projects, Oracle, terminal prompts, or source files.|After changing credentials, audit and sync before launching agents."
            >
              <div className="flex flex-col gap-4">
                <div className="flex items-start gap-4 min-w-0">
                  <div className="w-10 h-10 rounded-xl bg-cream-50 flex items-center justify-center shrink-0">
                    <KeyRound className="w-5 h-5 text-cream-500" />
                  </div>
                  <div className="min-w-0">
                    <div className="flex items-center gap-2 mb-0.5">
                      <p className="text-[14px] font-medium text-cream-800">
                        {providerLabels[provider] || provider}
                      </p>
                      <span className="text-[10px] font-medium px-2 py-0.5 rounded-full bg-cream-100 text-cream-500">
                        {provider}
                      </span>
                    </div>
                    <div className="flex flex-wrap items-center gap-3">
                      <span
                        className={`inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-[10px] font-medium ${cfg.bg} ${cfg.text}`}
                      >
                        <StatusIcon className="w-3 h-3" />
                        {cfg.label}
                      </span>
                      <span className="text-[11px] text-cream-400">
                        Local read {secret.lastCheckedAt || "never"}
                      </span>
                      {secret.message && (
                        <span className="text-[11px] text-amber-dark font-medium">
                          {secret.message}
                        </span>
                      )}
                    </div>
                    <p className="text-[11px] text-cream-400 mt-1">
                      {providerHelp[provider]}
                    </p>
                    <div className="mt-2 grid gap-1">
                      {providerRequirements[provider].map((item) => (
                        <p key={item} className="text-[10px] text-cream-400">
                          {item}
                        </p>
                      ))}
                    </div>
                    <div className="mt-3 flex flex-col gap-2 sm:flex-row sm:items-center">
                      <input
                        type="password"
                        value={draft}
                        onChange={(event) => handleDraftChange(provider, event.target.value)}
                        placeholder={`Paste ${providerLabels[provider]} token`}
                        data-help-title={`${providerLabels[provider]} token is a private cloud access key.`}
                        data-help-lines="A token lets the app read or change provider resources depending on its scopes.|Save human dashboard tokens here, not inside project notes or code.|Temporary tokens expire; rotate and save a new token here when sync starts failing.|Prefer narrow scopes and the pinned Devboule account or project."
                        autoComplete="off"
                        spellCheck={false}
                        className="min-w-0 flex-1 rounded-xl border border-cream-200 bg-cream-50 px-3 py-2 text-[12px] font-mono text-cream-800 outline-none focus:border-terracotta-200 focus:ring-2 focus:ring-terracotta/15"
                      />
                      <button
                        onClick={() => void handleSave(provider)}
                        disabled={isLoading || draft.trim().length === 0}
                        data-help-title={`This saves or rotates the ${providerLabels[provider]} token.`}
                        data-help-lines="The token goes to Windows vault through the backend.|After saving, the app syncs provider inventory when possible.|It does not expose the raw token to Oracle or project files.|If the token is temporary, note its expiry outside the secret value and replace it before it dies."
                        className="flex items-center justify-center gap-1.5 px-3 py-2 rounded-xl
                                   bg-terracotta text-white text-[12px] font-medium
                                   hover:bg-terracotta-500 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-terracotta/30
                                   disabled:cursor-not-allowed disabled:opacity-50 transition-all duration-200"
                      >
                        <RotateCw className="w-3.5 h-3.5" />
                        {secret.configured ? "Rotate & Sync" : "Save & Sync"}
                      </button>
                      <button
                        onClick={() => void handleDelete(provider)}
                        disabled={isLoading || !secret.configured}
                        data-help-title={`This deletes the saved ${providerLabels[provider]} token.`}
                        data-help-lines="Deleting removes the local vault copy only.|It does not revoke the token at the provider website.|Use confirm-delete when a token expires, leaks, or has the wrong scope.|Syncs and provider actions will fail until a valid token is saved."
                        className="flex items-center justify-center gap-1.5 px-3 py-2 rounded-xl
                                   border border-cream-200 text-[12px] font-medium text-cream-600
                                   hover:border-coral/30 hover:text-coral hover:bg-coral/5
                                   focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-coral/20
                                   disabled:cursor-not-allowed disabled:opacity-50 transition-all duration-200"
                      >
                        <Trash2 className="w-3.5 h-3.5" />
                        {pendingDelete === provider ? "Confirm" : "Delete"}
                      </button>
                      <button
                        onClick={() =>
                          void handleConnectionAudit(
                            provider,
                            draft,
                            auditPinnedId,
                            secret.configured,
                          )
                        }
                        disabled={isLoading}
                        data-help-title={`This audits the ${providerLabels[provider]} connection.`}
                        data-help-lines="Audit checks whether the token and pinned scope can read the expected account or project.|It should reveal wrong project IDs, expired keys, or missing permissions.|It is a safer first step before sync or write operations.|Use it after changing token, scope, or provider account."
                        className="flex items-center justify-center gap-1.5 px-3 py-2 rounded-xl
                                   border border-cream-200 text-[12px] font-medium text-cream-600
                                   hover:border-teal-200 hover:text-teal hover:bg-teal/5
                                   focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-teal/30
                                   disabled:opacity-60 transition-all duration-200"
                      >
                        <ClipboardCheck className="w-3.5 h-3.5" />
                        {draft.trim() ? "Audit Token" : secret.configured ? "Audit Saved" : "Refresh"}
                      </button>
                    </div>
                    {secret.configured && (
                      <p className="mt-1.5 text-[11px] text-cream-400">
                        Saved{" "}
                        <span className="font-mono text-cream-300">••••••••</span>{" "}
                        (hidden)
                      </p>
                    )}
                    <div className="mt-3 flex flex-col gap-2 rounded-xl border border-cream-100 bg-cream-50/60 p-3">
                      <div className="flex flex-wrap items-center justify-between gap-2">
                        <div>
                          <p className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
                            {scope?.label ?? "Provider scope"}
                          </p>
                          <p className="mt-0.5 text-[11px] text-cream-400">
                            {scope?.configured
                              ? `Pinned: ${scope.pinnedId}`
                              : scope?.message ?? "Optional provider scope pin"}
                          </p>
                        </div>
                      </div>
                      <div className="flex flex-col gap-2 sm:flex-row">
                        <input
                          value={scopeDraft}
                          onChange={(event) => handleScopeDraftChange(provider, event.target.value)}
                          placeholder={
                            provider === "cloudflare"
                              ? "Optional account id"
                              : "Optional project id"
                          }
                          data-help-title="A provider scope pins the exact account or project."
                          data-help-lines="Pinning prevents the app from accidentally showing or changing the wrong Cloudflare account or Scaleway project.|For Scaleway, use the Devboule project id, not the default launcher project.|Saving a scope does not create cloud resources.|If the scope is wrong, sync results and agent tools become dangerous."
                          spellCheck={false}
                          className="min-w-0 flex-1 rounded-lg border border-cream-200 bg-white px-3 py-2 text-[11px] font-mono text-cream-800 outline-none focus:border-teal/40 focus:ring-2 focus:ring-teal/10"
                        />
                        <button
                          onClick={() => void handleSaveScope(provider)}
                          disabled={isLoading || scopeDraft.trim().length === 0}
                          data-help-title="This saves the provider scope pin."
                          data-help-lines="The pin is stored locally and used by dashboard syncs and provider tools.|It helps isolate Devboule from other projects in the same account.|It does not validate all permissions by itself; audit after saving.|Update it if you move resources to another account or project."
                          className="rounded-lg border border-cream-200 px-3 py-2 text-[11px] font-medium text-cream-600 hover:border-teal/30 hover:text-teal-dark disabled:cursor-not-allowed disabled:opacity-50"
                        >
                          Pin
                        </button>
                        <button
                          onClick={() => void handleDeleteScope(provider)}
                          disabled={isLoading || !scope?.configured}
                          data-help-title="This clears the provider scope pin."
                          data-help-lines="Clearing removes the local account or project restriction.|Future syncs may fall back to provider defaults, which is risky with multiple projects.|Use this only when replacing the scope or debugging provider access.|Run audit again after clearing or changing scope."
                          className="rounded-lg border border-cream-200 px-3 py-2 text-[11px] font-medium text-cream-600 hover:border-coral/30 hover:text-coral disabled:cursor-not-allowed disabled:opacity-50"
                        >
                          Clear
                        </button>
                      </div>
                    </div>
                    {audit && (
                      <div
                        className={`mt-3 rounded-xl border px-3 py-2 ${auditStyle?.border} ${auditStyle?.bg}`}
                      >
                        <div className="flex items-center gap-2">
                          <ShieldCheck
                            className={`h-3.5 w-3.5 ${auditStyle?.icon}`}
                          />
                          <p className="text-[11px] font-semibold text-cream-700">
                            {auditStyle?.label}
                          </p>
                        </div>
                        <p className="mt-1 text-[11px] text-cream-500">
                          {audit.selectedScope
                            ? `${audit.selectedScope.name ?? audit.selectedScope.id} / ${audit.selectedScope.source}`
                            : audit.message ?? "No provider scope selected"}
                          {" · "}
                          {audit.resourceCount} resource{audit.resourceCount === 1 ? "" : "s"}
                        </p>
                        {audit.message && (
                          <p className="mt-1 line-clamp-2 text-[10px] text-cream-500">
                            {audit.message}
                          </p>
                        )}
                        {audit.risks.length > 0 && (
                          <p className="mt-1 line-clamp-2 text-[10px] text-amber-dark">
                            {audit.risks[0]}
                          </p>
                        )}
                      </div>
                    )}
                    {provider === "scaleway" && (
                      <div className="mt-3 flex flex-col gap-2 rounded-xl border border-cream-100 bg-cream-50/60 p-3">
                        <div>
                          <p className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
                            Object Storage keypair
                          </p>
                          <p className="mt-0.5 text-[11px] text-cream-400">
                            {scalewayObjectAccessKeyStatus?.configured &&
                            scalewayObjectSecretKeyStatus?.configured
                              ? "Configured for live bucket inventory"
                              : "Access key and secret key are both required for live bucket inventory"}
                          </p>
                        </div>
                        <div className="flex flex-col gap-2">
                          <input
                            type="password"
                            value={objectAccessKeyDraft}
                            onChange={(event) => {
                              setPendingObjectAccessKeyDelete(false);
                              if (!maybeApplyObjectKeypairPaste(event.target.value)) {
                                setObjectAccessKeyDraft(event.target.value);
                              }
                            }}
                            placeholder="Paste Scaleway access key"
                            data-help-title="This is the Scaleway Object Storage access key."
                            data-help-lines="An access key identifies the Object Storage credential pair.|It is needed for live bucket inventory, together with the secret key.|It is saved through the backend vault, not project notes.|Temporary or rotated keys must be replaced here before bucket sync works."
                            spellCheck={false}
                            autoComplete="off"
                            className="min-w-0 flex-1 rounded-lg border border-cream-200 bg-white px-3 py-2 text-[11px] font-mono text-cream-800 outline-none focus:border-teal/40 focus:ring-2 focus:ring-teal/10"
                          />
                          <input
                            type="password"
                            value={objectSecretKeyDraft}
                            onChange={(event) => {
                              setPendingObjectSecretKeyDelete(false);
                              if (!maybeApplyObjectKeypairPaste(event.target.value)) {
                                setObjectSecretKeyDraft(event.target.value);
                              }
                            }}
                            placeholder="Paste Scaleway secret key"
                            data-help-title="This is the private Scaleway Object Storage secret key."
                            data-help-lines="The secret key is the password half of the Object Storage pair.|Do not paste it into Projects, Oracle, or chat.|It is saved in the Windows vault and used only by backend inventory calls.|If it expires or leaks, delete it here and rotate it in Scaleway."
                            spellCheck={false}
                            autoComplete="off"
                            className="min-w-0 flex-1 rounded-lg border border-cream-200 bg-white px-3 py-2 text-[11px] font-mono text-cream-800 outline-none focus:border-teal/40 focus:ring-2 focus:ring-teal/10"
                          />
                          <div className="flex flex-wrap gap-2">
                            <button
                              onClick={() => void handleSaveObjectAccessKey()}
                              disabled={isLoading || objectAccessKeyDraft.trim().length === 0}
                              data-help-title="This saves the Object Storage access key."
                              data-help-lines="The key is stored in the Windows vault through Tauri.|It does not test bucket access by itself unless the surrounding sync runs.|Use it with the matching secret key from the same Scaleway credential.|Replace it when the credential expires or is rotated."
                              className="rounded-lg border border-cream-200 px-3 py-2 text-[11px] font-medium text-cream-600 hover:border-teal/30 hover:text-teal-dark disabled:cursor-not-allowed disabled:opacity-50"
                            >
                              Save access
                            </button>
                            <button
                              onClick={() => void handleSaveObjectSecretKey()}
                              disabled={isLoading || objectSecretKeyDraft.trim().length === 0}
                              data-help-title="This saves the Object Storage secret key."
                              data-help-lines="The value is stored in the Windows vault and should never be indexed.|It must match the saved Object Storage access key.|Expired or leaked keys should be deleted and rotated at Scaleway.|Bucket inventory may fail until both halves are valid."
                              className="rounded-lg border border-cream-200 px-3 py-2 text-[11px] font-medium text-cream-600 hover:border-teal/30 hover:text-teal-dark disabled:cursor-not-allowed disabled:opacity-50"
                            >
                              Save secret
                            </button>
                            <button
                              onClick={() => void handleDeleteObjectAccessKey()}
                              disabled={isLoading || !scalewayObjectAccessKeyStatus?.configured}
                              data-help-title="This deletes the saved Object Storage access key."
                              data-help-lines="Deleting removes the local vault value only.|It does not revoke the key inside Scaleway.|Use it when a key expires, leaks, or belongs to the wrong project.|Delete the matching secret too if rotating the pair."
                              className="rounded-lg border border-cream-200 px-3 py-2 text-[11px] font-medium text-cream-600 hover:border-coral/30 hover:text-coral disabled:cursor-not-allowed disabled:opacity-50"
                            >
                              {pendingObjectAccessKeyDelete ? "Confirm access" : "Delete access"}
                            </button>
                            <button
                              onClick={() => void handleDeleteObjectSecretKey()}
                              disabled={isLoading || !scalewayObjectSecretKeyStatus?.configured}
                              data-help-title="This deletes the saved Object Storage secret key."
                              data-help-lines="Deleting removes the secret from the Windows vault.|It does not revoke it in Scaleway, so rotate there too if needed.|Use this when the credential expires or you pasted the wrong pair.|Bucket inventory will fail until a valid pair is saved."
                              className="rounded-lg border border-cream-200 px-3 py-2 text-[11px] font-medium text-cream-600 hover:border-coral/30 hover:text-coral disabled:cursor-not-allowed disabled:opacity-50"
                            >
                              {pendingObjectSecretKeyDelete ? "Confirm secret" : "Delete secret"}
                            </button>
                          </div>
                        </div>
                      </div>
                    )}
                  </div>
                </div>
              </div>
            </div>
          );
        })}
        {/* GitHub stays on its own bespoke path (not a ProviderId), but lives in
            the same Secrets list so all credentials are configured in one place. */}
        <GithubProviderCard />
        {sortedStatuses.length === 0 && (
          <div className="px-5 py-8 text-center text-[13px] text-cream-400">
            No provider status loaded yet. Unlock the app and run audit.
          </div>
        )}
      </div>

      {/* Summary footer */}
      {dueCount > 0 && (
        <div className="flex items-center gap-3 px-5 py-3.5 rounded-2xl bg-amber/10 border border-amber/20">
          <AlertTriangle className="w-4 h-4 text-amber shrink-0" />
          <p className="text-[12px] text-amber-dark font-medium">
            {dueCount} secret{dueCount > 1 ? "s" : ""} require
            {dueCount === 1 ? "s" : ""} rotation. Rotate before expiry to avoid
            service disruption.
          </p>
        </div>
      )}
    </div>
  );
}
