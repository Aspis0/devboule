// Polis P1.4 — store-connected anomaly ledger section (lazy-loaded).
//
// Separated from InspectSidebar so the cityStore import is deferred (avoids
// triggering module-level `isTauriRuntime()` in node test environments).

import { useState, useCallback, useMemo } from "react";
import { Flame, AlertTriangle, Wrench } from "lucide-react";
import type { SinRecord, UrbanSin } from "../../types/city";
import { useCityStore } from "../../store/cityStore";
import { buildAnomalyLedgerModel } from "./anomalyLedgerModel";

const SIN_TONE: Record<string, string> = {
  smoke: "text-cream-600 bg-cream-100 border-cream-300",
  fire: "text-amber-dark bg-amber/10 border-amber/30",
  inferno: "text-coral-dark bg-coral/10 border-coral/40",
};

function SectionTitle({
  icon,
  children,
}: {
  icon: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <h4 className="mb-2 flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-cream-400">
      {icon} {children}
    </h4>
  );
}

/** Inline error line (amber, one-line) shown under an action row. */
function ActionError({ text }: { text: string }) {
  if (!text) return null;
  return <p className="mt-1 pl-6 text-[11px] text-amber-dark">{text}</p>;
}

/** Single OPEN sin row with action buttons. */
function SinRow({
  sin,
  relPath,
  sinActionPending,
  disposeSin,
  fixSin,
}: {
  sin: SinRecord;
  relPath: string;
  sinActionPending: string[];
  disposeSin: (
    relPath: string,
    sinId: string,
    disposition: "open" | "ignored",
  ) => Promise<string | null>;
  fixSin: (relPath: string, sinId: string) => Promise<string | null>;
}) {
  const [error, setError] = useState<string | null>(null);
  const pending = sinActionPending.includes(sin.id);
  const SinIcon = sin.severity === "smoke" ? AlertTriangle : Flame;

  const handleIgnore = useCallback(async () => {
    setError(null);
    const err = await disposeSin(relPath, sin.id, "ignored");
    if (err) setError(err);
  }, [disposeSin, relPath, sin.id]);

  const handleFix = useCallback(async () => {
    setError(null);
    const err = await fixSin(relPath, sin.id);
    if (err) setError(err);
  }, [fixSin, relPath, sin.id]);

  return (
    <li className="flex flex-col">
      <div
        className={`flex items-start gap-2 rounded-xl border px-3 py-2 text-[12px] ${
          SIN_TONE[sin.severity] ?? SIN_TONE.smoke
        }`}
      >
        <SinIcon className="mt-0.5 h-3.5 w-3.5 shrink-0" />
        <div className="min-w-0 flex-1">
          <span className="font-semibold capitalize">{sin.severity}</span>{" "}
          <span className="ml-1 rounded bg-cream-200 px-1.5 py-0.5 font-mono text-[10px] text-cream-600">
            {sin.ruleId}
          </span>
          {sin.line != null && (
            <span className="ml-1 font-mono text-[10px] text-cream-500">
              :{sin.line}
            </span>
          )}
          <span className="ml-1">{sin.description}</span>
          {sin.evidence && (
            <p className="mt-0.5 line-clamp-2 text-[11px] text-cream-500 italic">
              {sin.evidence}
            </p>
          )}
        </div>
        {/* Actions (right-aligned) */}
        <div className="flex shrink-0 items-center gap-1">
          {sin.fixDirectiveId ? (
            <span
              className="rounded bg-amber/20 px-1.5 py-0.5 text-[10px] italic text-amber-dark"
              title={`Directive ${sin.fixDirectiveId}`}
            >
              fix dispatched
            </span>
          ) : (
            <button
              type="button"
              onClick={handleFix}
              disabled={pending}
              title="Send to coder"
              className="rounded px-1.5 py-0.5 text-[11px] text-cream-600 transition hover:bg-cream-200 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              <Wrench className="h-3 w-3" />
            </button>
          )}
          <button
            type="button"
            onClick={handleIgnore}
            disabled={pending}
            title="Ignore"
            className="rounded px-1.5 py-0.5 text-[11px] text-cream-600 transition hover:bg-cream-200 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            Ignore
          </button>
          {pending && (
            <span className="h-3 w-3 animate-spin rounded-full border-2 border-cream-300 border-t-terracotta" />
          )}
        </div>
      </div>
      <ActionError text={error ?? ""} />
    </li>
  );
}

/** Single ignored sin row with Un-ignore button. */
function IgnoredRow({
  sin,
  relPath,
  sinActionPending,
  disposeSin,
}: {
  sin: SinRecord;
  relPath: string;
  sinActionPending: string[];
  disposeSin: (
    relPath: string,
    sinId: string,
    disposition: "open" | "ignored",
  ) => Promise<string | null>;
}) {
  const [error, setError] = useState<string | null>(null);
  const pending = sinActionPending.includes(sin.id);
  const SinIcon = sin.severity === "smoke" ? AlertTriangle : Flame;

  const handleUnignore = useCallback(async () => {
    setError(null);
    const err = await disposeSin(relPath, sin.id, "open");
    if (err) setError(err);
  }, [disposeSin, relPath, sin.id]);

  return (
    <li className="flex flex-col">
      <div
        className={`flex items-start gap-2 rounded-xl border px-3 py-2 text-[12px] opacity-60 ${
          SIN_TONE[sin.severity] ?? SIN_TONE.smoke
        }`}
      >
        <SinIcon className="mt-0.5 h-3.5 w-3.5 shrink-0" />
        <div className="min-w-0 flex-1">
          <span className="font-semibold capitalize">{sin.severity}</span>{" "}
          <span className="ml-1 rounded bg-cream-200 px-1.5 py-0.5 font-mono text-[10px] text-cream-600">
            {sin.ruleId}
          </span>
          {sin.line != null && (
            <span className="ml-1 font-mono text-[10px] text-cream-500">
              :{sin.line}
            </span>
          )}
          <span className="ml-1">{sin.description}</span>
          {sin.evidence && (
            <p className="mt-0.5 line-clamp-2 text-[11px] text-cream-500 italic">
              {sin.evidence}
            </p>
          )}
        </div>
        <button
          type="button"
          onClick={handleUnignore}
          disabled={pending}
          title="Un-ignore"
          className="shrink-0 rounded px-1.5 py-0.5 text-[11px] text-cream-600 transition hover:bg-cream-200 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          Un-ignore
        </button>
        {pending && (
          <span className="h-3 w-3 animate-spin rounded-full border-2 border-cream-300 border-t-terracotta" />
        )}
      </div>
      <ActionError text={error ?? ""} />
    </li>
  );
}

/** Collapsible ignored sins section. */
function IgnoredSection({
  records,
  relPath,
  sinActionPending,
  disposeSin,
}: {
  records: SinRecord[];
  relPath: string;
  sinActionPending: string[];
  disposeSin: (
    relPath: string,
    sinId: string,
    disposition: "open" | "ignored",
  ) => Promise<string | null>;
}) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div className="mt-2">
      <button
        type="button"
        onClick={() => setExpanded((v) => !v)}
        aria-expanded={expanded}
        className="text-[11px] text-cream-400 transition hover:text-cream-600"
      >
        {records.length} ignored {expanded ? "▲" : "▼"}
      </button>
      {expanded && (
        <ul className="mt-1 space-y-1">
          {records.map((s) => (
            <IgnoredRow
              key={s.id}
              sin={s}
              relPath={relPath}
              sinActionPending={sinActionPending}
              disposeSin={disposeSin}
            />
          ))}
        </ul>
      )}
    </div>
  );
}

/** The list of open sin rows (or the old building.sins fallback). */
function AnomalyLedgerRows({
  records,
  fallbackSins,
  relPath,
  sinActionPending,
  disposeSin,
  fixSin,
}: {
  records: SinRecord[] | null;
  fallbackSins: UrbanSin[] | null;
  relPath: string;
  sinActionPending: string[];
  disposeSin: (
    relPath: string,
    sinId: string,
    disposition: "open" | "ignored",
  ) => Promise<string | null>;
  fixSin: (relPath: string, sinId: string) => Promise<string | null>;
}) {
  // Ledger-driven rows
  if (records && records.length > 0) {
    return (
      <ul className="space-y-1.5">
        {records.map((s) => (
          <SinRow
            key={s.id}
            sin={s}
            relPath={relPath}
            sinActionPending={sinActionPending}
            disposeSin={disposeSin}
            fixSin={fixSin}
          />
        ))}
      </ul>
    );
  }
  // Fallback: old building.sins (visual-layer, open-only, no ruleId)
  if (fallbackSins && fallbackSins.length > 0) {
    return (
      <ul className="space-y-1.5">
        {fallbackSins.map((s) => {
          const SinIcon = s.severity === "smoke" ? AlertTriangle : Flame;
          return (
            <li
              key={s.sinId}
              className={`flex items-start gap-2 rounded-xl border px-3 py-2 text-[12px] ${
                SIN_TONE[s.severity] ?? SIN_TONE.smoke
              }`}
            >
              <SinIcon className="mt-0.5 h-3.5 w-3.5 shrink-0" />
              <span>
                <span className="font-semibold capitalize">{s.severity}</span>{" "}
                — {s.description}
              </span>
            </li>
          );
        })}
      </ul>
    );
  }
  return null;
}

/**
 * Store-connected anomaly ledger section. Consumes the sin ledger from the
 * Zustand store and renders the full Issues section for a building.
 *
 * This component is lazy-loaded by InspectSidebar to avoid triggering the
 * cityStore module-level code in node test environments.
 */
export default function AnomalySection({
  buildingFilePath,
  buildingSins,
}: {
  buildingFilePath: string;
  buildingSins: UrbanSin[];
}) {
  const sinRecords = useCityStore((s) => s.sinRecords);
  const sinActionPending = useCityStore((s) => s.sinActionPending);
  const disposeSin = useCityStore((s) => s.disposeSin);
  const fixSin = useCityStore((s) => s.fixSin);
  const ledger = useMemo(
    () => buildAnomalyLedgerModel(sinRecords ?? [], buildingFilePath),
    [sinRecords, buildingFilePath],
  );
  const sinRecordsLoaded = sinRecords !== null;
  const hasOpen = ledger.open.length > 0;
  const hasSins = buildingSins.length > 0;
  const showSection = hasOpen || (!sinRecordsLoaded && hasSins);
  if (!showSection) return null;

  const totalIssues = hasOpen ? ledger.open.length : buildingSins.length;

  return (
    <section className="mt-4">
      <SectionTitle icon={<Flame className="h-3.5 w-3.5" />}>
        Issues ({totalIssues})
      </SectionTitle>

      <AnomalyLedgerRows
        records={sinRecordsLoaded ? ledger.open : null}
        fallbackSins={sinRecordsLoaded ? null : buildingSins}
        relPath={buildingFilePath}
        sinActionPending={sinActionPending}
        disposeSin={disposeSin}
        fixSin={fixSin}
      />

      {/* Ignored section toggle */}
      {sinRecordsLoaded && ledger.ignored.length > 0 && (
        <IgnoredSection
          records={ledger.ignored}
          relPath={buildingFilePath}
          sinActionPending={sinActionPending}
          disposeSin={disposeSin}
        />
      )}

      {/* Fixed count — informational only */}
      {sinRecordsLoaded && ledger.fixedCount > 0 && (
        <p className="mt-1.5 text-[11px] italic text-cream-400">
          {ledger.fixedCount} fixed by the checker
        </p>
      )}
    </section>
  );
}
