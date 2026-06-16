import {
  AlertTriangle,
  CheckCircle2,
  Globe,
  Trash2,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { invokeBackendCommand } from "../../context/AppContext";
import type { AuxCredentialStatus } from "../../types/backend";

// L2.4 — Settings card for the Exa web-search API key used by the LOCAL Devboule
// orchestrator (the on-device main coder). WRITE-ONLY: the key value is NEVER
// rendered or read back. `get_exa_key_status` returns present/absent ONLY
// (AuxCredentialStatus, no secret), `save_exa_key` SETs it, `delete_exa_key`
// CLEARs it. The key's PRESENCE is the switch — no key means the local coder's web
// tools stay off and it answers from the Oracle (private, on-device).
//
// Visual idiom mirrors MiniCoderBackendCard (the sibling coding-engine cards in
// ProvidersModelsTab); the write-only-secret handling (draft, in-flight lock,
// never-read-back) mirrors GithubProviderCard.

// status -> badge tone + copy. Pure so it is unit-testable without the DOM and so
// the present/absent/error mapping has a single source of truth.
export type ExaBadgeTone = "configured" | "missing" | "error";

export interface ExaBadge {
  tone: ExaBadgeTone;
  label: string;
}

export function exaKeyBadge(status: AuxCredentialStatus | null): ExaBadge {
  if (!status) return { tone: "missing", label: "No key" };
  if (status.status === "error") return { tone: "error", label: "Error" };
  if (status.configured) return { tone: "configured", label: "Key saved" };
  return { tone: "missing", label: "No key" };
}

const BADGE_TONE: Record<ExaBadgeTone, string> = {
  configured: "bg-sage/10 text-sage-dark",
  missing: "bg-amber/10 text-amber-dark",
  error: "bg-coral/10 text-coral-dark",
};

const BADGE_ICON: Record<ExaBadgeTone, typeof CheckCircle2> = {
  configured: CheckCircle2,
  missing: AlertTriangle,
  error: AlertTriangle,
};

export function ExaSearchKeyCard() {
  const [status, setStatus] = useState<AuxCredentialStatus | null>(null);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pendingRemove, setPendingRemove] = useState(false);
  // Guard against setState after unmount (load/save are async).
  const mountedRef = useRef(true);
  // Synchronous in-flight lock: `busy` updates asynchronously, so a fast
  // double-click would dispatch two concurrent IPC writes before the disabled
  // attribute re-renders. This ref flips synchronously to drop the second call.
  const inflightRef = useRef(false);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const refresh = useCallback(async () => {
    try {
      const next =
        await invokeBackendCommand<AuxCredentialStatus>("get_exa_key_status");
      if (!mountedRef.current) return;
      setStatus(next);
    } catch (e) {
      if (!mountedRef.current) return;
      setError(e instanceof Error ? e.message : "Could not read the Exa key status.");
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const handleSave = useCallback(async () => {
    const key = draft.trim();
    if (!key) return;
    if (inflightRef.current) return;
    inflightRef.current = true;
    setBusy(true);
    setError(null);
    try {
      const next = await invokeBackendCommand<AuxCredentialStatus>(
        "save_exa_key",
        { key },
      );
      if (!mountedRef.current) return;
      setStatus(next);
      setPendingRemove(false);
      // On success the value lives only in the OS keychain; never keep the draft.
      if (next.configured) {
        setDraft("");
      } else {
        // Backend rejected it (too short / whitespace) — surface inline, drop draft
        // so the rejected secret is not retained in component state.
        setDraft("");
        setError(next.message ?? "The Exa key was not accepted.");
      }
    } catch (e) {
      if (!mountedRef.current) return;
      setError(e instanceof Error ? e.message : "Saving the Exa key failed.");
    } finally {
      inflightRef.current = false;
      if (mountedRef.current) setBusy(false);
    }
  }, [draft]);

  const handleRemove = useCallback(async () => {
    if (!pendingRemove) {
      setPendingRemove(true);
      return;
    }
    if (inflightRef.current) return;
    inflightRef.current = true;
    setBusy(true);
    setError(null);
    try {
      const next =
        await invokeBackendCommand<AuxCredentialStatus>("delete_exa_key");
      if (!mountedRef.current) return;
      setStatus(next);
      setDraft("");
    } catch (e) {
      if (!mountedRef.current) return;
      setError(e instanceof Error ? e.message : "Removing the Exa key failed.");
    } finally {
      inflightRef.current = false;
      if (mountedRef.current) {
        setBusy(false);
        setPendingRemove(false);
      }
    }
  }, [pendingRemove]);

  const badge = exaKeyBadge(status);
  const BadgeIcon = BADGE_ICON[badge.tone];
  const hasKey = status?.configured === true;

  return (
    <section
      className="rounded-2xl border border-cream-200 bg-white p-4"
      data-help-title="The Exa key powers the local coder's web search and fetch."
      data-help-lines="Optional and opt-in — the key's presence IS the switch.|With no key, the local coder's web tools stay off and it answers from the Oracle (private, on-device).|When enabled, your search queries and fetched URLs go to Exa.|The key is write-only here: it is stored in the OS keychain and never shown back."
    >
      <div className="mb-3 flex items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <Globe className="h-4 w-4 text-teal" />
          <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
            Exa web-search key
          </h3>
        </div>
        <span
          className={`inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[10px] font-medium ${BADGE_TONE[badge.tone]}`}
        >
          <BadgeIcon className="h-3 w-3" />
          {badge.label}
        </span>
      </div>

      <p className="mb-3 max-w-3xl text-[12px] leading-5 text-cream-500">
        Exa powers the local coder&apos;s web search + fetch. Optional and opt-in
        — the key presence IS the switch. No key → web tools stay off and the
        coder answers from the Oracle (private, on-device). Your queries +
        fetched URLs go to Exa when enabled.
      </p>

      <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
        <input
          type="password"
          value={draft}
          onChange={(event) => {
            setError(null);
            setPendingRemove(false);
            setDraft(event.target.value);
          }}
          placeholder={hasKey ? "Paste a new Exa key to rotate" : "Paste your Exa API key"}
          autoComplete="off"
          spellCheck={false}
          data-help-title="An Exa API key is a private credential."
          data-help-lines="Paste your Exa API key to enable the local coder's web search + fetch.|The value is write-only here: it goes straight to the OS keychain and is never displayed back.|Saving with no key, or clearing, keeps web tools off (Oracle-only, on-device).|Rotate by pasting a new key; clear to disable web egress."
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

      {hasKey ? (
        <p className="mt-1.5 text-[11px] text-cream-400">
          Saved <span className="font-mono text-cream-300">••••••••</span>{" "}
          (hidden). Web search + fetch are enabled for the local coder.
        </p>
      ) : (
        <p className="mt-1.5 text-[11px] text-cream-400">
          No key — the local coder&apos;s web tools stay off; it answers from the
          Oracle (private, on-device).
        </p>
      )}

      {error && (
        <p className="mt-2 flex items-start gap-2 rounded-2xl border border-coral/30 bg-coral/[0.05] px-3 py-2 text-[11px] leading-4 text-coral-dark">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>{error}</span>
        </p>
      )}
    </section>
  );
}

export default ExaSearchKeyCard;
