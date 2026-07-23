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

const EMPTY_CONFIG: AppConfig = {
  project: { name: "Devboule", version: "" },
  // Compressed top-level nav (Polis is injected by Sidebar). Re-homed pages now
  // live as tabs: Agents→Projects; Secrets/Devices/Workspace→Settings
  // (opened from the user area).
  navigation: [
    { id: "projects", label: "Projects", icon: "FolderKanban" },
    { id: "oracle", label: "Oracle", icon: "BrainCircuit" },
  ],
  providers: [],
  bookmarks: [],
  secrets: [],
  compute: {
    gpus: { active: 0, total: 0, provider: "" },
    cpus: { active: 0, total: 0, provider: "" },
    workers: { active: 0, total: 0, provider: "" },
  },
  budget: { monthly_limit: 0, currency: "EUR", categories: [] },
  customAgentClients: [],
};

interface AppState {
  config: AppConfig;
  activeView: string;
  // The sub-tab a deep-link asked the active view to open (e.g. "secrets"
  // inside Settings). Null when none requested.
  pendingTab: string | null;
  isLoading: boolean;
  unlockRetryBlocked: boolean;
  error: string | null;
  isDesktopRuntime: boolean;
  isLocked: boolean;
  /** Soft-lock: copy about agents still running when the vault locked (null if none). */
  lockActiveAgentsNotice: string | null;
  authState: AuthState | null;
  oracleSnapshot: OracleSnapshot | null;
  oracleCoverage: OracleCoverage | null;
  oracleRuntime: OracleRuntime | null;
  oracleLlmSettings: OracleLlmSettingsStatus | null;
  oracleIndexPreferences: OracleIndexPreferences | null;
  oracleIndexStatus: OracleIndexStatus | null;
  // Verified local role + onboarding signal (null while locked/loading).
  roleStatus: LocalRoleStatus | null;
}

interface AppActions {
  setActiveView: (view: string) => void;
  // Navigate to a view and optionally request an inner sub-tab. The target view
  // reads (and clears) the requested tab via consumePendingTab(). Used by the
  // sidebar and jump-search deep-links.
  requestView: (view: string, tab?: string | null) => void;
  // Read-and-clear the pending sub-tab. A view calls this on mount; returns the
  // requested tab once, then null so a later re-render does not re-hijack the
  // user's manual tab choice.
  consumePendingTab: () => string | null;
  refreshConfig: () => Promise<void>;
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

/** Cached Tauri `invoke` after first successful dynamic import. Hot polls
 *  (oracle status, agent refresh, idle touch) must not re-import every call. */
let cachedInvoke:
  | (<T>(command: string, args?: Record<string, unknown>) => Promise<T>)
  | null = null;
let invokeImportPromise: Promise<
  <T>(command: string, args?: Record<string, unknown>) => Promise<T>
> | null = null;

async function getTauriInvoke(): Promise<
  <T>(command: string, args?: Record<string, unknown>) => Promise<T>
> {
  if (cachedInvoke) return cachedInvoke;
  if (!invokeImportPromise) {
    invokeImportPromise = import("@tauri-apps/api/core")
      .then((m) => {
        const fn = m.invoke as <T>(
          command: string,
          args?: Record<string, unknown>,
        ) => Promise<T>;
        cachedInvoke = fn;
        return fn;
      })
      .catch((e) => {
        // Do not cache a rejected import forever — clear it so a later call can retry.
        invokeImportPromise = null;
        throw e;
      });
  }
  return invokeImportPromise;
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
  const invoke = await getTauriInvoke();
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
  const [roleStatus, setRoleStatus] = useState<LocalRoleStatus | null>(null);
  const authEpochRef = useRef(0);
  const unlockInFlightRef = useRef(false);
  const unlockRetryBlockedRef = useRef(false);
  const unlockRetryTimerRef = useRef<number | null>(null);
  const autoWatchAttemptedEpochRef = useRef<number | null>(null);

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
    // A lock bumps the auth epoch, so any in-flight write's `finally` will skip
    // its `setIsLoading(false)` (epoch mismatch) and strand the global loading
    // state. Clear it here at the source so a lock mid-write never hangs the UI.
    setIsLoading(false);
    setOracleSnapshot(null);
    setOracleCoverage(null);
    setOracleRuntime(null);
    setOracleLlmSettings(null);
    setOracleIndexPreferences(null);
    setOracleIndexStatus(null);
    setRoleStatus(null);
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

  const refreshAuthState = useCallback(async () => {
    if (unlockInFlightRef.current) return;
    if (!isDesktopRuntime) {
      setIsLocked(true);
      setAuthState({
        locked: true,
        helloAvailable: false,
        lastUnlockedAt: null,
        lockReason: "unavailable",
        devUnlock: false,
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
        devUnlock: false,
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
    const requestEpoch = authEpochRef.current;
    try {
      const status = await invokeBackendCommand<OracleIndexStatus>(
        "get_oracle_index_status",
      );
      if (requestEpoch !== authEpochRef.current) return;
      setOracleIndexStatus(status);
    } catch {
      // HF1-6: same epoch guard as success — a stale failed request must not
      // null a fresh session's status after lock/unlock.
      if (requestEpoch !== authEpochRef.current) return;
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
    refreshOracleSnapshot,
    refreshOracleCoverage,
    refreshOracleRuntime,
    refreshOracleLlmSettings,
    refreshOracleIndexPreferences,
    refreshOracleIndexStatus,
  ]);


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
    // DEV unlock sessions (pilot automation, overnight agent drives) keep the
    // window hidden for long stretches — installing visibility auto-lock would
    // fight that. Backend already promises "no idle soft-lock" in that mode.
    if (authState?.devUnlock) return;
    // Auto-lock when the window stays HIDDEN for a grace period — NOT instantly
    // on every visibilitychange. The old instant lock fired on momentary macOS
    // Space switches, a window briefly covered, and the dev-rebuild window
    // flash, locking the user out constantly ("ogni tanto torna in lock").
    return installVisibilityLock(() => {
      if (!unlockInFlightRef.current) void lock();
    }, VISIBILITY_LOCK_GRACE_MS);
  }, [authState?.devUnlock, isDesktopRuntime, isLocked, lock]);

  // Soft-lock idle TTL tracks USER activity only. Background pollers call
  // ensure_unlocked and must not refresh the clock (that was the security hole).
  // pointerdown/keydown are genuine interaction; throttle to at most one IPC/min.
  // Backend `expire_if_needed` already never idles out under DEVBOULE_DEV_UNLOCK,
  // so this touch channel is left installed even in dev (harmless no-op for lock).
  useEffect(() => {
    if (isLocked || !isDesktopRuntime) return;
    let lastTouchMs = 0;
    const touch = () => {
      const now = Date.now();
      if (now - lastTouchMs < IDLE_ACTIVITY_TOUCH_THROTTLE_MS) return;
      lastTouchMs = now;
      void invokeBackendCommand("touch_idle_activity").catch((e) => {
        // If the backend already expired idle, surface lock immediately instead
        // of waiting for the next auth refresh.
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
      unlock,
      lock,
      refreshRole: fetchRole,
    }),
    [
      setActiveView,
      requestView,
      consumePendingTab,
      refreshConfig,
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
      oracleSnapshot,
      oracleCoverage,
      oracleRuntime,
      oracleLlmSettings,
      oracleIndexPreferences,
      oracleIndexStatus,
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
      oracleSnapshot,
      oracleCoverage,
      oracleRuntime,
      oracleLlmSettings,
      oracleIndexPreferences,
      oracleIndexStatus,
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
