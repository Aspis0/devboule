// Web search settings card for the pi-web-access extension.
//
// Manages the default provider (web-search.json) and 7 provider API keys
// (Exa, Brave, Tavily, Perplexity, Gemini, OpenAI, Parallel). Keys are
// stored in the OS vault (keyring) and injected as env vars at pi-sidecar
// spawn time — they never touch pi's own config files.
//
// UI shows ONLY the selected provider's key row (no endless column).

import {
  AlertTriangle,
  CheckCircle2,
  Globe,
  Trash2,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invokeBackendCommand } from "../../context/AppContext";
import type { AuxCredentialStatus } from "../../types/backend";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type WebsearchBadgeTone = "configured" | "missing" | "error";

export interface WebsearchBadge {
  tone: WebsearchBadgeTone;
  label: string;
}

export function websearchKeyBadge(status: AuxCredentialStatus | null): WebsearchBadge {
  if (!status) return { tone: "missing", label: "No key" };
  if (status.status === "error") return { tone: "error", label: "Error" };
  if (status.configured) return { tone: "configured", label: "Key saved" };
  return { tone: "missing", label: "No key" };
}

const BADGE_TONE: Record<WebsearchBadgeTone, string> = {
  configured: "bg-sage/10 text-sage-dark",
  missing: "bg-amber/10 text-amber-dark",
  error: "bg-coral/10 text-coral-dark",
};

const BADGE_ICON: Record<WebsearchBadgeTone, typeof CheckCircle2> = {
  configured: CheckCircle2,
  missing: AlertTriangle,
  error: AlertTriangle,
};

// ---------------------------------------------------------------------------
// Provider metadata
// ---------------------------------------------------------------------------

interface ProviderSpec {
  /** Vault key id (used in IPC commands). */
  id: string;
  /** Display label. */
  label: string;
  /** Note shown below the key row (or null for none). */
  note: string | null;
}

/// Maps the config-file provider id (select value) → vault key id + metadata.
/// `auto` has no vault key (no key row). Providers without notes get `null`.
const CONFIG_TO_KEY: Record<string, ProviderSpec | null> = {
  auto: null,
  exa: { id: "exa", label: "Exa", note: "Optional — works without a key (higher limits with one)." },
  brave: { id: "brave", label: "Brave", note: null },
  tavily: { id: "tavily", label: "Tavily", note: null },
  perplexity: { id: "perplexity", label: "Perplexity", note: null },
  gemini: { id: "gemini_search", label: "Gemini", note: null },
  openai: { id: "openai_search", label: "OpenAI", note: null },
  parallel: { id: "parallel", label: "Parallel", note: null },
};

const CONFIG_SELECT_OPTIONS: { id: string; label: string }[] = [
  { id: "auto", label: "Auto (recommended)" },
  { id: "exa", label: "Exa (no key needed)" },
  { id: "brave", label: "Brave" },
  { id: "tavily", label: "Tavily" },
  { id: "perplexity", label: "Perplexity" },
  { id: "gemini", label: "Gemini" },
  { id: "openai", label: "OpenAI" },
  { id: "parallel", label: "Parallel" },
];

// ---------------------------------------------------------------------------
// WebsearchConfig type (mirrors the backend WebsearchConfig)
// ---------------------------------------------------------------------------

interface WebsearchConfig {
  provider: string;
}

// ---------------------------------------------------------------------------
// Key row — mirrors ExaSearchKeyCard's save/delete/status UX
// ---------------------------------------------------------------------------

function KeyRow({ spec }: { spec: ProviderSpec }) {
  const [status, setStatus] = useState<AuxCredentialStatus | null>(null);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pendingRemove, setPendingRemove] = useState(false);
  const mountedRef = useRef(true);
  const inflightRef = useRef(false);

  useEffect(() => {
    mountedRef.current = true;
    return () => { mountedRef.current = false; };
  }, []);

  const refresh = useCallback(async () => {
    try {
      const next = await invokeBackendCommand<AuxCredentialStatus>(
        "websearch_key_status",
        { provider: spec.id },
      );
      if (!mountedRef.current) return;
      setStatus(next);
    } catch (e) {
      if (!mountedRef.current) return;
      setError(e instanceof Error ? e.message : `Could not read the ${spec.label} key status.`);
    }
  }, [spec.id, spec.label]);

  useEffect(() => { void refresh(); }, [refresh]);

  const handleSave = useCallback(async () => {
    const key = draft.trim();
    if (!key) return;
    if (inflightRef.current) return;
    inflightRef.current = true;
    setBusy(true);
    setError(null);
    try {
      const next = await invokeBackendCommand<AuxCredentialStatus>(
        "websearch_save_key",
        { provider: spec.id, key },
      );
      if (!mountedRef.current) return;
      setStatus(next);
      setPendingRemove(false);
      if (next.configured) {
        setDraft("");
      } else {
        setDraft("");
        setError(next.message ?? `The ${spec.label} key was not accepted.`);
      }
    } catch (e) {
      if (!mountedRef.current) return;
      setError(e instanceof Error ? e.message : `Saving the ${spec.label} key failed.`);
    } finally {
      inflightRef.current = false;
      if (mountedRef.current) setBusy(false);
    }
  }, [draft, spec.id, spec.label]);

  const handleRemove = useCallback(async () => {
    if (!pendingRemove) { setPendingRemove(true); return; }
    if (inflightRef.current) return;
    inflightRef.current = true;
    setBusy(true);
    setError(null);
    try {
      const next = await invokeBackendCommand<AuxCredentialStatus>(
        "websearch_delete_key",
        { provider: spec.id },
      );
      if (!mountedRef.current) return;
      setStatus(next);
      setDraft("");
    } catch (e) {
      if (!mountedRef.current) return;
      setError(e instanceof Error ? e.message : `Removing the ${spec.label} key failed.`);
    } finally {
      inflightRef.current = false;
      if (mountedRef.current) { setBusy(false); setPendingRemove(false); }
    }
  }, [pendingRemove, spec.id, spec.label]);

  const badge = websearchKeyBadge(status);
  const BadgeIcon = BADGE_ICON[badge.tone];
  const hasKey = status?.configured === true;

  return (
    <div className="flex flex-col gap-2 rounded-xl border border-cream-100 bg-cream-50/40 p-3">
      <div className="flex items-center justify-between gap-2">
        <span className="text-[12px] font-semibold text-cream-700">{spec.label}</span>
        <span
          className={`inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[10px] font-medium ${BADGE_TONE[badge.tone]}`}
        >
          <BadgeIcon className="h-3 w-3" />
          {badge.label}
        </span>
      </div>

      <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
        <input
          type="password"
          value={draft}
          onChange={(event) => { setError(null); setPendingRemove(false); setDraft(event.target.value); }}
          placeholder={hasKey ? `Paste a new ${spec.label} key to rotate` : `Paste your ${spec.label} API key`}
          autoComplete="off"
          spellCheck={false}
          className="min-w-0 flex-1 rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] text-cream-700 outline-none focus:border-teal/30"
        />
        <button
          type="button"
          onClick={() => void handleSave()}
          disabled={busy || draft.trim().length === 0}
          className="inline-flex items-center justify-center gap-1.5 rounded-md bg-teal px-3 py-2 text-[12px] font-semibold text-white hover:bg-teal/90 disabled:cursor-not-allowed disabled:opacity-60"
        >
          <CheckCircle2 className="h-3.5 w-3.5" />
          {hasKey ? "Rotate" : "Save"}
        </button>
        {hasKey ? (
          <button
            type="button"
            onClick={() => void handleRemove()}
            disabled={busy}
            className="inline-flex items-center justify-center gap-1 rounded-md border border-cream-200 bg-white px-3 py-2 text-[11px] font-semibold text-cream-500 hover:border-coral/30 hover:text-coral-dark disabled:cursor-not-allowed disabled:opacity-60"
          >
            <Trash2 className="h-3.5 w-3.5" />
            {pendingRemove ? "Confirm" : "Clear"}
          </button>
        ) : null}
      </div>

      {spec.note ? (
        <p className="text-[11px] leading-4 text-cream-400">{spec.note}</p>
      ) : null}

      {error && (
        <p className="flex items-start gap-2 rounded-2xl border border-coral/30 bg-coral/[0.05] px-3 py-2 text-[11px] leading-4 text-coral-dark">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>{error}</span>
        </p>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Main card
// ---------------------------------------------------------------------------

export function WebSearchCard() {
  const [config, setConfig] = useState<WebsearchConfig | null>(null);
  const [configBusy, setConfigBusy] = useState(false);
  const [configError, setConfigError] = useState<string | null>(null);
  const mountedRef = useRef(true);
  const inflightRef = useRef(false);

  useEffect(() => {
    mountedRef.current = true;
    return () => { mountedRef.current = false; };
  }, []);

  const refreshConfig = useCallback(async () => {
    try {
      const next = await invokeBackendCommand<WebsearchConfig>("websearch_get_config");
      if (!mountedRef.current) return;
      setConfig(next);
    } catch (e) {
      if (!mountedRef.current) return;
      setConfigError(e instanceof Error ? e.message : "Could not read the web search config.");
    }
  }, []);

  useEffect(() => { void refreshConfig(); }, [refreshConfig]);

  const handleProviderChange = useCallback(async (newProvider: string) => {
    if (inflightRef.current) return;
    inflightRef.current = true;
    setConfigBusy(true);
    setConfigError(null);
    try {
      const next = await invokeBackendCommand<WebsearchConfig>(
        "websearch_set_config",
        { provider: newProvider },
      );
      if (!mountedRef.current) return;
      setConfig(next);
    } catch (e) {
      if (!mountedRef.current) return;
      setConfigError(e instanceof Error ? e.message : "Could not save the default provider.");
    } finally {
      inflightRef.current = false;
      if (mountedRef.current) setConfigBusy(false);
    }
  }, []);

  const currentProvider = useMemo(() => config?.provider ?? "auto", [config]);
  const keySpec = useMemo(() => CONFIG_TO_KEY[currentProvider] ?? null, [currentProvider]);

  return (
    <section
      className="rounded-2xl border border-cream-200 bg-white p-4"
      data-help-title="Web search settings for the pi-web-access extension."
      data-help-lines="Works out of the box with Exa (no key needed).|Add keys for more providers — they are stored in the app vault and injected as env vars.|The default provider sets which search engine pi uses when none is specified in the prompt."
    >
      <div className="mb-3 flex items-center gap-2">
        <Globe className="h-4 w-4 text-teal" />
        <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          Web search
        </h3>
      </div>

      <p className="mb-3 max-w-3xl text-[12px] leading-5 text-cream-500">
        Powered by the pi-web-access extension. Works out of the box (Exa)
        — add keys for more providers.
      </p>

      {/* Default provider select */}
      <div className="mb-4 flex flex-col gap-2 sm:flex-row sm:items-center">
        <label
          htmlFor="websearch-default-provider"
          className="text-[12px] font-medium text-cream-600"
        >
          Default provider
        </label>
        <select
          id="websearch-default-provider"
          value={currentProvider}
          onChange={(event) => void handleProviderChange(event.target.value)}
          disabled={configBusy}
          className="rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] text-cream-700 outline-none focus:border-teal/30 disabled:opacity-60"
        >
          {CONFIG_SELECT_OPTIONS.map(({ id, label }) => (
            <option key={id} value={id}>{label}</option>
          ))}
        </select>
        {configBusy ? (
          <span className="text-[11px] text-cream-400">Saving…</span>
        ) : null}
      </div>

      {configError && (
        <p className="mb-3 flex items-start gap-2 rounded-2xl border border-coral/30 bg-coral/[0.05] px-3 py-2 text-[11px] leading-4 text-coral-dark">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>{configError}</span>
        </p>
      )}

      {/* Conditional: auto → muted line; selected provider → its key row */}
      {keySpec === null ? (
        <p className="text-[12px] leading-5 text-cream-400">
          Automatically picks the best available provider.
        </p>
      ) : (
        <KeyRow spec={keySpec} />
      )}

      {/* Privacy note */}
      <p className="mt-3 text-[11px] leading-4 text-cream-400">
        Keys are stored in the app vault and injected into pi sessions as env
        vars — they never touch pi&apos;s config files.
      </p>
    </section>
  );
}

export default WebSearchCard;
