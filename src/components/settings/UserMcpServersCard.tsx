import {
  AlertTriangle,
  Network,
  Plus,
  RefreshCw,
  Trash2,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { invokeBackendCommand } from "../../context/AppContext";
import type { McpScope, UserMcpServer } from "../../types/userMcpServers";
import { UserMcpConsentDialog } from "./UserMcpConsentDialog";

// Shared server list UI for both global and project scopes. Extracted so both
// cards compose the same list without duplicating the rows/toggle/remove logic.
// The calling card passes the scope and (for project scope) the projectRoot.

interface McpServerListProps {
  scope: McpScope;
  projectRoot?: string;
}

export function McpServerList({ scope, projectRoot }: McpServerListProps) {
  const [servers, setServers] = useState<UserMcpServer[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showDialog, setShowDialog] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const mountedRef = useRef(true);
  const loadSeqRef = useRef(0);
  // F2: synchronous reentrancy guard for mutating handlers. useState `busy` is
  // async — two rapid clicks both observe busy===false before the first flush.
  // This ref is set synchronously at the top of each handler so the second
  // call returns immediately, before any await.
  const busyRef = useRef(false);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Build the base args for every command. project scope requires projectRoot.
  const baseArgs = useCallback((): Record<string, unknown> => {
    const a: Record<string, unknown> = { scope };
    if (scope === "project" && projectRoot) a.projectRoot = projectRoot;
    return a;
  }, [scope, projectRoot]);

  const load = useCallback(async () => {
    const seq = loadSeqRef.current + 1;
    loadSeqRef.current = seq;
    setLoading(true);
    setError(null);
    try {
      const result = await invokeBackendCommand<UserMcpServer[]>(
        "user_mcp_list",
        baseArgs(),
      );
      if (!mountedRef.current || loadSeqRef.current !== seq) return;
      setServers(Array.isArray(result) ? result : []);
    } catch (e) {
      if (!mountedRef.current || loadSeqRef.current !== seq) return;
      setError(
        e instanceof Error ? e.message : "Failed to load MCP servers.",
      );
      setServers([]);
    } finally {
      if (mountedRef.current && loadSeqRef.current === seq) setLoading(false);
    }
  }, [baseArgs]);

  useEffect(() => {
    void load();
  }, [load]);

  const toggleEnabled = useCallback(
    async (name: string, enabled: boolean) => {
      // F2: synchronous check before any state read/write.
      if (busyRef.current) return;
      busyRef.current = true;
      setBusy(true);
      setActionError(null);
      try {
        // Enabling a global server re-runs its command on launch. Backend requires
        // confirmGlobalCommand; the server was already consented at add-time.
        const args: Record<string, unknown> = {
          ...baseArgs(),
          name,
          enabled,
        };
        if (scope === "global" && enabled) {
          args.confirmGlobalCommand = true;
        }
        await invokeBackendCommand<void>("user_mcp_set_enabled", args);
        if (!mountedRef.current) return;
        // Optimistic local update, then re-fetch for consistency.
        setServers((prev) =>
          prev.map((s) => (s.name === name ? { ...s, enabled } : s)),
        );
        await load();
      } catch (e) {
        if (mountedRef.current) {
          setActionError(
            e instanceof Error ? e.message : "Failed to update server.",
          );
        }
      } finally {
        busyRef.current = false;
        if (mountedRef.current) setBusy(false);
      }
    },
    [baseArgs, load, scope],
  );

  const removeServer = useCallback(
    async (name: string) => {
      // F2: synchronous check before any state read/write.
      if (busyRef.current) return;
      busyRef.current = true;
      setBusy(true);
      setActionError(null);
      setConfirmRemove(null);
      try {
        await invokeBackendCommand<void>("user_mcp_remove", {
          ...baseArgs(),
          name,
        });
        if (!mountedRef.current) return;
        await load();
      } catch (e) {
        if (mountedRef.current) {
          setActionError(
            e instanceof Error ? e.message : "Failed to remove server.",
          );
        }
      } finally {
        busyRef.current = false;
        if (mountedRef.current) setBusy(false);
      }
    },
    [baseArgs, load],
  );

  const onAdded = useCallback(async () => {
    setShowDialog(false);
    await load();
  }, [load]);

  const enabledCount = servers.filter((s) => s.enabled).length;

  return (
    <div className="space-y-3">
      {/* Network access warning — shown when ≥1 server is enabled (§4.2) */}
      {enabledCount > 0 && (
        <p
          data-testid="network-access-warning"
          className="flex items-center gap-2 rounded-xl border border-amber/40 bg-amber/[0.07] px-3 py-2 text-[11px] text-amber-dark"
        >
          <Network className="h-3.5 w-3.5 shrink-0" />
          <span>
            {enabledCount} user MCP server{enabledCount === 1 ? "" : "s"} active
            — may have network access
          </span>
        </p>
      )}

      {/* Server list */}
      {loading ? (
        <p className="text-[11px] text-cream-400">Loading servers…</p>
      ) : error ? (
        <p className="flex items-start gap-2 rounded-xl border border-coral/30 bg-coral/5 px-3 py-2 text-[11px] text-coral-dark">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>{error}</span>
        </p>
      ) : servers.length === 0 ? (
        <p className="text-[11px] text-cream-400">
          No servers configured for this scope.
        </p>
      ) : (
        <ul className="space-y-2">
          {servers.map((server) => (
            <li
              key={server.name}
              data-testid={`server-row-${server.name}`}
              className={`rounded-xl border px-3 py-2 ${
                server.enabled
                  ? "border-cream-200 bg-white"
                  : "border-cream-100 bg-cream-50 opacity-60"
              }`}
            >
              <div className="flex items-center justify-between gap-2">
                <div className="min-w-0 flex-1">
                  <p
                    className={`truncate text-[12px] font-semibold ${
                      server.enabled ? "text-cream-800" : "text-cream-500"
                    }`}
                  >
                    {server.name}
                  </p>
                  {/* F5: title exposes full value on hover (truncation can hide
                      meaningful differences); dimmed separator disambiguates
                      command from args when they share a common prefix. */}
                  <p
                    className="truncate font-mono text-[10px] text-cream-400"
                    title={
                      server.args.length > 0
                        ? `${server.command} ${server.args.join(" ")}`
                        : server.command
                    }
                  >
                    {server.command}
                    {server.args.length > 0 && (
                      <>
                        <span className="mx-0.5 opacity-40">·</span>
                        <span>{server.args.join(" ")}</span>
                      </>
                    )}
                  </p>
                </div>

                <div className="flex shrink-0 items-center gap-2">
                  {/* Enable / disable toggle */}
                  <button
                    type="button"
                    data-testid={`toggle-${server.name}`}
                    onClick={() =>
                      void toggleEnabled(server.name, !server.enabled)
                    }
                    disabled={busy}
                    className={`rounded-full px-2.5 py-1 text-[10px] font-semibold transition-colors disabled:opacity-60 ${
                      server.enabled
                        ? "bg-sage/15 text-sage-dark hover:bg-sage/25"
                        : "bg-cream-100 text-cream-500 hover:bg-cream-200"
                    }`}
                  >
                    {server.enabled ? "Enabled" : "Disabled"}
                  </button>

                  {/* Remove — with a confirm step */}
                  {confirmRemove === server.name ? (
                    <div className="flex items-center gap-1">
                      <button
                        type="button"
                        data-testid={`confirm-remove-${server.name}`}
                        onClick={() => void removeServer(server.name)}
                        disabled={busy}
                        className="rounded-md bg-coral px-2 py-1 text-[10px] font-semibold text-white hover:bg-coral-dark disabled:opacity-60"
                      >
                        Confirm
                      </button>
                      <button
                        type="button"
                        onClick={() => setConfirmRemove(null)}
                        disabled={busy}
                        className="rounded-md border border-cream-200 bg-white px-2 py-1 text-[10px] font-semibold text-cream-500 hover:text-cream-700 disabled:opacity-60"
                      >
                        Keep
                      </button>
                    </div>
                  ) : (
                    <button
                      type="button"
                      data-testid={`remove-${server.name}`}
                      onClick={() => setConfirmRemove(server.name)}
                      disabled={busy}
                      className="rounded-md border border-cream-100 bg-white p-1 text-cream-400 hover:border-coral/30 hover:text-coral-dark disabled:opacity-60"
                      aria-label={`Remove ${server.name}`}
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </button>
                  )}
                </div>
              </div>
            </li>
          ))}
        </ul>
      )}

      {/* Action error from toggle / remove */}
      {actionError && (
        <p className="flex items-start gap-2 rounded-xl border border-coral/30 bg-coral/5 px-3 py-2 text-[11px] text-coral-dark">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>{actionError}</span>
        </p>
      )}

      {/* Toolbar: Add + Refresh */}
      <div className="flex items-center gap-2">
        <button
          type="button"
          data-testid="add-server-btn"
          onClick={() => {
            setActionError(null);
            setConfirmRemove(null);
            setShowDialog(true);
          }}
          disabled={busy || loading}
          className="inline-flex items-center gap-1.5 rounded-md bg-amber-dark px-3 py-2 text-[12px] font-semibold text-white hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-60"
        >
          <Plus className="h-3.5 w-3.5" />
          Add server
        </button>
        <button
          type="button"
          onClick={() => void load()}
          disabled={busy || loading}
          className="inline-flex items-center gap-1.5 rounded-md border border-cream-200 bg-white px-3 py-2 text-[11px] font-semibold text-cream-500 hover:text-cream-700 disabled:opacity-60"
        >
          <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
          Refresh
        </button>
      </div>

      {/* Consent + add dialog */}
      {showDialog && (
        <UserMcpConsentDialog
          scope={scope}
          projectRoot={projectRoot}
          onAdded={() => void onAdded()}
          onCancel={() => setShowDialog(false)}
        />
      )}
    </div>
  );
}

// Settings card: GLOBAL scope. The Oracle (`devboule`) is a built-in and
// is excluded from this list by the backend (the backend never puts it in the
// user-mcp-servers.json file). No projectRoot is needed for global scope.
export function UserMcpServersCard() {
  return (
    <section
      className="rounded-2xl border border-cream-200 bg-white p-4"
      data-help-title="Global user MCP servers — available in every project."
      data-help-lines="MCP servers run as your user account, can access your files, and may reach external networks.|Each server is added via a consent dialog so you always see what command and env keys will be used.|Disable a server to soft-remove it without losing its config.|The Devboule Oracle is always present separately and never appears here.|Project-scoped servers live inside each project's workspace settings."
    >
      <div className="mb-3 flex items-center gap-2">
        <Network className="h-4 w-4 text-amber-dark" />
        <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          User MCP servers (global)
        </h3>
      </div>
      <p className="mb-4 max-w-3xl text-[12px] leading-5 text-cream-500">
        MCP servers available in every project. Each server runs as your user
        account, can read files, and may reach external networks. The Devboule
        Oracle is always present separately and does not appear here. For
        project-specific servers, use the MCP tab in the project workspace.
      </p>
      <McpServerList scope="global" />
    </section>
  );
}

export const __test_UserMcpServersCard = UserMcpServersCard;
export const __test_McpServerList = McpServerList;
