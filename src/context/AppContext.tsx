import {
  createContext,
  useContext,
  useState,
  useEffect,
  useCallback,
  useMemo,
  useRef,
  type ReactNode,
} from "react";
import type { AppConfig } from "../types/config";
import type {
  AuthState,
  AuxCredentialStatus,
  CloudDashboardSnapshot,
  CloudflareAgentTokenProfileStatus,
  SecretStatus,
  ProviderScopeStatus,
  ProviderConnectionAudit,
  ProviderId,
  SecretRotationResult,
  CloudflareSmokeDryRunResult,
  CloudflareWorkerSettings,
  CloudflareEnvDryRunResult,
  CloudflareEnvWriteResult,
  CloudflareBilling,
  CloudflareAiGatewaySettings,
  CloudflareAiGatewaySettingsPatch,
  CloudflareAutoragReindexResult,
  CloudflareKvKeysPage,
  CloudflareKvValue,
  CloudflareKvWriteResult,
  CloudflareD1QueryResult,
  CloudflareR2Config,
  CloudflareR2WriteResult,
  ScalewayActionResult,
  ScalewayBilling,
  ScalewayResourceAction,
  ScalewayInstanceCreateRequest,
  ScalewayInstanceDryRunResult,
  ScalewayBlockVolumeCreateRequest,
  ScalewayFilesystemCreateRequest,
  ScalewayObjectBucketCreateRequest,
  ScalewaySqlDatabaseCreateRequest,
  ScalewayFunctionCreateRequest,
  ScalewayContainerCreateRequest,
  OracleSnapshot,
  OracleCoverage,
  OracleRuntime,
  OracleAnswer,
  OracleIndexPreferences,
  OracleIndexStatus,
  OracleLlmSettings,
  OracleLlmSettingsStatus,
  OracleNodeCard,
  OracleDuplicateLabel,
  OracleResult,
  OracleDoctorReport,
  OracleIndexedFiles,
  CliAgentsStatus,
  LocalRoleStatus,
  AgentLiveState,
} from "../types/backend";
import { toOracleError } from "../utils/oracleError";
import { mapLegacyViewTarget } from "../utils/deepLink";
import { installVisibilityLock } from "../utils/visibilityLock";
import {
  countLiveAgentSessions,
  softLockActiveAgentsNotice,
} from "../components/agents/agentFleet";
import { useAgentAttentionStore } from "../store/agentAttentionStore";

const LIVE_SYNC_INTERVAL_MS = 60_000;
// Auto-lock only after the window has stayed hidden this long — bumped from
// 20s to 120s (2 minutes). A short hide (macOS App Nap flap, a Space switch,
// a window briefly covering it, the dev-rebuild flash) that returns to visible
// before this elapses does NOT lock. The longer grace avoids locking the user
// out on transient macOS occlusion/App-Nap events that previously made the app
// feel "minimized + frozen". Security intent kept: a genuine walk-away (staying
// hidden) still locks after 2 minutes.
const VISIBILITY_LOCK_GRACE_MS = 120_000;
// Soft-lock idle TTL is refreshed only on genuine user input (not pollers).
// Throttle IPC so pointer/key spam does not flood the backend.
const IDLE_ACTIVITY_TOUCH_THROTTLE_MS = 60_000;
const SCALEWAY_ACTION_FOLLOWUP_DELAYS_MS = [5_000, 15_000, 30_000];

const EMPTY_CONFIG: AppConfig = {
  project: { name: "Devboule", version: "" },
  // Compressed top-level nav (Polis is injected by Sidebar). Re-homed pages now
  // live as tabs: Agents→Projects; Cloudflare/Compute/Budget→Providers;
  // Secrets/Devices/Workspace→Settings (opened from the user area).
  // The cloud "providers" entry is intentionally omitted from the default nav
  // (hidden until the provider-agnostic refactor); it stays reachable by deep
  // link via requestView("providers").
  navigation: [
    { id: "projects", label: "Projects", icon: "FolderKanban" },
    { id: "oracle", label: "Oracle", icon: "BrainCircuit" },
  ],
  providers: [],
  bookmarks: [],
  secrets: [],
  compute: {
    gpus: { active: 0, total: 0, provider: "Scaleway" },
    cpus: { active: 0, total: 0, provider: "Scaleway" },
    workers: { active: 0, total: 0, provider: "Cloudflare" },
  },
  budget: { monthly_limit: 0, currency: "EUR", categories: [] },
  customAgentClients: [],
};

interface AppState {
  config: AppConfig;
  activeView: string;
  // The sub-tab a deep-link asked the active view to open (e.g. "cloudflare"
  // inside Providers, "secrets" inside Settings). Null when none requested.
  pendingTab: string | null;
  isLoading: boolean;
  unlockRetryBlocked: boolean;
  error: string | null;
  isDesktopRuntime: boolean;
  isLocked: boolean;
  /** Soft-lock: copy about agents still running when the vault locked (null if none). */
  lockActiveAgentsNotice: string | null;
  authState: AuthState | null;
  cloudSnapshot: CloudDashboardSnapshot | null;
  oracleSnapshot: OracleSnapshot | null;
  oracleCoverage: OracleCoverage | null;
  oracleRuntime: OracleRuntime | null;
  oracleLlmSettings: OracleLlmSettingsStatus | null;
  oracleIndexPreferences: OracleIndexPreferences | null;
  oracleIndexStatus: OracleIndexStatus | null;
  secretStatuses: SecretStatus[];
  providerScopeStatuses: ProviderScopeStatus[];
  scalewayObjectAccessKeyStatus: AuxCredentialStatus | null;
  scalewayObjectSecretKeyStatus: AuxCredentialStatus | null;
  cloudflareAgentTokenProfiles: CloudflareAgentTokenProfileStatus[];
  // Verified local role + onboarding signal (null while locked/loading).
  roleStatus: LocalRoleStatus | null;
}

interface AppActions {
  setActiveView: (view: string) => void;
  // Navigate to a view and optionally request an inner sub-tab. The target view
  // reads (and clears) the requested tab via consumePendingTab(). Used by the
  // sidebar, jump-search and risk-flag deep-links.
  requestView: (view: string, tab?: string | null) => void;
  // Read-and-clear the pending sub-tab. A view calls this on mount; returns the
  // requested tab once, then null so a later re-render does not re-hijack the
  // user's manual tab choice.
  consumePendingTab: () => string | null;
  refreshConfig: () => Promise<void>;
  refreshCloudDashboard: () => Promise<void>;
  refreshOracleSnapshot: () => Promise<void>;
  refreshOracleCoverage: () => Promise<void>;
  refreshOracleRuntime: () => Promise<void>;
  refreshOracleLlmSettings: () => Promise<void>;
  saveOracleLlmSettings: (
    settings: OracleLlmSettings,
    apiKey?: string | null,
  ) => Promise<OracleLlmSettingsStatus | null>;
  deleteOracleLlmApiKey: () => Promise<OracleLlmSettingsStatus | null>;
  refreshOracleIndexPreferences: () => Promise<void>;
  saveOracleIndexPreferences: (
    preferences: OracleIndexPreferences,
  ) => Promise<OracleIndexPreferences | null>;
  refreshOracleIndexStatus: () => Promise<void>;
  syncOracleTextChunks: () => Promise<OracleIndexStatus | null>;
  startOracleIndexJob: (
    force?: boolean,
    maxBatches?: number,
    idle?: boolean,
    manual?: boolean,
  ) => Promise<Record<string, unknown> | null>;
  startOracleIndexWatcher: () => Promise<Record<string, unknown> | null>;
  stopOracleIndexWatcher: () => Promise<Record<string, unknown> | null>;
  // Oracle query methods now PROPAGATE failures: on a rejected backend command
  // they throw an `isOracleError` value (via toOracleError) instead of
  // swallowing it and returning null/[]. Callers must try/catch and render
  // `error.remediation`. `getOracleNode` still resolves to null for a genuinely
  // missing node (distinct from a runtime error, which throws).
  askOracle: (query: string, limit?: number) => Promise<OracleAnswer>;
  getOracleNode: (nodeId: string) => Promise<OracleNodeCard | null>;
  getOracleSimilar: (nodeId: string, limit?: number) => Promise<OracleResult[]>;
  getOracleDuplicates: () => Promise<OracleDuplicateLabel[]>;
  // New typed diagnostics + CLI-agent wiring bindings (Step 6b backend mirror).
  getOracleDoctor: () => Promise<OracleDoctorReport>;
  getOracleIndexedFiles: (opts?: {
    limit?: number;
    offset?: number;
    filter?: string;
  }) => Promise<OracleIndexedFiles>;
  configureCliAgents: () => Promise<CliAgentsStatus>;
  cliAgentsStatus: () => Promise<CliAgentsStatus>;
  unconfigureCliAgents: () => Promise<CliAgentsStatus>;
  refreshSecretStatuses: () => Promise<void>;
  refreshProviderScopeStatuses: () => Promise<void>;
  refreshScalewayObjectAccessKeyStatus: () => Promise<void>;
  refreshScalewayObjectSecretKeyStatus: () => Promise<void>;
  refreshCloudflareAgentTokenProfiles: () => Promise<void>;
  saveScalewayObjectAccessKey: (
    accessKey: string,
  ) => Promise<AuxCredentialStatus | null>;
  deleteScalewayObjectAccessKey: () => Promise<AuxCredentialStatus | null>;
  saveScalewayObjectSecretKey: (
    secretKey: string,
  ) => Promise<AuxCredentialStatus | null>;
  deleteScalewayObjectSecretKey: () => Promise<AuxCredentialStatus | null>;
  saveProviderScope: (
    provider: ProviderId,
    pinnedId: string,
  ) => Promise<ProviderScopeStatus | null>;
  deleteProviderScope: (
    provider: ProviderId,
  ) => Promise<ProviderScopeStatus | null>;
  auditProviderConnection: (
    provider: ProviderId,
    token: string,
    pinnedId?: string | null,
  ) => Promise<ProviderConnectionAudit | null>;
  auditSavedProviderConnection: (
    provider: ProviderId,
    pinnedId?: string | null,
  ) => Promise<ProviderConnectionAudit | null>;
  syncProviderInventory: (provider?: ProviderId) => Promise<void>;
  saveProviderToken: (
    provider: ProviderId,
    token: string,
    pinnedId?: string | null,
  ) => Promise<SecretStatus | null>;
  deleteProviderToken: (provider: ProviderId) => Promise<SecretStatus | null>;
  saveCloudflareAgentTokenProfile: (
    profileId: string,
    token: string,
  ) => Promise<CloudflareAgentTokenProfileStatus | null>;
  deleteCloudflareAgentTokenProfile: (
    profileId: string,
  ) => Promise<CloudflareAgentTokenProfileStatus | null>;
  rotateCloudflareWorkerSecret: (
    accountId: string,
    workerName: string,
    secretName: string,
    secretValue: string,
  ) => Promise<SecretRotationResult | null>;
  runCloudflareSmokeDryRun: () => Promise<CloudflareSmokeDryRunResult | null>;
  fetchCloudflareWorkerSettings: (
    workerName: string,
  ) => Promise<CloudflareWorkerSettings | null>;
  cloudflareEnvDryRun: (
    workerName: string,
    varName: string,
    newValue: string,
  ) => Promise<CloudflareEnvDryRunResult | null>;
  cloudflareSetWorkerEnv: (
    workerName: string,
    varName: string,
    newValue: string,
  ) => Promise<CloudflareEnvWriteResult | null>;
  fetchCloudflareBilling: () => Promise<CloudflareBilling | null>;
  fetchScalewayBilling: () => Promise<ScalewayBilling | null>;
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
  ) => Promise<CloudflareKvKeysPage | null>;
  fetchCloudflareKvValue: (
    namespaceId: string,
    key: string,
  ) => Promise<CloudflareKvValue | null>;
  setCloudflareKvValue: (
    namespaceId: string,
    key: string,
    value: string,
  ) => Promise<CloudflareKvWriteResult | null>;
  deleteCloudflareKvValue: (
    namespaceId: string,
    key: string,
    confirmKey: string,
  ) => Promise<CloudflareKvWriteResult | null>;
  cloudflareD1Query: (
    databaseId: string,
    sql: string,
    confirm: boolean,
  ) => Promise<CloudflareD1QueryResult | null>;
  fetchCloudflareR2Config: (
    bucket: string,
  ) => Promise<CloudflareR2Config | null>;
  setCloudflareR2Lifecycle: (
    bucket: string,
    rules: unknown,
  ) => Promise<CloudflareR2WriteResult | null>;
  setCloudflareR2Cors: (
    bucket: string,
    rules: unknown,
  ) => Promise<CloudflareR2WriteResult | null>;
  performScalewayResourceAction: (
    resourceId: string,
    action: ScalewayResourceAction,
    confirmResourceName?: string | null,
  ) => Promise<ScalewayActionResult | null>;
  // --- Scaleway P1 storage + P6 generic create/delete/mutate wrappers -------
  // READ: epoch-guarded, no global isLoading, no setError(null) on entry.
  scalewayInstanceCreateDryRun: (
    request: ScalewayInstanceCreateRequest,
  ) => Promise<ScalewayInstanceDryRunResult | null>;
  // WRITE: mirror performScalewayResourceAction (isLoading + epoch guard +
  // re-sync on success so the new/removed resource appears after a sync).
  createScalewayInstance: (
    request: ScalewayInstanceCreateRequest,
  ) => Promise<ScalewayActionResult | null>;
  createScalewayBlockVolume: (
    request: ScalewayBlockVolumeCreateRequest,
  ) => Promise<ScalewayActionResult | null>;
  resizeScalewayBlockVolume: (
    resourceId: string,
    newSizeGib: number,
  ) => Promise<ScalewayActionResult | null>;
  createScalewayBlockSnapshot: (
    volumeId: string,
    name: string,
  ) => Promise<ScalewayActionResult | null>;
  deleteScalewayBlockStorage: (
    resourceId: string,
    confirmResourceName?: string | null,
  ) => Promise<ScalewayActionResult | null>;
  createScalewayFilesystem: (
    request: ScalewayFilesystemCreateRequest,
  ) => Promise<ScalewayActionResult | null>;
  deleteScalewayFilesystem: (
    resourceId: string,
    confirmResourceName?: string | null,
  ) => Promise<ScalewayActionResult | null>;
  createScalewayObjectBucket: (
    request: ScalewayObjectBucketCreateRequest,
  ) => Promise<ScalewayActionResult | null>;
  deleteScalewayObjectBucket: (
    resourceId: string,
    confirmResourceName?: string | null,
  ) => Promise<ScalewayActionResult | null>;
  setScalewayObjectBucketLifecycle: (
    resourceId: string,
    rules: unknown,
  ) => Promise<ScalewayActionResult | null>;
  createScalewaySqlDatabase: (
    request: ScalewaySqlDatabaseCreateRequest,
  ) => Promise<ScalewayActionResult | null>;
  deleteScalewaySqlDatabase: (
    resourceId: string,
    confirmResourceName?: string | null,
  ) => Promise<ScalewayActionResult | null>;
  createScalewayFunction: (
    request: ScalewayFunctionCreateRequest,
  ) => Promise<ScalewayActionResult | null>;
  deleteScalewayFunction: (
    resourceId: string,
    confirmResourceName?: string | null,
  ) => Promise<ScalewayActionResult | null>;
  createScalewayContainer: (
    request: ScalewayContainerCreateRequest,
  ) => Promise<ScalewayActionResult | null>;
  deleteScalewayContainer: (
    resourceId: string,
    confirmResourceName?: string | null,
  ) => Promise<ScalewayActionResult | null>;
  unlock: () => Promise<void>;
  lock: () => Promise<void>;
  // Re-resolve the verified local role (used by the dev-only role switcher).
  refreshRole: () => Promise<void>;
}

type AppContextValue = AppState & AppActions;

const AppContext = createContext<AppContextValue | null>(null);

// Stable actions-only context. Every action is useCallback-stable, so this
// object is memoized once and never changes identity for the provider's
// lifetime. Consumers that only need actions (Sidebar, Header, tables) can
// subscribe here and avoid re-rendering on every state change.
const AppActionsContext = createContext<AppActions | null>(null);

export function isTauriRuntime(): boolean {
  const tauriInternals = (
    window as Window & {
      __TAURI_INTERNALS__?: { invoke?: unknown };
    }
  ).__TAURI_INTERNALS__;
  return typeof tauriInternals?.invoke === "function";
}

export async function invokeBackendCommand<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (!isTauriRuntime()) {
    throw new Error(
      "Devboule must be opened as the desktop app to use device authentication.",
    );
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(command, args);
}

// Normalize raw config to prevent shell crashes from bootstrapped/invalid configs (e.g. {}).
// Exported for the regression test (the macOS white-screen bug: `{}` config →
// `config.navigation.some` threw in Sidebar with no shell error boundary).
export function normalizeConfig(raw: unknown): AppConfig {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
    return EMPTY_CONFIG;
  }
  return { ...EMPTY_CONFIG, ...(raw as Partial<AppConfig>) };
}

async function fetchConfig(): Promise<AppConfig> {
  try {
    const result = await invokeBackendCommand<{ raw: AppConfig }>("get_config");
    return normalizeConfig(result.raw);
  } catch (e) {
    if (isTauriRuntime()) throw e;
    const resp = await fetch("/config.json");
    if (resp.ok) return normalizeConfig(await resp.json());
    throw e;
  }
}

export function AppProvider({ children }: { children: ReactNode }) {
  const [config, setConfig] = useState<AppConfig>(EMPTY_CONFIG);
  const [activeView, setActiveView] = useState("projects");
  // A deep-link's requested sub-tab, consumed once by the target view. Mirrored
  // in a ref so consumePendingTab() reads-and-clears synchronously.
  const [pendingTab, setPendingTab] = useState<string | null>(null);
  const pendingTabRef = useRef<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [unlockRetryBlocked, setUnlockRetryBlocked] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [isDesktopRuntime] = useState(() => isTauriRuntime());
  const [isLocked, setIsLocked] = useState(true);
  const [lockActiveAgentsNotice, setLockActiveAgentsNotice] = useState<
    string | null
  >(null);
  const [authState, setAuthState] = useState<AuthState | null>(null);
  const [cloudSnapshot, setCloudSnapshot] =
    useState<CloudDashboardSnapshot | null>(null);
  const [oracleSnapshot, setOracleSnapshot] = useState<OracleSnapshot | null>(
    null,
  );
  const [oracleCoverage, setOracleCoverage] = useState<OracleCoverage | null>(
    null,
  );
  const [oracleRuntime, setOracleRuntime] = useState<OracleRuntime | null>(
    null,
  );
  const [oracleLlmSettings, setOracleLlmSettings] =
    useState<OracleLlmSettingsStatus | null>(null);
  const [oracleIndexPreferences, setOracleIndexPreferences] =
    useState<OracleIndexPreferences | null>(null);
  const [oracleIndexStatus, setOracleIndexStatus] =
    useState<OracleIndexStatus | null>(null);
  const [secretStatuses, setSecretStatuses] = useState<SecretStatus[]>([]);
  const [providerScopeStatuses, setProviderScopeStatuses] = useState<
    ProviderScopeStatus[]
  >([]);
  const [scalewayObjectAccessKeyStatus, setScalewayObjectAccessKeyStatus] =
    useState<AuxCredentialStatus | null>(null);
  const [scalewayObjectSecretKeyStatus, setScalewayObjectSecretKeyStatus] =
    useState<AuxCredentialStatus | null>(null);
  const [cloudflareAgentTokenProfiles, setCloudflareAgentTokenProfiles] =
    useState<CloudflareAgentTokenProfileStatus[]>([]);
  const [roleStatus, setRoleStatus] = useState<LocalRoleStatus | null>(null);
  const authEpochRef = useRef(0);
  const unlockInFlightRef = useRef(false);
  const unlockRetryBlockedRef = useRef(false);
  const unlockRetryTimerRef = useRef<number | null>(null);
  const autoWatchAttemptedEpochRef = useRef<number | null>(null);
  const cloudRefreshInFlightRef = useRef(false);
  const scalewayFollowupTimersRef = useRef<number[]>([]);

  const clearUnlockRetryCooldown = useCallback(() => {
    if (unlockRetryTimerRef.current !== null) {
      window.clearTimeout(unlockRetryTimerRef.current);
      unlockRetryTimerRef.current = null;
    }
    unlockRetryBlockedRef.current = false;
    setUnlockRetryBlocked(false);
  }, []);

  const startUnlockRetryCooldown = useCallback(
    (durationMs = 6500) => {
      clearUnlockRetryCooldown();
      unlockRetryBlockedRef.current = true;
      setUnlockRetryBlocked(true);
      unlockRetryTimerRef.current = window.setTimeout(() => {
        unlockRetryBlockedRef.current = false;
        unlockRetryTimerRef.current = null;
        setUnlockRetryBlocked(false);
      }, durationMs);
    },
    [clearUnlockRetryCooldown],
  );

  const clearSensitiveState = useCallback(() => {
    scalewayFollowupTimersRef.current.forEach((id) => window.clearTimeout(id));
    scalewayFollowupTimersRef.current = [];
    // A lock bumps the auth epoch, so any in-flight write's `finally` will skip
    // its `setIsLoading(false)` (epoch mismatch) and strand the global loading
    // state. Clear it here at the source so a lock mid-write never hangs the UI.
    setIsLoading(false);
    setCloudSnapshot(null);
    setOracleSnapshot(null);
    setOracleCoverage(null);
    setOracleRuntime(null);
    setOracleLlmSettings(null);
    setOracleIndexPreferences(null);
    setOracleIndexStatus(null);
    setSecretStatuses([]);
    setProviderScopeStatuses([]);
    setScalewayObjectAccessKeyStatus(null);
    setScalewayObjectSecretKeyStatus(null);
    setRoleStatus(null);
    // Credential metadata (agent token profile ids + token health) must not
    // outlive a lock either, otherwise it lingers in memory after the vault is
    // sealed and re-renders into the UI.
    setCloudflareAgentTokenProfiles([]);
  }, []);

  const applyAuthState = useCallback(
    (next: AuthState) => {
      setAuthState(next);
      setIsLocked(next.locked);
      if (next.locked) {
        authEpochRef.current += 1;
        clearSensitiveState();
      }
    },
    [clearSensitiveState],
  );

  // Resolve the verified local role once unlocked. Resolution can never deny
  // access (see roles::resolve_local_role), so this never locks anyone out.
  const fetchRole = useCallback(async () => {
    try {
      const status =
        await invokeBackendCommand<LocalRoleStatus>("get_local_role");
      setRoleStatus(status);
    } catch {
      // Keep the PREVIOUS role on a transient error. Nulling it here would bounce
      // an already-onboarded user back into the wizard on a momentary hiccup
      // (audit H1). The lock effect below is what clears the role.
    }
  }, []);

  useEffect(() => {
    if (!isLocked && isDesktopRuntime) {
      void fetchRole();
    } else {
      setRoleStatus(null);
    }
  }, [isLocked, isDesktopRuntime, fetchRole]);

  const applyCloudSnapshot = useCallback(
    (snapshot: CloudDashboardSnapshot, requestEpoch: number) => {
      if (requestEpoch !== authEpochRef.current) return false;
      applyAuthState(snapshot.auth);
      if (snapshot.auth.locked) return false;
      setCloudSnapshot(snapshot);
      return true;
    },
    [applyAuthState],
  );

  const scheduleScalewayActionFollowups = useCallback(
    (requestEpoch: number) => {
      scalewayFollowupTimersRef.current.forEach((id) =>
        window.clearTimeout(id),
      );
      scalewayFollowupTimersRef.current =
        SCALEWAY_ACTION_FOLLOWUP_DELAYS_MS.map((delay) =>
          window.setTimeout(async () => {
            if (
              requestEpoch !== authEpochRef.current ||
              cloudRefreshInFlightRef.current
            ) {
              return;
            }
            cloudRefreshInFlightRef.current = true;
            try {
              const snapshot =
                await invokeBackendCommand<CloudDashboardSnapshot>(
                  "sync_provider_inventory",
                  { provider: "scaleway" },
                );
              if (requestEpoch === authEpochRef.current) {
                applyCloudSnapshot(snapshot, requestEpoch);
              }
            } catch {
              // Follow-up syncs are best-effort; the immediate action path reports user-visible failures.
            } finally {
              cloudRefreshInFlightRef.current = false;
            }
          }, delay),
        );
    },
    [applyCloudSnapshot],
  );

  const refreshAuthState = useCallback(async () => {
    if (unlockInFlightRef.current) return;
    if (!isDesktopRuntime) {
      setIsLocked(true);
      setAuthState({
        locked: true,
        helloAvailable: false,
        lastUnlockedAt: null,
        lockReason: "unavailable",
      });
      authEpochRef.current += 1;
      clearSensitiveState();
      return;
    }

    try {
      const next = await invokeBackendCommand<AuthState>("get_auth_state");
      applyAuthState(next);
    } catch (e) {
      setIsLocked(true);
      setAuthState({
        locked: true,
        helloAvailable: false,
        lastUnlockedAt: null,
        lockReason: "unavailable",
      });
      authEpochRef.current += 1;
      clearSensitiveState();
      console.warn(
        e instanceof Error
          ? e.message
          : "Authentication backend is unavailable.",
      );
    }
  }, [applyAuthState, clearSensitiveState, isDesktopRuntime]);

  const refreshCloudDashboard = useCallback(async () => {
    if (unlockInFlightRef.current) return;
    if (cloudRefreshInFlightRef.current) return;
    cloudRefreshInFlightRef.current = true;
    const requestEpoch = authEpochRef.current;
    try {
      const snapshot = await invokeBackendCommand<CloudDashboardSnapshot>(
        "get_cloud_dashboard_snapshot",
      );
      applyCloudSnapshot(snapshot, requestEpoch);
    } catch (e) {
      if (requestEpoch !== authEpochRef.current) return;
      setError(
        e instanceof Error ? e.message : "Cloud dashboard refresh failed.",
      );
    } finally {
      cloudRefreshInFlightRef.current = false;
    }
  }, [applyCloudSnapshot]);

  const refreshOracleSnapshot = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    const snapshot = await invokeBackendCommand<OracleSnapshot>(
      "get_oracle_snapshot",
    );
    if (requestEpoch !== authEpochRef.current) return;
    setOracleSnapshot(snapshot);
  }, []);

  const refreshOracleCoverage = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    const coverage = await invokeBackendCommand<OracleCoverage>(
      "get_oracle_coverage",
    );
    if (requestEpoch !== authEpochRef.current) return;
    setOracleCoverage(coverage);
  }, []);

  const refreshOracleRuntime = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    const runtime =
      await invokeBackendCommand<OracleRuntime>("get_oracle_runtime");
    if (requestEpoch !== authEpochRef.current) return;
    setOracleRuntime(runtime);
  }, []);

  const refreshOracleLlmSettings = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    const status = await invokeBackendCommand<OracleLlmSettingsStatus>(
      "get_oracle_llm_settings",
    );
    if (requestEpoch !== authEpochRef.current) return;
    setOracleLlmSettings(status);
  }, []);

  const saveOracleLlmSettings = useCallback(
    async (settings: OracleLlmSettings, apiKey?: string | null) => {
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        const status = await invokeBackendCommand<OracleLlmSettingsStatus>(
          "save_oracle_llm_settings",
          {
            settings,
            apiKey: apiKey ?? null,
          },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        setOracleLlmSettings(status);
        return status;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error ? e.message : "Saving Oracle LLM settings failed.",
        );
        return null;
      } finally {
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [],
  );

  const deleteOracleLlmApiKey = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    setIsLoading(true);
    setError(null);
    try {
      const status = await invokeBackendCommand<OracleLlmSettingsStatus>(
        "delete_oracle_llm_api_key",
      );
      if (requestEpoch !== authEpochRef.current) return null;
      setOracleLlmSettings(status);
      return status;
    } catch (e) {
      if (requestEpoch !== authEpochRef.current) return null;
      setError(
        e instanceof Error ? e.message : "Deleting Oracle LLM API key failed.",
      );
      return null;
    } finally {
      if (requestEpoch === authEpochRef.current) setIsLoading(false);
    }
  }, []);

  const refreshOracleIndexPreferences = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    const preferences = await invokeBackendCommand<OracleIndexPreferences>(
      "get_oracle_index_preferences",
    );
    if (requestEpoch !== authEpochRef.current) return;
    setOracleIndexPreferences(preferences);
  }, []);

  const saveOracleIndexPreferences = useCallback(
    async (preferences: OracleIndexPreferences) => {
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        const saved = await invokeBackendCommand<OracleIndexPreferences>(
          "save_oracle_index_preferences",
          { preferences },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        setOracleIndexPreferences(saved);
        return saved;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error
            ? e.message
            : "Saving Oracle index preferences failed.",
        );
        return null;
      } finally {
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [],
  );

  const refreshOracleIndexStatus = useCallback(async () => {
    try {
      const requestEpoch = authEpochRef.current;
      const status = await invokeBackendCommand<OracleIndexStatus>(
        "get_oracle_index_status",
      );
      if (requestEpoch !== authEpochRef.current) return;
      setOracleIndexStatus(status);
    } catch {
      // A failed fetch means the server is unreachable — a stale "indexing"
      // status must NOT survive an outage and mask a genuinely-down Oracle.
      setOracleIndexStatus(null);
    }
  }, []);

  const syncOracleTextChunks = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    setIsLoading(true);
    setError(null);
    try {
      await invokeBackendCommand<Record<string, unknown>>(
        "sync_oracle_text_chunks",
      );
      if (requestEpoch !== authEpochRef.current) return null;
      const status = await invokeBackendCommand<OracleIndexStatus>(
        "get_oracle_index_status",
      );
      if (requestEpoch !== authEpochRef.current) return null;
      setOracleIndexStatus(status);
      return status;
    } catch (e) {
      if (requestEpoch !== authEpochRef.current) return null;
      setError(
        e instanceof Error ? e.message : "Oracle text chunk sync failed.",
      );
      return null;
    } finally {
      if (requestEpoch === authEpochRef.current) setIsLoading(false);
    }
  }, []);

  const startOracleIndexJob = useCallback(
    async (force = false, maxBatches = 1, idle = true, manual = false) => {
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        const result = await invokeBackendCommand<Record<string, unknown>>(
          "start_oracle_index_job",
          {
            force,
            maxBatches,
            idle,
            manual,
          },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        const status = await invokeBackendCommand<OracleIndexStatus>(
          "get_oracle_index_status",
        );
        if (requestEpoch !== authEpochRef.current) return null;
        setOracleIndexStatus(status);
        return result;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error
            ? e.message
            : "Oracle dense index job failed to start.",
        );
        return null;
      } finally {
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [],
  );

  const startOracleIndexWatcher = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    setError(null);
    try {
      const result = await invokeBackendCommand<Record<string, unknown>>(
        "start_oracle_index_watcher",
      );
      if (requestEpoch !== authEpochRef.current) return null;
      await refreshOracleIndexStatus();
      return result;
    } catch (e) {
      if (requestEpoch !== authEpochRef.current) return null;
      setError(
        e instanceof Error
          ? e.message
          : "Oracle index watcher failed to start.",
      );
      return null;
    }
  }, [refreshOracleIndexStatus]);

  const stopOracleIndexWatcher = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    setError(null);
    try {
      const result = await invokeBackendCommand<Record<string, unknown>>(
        "stop_oracle_index_watcher",
      );
      if (requestEpoch !== authEpochRef.current) return null;
      await refreshOracleIndexStatus();
      return result;
    } catch (e) {
      if (requestEpoch !== authEpochRef.current) return null;
      setError(
        e instanceof Error ? e.message : "Oracle index watcher failed to stop.",
      );
      return null;
    }
  }, [refreshOracleIndexStatus]);

  // PROPAGATE typed errors instead of swallowing. On a rejected backend command
  // we normalize the rejection (object | stringified-JSON | string | Error)
  // into an `isOracleError` value and re-throw it so the caller (OracleView 6c)
  // can render `error.remediation`. We intentionally do NOT setError() here —
  // surfacing is the caller's job to avoid a duplicate global banner.
  const askOracle = useCallback(async (query: string, limit = 6) => {
    setError(null);
    try {
      return await invokeBackendCommand<OracleAnswer>("ask_oracle", {
        query,
        limit,
      });
    } catch (e) {
      throw toOracleError(e);
    }
  }, []);

  const getOracleNode = useCallback(async (nodeId: string) => {
    setError(null);
    try {
      return await invokeBackendCommand<OracleNodeCard>("get_oracle_node", {
        nodeId,
      });
    } catch (e) {
      throw toOracleError(e);
    }
  }, []);

  const getOracleSimilar = useCallback(async (nodeId: string, limit = 8) => {
    setError(null);
    try {
      return await invokeBackendCommand<OracleResult[]>("get_oracle_similar", {
        nodeId,
        limit,
      });
    } catch (e) {
      throw toOracleError(e);
    }
  }, []);

  const getOracleDoctor = useCallback(async () => {
    try {
      return await invokeBackendCommand<OracleDoctorReport>(
        "get_oracle_doctor",
      );
    } catch (e) {
      throw toOracleError(e);
    }
  }, []);

  const getOracleIndexedFiles = useCallback(
    async (opts?: { limit?: number; offset?: number; filter?: string }) => {
      try {
        return await invokeBackendCommand<OracleIndexedFiles>(
          "get_oracle_indexed_files",
          {
            limit: opts?.limit,
            offset: opts?.offset,
            filter: opts?.filter,
          },
        );
      } catch (e) {
        throw toOracleError(e);
      }
    },
    [],
  );

  const configureCliAgents = useCallback(async () => {
    try {
      return await invokeBackendCommand<CliAgentsStatus>(
        "configure_cli_agents",
      );
    } catch (e) {
      throw toOracleError(e);
    }
  }, []);

  const cliAgentsStatus = useCallback(async () => {
    try {
      return await invokeBackendCommand<CliAgentsStatus>("cli_agents_status");
    } catch (e) {
      throw toOracleError(e);
    }
  }, []);

  const unconfigureCliAgents = useCallback(async () => {
    try {
      return await invokeBackendCommand<CliAgentsStatus>(
        "unconfigure_cli_agents",
      );
    } catch (e) {
      throw toOracleError(e);
    }
  }, []);

  const getOracleDuplicates = useCallback(async () => {
    setError(null);
    try {
      return await invokeBackendCommand<OracleDuplicateLabel[]>(
        "get_oracle_duplicates",
      );
    } catch (e) {
      setError(
        e instanceof Error
          ? e.message
          : "Architecture Oracle duplicate lookup failed.",
      );
      return [];
    }
  }, []);

  const refreshSecretStatuses = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    const statuses =
      await invokeBackendCommand<SecretStatus[]>("get_secret_status");
    if (requestEpoch !== authEpochRef.current) return;
    setSecretStatuses(statuses);
  }, []);

  const refreshProviderScopeStatuses = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    const statuses = await invokeBackendCommand<ProviderScopeStatus[]>(
      "get_provider_scope_status",
    );
    if (requestEpoch !== authEpochRef.current) return;
    setProviderScopeStatuses(statuses);
  }, []);

  const refreshScalewayObjectAccessKeyStatus = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    const status = await invokeBackendCommand<AuxCredentialStatus>(
      "get_scaleway_object_access_key_status",
    );
    if (requestEpoch !== authEpochRef.current) return;
    setScalewayObjectAccessKeyStatus(status);
  }, []);

  const refreshScalewayObjectSecretKeyStatus = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    const status = await invokeBackendCommand<AuxCredentialStatus>(
      "get_scaleway_object_secret_key_status",
    );
    if (requestEpoch !== authEpochRef.current) return;
    setScalewayObjectSecretKeyStatus(status);
  }, []);

  const refreshCloudflareAgentTokenProfiles = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    const statuses = await invokeBackendCommand<
      CloudflareAgentTokenProfileStatus[]
    >("get_cloudflare_agent_token_profiles");
    if (requestEpoch !== authEpochRef.current) return;
    setCloudflareAgentTokenProfiles(statuses);
  }, []);

  const saveScalewayObjectAccessKey = useCallback(async (accessKey: string) => {
    const requestEpoch = authEpochRef.current;
    setIsLoading(true);
    setError(null);
    try {
      const status = await invokeBackendCommand<AuxCredentialStatus>(
        "save_scaleway_object_access_key",
        { accessKey },
      );
      if (requestEpoch !== authEpochRef.current) return null;
      setCloudSnapshot(null);
      setScalewayObjectAccessKeyStatus(status);
      return status;
    } catch (e) {
      if (requestEpoch !== authEpochRef.current) return null;
      setError(
        e instanceof Error
          ? e.message
          : "Saving Scaleway Object Storage access key failed.",
      );
      return null;
    } finally {
      if (requestEpoch === authEpochRef.current) setIsLoading(false);
    }
  }, []);

  const saveScalewayObjectSecretKey = useCallback(async (secretKey: string) => {
    const requestEpoch = authEpochRef.current;
    setIsLoading(true);
    setError(null);
    try {
      const status = await invokeBackendCommand<AuxCredentialStatus>(
        "save_scaleway_object_secret_key",
        { secretKey },
      );
      if (requestEpoch !== authEpochRef.current) return null;
      setCloudSnapshot(null);
      setScalewayObjectSecretKeyStatus(status);
      return status;
    } catch (e) {
      if (requestEpoch !== authEpochRef.current) return null;
      setError(
        e instanceof Error
          ? e.message
          : "Saving Scaleway Object Storage secret key failed.",
      );
      return null;
    } finally {
      if (requestEpoch === authEpochRef.current) setIsLoading(false);
    }
  }, []);

  const deleteScalewayObjectAccessKey = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    setIsLoading(true);
    setError(null);
    try {
      const status = await invokeBackendCommand<AuxCredentialStatus>(
        "delete_scaleway_object_access_key",
      );
      if (requestEpoch !== authEpochRef.current) return null;
      setScalewayObjectAccessKeyStatus(status);
      const snapshot = await invokeBackendCommand<CloudDashboardSnapshot>(
        "sync_provider_inventory",
        {
          provider: "scaleway",
        },
      );
      if (requestEpoch !== authEpochRef.current) return null;
      applyCloudSnapshot(snapshot, requestEpoch);
      return status;
    } catch (e) {
      if (requestEpoch !== authEpochRef.current) return null;
      setError(
        e instanceof Error
          ? e.message
          : "Deleting Scaleway Object Storage access key failed.",
      );
      return null;
    } finally {
      if (requestEpoch === authEpochRef.current) setIsLoading(false);
    }
  }, [applyCloudSnapshot]);

  const deleteScalewayObjectSecretKey = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    setIsLoading(true);
    setError(null);
    try {
      const status = await invokeBackendCommand<AuxCredentialStatus>(
        "delete_scaleway_object_secret_key",
      );
      if (requestEpoch !== authEpochRef.current) return null;
      setScalewayObjectSecretKeyStatus(status);
      const snapshot = await invokeBackendCommand<CloudDashboardSnapshot>(
        "sync_provider_inventory",
        {
          provider: "scaleway",
        },
      );
      if (requestEpoch !== authEpochRef.current) return null;
      applyCloudSnapshot(snapshot, requestEpoch);
      return status;
    } catch (e) {
      if (requestEpoch !== authEpochRef.current) return null;
      setError(
        e instanceof Error
          ? e.message
          : "Deleting Scaleway Object Storage secret key failed.",
      );
      return null;
    } finally {
      if (requestEpoch === authEpochRef.current) setIsLoading(false);
    }
  }, [applyCloudSnapshot]);

  const saveProviderScope = useCallback(
    async (provider: ProviderId, pinnedId: string) => {
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        const status = await invokeBackendCommand<ProviderScopeStatus>(
          "save_provider_scope",
          {
            provider,
            pinnedId,
          },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        setCloudSnapshot(null);
        setProviderScopeStatuses((prev) => {
          const rest = prev.filter((item) => item.provider !== status.provider);
          return [...rest, status].sort((a, b) =>
            a.provider.localeCompare(b.provider),
          );
        });
        return status;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error ? e.message : "Saving provider scope failed.",
        );
        return null;
      } finally {
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [],
  );

  const deleteProviderScope = useCallback(async (provider: ProviderId) => {
    const requestEpoch = authEpochRef.current;
    setIsLoading(true);
    setError(null);
    try {
      const status = await invokeBackendCommand<ProviderScopeStatus>(
        "delete_provider_scope",
        {
          provider,
        },
      );
      if (requestEpoch !== authEpochRef.current) return null;
      setCloudSnapshot(null);
      setProviderScopeStatuses((prev) => {
        const rest = prev.filter((item) => item.provider !== status.provider);
        return [...rest, status].sort((a, b) =>
          a.provider.localeCompare(b.provider),
        );
      });
      return status;
    } catch (e) {
      if (requestEpoch !== authEpochRef.current) return null;
      setError(
        e instanceof Error ? e.message : "Deleting provider scope failed.",
      );
      return null;
    } finally {
      if (requestEpoch === authEpochRef.current) setIsLoading(false);
    }
  }, []);

  const auditProviderConnection = useCallback(
    async (provider: ProviderId, token: string, pinnedId?: string | null) => {
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        const audit = await invokeBackendCommand<ProviderConnectionAudit>(
          "audit_provider_connection",
          {
            provider,
            token,
            pinnedId: pinnedId ?? null,
          },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        return audit;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error ? e.message : "Provider connection audit failed.",
        );
        return null;
      } finally {
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [],
  );

  const auditSavedProviderConnection = useCallback(
    async (provider: ProviderId, pinnedId?: string | null) => {
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        const audit = await invokeBackendCommand<ProviderConnectionAudit>(
          "audit_saved_provider_connection",
          {
            provider,
            pinnedId: pinnedId ?? null,
          },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        return audit;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error
            ? e.message
            : "Saved provider connection audit failed.",
        );
        return null;
      } finally {
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [],
  );

  const syncProviderInventory = useCallback(
    async (provider?: ProviderId) => {
      if (cloudRefreshInFlightRef.current) return;
      cloudRefreshInFlightRef.current = true;
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        const snapshot = await invokeBackendCommand<CloudDashboardSnapshot>(
          "sync_provider_inventory",
          {
            provider: provider ?? null,
          },
        );
        applyCloudSnapshot(snapshot, requestEpoch);
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return;
        setError(
          e instanceof Error ? e.message : "Provider inventory sync failed.",
        );
      } finally {
        cloudRefreshInFlightRef.current = false;
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [applyCloudSnapshot],
  );

  const saveProviderToken = useCallback(
    async (provider: ProviderId, token: string, pinnedId?: string | null) => {
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        const status = await invokeBackendCommand<SecretStatus>(
          "save_provider_token",
          {
            provider,
            token,
            pinnedId: pinnedId ?? null,
          },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        setSecretStatuses((prev) => {
          const rest = prev.filter((item) => item.provider !== status.provider);
          return [...rest, status].sort((a, b) =>
            a.provider.localeCompare(b.provider),
          );
        });
        return status;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error ? e.message : "Saving provider token failed.",
        );
        return null;
      } finally {
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [],
  );

  const deleteProviderToken = useCallback(
    async (provider: ProviderId) => {
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        const status = await invokeBackendCommand<SecretStatus>(
          "delete_provider_token",
          {
            provider,
          },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        setSecretStatuses((prev) => {
          const rest = prev.filter((item) => item.provider !== status.provider);
          return [...rest, status].sort((a, b) =>
            a.provider.localeCompare(b.provider),
          );
        });
        const snapshot = await invokeBackendCommand<CloudDashboardSnapshot>(
          "sync_provider_inventory",
          {
            provider,
          },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        applyCloudSnapshot(snapshot, requestEpoch);
        return status;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error ? e.message : "Deleting provider token failed.",
        );
        return null;
      } finally {
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [applyCloudSnapshot],
  );

  const saveCloudflareAgentTokenProfile = useCallback(
    async (profileId: string, token: string) => {
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        const status =
          await invokeBackendCommand<CloudflareAgentTokenProfileStatus>(
            "save_cloudflare_agent_token_profile",
            { profileId, token },
          );
        if (requestEpoch !== authEpochRef.current) return null;
        setCloudflareAgentTokenProfiles((prev) => {
          const rest = prev.filter((item) => item.id !== status.id);
          return [...rest, status].sort((a, b) => a.id.localeCompare(b.id));
        });
        return status;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error
            ? e.message
            : "Saving Cloudflare agent token profile failed.",
        );
        return null;
      } finally {
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [],
  );

  const deleteCloudflareAgentTokenProfile = useCallback(
    async (profileId: string) => {
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        const status =
          await invokeBackendCommand<CloudflareAgentTokenProfileStatus>(
            "delete_cloudflare_agent_token_profile",
            { profileId },
          );
        if (requestEpoch !== authEpochRef.current) return null;
        setCloudflareAgentTokenProfiles((prev) => {
          const rest = prev.filter((item) => item.id !== status.id);
          return [...rest, status].sort((a, b) => a.id.localeCompare(b.id));
        });
        return status;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error
            ? e.message
            : "Deleting Cloudflare agent token profile failed.",
        );
        return null;
      } finally {
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [],
  );

  const rotateCloudflareWorkerSecret = useCallback(
    async (
      accountId: string,
      workerName: string,
      secretName: string,
      secretValue: string,
    ) => {
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        const result = await invokeBackendCommand<SecretRotationResult>(
          "rotate_cloudflare_worker_secret",
          {
            accountId,
            workerName,
            secretName,
            secretValue,
          },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        const snapshot = await invokeBackendCommand<CloudDashboardSnapshot>(
          "get_cloud_dashboard_snapshot",
        );
        if (requestEpoch !== authEpochRef.current) return null;
        applyCloudSnapshot(snapshot, requestEpoch);
        return result;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error
            ? e.message
            : "Cloudflare Worker secret rotation failed.",
        );
        return null;
      } finally {
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [applyCloudSnapshot],
  );

  const runCloudflareSmokeDryRun = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    setIsLoading(true);
    setError(null);
    try {
      const result = await invokeBackendCommand<CloudflareSmokeDryRunResult>(
        "cloudflare_smoke_dry_run",
      );
      if (requestEpoch !== authEpochRef.current) return null;
      const snapshot = await invokeBackendCommand<CloudDashboardSnapshot>(
        "get_cloud_dashboard_snapshot",
      );
      if (requestEpoch !== authEpochRef.current) return null;
      applyCloudSnapshot(snapshot, requestEpoch);
      return result;
    } catch (e) {
      if (requestEpoch !== authEpochRef.current) return null;
      setError(
        e instanceof Error ? e.message : "Cloudflare smoke dry run failed.",
      );
      return null;
    } finally {
      if (requestEpoch === authEpochRef.current) setIsLoading(false);
    }
  }, [applyCloudSnapshot]);

  const fetchCloudflareWorkerSettings = useCallback(
    async (workerName: string) => {
      const requestEpoch = authEpochRef.current;
      // Read-only: do not wipe an unrelated in-flight error banner. Failures
      // surface via the view's local message state.
      try {
        const result = await invokeBackendCommand<CloudflareWorkerSettings>(
          "fetch_cloudflare_worker_settings",
          { workerName },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        return result;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error
            ? e.message
            : "Cloudflare Worker settings could not be loaded.",
        );
        return null;
      }
    },
    [],
  );

  const cloudflareEnvDryRun = useCallback(
    async (workerName: string, varName: string, newValue: string) => {
      const requestEpoch = authEpochRef.current;
      // Read-only: do not wipe an unrelated in-flight error banner.
      try {
        const result = await invokeBackendCommand<CloudflareEnvDryRunResult>(
          "cloudflare_env_dry_run",
          { workerName, varName, newValue },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        return result;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error
            ? e.message
            : "Cloudflare env dry run failed.",
        );
        return null;
      }
    },
    [],
  );

  const cloudflareSetWorkerEnv = useCallback(
    async (workerName: string, varName: string, newValue: string) => {
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        const result = await invokeBackendCommand<CloudflareEnvWriteResult>(
          "cloudflare_set_worker_env",
          { workerName, varName, newValue },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        const snapshot = await invokeBackendCommand<CloudDashboardSnapshot>(
          "get_cloud_dashboard_snapshot",
        );
        if (requestEpoch !== authEpochRef.current) return null;
        applyCloudSnapshot(snapshot, requestEpoch);
        return result;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error
            ? e.message
            : "Cloudflare Worker env write failed.",
        );
        return null;
      } finally {
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [applyCloudSnapshot],
  );

  const fetchCloudflareBilling = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    // Read-only: do not wipe an unrelated in-flight error banner.
    try {
      const result = await invokeBackendCommand<CloudflareBilling>(
        "fetch_cloudflare_billing",
      );
      if (requestEpoch !== authEpochRef.current) return null;
      return result;
    } catch (e) {
      if (requestEpoch !== authEpochRef.current) return null;
      setError(
        e instanceof Error
          ? e.message
          : "Cloudflare billing could not be loaded.",
      );
      return null;
    }
  }, []);

  const fetchScalewayBilling = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    // Read-only: do not wipe an unrelated in-flight error banner.
    try {
      const result = await invokeBackendCommand<ScalewayBilling>(
        "fetch_scaleway_billing",
      );
      if (requestEpoch !== authEpochRef.current) return null;
      return result;
    } catch (e) {
      if (requestEpoch !== authEpochRef.current) return null;
      setError(
        e instanceof Error
          ? e.message
          : "Scaleway billing could not be loaded.",
      );
      return null;
    }
  }, []);

  // --- Phase 4 per-type safe-edit actions ---------------------------------
  // READ methods: epoch-guarded, do NOT hold the global isLoading and do NOT
  // call setError(null) (consistent with the read-only Cloudflare fetches
  // above). WRITE/trigger methods follow the cloudflareSetWorkerEnv pattern
  // (setIsLoading + setError(null) + epoch guard; refresh the snapshot on
  // success so inventory counts/state stay coherent).

  const fetchCloudflareAiGatewaySettings = useCallback(
    async (gatewayId: string) => {
      const requestEpoch = authEpochRef.current;
      try {
        const result =
          await invokeBackendCommand<CloudflareAiGatewaySettings>(
            "fetch_cloudflare_ai_gateway_settings",
            { gatewayId },
          );
        if (requestEpoch !== authEpochRef.current) return null;
        return result;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error
            ? e.message
            : "Cloudflare AI Gateway settings could not be loaded.",
        );
        return null;
      }
    },
    [],
  );

  const setCloudflareAiGatewaySettings = useCallback(
    async (gatewayId: string, patch: CloudflareAiGatewaySettingsPatch) => {
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        // Rust param is `settings`; send only the changed fields in the patch.
        const result =
          await invokeBackendCommand<CloudflareAiGatewaySettings>(
            "set_cloudflare_ai_gateway_settings",
            { gatewayId, settings: patch },
          );
        if (requestEpoch !== authEpochRef.current) return null;
        return result;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error
            ? e.message
            : "Cloudflare AI Gateway settings write failed.",
        );
        return null;
      } finally {
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [],
  );

  const cloudflareAutoragReindex = useCallback(async (instanceId: string) => {
    const requestEpoch = authEpochRef.current;
    setIsLoading(true);
    setError(null);
    try {
      const result =
        await invokeBackendCommand<CloudflareAutoragReindexResult>(
          "cloudflare_autorag_reindex",
          { instanceId },
        );
      if (requestEpoch !== authEpochRef.current) return null;
      return result;
    } catch (e) {
      if (requestEpoch !== authEpochRef.current) return null;
      setError(
        e instanceof Error
          ? e.message
          : "Cloudflare AutoRAG reindex could not be triggered.",
      );
      return null;
    } finally {
      if (requestEpoch === authEpochRef.current) setIsLoading(false);
    }
  }, []);

  const fetchCloudflareKvKeys = useCallback(
    async (namespaceId: string, prefix?: string) => {
      const requestEpoch = authEpochRef.current;
      try {
        const result = await invokeBackendCommand<CloudflareKvKeysPage>(
          "fetch_cloudflare_kv_keys",
          { namespaceId, prefix: prefix && prefix.length ? prefix : null },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        return result;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error
            ? e.message
            : "Cloudflare KV keys could not be loaded.",
        );
        return null;
      }
    },
    [],
  );

  const fetchCloudflareKvValue = useCallback(
    async (namespaceId: string, key: string) => {
      const requestEpoch = authEpochRef.current;
      try {
        const result = await invokeBackendCommand<CloudflareKvValue>(
          "fetch_cloudflare_kv_value",
          { namespaceId, key },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        return result;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error
            ? e.message
            : "Cloudflare KV value could not be loaded.",
        );
        return null;
      }
    },
    [],
  );

  const setCloudflareKvValue = useCallback(
    async (namespaceId: string, key: string, value: string) => {
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        const result = await invokeBackendCommand<CloudflareKvWriteResult>(
          "set_cloudflare_kv_value",
          { namespaceId, key, value },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        return result;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error ? e.message : "Cloudflare KV value write failed.",
        );
        return null;
      } finally {
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [],
  );

  const deleteCloudflareKvValue = useCallback(
    async (namespaceId: string, key: string, confirmKey: string) => {
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        const result = await invokeBackendCommand<CloudflareKvWriteResult>(
          "delete_cloudflare_kv_value",
          { namespaceId, key, confirmKey },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        return result;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error
            ? e.message
            : "Cloudflare KV value delete failed.",
        );
        return null;
      } finally {
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [],
  );

  const cloudflareD1Query = useCallback(
    async (databaseId: string, sql: string, confirm: boolean) => {
      const requestEpoch = authEpochRef.current;
      // Only a real mutation (confirm=true) holds the global isLoading and
      // clears the error banner. A SELECT or a confirm=false write probe is
      // non-mutating from the app's perspective — the D1 panel's local
      // `running` state already covers its progress, so leave the global
      // isLoading/error untouched for those.
      if (confirm) {
        setIsLoading(true);
        setError(null);
      }
      try {
        const result = await invokeBackendCommand<CloudflareD1QueryResult>(
          "cloudflare_d1_query",
          { databaseId, sql, confirm },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        return result;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        if (confirm) {
          setError(
            e instanceof Error ? e.message : "Cloudflare D1 query failed.",
          );
        }
        return null;
      } finally {
        if (confirm && requestEpoch === authEpochRef.current) {
          setIsLoading(false);
        }
      }
    },
    [],
  );

  const fetchCloudflareR2Config = useCallback(async (bucket: string) => {
    const requestEpoch = authEpochRef.current;
    try {
      const result = await invokeBackendCommand<CloudflareR2Config>(
        "fetch_cloudflare_r2_config",
        { bucket },
      );
      if (requestEpoch !== authEpochRef.current) return null;
      return result;
    } catch (e) {
      if (requestEpoch !== authEpochRef.current) return null;
      setError(
        e instanceof Error
          ? e.message
          : "Cloudflare R2 config could not be loaded.",
      );
      return null;
    }
  }, []);

  const setCloudflareR2Lifecycle = useCallback(
    async (bucket: string, rules: unknown) => {
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        const result = await invokeBackendCommand<CloudflareR2WriteResult>(
          "set_cloudflare_r2_lifecycle",
          { bucket, rules },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        return result;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error
            ? e.message
            : "Cloudflare R2 lifecycle write failed.",
        );
        return null;
      } finally {
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [],
  );

  const setCloudflareR2Cors = useCallback(
    async (bucket: string, rules: unknown) => {
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        const result = await invokeBackendCommand<CloudflareR2WriteResult>(
          "set_cloudflare_r2_cors",
          { bucket, rules },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        return result;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error ? e.message : "Cloudflare R2 CORS write failed.",
        );
        return null;
      } finally {
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [],
  );

  const performScalewayResourceAction = useCallback(
    async (
      resourceId: string,
      action: ScalewayResourceAction,
      confirmResourceName?: string | null,
    ) => {
      // Single-flight: surface a retry hint rather than dropping the click.
      if (cloudRefreshInFlightRef.current) {
        setError("A Scaleway sync is in progress — wait for it to finish, then retry.");
        return null;
      }
      cloudRefreshInFlightRef.current = true;
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        const result = await invokeBackendCommand<ScalewayActionResult>(
          "perform_scaleway_resource_action",
          {
            resourceId,
            action,
            confirmResourceName: confirmResourceName ?? null,
          },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        try {
          const snapshot = await invokeBackendCommand<CloudDashboardSnapshot>(
            "sync_provider_inventory",
            { provider: "scaleway" },
          );
          if (requestEpoch !== authEpochRef.current) return null;
          applyCloudSnapshot(snapshot, requestEpoch);
        } catch (syncError) {
          if (requestEpoch !== authEpochRef.current) return null;
          setError(
            syncError instanceof Error
              ? `Scaleway action was requested, but refresh failed: ${syncError.message}`
              : "Scaleway action was requested, but refresh failed.",
          );
        }
        scheduleScalewayActionFollowups(requestEpoch);
        return result;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error ? e.message : "Scaleway resource action failed.",
        );
        return null;
      } finally {
        cloudRefreshInFlightRef.current = false;
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [applyCloudSnapshot, scheduleScalewayActionFollowups],
  );

  // Shared mutation runner for the Scaleway P1/P6 create/delete/resize/lifecycle
  // commands. Identical lifecycle to performScalewayResourceAction: it holds the
  // single-flight refresh lock, runs the command, then re-syncs Scaleway so the
  // new/removed resource appears (and schedules the eventual-consistency
  // follow-up syncs). `failMessage` is the generic banner if the call throws.
  // `invoke` performs only the provider command; the snapshot refresh is shared.
  const runScalewayMutation = useCallback(
    async (
      failMessage: string,
      invoke: () => Promise<ScalewayActionResult>,
    ): Promise<ScalewayActionResult | null> => {
      // Single-flight: a sync or another mutation is already running. Surface a
      // retry hint instead of dropping the click silently — otherwise a write
      // (real money) appears to do nothing with no banner. Mirrors the same
      // guard in performScalewayResourceAction.
      if (cloudRefreshInFlightRef.current) {
        setError("A Scaleway sync is in progress — wait for it to finish, then retry.");
        return null;
      }
      cloudRefreshInFlightRef.current = true;
      const requestEpoch = authEpochRef.current;
      setIsLoading(true);
      setError(null);
      try {
        const result = await invoke();
        if (requestEpoch !== authEpochRef.current) return null;
        try {
          const snapshot = await invokeBackendCommand<CloudDashboardSnapshot>(
            "sync_provider_inventory",
            { provider: "scaleway" },
          );
          if (requestEpoch !== authEpochRef.current) return null;
          applyCloudSnapshot(snapshot, requestEpoch);
        } catch (syncError) {
          if (requestEpoch !== authEpochRef.current) return null;
          setError(
            syncError instanceof Error
              ? `Scaleway action was requested, but refresh failed: ${syncError.message}`
              : "Scaleway action was requested, but refresh failed.",
          );
        }
        scheduleScalewayActionFollowups(requestEpoch);
        return result;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(e instanceof Error ? e.message : failMessage);
        return null;
      } finally {
        cloudRefreshInFlightRef.current = false;
        if (requestEpoch === authEpochRef.current) setIsLoading(false);
      }
    },
    [applyCloudSnapshot, scheduleScalewayActionFollowups],
  );

  // READ-ONLY: mirrors the Cloudflare dry-run reads — epoch-guarded, does NOT
  // hold the global isLoading and does NOT clear an unrelated error banner.
  const scalewayInstanceCreateDryRun = useCallback(
    async (request: ScalewayInstanceCreateRequest) => {
      const requestEpoch = authEpochRef.current;
      try {
        const result = await invokeBackendCommand<ScalewayInstanceDryRunResult>(
          "scaleway_instance_create_dry_run",
          { request },
        );
        if (requestEpoch !== authEpochRef.current) return null;
        return result;
      } catch (e) {
        if (requestEpoch !== authEpochRef.current) return null;
        setError(
          e instanceof Error
            ? e.message
            : "Scaleway instance dry run failed.",
        );
        return null;
      }
    },
    [],
  );

  const createScalewayInstance = useCallback(
    (request: ScalewayInstanceCreateRequest) =>
      runScalewayMutation("Scaleway instance create failed.", () =>
        invokeBackendCommand<ScalewayActionResult>("create_scaleway_instance", {
          request,
        }),
      ),
    [runScalewayMutation],
  );

  const createScalewayBlockVolume = useCallback(
    (request: ScalewayBlockVolumeCreateRequest) =>
      runScalewayMutation("Scaleway block volume create failed.", () =>
        invokeBackendCommand<ScalewayActionResult>(
          "create_scaleway_block_volume",
          { request },
        ),
      ),
    [runScalewayMutation],
  );

  const resizeScalewayBlockVolume = useCallback(
    (resourceId: string, newSizeGib: number) =>
      runScalewayMutation("Scaleway block volume resize failed.", () =>
        invokeBackendCommand<ScalewayActionResult>(
          "resize_scaleway_block_volume",
          { resourceId, newSizeGib },
        ),
      ),
    [runScalewayMutation],
  );

  const createScalewayBlockSnapshot = useCallback(
    (volumeId: string, name: string) =>
      runScalewayMutation("Scaleway block snapshot create failed.", () =>
        invokeBackendCommand<ScalewayActionResult>(
          "create_scaleway_block_snapshot",
          { volumeId, name },
        ),
      ),
    [runScalewayMutation],
  );

  const deleteScalewayBlockStorage = useCallback(
    (resourceId: string, confirmResourceName?: string | null) =>
      runScalewayMutation("Scaleway block storage delete failed.", () =>
        invokeBackendCommand<ScalewayActionResult>(
          "delete_scaleway_block_storage",
          { resourceId, confirmResourceName: confirmResourceName ?? null },
        ),
      ),
    [runScalewayMutation],
  );

  const createScalewayFilesystem = useCallback(
    (request: ScalewayFilesystemCreateRequest) =>
      runScalewayMutation("Scaleway filesystem create failed.", () =>
        invokeBackendCommand<ScalewayActionResult>(
          "create_scaleway_filesystem",
          { request },
        ),
      ),
    [runScalewayMutation],
  );

  const deleteScalewayFilesystem = useCallback(
    (resourceId: string, confirmResourceName?: string | null) =>
      runScalewayMutation("Scaleway filesystem delete failed.", () =>
        invokeBackendCommand<ScalewayActionResult>(
          "delete_scaleway_filesystem",
          { resourceId, confirmResourceName: confirmResourceName ?? null },
        ),
      ),
    [runScalewayMutation],
  );

  const createScalewayObjectBucket = useCallback(
    (request: ScalewayObjectBucketCreateRequest) =>
      runScalewayMutation("Scaleway object bucket create failed.", () =>
        invokeBackendCommand<ScalewayActionResult>(
          "create_scaleway_object_bucket",
          { request },
        ),
      ),
    [runScalewayMutation],
  );

  const deleteScalewayObjectBucket = useCallback(
    (resourceId: string, confirmResourceName?: string | null) =>
      runScalewayMutation("Scaleway object bucket delete failed.", () =>
        invokeBackendCommand<ScalewayActionResult>(
          "delete_scaleway_object_bucket",
          { resourceId, confirmResourceName: confirmResourceName ?? null },
        ),
      ),
    [runScalewayMutation],
  );

  const setScalewayObjectBucketLifecycle = useCallback(
    (resourceId: string, rules: unknown) =>
      runScalewayMutation("Scaleway object bucket lifecycle write failed.", () =>
        invokeBackendCommand<ScalewayActionResult>(
          "set_scaleway_object_bucket_lifecycle",
          { resourceId, rules },
        ),
      ),
    [runScalewayMutation],
  );

  const createScalewaySqlDatabase = useCallback(
    (request: ScalewaySqlDatabaseCreateRequest) =>
      runScalewayMutation("Scaleway SQL database create failed.", () =>
        invokeBackendCommand<ScalewayActionResult>(
          "create_scaleway_sql_database",
          { request },
        ),
      ),
    [runScalewayMutation],
  );

  const deleteScalewaySqlDatabase = useCallback(
    (resourceId: string, confirmResourceName?: string | null) =>
      runScalewayMutation("Scaleway SQL database delete failed.", () =>
        invokeBackendCommand<ScalewayActionResult>(
          "delete_scaleway_sql_database",
          { resourceId, confirmResourceName: confirmResourceName ?? null },
        ),
      ),
    [runScalewayMutation],
  );

  const createScalewayFunction = useCallback(
    (request: ScalewayFunctionCreateRequest) =>
      runScalewayMutation("Scaleway function create failed.", () =>
        invokeBackendCommand<ScalewayActionResult>("create_scaleway_function", {
          request,
        }),
      ),
    [runScalewayMutation],
  );

  const deleteScalewayFunction = useCallback(
    (resourceId: string, confirmResourceName?: string | null) =>
      runScalewayMutation("Scaleway function delete failed.", () =>
        invokeBackendCommand<ScalewayActionResult>("delete_scaleway_function", {
          resourceId,
          confirmResourceName: confirmResourceName ?? null,
        }),
      ),
    [runScalewayMutation],
  );

  const createScalewayContainer = useCallback(
    (request: ScalewayContainerCreateRequest) =>
      runScalewayMutation("Scaleway container create failed.", () =>
        invokeBackendCommand<ScalewayActionResult>("create_scaleway_container", {
          request,
        }),
      ),
    [runScalewayMutation],
  );

  const deleteScalewayContainer = useCallback(
    (resourceId: string, confirmResourceName?: string | null) =>
      runScalewayMutation("Scaleway container delete failed.", () =>
        invokeBackendCommand<ScalewayActionResult>("delete_scaleway_container", {
          resourceId,
          confirmResourceName: confirmResourceName ?? null,
        }),
      ),
    [runScalewayMutation],
  );

  const unlock = useCallback(async () => {
    if (unlockInFlightRef.current || unlockRetryBlockedRef.current) return;
    unlockInFlightRef.current = true;
    setIsLoading(true);
    setError(null);
    try {
      if (!isDesktopRuntime) {
        throw new Error("Open the desktop app to use device authentication.");
      }
      const next = await invokeBackendCommand<AuthState>("request_unlock", {
        reason: "Unlock Devboule",
      });
      if (!next.locked) {
        authEpochRef.current += 1;
        clearUnlockRetryCooldown();
        setLockActiveAgentsNotice(null);
      }
      applyAuthState(next);
      if (next.locked) {
        startUnlockRetryCooldown();
        setError("Device authentication did not unlock the app.");
      }
    } catch (e) {
      setIsLocked(true);
      startUnlockRetryCooldown();
      setError(e instanceof Error ? e.message : "Device authentication failed.");
    } finally {
      unlockInFlightRef.current = false;
      setIsLoading(false);
    }
  }, [
    applyAuthState,
    clearUnlockRetryCooldown,
    isDesktopRuntime,
    startUnlockRetryCooldown,
  ]);

  const lock = useCallback(async () => {
    if (unlockInFlightRef.current) return;

    // Soft lock: agents keep running. Snapshot live count BEFORE lock_app so we
    // can warn on the lock screen (get_agent_live_state requires unlock).
    let notice: string | null = null;
    try {
      const live = await invokeBackendCommand<AgentLiveState>(
        "get_agent_live_state",
      );
      notice = softLockActiveAgentsNotice(
        countLiveAgentSessions(live.sessions ?? []),
      );
    } catch {
      const cached = useAgentAttentionStore.getState().sessions;
      notice = softLockActiveAgentsNotice(countLiveAgentSessions(cached));
    }
    setLockActiveAgentsNotice(notice);

    authEpochRef.current += 1;
    setIsLocked(true);
    clearSensitiveState();
    try {
      const next = await invokeBackendCommand<AuthState>("lock_app");
      applyAuthState(next);
    } catch {
      setAuthState((prev) =>
        prev ? { ...prev, locked: true, lockReason: "manual" } : prev,
      );
    }
  }, [applyAuthState, clearSensitiveState]);

  const refreshConfig = useCallback(async () => {
    const requestEpoch = authEpochRef.current;
    setIsLoading(true);
    setError(null);
    try {
      const data = await fetchConfig();
      // A lock/unlock mid-fetch bumps the epoch; don't write stale config or a
      // stale error banner into a freshly re-authenticated session.
      if (requestEpoch === authEpochRef.current) setConfig(data);
    } catch (e) {
      if (requestEpoch === authEpochRef.current) {
        setError(e instanceof Error ? e.message : "Failed to load config");
      }
      // Re-throw so the post-unlock Promise.allSettled batch records the failure.
      throw e;
    } finally {
      // Never strand the global loading state on an epoch change (clearSensitiveState
      // already reset it for the new session).
      if (requestEpoch === authEpochRef.current) setIsLoading(false);
    }
  }, []);

  // Navigate to a view and stage an optional sub-tab for it to consume. The ref
  // mirrors state so consumePendingTab() reads the freshest value even if the
  // consuming view mounts in the same tick as the navigation.
  //
  // Legacy-redirect guard: mapLegacyViewTarget transparently maps any removed
  // view id to its canonical replacement, so persisted/hand-built deep-links
  // land correctly without per-call awareness. The standalone "oracle" view was
  // RESTORED, so it now passes through unchanged (it is a real view again).
  const requestView = useCallback((view: string, tab: string | null = null) => {
    const mapped = mapLegacyViewTarget(view, tab);
    pendingTabRef.current = mapped.tab;
    setPendingTab(mapped.tab);
    setActiveView(mapped.view);
  }, []);

  const consumePendingTab = useCallback(() => {
    const tab = pendingTabRef.current;
    if (tab !== null) {
      pendingTabRef.current = null;
      setPendingTab(null);
    }
    return tab;
  }, []);

  useEffect(() => {
    refreshAuthState();
  }, [refreshAuthState]);

  useEffect(() => {
    if (isLocked) return;
    void (async () => {
      const requestEpoch = authEpochRef.current;
      let firstFailure: unknown = null;
      const recordFailure = (reason: unknown) => {
        firstFailure ??= reason;
      };

      const results = await Promise.allSettled([
        refreshConfig(),
        refreshCloudDashboard(),
        refreshSecretStatuses(),
        refreshProviderScopeStatuses(),
        refreshScalewayObjectAccessKeyStatus(),
        refreshScalewayObjectSecretKeyStatus(),
        refreshCloudflareAgentTokenProfiles(),
      ]);
      if (requestEpoch !== authEpochRef.current) return;
      results.forEach((result) => {
        if (result.status === "rejected") recordFailure(result.reason);
      });

      for (const refreshOracleStep of [
        refreshOracleIndexPreferences,
        refreshOracleSnapshot,
        refreshOracleCoverage,
        refreshOracleRuntime,
        refreshOracleLlmSettings,
        refreshOracleIndexStatus,
      ]) {
        if (requestEpoch !== authEpochRef.current) return;
        try {
          await refreshOracleStep();
        } catch (e) {
          // Oracle readiness (not installed / not indexed yet / mid-index /
          // partial index) is a NORMAL state surfaced on the Oracle page + the
          // health strip + the doctor. Since Step 1 these steps THROW a typed
          // OracleError instead of silently degrading — correct, but it must NOT
          // turn the whole post-unlock boot into a global "Post-unlock refresh
          // failed." error and block the rest of the app. Keep Oracle refresh
          // failures non-fatal here; the Oracle UI shows the real status.
          console.warn("Oracle post-unlock refresh step failed (non-fatal):", e);
        }
      }

      if (requestEpoch !== authEpochRef.current) return;
      if (firstFailure) {
        setError(
          firstFailure instanceof Error
            ? firstFailure.message
            : "Post-unlock refresh failed.",
        );
      }
    })();
  }, [
    isLocked,
    refreshConfig,
    refreshCloudDashboard,
    refreshOracleSnapshot,
    refreshOracleCoverage,
    refreshOracleRuntime,
    refreshOracleLlmSettings,
    refreshOracleIndexPreferences,
    refreshOracleIndexStatus,
    refreshSecretStatuses,
    refreshProviderScopeStatuses,
    refreshScalewayObjectAccessKeyStatus,
    refreshScalewayObjectSecretKeyStatus,
    refreshCloudflareAgentTokenProfiles,
  ]);

  useEffect(() => {
    if (isLocked) return;
    let timer: number | null = null;
    let cancelled = false;
    const tick = () => {
      if (cancelled) return;
      // Only hit the backend when the window is visible. refreshCloudDashboard
      // has its own in-flight guard (cloudRefreshInFlightRef), so a slow invoke
      // cannot stack here.
      if (document.visibilityState === "visible") {
        void refreshCloudDashboard();
      }
      timer = window.setTimeout(tick, LIVE_SYNC_INTERVAL_MS);
    };
    timer = window.setTimeout(tick, LIVE_SYNC_INTERVAL_MS);
    return () => {
      cancelled = true;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [isLocked, refreshCloudDashboard]);

  useEffect(() => {
    if (
      isLocked ||
      !isDesktopRuntime ||
      !oracleIndexPreferences?.autoWatchOnUnlock
    )
      return;
    const requestEpoch = authEpochRef.current;
    if (autoWatchAttemptedEpochRef.current === requestEpoch) return;
    autoWatchAttemptedEpochRef.current = requestEpoch;
    void startOracleIndexWatcher();
  }, [
    isDesktopRuntime,
    isLocked,
    oracleIndexPreferences?.autoWatchOnUnlock,
    startOracleIndexWatcher,
  ]);

  useEffect(() => {
    if (isLocked || !isDesktopRuntime) return;
    // Auto-lock when the window stays HIDDEN for a grace period — NOT instantly
    // on every visibilitychange. The old instant lock fired on momentary macOS
    // Space switches, a window briefly covered, and the dev-rebuild window
    // flash, locking the user out constantly ("ogni tanto torna in lock").
    return installVisibilityLock(() => {
      if (!unlockInFlightRef.current) void lock();
    }, VISIBILITY_LOCK_GRACE_MS);
  }, [isDesktopRuntime, isLocked, lock]);

  // Soft-lock idle TTL tracks USER activity only. Background pollers call
  // ensure_unlocked and must not refresh the clock (that was the security hole).
  // pointerdown/keydown are genuine interaction; throttle to at most one IPC/min.
  useEffect(() => {
    if (isLocked || !isDesktopRuntime) return;
    let lastTouchMs = 0;
    const touch = () => {
      const now = Date.now();
      if (now - lastTouchMs < IDLE_ACTIVITY_TOUCH_THROTTLE_MS) return;
      lastTouchMs = now;
      void invokeBackendCommand("touch_idle_activity").catch((e) => {
        // If the backend already expired idle, surface lock immediately instead
        // of waiting for the next LIVE_SYNC poll (~60s UI desync).
        const msg = typeof e === "string" ? e : e instanceof Error ? e.message : "";
        if (msg.toLowerCase().includes("app is locked")) {
          void refreshAuthState();
        }
      });
    };
    // Immediate touch on unlock / effect mount so the idle window starts from
    // the moment the user is actively in the app (not only first key/click).
    touch();
    window.addEventListener("pointerdown", touch, true);
    window.addEventListener("keydown", touch, true);
    return () => {
      window.removeEventListener("pointerdown", touch, true);
      window.removeEventListener("keydown", touch, true);
    };
  }, [isDesktopRuntime, isLocked, refreshAuthState]);

  useEffect(
    () => () => {
      if (unlockRetryTimerRef.current !== null) {
        window.clearTimeout(unlockRetryTimerRef.current);
        unlockRetryTimerRef.current = null;
      }
      scalewayFollowupTimersRef.current.forEach((id) =>
        window.clearTimeout(id),
      );
      scalewayFollowupTimersRef.current = [];
    },
    [],
  );

  // All actions are useCallback-stable, so this object is created once and
  // keeps a stable identity. Consumers of useAppActions() never re-render on
  // state changes.
  const actions = useMemo<AppActions>(
    () => ({
      setActiveView,
      requestView,
      consumePendingTab,
      refreshConfig,
      refreshCloudDashboard,
      refreshOracleSnapshot,
      refreshOracleCoverage,
      refreshOracleRuntime,
      refreshOracleLlmSettings,
      saveOracleLlmSettings,
      deleteOracleLlmApiKey,
      refreshOracleIndexPreferences,
      saveOracleIndexPreferences,
      refreshOracleIndexStatus,
      syncOracleTextChunks,
      startOracleIndexJob,
      startOracleIndexWatcher,
      stopOracleIndexWatcher,
      askOracle,
      getOracleNode,
      getOracleSimilar,
      getOracleDuplicates,
      getOracleDoctor,
      getOracleIndexedFiles,
      configureCliAgents,
      cliAgentsStatus,
      unconfigureCliAgents,
      refreshSecretStatuses,
      refreshProviderScopeStatuses,
      refreshScalewayObjectAccessKeyStatus,
      refreshScalewayObjectSecretKeyStatus,
      refreshCloudflareAgentTokenProfiles,
      saveScalewayObjectAccessKey,
      deleteScalewayObjectAccessKey,
      saveScalewayObjectSecretKey,
      deleteScalewayObjectSecretKey,
      saveProviderScope,
      deleteProviderScope,
      auditProviderConnection,
      auditSavedProviderConnection,
      syncProviderInventory,
      saveProviderToken,
      deleteProviderToken,
      saveCloudflareAgentTokenProfile,
      deleteCloudflareAgentTokenProfile,
      rotateCloudflareWorkerSecret,
      runCloudflareSmokeDryRun,
      fetchCloudflareWorkerSettings,
      cloudflareEnvDryRun,
      cloudflareSetWorkerEnv,
      fetchCloudflareBilling,
      fetchScalewayBilling,
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
      performScalewayResourceAction,
      scalewayInstanceCreateDryRun,
      createScalewayInstance,
      createScalewayBlockVolume,
      resizeScalewayBlockVolume,
      createScalewayBlockSnapshot,
      deleteScalewayBlockStorage,
      createScalewayFilesystem,
      deleteScalewayFilesystem,
      createScalewayObjectBucket,
      deleteScalewayObjectBucket,
      setScalewayObjectBucketLifecycle,
      createScalewaySqlDatabase,
      deleteScalewaySqlDatabase,
      createScalewayFunction,
      deleteScalewayFunction,
      createScalewayContainer,
      deleteScalewayContainer,
      unlock,
      lock,
      refreshRole: fetchRole,
    }),
    [
      setActiveView,
      requestView,
      consumePendingTab,
      refreshConfig,
      refreshCloudDashboard,
      refreshOracleSnapshot,
      refreshOracleCoverage,
      refreshOracleRuntime,
      refreshOracleLlmSettings,
      saveOracleLlmSettings,
      deleteOracleLlmApiKey,
      refreshOracleIndexPreferences,
      saveOracleIndexPreferences,
      refreshOracleIndexStatus,
      syncOracleTextChunks,
      startOracleIndexJob,
      startOracleIndexWatcher,
      stopOracleIndexWatcher,
      askOracle,
      getOracleNode,
      getOracleSimilar,
      getOracleDuplicates,
      getOracleDoctor,
      getOracleIndexedFiles,
      configureCliAgents,
      cliAgentsStatus,
      unconfigureCliAgents,
      refreshSecretStatuses,
      refreshProviderScopeStatuses,
      refreshScalewayObjectAccessKeyStatus,
      refreshScalewayObjectSecretKeyStatus,
      refreshCloudflareAgentTokenProfiles,
      saveScalewayObjectAccessKey,
      deleteScalewayObjectAccessKey,
      saveScalewayObjectSecretKey,
      deleteScalewayObjectSecretKey,
      saveProviderScope,
      deleteProviderScope,
      auditProviderConnection,
      auditSavedProviderConnection,
      syncProviderInventory,
      saveProviderToken,
      deleteProviderToken,
      saveCloudflareAgentTokenProfile,
      deleteCloudflareAgentTokenProfile,
      rotateCloudflareWorkerSecret,
      runCloudflareSmokeDryRun,
      fetchCloudflareWorkerSettings,
      cloudflareEnvDryRun,
      cloudflareSetWorkerEnv,
      fetchCloudflareBilling,
      fetchScalewayBilling,
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
      performScalewayResourceAction,
      scalewayInstanceCreateDryRun,
      createScalewayInstance,
      createScalewayBlockVolume,
      resizeScalewayBlockVolume,
      createScalewayBlockSnapshot,
      deleteScalewayBlockStorage,
      createScalewayFilesystem,
      deleteScalewayFilesystem,
      createScalewayObjectBucket,
      deleteScalewayObjectBucket,
      setScalewayObjectBucketLifecycle,
      createScalewaySqlDatabase,
      deleteScalewaySqlDatabase,
      createScalewayFunction,
      deleteScalewayFunction,
      createScalewayContainer,
      deleteScalewayContainer,
      unlock,
      lock,
      fetchRole,
    ],
  );

  // Full state + actions value for the backward-compatible useAppContext().
  // Memoized so consumers only re-render when a referenced field actually
  // changes identity (instead of every render of the provider).
  const value = useMemo<AppContextValue>(
    () => ({
      config,
      activeView,
      pendingTab,
      isLoading,
      unlockRetryBlocked,
      error,
      isDesktopRuntime,
      isLocked,
      lockActiveAgentsNotice,
      authState,
      cloudSnapshot,
      oracleSnapshot,
      oracleCoverage,
      oracleRuntime,
      oracleLlmSettings,
      oracleIndexPreferences,
      oracleIndexStatus,
      secretStatuses,
      providerScopeStatuses,
      scalewayObjectAccessKeyStatus,
      scalewayObjectSecretKeyStatus,
      cloudflareAgentTokenProfiles,
      roleStatus,
      ...actions,
    }),
    [
      config,
      activeView,
      pendingTab,
      isLoading,
      unlockRetryBlocked,
      error,
      isDesktopRuntime,
      isLocked,
      lockActiveAgentsNotice,
      authState,
      cloudSnapshot,
      oracleSnapshot,
      oracleCoverage,
      oracleRuntime,
      oracleLlmSettings,
      oracleIndexPreferences,
      oracleIndexStatus,
      secretStatuses,
      providerScopeStatuses,
      scalewayObjectAccessKeyStatus,
      scalewayObjectSecretKeyStatus,
      cloudflareAgentTokenProfiles,
      roleStatus,
      actions,
    ],
  );

  return (
    <AppActionsContext.Provider value={actions}>
      <AppContext.Provider value={value}>{children}</AppContext.Provider>
    </AppActionsContext.Provider>
  );
}

export function useAppContext(): AppContextValue {
  const ctx = useContext(AppContext);
  if (!ctx) throw new Error("useAppContext must be used within AppProvider");
  return ctx;
}

// Subscribe only to the stable actions object. Consumers using this hook do
// NOT re-render when app state changes — use it for action-only consumers
// (Sidebar nav, Header lock button, dashboard table action buttons).
export function useAppActions(): AppActions {
  const ctx = useContext(AppActionsContext);
  if (!ctx) throw new Error("useAppActions must be used within AppProvider");
  return ctx;
}
