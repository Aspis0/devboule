import {
  AlertTriangle,
  CheckCircle2,
  Cpu,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  invokeBackendCommand,
  useAppActions,
  useAppContext,
} from "../../context/AppContext";
import {
  validateLocalBackend,
  LOCAL_MODEL_MAX_LENGTH,
  LOCAL_BASE_URL_MAX_LENGTH,
} from "../agents/localCoderBackend";
import {
  buildProviderStatusMap,
  type ProviderStatusMap,
} from "../design/designProviderDetection";
import type {
  DetectedProvider,
  LocalCoderBackend,
  LocalCoderBackendKind,
} from "../../types/config";

// Settings → Providers card to configure the single global LOCAL MAIN-CODER backend — the
// model the Devboule orchestrator binary (the local MAIN coder, client === "orchestrator")
// runs on.
//
// TIER DISTINCTION (the whole point of this card): this is a SEPARATE backend from the
// Mini-coder card. The LOCAL MAIN CODER (orchestrator) and the MINI worker it delegates to
// are DISTINCT tiers with DISTINCT models. This card configures the MAIN coder; the
// Mini-coder card configures the delegated worker. They are independent config values.
//
// A discriminated form: pick the kind (Ollama / local oMLX) and fill its field. Validation
// is the SHARED pure helper (validateLocalBackend, which delegates to validateMiniBackend)
// so the UI and the Rust boundary (validate_local_coder_backend) never disagree. Persists
// through set_local_coder_backend (null clears it), then refreshes the global config.
//
// KINDS: only the two LOCAL HTTP runtimes the orchestrator binary can drive are offered —
// the binary's OmlxModel client POSTs to a loopback OpenAI-compatible endpoint, so there is
// no api/codex/appleFm option here.
export function LocalCoderBackendCard() {
  const { config } = useAppContext();
  const { refreshConfig } = useAppActions();
  const current = config.localCoderBackend ?? null;

  const [kind, setKind] = useState<LocalCoderBackendKind>(current?.kind ?? "ollama");
  const [model, setModel] = useState(current?.model ?? "");
  const [baseUrl, setBaseUrl] = useState(current?.baseUrl ?? "");
  const [detected, setDetected] = useState<DetectedProvider[] | null>(null);
  const detectId = useRef(0);
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const runDetect = useCallback(async () => {
    const id = detectId.current + 1;
    detectId.current = id;
    try {
      const result = await invokeBackendCommand<DetectedProvider[]>("detect_providers");
      if (mountedRef.current && detectId.current === id) {
        setDetected(Array.isArray(result) ? result : []);
      }
    } catch {
      // Degrade silently to a free-text input.
    }
  }, []);
  useEffect(() => {
    void runDetect();
  }, [runDetect]);

  const statusMap: ProviderStatusMap = useMemo(
    () => buildProviderStatusMap(detected),
    [detected],
  );
  const detectedModels = useMemo(
    () => (kind === "ollama" || kind === "omlx" ? statusMap[kind].models : []),
    [kind, statusMap],
  );

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedTick, setSavedTick] = useState(false);

  // Reflect a config change made elsewhere (or after a save) into the draft.
  useEffect(() => {
    setKind(current?.kind ?? "ollama");
    setModel(current?.model ?? "");
    setBaseUrl(current?.baseUrl ?? "");
  }, [current?.kind, current?.model, current?.baseUrl]);

  const validation = useMemo(
    () => validateLocalBackend({ kind, model, baseUrl }),
    [kind, model, baseUrl],
  );
  // Model is REQUIRED for both kinds, so surface its error even when empty (otherwise Save
  // just greys out with no inline reason for WHY).
  const showModelError = Boolean(validation.errors.model);
  // The omlx base URL is REQUIRED, so surface its error even when empty.
  const showBaseUrlError = kind === "omlx" && Boolean(validation.errors.baseUrl);

  const save = useCallback(
    async (next: LocalCoderBackend | null) => {
      setBusy(true);
      setError(null);
      try {
        await invokeBackendCommand<LocalCoderBackend | null>(
          "set_local_coder_backend",
          { backend: next },
        );
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
              : "Could not save the local main-coder backend.",
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

  const onClear = async () => {
    try {
      await save(null);
    } catch {
      // Error surfaced by save.
    }
  };

  return (
    <section
      className="rounded-2xl border border-cream-200 bg-white p-4"
      data-help-title="The local main-coder backend is the model the Devboule orchestrator (your local MAIN coder) runs on."
      data-help-lines="This is a DIFFERENT tier from the Mini-coder: the main coder is the orchestrator; the mini is the small worker it delegates to.|Ollama runs a local model; oMLX is a local MLX server exposing an OpenAI-compatible API.|The orchestrator only drives a loopback HTTP endpoint, so only local kinds are offered.|When unset, the orchestrator falls back to a safe local stub.|Stored in your local config.json under localCoderBackend."
    >
      <div className="mb-3 flex items-center gap-2">
        <Cpu className="h-4 w-4 text-teal" />
        <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          Local main coder (Devboule)
        </h3>
      </div>
      <p className="mb-3 max-w-3xl text-[12px] leading-5 text-cream-500">
        The model the local Devboule orchestrator — your <strong>main</strong>{" "}
        coder — runs on. This is a different tier from the Mini-coder below (the
        small worker it delegates to); the two have independent models. One global
        backend; pick the kind and fill its field.
      </p>

      {/* Privacy/safety disclosure — loopback only; the prompt stays on-device. */}
      <p className="mb-4 flex items-start gap-2 rounded-2xl border border-terracotta/30 bg-terracotta/[0.06] px-3 py-2 text-[11px] leading-4 text-cream-700">
        <ShieldCheck className="mt-0.5 h-3.5 w-3.5 shrink-0 text-terracotta" />
        <span>
          The orchestrator drives a loopback (on-device) endpoint only, so your
          code never leaves this machine for the local coder model.
        </span>
      </p>

      <div className="grid gap-3 rounded-2xl border border-cream-200 p-3 md:grid-cols-2">
        <label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
          Backend
          <select
            value={kind}
            onChange={(event) =>
              setKind(event.target.value as LocalCoderBackendKind)
            }
            className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
          >
            <option value="ollama">Ollama (local model)</option>
            <option value="omlx">oMLX (local MLX server)</option>
          </select>
        </label>

        <label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
          Model tag
          <input
            value={model}
            onChange={(event) => setModel(event.target.value)}
            placeholder="qwen2.5-coder"
            maxLength={LOCAL_MODEL_MAX_LENGTH}
            list={detectedModels.length ? "local-coder-detected-models" : undefined}
            className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
          />
          {detectedModels.length ? (
            <datalist id="local-coder-detected-models">
              {detectedModels.map((m) => (
                <option key={m} value={m} />
              ))}
            </datalist>
          ) : null}
          {showModelError && (
            <span className="mt-1 block text-[10px] normal-case tracking-normal text-coral-dark">
              {validation.errors.model}
            </span>
          )}
        </label>

        {kind === "omlx" ? (
          <label className="md:col-span-2 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
            Base URL
            <input
              value={baseUrl}
              onChange={(event) => setBaseUrl(event.target.value)}
              placeholder="http://localhost:8000/v1"
              maxLength={LOCAL_BASE_URL_MAX_LENGTH}
              className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
            />
            {showBaseUrlError && (
              <span className="mt-1 block text-[10px] normal-case tracking-normal text-coral-dark">
                {validation.errors.baseUrl}
              </span>
            )}
          </label>
        ) : null}

        {kind === "omlx" ? (
          <p className="md:col-span-2 text-[11px] leading-4 text-cream-400">
            Local oMLX OpenAI-compatible endpoint; loopback only (localhost,
            127.0.0.1 or [::1]). The orchestrator POSTs the prompt — which may
            carry file content — to this server, so a non-loopback host is refused
            to keep your code on this machine.
          </p>
        ) : null}

        {kind === "ollama" ? (
          <p className="md:col-span-2 text-[11px] leading-4 text-cream-400">
            The orchestrator drives your local Ollama server over its loopback
            OpenAI-compatible API. Make sure Ollama is running and the model tag
            is pulled.
          </p>
        ) : null}

        <div className="md:col-span-2 flex items-center gap-2">
          <button
            type="button"
            onClick={() => void onSave()}
            disabled={busy || !validation.ok}
            className="inline-flex items-center gap-2 rounded-md bg-teal px-3 py-2 text-[12px] font-semibold text-white hover:bg-teal/90 disabled:opacity-60"
          >
            <CheckCircle2 className="h-3.5 w-3.5" />
            {savedTick ? "Saved" : "Save backend"}
          </button>
          {current ? (
            <button
              type="button"
              onClick={() => void onClear()}
              disabled={busy}
              className="inline-flex items-center gap-1 rounded-md border border-cream-200 bg-white px-3 py-2 text-[11px] font-semibold text-cream-500 hover:border-coral/30 hover:text-coral-dark disabled:opacity-60"
            >
              <Trash2 className="h-3.5 w-3.5" />
              Clear
            </button>
          ) : null}
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

// Test-only alias kept for parity with the sibling cards.
export const __test_LocalCoderBackendCard = LocalCoderBackendCard;
