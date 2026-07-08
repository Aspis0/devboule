import { useCallback, useEffect, useRef, useState } from "react";
import { Puzzle, AlertTriangle, Search, Trash2, Download } from "lucide-react";
import { invokeBackendCommand } from "../../context/AppContext";
import { CollapsibleSection } from "./CollapsibleSection";

interface ExtensionStatus {
  agentDir: string;
  mode: "global" | "appManaged" | "envOverride";
  bootstrap: "idle" | "running" | "done" | "failed";
  bootstrapError: string | null;
}

interface InstalledExtension {
  source: string;
  name: string;
  version: string;
  description: string;
  author: string;
  installedOk: boolean;
}

interface MarketplaceExtension {
  name: string;
  version: string;
  description: string;
  author: string;
  date: string;
}

const MODE_LABEL: Record<ExtensionStatus["mode"], string> = {
  global: "shared with pi CLI",
  appManaged: "app-managed",
  envOverride: "env override",
};

export function PiExtensionsCard() {
  const [status, setStatus] = useState<ExtensionStatus | null>(null);
  const [installed, setInstalled] = useState<InstalledExtension[]>([]);
  const [loading, setLoading] = useState(true);
  // Fix 1: arm/disarm on mount/unmount so every guard is live.
  const mountedRef = useRef(true);
  // Fix 2: separate generation counters so concurrent loadAll / handleSearch
  // never invalidate each other's seq guard.
  const loadSeqRef = useRef(0);
  const searchSeqRef = useRef(0);

  // Install-by-source
  const [installSource, setInstallSource] = useState("");
  const [installBusy, setInstallBusy] = useState(false);
  const [installError, setInstallError] = useState<string | null>(null);

  // Remove-per-row: source → error (spinner is per-row via removingSource)
  const [removingSource, setRemovingSource] = useState<string | null>(null);
  const [removeErrors, setRemoveErrors] = useState<Record<string, string>>({});

  // Marketplace
  const [searchText, setSearchText] = useState("");
  const [marketplaceResults, setMarketplaceResults] = useState<MarketplaceExtension[]>([]);
  const [searchBusy, setSearchBusy] = useState(false);
  const [marketplaceError, setMarketplaceError] = useState<string | null>(null);
  const [installingMarketplaceSource, setInstallingMarketplaceSource] = useState<string | null>(null);
  // Deferred search: track whether the marketplace section has been expanded at
  // least once so the auto-search only fires on first expand, not on mount.
  const marketplaceSearchedRef = useRef(false);

  // ── Fix 1: mount/unmount lifecycle ─────────────────────────────────────

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // ── Load status + installed list (with seq guard) ──────────────────────

  const loadAll = useCallback(async () => {
    const seq = loadSeqRef.current + 1;
    loadSeqRef.current = seq;
    setLoading(true);
    try {
      const [s, list] = await Promise.all([
        invokeBackendCommand<ExtensionStatus>("pi_extensions_status"),
        invokeBackendCommand<InstalledExtension[]>("pi_extensions_list"),
      ]);
      if (!mountedRef.current || loadSeqRef.current !== seq) return;
      setStatus(s);
      setInstalled(list);
    } catch {
      // Status fetch failed — card still renders with error
    } finally {
      if (mountedRef.current && loadSeqRef.current === seq) setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  // ── Bootstrap poll ──────────────────────────────────────────────────────

  useEffect(() => {
    if (status?.bootstrap !== "running") return;
    const id = setInterval(() => {
      void loadAll();
    }, 2000);
    return () => clearInterval(id);
  }, [status?.bootstrap, loadAll]);

  // ── Shared install flow (Fix 4) ────────────────────────────────────────

  const doInstall = useCallback(
    async (source: string, clearInput: boolean) => {
      if (installBusy) return; // re-entrancy guard
      setInstallBusy(true);
      setInstallError(null);
      try {
        await invokeBackendCommand("pi_extension_install", { source });
        if (clearInput) setInstallSource("");
        await loadAll();
      } catch (e) {
        if (mountedRef.current) setInstallError(String(e));
      } finally {
        if (mountedRef.current) setInstallBusy(false);
      }
    },
    [installBusy, loadAll],
  );

  // ── Remove (per-row visual, global re-entrancy guard — Fix 7) ──────────

  const handleRemove = useCallback(
    async (source: string) => {
      if (removingSource) return; // block concurrent removes
      setRemovingSource(source);
      setRemoveErrors((prev) => ({ ...prev, [source]: "" }));
      try {
        await invokeBackendCommand("pi_extension_remove", { source });
        if (mountedRef.current) await loadAll();
      } catch (e) {
        if (mountedRef.current) {
          setRemoveErrors((prev) => ({ ...prev, [source]: String(e) }));
        }
      } finally {
        if (mountedRef.current) setRemovingSource(null);
      }
    },
    [removingSource, loadAll],
  );

  // ── Marketplace search (with seq guard — Fix 2) ────────────────────────

  const handleSearch = useCallback(async () => {
    const seq = searchSeqRef.current + 1;
    searchSeqRef.current = seq;
    setSearchBusy(true);
    setMarketplaceError(null);
    try {
      const results = await invokeBackendCommand<MarketplaceExtension[]>(
        "pi_marketplace_search",
        searchText.trim() ? { query: searchText.trim() } : undefined,
      );
      if (!mountedRef.current || searchSeqRef.current !== seq) return;
      setMarketplaceResults(results);
    } catch (e) {
      if (!mountedRef.current || searchSeqRef.current !== seq) return;
      setMarketplaceError(String(e));
    } finally {
      if (mountedRef.current && searchSeqRef.current === seq) setSearchBusy(false);
    }
  }, [searchText]);

  // Deferred marketplace search: fire only on first expand, not on mount.
  const handleMarketplaceExpand = useCallback(() => {
    if (!marketplaceSearchedRef.current) {
      marketplaceSearchedRef.current = true;
      void handleSearch();
    }
  }, [handleSearch]);

  // ── Helpers ─────────────────────────────────────────────────────────────

  const npmSourceFor = (name: string) => `npm:${name}`;

  const isInstalledNpm = (name: string) =>
    installed.some((ext) => ext.source === npmSourceFor(name));

  const isBootstrapRunning = status?.bootstrap === "running";
  const isBootstrapFailed = status?.bootstrap === "failed";

  return (
    <CollapsibleSection
      title="pi Extensions"
      badge={installed.length || undefined}
      defaultOpen={false}
    >
      <div className="rounded-2xl border border-cream-200 bg-white p-5">
        {/* ── Header + status line ── */}
        <div className="mb-3 flex items-center gap-3">
          <div className="flex h-9 w-9 items-center justify-center rounded-xl bg-teal/10">
            <Puzzle className="h-4 w-4 text-teal" />
          </div>
          <div>
            <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
              pi Extensions
            </h3>
          </div>
        </div>

        {loading && !status ? (
          <p className="text-[11px] text-cream-400">Loading extension info…</p>
        ) : status ? (
          <>
            <p className="text-[11px] text-cream-400">
              Agent dir:{" "}
              <span className="font-mono text-cream-600">{status.agentDir}</span>{" "}
              ({MODE_LABEL[status.mode]})
            </p>

            {isBootstrapRunning && (
              <p className="mt-2 flex items-center gap-1.5 text-[11px] text-teal">
                <span className="inline-block h-3 w-3 animate-spin rounded-full border-2 border-teal border-t-transparent" />
                Installing the starter extension set…
              </p>
            )}

            {/* Fix 6: show generic message when bootstrap failed but error is null */}
            {isBootstrapFailed && (
              <p className="mt-2 flex items-start gap-2 rounded-xl border border-coral/30 bg-coral/5 px-3 py-2 text-[11px] leading-4 text-coral-dark">
                <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                <span>
                  {status.bootstrapError ??
                    "Extension bootstrap failed."}{" "}
                  Retry happens on next app launch.
                </span>
              </p>
            )}

            {/* ── Installed list (collapsible sub-section, open by default) ── */}
            <CollapsibleSection
              title="Installed"
              badge={installed.length || undefined}
              defaultOpen={true}
            >
              {installed.length === 0 ? (
                <p className="text-[12px] text-cream-400">
                  No extensions installed yet.
                </p>
              ) : (
                <ul className="space-y-2">
                  {installed.map((ext) => (
                    <li
                      key={ext.source}
                      className="flex flex-col gap-1 rounded-xl bg-cream-50 px-3 py-2"
                    >
                      <div className="flex items-center justify-between gap-2">
                        <div className="min-w-0 flex-1">
                          <div className="flex items-center gap-2">
                            <span className="text-[12px] font-semibold text-cream-800">
                              {ext.name}
                            </span>
                            <span className="rounded-full bg-cream-200 px-1.5 py-0.5 text-[10px] text-cream-600">
                              {ext.version}
                            </span>
                            {!ext.installedOk && (
                              <span className="rounded-full bg-amber/10 px-1.5 py-0.5 text-[10px] font-semibold text-amber-dark">
                                broken install
                              </span>
                            )}
                          </div>
                          {ext.description && (
                            <p className="mt-0.5 line-clamp-2 text-[11px] text-cream-500">
                              {ext.description}
                            </p>
                          )}
                          {ext.author && (
                            <p className="mt-0.5 text-[10px] text-cream-400">
                              {ext.author}
                            </p>
                          )}
                        </div>
                        {/* Fix 7: per-row visual — only the active row shows spinner+disabled */}
                        <button
                          onClick={() => void handleRemove(ext.source)}
                          disabled={removingSource === ext.source}
                          data-testid={`remove-${ext.source}`}
                          className="inline-flex shrink-0 items-center gap-1.5 rounded-xl border border-cream-200 bg-white px-2.5 py-1.5 text-[11px] font-semibold text-cream-600 transition-colors hover:border-coral/30 hover:text-coral-dark disabled:cursor-not-allowed disabled:opacity-60"
                        >
                          {removingSource === ext.source ? (
                            <span className="animate-spin h-3 w-3 rounded-full border-2 border-cream-400 border-t-transparent" />
                          ) : (
                            <Trash2 className="h-3 w-3" />
                          )}
                          Remove
                        </button>
                      </div>
                      {removeErrors[ext.source] && (
                        <p className="text-[11px] text-coral-dark">
                          {removeErrors[ext.source]}
                        </p>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </CollapsibleSection>

            {/* ── Install by source ── */}
            <div className="mt-4">
              <h4 className="text-[10px] font-semibold uppercase tracking-widest text-cream-400">
                Install by source
              </h4>
              <div className="mt-2 flex gap-2">
                <input
                  placeholder="npm:pi-lens · git:github.com/o/r"
                  value={installSource}
                  onChange={(e) => setInstallSource(e.target.value)}
                  /* Fix 5: Enter-key checks busy flag */
                  onKeyDown={(e) => {
                    if (
                      e.key === "Enter" &&
                      installSource.trim() &&
                      !installBusy
                    ) {
                      void doInstall(installSource.trim(), true);
                    }
                  }}
                  className="flex-1 rounded-xl border border-cream-200 bg-white px-3 py-1.5 text-[12px] text-cream-800 placeholder:text-cream-300 focus:border-teal/40 focus:outline-none"
                />
                <button
                  onClick={() => void doInstall(installSource.trim(), true)}
                  disabled={!installSource.trim() || installBusy}
                  data-testid="install-source-btn"
                  className="inline-flex shrink-0 items-center gap-1.5 rounded-xl bg-cream-800 px-3 py-1.5 text-[12px] font-semibold text-cream-50 transition-colors hover:bg-cream-700 disabled:cursor-not-allowed disabled:opacity-60"
                >
                  {installBusy ? (
                    <span className="animate-spin h-3 w-3 rounded-full border-2 border-cream-300 border-t-transparent" />
                  ) : (
                    <Download className="h-3 w-3" />
                  )}
                  Install
                </button>
              </div>
              {installError && (
                <p className="mt-2 text-[11px] text-coral-dark">{installError}</p>
              )}
            </div>
          </>
        ) : null}

        {/* ── Marketplace (collapsible sub-section, collapsed by default, deferred search) ── */}
        <CollapsibleSection
          title="Marketplace"
          defaultOpen={false}
          onExpand={handleMarketplaceExpand}
        >
          <div className="flex gap-2">
            <input
              placeholder="Search extensions…"
              value={searchText}
              onChange={(e) => setSearchText(e.target.value)}
              /* Fix 5: Enter-key checks busy flag */
              onKeyDown={(e) => {
                if (e.key === "Enter" && !searchBusy) void handleSearch();
              }}
              className="flex-1 rounded-xl border border-cream-200 bg-white px-3 py-1.5 text-[12px] text-cream-800 placeholder:text-cream-300 focus:border-teal/40 focus:outline-none"
            />
            <button
              onClick={() => void handleSearch()}
              disabled={searchBusy}
              data-testid="marketplace-search-btn"
              className="inline-flex shrink-0 items-center gap-1.5 rounded-xl border border-cream-200 bg-cream-50 px-3 py-1.5 text-[12px] font-semibold text-cream-700 transition-colors hover:bg-cream-100 disabled:opacity-40"
            >
              {searchBusy ? (
                <span className="animate-spin h-3 w-3 rounded-full border-2 border-cream-400 border-t-transparent" />
              ) : (
                <Search className="h-3 w-3" />
              )}
              Search
            </button>
          </div>

          {marketplaceError && (
            <p className="mt-2 rounded-xl border border-coral/30 bg-coral/5 px-3 py-2 text-[11px] text-coral-dark">
              {marketplaceError}
            </p>
          )}

          {marketplaceResults.length > 0 && (
            <ul className="mt-3 space-y-1.5">
              {marketplaceResults.map((ext) => {
                const npmSrc = npmSourceFor(ext.name);
                const alreadyInstalled = isInstalledNpm(ext.name);
                const isInstallingThis = installingMarketplaceSource === npmSrc;
                return (
                  <li
                    key={ext.name}
                    className="flex items-center justify-between gap-2 rounded-xl bg-cream-50 px-3 py-2"
                  >
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-2">
                        <span className="text-[12px] font-semibold text-cream-800">
                          {ext.name}
                        </span>
                        <span className="rounded-full bg-cream-200 px-1.5 py-0.5 text-[10px] text-cream-600">
                          {ext.version}
                        </span>
                      </div>
                      {ext.description && (
                        <p className="mt-0.5 line-clamp-1 text-[11px] text-cream-500">
                          {ext.description}
                        </p>
                      )}
                      <p className="mt-0.5 text-[10px] text-cream-400">
                        {ext.author}
                        {ext.date ? ` · ${ext.date}` : ""}
                      </p>
                    </div>
                    {alreadyInstalled ? (
                      <span className="shrink-0 rounded-xl bg-sage/10 px-2.5 py-1.5 text-[11px] font-semibold text-sage-dark">
                        Installed
                      </span>
                    ) : (
                      /* Fix 4: marketplace rows use shared doInstall (clearInput=false) */
                      <button
                        onClick={() => {
                          setInstallingMarketplaceSource(npmSrc);
                          void doInstall(npmSrc, false).finally(() => {
                            if (mountedRef.current) setInstallingMarketplaceSource(null);
                          });
                        }}
                        disabled={installBusy}
                        data-testid={`install-marketplace-${ext.name}`}
                        className="inline-flex shrink-0 items-center gap-1.5 rounded-xl bg-cream-800 px-2.5 py-1.5 text-[11px] font-semibold text-cream-50 transition-colors hover:bg-cream-700 disabled:cursor-not-allowed disabled:opacity-60"
                      >
                        {isInstallingThis ? (
                          <span className="animate-spin h-3 w-3 rounded-full border-2 border-cream-300 border-t-transparent" />
                        ) : (
                          <Download className="h-3 w-3" />
                        )}
                        Install
                      </button>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </CollapsibleSection>

        <p className="mt-4 text-[10px] text-cream-400">
          Marketplace search failures are non-fatal — the installed extensions list
          above is always shown.
        </p>
      </div>
    </CollapsibleSection>
  );
}
