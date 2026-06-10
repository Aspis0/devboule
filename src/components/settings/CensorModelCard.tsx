import {
  AlertTriangle,
  CheckCircle2,
  ShieldCheck,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  invokeBackendCommand,
  useAppActions,
  useAppContext,
} from "../../context/AppContext";
import { CENSOR_MODEL_MAX_LENGTH } from "../projects/censorLocalAi";

// Phase 5 — Settings → Providers & Models "Censor model" card.
//
// A SMALL, focused card for the optional Ollama MODEL OVERRIDE Censor uses for its
// tier-2 (Gemma) review. This is intentionally separate from the existing
// CensorLocalAiCard (which picks the PROVIDER: Ollama vs a local oMLX server). Here
// the user only overrides which Ollama tag Censor pulls; the default is gemma4:e4b
// and the backend falls back to e2b when e4b is not pulled.
//
// PERSISTENCE CONTRACT: the Rust backend reads the Ollama override from the dedicated
// `ollamaModel` field on set_censor_local_ai. This card writes ONLY that field through the
// SAME command the provider card uses:
//   set_censor_local_ai({ config: { provider: "ollama", ollamaModel } })
// It deliberately does NOT send `model`: the Ollama branch of the Rust resolver reads only
// `ollama_model`, so a `model` value would be dead config noise AND would clobber a prior
// oMLX `model` left in config.json.
//
// READ PATH: config.censorLocalAi.ollamaModel is the persisted Ollama tag (the untyped
// get_config passthrough). We still tolerate a legacy `model` value on read (older configs
// that predate the dedicated field) and fall back to "" when neither is present.
const DEFAULT_OLLAMA_MODEL = "gemma4:e4b";

// Coerce an untyped raw value to a string ("" for anything non-string) — the config
// passthrough is serde_json::Value, so a hand-edited config must not break the input.
function rawString(raw: unknown): string {
  return typeof raw === "string" ? raw : "";
}

export function CensorModelCard() {
  const { config } = useAppContext();
  const { refreshConfig } = useAppActions();
  // Untyped passthrough (get_config returns serde_json::Value); coerce every field.
  const current = (config.censorLocalAi ?? null) as
    | { model?: unknown; ollamaModel?: unknown }
    | null;
  // Prefer the new `ollamaModel` field if the backend already exposes it; else the
  // existing `model` field; else empty (placeholder shows the default).
  const seeded =
    rawString(current?.ollamaModel) || rawString(current?.model) || "";

  const [model, setModel] = useState(seeded);
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

  // Reflect a config change made elsewhere (or after a save) into the draft.
  useEffect(() => {
    setModel(rawString(current?.ollamaModel) || rawString(current?.model) || "");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    rawString(current?.ollamaModel),
    rawString(current?.model),
  ]);

  const tooLong = model.trim().length > CENSOR_MODEL_MAX_LENGTH;

  const onSave = useCallback(async () => {
    if (tooLong) return;
    const trimmed = model.trim();
    setBusy(true);
    setError(null);
    try {
      // Send ONLY `ollamaModel` — the field the Ollama resolver reads. Sending `model`
      // too would be dead config (the Ollama branch ignores it) and could clobber a
      // prior oMLX `model`. An empty value clears the override (back to gemma4:e4b).
      await invokeBackendCommand("set_censor_local_ai", {
        config: {
          provider: "ollama",
          ollamaModel: trimmed || undefined,
        },
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
            : "Could not save the Censor model override.",
        );
      }
    } finally {
      if (mountedRef.current) setBusy(false);
    }
  }, [model, tooLong, refreshConfig]);

  return (
    <section
      className="rounded-2xl border border-cream-200 bg-white p-4"
      data-help-title="Censor's tier-2 review runs a local Gemma model on Ollama."
      data-help-lines="Leave this blank to use the default gemma4:e4b.|If e4b is not pulled, Censor falls back to the lighter e2b automatically.|Override only if you pulled a different Gemma tag and want Censor to use it.|Stored in your local config.json; this is just the Ollama model tag, not the provider."
    >
      <div className="mb-3 flex items-center gap-2">
        <ShieldCheck className="h-4 w-4 text-teal" />
        <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          Censor model
        </h3>
      </div>
      <p className="mb-4 max-w-3xl text-[12px] leading-5 text-cream-500">
        Override the local Ollama model Censor uses for its optional tier-2
        (Gemma) review. Leave blank to use the default.
      </p>

      <div className="grid gap-3 rounded-2xl border border-cream-200 p-3">
        <label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
          Ollama model tag
          <input
            value={model}
            onChange={(event) => setModel(event.target.value)}
            placeholder={DEFAULT_OLLAMA_MODEL}
            maxLength={CENSOR_MODEL_MAX_LENGTH + 1}
            className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
          />
          <span className="mt-1 block text-[10px] normal-case leading-4 tracking-normal text-cream-400">
            Leave empty to auto-select{" "}
            <code className="font-mono">gemma4:e4b</code>, falling back to{" "}
            <code className="font-mono">gemma4:e2b</code> if only that is
            installed. When you set an override, it is used verbatim — no
            fallback.
          </span>
          {tooLong && (
            <span className="mt-1 block text-[10px] normal-case tracking-normal text-coral-dark">
              Model must be at most {CENSOR_MODEL_MAX_LENGTH} characters.
            </span>
          )}
        </label>

        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => void onSave()}
            disabled={busy || tooLong}
            className="inline-flex items-center gap-2 rounded-md bg-teal px-3 py-2 text-[12px] font-semibold text-white hover:bg-teal/90 disabled:opacity-60"
          >
            <CheckCircle2 className="h-3.5 w-3.5" />
            {savedTick ? "Saved" : "Save model"}
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
