import { useCallback, useEffect, useRef, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  RefreshCw,
  Stethoscope,
} from "lucide-react";
import { useAppContext } from "../../context/AppContext";
import { toOracleError } from "../../utils/oracleError";
import type { OracleDoctorCheck, OracleDoctorReport } from "../../types/backend";

// Human labels for the six fixed doctor checks. Falls back to the raw id so a
// new backend check still renders (just without a friendly title).
const CHECK_LABELS: Record<string, string> = {
  runtime: "Runtime",
  embedder: "Embedder",
  workspace: "Workspace",
  index: "Index",
  live_server: "Live server",
  provider: "Provider",
};

// How long to wait before auto-retrying after a soft doctor failure (e.g. the
// backend was momentarily busy). Short enough to feel responsive, long enough
// not to hammer the process-wide embedder lock.
const RETRY_DELAY_MS = 4000;

// The truthful-health panel (mockup section 5). On mount it runs the Oracle
// doctor and renders each check as a row (green check / red alert, detail,
// and remediation when not ok). Check count comes from the report, never a
// hardcoded number. A doctor call can itself fail (the command throws an
// OracleError), so loading + error states are first-class.
export function OracleDoctorPanel({
  onReport,
}: {
  // Called with the report of each successful doctor run so the parent's health
  // strip can reflect the precise per-check verdicts WITHOUT issuing its own
  // (heavy, model-loading) doctor call. The panel is the single doctor runner.
  onReport?: (report: OracleDoctorReport) => void;
} = {}) {
  const { getOracleDoctor } = useAppContext();
  const [report, setReport] = useState<OracleDoctorReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);
  // Guards against an out-of-order resolve clobbering a newer run (double-click
  // Re-run, or unmount mid-flight).
  const runSeqRef = useRef(0);
  // Pending auto-retry timer for a soft failure; cleared on unmount and before
  // every new run so we never fire into an unmounted component or stack timers.
  const retryTimerRef = useRef<number | null>(null);
  // Keep the latest onReport without putting it in runDoctor's deps, so a new
  // callback identity from the parent never re-triggers the heavy mount run.
  const onReportRef = useRef(onReport);
  useEffect(() => {
    onReportRef.current = onReport;
  }, [onReport]);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      if (retryTimerRef.current !== null) {
        window.clearTimeout(retryTimerRef.current);
        retryTimerRef.current = null;
      }
    };
  }, []);

  const runDoctor = useCallback(async () => {
    const seq = runSeqRef.current + 1;
    runSeqRef.current = seq;
    // A new run supersedes any scheduled auto-retry.
    if (retryTimerRef.current !== null) {
      window.clearTimeout(retryTimerRef.current);
      retryTimerRef.current = null;
    }
    setLoading(true);
    setError(null);
    try {
      const next = await getOracleDoctor();
      if (!mountedRef.current || runSeqRef.current !== seq) return;
      setReport(next);
      onReportRef.current?.(next);
    } catch (e) {
      if (!mountedRef.current || runSeqRef.current !== seq) return;
      // A doctor call should not dead-end the user. Surface a soft message and
      // schedule a single auto-retry; the manual Retry button is always there
      // too. We keep the last good report visible (if any) under the notice.
      setError(toOracleError(e).message);
      retryTimerRef.current = window.setTimeout(() => {
        retryTimerRef.current = null;
        if (mountedRef.current) void runDoctor();
      }, RETRY_DELAY_MS);
    } finally {
      if (mountedRef.current && runSeqRef.current === seq) setLoading(false);
    }
  }, [getOracleDoctor]);

  useEffect(() => {
    void runDoctor();
  }, [runDoctor]);

  return (
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
              {report && (report.checks ?? []).length > 0
                ? `Truthful health, ${(report.checks ?? []).length} checks`
                : "Truthful health"}
            </p>
          </div>
        </div>
        <button
          onClick={() => void runDoctor()}
          disabled={loading}
          data-help-title="This re-runs the Oracle health checks."
          data-help-lines="The doctor verifies the runtime, embedder, workspace, index, and answer provider.|Each check reports a fixable remediation when it fails.|It does not change your data or index; it only inspects state.|Re-run after fixing a problem to confirm the green state."
          className="inline-flex items-center gap-1.5 rounded-xl border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-600 hover:border-teal-light hover:text-teal-dark disabled:cursor-not-allowed disabled:opacity-60"
        >
          <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
          Re-run
        </button>
      </div>

      {/* Soft, never-terminal error notice. A doctor call can momentarily fail
          (busy backend / process lock); we auto-retry and offer a manual Retry,
          and keep the last good report visible below so the panel is never an
          empty dead-end. */}
      {error && (
        <div className="mb-2 rounded-xl border border-amber/30 bg-amber/[0.07] px-3 py-3">
          <div className="flex items-start justify-between gap-3">
            <div className="min-w-0">
              <p className="flex items-center gap-2 text-[12px] font-semibold text-amber-dark">
                {loading && (
                  <span className="h-3 w-3 shrink-0 animate-spin rounded-full border-2 border-amber/30 border-t-amber-dark" />
                )}
                Diagnostics are busy — retrying…
              </p>
              <p className="mt-1 break-words text-[11px] leading-5 text-cream-600">
                {error}
              </p>
            </div>
            <button
              onClick={() => void runDoctor()}
              disabled={loading}
              className="inline-flex shrink-0 items-center gap-1.5 rounded-lg border border-cream-200 bg-white px-2.5 py-1 text-[11px] font-semibold text-cream-600 hover:border-teal-light hover:text-teal-dark disabled:cursor-not-allowed disabled:opacity-60"
            >
              <RefreshCw className={`h-3 w-3 ${loading ? "animate-spin" : ""}`} />
              Retry
            </button>
          </div>
        </div>
      )}

      {report ? (
        <div className="space-y-2">
          {(report.checks ?? []).map((check) => (
            <DoctorRow key={check.id} check={check} />
          ))}
          {(report.checks ?? []).length === 0 && (
            <p className="text-[12px] text-cream-400">
              No checks were reported.
            </p>
          )}
        </div>
      ) : loading ? (
        <div className="flex items-center gap-2 text-[12px] text-cream-400">
          <div className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-cream-300 border-t-teal-dark" />
          Running diagnostics…
        </div>
      ) : error ? null : (
        <p className="text-[12px] text-cream-400">No health report yet.</p>
      )}
    </section>
  );
}

function DoctorRow({ check }: { check: OracleDoctorCheck }) {
  const label = CHECK_LABELS[check.id] ?? check.id;
  // Failing rows are tinted so the eye lands on what's broken; the remediation
  // is the load-bearing copy, rendered as a prominent highlighted "→" line.
  return (
    <div
      className={`flex items-start gap-2 rounded-xl px-3 py-2 ${
        check.ok ? "bg-cream-50" : "border border-coral/30 bg-coral/[0.06]"
      }`}
    >
      {check.ok ? (
        <CheckCircle2 className="mt-0.5 h-4 w-4 shrink-0 text-sage-dark" />
      ) : (
        <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-coral-dark" />
      )}
      <div className="min-w-0 flex-1">
        <p className="text-[12px] font-semibold text-cream-700">{label}</p>
        {/* Always show the per-check DETAIL — never just an icon. */}
        <p className="text-[11px] leading-5 text-cream-500">
          {check.detail || (check.ok ? "Healthy." : "Check failed.")}
        </p>
        {!check.ok && (
          <p className="mt-1.5 rounded-lg bg-coral/10 px-2 py-1 text-[11px] font-semibold leading-5 text-coral-dark">
            → {check.remediation ?? "Run the doctor again, or open the runtime panel to repair this."}
          </p>
        )}
      </div>
    </div>
  );
}
