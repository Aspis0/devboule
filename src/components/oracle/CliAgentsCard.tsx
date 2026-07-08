// CliAgentsCard — registers the Oracle MCP in the local `claude`/`codex` user
// config. Oracle-specific (registers the `aspis-management` Oracle MCP server),
// not a general CLI-agents feature. Moved here from SettingsView's Account tab
// so all Oracle surfaces live on the Oracle page.
import { useCallback, useEffect, useRef, useState } from "react";
import {
  Terminal,
  CheckCircle2,
  MinusCircle,
  AlertTriangle,
  Info,
  Trash2,
} from "lucide-react";
import { useAppActions } from "../../context/AppContext";
import { toOracleError } from "../../utils/oracleError";
import type { CliAgentsStatus } from "../../types/backend";

export function CliAgentsCard() {
  const { configureCliAgents, cliAgentsStatus, unconfigureCliAgents } =
    useAppActions();
  const [status, setStatus] = useState<CliAgentsStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);
  // Guards out-of-order resolves (rapid Configure/Remove, or unmount mid-flight).
  const runSeqRef = useRef(0);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const loadStatus = useCallback(async () => {
    const seq = runSeqRef.current + 1;
    runSeqRef.current = seq;
    setLoading(true);
    setError(null);
    try {
      const next = await cliAgentsStatus();
      if (!mountedRef.current || runSeqRef.current !== seq) return;
      setStatus(next);
    } catch (e) {
      if (!mountedRef.current || runSeqRef.current !== seq) return;
      setError(toOracleError(e).message);
      setStatus(null);
    } finally {
      if (mountedRef.current && runSeqRef.current === seq) setLoading(false);
    }
  }, [cliAgentsStatus]);

  useEffect(() => {
    void loadStatus();
  }, [loadStatus]);

  // Both the primary and the secondary action run a mutating command and then
  // adopt its returned status directly (the commands echo the fresh status), so
  // no extra round-trip is needed. The seq guard still protects against unmount.
  const runAction = useCallback(
    async (action: () => Promise<CliAgentsStatus>) => {
      const seq = runSeqRef.current + 1;
      runSeqRef.current = seq;
      setBusy(true);
      // runAction supersedes any in-flight loadStatus; clear loading so a
      // loadStatus that bails out of its own finally (seq mismatch) never leaves
      // loading stuck true.
      setLoading(false);
      setError(null);
      try {
        const next = await action();
        if (!mountedRef.current || runSeqRef.current !== seq) return;
        setStatus(next);
      } catch (e) {
        if (!mountedRef.current || runSeqRef.current !== seq) return;
        setError(toOracleError(e).message);
      } finally {
        if (mountedRef.current && runSeqRef.current === seq) setBusy(false);
      }
    },
    [],
  );

  const runtimeReady = status?.runtimeReady ?? false;
  // The backend fail-closes configure when the runtime is not usable; mirror that
  // intent in the UI by disabling the primary button until the runtime is ready.
  const configureDisabled = loading || busy || !runtimeReady;

  return (
    <section className="rounded-2xl border border-cream-200 bg-white p-5">
      <div className="mb-3 flex items-center gap-3">
        <div className="flex h-9 w-9 items-center justify-center rounded-xl bg-amber/10">
          <Terminal className="h-4 w-4 text-amber-dark" />
        </div>
        <div>
          <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
            CLI agents
          </h3>
          <p className="text-[11px] text-cream-400">
            Give the local Claude/Codex CLI the Oracle MCP
          </p>
        </div>
      </div>

      <p className="text-[12px] leading-5 text-cream-600">
        Registers the Oracle MCP in your user config so any{" "}
        <span className="font-mono">claude</span> you open in a terminal already
        has it. Writes <span className="font-mono">~/.claude.json</span> (backed
        up first). No token is stored there.
      </p>

      {loading ? (
        <p className="mt-3 text-[11px] text-cream-400">Checking local CLI config…</p>
      ) : status ? (
        <>
          <div className="mt-3 space-y-1.5 text-[11px]">
            <div className="flex items-center gap-2">
              {status.claudeConfigured ? (
                <CheckCircle2 className="h-3.5 w-3.5 shrink-0 text-sage-dark" />
              ) : (
                <MinusCircle className="h-3.5 w-3.5 shrink-0 text-cream-400" />
              )}
              <span className="text-cream-700">Claude</span>
              <span
                className={
                  status.claudeConfigured ? "text-sage-dark" : "text-cream-500"
                }
              >
                — {status.claudeConfigured ? "configured" : "not configured"}
              </span>
              {status.claudeConfigured && status.claudeConfigPath && (
                <span className="truncate font-mono text-[10px] text-cream-400">
                  {status.claudeConfigPath}
                </span>
              )}
            </div>
            <div className="flex items-center gap-2">
              {status.codexConfigured ? (
                <CheckCircle2 className="h-3.5 w-3.5 shrink-0 text-sage-dark" />
              ) : (
                <MinusCircle className="h-3.5 w-3.5 shrink-0 text-cream-400" />
              )}
              <span className="text-cream-700">Codex</span>
              <span
                className={
                  status.codexConfigured ? "text-sage-dark" : "text-cream-400"
                }
              >
                —{" "}
                {status.codexNote ??
                  (status.codexConfigured ? "configured" : "not configured")}
              </span>
            </div>
          </div>

          <div className="mt-3 grid gap-1 rounded-xl bg-cream-50 px-3 py-2 text-[10px] text-cream-500">
            <p>
              Interpreter:{" "}
              <span className="font-mono text-cream-600">
                {status.interpreter ?? "unresolved"}
              </span>
            </p>
            <p>
              Root:{" "}
              <span className="font-mono text-cream-600">
                {status.root ?? "unresolved"}
              </span>
            </p>
            <p>
              Runtime:{" "}
              <span
                className={
                  runtimeReady ? "text-sage-dark" : "text-amber-dark"
                }
              >
                {runtimeReady ? "ready" : "not ready"}
              </span>
            </p>
          </div>

          {status.warning && (
            <p className="mt-3 flex items-start gap-2 rounded-xl bg-cream-50 px-3 py-2 text-[11px] leading-4 text-cream-500">
              <Info className="mt-0.5 h-3.5 w-3.5 shrink-0 text-teal" />
              <span>{status.warning}</span>
            </p>
          )}
        </>
      ) : null}

      {error && (
        <p className="mt-3 flex items-start gap-2 rounded-xl border border-coral/30 bg-coral/5 px-3 py-2 text-[11px] leading-4 text-coral-dark">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>{error}</span>
        </p>
      )}

      {!loading && status && !runtimeReady && (
        <p className="mt-3 text-[10px] leading-4 text-amber-dark">
          Install the Oracle runtime first (Oracle → Setup).
        </p>
      )}

      <div className="mt-3 flex flex-wrap gap-2">
        <button
          onClick={() => void runAction(configureCliAgents)}
          disabled={configureDisabled}
          data-help-title="This registers the Oracle MCP in your local CLI config."
          data-help-lines="It writes the Oracle MCP entry into your user-scope ~/.claude.json so any claude you start in a terminal already sees it.|The file is backed up first, and no API token is written there — the MCP reaches the vault through the app.|It is disabled until the Oracle runtime is installed, because registering a broken MCP would just fail to start.|Changes apply to new terminal sessions; reopen an existing claude session to pick them up."
          className="inline-flex items-center gap-2 rounded-xl bg-cream-800 px-3 py-2 text-[12px] font-semibold text-cream-50 transition-colors hover:bg-cream-700 disabled:cursor-not-allowed disabled:opacity-60"
        >
          <Terminal className="h-3.5 w-3.5" />
          Configure CLI agents
        </button>
        {status?.claudeConfigured && (
          <button
            onClick={() => void runAction(unconfigureCliAgents)}
            disabled={loading || busy}
            data-help-title="This removes the Oracle MCP from your local CLI config."
            data-help-lines="It deletes the Oracle MCP entry from your user-scope ~/.claude.json.|It does not touch the Oracle runtime, the index, or any saved token.|Use it if you no longer want a terminal claude to reach this workspace.|Reopen any open claude session for the removal to take effect."
            className="inline-flex items-center gap-2 rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 transition-colors hover:border-coral/30 hover:text-coral-dark disabled:cursor-not-allowed disabled:opacity-60"
          >
            <Trash2 className="h-3.5 w-3.5" />
            Remove
          </button>
        )}
      </div>
    </section>
  );
}

// Test-only alias for the loading-stuck regression (C-F3).
export const __test_CliAgentsCard = CliAgentsCard;
