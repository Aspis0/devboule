// OracleAskPanel — a parchment-styled Oracle search panel for the Polis map.
//
// Rendered by PolisBottomBar when panelId === "oracle". Follows the SAME
// pointer-events discipline as LegendOverlay / FileTypesPanel:
//   - The outer wrapper (owned by PolisBottomBar) is `pointer-events-none`.
//   - THIS component itself carries `pointer-events-auto` so it captures events
//     while the canvas outside it stays fully interactive (pan/zoom pass through).
//
// Panel layout: Google-style search bar + suggestion chips + a parchment answer
// card + a close button. Parchment styling uses ONLY existing cream-* tokens
// (cream-50/100/200 layering, border-cream-200, shadow-soft-*) for a warm
// aged-paper look with no new tokens/images.
//
// Provider gate: if deriveProviderConfigured() returns false, the Ask button is
// disabled and a hint with a deep-link to provider settings is shown.
//
// Citation focus: clicking a citation chip calls onFocusFile(fileSource), which
// the parent (PolisView) maps to a building via findBuildingByCitation and
// focuses it in the renderer.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { X, Scroll, Sparkles } from "lucide-react";
import { useAppContext } from "../../context/AppContext";
import { toOracleError } from "../../utils/oracleError";
import type { OracleAnswer, OracleError } from "../../types/backend";
import { deriveProviderConfigured } from "../oracle/oracleProviderState";
import { AnswerCard, AskErrorCard } from "../oracle/OracleAnswerCards";
import {
  buildOracleSuggestions,
  seedQuestions,
} from "./oracleSuggestions";
import { useCityStore } from "../../store/cityStore";

// ---------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------

export interface OracleAskPanelProps {
  /** Called when the user clicks a citation chip. The string is the
   *  citation's `fileSource` (index-root-relative path). The parent (PolisView)
   *  resolves it to a building via findBuildingByCitation and focuses it. */
  onFocusFile: (fileSource: string) => void;
  /** Called when the user clicks the X / close button. */
  onClose: () => void;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function OracleAskPanel({ onFocusFile, onClose }: OracleAskPanelProps) {
  const { askOracle, requestView, oracleLlmSettings } = useAppContext();

  const cityState = useCityStore((s) => s.cityState);

  // All hooks unconditionally (no hooks-after-return / conditional hook bugs).
  const [query, setQuery] = useState(seedQuestions[0]);
  const [answer, setAnswer] = useState<OracleAnswer | null>(null);
  const [askError, setAskError] = useState<OracleError | null>(null);
  const [querying, setQuerying] = useState(false);

  const mountedRef = useRef(true);

  // mountedRef cleanup: set false on unmount so async callbacks don't setState
  // on an unmounted component.
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const suggestions = useMemo(
    () => buildOracleSuggestions(cityState),
    [cityState],
  );
  const providerConfigured = deriveProviderConfigured(
    oracleLlmSettings ?? null,
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

  const goToAdmin = useCallback(() => {
    requestView("oracle");
  }, [requestView]);

  // The Oracle LLM settings now live on the Oracle page (inside OracleAdminPanel),
  // not in Settings. Navigate to the Oracle page directly.
  const goToProviderSettings = useCallback(() => {
    requestView("oracle");
  }, [requestView]);

  // -------------------------------------------------------------------------
  // Render
  // -------------------------------------------------------------------------

  return (
    // pointer-events-auto re-enables interaction on the panel itself while the
    // outer wrapper (PolisBottomBar) is pointer-events-none so the canvas is
    // still interactive outside this panel.
    <div
      className="pointer-events-auto absolute bottom-16 left-1/2 w-[520px] max-w-[94vw] -translate-x-1/2 rounded-2xl border border-cream-200 bg-cream-50 shadow-soft-lg"
      data-testid="oracle-ask-panel"
    >
      {/* Parchment header: warm cream layering for an aged-paper feel. */}
      <div className="flex items-center justify-between border-b border-cream-200 bg-cream-100 px-4 py-2.5 rounded-t-2xl">
        <div className="flex items-center gap-2">
          <div className="flex h-7 w-7 items-center justify-center rounded-lg bg-terracotta-100 border border-terracotta-200">
            <Scroll className="h-3.5 w-3.5 text-terracotta-600" />
          </div>
          <div>
            <h4 className="text-[12px] font-semibold text-cream-700">
              Oracle
            </h4>
            <p className="text-[10px] text-cream-500">
              Ask about your codebase
            </p>
          </div>
        </div>
        <button
          onClick={onClose}
          className="rounded-full p-1 text-cream-400 hover:bg-cream-200 hover:text-cream-700"
          aria-label="Close Oracle panel"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>

      {/* Body */}
      <div className="px-4 pb-4 pt-3">
        {/* Search row */}
        <div className="flex gap-2">
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void runQuery(query);
            }}
            placeholder="Ask about your codebase…"
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

        {/* Suggestion chips */}
        <div className="mt-2.5 flex flex-wrap gap-1.5">
          {suggestions.map((s) => (
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
            onChooseFolder={goToAdmin}
            onRunDoctor={goToAdmin}
            onConfigureProvider={goToProviderSettings}
          />
        ) : answer ? (
          <AnswerCard answer={answer} onCitationClick={onFocusFile} />
        ) : null}
      </div>
    </div>
  );
}

export default OracleAskPanel;
