import {
  AlertTriangle,
  CheckCircle2,
  Cloud as CloudIcon,
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
import type { AuxCredentialStatus } from "../../types/backend";
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

  // CLOUD (opt-in) API-key surface. WRITE-ONLY, mirroring ExaSearchKeyCard: the value is
  // never read back; `get_cloud_llm_key_status` reports present/absent ONLY, `save_cloud_llm_key`
  // SETs it, `delete_cloud_llm_key` CLEARs it. The key lives ONLY in the OS vault — never in
  // config.json. Loaded lazily (only meaningful for the Cloud kind).
  const [cloudKeyStatus, setCloudKeyStatus] = useState<AuxCredentialStatus | null>(null);
  const [cloudKeyDraft, setCloudKeyDraft] = useState("");
  const cloudKeyInflightRef = useRef(false);

  const refreshCloudKey = useCallback(async () => {
    try {
      const next = await invokeBackendCommand<AuxCredentialStatus>(
        "get_cloud_llm_key_status",
      );
      if (mountedRef.current) setCloudKeyStatus(next);
    } catch {
      // Degrade silently — the consent/validation still gate the unsafe action.
    }
  }, []);
  useEffect(() => {
    void refreshCloudKey();
  }, [refreshCloudKey]);

  const hasCloudKey = cloudKeyStatus?.configured === true;

  // ACTIVE consent gate for Cloud mode (privacy). The warning paragraph is informative; this
  // checkbox is the explicit acknowledgement that content LEAVES the machine. Save is gated on
  // it for the cloud kind so a user cannot enable Cloud by passively ignoring the disclosure.
  // Reset to false whenever the kind is not "cloud" (mount, current-load, or a switch away)
  // so re-entering Cloud always re-requires a fresh acknowledgement.
  const [cloudConsentAck, setCloudConsentAck] = useState(false);
  useEffect(() => {
    if (kind !== "cloud") setCloudConsentAck(false);
  }, [kind]);

  const saveCloudKey = useCallback(async () => {
    const key = cloudKeyDraft.trim();
    if (!key || cloudKeyInflightRef.current) return;
    cloudKeyInflightRef.current = true;
    setBusy(true);
    setError(null);
    try {
      const next = await invokeBackendCommand<AuxCredentialStatus>(
        "save_cloud_llm_key",
        { key },
      );
      if (!mountedRef.current) return;
      setCloudKeyStatus(next);
      // The value lives only in the OS keychain on success; never retain the draft.
      setCloudKeyDraft("");
      if (!next.configured) {
        setError(next.message ?? "The Cloud API key was not accepted.");
      }
    } catch (e) {
      if (mountedRef.current) {
        setError(e instanceof Error ? e.message : "Saving the Cloud API key failed.");
      }
    } finally {
      cloudKeyInflightRef.current = false;
      if (mountedRef.current) setBusy(false);
    }
  }, [cloudKeyDraft]);

  const clearCloudKey = useCallback(async () => {
    if (cloudKeyInflightRef.current) return;
    cloudKeyInflightRef.current = true;
    setBusy(true);
    setError(null);
    try {
      const next = await invokeBackendCommand<AuxCredentialStatus>(
        "delete_cloud_llm_key",
      );
      if (mountedRef.current) {
        setCloudKeyStatus(next);
        setCloudKeyDraft("");
      }
    } catch (e) {
      if (mountedRef.current) {
        setError(e instanceof Error ? e.message : "Removing the Cloud API key failed.");
      }
    } finally {
      cloudKeyInflightRef.current = false;
      if (mountedRef.current) setBusy(false);
    }
  }, []);

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
  // Base URL: omlx is REQUIRED (its error shows even when empty); ollama is OPTIONAL, so a
  // baseUrl error is only set when the user typed a non-empty INVALID URL — show it whenever
  // present. The validator never emits a baseUrl error for an empty ollama field.
  const showBaseUrlError = Boolean(validation.errors.baseUrl);

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
        {kind === "cloud" ? (
          <CloudIcon className="h-4 w-4 text-coral-dark" />
        ) : (
          <Cpu className="h-4 w-4 text-teal" />
        )}
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

      {/* Privacy/safety disclosure. LOCAL kinds: loopback only, the prompt stays on-device.
          CLOUD kind: a MANDATORY consent warning that content LEAVES the machine. This is the
          privacy contract — the copy is the switch between "stays on your machine" and "leaves
          this machine to the provider". */}
      {kind === "cloud" ? (
        <p
          data-testid="cloud-consent-warning"
          className="mb-4 flex items-start gap-2 rounded-2xl border border-coral/40 bg-coral/[0.07] px-3 py-2 text-[11px] leading-4 text-coral-dark"
        >
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-coral-dark" />
          <span>
            <strong>Cloud mode sends your code off this machine.</strong> The
            orchestrator POSTs your prompts and file content to the configured
            cloud provider over the internet. Unlike the Local options, your code
            does <strong>not</strong> stay on this machine. Only enable Cloud if
            you accept sending this project&apos;s content to that third-party
            provider.
          </span>
        </p>
      ) : (
        <p className="mb-4 flex items-start gap-2 rounded-2xl border border-terracotta/30 bg-terracotta/[0.06] px-3 py-2 text-[11px] leading-4 text-cream-700">
          <ShieldCheck className="mt-0.5 h-3.5 w-3.5 shrink-0 text-terracotta" />
          <span>
            The orchestrator drives a loopback (on-device) endpoint only, so your
            code never leaves this machine for the local coder model.
          </span>
        </p>
      )}

      <div className="grid gap-3 rounded-2xl border border-cream-200 p-3 md:grid-cols-2">
        <label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
          Backend
          <select
            value={kind}
            onChange={(event) => {
              const next = event.target.value as LocalCoderBackendKind;
              setKind(next);
              // S8: omlx REQUIRES a base URL — prefill the standard loopback default when
              // the field is empty so it doesn't block Save with a "required" error
              // (still fully editable). Never overwrite a URL the user already typed.
              if (next === "omlx" && baseUrl.trim() === "") {
                setBaseUrl("http://localhost:8000/v1");
              }
            }}
            className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
          >
            <option value="ollama">Ollama (local model)</option>
            <option value="omlx">oMLX (local MLX server)</option>
            <option value="cloud">Cloud (remote API — leaves this machine)</option>
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

        <label className="md:col-span-2 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
          {kind === "ollama" ? "Base URL (optional)" : "Base URL"}
          <input
            value={baseUrl}
            onChange={(event) => setBaseUrl(event.target.value)}
            placeholder={
              kind === "omlx"
                ? "http://localhost:8000/v1"
                : kind === "cloud"
                  ? "https://openrouter.ai/api/v1"
                  : "http://localhost:11434/v1"
            }
            maxLength={LOCAL_BASE_URL_MAX_LENGTH}
            className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
          />
          {showBaseUrlError && (
            <span className="mt-1 block text-[10px] normal-case tracking-normal text-coral-dark">
              {validation.errors.baseUrl}
            </span>
          )}
        </label>

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
            is pulled. Leave the Base URL empty to use the default{" "}
            <span className="font-mono">http://localhost:11434/v1</span>, or set
            it (loopback http only) if Ollama listens on a non-default port.
          </p>
        ) : null}

        {kind === "cloud" ? (
          <>
            <p className="md:col-span-2 text-[11px] leading-4 text-cream-400">
              An https OpenAI-compatible endpoint (e.g. OpenRouter:{" "}
              <span className="font-mono">https://openrouter.ai/api/v1</span>).
              The orchestrator authenticates with the API key below and POSTs the
              prompt — which carries file content — to this provider. The key is
              write-only here: it goes straight to the OS keychain and is never
              shown back or written to your config.
            </p>
            <label className="md:col-span-2 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
              Cloud API key
              <div className="mt-1 flex flex-col gap-2 sm:flex-row sm:items-center">
                <input
                  type="password"
                  value={cloudKeyDraft}
                  onChange={(event) => {
                    setError(null);
                    setCloudKeyDraft(event.target.value);
                  }}
                  placeholder={
                    hasCloudKey
                      ? "Paste a new key to rotate"
                      : "Paste your provider API key"
                  }
                  autoComplete="off"
                  spellCheck={false}
                  className="min-w-0 flex-1 rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
                />
                <button
                  type="button"
                  onClick={() => void saveCloudKey()}
                  disabled={busy || cloudKeyDraft.trim().length === 0}
                  className="inline-flex items-center justify-center gap-1.5 rounded-md bg-teal px-3 py-2 text-[12px] font-semibold normal-case tracking-normal text-white hover:bg-teal/90 disabled:cursor-not-allowed disabled:opacity-60"
                >
                  <CheckCircle2 className="h-3.5 w-3.5" />
                  {hasCloudKey ? "Rotate key" : "Save key"}
                </button>
                {hasCloudKey ? (
                  <button
                    type="button"
                    onClick={() => void clearCloudKey()}
                    disabled={busy}
                    className="inline-flex items-center justify-center gap-1 rounded-md border border-cream-200 bg-white px-3 py-2 text-[11px] font-semibold normal-case tracking-normal text-cream-500 hover:border-coral/30 hover:text-coral-dark disabled:cursor-not-allowed disabled:opacity-60"
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                    Clear key
                  </button>
                ) : null}
              </div>
              <span className="mt-1 block text-[10px] normal-case tracking-normal text-cream-400">
                {hasCloudKey
                  ? "A key is saved (hidden). Required for Cloud mode."
                  : "No key saved — Cloud mode needs a key before the orchestrator can run."}
              </span>
            </label>
            <label className="md:col-span-2 flex items-start gap-2 text-[11px] leading-4 normal-case tracking-normal text-cream-700">
              <input
                type="checkbox"
                data-testid="cloud-consent-ack"
                checked={cloudConsentAck}
                onChange={(event) => setCloudConsentAck(event.target.checked)}
                className="mt-0.5 h-3.5 w-3.5 shrink-0 accent-coral-dark"
              />
              <span>
                I understand that my code and prompts will be sent over the internet to the
                cloud provider I configure.
              </span>
            </label>
          </>
        ) : null}

        <div className="md:col-span-2 flex items-center gap-2">
          <button
            type="button"
            onClick={() => void onSave()}
            // For Cloud, gate Save on (a) a key being present — saving a Cloud backend with no
            // key would launch into the binary's safe Mock (the key is missing), which silently
            // looks broken — AND (b) the active consent acknowledgement (content leaves the
            // machine). Local kinds never need a key or consent.
            disabled={
              busy ||
              !validation.ok ||
              (kind === "cloud" && (!hasCloudKey || !cloudConsentAck))
            }
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
