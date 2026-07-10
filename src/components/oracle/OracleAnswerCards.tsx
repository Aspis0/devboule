// Shared Oracle answer/error card components.
//
// Extracted from OracleView so both OracleView (the standalone page) and the
// Polis OracleAskPanel can render the same answer/error UI without duplication.
// Neither component has any state or side-effects — they are pure presentation.

import { AlertTriangle, FileText, FolderOpen, Sparkles, Stethoscope } from "lucide-react";
import type { OracleAnswer, OracleError, OracleErrorKind } from "../../types/backend";
import { useAppContext } from "../../context/AppContext";

// ---------------------------------------------------------------------------
// AnswerCard
// ---------------------------------------------------------------------------

export interface AnswerCardProps {
  answer: OracleAnswer;
  /** Optional handler called when a citation chip is clicked. */
  onCitationClick?: (fileSource: string) => void;
}

export function AnswerCard({ answer, onCitationClick }: AnswerCardProps) {
  return (
    <div
      className={`mt-4 rounded-xl border p-4 ${
        answer.notFound
          ? "border-amber/20 bg-amber/10"
          : "border-cream-200 bg-cream-50"
      }`}
    >
      <p className="text-[13px] leading-relaxed text-cream-800">
        {answer.answer ||
          answer.summary ||
          (answer.notFound ? "No matching context found." : "Empty answer.")}
      </p>

      {/* The extractive (retrieval-only) degrade reason — the only fallback. */}
      {answer.fallbackReason && (
        <p className="mt-2 rounded-lg bg-amber/10 px-2 py-1 text-[10px] leading-4 text-amber-dark">
          {answer.fallbackReason}
        </p>
      )}

      {(answer.citations ?? []).length > 0 && (
        <div className="mt-3 flex flex-wrap gap-2">
          {answer.citations.map((citation, idx) => {
            const key = `${citation.chunkId ?? citation.fileSource}-${citation.ref ?? citation.startChar ?? idx}-${idx}`;
            const content = (
              <>
                <FileText className="h-3 w-3" />
                {citation.fileSource}:{citation.startChar ?? "?"}-
                {citation.endChar ?? "?"}
              </>
            );
            const className =
              "inline-flex items-center gap-1 rounded-lg border border-cream-200 bg-white px-2 py-1 font-mono text-[10px] font-semibold text-cream-600";
            if (onCitationClick) {
              return (
                <button
                  key={key}
                  onClick={() => onCitationClick(citation.fileSource)}
                  className={`${className} cursor-pointer hover:border-teal-light hover:text-teal-dark`}
                  title={`${citation.fileSource}#chunk-${citation.chunkIndex ?? "?"}`}
                >
                  {content}
                </button>
              );
            }
            return (
              <span
                key={key}
                className={className}
                title={`${citation.fileSource}#chunk-${citation.chunkIndex ?? "?"}`}
              >
                {content}
              </span>
            );
          })}
        </div>
      )}

      {answer.notFound && answer.suggestedPath && (
        <p className="mt-2 font-mono text-[10px] text-amber-dark">
          Possible path: {answer.suggestedPath}
        </p>
      )}

      {(answer.llmProvider || answer.llmModel) && (
        <p className="mt-3 text-[10px] text-cream-400">
          Answer by {answer.llmProvider ?? "llm"} · {answer.llmModel ?? "model"}
          {answer.answerSource ? ` · ${answer.answerSource}` : ""} ·{" "}
          {(answer.citations ?? []).length} sources
        </p>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// AskErrorCard
// ---------------------------------------------------------------------------

export interface AskErrorCardProps {
  error: OracleError;
  onChooseFolder: () => void;
  onRunDoctor: () => void;
  onConfigureProvider: () => void;
}

export function AskErrorCard({
  error,
  onChooseFolder,
  onRunDoctor,
  onConfigureProvider,
}: AskErrorCardProps) {
  const kind: OracleErrorKind = error.kind;
  // A clean, app-wide indexing signal: an active index job (or pending files)
  // means Oracle is busy building the search index — a healthy, transient
  // state — NOT actually down. Use it to soften transient-availability errors.
  const { oracleIndexStatus } = useAppContext();
  const isIndexing =
    oracleIndexStatus?.watcherRunning === true &&
    (!!oracleIndexStatus?.job || (oracleIndexStatus?.index?.pendingFiles ?? 0) > 0);

  const transientAvailability =
    kind === "serverUnavailable" ||
    kind === "embedderUnavailable" ||
    kind === "pythonError";

  // While indexing, transient "unavailable" errors are just the index being
  // built — show a calm message instead of a hard error.
  const indexingTitle = "Oracle is indexing your workspace";
  const indexingBody =
    "It's building the search index right now — ask again in a moment. This is normal, not an error.";
  const showIndexingCopy = transientAvailability && isIndexing;

  return (
    <div className="mt-4 rounded-xl border border-coral/30 bg-coral/5 p-4">
      <div className="flex items-start gap-3">
        <div className="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-coral/15">
          <AlertTriangle className="h-4 w-4 text-coral-dark" />
        </div>
        <div className="min-w-0">
          <p className={`text-[13px] font-semibold ${showIndexingCopy ? "text-amber-dark" : "text-coral-dark"}`}>
            {showIndexingCopy ? indexingTitle : error.message}
          </p>
          {error.remediation && (
            <p className="mt-2 text-[11px] leading-5 text-cream-500">
              Remediation: {error.remediation}
            </p>
          )}
          {showIndexingCopy && (
            <p className="mt-2 text-[11px] leading-5 text-cream-500">
              {indexingBody}
            </p>
          )}
          {kind === "indexEmpty" && isIndexing && (
            <p className="mt-2 text-[11px] leading-5 text-cream-500">
              Indexing may still be in progress — try again in a moment.
            </p>
          )}
          <div className="mt-3 flex flex-wrap gap-2">
            {kind === "noWorkspaceRoot" && (
              <button
                onClick={onChooseFolder}
                className="inline-flex items-center gap-1.5 rounded-lg bg-terracotta px-3 py-1.5 text-[12px] font-semibold text-white hover:bg-terracotta-600"
              >
                <FolderOpen className="h-3.5 w-3.5" />
                Choose folder
              </button>
            )}
            {kind === "missingApiKey" && (
              <button
                onClick={onConfigureProvider}
                className="inline-flex items-center gap-1.5 rounded-lg bg-terracotta px-3 py-1.5 text-[12px] font-semibold text-white hover:bg-terracotta-600"
              >
                <Sparkles className="h-3.5 w-3.5" />
                Configure provider →
              </button>
            )}
            {(kind === "indexEmpty" ||
              kind === "internal" ||
              (!isIndexing && transientAvailability)) && (
              <button
                onClick={onRunDoctor}
                className="inline-flex items-center gap-1.5 rounded-lg border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-600 hover:border-teal-light hover:text-teal-dark"
              >
                <Stethoscope className="h-3.5 w-3.5" />
                Run doctor
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
