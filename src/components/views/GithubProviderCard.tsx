import { useCallback, useEffect, useRef, useState } from "react";
import {
  Github,
  RotateCw,
  Trash2,
  DownloadCloud,
  CheckCircle2,
  AlertTriangle,
  Clock,
} from "lucide-react";
import { invokeBackendCommand } from "../../context/AppContext";
import type { GithubConnectionStatus } from "../../types/backend";
import {
  githubCardPill,
  shouldShowGithubImportButton,
  shouldShowGithubRemoveButton,
  loadGithubStatus,
  saveGithubToken,
  importGithubTokenFromCli,
  deleteGithubToken,
  type GithubPillTone,
} from "./githubCardModel";

// Status pill tone -> cream/terracotta/teal classes, matching SecretsView's
// statusConfig idiom (sage = good, coral = broken, amber = attention).
const pillTone: Record<GithubPillTone, { bg: string; text: string }> = {
  valid: { bg: "bg-sage/10", text: "text-sage-dark" },
  error: { bg: "bg-coral/10", text: "text-coral-dark" },
  missing: { bg: "bg-amber/10", text: "text-amber-dark" },
  checking: { bg: "bg-cream-100", text: "text-cream-500" },
};

const pillIcon: Record<GithubPillTone, typeof CheckCircle2> = {
  valid: CheckCircle2,
  error: AlertTriangle,
  missing: AlertTriangle,
  checking: Clock,
};

/**
 * GitHub provider card. Mirrors the Cloudflare/Scaleway token rows in
 * SecretsView: a WRITE-ONLY token paste field, Save, Disconnect, and an
 * "Import from GitHub CLI" button gated on `cliAvailable`. The token value is
 * NEVER read back into the field — `get_github_connection_status` returns only
 * non-secret metadata (login/scopes/...), and the backend `sanitize_error`
 * redacts any token prefix that might appear in a `gh` error.
 */
export function GithubProviderCard() {
  const [status, setStatus] = useState<GithubConnectionStatus | null>(null);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pendingRemove, setPendingRemove] = useState(false);
  // Guard against setState after unmount (the load/save calls are async).
  const mountedRef = useRef(true);
  // Synchronous in-flight lock: `busy` state updates asynchronously, so a fast
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
      const next = await loadGithubStatus(invokeBackendCommand);
      if (!mountedRef.current) return;
      setStatus(next);
    } catch (e) {
      if (!mountedRef.current) return;
      setError(e instanceof Error ? e.message : "Could not read GitHub status.");
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const handleSave = useCallback(async () => {
    const token = draft.trim();
    if (!token) return;
    if (inflightRef.current) return;
    inflightRef.current = true;
    setBusy(true);
    setError(null);
    try {
      const next = await saveGithubToken(invokeBackendCommand, token);
      if (!mountedRef.current) return;
      setStatus(next);
      // Cancel a pending Disconnect confirmation the user has moved on from.
      setPendingRemove(false);
      // Surface the backend's validation/rejection message inline (e.g. token
      // too short / rejected by GitHub) instead of silently dropping the draft.
      if (next.status === "valid") {
        setDraft("");
      } else {
        setError(next.message ?? "GitHub did not accept the token.");
      }
    } catch (e) {
      if (!mountedRef.current) return;
      setError(e instanceof Error ? e.message : "Saving the GitHub token failed.");
    } finally {
      inflightRef.current = false;
      setBusy(false);
    }
  }, [draft]);

  const handleImport = useCallback(async () => {
    if (inflightRef.current) return;
    inflightRef.current = true;
    setBusy(true);
    setError(null);
    try {
      const next = await importGithubTokenFromCli(invokeBackendCommand);
      if (!mountedRef.current) return;
      setStatus(next);
      // Cancel a pending Disconnect confirmation the user has moved on from.
      setPendingRemove(false);
      if (next.status !== "valid" && next.message) setError(next.message);
    } catch (e) {
      if (!mountedRef.current) return;
      // The backend already ran `gh`'s output through sanitize_error, so any
      // surfaced message has token prefixes redacted.
      setError(e instanceof Error ? e.message : "GitHub CLI import failed.");
    } finally {
      inflightRef.current = false;
      setBusy(false);
    }
  }, []);

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
      const next = await deleteGithubToken(invokeBackendCommand);
      if (!mountedRef.current) return;
      setStatus(next);
      setDraft("");
    } catch (e) {
      if (!mountedRef.current) return;
      setError(e instanceof Error ? e.message : "Removing the GitHub token failed.");
    } finally {
      inflightRef.current = false;
      setBusy(false);
      setPendingRemove(false);
    }
  }, [pendingRemove]);

  const pill = githubCardPill(status);
  const PillIcon = pillIcon[pill.tone];
  const tone = pillTone[pill.tone];
  const showImport = shouldShowGithubImportButton(status);
  const showRemove = shouldShowGithubRemoveButton(status);
  const scopes = status?.scopes ?? [];

  return (
    <div
      className="px-5 py-4"
      data-help-title="GitHub credentials let Aspis Management work with your private repositories."
      data-help-lines="Save a fine-grained GitHub token (or import your GitHub CLI login) so the app can clone, pull, and push inside Aspis Management.|The token is stored in the OS keychain, exactly like the Cloudflare and Scaleway tokens, and is never shown back here.|It is never copied into projects, Oracle, chat, or source files.|If the token expires or leaks, remove it here and rotate it on GitHub."
    >
      <div className="flex items-start gap-4 min-w-0">
        <div className="w-10 h-10 rounded-xl bg-cream-50 flex items-center justify-center shrink-0">
          <Github className="w-5 h-5 text-cream-500" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2 mb-0.5">
            <p className="text-[14px] font-medium text-cream-800">GitHub</p>
            <span className="text-[10px] font-medium px-2 py-0.5 rounded-full bg-cream-100 text-cream-500">
              github
            </span>
          </div>
          <div className="flex flex-wrap items-center gap-3">
            <span
              className={`inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-[10px] font-medium ${tone.bg} ${tone.text}`}
            >
              <PillIcon className="w-3 h-3" />
              {pill.label}
            </span>
            {status?.lastCheckedAt && (
              <span className="text-[11px] text-cream-400">
                Checked {status.lastCheckedAt}
              </span>
            )}
            {status?.message && (
              <span className="text-[11px] text-amber-dark font-medium">
                {status.message}
              </span>
            )}
          </div>
          <p className="text-[11px] text-cream-400 mt-1">
            Stored in the OS keychain (same as Cloudflare and Scaleway). The
            token enables in-app Clone, Pull, and Push. Use a fine-grained token
            scoped to the repositories you coordinate.
          </p>
          {scopes.length > 0 && (
            <p className="mt-1 text-[10px] text-cream-400">
              Scopes: {scopes.join(", ")}
            </p>
          )}

          <div className="mt-3 flex flex-col gap-2 sm:flex-row sm:items-center">
            <input
              type="password"
              value={draft}
              onChange={(event) => {
                setError(null);
                setPendingRemove(false);
                setDraft(event.target.value);
              }}
              placeholder="Paste GitHub fine-grained token"
              data-help-title="A GitHub token is a private credential for your repositories."
              data-help-lines="Paste a fine-grained personal access token with access to the repositories you coordinate.|The value is write-only here: it goes straight to the OS keychain and is never displayed back.|Prefer narrow repository scopes over a classic all-repo token.|Rotate and re-paste a new token here when GitHub checks start failing."
              autoComplete="off"
              spellCheck={false}
              className="min-w-0 flex-1 rounded-xl border border-cream-200 bg-cream-50 px-3 py-2 text-[12px] font-mono text-cream-800 outline-none focus:border-terracotta-200 focus:ring-2 focus:ring-terracotta/15"
            />
            <button
              onClick={() => void handleSave()}
              disabled={busy || draft.trim().length === 0}
              data-help-title="This saves or rotates the GitHub token."
              data-help-lines="The token is validated against GitHub and then stored in the OS keychain through the backend.|If GitHub rejects it or it is too short, the error is shown here and nothing is saved.|It is never exposed to Oracle, agents, or project files.|Rotate it here whenever it expires or is revoked."
              className="flex items-center justify-center gap-1.5 px-3 py-2 rounded-xl
                         bg-terracotta text-white text-[12px] font-medium
                         hover:bg-terracotta-500 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-terracotta/30
                         disabled:cursor-not-allowed disabled:opacity-50 transition-all duration-200"
            >
              <RotateCw className="w-3.5 h-3.5" />
              {showRemove ? "Rotate" : "Save"}
            </button>
            {showImport && (
              <button
                onClick={() => void handleImport()}
                disabled={busy}
                data-help-title="This imports your existing GitHub CLI login."
                data-help-lines="If the GitHub CLI (gh) is signed in on this machine, this copies its token into the OS keychain.|The token is validated before it is stored; any CLI error shown here has token values redacted.|This is a convenience over pasting a token manually.|It does not change your GitHub CLI login."
                className="flex items-center justify-center gap-1.5 px-3 py-2 rounded-xl
                           border border-cream-200 text-[12px] font-medium text-cream-600
                           hover:border-teal-200 hover:text-teal hover:bg-teal/5
                           focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-teal/30
                           disabled:cursor-not-allowed disabled:opacity-50 transition-all duration-200"
              >
                <DownloadCloud className="w-3.5 h-3.5" />
                Import from GitHub CLI
              </button>
            )}
            {showRemove && (
              <button
                onClick={() => void handleRemove()}
                disabled={busy}
                data-help-title="This removes the saved GitHub token."
                data-help-lines="Removing deletes the local keychain copy only.|It does not revoke the token on GitHub — rotate it there too if it leaked.|In-app Clone, Pull, and Push will fail until a valid token is saved again.|Use this when the token expires, leaks, or has the wrong repository access."
                className="flex items-center justify-center gap-1.5 px-3 py-2 rounded-xl
                           border border-cream-200 text-[12px] font-medium text-cream-600
                           hover:border-coral/30 hover:text-coral hover:bg-coral/5
                           focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-coral/20
                           disabled:cursor-not-allowed disabled:opacity-50 transition-all duration-200"
              >
                <Trash2 className="w-3.5 h-3.5" />
                {pendingRemove ? "Confirm" : "Disconnect"}
              </button>
            )}
          </div>
          {showRemove && (
            <p className="mt-1.5 text-[11px] text-cream-400">
              Saved <span className="font-mono text-cream-300">••••••••</span>{" "}
              (hidden)
            </p>
          )}
          {error && (
            <p className="mt-2 text-[11px] font-medium text-coral-dark">{error}</p>
          )}
        </div>
      </div>
    </div>
  );
}
