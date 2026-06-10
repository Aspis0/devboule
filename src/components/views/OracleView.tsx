// OracleView — the STANDALONE Oracle page (restored).
//
// The full Oracle surface, as its own nav entry: an ask section (search + seed
// chips + answer/error rendering) on top, and the operator admin panel (health,
// doctor, indexed files, index preferences, workspace picker) below.
//
// Recomposition — every piece already exists as an extracted module:
//   - AnswerCard / AskErrorCard  ← components/oracle/OracleAnswerCards
//   - OracleAdminPanel           ← components/oracle/OracleAdminPanel
//   - deriveProviderConfigured   ← components/oracle/oracleProviderState
//   - seedQuestions              ← components/polis/oracleSuggestions
// This page DUPLICATES none of that logic; it only wires the ask flow to the
// shared cards and stacks the admin panel beneath it.
//
// The Polis parchment ask panel (components/polis/OracleAskPanel) is a SEPARATE,
// additional surface and is intentionally left untouched.

import { useCallback, useEffect, useRef, useState } from "react";
import { BrainCircuit, Sparkles } from "lucide-react";
import { useAppContext } from "../../context/AppContext";
import { toOracleError } from "../../utils/oracleError";
import { AnswerCard, AskErrorCard } from "../oracle/OracleAnswerCards";
import { OracleAdminPanel } from "../oracle/OracleAdminPanel";
import { deriveProviderConfigured } from "../oracle/oracleProviderState";
import { seedQuestions } from "../polis/oracleSuggestions";
import type { OracleAnswer, OracleError } from "../../types/backend";

export function OracleView() {
  const { askOracle, requestView, oracleLlmSettings, secretStatuses } =
    useAppContext();

  // All hooks declared unconditionally and BEFORE any early return so the hook
  // order is stable across renders (no conditional-hook bug).
  const [query, setQuery] = useState(seedQuestions[0] ?? "");
  const [answer, setAnswer] = useState<OracleAnswer | null>(null);
  const [askError, setAskError] = useState<OracleError | null>(null);
  const [querying, setQuerying] = useState(false);

  // Guards async setState after unmount.
  const mountedRef = useRef(true);
  // The admin section below; "Choose folder" / "Run doctor" scroll the user to
  // it (the workspace picker + doctor live there) instead of duplicating those
  // controls in the ask section.
  const adminRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Lightweight, model-free provider gate (shared with the admin panel + the
  // Polis ask panel so all three agree).
  const providerConfigured = deriveProviderConfigured(
    oracleLlmSettings ?? null,
    secretStatuses,
  );

  const runQuery = useCallback(
    async (nextQuery: string) => {
      const trimmed = nextQuery.trim();
      if (trimmed.length < 3) return;
      if (!providerConfigured) return;
      setQuery(trimmed);
      setQuerying(true);
      setAnswer(null);
      setAskError(null);
      try {
        const result = await askOracle(trimmed, 8);
        if (!mountedRef.current) return;
        setAnswer(result);
        setAskError(null);
      } catch (e) {
        if (!mountedRef.current) return;
        setAskError(toOracleError(e));
      } finally {
        if (mountedRef.current) setQuerying(false);
      }
    },
    [askOracle, providerConfigured],
  );

  // The Oracle answer-provider config now lives in Settings → Providers & Models
  // (Phase 5: settings tab id "providers"). The AskErrorCard "Configure provider"
  // action and the inline gate hint both deep-link there.
  const goToProviderSettings = useCallback(() => {
    requestView("settings", "providers");
  }, [requestView]);

  // "Choose folder" / "Run doctor" land the user on the admin section of THIS
  // page, which owns the workspace picker and the doctor.
  const scrollToAdmin = useCallback(() => {
    adminRef.current?.scrollIntoView({ behavior: "smooth", block: "start" });
  }, []);

  return (
    <div className="space-y-6">
      {/* Page header */}
      <div className="flex items-center gap-3">
        <div className="flex h-11 w-11 items-center justify-center rounded-2xl bg-teal/10">
          <BrainCircuit className="h-5 w-5 text-teal-dark" />
        </div>
        <div>
          <h2 className="text-[16px] font-bold tracking-tight text-cream-800">
            Oracle
          </h2>
          <p className="text-[12px] text-cream-500">
            Ask about your codebase, then manage the retrieval index below
          </p>
        </div>
      </div>

      {/* ASK SECTION */}
      <section className="rounded-2xl border border-cream-200 bg-white p-5">
        <div className="flex gap-2">
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void runQuery(query);
            }}
            placeholder="Ask about your codebase…"
            data-help-title="This asks Oracle a question about your codebase."
            data-help-lines="Oracle retrieves relevant chunks from the indexed workspace and answers with citations.|Answers need a configured provider and a populated index.|Click a citation to jump to the cited file.|If it fails, the error card links to the fix (folder, doctor, or provider)."
            className="min-w-0 flex-1 rounded-xl border border-cream-200 bg-white px-3 py-2 text-[13px] text-cream-700 outline-none focus:border-teal-light placeholder:text-cream-400"
          />
          <button
            onClick={() => void runQuery(query)}
            disabled={querying || query.trim().length < 3 || !providerConfigured}
            className="inline-flex items-center gap-1.5 rounded-xl bg-terracotta px-3 py-2 text-[12px] font-semibold text-white hover:bg-terracotta-500 disabled:cursor-not-allowed disabled:opacity-60"
          >
            <Sparkles className="h-3.5 w-3.5" />
            Ask
          </button>
        </div>

        {/* Provider-not-configured gate */}
        {!providerConfigured && (
          <p className="mt-2 text-[11px] text-cream-500">
            Oracle answers require a provider.{" "}
            <button
              onClick={goToProviderSettings}
              className="font-semibold text-terracotta-500 underline hover:text-terracotta-600"
            >
              Configure Oracle provider →
            </button>
          </p>
        )}

        {/* Seed-question chips */}
        <div className="mt-2.5 flex flex-wrap gap-1.5">
          {seedQuestions.map((s) => (
            <button
              key={s}
              onClick={() => void runQuery(s)}
              disabled={querying || !providerConfigured}
              className="rounded-lg border border-cream-200 bg-white px-2.5 py-1 text-[11px] font-medium text-cream-500 hover:bg-cream-100 hover:text-cream-700 disabled:cursor-not-allowed disabled:opacity-60"
            >
              {s}
            </button>
          ))}
        </div>

        {/* Answer / loading / error */}
        {querying ? (
          <div className="mt-4 flex items-center gap-2 text-[12px] text-cream-400">
            <div className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-cream-200 border-t-terracotta" />
            Querying Oracle…
          </div>
        ) : askError ? (
          <AskErrorCard
            error={askError}
            onChooseFolder={scrollToAdmin}
            onRunDoctor={scrollToAdmin}
            onConfigureProvider={goToProviderSettings}
          />
        ) : answer ? (
          <AnswerCard answer={answer} />
        ) : null}
      </section>

      {/* ADMIN SECTION — health, doctor, indexed files, index prefs, workspace */}
      <div ref={adminRef}>
        <OracleAdminPanel />
      </div>
    </div>
  );
}

export default OracleView;
