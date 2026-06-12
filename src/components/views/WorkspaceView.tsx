import {
  AlertTriangle,
  CheckCircle2,
  Copy,
  Cpu,
  Database,
  Download,
  FileWarning,
  FolderOpen,
  GitBranch,
  HardDrive,
  PackageCheck,
  Plus,
  RefreshCw,
  ShieldCheck,
  Terminal,
  Trash2,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  invokeBackendCommand,
  useAppActions,
  useAppContext,
} from "../../context/AppContext";
import {
  validateCustomClient,
  slugifyClientId,
  CLIENT_LABEL_MAX_LENGTH,
  CLIENT_COMMAND_MAX_LENGTH,
} from "../agents/customAgentClients";
import {
  validateCensorLocalAi,
  CENSOR_AI_PROVIDERS,
  CENSOR_MODEL_MAX_LENGTH,
  CENSOR_BASE_URL_MAX_LENGTH,
} from "../projects/censorLocalAi";
import type {
  CensorAiProvider,
  CensorLocalAi,
  CustomAgentClient,
} from "../../types/config";
import type {
  WorkspaceClassificationEntry,
  WorkspaceDecryptResult,
  WorkspaceGitRepoStatus,
  WorkspaceHygieneSnapshot,
  WorkspaceLargeFile,
  WorkspacePackageInfo,
  WorkspacePackageResult,
  WorkspacePackageSnapshot,
  WorkspaceSizeEntry,
} from "../../types/backend";

function formatGb(value: number) {
  if (!Number.isFinite(value)) return "0 GB";
  if (value >= 10) return `${value.toFixed(1)} GB`;
  if (value >= 1) return `${value.toFixed(2)} GB`;
  return `${Math.max(0, value * 1024).toFixed(0)} MB`;
}

function formatBytes(value: number) {
  if (!Number.isFinite(value)) return "0 MB";
  return formatGb(value / 1024 / 1024 / 1024);
}

function formatCount(value: number) {
  return new Intl.NumberFormat().format(value || 0);
}

function formatDate(value: string | null | undefined) {
  if (!value) return "unknown";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value.slice(0, 19);
  return date.toLocaleString(undefined, {
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function repoTone(repo: WorkspaceGitRepoStatus) {
  if (repo.warnings.length > 0 || repo.dirtyCount > 0)
    return "border-amber/30 bg-amber/[0.05]";
  return "border-sage/20 bg-sage/[0.04]";
}

function classTone(label: string) {
  const lower = label.toLowerCase();
  if (lower.includes("secret")) return "bg-coral/10 text-coral-dark";
  if (lower.includes("cache")) return "bg-amber/10 text-amber-dark";
  if (lower.includes("data") || lower.includes("model"))
    return "bg-teal/10 text-teal";
  if (lower.includes("code")) return "bg-sage/10 text-sage-dark";
  return "bg-cream-100 text-cream-500";
}

function dangerLabel(file: WorkspaceLargeFile) {
  if (file.classLabel.includes("CACHE")) return "regenerate";
  if (file.classLabel.includes("DATA") || file.classLabel.includes("MODEL"))
    return "external storage";
  return "review";
}

export function WorkspaceView() {
  const [snapshot, setSnapshot] = useState<WorkspaceHygieneSnapshot | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [isScanning, setIsScanning] = useState(false);
  const [isPackaging, setIsPackaging] = useState(false);
  const [isDecrypting, setIsDecrypting] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);
  const [packageSnapshot, setPackageSnapshot] =
    useState<WorkspacePackageSnapshot | null>(null);
  const [packageResult, setPackageResult] =
    useState<WorkspacePackageResult | null>(null);
  const [decryptResult, setDecryptResult] =
    useState<WorkspaceDecryptResult | null>(null);
  // C1: when decrypt is refused because the package is signed by a valid but
  // UNKNOWN (unapproved) device, we capture the refusal so the UI can render a
  // danger panel with the fingerprint and an explicit "Import anyway" opt-in.
  const [unknownSignerRefusal, setUnknownSignerRefusal] = useState<
    string | null
  >(null);
  const [decryptPath, setDecryptPath] = useState("");
  // Collaborator X path: pull the encrypted .aspiswspkg straight from a cloud URL
  // (e.g. a Scaleway/S3 presigned link) instead of downloading it by hand. On
  // success we auto-fill the decrypt path with the saved local file.
  const [downloadUrl, setDownloadUrl] = useState("");
  const [isDownloading, setIsDownloading] = useState(false);
  const [downloadInfo, setDownloadInfo] = useState<WorkspacePackageInfo | null>(
    null,
  );
  const requestId = useRef(0);

  const loadSnapshot = useCallback(async () => {
    const id = requestId.current + 1;
    requestId.current = id;
    setIsLoading(true);
    setError(null);
    try {
      const result = await invokeBackendCommand<WorkspaceHygieneSnapshot>(
        "get_workspace_hygiene_snapshot",
      );
      if (requestId.current === id) {
        setSnapshot(result);
      }
    } catch (e) {
      if (requestId.current === id) {
        setError(
          e instanceof Error
            ? e.message
            : "Workspace snapshot could not be loaded.",
        );
      }
    } finally {
      if (requestId.current === id) {
        setIsLoading(false);
      }
    }
  }, []);

  useEffect(() => {
    void loadSnapshot();
  }, [loadSnapshot]);

  const loadPackageSnapshot = useCallback(async () => {
    try {
      const result = await invokeBackendCommand<WorkspacePackageSnapshot>(
        "get_workspace_package_snapshot",
      );
      setPackageSnapshot(result);
    } catch (e) {
      setError(
        e instanceof Error
          ? e.message
          : "Workspace package state could not be loaded.",
      );
    }
  }, []);

  useEffect(() => {
    void loadPackageSnapshot();
  }, [loadPackageSnapshot]);

  const scanWorkspace = async () => {
    const id = requestId.current + 1;
    requestId.current = id;
    setIsScanning(true);
    setError(null);
    try {
      const result = await invokeBackendCommand<WorkspaceHygieneSnapshot>(
        "scan_workspace_hygiene",
      );
      if (requestId.current === id) {
        setSnapshot(result);
      }
    } catch (e) {
      if (requestId.current === id) {
        setError(e instanceof Error ? e.message : "Workspace scan failed.");
      }
    } finally {
      if (requestId.current === id) {
        setIsScanning(false);
      }
    }
  };

  const copy = async (id: string, value: string | null | undefined) => {
    if (!value) return;
    await navigator.clipboard.writeText(value);
    setCopied(id);
    window.setTimeout(() => setCopied(null), 1200);
  };

  const createPackage = async () => {
    setIsPackaging(true);
    setError(null);
    setPackageResult(null);
    try {
      const result = await invokeBackendCommand<WorkspacePackageResult>(
        "create_workspace_bootstrap_package",
      );
      setPackageResult(result);
      await loadPackageSnapshot();
    } catch (e) {
      setError(
        e instanceof Error ? e.message : "Workspace package creation failed.",
      );
    } finally {
      setIsPackaging(false);
    }
  };

  const decryptPackage = async (allowUnknownSigner = false) => {
    if (!decryptPath.trim()) return;
    setIsDecrypting(true);
    setError(null);
    setDecryptResult(null);
    setUnknownSignerRefusal(null);
    try {
      const result = await invokeBackendCommand<WorkspaceDecryptResult>(
        "decrypt_workspace_bootstrap_package",
        { packagePath: decryptPath.trim(), allowUnknownSigner },
      );
      setDecryptResult(result);
      await loadPackageSnapshot();
    } catch (e) {
      const message =
        e instanceof Error ? e.message : "Workspace package decrypt failed.";
      // C1: the backend refuses an unknown signer fail-closed. Surface it as a
      // distinct danger panel (with the fingerprint + opt-in) rather than a
      // generic error, so the user can verify out-of-band and consciously trust.
      if (/UNKNOWN device/.test(message)) {
        setUnknownSignerRefusal(message);
      } else {
        setError(message);
      }
    } finally {
      setIsDecrypting(false);
    }
  };

  const downloadPackage = async () => {
    const url = downloadUrl.trim();
    if (!url) return;
    setIsDownloading(true);
    setError(null);
    setDownloadInfo(null);
    try {
      const info = await invokeBackendCommand<WorkspacePackageInfo>(
        "download_workspace_bootstrap_package",
        { url },
      );
      setDownloadInfo(info);
      // Hand the saved local path straight to the decrypt step.
      setDecryptPath(info.path);
    } catch (e) {
      setError(
        e instanceof Error ? e.message : "Workspace package download failed.",
      );
    } finally {
      setIsDownloading(false);
    }
  };

  const topLevel = useMemo(
    () =>
      [...(snapshot?.topLevel ?? [])]
        .sort((a, b) => b.sizeGb - a.sizeGb)
        .slice(0, 14),
    [snapshot?.topLevel],
  );

  const largeFiles = useMemo(
    () =>
      [...(snapshot?.largeFiles ?? [])]
        .sort((a, b) => b.sizeGb - a.sizeGb)
        .slice(0, 18),
    [snapshot?.largeFiles],
  );

  const heavyTotal = useMemo(
    () =>
      (snapshot?.largeFiles ?? [])
        .filter((file) => file.classLabel !== "LARGE_FILE")
        .reduce((sum, file) => sum + file.sizeGb, 0),
    [snapshot?.largeFiles],
  );

  const dirtyRepoCount =
    snapshot?.gitRepos.filter((repo) => repo.dirtyCount > 0).length ?? 0;
  const missingPolicyCount =
    snapshot?.policyFiles.filter((policy) => !policy.exists).length ?? 0;

  return (
    <div className="max-w-7xl space-y-5">
      <section className="rounded-lg border border-cream-200 bg-white p-4">
        <div className="flex flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
          <div className="min-w-0">
            <div className="mb-3 flex items-center gap-3">
              <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-teal/10">
                <HardDrive className="h-5 w-5 text-teal" />
              </div>
              <div className="min-w-0">
                <h2 className="text-sm font-semibold text-cream-800">
                  Workspace Hygiene
                </h2>
                <p
                  className="truncate font-mono text-[11px] text-cream-400"
                  title={snapshot?.root}
                >
                  {snapshot?.root ?? "workspace root not loaded"}
                </p>
              </div>
            </div>
            <p className="max-w-3xl text-[12px] leading-5 text-cream-500">
              Code stays in GitHub repos. Data, caches, models, outputs, secrets
              and agent logs stay out of Git and out of Oracle full indexing.
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              onClick={() => void loadSnapshot()}
              disabled={isLoading || isScanning}
              data-help-title="This reloads the workspace report."
              data-help-lines="Reload reads the existing workspace manifest and inventory files.|It does not scan the whole disk again.|Use it after another tool updates _workspace.|It never deletes or moves files."
              className="inline-flex items-center gap-2 rounded-lg border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:text-terracotta disabled:opacity-60"
            >
              <RefreshCw
                className={`h-3.5 w-3.5 ${isLoading ? "animate-spin" : ""}`}
              />
              Reload
            </button>
            <button
              type="button"
              onClick={() => void scanWorkspace()}
              disabled={isLoading || isScanning}
              data-help-title="This rescans the configured workspace."
              data-help-lines="Scan workspace recalculates folder size, large files and Git repo status.|It can take a while because the workspace contains dependency caches and raw data.|It writes CSV reports under _workspace/inventory.|It does not delete, commit, push, or move source files."
              className="inline-flex items-center gap-2 rounded-lg bg-terracotta px-3 py-2 text-[12px] font-semibold text-white shadow-soft-xs disabled:opacity-60"
            >
              <RefreshCw
                className={`h-3.5 w-3.5 ${isScanning ? "animate-spin" : ""}`}
              />
              {isScanning ? "Scanning..." : "Scan workspace"}
            </button>
          </div>
        </div>
      </section>

      <CustomAgentClientsCard />

      {/* Phase 5: the Mini-coder and Design LLM backend cards moved to
          Settings → Providers & Models. The Censor provider card moved there
          too (2026-06-12): users expect every provider picker in one tab. */}

      {error && (
        <div className="rounded-lg border border-coral/20 bg-coral/[0.04] px-4 py-3 text-[12px] font-medium text-coral-dark">
          {error}
        </div>
      )}

      {isLoading && !snapshot ? (
        <section className="rounded-lg border border-cream-200 bg-white p-8 text-center">
          <RefreshCw className="mx-auto mb-3 h-5 w-5 animate-spin text-terracotta" />
          <p className="text-[13px] font-semibold text-cream-800">
            Loading workspace inventory
          </p>
          <p className="mt-1 text-[12px] text-cream-400">
            Reading the saved reports. This is not scanning the whole workspace
            yet.
          </p>
        </section>
      ) : null}

      {snapshot?.warnings.length ? (
        <section className="rounded-lg border border-amber/20 bg-amber/[0.06] p-4">
          <div className="mb-2 flex items-center gap-2">
            <AlertTriangle className="h-4 w-4 text-amber-dark" />
            <h3 className="text-[11px] font-semibold uppercase tracking-widest text-amber-dark">
              Attention
            </h3>
          </div>
          <div className="grid gap-2 md:grid-cols-2">
            {snapshot.warnings.map((warning) => (
              <p
                key={warning}
                className="rounded-md bg-white/70 px-3 py-2 text-[12px] text-cream-700"
              >
                {warning}
              </p>
            ))}
          </div>
        </section>
      ) : null}

      {snapshot ? (
        <>
          <WorkspaceBootstrapPanel
            packageSnapshot={packageSnapshot}
            packageResult={packageResult}
            decryptResult={decryptResult}
            unknownSignerRefusal={unknownSignerRefusal}
            decryptPath={decryptPath}
            downloadUrl={downloadUrl}
            downloadInfo={downloadInfo}
            isDownloading={isDownloading}
            isPackaging={isPackaging}
            isDecrypting={isDecrypting}
            copied={copied}
            onDecryptPathChange={setDecryptPath}
            onDownloadUrlChange={setDownloadUrl}
            onDownloadPackage={() => void downloadPackage()}
            onCreatePackage={() => void createPackage()}
            onDecryptPackage={() => void decryptPackage(false)}
            onImportUnknownSigner={() => void decryptPackage(true)}
            onRefresh={() => void loadPackageSnapshot()}
            onCopy={(id, value) => void copy(id, value)}
          />

          <section className="grid grid-cols-2 gap-3 lg:grid-cols-5">
            <Metric
              label="Total"
              value={formatGb(snapshot?.totalSizeGb ?? 0)}
              sub="visible workspace"
              icon={HardDrive}
            />
            <Metric
              label="Files"
              value={formatCount(snapshot?.totalFiles ?? 0)}
              sub="all counted files"
              icon={FolderOpen}
            />
            <Metric
              label="Oracle"
              value={formatCount(snapshot?.oracleCandidateFiles ?? 0)}
              sub="candidate text files"
              icon={Database}
            />
            <Metric
              label="Dirty Repos"
              value={String(dirtyRepoCount)}
              sub={`${snapshot?.gitRepos.length ?? 0} repo roots`}
              icon={GitBranch}
            />
            <Metric
              label="Heavy"
              value={formatGb(heavyTotal)}
              sub="large non-code files"
              icon={FileWarning}
            />
          </section>

          <div className="grid grid-cols-1 gap-5 xl:grid-cols-[minmax(0,1fr)_420px]">
            <section className="rounded-lg border border-cream-200 bg-white p-4">
              <div className="mb-3 flex items-center justify-between gap-3">
                <div>
                  <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
                    Code Repositories
                  </h3>
                  <p className="mt-1 text-[12px] text-cream-400">
                    These are the repos a collaborator should clone.
                  </p>
                </div>
                <span className="rounded-md bg-cream-50 px-2 py-1 text-[10px] font-semibold text-cream-500">
                  GitHub code only
                </span>
              </div>
              <div className="space-y-3">
                {(snapshot?.gitRepos ?? []).map((repo) => (
                  <article
                    key={repo.path}
                    className={`rounded-lg border p-3 ${repoTone(repo)}`}
                    data-help-title={`${repo.name} is a code repository.`}
                    data-help-lines="Collaborators clone code repositories, not the whole Aspis Bio folder.|Dirty changes mean local edits exist and must be reviewed before push.|The clone command is safe to share, but it does not grant access by itself.|Data folders and caches are separate from these Git roots."
                  >
                    <div className="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
                      <div className="min-w-0">
                        <div className="mb-1 flex flex-wrap items-center gap-2">
                          <p className="truncate text-[13px] font-semibold text-cream-800">
                            {repo.name}
                          </p>
                          <span className="rounded-md bg-white px-2 py-1 font-mono text-[10px] text-cream-500">
                            {repo.branch || "unknown"}
                          </span>
                          {repo.dirtyCount > 0 ? (
                            <span className="rounded-md bg-amber/10 px-2 py-1 text-[10px] font-semibold text-amber-dark">
                              {repo.dirtyCount} dirty
                            </span>
                          ) : (
                            <span className="rounded-md bg-sage/10 px-2 py-1 text-[10px] font-semibold text-sage-dark">
                              clean
                            </span>
                          )}
                        </div>
                        <p
                          className="truncate font-mono text-[10px] text-cream-400"
                          title={repo.relativePath}
                        >
                          {repo.relativePath}
                        </p>
                        <p
                          className="mt-1 truncate font-mono text-[10px] text-cream-400"
                          title={repo.origin ?? ""}
                        >
                          {repo.origin ?? "no origin"}
                        </p>
                        {repo.warnings.length > 0 && (
                          <div className="mt-2 flex flex-wrap gap-1">
                            {repo.warnings.map((warning) => (
                              <span
                                key={warning}
                                className="rounded-md bg-white px-2 py-1 text-[10px] text-amber-dark"
                              >
                                {warning}
                              </span>
                            ))}
                          </div>
                        )}
                      </div>
                      <button
                        type="button"
                        onClick={() => void copy(repo.path, repo.cloneCommand)}
                        disabled={!repo.cloneCommand}
                        data-help-title="This copies a safe clone command."
                        data-help-lines="The command downloads only this code repository, with credentials stripped from the URL.|The collaborator still needs GitHub permission from the owner.|After work, they should push a branch and open a pull request.|It does not download data, models, or Oracle cache."
                        className="inline-flex shrink-0 items-center justify-center gap-2 rounded-md border border-cream-200 bg-white px-3 py-2 text-[11px] font-semibold text-cream-600 hover:text-terracotta disabled:opacity-60"
                      >
                        <Copy className="h-3.5 w-3.5" />
                        {copied === repo.path ? "Copied" : "Copy clone"}
                      </button>
                    </div>
                  </article>
                ))}
              </div>
            </section>

            <aside className="space-y-5">
              <section className="rounded-lg border border-cream-200 bg-white p-4">
                <div className="mb-3 flex items-center gap-2">
                  <ShieldCheck className="h-4 w-4 text-sage-dark" />
                  <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
                    Policies
                  </h3>
                </div>
                <div className="space-y-2">
                  {(snapshot?.policyFiles ?? []).map((policy) => (
                    <div
                      key={policy.path}
                      title={policy.path}
                      className="flex items-center justify-between gap-3 rounded-lg bg-cream-50 px-3 py-2"
                      data-help-title={`${policy.name} controls workspace hygiene.`}
                      data-help-lines="Policy files tell the app, Oracle and future agents what should be ignored or indexed.|For Aspis Bio, these prevent secrets, model binaries, raw datasets and caches from being treated as source code.|Missing policy files make collaborator setup and Oracle indexing less reliable.|Active rules are non-comment lines."
                    >
                      <div className="min-w-0">
                        <p className="truncate text-[12px] font-semibold text-cream-800">
                          {policy.name}
                        </p>
                        <p className="text-[10px] text-cream-400">
                          {policy.activeRules} rules / {policy.lineCount} lines
                        </p>
                      </div>
                      <div className="flex shrink-0 items-center gap-2">
                        <span
                          className={`rounded-md px-2 py-1 text-[10px] font-semibold ${
                            policy.exists
                              ? "bg-sage/10 text-sage-dark"
                              : "bg-coral/10 text-coral-dark"
                          }`}
                        >
                          {policy.exists ? "present" : "missing"}
                        </span>
                        {policy.exists ? (
                          <CheckCircle2
                            className="h-4 w-4 text-sage-dark"
                            aria-label="Policy present"
                          />
                        ) : (
                          <AlertTriangle
                            className="h-4 w-4 text-coral-dark"
                            aria-label="Policy missing"
                          />
                        )}
                      </div>
                    </div>
                  ))}
                  {missingPolicyCount > 0 && (
                    <p className="rounded-md bg-coral/[0.04] px-3 py-2 text-[11px] text-coral-dark">
                      Missing policy files should be restored before onboarding
                      collaborators.
                    </p>
                  )}
                </div>
              </section>

              <section className="rounded-lg border border-cream-200 bg-white p-4">
                <div className="mb-3 flex items-center gap-2">
                  <Database className="h-4 w-4 text-teal" />
                  <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
                    Classification
                  </h3>
                </div>
                <div className="space-y-2">
                  {(snapshot?.classifications ?? [])
                    .slice(0, 12)
                    .map((item) => (
                      <ClassificationRow key={item.path} item={item} />
                    ))}
                </div>
              </section>
            </aside>
          </div>

          <section className="grid grid-cols-1 gap-5 xl:grid-cols-2">
            <InventoryPanel items={topLevel} />
            <LargeFilePanel items={largeFiles} />
          </section>

          <p className="text-[10px] text-cream-400">
            Last loaded {formatDate(snapshot?.scannedAt)}. Reports live under{" "}
            <span className="font-mono">
              {snapshot?.workspaceDir ?? "_workspace"}
            </span>
            .
          </p>
        </>
      ) : !isLoading ? (
        <section className="rounded-lg border border-cream-200 bg-white p-8 text-center">
          <AlertTriangle className="mx-auto mb-3 h-5 w-5 text-amber-dark" />
          <p className="text-[13px] font-semibold text-cream-800">
            No workspace inventory loaded
          </p>
          <p className="mt-1 text-[12px] text-cream-400">
            Reload the saved reports or run a scan after checking the configured
            workspace root.
          </p>
        </section>
      ) : null}
    </div>
  );
}

// Settings → Workspace card to manage user-defined extra agent CLIs. Lists the
// configured clients (label, command, remove) and an add form (label → auto id,
// or an explicit id; command line). Validation is the SHARED pure helper
// (validateCustomClient) so the UI and the Rust boundary never disagree. Persists
// through set_custom_agent_clients, then refreshes the global config so the Spawn
// panel's CLI selector immediately reflects the change.
function CustomAgentClientsCard() {
  const { config } = useAppContext();
  const { refreshConfig } = useAppActions();
  const clients = useMemo<CustomAgentClient[]>(
    () => config.customAgentClients ?? [],
    [config.customAgentClients],
  );

  const [labelDraft, setLabelDraft] = useState("");
  const [idDraft, setIdDraft] = useState("");
  // True once the user types an explicit id; until then the id auto-derives from
  // the label so most operators never touch the id field.
  const [idTouched, setIdTouched] = useState(false);
  const [commandDraft, setCommandDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const effectiveId = idTouched ? idDraft : slugifyClientId(labelDraft);
  const validation = useMemo(
    () =>
      validateCustomClient(
        { id: effectiveId, label: labelDraft, command: commandDraft },
        clients,
      ),
    [effectiveId, labelDraft, commandDraft, clients],
  );
  // Only surface inline field errors once the user has typed something in that
  // field, so the form is not red on first paint.
  const showIdError =
    (idTouched ? idDraft.length > 0 : labelDraft.length > 0) &&
    Boolean(validation.errors.id);
  const showCommandError =
    commandDraft.length > 0 && Boolean(validation.errors.command);

  const persist = useCallback(
    async (next: CustomAgentClient[]) => {
      setBusy(true);
      setError(null);
      try {
        await invokeBackendCommand<CustomAgentClient[]>(
          "set_custom_agent_clients",
          { clients: next },
        );
        await refreshConfig();
      } catch (e) {
        if (mountedRef.current) {
          setError(
            e instanceof Error
              ? e.message
              : "Could not save custom agent CLIs.",
          );
        }
        throw e;
      } finally {
        if (mountedRef.current) setBusy(false);
      }
    },
    [refreshConfig],
  );

  const addClient = async () => {
    if (!validation.ok || !validation.value) return;
    try {
      await persist([...clients, validation.value]);
      if (!mountedRef.current) return;
      setLabelDraft("");
      setIdDraft("");
      setIdTouched(false);
      setCommandDraft("");
    } catch {
      // Error already surfaced by persist; keep the draft so the user can retry.
    }
  };

  const removeClient = async (id: string) => {
    try {
      await persist(clients.filter((client) => client.id !== id));
    } catch {
      // Error surfaced by persist.
    }
  };

  return (
    <section
      className="rounded-lg border border-cream-200 bg-white p-4"
      data-help-title="Custom agent CLIs let you launch your own command-line agents."
      data-help-lines="Define an extra CLI (e.g. a DeepSeek CLI) and it appears in the Spawn panel beside Codex and Claude.|The command line runs verbatim in the agent terminal; the prompt is copied to the clipboard and exposed via the ASPIS_AGENT_PROMPT_FILE environment variable.|The launch token is never put on the command line.|These are stored in your local config.json."
    >
      <div className="mb-3 flex items-center gap-2">
        <Terminal className="h-4 w-4 text-terracotta" />
        <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          Custom agent CLIs
        </h3>
      </div>
      <p className="mb-3 max-w-3xl text-[12px] leading-5 text-cream-500">
        Add your own agent command-line tools. Each one appears in the Spawn panel
        next to Codex and Claude and launches the same way. The command runs as
        written; the prompt is on the clipboard and at{" "}
        <span className="font-mono">$ASPIS_AGENT_PROMPT_FILE</span>.
      </p>

      {clients.length > 0 ? (
        <div className="mb-4 space-y-2">
          {clients.map((client) => (
            <div
              key={client.id}
              className="flex items-start justify-between gap-3 rounded-lg bg-cream-50 px-3 py-2"
            >
              <div className="min-w-0">
                <p className="truncate text-[12px] font-semibold text-cream-800">
                  {client.label}{" "}
                  <span className="font-mono text-[10px] font-normal text-cream-400">
                    ({client.id})
                  </span>
                </p>
                <p className="truncate font-mono text-[11px] text-cream-500">
                  {client.command}
                </p>
              </div>
              <button
                type="button"
                onClick={() => void removeClient(client.id)}
                disabled={busy}
                className="inline-flex shrink-0 items-center gap-1 rounded-md border border-cream-200 bg-white px-2 py-1 text-[10px] font-semibold text-cream-500 hover:border-coral/30 hover:text-coral-dark disabled:opacity-60"
              >
                <Trash2 className="h-3 w-3" />
                Remove
              </button>
            </div>
          ))}
        </div>
      ) : (
        <p className="mb-4 rounded-lg bg-cream-50 px-3 py-2 text-[11px] text-cream-400">
          No custom CLIs yet. Add one below.
        </p>
      )}

      <div className="grid gap-3 rounded-lg border border-cream-200 p-3 md:grid-cols-2">
        <label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
          Label
          <input
            value={labelDraft}
            onChange={(event) => setLabelDraft(event.target.value)}
            placeholder="DeepSeek"
            maxLength={CLIENT_LABEL_MAX_LENGTH}
            className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-terracotta/30"
          />
        </label>
        <label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
          Id
          <input
            value={effectiveId}
            onChange={(event) => {
              setIdTouched(true);
              setIdDraft(event.target.value);
            }}
            placeholder="deepseek"
            maxLength={32}
            className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-terracotta/30"
          />
          {showIdError && (
            <span className="mt-1 block text-[10px] normal-case tracking-normal text-coral-dark">
              {validation.errors.id}
            </span>
          )}
        </label>
        <label className="md:col-span-2 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
          Command line
          <input
            value={commandDraft}
            onChange={(event) => setCommandDraft(event.target.value)}
            placeholder="deepseek chat --some-flag"
            maxLength={CLIENT_COMMAND_MAX_LENGTH}
            className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-terracotta/30"
          />
          {showCommandError && (
            <span className="mt-1 block text-[10px] normal-case tracking-normal text-coral-dark">
              {validation.errors.command}
            </span>
          )}
        </label>
        <div className="md:col-span-2">
          <button
            type="button"
            onClick={() => void addClient()}
            disabled={busy || !validation.ok}
            className="inline-flex items-center gap-2 rounded-md bg-terracotta px-3 py-2 text-[12px] font-semibold text-white hover:bg-terracotta/90 disabled:opacity-60"
          >
            <Plus className="h-3.5 w-3.5" />
            Add CLI
          </button>
        </div>
      </div>

      {error && (
        <p className="mt-3 flex items-start gap-2 rounded-lg border border-coral/30 bg-coral/[0.05] px-3 py-2 text-[11px] leading-4 text-coral-dark">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>{error}</span>
        </p>
      )}
    </section>
  );
}

// Phase 5: MiniCoderBackendCard and DesignLlmBackendCard were extracted to
// src/components/settings/. They are re-exported here ONLY so the existing
// isolation tests (which import the __test_* aliases from this module) keep
// passing; the cards now render inside Settings -> Providers & Models.
export {
  __test_MiniCoderBackendCard,
} from "../settings/MiniCoderBackendCard";
export {
  __test_DesignLlmBackendCard,
} from "../settings/DesignLlmBackendCard";

// Settings → Workspace card to pick Censor's tier-2 (Gemma) local-AI PROVIDER:
// Ollama (the default — today's behavior, zero config) or a local oMLX (MLX) server.
// Mirrors MiniCoderBackendCard: a provider <select>, oMLX-only Base URL + Model inputs,
// shared pure validation (validateCensorLocalAi) gating Save, and persistence through
// set_censor_local_ai (the bare Ollama default is persisted minimally / drops the key —
// no churn). The form seeds from config.censorLocalAi and renders the oMLX fields ONLY
// when provider==omlx, so the persisted payload carries only the active provider's
// fields (no stale-field bleed on a provider switch).
//
// PRIVACY: Censor sends FILE CONTENT to this endpoint. The oMLX base is validated
// client-side to a LOOPBACK http origin (defense in depth — the backend also clamps),
// and there is NO API-key field (loopback-only, like Ollama).
// Clamp a raw (hand-edited / untyped) `censorLocalAi.provider` to a KNOWN provider so a
// bogus value (`"bogus"`, a number, undefined) never seeds an indeterminate <select>;
// defaults to "ollama" (today's behavior). The Rust `read_censor_local_ai` already
// fail-safes a bogus provider on the backend — this is the frontend half (the card seeds
// from the UNTYPED `config.censorLocalAi` passthrough, so it must not trust its shape).
function seedCensorProvider(raw: unknown): CensorAiProvider {
  return CENSOR_AI_PROVIDERS.includes(raw as CensorAiProvider)
    ? (raw as CensorAiProvider)
    : "ollama";
}

// Coerce a raw (untyped) baseUrl/model to a string. A non-string value (e.g. a number
// from a hand-edited config) would otherwise crash the controlled input / `.trim()` in
// the validator; we coerce anything non-string to "".
function seedCensorString(raw: unknown): string {
  return typeof raw === "string" ? raw : "";
}

function inferIsAppleHostMac(): boolean | null {
  if (typeof navigator === "undefined") return null;
  const platform = (navigator.platform ?? "").toLowerCase();
  const userAgent = (navigator.userAgent ?? "").toLowerCase();
  const haystack = `${platform} ${userAgent}`;
  if (haystack.includes("mac") || haystack.includes("darwin")) return true;
  if (
    haystack.includes("win") ||
    haystack.includes("linux") ||
    haystack.includes("android") ||
    haystack.includes("iphone") ||
    haystack.includes("ipad")
  )
    return false;
  return null;
}

export function CensorLocalAiCard() {
  const { config } = useAppContext();
  const { refreshConfig } = useAppActions();
  // Absent censorLocalAi == the Ollama default (today's behavior). This is the UNTYPED
  // raw passthrough (serde_json::Value via get_config), so every field below is coerced/
  // clamped before it seeds React state — a hand-edited config must not break the form.
  const current = (config.censorLocalAi ?? null) as Partial<CensorLocalAi> | null;

  const [provider, setProvider] = useState<CensorAiProvider>(
    seedCensorProvider(current?.provider),
  );
  const [baseUrl, setBaseUrl] = useState(seedCensorString(current?.baseUrl));
  const [model, setModel] = useState(seedCensorString(current?.model));
  const [ollamaModel, setOllamaModel] = useState(
    seedCensorString((current as { ollamaModel?: unknown } | null)?.ollamaModel),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedTick, setSavedTick] = useState(false);
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Reflect a config change made elsewhere (or after a save) into the draft. Coerce/clamp
  // the untyped raw values the SAME way as the initial seed so a hand-edited config can
  // never push a bogus provider or non-string base/model into the form.
  useEffect(() => {
    setProvider(seedCensorProvider(current?.provider));
    setBaseUrl(seedCensorString(current?.baseUrl));
    setModel(seedCensorString(current?.model));
    setOllamaModel(seedCensorString((current as { ollamaModel?: unknown } | null)?.ollamaModel));
  }, [current?.provider, current?.baseUrl, current?.model, (current as { ollamaModel?: unknown } | null)?.ollamaModel]);

  // This card now OWNS the Ollama model-tag override (the separate CensorModelCard was
  // merged into it 2026-06-12): the draft state above feeds validation/save directly.
  const validation = useMemo(
    () => validateCensorLocalAi({ provider, baseUrl, model, ollamaModel }),
    [provider, baseUrl, model, ollamaModel],
  );
  const isAppleHostMac = useMemo(() => inferIsAppleHostMac(), []);
  const appleFmDisabled = provider === "appleFm" && isAppleHostMac === false;
  const appleFmAvailabilityNote = useMemo(() => {
    if (provider !== "appleFm") return null;
    if (isAppleHostMac === true) return null;
    if (isAppleHostMac === false) {
      return "Apple on-device is not available on this OS. Configure it on macOS 27+.";
    }
    return "Apple on-device requires macOS 27+; saving is still allowed for cross-machine use.";
  }, [provider, isAppleHostMac]);
  // The oMLX base/model are REQUIRED, so surface their errors even when empty (mirroring
  // the mini-coder card) — an empty/invalid field just greys out Save otherwise, with no
  // inline reason for WHY.
  const showBaseUrlError = provider === "omlx" && Boolean(validation.errors.baseUrl);
  const showModelError =
    (provider === "omlx" || provider === "appleFm") &&
    Boolean(validation.errors.model);

  const save = useCallback(
    async (next: CensorLocalAi) => {
      setBusy(true);
      setError(null);
      try {
        await invokeBackendCommand<CensorLocalAi>("set_censor_local_ai", {
          config: next,
        });
        await refreshConfig();
        if (mountedRef.current) {
          setSavedTick(true);
          window.setTimeout(() => {
            if (mountedRef.current) setSavedTick(false);
          }, 2000);
        }
      } catch (e) {
        if (mountedRef.current) {
          setError(
            e instanceof Error
              ? e.message
              : "Could not save the Censor local-AI provider.",
          );
        }
        throw e;
      } finally {
        if (mountedRef.current) setBusy(false);
      }
    },
    [refreshConfig],
  );

  const onSave = async () => {
    if (!validation.ok || !validation.value) return;
    try {
      await save(validation.value);
    } catch {
      // Error surfaced by save; keep the draft.
    }
  };

  return (
    <section
      className="rounded-2xl border border-cream-200 bg-white p-4"
      data-help-title="Censor's tier-2 model can run on Ollama, local oMLX, or Apple on-device."
      data-help-lines="Ollama is the default (today's behavior, no setup). oMLX points Censor at a local MLX server exposing an OpenAI-compatible HTTP API; Apple on-device uses the local Apple runtime. Censor sends file content to tier-2 models, so remote endpoints are loopback-only.|Stored in your local config.json; absent means the Ollama default."
    >
      <div className="mb-3 flex items-center gap-2">
        <Cpu className="h-4 w-4 text-teal" />
        <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          Censor local AI
        </h3>
      </div>
      <p className="mb-4 max-w-3xl text-[12px] leading-5 text-cream-500">
        Choose the on-device model provider Censor uses for its optional tier-2
        (Gemma) review. Ollama is the default; oMLX points Censor at a local MLX
        server; Apple on-device uses macOS Foundation Models.
      </p>

      <div className="grid gap-3 rounded-2xl border border-cream-200 p-3 md:grid-cols-2">
        <label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
          Provider
          <select
            value={provider}
            onChange={(event) =>
              setProvider(event.target.value as CensorAiProvider)
            }
            className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
          >
            <option value="ollama">Ollama (default)</option>
            <option value="omlx">oMLX (local MLX server)</option>
            <option value="appleFm" disabled={isAppleHostMac === false}>
              Apple on-device
            </option>
          </select>
        </label>

        {provider === "omlx" || provider === "appleFm" ? (
          <label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
            Model {provider === "appleFm" ? "(optional)" : "tag"}
            <input
              value={model}
              onChange={(event) => setModel(event.target.value)}
              placeholder={provider === "appleFm" ? "default" : "mlx-community/gemma"}
              maxLength={CENSOR_MODEL_MAX_LENGTH}
              className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
            />
            {showModelError && (
              <span className="mt-1 block text-[10px] normal-case tracking-normal text-coral-dark">
                {validation.errors.model}
              </span>
            )}
          </label>
        ) : (
          <label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
            Ollama model tag (optional)
            <input
              value={ollamaModel}
              onChange={(event) => setOllamaModel(event.target.value)}
              placeholder="gemma4:e4b"
              maxLength={CENSOR_MODEL_MAX_LENGTH}
              className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
            />
            <span className="mt-1 block text-[10px] normal-case tracking-normal text-cream-400">
              Blank uses the default gemma4:e4b (auto-falls back to e2b).
            </span>
          </label>
        )}

        {provider === "appleFm" ? (
          <p className="md:col-span-2 text-[11px] leading-4 text-cream-400">
            Apple on-device does not use a network base URL; your model name is
            optional.
          </p>
        ) : null}

        {provider === "omlx" ? (
          <label className="md:col-span-2 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
            Base URL
            <input
              value={baseUrl}
              onChange={(event) => setBaseUrl(event.target.value)}
              placeholder="http://localhost:8000/v1"
              maxLength={CENSOR_BASE_URL_MAX_LENGTH}
              className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
            />
            {showBaseUrlError && (
              <span className="mt-1 block text-[10px] normal-case tracking-normal text-coral-dark">
                {validation.errors.baseUrl}
              </span>
            )}
          </label>
        ) : null}

        {provider === "omlx" ? (
          <p className="md:col-span-2 text-[11px] leading-4 text-cream-400">
            Local OpenAI-compatible endpoint; loopback only (localhost, 127.0.0.1
            or [::1]) over http. Censor sends file content to this model, so a
            non-loopback host is refused to keep your code on this machine. No API
            key — loopback only.
          </p>
        ) : null}

        {appleFmAvailabilityNote ? (
          <p className="md:col-span-2 text-[11px] leading-4 text-amber-dark">
            {appleFmAvailabilityNote}
          </p>
        ) : null}

        <div className="md:col-span-2 flex items-center gap-2">
          <button
            type="button"
            onClick={() => void onSave()}
            disabled={busy || !validation.ok || appleFmDisabled}
            className="inline-flex items-center gap-2 rounded-md bg-teal px-3 py-2 text-[12px] font-semibold text-white hover:bg-teal/90 disabled:opacity-60"
          >
            <CheckCircle2 className="h-3.5 w-3.5" />
            {savedTick ? "Saved" : "Save provider"}
          </button>
        </div>
      </div>

      {error && (
        <p className="mt-3 flex items-start gap-2 rounded-2xl border border-coral/30 bg-coral/[0.05] px-3 py-2 text-[11px] leading-4 text-coral-dark">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>{error}</span>
        </p>
      )}
    </section>
  );
}

// Test-only export so the card can be rendered in isolation (vitest), like the
// mini-coder card above.
export const __test_CensorLocalAiCard = CensorLocalAiCard;

function WorkspaceBootstrapPanel({
  packageSnapshot,
  packageResult,
  decryptResult,
  unknownSignerRefusal,
  decryptPath,
  downloadUrl,
  downloadInfo,
  isDownloading,
  isPackaging,
  isDecrypting,
  copied,
  onDecryptPathChange,
  onDownloadUrlChange,
  onDownloadPackage,
  onCreatePackage,
  onDecryptPackage,
  onImportUnknownSigner,
  onRefresh,
  onCopy,
}: {
  packageSnapshot: WorkspacePackageSnapshot | null;
  packageResult: WorkspacePackageResult | null;
  decryptResult: WorkspaceDecryptResult | null;
  unknownSignerRefusal: string | null;
  decryptPath: string;
  downloadUrl: string;
  downloadInfo: WorkspacePackageInfo | null;
  isDownloading: boolean;
  isPackaging: boolean;
  isDecrypting: boolean;
  copied: string | null;
  onDecryptPathChange: (value: string) => void;
  onDownloadUrlChange: (value: string) => void;
  onDownloadPackage: () => void;
  onCreatePackage: () => void;
  onDecryptPackage: () => void;
  onImportUnknownSigner: () => void;
  onRefresh: () => void;
  onCopy: (id: string, value: string | null | undefined) => void;
}) {
  const latest = packageSnapshot?.latestPackages ?? [];
  const recipients = packageSnapshot?.approvedRecipients ?? [];
  return (
    <section
      className="rounded-lg border border-cream-200 bg-white p-4"
      data-help-title="Workspace Bootstrap creates the encrypted first-download package."
      data-help-lines="This is for first collaborator setup, not daily Git work.|The app selects source, docs, tests and small config files using .aspisignore.|Bulk data is AES-256-GCM encrypted, and the package key is wrapped only for approved device fingerprints.|Upload the resulting .aspiswspkg to kDrive, Drive, Dropbox or another cloud; the cloud should only see encrypted bytes."
    >
      <div className="mb-4 flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
        <div className="min-w-0">
          <div className="mb-2 flex items-center gap-2">
            <PackageCheck className="h-4 w-4 text-teal" />
            <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
              Workspace Bootstrap
            </h3>
          </div>
          <p className="max-w-3xl text-[12px] leading-5 text-cream-500">
            Create one encrypted package for approved devices, then upload the
            file to any cloud. A collaborator can decrypt it only from an
            approved Aspis Management install.
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            onClick={onRefresh}
            disabled={isPackaging || isDecrypting}
            className="inline-flex items-center gap-2 rounded-lg border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:text-terracotta disabled:opacity-60"
          >
            <RefreshCw className="h-3.5 w-3.5" />
            Refresh
          </button>
          <button
            type="button"
            onClick={onCreatePackage}
            disabled={isPackaging || recipients.length === 0}
            data-help-title="This creates the encrypted bootstrap package."
            data-help-lines="The package is written under _workspace/packages.|It includes only files allowed by workspace policy and skips large files over the safety limit.|Each approved device gets an encrypted copy of the package key.|You can upload the .aspiswspkg file to kDrive, Google Drive, Dropbox or similar storage."
            className="inline-flex items-center gap-2 rounded-lg bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-60"
          >
            <ShieldCheck className="h-3.5 w-3.5" />
            {isPackaging ? "Creating..." : "Create encrypted package"}
          </button>
        </div>
      </div>

      <div className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
        <div className="rounded-lg bg-cream-50 p-3">
          <div className="mb-2 flex items-center justify-between gap-3">
            <p className="text-[12px] font-semibold text-cream-800">
              Approved recipients
            </p>
            <span className="rounded-md bg-white px-2 py-1 text-[10px] font-semibold text-cream-500">
              {recipients.length} devices
            </span>
          </div>
          <div className="space-y-2">
            {recipients.slice(0, 5).map((recipient) => (
              <div
                key={recipient.fingerprint}
                className="grid grid-cols-[minmax(0,1fr)_138px] items-center gap-2 rounded-md bg-white px-2 py-2"
              >
                <div className="min-w-0">
                  <p className="truncate text-[12px] font-semibold text-cream-700">
                    {recipient.collaboratorName}
                  </p>
                  <p className="truncate text-[10px] text-cream-400">
                    {recipient.deviceName} / {recipient.platform}
                  </p>
                </div>
                <p className="truncate text-right font-mono text-[10px] text-cream-500">
                  {recipient.fingerprint}
                </p>
              </div>
            ))}
            {recipients.length === 0 ? (
              <p className="rounded-md bg-white px-3 py-2 text-[12px] text-amber-dark">
                Approve at least one device in Devices & Invites first.
              </p>
            ) : null}
          </div>
        </div>

        <div className="rounded-lg bg-cream-50 p-3">
          <p className="mb-2 text-[12px] font-semibold text-cream-800">
            Download from cloud
          </p>
          <div className="flex flex-col gap-2 sm:flex-row">
            <input
              value={downloadUrl}
              onChange={(event) => onDownloadUrlChange(event.target.value)}
              placeholder="https://… link to the encrypted .aspiswspkg"
              data-help-title="Fetch the encrypted package straight from a cloud URL."
              data-help-lines="For a collaborator who does not have the Aspis Bio folder yet.|Paste an https link (e.g. a Scaleway/S3 presigned URL) to the .aspiswspkg.|The app downloads the encrypted bytes only — it never trusts them; the normal signature-verified decrypt still runs.|On success the local path is filled in below, ready to decrypt."
              className="min-w-0 flex-1 rounded-lg border border-cream-200 bg-white px-3 py-2 font-mono text-[11px] text-cream-700 outline-none focus:border-terracotta-200"
            />
            <button
              type="button"
              onClick={onDownloadPackage}
              disabled={isDownloading || !downloadUrl.trim()}
              className="inline-flex items-center justify-center gap-2 rounded-lg border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:text-terracotta disabled:opacity-60"
            >
              <Download className="h-3.5 w-3.5" />
              {isDownloading ? "Downloading..." : "Download"}
            </button>
          </div>
          {downloadInfo ? (
            <p className="mt-2 text-[11px] text-cream-400">
              Downloaded {downloadInfo.fileName} (
              {downloadInfo.sizeMb.toFixed(1)} MB) — path filled in below.
            </p>
          ) : null}
        </div>

        <div className="rounded-lg bg-cream-50 p-3">
          <p className="mb-2 text-[12px] font-semibold text-cream-800">
            Decrypt downloaded package
          </p>
          <div className="flex flex-col gap-2 sm:flex-row">
            <input
              value={decryptPath}
              onChange={(event) => onDecryptPathChange(event.target.value)}
              placeholder="Paste path to downloaded .aspiswspkg"
              data-help-title="This is the local path to the downloaded encrypted package."
              data-help-lines="The cloud provider only stores the encrypted .aspiswspkg file.|Download it from the cloud above (or paste a local path).|Decrypt succeeds only if this app's device fingerprint is in the package header.|The restored files go under _workspace/imports."
              className="min-w-0 flex-1 rounded-lg border border-cream-200 bg-white px-3 py-2 font-mono text-[11px] text-cream-700 outline-none focus:border-terracotta-200"
            />
            <button
              type="button"
              onClick={onDecryptPackage}
              disabled={isDecrypting || !decryptPath.trim()}
              className="inline-flex items-center justify-center gap-2 rounded-lg bg-teal px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-60"
            >
              <ShieldCheck className="h-3.5 w-3.5" />
              {isDecrypting ? "Decrypting..." : "Decrypt"}
            </button>
          </div>
          <p className="mt-2 text-[11px] text-cream-400">
            Import folder:{" "}
            <span className="font-mono">
              {packageSnapshot?.importDir ?? "_workspace/imports"}
            </span>
          </p>
        </div>
      </div>

      {packageResult ? (
        <ResultBox
          tone="success"
          title={`Package created: ${packageResult.fileName}`}
          lines={[
            `${packageResult.fileCount} files / ${formatBytes(packageResult.totalBytes)} selected`,
            `${formatBytes(packageResult.packageBytes)} encrypted for ${packageResult.recipientCount} devices`,
            packageResult.path,
          ]}
          actionLabel={copied === "package-result" ? "Copied" : "Copy path"}
          onAction={() => onCopy("package-result", packageResult.path)}
        />
      ) : null}

      {unknownSignerRefusal ? (
        <UnknownSignerRefusalBox
          message={unknownSignerRefusal}
          isDecrypting={isDecrypting}
          onImportUnknownSigner={onImportUnknownSigner}
        />
      ) : null}

      {decryptResult ? (
        <DecryptResultBox
          result={decryptResult}
          copied={copied}
          onCopy={onCopy}
        />
      ) : null}

      {latest.length > 0 ? (
        <div className="mt-4">
          <p className="mb-2 text-[11px] font-semibold uppercase tracking-widest text-cream-500">
            Latest Packages
          </p>
          <div className="grid gap-2 lg:grid-cols-2">
            {latest.map((pkg) => (
              <PackageRow
                key={pkg.path}
                pkg={pkg}
                copied={copied}
                onCopy={onCopy}
                onUsePath={onDecryptPathChange}
              />
            ))}
          </div>
        </div>
      ) : null}

      {packageSnapshot?.warnings.length ? (
        <div className="mt-3 rounded-lg border border-amber/20 bg-amber/[0.06] px-3 py-2">
          {packageSnapshot.warnings.map((warning) => (
            <p key={warning} className="text-[11px] text-amber-dark">
              {warning}
            </p>
          ))}
        </div>
      ) : null}
    </section>
  );
}

function PackageRow({
  pkg,
  copied,
  onCopy,
  onUsePath,
}: {
  pkg: WorkspacePackageInfo;
  copied: string | null;
  onCopy: (id: string, value: string | null | undefined) => void;
  onUsePath: (value: string) => void;
}) {
  return (
    <div className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-2 rounded-lg bg-cream-50 px-3 py-2">
      <div className="min-w-0">
        <p className="truncate text-[12px] font-semibold text-cream-800">
          {pkg.fileName}
        </p>
        <p className="truncate font-mono text-[10px] text-cream-400">
          {pkg.sizeMb.toFixed(2)} MB / {formatDate(pkg.createdAt)}
        </p>
      </div>
      <div className="flex gap-1">
        <button
          type="button"
          onClick={() => onUsePath(pkg.path)}
          className="rounded-md border border-cream-200 bg-white px-2 py-1 text-[10px] font-semibold text-cream-500 hover:text-terracotta"
        >
          Use
        </button>
        <button
          type="button"
          onClick={() => onCopy(`pkg:${pkg.path}`, pkg.path)}
          className="inline-flex items-center gap-1 rounded-md border border-cream-200 bg-white px-2 py-1 text-[10px] font-semibold text-cream-500 hover:text-terracotta"
        >
          <Copy className="h-3 w-3" />
          {copied === `pkg:${pkg.path}` ? "Copied" : "Path"}
        </button>
      </div>
    </div>
  );
}

function ResultBox({
  tone,
  title,
  lines,
  actionLabel,
  onAction,
}: {
  tone: "success";
  title: string;
  lines: string[];
  actionLabel: string;
  onAction: () => void;
}) {
  const toneClass = tone === "success" ? "border-sage/20 bg-sage/[0.04]" : "";
  return (
    <div className={`mt-4 rounded-lg border px-3 py-2 ${toneClass}`}>
      <div className="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
        <div className="min-w-0">
          <p className="text-[12px] font-semibold text-cream-800">{title}</p>
          {lines.map((line) => (
            <p key={line} className="truncate text-[11px] text-cream-500">
              {line}
            </p>
          ))}
        </div>
        <button
          type="button"
          onClick={onAction}
          className="inline-flex shrink-0 items-center justify-center gap-2 rounded-md border border-cream-200 bg-white px-2 py-1 text-[10px] font-semibold text-cream-500 hover:text-terracotta"
        >
          <Copy className="h-3 w-3" />
          {actionLabel}
        </button>
      </div>
    </div>
  );
}

// C1: renders the decrypt result with signer provenance. A known signer is a
// success state; a valid-but-unknown signer (only reachable after an explicit
// opt-in) is a DANGER state, never a green "success", so an opt-in import is not
// visually indistinguishable from a trusted one.
function DecryptResultBox({
  result,
  copied,
  onCopy,
}: {
  result: WorkspaceDecryptResult;
  copied: string | null;
  onCopy: (id: string, value: string | null | undefined) => void;
}) {
  const restored = `${result.filesRestored} files restored / ${formatBytes(result.bytesRestored)}`;
  const copyLabel = copied === "decrypt-result" ? "Copied" : "Copy folder";
  // Always shown so the user can compare the signer fingerprint out-of-band.
  const fingerprint = result.signerFingerprint || "unavailable";

  if (result.signerKnown) {
    return (
      <div className="mt-4 rounded-lg border border-sage/20 bg-sage/[0.04] px-3 py-2">
        <div className="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <CheckCircle2 className="h-4 w-4 shrink-0 text-sage" />
              <p className="text-[12px] font-semibold text-cream-800">
                Package decrypted: {result.packageId}
              </p>
            </div>
            <p className="mt-1 text-[11px] font-semibold text-sage">
              Signed by {result.signerName ?? "an approved device"} (verified
              device)
            </p>
            <p className="truncate text-[11px] text-cream-500">{restored}</p>
            <p className="truncate text-[11px] text-cream-500">
              Signer fingerprint: {fingerprint}
            </p>
            <p className="truncate text-[11px] text-cream-500">
              Device fingerprint: {result.recipientFingerprint}
            </p>
            <p className="truncate text-[11px] text-cream-500">
              {result.outputDir}
            </p>
          </div>
          <button
            type="button"
            onClick={() => onCopy("decrypt-result", result.outputDir)}
            className="inline-flex shrink-0 items-center justify-center gap-2 rounded-md border border-cream-200 bg-white px-2 py-1 text-[10px] font-semibold text-cream-500 hover:text-terracotta"
          >
            <Copy className="h-3 w-3" />
            {copyLabel}
          </button>
        </div>
      </div>
    );
  }

  // signatureValid && !signerKnown -> imported via opt-in, but UNTRUSTED signer.
  return (
    <div className="mt-4 rounded-lg border border-coral/40 bg-coral/[0.06] px-3 py-2">
      <div className="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <AlertTriangle className="h-4 w-4 shrink-0 text-coral-dark" />
            <p className="text-[12px] font-semibold text-coral-dark">
              Imported from an UNKNOWN signer: {result.packageId}
            </p>
          </div>
          <p className="mt-1 text-[11px] text-coral-dark">
            This package was signed by a device that is not a trusted signer.
            Verify this fingerprint out-of-band before trusting these files.
          </p>
          <p className="truncate text-[11px] font-semibold text-coral-dark">
            Signer fingerprint: {fingerprint}
          </p>
          <p className="truncate text-[11px] text-cream-500">{restored}</p>
          <p className="truncate text-[11px] text-cream-500">
            {result.outputDir}
          </p>
        </div>
        <button
          type="button"
          onClick={() => onCopy("decrypt-result", result.outputDir)}
          className="inline-flex shrink-0 items-center justify-center gap-2 rounded-md border border-cream-200 bg-white px-2 py-1 text-[10px] font-semibold text-cream-500 hover:text-terracotta"
        >
          <Copy className="h-3 w-3" />
          {copyLabel}
        </button>
      </div>
    </div>
  );
}

// C1: shown when decrypt is refused fail-closed because the package is signed by
// a valid but UNKNOWN device. Surfaces the fingerprint and an explicit opt-in
// that re-invokes decrypt with allowUnknownSigner=true.
function UnknownSignerRefusalBox({
  message,
  isDecrypting,
  onImportUnknownSigner,
}: {
  message: string;
  isDecrypting: boolean;
  onImportUnknownSigner: () => void;
}) {
  // The backend message ends with "...trust it: <fingerprint>".
  const fingerprint = message.split(": ").pop()?.trim() || "unavailable";
  return (
    <div className="mt-4 rounded-lg border border-coral/40 bg-coral/[0.06] px-3 py-3">
      <div className="flex items-center gap-2">
        <AlertTriangle className="h-4 w-4 shrink-0 text-coral-dark" />
        <p className="text-[12px] font-semibold text-coral-dark">
          Refused: package signed by an UNKNOWN device
        </p>
      </div>
      <p className="mt-2 text-[11px] text-coral-dark">
        The Ed25519 signature is valid, but the signer is not a trusted (approved)
        device. Verify this fingerprint out-of-band before trusting it:
      </p>
      <p className="mt-1 break-all font-mono text-[11px] font-semibold text-coral-dark">
        {fingerprint}
      </p>
      <button
        type="button"
        onClick={onImportUnknownSigner}
        disabled={isDecrypting}
        className="mt-3 inline-flex items-center justify-center gap-2 rounded-lg border border-coral/50 bg-white px-3 py-2 text-[12px] font-semibold text-coral-dark hover:bg-coral/[0.08] disabled:opacity-60"
      >
        <AlertTriangle className="h-3.5 w-3.5" />
        {isDecrypting
          ? "Importing..."
          : "Import anyway (I trust this fingerprint)"}
      </button>
    </div>
  );
}

function Metric({
  label,
  value,
  sub,
  icon: Icon,
}: {
  label: string;
  value: string;
  sub: string;
  icon: LucideIcon;
}) {
  return (
    <article
      className="rounded-lg border border-cream-200 bg-white p-4"
      data-help-title={`${label} is a workspace hygiene metric.`}
      data-help-lines="Workspace metrics separate source code from local data, caches and generated artifacts.|For Aspis Bio, this prevents collaborators and Oracle from touching the wrong files.|Large counts are not automatically bad, but they need classification.|Use Scan workspace after major file changes."
    >
      <div className="mb-2 flex items-center gap-2">
        <Icon className="h-4 w-4 text-terracotta" />
        <p className="text-[10px] font-semibold uppercase tracking-widest text-cream-400">
          {label}
        </p>
      </div>
      <p className="text-xl font-semibold text-cream-800">{value}</p>
      <p className="mt-1 text-[11px] text-cream-400">{sub}</p>
    </article>
  );
}

function InventoryPanel({ items }: { items: WorkspaceSizeEntry[] }) {
  return (
    <section className="rounded-lg border border-cream-200 bg-white p-4">
      <div className="mb-3 flex items-center gap-2">
        <FolderOpen className="h-4 w-4 text-terracotta" />
        <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          Top-Level Size
        </h3>
      </div>
      <div className="space-y-2">
        {items.map((item) => (
          <div
            key={item.path}
            title={item.path}
            className="grid grid-cols-[minmax(0,1fr)_84px_84px] items-center gap-2 rounded-lg bg-cream-50 px-3 py-2"
            data-help-title={`${item.name} is a top-level workspace entry.`}
            data-help-lines="Top-level size shows where the workspace weight actually lives.|For Aspis Bio, code repos can contain local caches and data that should not be pushed.|Large folders should be classified before any cleanup.|This panel is read-only."
          >
            <div className="min-w-0">
              <p className="truncate text-[12px] font-semibold text-cream-800">
                {item.name}
              </p>
              <p className="text-[10px] text-cream-400">{item.entryType}</p>
            </div>
            <p className="text-right text-[12px] font-semibold text-cream-700">
              {formatGb(item.sizeGb)}
            </p>
            <p className="text-right text-[10px] text-cream-400">
              {formatCount(item.fileCount)} files
            </p>
          </div>
        ))}
      </div>
    </section>
  );
}

function LargeFilePanel({ items }: { items: WorkspaceLargeFile[] }) {
  return (
    <section className="rounded-lg border border-cream-200 bg-white p-4">
      <div className="mb-3 flex items-center gap-2">
        <FileWarning className="h-4 w-4 text-amber-dark" />
        <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          Large Files
        </h3>
      </div>
      <div className="space-y-2">
        {items.map((file) => (
          <div
            key={file.path}
            className="rounded-lg bg-cream-50 px-3 py-2"
            data-help-title={`${file.relativePath} is a large file.`}
            data-help-lines="Large files are usually data, models, generated builds or dependency caches.|For Aspis Bio, these should not go to GitHub and should only reach Oracle as summaries or manifests.|The label tells whether it looks regenerable, data-like or suspicious.|Review before delete or move."
          >
            <div className="mb-1 flex items-start justify-between gap-3">
              <p className="min-w-0 break-words text-[12px] font-semibold text-cream-800">
                {file.relativePath}
              </p>
              <span className="shrink-0 rounded-md bg-white px-2 py-1 text-[10px] font-semibold text-cream-600">
                {formatGb(file.sizeGb)}
              </span>
            </div>
            <div className="flex flex-wrap items-center gap-1">
              <span
                className={`rounded-md px-2 py-1 text-[10px] font-semibold ${classTone(file.classLabel)}`}
              >
                {file.classLabel}
              </span>
              <span className="rounded-md bg-white px-2 py-1 text-[10px] text-cream-500">
                {dangerLabel(file)}
              </span>
              <span className="rounded-md bg-white px-2 py-1 text-[10px] text-cream-400">
                {formatDate(file.lastWrite)}
              </span>
            </div>
          </div>
        ))}
      </div>
    </section>
  );
}

function ClassificationRow({ item }: { item: WorkspaceClassificationEntry }) {
  return (
    <div
      className="rounded-lg bg-cream-50 px-3 py-2"
      title={`${item.path}: ${item.notes}`}
      data-help-title={`${item.path} is classified as ${item.classLabel}.`}
      data-help-lines="Classification is the rule agents should follow when deciding Git, Oracle and storage behavior.|Code goes to GitHub and Oracle full text.|Data and outputs use storage/summary mode.|Secrets never go to Git, Oracle or shared sync."
    >
      <div className="mb-1 flex items-center justify-between gap-2">
        <p className="truncate text-[12px] font-semibold text-cream-800">
          {item.path}
        </p>
        <span
          className={`shrink-0 rounded-md px-2 py-1 text-[10px] font-semibold ${classTone(item.classLabel)}`}
        >
          {item.classLabel}
        </span>
      </div>
      <p className="line-clamp-2 text-[11px] leading-4 text-cream-500">
        {item.notes}
      </p>
      <p className="mt-1 truncate font-mono text-[10px] text-cream-400">
        git {item.git} / oracle {item.oracle} / storage {item.storage}
      </p>
    </div>
  );
}
