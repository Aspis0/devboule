import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { CollapsibleSection } from "../views/CollapsibleSection";
import {
  AlertTriangle,
  BrainCircuit,
  Clock,
  Eye,
  Files,
  FolderOpen,
  Loader2,
  Play,
  RefreshCw,
  Search,
  Snowflake,
  Stethoscope,
  StopCircle,
} from "lucide-react";
import { invokeBackendCommand, useAppContext } from "../../context/AppContext";
import { toOracleError } from "../../utils/oracleError";
import { oracleIndexPhaseHint } from "../../utils/oracleIndexPhase";
import {
  estimateOracleIndexEta,
  normalizeIndexPhase,
  type OracleIndexProgressSample,
} from "../../utils/oracleIndexEta";
import { classifyRuntimeStage } from "../../utils/oracleRuntimeState";
import { OracleDoctorPanel } from "../views/OracleDoctorPanel";
import { OracleAnswerSettingsCard } from "../settings/OracleAnswerSettingsCard";
import { CliAgentsCard } from "./CliAgentsCard";
import { deriveProviderConfigured } from "./oracleProviderState";
import type {
  OracleDoctorReport,
  OracleError,
  OracleIndexedFile,
  OracleRuntimeSetup,
} from "../../types/backend";

// ---------------------------------------------------------------------------
// Admin constants (travelled WITH the admin surface out of OracleView).
// ---------------------------------------------------------------------------

// The fixed doctor checks, in the order the health strip renders their dots.
// Mirrors the backend OracleDoctorReport check ids.
const DOCTOR_CHECK_ORDER = [
  "runtime",
  "embedder",
  "workspace",
  "index",
  "live_server",
  "provider",
] as const;

const FILES_PAGE_SIZE = 100;
const FILE_FILTER_DEBOUNCE_MS = 250;
const INDEX_POLL_MS = 3000;
// Stop polling a stuck job after ~5 minutes of continuous polling so a job that
// never leaves "running" does not poll the backend forever.
const INDEX_POLL_MAX_MS = 5 * 60 * 1000;

type FileTab = "indexed" | "pending" | "stale";

// Relative-time formatter for the indexed-files table. Treats "" as unknown and
// NEVER calls new Date("") (which is Invalid Date). Returns a short "2m ago"
// style label; falls back to the raw string if it cannot be parsed.
function formatIndexedAt(value: string): string {
  if (!value) return "unknown";
  const ms = Date.parse(value);
  if (Number.isNaN(ms)) return "unknown";
  const diff = Date.now() - ms;
  if (diff < 0) return "just now";
  const sec = Math.floor(diff / 1000);
  if (sec < 60) return `${sec}s ago`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.floor(hr / 24);
  return `${day}d ago`;
}

// ---------------------------------------------------------------------------
// Oracle ADMIN panel — the operator surface for the local retrieval runtime:
// health strip, runtime setup, the doctor, the workspace / index-root picker,
// the index job-progress polling, the index-preferences card, and the indexed-
// files browser. Self-contained: it owns all the state lifted out of OracleView
// and reads everything it needs from AppContext (no props). Phase 4(b): mounted
// under Settings → Workspace; the human ASK flow lives in OracleAskPanel,
// which PolisBottomBar mounts when panelId === "oracle".
// ---------------------------------------------------------------------------
export function OracleAdminPanel() {
  const {
    oracleRuntime,
    oracleLlmSettings,
    oracleIndexPreferences,
    oracleIndexStatus,
    refreshOracleRuntime,
    refreshOracleLlmSettings,
    refreshOracleIndexStatus,
    saveOracleIndexPreferences,
    startOracleIndexJob,
    startOracleIndexWatcher,
    stopOracleIndexWatcher,
    getOracleIndexedFiles,
    isLoading,
  } = useAppContext();

  const [doctor, setDoctor] = useState<OracleDoctorReport | null>(null);
  const [doctorOpen, setDoctorOpen] = useState(false);

  const [runtimeSetup, setRuntimeSetup] = useState<OracleRuntimeSetup | null>(
    null,
  );
  const [installingRuntime, setInstallingRuntime] = useState(false);
  const [runtimeError, setRuntimeError] = useState<string | null>(null);
  // Download/install progress tracked ACROSS navigation: the backend emits
  // `oracle://install-progress` events from a spawn_blocking task that keeps
  // running even if the user leaves this page. The state survives unmount
  // because it is lifted to a module-level ref, not a local useState.
  const [installProgress, setInstallProgress] = useState<{
    stage: string;
    percent: number;
    message: string;
  } | null>(null);

  const [workspaceKicked, setWorkspaceKicked] = useState(false);
  const [workspaceError, setWorkspaceError] = useState<OracleError | null>(null);

  const [oracleLoad, setOracleLoad] = useState({
    loading: false,
    message: "Oracle not loaded yet.",
    error: null as string | null,
  });
  const oracleLoadSeqRef = useRef(0);
  const mountedRef = useRef(true);

  // True when the index poll saw NO progress for INDEX_POLL_MAX_MS while the
  // job was still active; surfaces a subtle "status may be stale" hint instead
  // of polling forever. A genuinely advancing (but slow) job never trips it.
  const [indexPollStale, setIndexPollStale] = useState(false);
  // Last observed indexed-file count + the time it last ADVANCED — the poll's
  // stall detector resets on real progress so a slow first-index keeps polling.
  const indexProgressRef = useRef<{ count: number; at: number }>({
    count: -1,
    at: 0,
  });
  // Progress samples for the pure ETA estimator (phase-scoped rate). Cleared
  // when the job leaves active; never drives the stall detector.
  const indexEtaSamplesRef = useRef<OracleIndexProgressSample[]>([]);
  // Last remaining-ms shown, so the pure function can smooth; reset on phase
  // change / pause so a prior phase cannot poison the next.
  const indexEtaPrevMsRef = useRef<number | null>(null);
  const indexEtaPhaseRef = useRef<string | null>(null);
  // Display label next to "Indexing… n / N" — null when no ETA surface.
  const [indexEtaLabel, setIndexEtaLabel] = useState<string | null>(null);
  // Synchronous re-entrancy guard for the "Index now" click. The disabled gate
  // is intentionally relaxed when the poll goes stale (so a genuinely stuck job
  // can be retried), but a slow-but-alive single large file can also trip the
  // stale flag — without this guard a double-click would start a SECOND
  // concurrent index job. The ref is flipped synchronously, so a second click
  // is rejected immediately even while the button is momentarily enabled.
  const indexFiringRef = useRef(false);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Listen for install-progress events from the backend. The download runs on
  // a spawn_blocking task that survives navigation, so we keep listening for the
  // lifetime of the component. When the user returns to this page after
  // navigating away, `installProgress` reflects the latest event received.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void (async () => {
      try {
        unlisten = await listen<{
          stage: string;
          percent: number;
          message: string;
        }>("oracle://install-progress", (event) => {
          if (!mountedRef.current) return;
          const { stage, percent, message } = event.payload;
          setInstallProgress({ stage, percent, message });
          if (stage === "done") {
            setInstallingRuntime(false);
          }
        });
      } catch {
        // Tauri event listener unavailable (e.g. in vitest) — silently ignore.
      }
      if (cancelled && unlisten) unlisten();
    })();
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  const loadOraclePage = useCallback(async () => {
    const seq = oracleLoadSeqRef.current + 1;
    oracleLoadSeqRef.current = seq;
    const update = (message: string) => {
      if (oracleLoadSeqRef.current !== seq) return;
      setOracleLoad({ loading: true, message, error: null });
    };
    const warnings: string[] = [];
    const runStep = async (message: string, action: () => Promise<unknown>) => {
      update(message);
      try {
        await action();
      } catch (e) {
        if (!mountedRef.current) return;
        warnings.push(e instanceof Error ? e.message : message);
      }
    };

    // The admin panel only refreshes what it actually renders (the snapshot /
    // coverage data are primed by the AppContext post-unlock boot sequence and
    // consumed elsewhere).
    await runStep("Checking vector runtime...", refreshOracleRuntime);
    if (!mountedRef.current) return;
    await runStep("Loading LLM settings...", refreshOracleLlmSettings);
    if (!mountedRef.current) return;
    await runStep("Loading dense index status...", refreshOracleIndexStatus);
    if (!mountedRef.current) return;

    if (oracleLoadSeqRef.current !== seq) return;
    setOracleLoad({
      loading: false,
      message: "Oracle ready: dense index status loaded.",
      error: warnings.length > 0 ? warnings.join(" | ") : null,
    });
  }, [
    refreshOracleIndexStatus,
    refreshOracleLlmSettings,
    refreshOracleRuntime,
  ]);

  useEffect(() => {
    void loadOraclePage();
  }, [loadOraclePage]);

  // The full doctor is HEAVY: it spawns Python and loads the Qwen3 embedding
  // model (tens of seconds, process-wide lock). It must run ONLY on explicit
  // user action — i.e. when OracleDoctorPanel mounts (opened via "Run doctor").
  // We therefore do NOT auto-run the doctor on page mount. The panel reports its
  // single run back through this callback so the health strip can reflect the
  // precise per-check dots once the user has actually run it this session.
  const handleDoctorReport = useCallback((report: OracleDoctorReport) => {
    if (mountedRef.current) setDoctor(report);
  }, []);

  // Check whether the local retrieval runtime (Python venv + LanceDB + Qwen3
  // embedder) is installed. Non-blocking; failures leave the setup card hidden.
  const loadRuntimeSetup = useCallback(async () => {
    try {
      const setup =
        await invokeBackendCommand<OracleRuntimeSetup>("get_oracle_runtime_setup");
      if (mountedRef.current) setRuntimeSetup(setup);
    } catch {
      if (mountedRef.current) setRuntimeSetup(null);
    }
  }, []);

  useEffect(() => {
    void loadRuntimeSetup();
  }, [loadRuntimeSetup]);

  const installRuntime = useCallback(async () => {
    setInstallingRuntime(true);
    setRuntimeError(null);
    setInstallProgress({ stage: "venv", percent: 0, message: "Starting..." });
    try {
      const setup =
        await invokeBackendCommand<OracleRuntimeSetup>("install_oracle_runtime");
      if (mountedRef.current) setRuntimeSetup(setup);
    } catch (error) {
      if (mountedRef.current) {
        setRuntimeError(error instanceof Error ? error.message : String(error));
      }
    } finally {
      // Only clear installing if we didn't get a "done" event (e.g. the call
      // threw before any event was emitted). The progress listener clears it
      // on "done" so navigating away+back keeps the spinner until completion.
      if (mountedRef.current && installProgress?.stage !== "done") {
        setInstallingRuntime(false);
      }
    }
  }, [installProgress]);

  const index = oracleIndexStatus?.index;
  // Only a non-empty string status is meaningful; anything else (missing key,
  // null, undefined, non-string) falls back to "idle" so the badge never reads
  // the literal "undefined".
  const rawJobStatus = oracleIndexStatus?.job?.status;
  const jobStatus =
    typeof rawJobStatus === "string" && rawJobStatus.length > 0
      ? rawJobStatus
      : "idle";
  const jobActive = jobStatus === "queued" || jobStatus === "running";
  const jobMessage =
    typeof oracleIndexStatus?.job?.message === "string"
      ? (oracleIndexStatus.job.message as string)
      : null;
  // Live sub-state hint: while the job is paused on GPU heat / low RAM it stays
  // "running" but does not progress, so surface a calm "resuming…" line instead
  // of a frozen-looking bar. null when progressing normally.
  const phaseHint = oracleIndexPhaseHint(
    oracleIndexStatus?.job?.phase,
    oracleIndexStatus?.job?.phaseMessage,
  );

  // Let the user pick the Devboule workspace folder. After the preference is
  // saved, AUTOMATICALLY kick a dense index job + the watcher and refresh status
  // so choosing the folder really points Oracle there and starts working. Shows
  // a confirmation line.
  const chooseWorkspaceFolder = useCallback(async () => {
    // 1) Open the folder picker. Only the dialog open is wrapped here: a
    //    dismissed dialog or an unavailable plugin is a silent no-op and must
    //    NOT be reported as a workspace error.
    let picked: string | null = null;
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const result = await open({
        directory: true,
        multiple: false,
        title: "Choose your Devboule workspace folder",
      });
      if (typeof result === "string" && result.trim()) picked = result;
    } catch {
      // Dialog plugin unavailable or user dismissed — no-op.
    }
    if (picked === null) return;

    // 2) Persist the preference + kick the index/watch chain. Backend failures
    //    here MUST surface (not be swallowed) and must NOT mark the workspace as
    //    successfully kicked.
    if (mountedRef.current) setWorkspaceError(null);
    try {
      const saved = await saveOracleIndexPreferences({
        autoWatchOnUnlock: oracleIndexPreferences?.autoWatchOnUnlock ?? true,
        indexRoot: picked,
      });
      if (!saved) return;
      // Point Oracle at the new folder and start working immediately. This is a
      // deliberate user action (they just picked the workspace), so run the full
      // manual index (idle=false, unbounded batches) instead of the opportunistic
      // single-batch warm pass — otherwise a large workspace sits at 0%.
      await startOracleIndexJob(false, 1, false, true);
      await startOracleIndexWatcher();
      await refreshOracleIndexStatus();
      // Do NOT run the heavy doctor here: the index job already exercises the
      // embedder, and a doctor call would fight it for the process-wide lock.
      // Only confirm success AFTER the whole chain resolved.
      if (mountedRef.current) setWorkspaceKicked(true);
    } catch (e) {
      if (mountedRef.current) {
        setWorkspaceKicked(false);
        setWorkspaceError(toOracleError(e));
      }
    }
  }, [
    oracleIndexPreferences,
    saveOracleIndexPreferences,
    startOracleIndexJob,
    startOracleIndexWatcher,
    refreshOracleIndexStatus,
  ]);

  // Reset the stall detector each time a job becomes active: progress is
  // measured from the count at activation, the clock from now.
  useEffect(() => {
    if (jobActive) {
      indexProgressRef.current = {
        count: index?.indexedFiles ?? 0,
        at: Date.now(),
      };
    }
  }, [jobActive]);

  // Whenever the indexed count ADVANCES, stamp the progress time and clear any
  // stale hint — the job is alive, keep polling.
  useEffect(() => {
    const count = index?.indexedFiles ?? 0;
    if (count !== indexProgressRef.current.count) {
      indexProgressRef.current = { count, at: Date.now() };
      setIndexPollStale(false);
    }
  }, [index?.indexedFiles]);

  // Poll index status while a job is queued/running so the progress bar advances;
  // stop as soon as it goes idle/error. Interval is cleaned up on unmount and
  // whenever the active flag flips (no leak, no stale closure on the callback).
  useEffect(() => {
    if (!jobActive) {
      // Job left the active state on its own — drop any stale-poll hint.
      setIndexPollStale(false);
      return;
    }
    setIndexPollStale(false);
    const intervalId = window.setInterval(() => {
      // Stall cap: stop ONLY when the count has not advanced for the whole
      // window (a slow-but-advancing first-index resets `at` via the progress
      // effect above, so it keeps polling). A truly stuck job trips it.
      if (Date.now() - indexProgressRef.current.at >= INDEX_POLL_MAX_MS) {
        window.clearInterval(intervalId);
        setIndexPollStale(true);
        return;
      }
      void refreshOracleIndexStatus();
    }, INDEX_POLL_MS);
    return () => {
      window.clearInterval(intervalId);
    };
  }, [jobActive, refreshOracleIndexStatus]);

  // Feed the pure ETA estimator: sample count/phase while the job is active,
  // recompute the label, and clear everything when the job ends. The component
  // only samples + renders; rate math lives in oracleIndexEta.ts.
  useEffect(() => {
    if (!jobActive) {
      indexEtaSamplesRef.current = [];
      indexEtaPrevMsRef.current = null;
      indexEtaPhaseRef.current = null;
      setIndexEtaLabel(null);
      return;
    }
    const now = Date.now();
    const count = index?.indexedFiles ?? 0;
    const expected = index?.expectedFiles ?? 0;
    const phaseKey = normalizeIndexPhase(oracleIndexStatus?.job?.phase);

    // Phase change: drop the smoothed prior so file-scan speed cannot leak
    // into the embedding estimate (and vice versa).
    if (
      indexEtaPhaseRef.current != null &&
      indexEtaPhaseRef.current !== phaseKey
    ) {
      indexEtaPrevMsRef.current = null;
    }
    indexEtaPhaseRef.current = phaseKey;

    const samples = indexEtaSamplesRef.current;
    const last = samples[samples.length - 1];
    // Record a sample when count or phase moved, or on the first tick.
    // Identical consecutive snapshots are skipped so a re-render storm does
    // not flood the window with zero-delta clones of the same instant.
    if (
      !last ||
      last.count !== count ||
      last.phase !== phaseKey ||
      now - last.at >= INDEX_POLL_MS
    ) {
      samples.push({ count, at: now, phase: phaseKey });
      // Hard cap: keep a few minutes of history at poll cadence.
      if (samples.length > 80) samples.splice(0, samples.length - 80);
    }

    const result = estimateOracleIndexEta({
      samples,
      expectedFiles: expected,
      currentCount: count,
      phase: phaseKey,
      now,
      prevRemainingMs: indexEtaPrevMsRef.current,
      stalled: indexPollStale,
    });

    if (result.kind === "eta") {
      indexEtaPrevMsRef.current = result.remainingMs;
      setIndexEtaLabel(result.label);
    } else if (result.kind === "paused" || result.kind === "estimating") {
      // Pause: clear smoothed prior so resume re-learns the rate.
      if (result.kind === "paused") indexEtaPrevMsRef.current = null;
      setIndexEtaLabel(result.label);
    } else {
      indexEtaPrevMsRef.current = null;
      setIndexEtaLabel(null);
    }
  }, [
    jobActive,
    index?.indexedFiles,
    index?.expectedFiles,
    oracleIndexStatus?.job?.phase,
    indexPollStale,
  ]);

  // "Run doctor" just mounts OracleDoctorPanel, which runs the heavy doctor
  // exactly once on its own mount and reports back via handleDoctorReport. We do
  // NOT also call the doctor here, or the two runs would race the process lock
  // ("diagnostics already running").
  const openDoctor = useCallback(() => {
    setDoctorOpen(true);
  }, []);

  // ---- Derived health-strip state ------------------------------------------
  const hasWorkspace = Boolean(oracleIndexPreferences?.indexRoot || index?.root);

  // Is an answer-provider API key configured? Lightweight only — no model load.
  // Lifted into the shared oracleProviderState util so the future Polis ask-panel
  // agrees with this admin surface.
  const providerConfigured = useMemo(
    () => deriveProviderConfigured(oracleLlmSettings),
    [oracleLlmSettings],
  );

  // Debounced runtime-readiness so a KNOWN transient restart does not flash red.
  // Saving the Oracle LLM key triggers a supervised resident-server restart
  // (backend: oracle_service.request_llm_restart), during which the "runtime"
  // probe (oracleRuntime.vectorStore.ready) briefly reads false. Rather than
  // paint the dot red for that ~1-2s blip, we hold it NEUTRAL ("checking") and
  // only commit to red if readiness stays false for >RUNTIME_DOWN_GRACE_MS.
  // A `true` reading is applied immediately (recovery is never delayed).
  //   true  -> ready (green)
  //   false -> down long enough to be real (coral)
  //   null  -> unknown / transient restart (neutral, "checking")
  const RUNTIME_DOWN_GRACE_MS = 3000;
  const [runtimeReadyStable, setRuntimeReadyStable] = useState<boolean | null>(
    null,
  );
  const runtimeGraceTimerRef = useRef<ReturnType<typeof setTimeout> | null>(
    null,
  );
  // Readiness comes from the CHUNK store (chunks.lancedb + SQLite chunks) — the
  // index dense retrieval and /ask actually use — NOT the legacy node-level
  // vectorStore (vectors.lancedb), which is no longer produced and is typically
  // empty (records:0 -> ready:false). Reading vectorStore.ready here is what made
  // the runtime dot stick red / "Checking vector runtime" forever even though the
  // real index was fully ready. Prefer chunkStore.ready; fall back to the
  // top-level `ready` mirror; tolerate an older payload that has neither.
  const rawRuntimeReady =
    oracleRuntime?.chunkStore?.ready ?? oracleRuntime?.ready;
  useEffect(() => {
    const clearGrace = () => {
      if (runtimeGraceTimerRef.current !== null) {
        clearTimeout(runtimeGraceTimerRef.current);
        runtimeGraceTimerRef.current = null;
      }
    };
    if (typeof rawRuntimeReady !== "boolean") {
      // No reading yet — stay neutral, no pending commit.
      clearGrace();
      setRuntimeReadyStable(null);
      return clearGrace;
    }
    if (rawRuntimeReady) {
      // Ready: commit green immediately and cancel any pending red.
      clearGrace();
      setRuntimeReadyStable(true);
      return clearGrace;
    }
    // Not ready: hold neutral ("checking") and only commit red after the grace
    // window, so a transient restart blip never flashes red.
    setRuntimeReadyStable((prev) => (prev === false ? false : null));
    if (runtimeGraceTimerRef.current === null) {
      runtimeGraceTimerRef.current = setTimeout(() => {
        runtimeGraceTimerRef.current = null;
        setRuntimeReadyStable(false);
      }, RUNTIME_DOWN_GRACE_MS);
    }
    return clearGrace;
  }, [rawRuntimeReady]);

  // File / chunk counts to display. The dense-index status command is the
  // primary source, but it can briefly read zero on first load / during a
  // restart while the resident server is still answering from a fully populated
  // chunk store. In that case fall back to the runtime chunkStore counts so the
  // strip shows the real "1314 files / 4177 chunks" instead of a misleading
  // "0 chunks". The chunkStore is the same index the status command reports, so
  // the two never disagree once both are loaded.
  const displayChunks = useMemo(() => {
    const fromIndex = index?.sqliteChunks ?? 0;
    if (fromIndex > 0) return fromIndex;
    return oracleRuntime?.chunkStore?.records ?? 0;
  }, [index, oracleRuntime]);
  const displayFiles = useMemo(() => {
    const fromIndex = index?.indexedFiles ?? 0;
    if (fromIndex > 0) return fromIndex;
    return oracleRuntime?.chunkStore?.files ?? 0;
  }, [index, oracleRuntime]);

  // Per-check dot state for the health strip.
  //   true  -> green (healthy)   false -> coral (failed)   undefined -> neutral
  // When the user has actually run the doctor THIS SESSION we mirror its precise
  // per-check verdicts. Otherwise we derive COARSE dots from the lightweight
  // context already loaded (runtime/vector-store readiness, workspace selection,
  // index population, provider key) and leave the dots we cannot prove without
  // loading the heavy embedding model as NEUTRAL ("Run doctor for the full
  // check"). We NEVER trigger a model load just to paint the strip.
  const doctorOkById = useMemo(() => {
    const map = new Map<string, boolean>();
    if (doctor) {
      for (const check of doctor.checks ?? []) map.set(check.id, check.ok);
      return map;
    }
    // Coarse, model-free inference. The runtime dot uses the DEBOUNCED readiness
    // so a just-triggered restart shows neutral ("checking"), not red. A `null`
    // value is left unset on the map -> rendered neutral by HealthStrip.
    if (typeof runtimeReadyStable === "boolean") {
      map.set("runtime", runtimeReadyStable);
      // The runtime readiness signal IS a live-server probe (get_oracle_runtime
      // hits the resident server's /runtime), so the coarse live_server dot can
      // mirror it until the user runs the full doctor for the authoritative
      // reachable + chunk-store-ready verdict.
      map.set("live_server", runtimeReadyStable);
    }
    // "embedder" requires loading the Qwen3 model to verify, so leave it neutral
    // until the user runs the doctor.
    map.set("workspace", hasWorkspace);
    // Index dot reflects the REAL chunk store: prefer the dense-index status,
    // fall back to the runtime chunkStore counts (same index) so a transient
    // zero from the status command does not paint a populated index as empty.
    const populated = displayFiles > 0 || displayChunks > 0;
    if (hasWorkspace) map.set("index", populated);
    map.set("provider", providerConfigured);
    return map;
  }, [
    doctor,
    runtimeReadyStable,
    hasWorkspace,
    displayFiles,
    displayChunks,
    providerConfigured,
  ]);

  // Pass/total — only claim a count once a real doctor report with checks
  // exists. Pre-run defaults used to hardcode DOCTOR_CHECK_ORDER.length (6),
  // which disagreed with the live doctor (5 checks). Without a report — or a
  // report with zero/missing checks — the UI shows the no-count variant
  // ("— checks"), never "0/0 checks pass".
  const doctorChecks = doctor?.checks ?? [];
  const totalChecks =
    doctor && doctorChecks.length > 0 ? doctorChecks.length : null;
  const passCount =
    doctor && doctorChecks.length > 0
      ? doctorChecks.filter((c) => c.ok).length
      : null;

  // server badge: honest liveness — not "has workspace ⇒ running".
  //   no-workspace → coral
  //   indexing     → amber (job active)
  //   running      → sage (runtime probe true = live HTTP)
  //   down         → coral (probe false after grace)
  //   starting     → neutral (probe null / still checking)
  const serverState:
    | "running"
    | "indexing"
    | "no-workspace"
    | "down"
    | "starting" = !hasWorkspace
    ? "no-workspace"
    : jobActive
      ? "indexing"
      : runtimeReadyStable === true
        ? "running"
        : runtimeReadyStable === false
          ? "down"
          : "starting";

  return (
    <div className="space-y-5">
      {/* Page header */}
      <div className="flex items-center gap-3">
        <div className="flex h-11 w-11 items-center justify-center rounded-2xl bg-teal/10">
          <BrainCircuit className="h-5 w-5 text-teal-dark" />
        </div>
        <div>
          <h2 className="text-[16px] font-bold tracking-tight text-cream-800">
            Oracle administration
          </h2>
          <p className="text-[12px] text-cream-500">
            Runtime, workspace indexing &amp; health for the Devboule retrieval
            index
          </p>
        </div>
      </div>

      {/* HEALTH STRIP */}
      <HealthStrip
        serverState={serverState}
        checkOk={doctorOkById}
        passCount={passCount}
        totalChecks={totalChecks}
        chunks={displayChunks}
        files={displayFiles}
        backend={
          oracleRuntime?.chunkStore?.backend ??
          oracleRuntime?.vectorStore?.backend ??
          "unknown"
        }
        onRunDoctor={openDoctor}
      />

      <OracleLoadBanner
        loading={oracleLoad.loading}
        message={oracleLoad.message}
        error={oracleLoad.error}
      />

      {runtimeSetup && !runtimeSetup.ready && (
        <OracleRuntimeSetupBanner
          setup={runtimeSetup}
          installing={installingRuntime}
          error={runtimeError}
          progress={installProgress}
          onInstall={() => void installRuntime()}
          onRetry={() => void loadRuntimeSetup()}
        />
      )}

      <div className="grid grid-cols-1 gap-5 lg:grid-cols-3">
        {/* LEFT COLUMN: Indexed files */}
        <div className="space-y-5 lg:col-span-2">
          <IndexedFilesBrowser
            indexedCount={index?.indexedFiles ?? 0}
            pendingCount={index?.pendingFiles ?? 0}
            staleCount={index?.staleFiles ?? 0}
            getOracleIndexedFiles={getOracleIndexedFiles}
          />
        </div>

        {/* RIGHT COLUMN: Workspace + Doctor */}
        <div className="space-y-5">
          {/* WORKSPACE & INDEXING */}
          <section className="rounded-2xl border border-cream-200 bg-white p-5">
            <div className="mb-3 flex items-center justify-between gap-3">
              <div className="flex items-center gap-3">
                <div className="flex h-9 w-9 items-center justify-center rounded-xl bg-terracotta/10">
                  <FolderOpen className="h-4 w-4 text-terracotta-600" />
                </div>
                <div>
                  <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
                    Workspace
                  </h3>
                  <p className="text-[11px] text-cream-400">
                    The folder Oracle indexes
                  </p>
                </div>
              </div>
              <span
                className={`rounded-lg px-2 py-1 text-[10px] font-semibold uppercase ${
                  jobActive
                    ? "bg-amber/10 text-amber-dark"
                    : oracleIndexStatus?.watcherRunning
                      ? "bg-sage/10 text-sage-dark"
                      : "bg-cream-50 text-cream-500"
                }`}
              >
                {oracleIndexStatus?.watcherRunning ? "watching" : jobStatus}
              </span>
            </div>

            <div className="rounded-xl border border-cream-200 bg-cream-50 p-3">
              <p className="text-[10px] uppercase tracking-wide text-cream-400">
                Indexed folder
              </p>
              <p className="mt-1 truncate font-mono text-[12px] text-cream-700">
                {oracleIndexPreferences?.indexRoot ??
                  index?.root ??
                  "no workspace folder selected"}
              </p>
              <div className="mt-2 flex items-center justify-between gap-2">
                <p className="text-[10px] text-cream-400">
                  → data in <span className="font-mono">oracle-data/</span>
                </p>
                <button
                  onClick={() => void chooseWorkspaceFolder()}
                  data-help-title="This changes the indexed workspace folder."
                  data-help-lines="The workspace is the folder Oracle indexes.|Choosing it points Oracle there and immediately starts indexing + the watcher.|Existing index data for the old folder is kept until you re-index.|Pick the folder root, not a subfolder."
                  className="inline-flex shrink-0 items-center gap-1.5 rounded-lg border border-cream-200 bg-white px-2 py-1 text-[10px] font-semibold text-cream-600 hover:border-terracotta-200 hover:text-terracotta"
                >
                  <FolderOpen className="h-3 w-3" />
                  Change
                </button>
              </div>
            </div>

            <p
              className={`mt-2 text-[10px] ${
                hasWorkspace ? "text-sage-dark" : "text-coral-dark"
              }`}
            >
              {workspaceKicked
                ? "✓ Folder set — indexing + watch started."
                : hasWorkspace
                  ? "✓ Workspace is set — Oracle indexes this folder."
                  : "! Choose a folder to enable Oracle."}
            </p>

            {workspaceError && (
              <div className="mt-2 rounded-lg border border-coral/30 bg-coral/5 px-2 py-1.5">
                <p className="text-[10px] font-semibold leading-4 text-coral-dark">
                  {workspaceError.message}
                </p>
                {workspaceError.remediation && (
                  <p className="mt-1 text-[10px] leading-4 text-cream-500">
                    {workspaceError.remediation}
                  </p>
                )}
              </div>
            )}

            {jobActive && (
              <div className="mt-3">
                <div className="flex items-center justify-between gap-2 text-[11px] text-cream-500">
                  <span className="min-w-0 truncate">
                    Indexing… {(index?.indexedFiles ?? 0).toLocaleString()} /{" "}
                    {(index?.expectedFiles ?? 0).toLocaleString()}
                    {indexEtaLabel ? (
                      <span className="text-cream-400">
                        {" "}
                        · {indexEtaLabel}
                      </span>
                    ) : null}
                  </span>
                  <span className="shrink-0 font-mono">
                    {index && index.expectedFiles > 0
                      ? `${Math.min(
                          100,
                          Math.round(
                            (index.indexedFiles / index.expectedFiles) * 100,
                          ),
                        )}%`
                      : "…"}
                  </span>
                </div>
                <div className="mt-1 h-1.5 w-full overflow-hidden rounded-full bg-cream-100">
                  <div
                    className="h-full rounded-full bg-teal transition-all"
                    style={{
                      width:
                        index && index.expectedFiles > 0
                          ? `${Math.min(
                              100,
                              Math.round(
                                (index.indexedFiles / index.expectedFiles) * 100,
                              ),
                            )}%`
                          : "10%",
                    }}
                  />
                </div>
                {index != null && index.vectorRecords === 0 && index.expectedFiles > 0 && (
                  <p className="mt-2 rounded-lg bg-teal/[0.08] px-2 py-1 text-[10px] leading-4 text-teal-dark">
                    The first batch is the slowest — the embedding model is warming up. The counter starts moving once the first files finish (up to a few minutes on large folders).
                  </p>
                )}
                {phaseHint && (
                  <div
                    className="mt-2 flex items-center gap-1.5 rounded-lg bg-amber/10 px-2 py-1 text-[10px] font-semibold leading-4 text-amber-dark"
                    role="status"
                    aria-live="polite"
                    data-help-title={phaseHint.phase === "embedding" ? "The index is turning text into vectors." : "The index paused itself to stay safe."}
                    data-help-lines={phaseHint.phase === "embedding"
                      ? "Each chunk is embedded into a vector LanceDB can search.|The first batch is the slowest — the model is warming up.|The counter advances as batches finish.|The job is alive — it is not stuck."
                      : "Indexing keeps the GPU and memory within safe limits.|When the GPU gets hot it cools down, then resumes on its own.|When memory runs low it waits for it to free up, then resumes.|The job is still alive — it is not stuck."
                    }
                  >
                    {phaseHint.phase === "cooling_gpu" ? (
                      <Snowflake className="h-3 w-3 shrink-0" aria-hidden="true" />
                    ) : phaseHint.phase === "embedding" ? (
                      <Loader2 className="h-3 w-3 shrink-0 animate-spin" aria-hidden="true" />
                    ) : (
                      <Clock className="h-3 w-3 shrink-0" aria-hidden="true" />
                    )}
                    <span className="truncate">{phaseHint.label}</span>
                  </div>
                )}
              </div>
            )}

            {jobStatus === "error" && jobMessage && (
              <p className="mt-2 break-words rounded-lg bg-coral/[0.06] px-2 py-1 text-[10px] leading-4 text-coral-dark">
                {jobMessage}
              </p>
            )}

            {indexPollStale && jobActive && (
              <p className="mt-2 rounded-lg bg-amber/10 px-2 py-1 text-[10px] leading-4 text-amber-dark">
                Status may be stale — use Index now or the doctor to refresh.
              </p>
            )}

            <div className="mt-3 grid grid-cols-2 gap-2">
              <Stat label="Files" value={(index?.indexedFiles ?? 0).toLocaleString()} />
              <Stat label="Vectors" value={(index?.vectorRecords ?? 0).toLocaleString()} />
              <Stat
                label="Pending"
                value={(index?.pendingFiles ?? 0).toLocaleString()}
                accent
              />
              <Stat label="Chunks" value={(index?.sqliteChunks ?? 0).toLocaleString()} />
            </div>
            <div className="mt-2 grid grid-cols-2 gap-2">
              <Stat label="Stale" value={(index?.staleFiles ?? 0).toLocaleString()} />
              <Stat
                label="Backend"
                value={oracleRuntime?.vectorStore?.backend ?? "lancedb"}
              />
            </div>

            <OracleIndexPreferencesCard />

            <div className="mt-3 flex flex-wrap gap-2">
              <button
                onClick={() => {
                  // Re-entrancy guard: never let a click double-fire a
                  // (possibly already-running) index job even while the gate is
                  // relaxed for a stalled state. Reset in the promise finally.
                  if (indexFiringRef.current) return;
                  indexFiringRef.current = true;
                  void startOracleIndexJob(false, 1, false, true).finally(
                    () => {
                      indexFiringRef.current = false;
                    },
                  );
                }}
                // Stall-aware gate: if the stall detector has fired (the job is
                // stuck in queued/running and not progressing), re-enable so the
                // user can retry instead of being permanently locked out.
                disabled={isLoading || (jobActive && !indexPollStale)}
                data-help-title="This starts a dense Oracle indexing job."
                data-help-lines="Dense indexing turns chunks into vectors LanceDB can search semantically.|It uses the configured local embedding pipeline and can be slow on huge folders.|The job is resumable and should only process pending files unless Force is enabled.|Watch RAM and temperature when running many batches."
                className="inline-flex items-center gap-1.5 rounded-xl bg-teal px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-60"
              >
                <Play className="h-3.5 w-3.5" />
                Index now
              </button>
              <button
                onClick={() => void startOracleIndexWatcher()}
                disabled={isLoading || oracleIndexStatus?.watcherRunning}
                data-help-title="This starts the incremental file watcher."
                data-help-lines="The watcher keeps listening for new or modified files in the indexed root.|It avoids re-indexing unchanged files.|Use it for normal development days so Oracle stays fresh.|Stop it if the machine is hot or you need every resource free."
                className="inline-flex items-center gap-1.5 rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:border-sage/30 hover:text-sage-dark disabled:opacity-60"
              >
                <Eye className="h-3.5 w-3.5" />
                Watch
              </button>
              <button
                onClick={() => void stopOracleIndexWatcher()}
                disabled={isLoading || !oracleIndexStatus?.watcherRunning}
                data-help-title="This stops automatic Oracle file watching."
                data-help-lines="Stopping watch does not delete the index.|New files will wait until the next manual sync or watcher start.|Use this when the PC is under heavy RAM, CPU, or GPU load.|Existing LanceDB records remain available for search."
                className="inline-flex items-center gap-1.5 rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:border-coral/30 hover:text-coral-dark disabled:opacity-60"
              >
                <StopCircle className="h-3.5 w-3.5" />
                Stop
              </button>
            </div>
          </section>

          {/* DOCTOR PANEL */}
          {doctorOpen ? (
            <OracleDoctorPanel onReport={handleDoctorReport} />
          ) : (
            <section className="rounded-2xl border border-cream-200 bg-white p-5">
              <div className="mb-3 flex items-center justify-between gap-3">
                <div className="flex items-center gap-3">
                  <div className="flex h-9 w-9 items-center justify-center rounded-xl bg-teal/10">
                    <Stethoscope className="h-4 w-4 text-teal-dark" />
                  </div>
                  <div>
                    <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
                      Doctor
                    </h3>
                    <p className="text-[11px] text-cream-400">
                      {totalChecks != null
                        ? `Truthful health, ${totalChecks} checks`
                        : "Truthful health"}
                    </p>
                  </div>
                </div>
                <button
                  onClick={openDoctor}
                  data-help-title="This opens the Oracle doctor panel."
                  data-help-lines="The doctor inspects runtime, embedder, workspace, index, and provider.|Each failing check carries a fixable remediation.|It does not change your data.|Open it whenever an answer fails to see why."
                  className="inline-flex items-center gap-1.5 rounded-xl border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-600 hover:border-teal-light hover:text-teal-dark"
                >
                  <Stethoscope className="h-3.5 w-3.5" />
                  Run doctor
                </button>
              </div>
              <p className="text-[11px] leading-5 text-cream-500">
                {passCount != null && totalChecks != null
                  ? `${passCount}/${totalChecks} checks pass. Run the doctor for the full per-check breakdown and remediation steps.`
                  : "Run the doctor for the full per-check breakdown and remediation steps."}
              </p>
            </section>
          )}
        </div>
      </div>

      {/* Oracle LLM settings — the remote provider that writes answers */}
      <CollapsibleSection title="Oracle LLM" defaultOpen={false}>
        <OracleAnswerSettingsCard />
      </CollapsibleSection>

      {/* CLI Agents — register Oracle MCP in local Claude/Codex config */}
      <CollapsibleSection title="CLI Agents" defaultOpen={false}>
        <CliAgentsCard />
      </CollapsibleSection>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Oracle feature toggle — enable/disable the RAG server
// ---------------------------------------------------------------------------
export function OracleFeatureToggle() {
  const [enabled, setEnabled] = useState(true);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const mountedRef = useRef(true);

  useEffect(() => { mountedRef.current = true; return () => { mountedRef.current = false; }; }, []);

  useEffect(() => {
    invokeBackendCommand<boolean>("get_oracle_enabled")
      .then((v) => { if (mountedRef.current) setEnabled(Boolean(v)); })
      .catch(() => {})
      .finally(() => { if (mountedRef.current) setLoading(false); });
  }, []);

  const onToggle = () => {
    if (busy || loading) return;
    const next = !enabled;
    setEnabled(next);
    setBusy(true);
    invokeBackendCommand<boolean>("set_oracle_enabled", { enabled: next })
      .catch(() => { if (mountedRef.current) setEnabled(!next); })
      .finally(() => { if (mountedRef.current) setBusy(false); });
  };

  return (
    <div className="rounded-2xl border border-cream-200 bg-white p-5">
      <div className="flex items-center justify-between">
        <div className="flex flex-col">
          <div className="flex h-9 w-9 items-center justify-center rounded-xl bg-amber/10">
            <BrainCircuit className="h-4 w-4 text-amber-dark" />
          </div>
          <div className="mt-3 flex flex-col">
            <span className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
              Oracle
            </span>
            <span className="text-[11px] text-cream-400">RAG retrieval server</span>
          </div>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={enabled}
          aria-label="Toggle Oracle"
          disabled={busy || loading}
          onClick={() => void onToggle()}
          className={`relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors disabled:opacity-50 ${
            enabled ? "bg-teal" : "bg-cream-300"
          }`}
        >
          <span
            aria-hidden="true"
            className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform ${
              enabled ? "translate-x-4" : "translate-x-0.5"
            }`}
          />
        </button>
      </div>
      <div className="mt-4 flex gap-2 rounded-xl border border-amber/40 bg-amber/10 p-3">
        <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-amber-dark" />
        <p className="text-[12px] leading-5 text-amber-dark">
          Core dependency. Switching Oracle off stops the RAG/embeddings server, so agents lose semantic project context and the auto-reindex on commit. The app keeps running and the Kanban/plan tools still work, but agents get much weaker. Switch off only to debug.
        </p>
      </div>
      <p className="mt-2 text-[11px] text-cream-400">Applies on app restart.</p>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Health strip
// ---------------------------------------------------------------------------
function HealthStrip({
  serverState,
  checkOk,
  passCount,
  totalChecks,
  chunks,
  files,
  backend,
  onRunDoctor,
}: {
  serverState: "running" | "indexing" | "no-workspace" | "down" | "starting";
  checkOk: Map<string, boolean>;
  /** null until a real doctor report exists — never invent a pre-run total. */
  passCount: number | null;
  totalChecks: number | null;
  chunks: number;
  files: number;
  backend: string;
  onRunDoctor: () => void;
}) {
  const serverLabel =
    serverState === "no-workspace"
      ? "no workspace"
      : serverState === "indexing"
        ? "indexing"
        : serverState === "down"
          ? "down"
          : serverState === "starting"
            ? "starting…"
            : "running";
  const coreColor =
    serverState === "no-workspace" || serverState === "down"
      ? "bg-coral"
      : serverState === "indexing" || serverState === "starting"
        ? "bg-amber"
        : "bg-sage";
  const labelColor =
    serverState === "no-workspace" || serverState === "down"
      ? "text-coral-dark"
      : serverState === "indexing" || serverState === "starting"
        ? "text-amber-dark"
        : "text-sage-dark";
  const showPing =
    serverState === "running" ||
    serverState === "indexing" ||
    serverState === "starting";
  const pingColor =
    serverState === "indexing" || serverState === "starting"
      ? "bg-amber/60"
      : "bg-sage/60";

  return (
    <section
      className="rounded-2xl border border-cream-200 bg-white p-4"
      data-help-title="This strip is the at-a-glance Oracle health answer."
      data-help-lines="The server badge shows whether Oracle is running, indexing, or has no workspace.|The five dots are the doctor checks: runtime, embedder, workspace, index, provider.|Green is healthy; coral/red is a failed check.|Run doctor opens the full per-check breakdown."
    >
      <div className="flex flex-wrap items-center gap-x-6 gap-y-3">
        <div className="flex items-center gap-2">
          <span className="relative flex h-2.5 w-2.5">
            {showPing && (
              <span
                className={`absolute inline-flex h-full w-full animate-ping rounded-full ${pingColor}`}
              />
            )}
            <span
              className={`relative inline-flex h-2.5 w-2.5 rounded-full ${coreColor}`}
            />
          </span>
          <span className="text-[12px] font-semibold text-cream-700">
            Oracle server: <span className={labelColor}>{serverLabel}</span>
          </span>
        </div>

        <div className="h-5 w-px bg-cream-200" />

        <div className="flex items-center gap-3">
          <span className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
            Health
          </span>
          <div className="flex items-center gap-1.5">
            {DOCTOR_CHECK_ORDER.map((id) => {
              const ok = checkOk.get(id);
              const color =
                ok === undefined
                  ? "bg-cream-300"
                  : ok
                    ? "bg-sage"
                    : "bg-coral";
              return (
                <span
                  key={id}
                  title={id}
                  className={`h-2.5 w-2.5 rounded-full ${color}`}
                />
              );
            })}
          </div>
          <span className="text-[11px] text-cream-500">
            {passCount != null && totalChecks != null
              ? `${passCount}/${totalChecks} checks pass`
              : "— checks"}
          </span>
        </div>

        <div className="h-5 w-px bg-cream-200" />

        <div className="text-[11px] text-cream-500">
          <span className="font-mono text-cream-700">
            {chunks.toLocaleString()}
          </span>{" "}
          chunks ·{" "}
          <span className="font-mono text-cream-700">
            {files.toLocaleString()}
          </span>{" "}
          files · backend{" "}
          <span className="font-mono text-cream-700">{backend}</span>
        </div>

        <button
          onClick={onRunDoctor}
          data-help-title="This runs the Oracle doctor."
          data-help-lines="It verifies runtime, embedder, workspace, index, and provider.|Each failing check carries a remediation.|It does not modify your data or index.|Use it when an answer fails or a dot is red."
          className="ml-auto inline-flex items-center gap-1.5 rounded-xl border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-600 hover:border-teal-light hover:text-teal-dark"
        >
          <Stethoscope className="h-3.5 w-3.5" />
          Run doctor
        </button>
      </div>
    </section>
  );
}

// ---------------------------------------------------------------------------
// Indexed files browser
// ---------------------------------------------------------------------------
function IndexedFilesBrowser({
  indexedCount,
  pendingCount,
  staleCount,
  getOracleIndexedFiles,
}: {
  indexedCount: number;
  pendingCount: number;
  staleCount: number;
  getOracleIndexedFiles: (opts?: {
    limit?: number;
    offset?: number;
    filter?: string;
  }) => Promise<import("../../types/backend").OracleIndexedFiles>;
}) {
  const [tab, setTab] = useState<FileTab>("indexed");
  const [filterInput, setFilterInput] = useState("");
  const [filter, setFilter] = useState("");
  const [offset, setOffset] = useState(0);
  const [files, setFiles] = useState<OracleIndexedFile[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);
  const reqSeqRef = useRef(0);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Debounce the filter input (~250ms). Clears the timer on every keystroke and
  // on unmount so a late timer never fires into an unmounted component.
  useEffect(() => {
    const id = window.setTimeout(() => {
      setFilter(filterInput.trim());
      setOffset(0);
    }, FILE_FILTER_DEBOUNCE_MS);
    return () => window.clearTimeout(id);
  }, [filterInput]);

  // Reset paging when the tab changes.
  useEffect(() => {
    setOffset(0);
  }, [tab]);

  // The files endpoint only serves the indexed manifest. Pending/Stale are
  // surfaced as count-driven notes (the index-status job processes them), so we
  // only query the endpoint for the Indexed tab.
  const fetchFiles = useCallback(async () => {
    if (tab !== "indexed") return;
    const seq = reqSeqRef.current + 1;
    reqSeqRef.current = seq;
    setLoading(true);
    setError(null);
    try {
      const page = await getOracleIndexedFiles({
        limit: FILES_PAGE_SIZE,
        offset,
        filter: filter || undefined,
      });
      if (!mountedRef.current || reqSeqRef.current !== seq) return;
      setFiles(page.files);
      setTotal(page.total);
    } catch (e) {
      if (!mountedRef.current || reqSeqRef.current !== seq) return;
      setError(toOracleError(e).message);
      setFiles([]);
      setTotal(0);
    } finally {
      if (mountedRef.current && reqSeqRef.current === seq) setLoading(false);
    }
  }, [tab, offset, filter, getOracleIndexedFiles]);

  useEffect(() => {
    void fetchFiles();
  }, [fetchFiles]);

  const canPrev = offset > 0;
  const canNext = offset + FILES_PAGE_SIZE < total;

  return (
    <section className="rounded-2xl border border-cream-200 bg-white p-5">
      <div className="mb-3 flex items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          <div className="flex h-9 w-9 items-center justify-center rounded-xl bg-sage/10">
            <Files className="h-4 w-4 text-sage-dark" />
          </div>
          <div>
            <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
              Indexed files
            </h3>
            <p className="text-[11px] text-cream-400">
              Everything Oracle has embedded, from the index manifest
            </p>
          </div>
        </div>
        <div className="flex items-center gap-1 rounded-xl bg-cream-50 p-1 text-[11px] font-semibold">
          <FileTabButton
            active={tab === "indexed"}
            onClick={() => setTab("indexed")}
            label="Indexed"
            count={indexedCount}
            countClass="text-cream-400"
          />
          <FileTabButton
            active={tab === "pending"}
            onClick={() => setTab("pending")}
            label="Pending"
            count={pendingCount}
            countClass="text-amber-dark"
          />
          <FileTabButton
            active={tab === "stale"}
            onClick={() => setTab("stale")}
            label="Stale"
            count={staleCount}
            countClass="text-cream-400"
          />
        </div>
      </div>

      {tab === "indexed" ? (
        <>
          <div className="flex items-center gap-2 rounded-xl border border-cream-200 bg-cream-50 px-3 py-2">
            <Search className="h-3.5 w-3.5 text-cream-400" />
            <input
              value={filterInput}
              onChange={(event) => setFilterInput(event.target.value)}
              placeholder={`Filter ${indexedCount.toLocaleString()} files…`}
              data-help-title="This filters the indexed-files list by path."
              data-help-lines="Type part of a file path to narrow the list.|Filtering runs against the index manifest, not a live disk scan.|It is debounced so it does not query on every keystroke.|Clearing it shows the full paginated list again."
              className="min-w-0 flex-1 bg-transparent text-[12px] text-cream-700 outline-none"
            />
          </div>

          <div className="mt-3 max-h-64 overflow-auto rounded-xl border border-cream-200">
            <table className="w-full text-[12px]">
              <thead className="sticky top-0 bg-cream-50 text-[10px] uppercase tracking-wide text-cream-500">
                <tr>
                  <th className="px-3 py-2 text-left font-semibold">File</th>
                  <th className="px-3 py-2 text-right font-semibold">Chunks</th>
                  <th className="px-3 py-2 text-right font-semibold">Indexed</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-cream-100">
                {files.map((file) => (
                  <tr key={file.path} className="hover:bg-cream-50">
                    <td className="px-3 py-2 font-mono text-cream-700">
                      <span className="block truncate" title={file.path}>
                        {file.path}
                      </span>
                    </td>
                    <td className="px-3 py-2 text-right font-mono text-cream-500">
                      {file.chunks.toLocaleString()}
                    </td>
                    <td className="px-3 py-2 text-right text-cream-400">
                      {formatIndexedAt(file.updatedAt)}
                    </td>
                  </tr>
                ))}
                {!loading && files.length === 0 && !error && (
                  <tr>
                    <td
                      colSpan={3}
                      className="px-3 py-6 text-center text-[12px] text-cream-400"
                    >
                      {filter
                        ? "No indexed files match this filter."
                        : "No indexed files yet."}
                    </td>
                  </tr>
                )}
                {error && (
                  <tr>
                    <td
                      colSpan={3}
                      className="px-3 py-6 text-center text-[12px] text-coral-dark"
                    >
                      {error}
                    </td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>

          <div className="mt-2 flex items-center justify-between gap-2">
            <p className="text-[10px] text-cream-400">
              {loading
                ? "Loading…"
                : total > 0
                  ? `Showing ${offset + 1}–${Math.min(offset + FILES_PAGE_SIZE, total)} of ${total.toLocaleString()}`
                  : "No files."}
            </p>
            <div className="flex items-center gap-1">
              <button
                onClick={() =>
                  setOffset((prev) => Math.max(0, prev - FILES_PAGE_SIZE))
                }
                disabled={!canPrev || loading}
                className="rounded-lg border border-cream-200 bg-white px-2 py-1 text-[11px] font-semibold text-cream-600 disabled:cursor-not-allowed disabled:opacity-50"
              >
                Prev
              </button>
              <button
                onClick={() => setOffset((prev) => prev + FILES_PAGE_SIZE)}
                disabled={!canNext || loading}
                className="rounded-lg border border-cream-200 bg-white px-2 py-1 text-[11px] font-semibold text-cream-600 disabled:cursor-not-allowed disabled:opacity-50"
              >
                Next
              </button>
            </div>
          </div>

          <p className="mt-2 text-[10px] text-cream-400">
            Excluded by <span className="font-mono">.oracleignore</span>: secrets,
            node_modules, raw data, and <span className="font-mono">oracle-data/</span>{" "}
            (Oracle's own output).
          </p>
        </>
      ) : (
        <p className="rounded-xl bg-cream-50 px-3 py-6 text-center text-[12px] text-cream-400">
          {tab === "pending"
            ? `${pendingCount.toLocaleString()} files are waiting to be indexed. Run "Index now" or start the watcher to embed them.`
            : `${staleCount.toLocaleString()} files changed since they were last embedded. Re-index to refresh their chunks.`}
        </p>
      )}
    </section>
  );
}

function FileTabButton({
  active,
  onClick,
  label,
  count,
  countClass,
}: {
  active: boolean;
  onClick: () => void;
  label: string;
  count: number;
  countClass: string;
}) {
  return (
    <button
      onClick={onClick}
      className={`rounded-lg px-2.5 py-1 ${
        active ? "bg-white text-cream-700 shadow-sm" : "text-cream-500"
      }`}
    >
      {label}{" "}
      <span className={active ? "text-cream-400" : countClass}>
        {count.toLocaleString()}
      </span>
    </button>
  );
}

// ---------------------------------------------------------------------------
// Shared small pieces
// ---------------------------------------------------------------------------
function Stat({
  label,
  value,
  accent,
}: {
  label: string;
  value: string;
  accent?: boolean;
}) {
  return (
    <div
      className="min-w-0 rounded-xl bg-cream-50 px-3 py-2"
      data-help-title={`${label} is an Oracle index fact.`}
      data-help-lines="Index facts explain how much of the workspace Oracle has embedded.|Pending/stale counts show work the indexer still owes.|Low counts usually explain weak retrieval or empty answers.|If a value looks stale, refresh Oracle status."
    >
      <p className="text-[10px] uppercase tracking-wide text-cream-400">{label}</p>
      <p
        className={`mt-0.5 truncate font-mono text-[15px] font-semibold ${
          accent ? "text-amber-dark" : "text-cream-700"
        }`}
      >
        {value}
      </p>
    </div>
  );
}

/** Approx. free disk when the int8 embedder must be downloaded (lite package).
 *  Model ~600 MB + small MCP venv; shown up front so first-run is no surprise.
 */
const ORACLE_RUNTIME_LITE_DISK_MB = 600;
/** Approx. free disk when weights ship in the package (full) — MCP venv only. */
const ORACLE_RUNTIME_FULL_DISK_MB = 100;

export function OracleRuntimeSetupBanner({
  setup,
  installing,
  error,
  progress,
  onInstall,
  onRetry,
}: {
  setup: OracleRuntimeSetup;
  installing: boolean;
  error: string | null;
  progress: { stage: string; percent: number; message: string } | null;
  onInstall: () => void;
  // Re-probe the runtime. Used when the probe is still inconclusive ("checking")
  // so the user can re-check instead of being told Python is missing.
  onRetry: () => void;
}) {
  const stage = classifyRuntimeStage(setup);
  const checking = stage === "checking";
  // Full package ships int8 weights; install only seeds + builds the MCP venv.
  // Prefer the explicit flag; fall back to bundleKind only when the flag is
  // absent (older payloads). Never treat embedderBundled:false + full as included.
  const embedderBundled =
    setup.embedderBundled === true ||
    (setup.embedderBundled == null && setup.bundleKind === "full");
  const needsModelDownload = !setup.embedderReady && !embedderBundled;
  const installLabel = setup.embedderReady
    ? "Install runtime"
    : embedderBundled
      ? "Install runtime (model included)"
      : `Install runtime (~${ORACLE_RUNTIME_LITE_DISK_MB} MB model download)`;
  const diskClaimMb = embedderBundled
    ? ORACLE_RUNTIME_FULL_DISK_MB
    : ORACLE_RUNTIME_LITE_DISK_MB;
  const helpLines = embedderBundled
    ? "Oracle retrieval runs in-app (Rust ONNX). This full package already includes the Qwen3 embedding model — install only seeds it into app data and sets up a small MCP helper venv.|This is separate from the answer model: retrieval is always local, answers prefer a remote API key.|No large model download is required on first install.|If Python is missing, install Python 3 first, then run setup."
    : "Oracle retrieval runs in-app (Rust ONNX) and needs the Qwen3 embedding model (~600 MB), plus a small Python helper venv for the agent MCP server.|This is separate from the answer model: retrieval is always local, answers prefer a remote API key.|The lite package downloads the model on first install; the full package ships the weights in the app bundle.|If Python is missing, install Python 3 first, then run setup.";
  // While the probe is inconclusive we cannot trust the per-step verdicts (the
  // probe may simply not have answered yet), so the Python step renders as a
  // pending "checking" pill rather than a red ✗.
  const steps: { label: string; done: boolean }[] = [
    { label: "Python 3.9+", done: !checking && setup.pythonFound },
    { label: "Virtual env", done: setup.venvReady },
    { label: "MCP deps", done: setup.depsReady },
    { label: "Qwen3 embedder", done: setup.embedderReady },
  ];
  return (
    <div
      className={`rounded-2xl border px-4 py-4 ${
        checking
          ? "border-cream-200 bg-cream-50"
          : "border-amber/25 bg-amber/[0.07]"
      }`}
      data-help-title="This sets up the local retrieval runtime."
      data-help-lines={helpLines}
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h3
            className={`text-[12px] font-semibold ${
              checking ? "text-cream-700" : "text-amber-dark"
            }`}
          >
            {checking
              ? "Checking the local runtime…"
              : "Local retrieval runtime not ready"}
          </h3>
          <p className="mt-1 text-[11px] leading-5 text-cream-600">
            {checking ? (
              "First startup can be slow on a busy machine. We're still verifying the local Python runtime — no action needed yet."
            ) : embedderBundled ? (
              <>
                Oracle search runs fully on your machine (in-app Rust ONNX
                engine). This package already includes the{" "}
                <span className="font-mono">{setup.embedModel}</span> embedding
                model — one click seeds it into app data and sets up a small
                Python helper venv for the agent MCP server. Needs about{" "}
                <span className="font-semibold">{diskClaimMb} MB free disk</span>
                .
              </>
            ) : (
              <>
                Oracle search runs fully on your machine (in-app Rust ONNX
                engine). One click installs the{" "}
                <span className="font-mono">{setup.embedModel}</span> embedding
                model and a small Python helper venv for the agent MCP server —
                separate from the answer model, retrieval is always local. First
                install downloads ~{ORACLE_RUNTIME_LITE_DISK_MB} MB and needs
                about{" "}
                <span className="font-semibold">{diskClaimMb} MB free disk</span>
                .
              </>
            )}
          </p>
          {stage !== "checking" && (
            <p className="mt-1 text-[10px] leading-4 text-cream-500">
              {needsModelDownload
                ? "Nothing leaves your machine except the model download; you can keep working while it runs."
                : "Nothing leaves your machine; setup stays local."}
            </p>
          )}
          <div className="mt-2 flex flex-wrap gap-2">
            {steps.map((step) => {
              const isPythonChecking = checking && step.label === "Python 3.9+";
              return (
                <span
                  key={step.label}
                  className={`rounded-lg px-2 py-1 text-[10px] font-semibold ${
                    step.done
                      ? "bg-sage/10 text-sage-dark"
                      : isPythonChecking
                        ? "bg-cream-100 text-cream-500"
                        : "bg-white text-cream-500"
                  }`}
                >
                  {step.done ? "✓" : isPythonChecking ? "…" : "○"} {step.label}
                </span>
              );
            })}
          </div>
          {/* Only ever show the scary "missing Python" line when the probe has
              GENUINELY finished and found nothing — never while inconclusive. */}
          {stage === "missingPython" && (
            <p className="mt-2 text-[11px] leading-5 text-coral-dark">
              No Python 3.9+ found. Install Python 3 (python.org or your package
              manager), then run setup.
            </p>
          )}
          {checking && (
            <p className="mt-2 flex items-center gap-2 text-[11px] text-cream-500">
              <span className="h-3 w-3 animate-spin rounded-full border-2 border-cream-300 border-t-teal-dark" />
              Verifying — this can take a moment on first startup.
            </p>
          )}
          {installing && (
            <div className="mt-2">
              <p className="flex items-center gap-2 text-[11px] text-amber-dark">
                <span className="h-3 w-3 animate-spin rounded-full border-2 border-amber/30 border-t-amber-dark" />
                {progress
                  ? progress.stage === "venv"
                    ? `Setting up Python environment… ${progress.percent}%`
                    : progress.stage === "download"
                      ? `Downloading Qwen3 ONNX model… ${progress.percent}%`
                      : progress.message
                  : needsModelDownload
                    ? `Installing — downloading ~${ORACLE_RUNTIME_LITE_DISK_MB} MB model, this can take several minutes…`
                    : embedderBundled && !setup.embedderReady
                      ? "Installing — seeding bundled model and MCP venv…"
                      : "Installing — setting up the MCP helper venv…"}
              </p>
              {progress && (
                <div className="mt-1.5 h-1.5 w-full max-w-md overflow-hidden rounded-full bg-cream-100">
                  <div
                    className="h-full rounded-full bg-terracotta transition-all duration-300"
                    style={{ width: `${Math.min(100, progress.percent)}%` }}
                  />
                </div>
              )}
              {progress && progress.message && (
                <p className="mt-1 truncate font-mono text-[10px] text-cream-500">
                  {progress.message}
                </p>
              )}
            </div>
          )}
          {error && (
            <p className="mt-2 max-w-3xl break-words font-mono text-[10px] leading-5 text-coral-dark">
              {error}
            </p>
          )}
        </div>
        {checking ? (
          <button
            onClick={onRetry}
            data-help-title="This re-checks the local Oracle runtime."
            data-help-lines="The first runtime probe can be slow on a busy machine.|Re-check asks the backend to probe Python and the embedder again.|It does not install or change anything; it only inspects state.|Use it if the check seems stuck."
            className="inline-flex shrink-0 items-center gap-2 rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:border-teal-light hover:text-teal-dark"
          >
            <RefreshCw className="h-3.5 w-3.5" />
            Re-check
          </button>
        ) : (
          <button
            onClick={onInstall}
            disabled={installing || !setup.pythonFound}
            data-help-title="This installs the local Oracle retrieval runtime."
            data-help-lines={
              embedderBundled
                ? "It seeds the bundled Qwen3 ONNX model into app data and creates a small Python venv for the agent MCP server.|It does not touch your repository or send anything remote.|Re-running it repairs a partial install."
                : "It creates a Python virtual environment for the agent MCP server and downloads the Qwen3 ONNX int8 model (~600 MB).|It does not touch your repository.|It needs internet for the first model download.|Re-running it repairs a partial install."
            }
            className="inline-flex shrink-0 items-center gap-2 rounded-xl bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
          >
            {installing ? "Installing…" : installLabel}
          </button>
        )}
      </div>
    </div>
  );
}

function OracleLoadBanner({
  loading,
  message,
  error,
}: {
  loading: boolean;
  message: string;
  error: string | null;
}) {
  // Once Oracle has loaded cleanly (not loading, no error), drop the banner to
  // keep the redesigned page calm; the health strip carries the live status.
  if (!loading && !error) return null;
  return (
    <div
      className={`rounded-2xl border px-4 py-3 ${
        error
          ? "border-coral/20 bg-coral/[0.06]"
          : "border-amber/20 bg-amber/[0.08]"
      }`}
      data-help-title="This banner explains Oracle loading state."
      data-help-lines="Oracle may need to start a local Python server, load LanceDB, check embeddings, and verify the answer model.|For Devboule, a few minutes of first-load delay is normal, but a visible loading state prevents false failure assumptions.|Errors here mean retrieval or answering may be unreliable until fixed.|Refresh after changing model settings, index state, or watcher status."
    >
      <div className="flex flex-wrap items-center gap-3">
        <div
          className={`h-3.5 w-3.5 rounded-full border-2 ${
            loading
              ? "animate-spin border-amber/30 border-t-amber-dark"
              : "border-coral-dark bg-coral-dark"
          }`}
        />
        <p
          className={`text-[12px] font-semibold ${
            error ? "text-coral-dark" : "text-amber-dark"
          }`}
        >
          {message}
        </p>
        {loading && (
          <p className="text-[11px] text-cream-500">
            First local startup can take a few minutes while Python, LanceDB and
            the embedder warm up.
          </p>
        )}
      </div>
      {error && (
        <p className="mt-2 max-w-4xl break-words font-mono text-[10px] leading-5 text-coral-dark">
          {error}
        </p>
      )}
    </div>
  );
}

// ── Oracle index-preferences card (auto-watch toggle + index mode select) ──
// Extracted as a named component so it can be unit-tested (renderToStaticMarkup)
// without mounting the full admin panel. Travelled here with the admin surface
// in Phase 4(b).
function OracleIndexPreferencesCard() {
  const { oracleIndexPreferences, saveOracleIndexPreferences } = useAppContext();
  const prefs = oracleIndexPreferences;
  return (
    <>
      <label className="mt-3 flex items-center justify-between gap-3 rounded-xl bg-cream-50 px-3 py-2 text-[11px] font-medium text-cream-600">
        <span>Auto-watch after unlock</span>
        <input
          type="checkbox"
          checked={prefs?.autoWatchOnUnlock ?? true}
          data-help-title="Auto-watch keeps Oracle indexing new file changes."
          data-help-lines="The watcher notices new or modified files after you unlock the app.|It indexes incrementally instead of rebuilding everything from scratch.|It should skip useless folders according to Oracle rules.|Turn it off if the PC is too hot or you want indexing only by manual job."
          onChange={(event) =>
            void saveOracleIndexPreferences({
              autoWatchOnUnlock: event.target.checked,
              indexRoot: prefs?.indexRoot ?? null,
              indexMode: prefs?.indexMode,
            })
          }
        />
      </label>
      <label className="mt-2 flex items-center justify-between gap-3 rounded-xl bg-cream-50 px-3 py-2 text-[11px] font-medium text-cream-600">
        <span
          data-help-title="Indexing mode controls when Oracle indexes your files."
          data-help-lines="Continuous watcher keeps Oracle up to date as you edit files.|On commit is lighter — Oracle indexes only after an in-app git commit or pull.|Commit mode reduces background load on weaker GPUs."
        >
          Indexing mode
        </span>
        <select
          value={prefs?.indexMode ?? "watch"}
          className="rounded-lg border border-cream-200 bg-white px-2 py-1 text-[11px] text-cream-700"
          onChange={(event) =>
            void saveOracleIndexPreferences({
              autoWatchOnUnlock: prefs?.autoWatchOnUnlock ?? true,
              indexRoot: prefs?.indexRoot ?? null,
              indexMode: event.target.value as "watch" | "commit",
            })
          }
        >
          <option value="watch">Continuous watcher</option>
          <option value="commit">On commit — light</option>
        </select>
      </label>
    </>
  );
}

export const __test_OracleIndexPreferencesCard = OracleIndexPreferencesCard;
