// Pure, DOM-free validation + normalization for the Censor tier-2 (Gemma) local-AI
// provider config (Settings → Workspace "Censor local AI" card). Mirrors
// miniCoderBackend.ts for the mini-coder backend.
//
// These rules are the SINGLE source of truth for the UI form and MUST mirror the Rust
// boundary validation (backend/censor/gemma.rs validate_censor_local_ai) so a value the
// UI accepts is never rejected by the backend and vice-versa. The oMLX base URL is
// validated by the SHARED validateOmlxBaseUrl (reused from the mini-coder card) so the
// loopback/http/port rules cannot drift across the two oMLX surfaces.
//
// PRIVACY: Censor sends FILE CONTENT to this endpoint, so the oMLX base must be a
// LOOPBACK http origin — the same defense-in-depth refusal the backend enforces.

import {
  MODEL_PATTERN,
  validateOmlxBaseUrl,
  validateCloudBaseUrl,
} from "../agents/miniCoderBackend";
import type { CensorAiProvider, CensorLocalAi } from "../../types/config";

// The oMLX model id/tag cap (e.g. an mlx-community model). Generous: model ids can be
// `org/name` paths. Mirrors the Rust `CENSOR_OMLX_MODEL_MAX_LEN` (200) — the two MUST
// stay in EXACT agreement so a value the UI accepts is never rejected by the backend.
export const CENSOR_MODEL_MAX_LENGTH = 200;
// The oMLX base URL cap. Reuses the same bound as the mini-coder oMLX base URL.
export const CENSOR_BASE_URL_MAX_LENGTH = 200;

export const CENSOR_AI_PROVIDERS: readonly CensorAiProvider[] = [
  "ollama",
  "omlx",
  "appleFm",
  "cloud",
] as const;

export interface CensorLocalAiDraft {
  provider: CensorAiProvider;
  // The oMLX base URL field. Required+validated only for provider "omlx"; treated as
  // "" when absent. Unused for "ollama" (which uses the built-in loopback Ollama base).
  baseUrl: string;
  // The oMLX model field. Required only for provider "omlx"; unused for "ollama".
  model: string;
  // The OLLAMA-ONLY model-tag override (`ollamaModel`). The provider card (CensorLocalAiCard)
  // has NO input for it — the providers-tab CensorModelCard owns that input — but this card
  // must READ the existing value from config and ROUND-TRIP it here so saving the provider
  // never drops the override (split-brain fix). Empty -> omitted; validated with the SAME
  // bare-tag rule as the oMLX model. Optional so the oMLX card can leave it unset.
  ollamaModel?: string;
}

export interface CensorLocalAiValidation {
  ok: boolean;
  // Field-keyed inline messages; absent key == that field is valid.
  errors: Partial<Record<"baseUrl" | "model" | "ollamaModel", string>>;
  // The normalized config when ok (only the fields the provider uses are kept, so the
  // persisted payload carries ONLY the active provider's fields — no stale bleed).
  value: CensorLocalAi | null;
}

// Validate one draft. Pure and total: never throws, returns inline messages for each
// invalid field. Only the field(s) the provider requires are checked + kept, so
// switching provider clears stale errors AND drops the now-unused field from the
// persisted payload.
export function validateCensorLocalAi(
  draft: CensorLocalAiDraft,
): CensorLocalAiValidation {
  const errors: CensorLocalAiValidation["errors"] = {};
  const baseUrl = draft.baseUrl.trim();
  const model = draft.model.trim();
  // The Ollama-only override, trimmed. Validated with the SAME bare-tag rule as the oMLX
  // model (MODEL_PATTERN + length cap), mirroring the Rust validator's `ollama_model` branch.
  // Empty -> omitted from the persisted value; invalid -> a validation error.
  const ollamaModel = (draft.ollamaModel ?? "").trim();
  // Normalized base URL (trailing slash stripped) when valid; null when invalid.
  let normalizedBaseUrl: string | null = null;

  // The Ollama-only override is validated + kept ONLY on the ollama branch (same as the
  // Rust validator, which drops a stray ollama_model on the oMLX branch WITHOUT validating
  // it). On omlx it is ignored entirely, so a bad override left in config can never block
  // an oMLX save (whose card has no inline slot to surface an ollamaModel error anyway).
  if (draft.provider === "ollama" && ollamaModel.length > 0) {
    if (ollamaModel.length > CENSOR_MODEL_MAX_LENGTH) {
      errors.ollamaModel = `Model must be at most ${CENSOR_MODEL_MAX_LENGTH} characters.`;
    } else if (!MODEL_PATTERN.test(ollamaModel)) {
      errors.ollamaModel =
        "Model must be a bare tag (letters, digits, . _ : / -).";
    }
  }

  if (draft.provider === "appleFm") {
    // appleFm uses an optional model name and no base URL. The model follows the same
    // bare-token guardrail as oMLX so it is safe to pass as `fm respond --model <name>`.
    if (model.length > 0) {
      if (model.length > CENSOR_MODEL_MAX_LENGTH) {
        errors.model = `Model must be at most ${CENSOR_MODEL_MAX_LENGTH} characters.`;
      } else if (!MODEL_PATTERN.test(model)) {
        errors.model = "Model must be a bare tag (letters, digits, . _ : / -).";
      }
    }
  } else if (draft.provider === "omlx") {
    // omlx requires BOTH a non-empty model AND a loopback http (only) base URL. The
    // accept/reject set MUST match the Rust validator (validate_censor_local_ai).
    if (model.length === 0) {
      errors.model = "Enter the oMLX model id (e.g. mlx-community/gemma).";
    } else if (model.length > CENSOR_MODEL_MAX_LENGTH) {
      errors.model = `Model must be at most ${CENSOR_MODEL_MAX_LENGTH} characters.`;
    } else if (!MODEL_PATTERN.test(model)) {
      // Same bare-token char-class as the mini-coder oMLX/Ollama model (MODEL_PATTERN)
      // and the Rust `is_valid_model`. `org/name` HF paths stay valid (the `/` is
      // allowed); whitespace / control / shell metachars are rejected so all oMLX model
      // validators (mini Rust / Censor Rust / both TS) agree.
      errors.model = "Model must be a bare tag (letters, digits, . _ : / -).";
    }
    if (baseUrl.length === 0) {
      errors.baseUrl =
        "Enter the oMLX server base URL (e.g. http://localhost:8000/v1).";
    } else if (baseUrl.length > CENSOR_BASE_URL_MAX_LENGTH) {
      errors.baseUrl = `Base URL must be at most ${CENSOR_BASE_URL_MAX_LENGTH} characters.`;
    } else {
      normalizedBaseUrl = validateOmlxBaseUrl(baseUrl);
      if (normalizedBaseUrl === null) {
        errors.baseUrl =
          "Base URL must be a loopback http origin (localhost, 127.0.0.1 or [::1]).";
      }
    }
  } else if (draft.provider === "cloud") {
    // cloud requires BOTH a non-empty model AND an https (remote) base URL. SAME model rule
    // as oMLX, but the base is validated with validateCloudBaseUrl (https + non-loopback +
    // the shared SSRF-metadata denial) instead of the loopback validator. The API KEY is NOT
    // part of this draft/value — it is saved separately to the OS vault.
    if (model.length === 0) {
      errors.model = "Enter the cloud model id (e.g. openai/gpt-4o-mini).";
    } else if (model.length > CENSOR_MODEL_MAX_LENGTH) {
      errors.model = `Model must be at most ${CENSOR_MODEL_MAX_LENGTH} characters.`;
    } else if (!MODEL_PATTERN.test(model)) {
      errors.model = "Model must be a bare tag (letters, digits, . _ : / -).";
    }
    if (baseUrl.length === 0) {
      errors.baseUrl =
        "Enter the cloud endpoint base URL (e.g. https://openrouter.ai/api/v1).";
    } else if (baseUrl.length > CENSOR_BASE_URL_MAX_LENGTH) {
      errors.baseUrl = `Base URL must be at most ${CENSOR_BASE_URL_MAX_LENGTH} characters.`;
    } else {
      normalizedBaseUrl = validateCloudBaseUrl(baseUrl);
      if (normalizedBaseUrl === null) {
        errors.baseUrl =
          "Base URL must be an https origin (e.g. https://openrouter.ai/api/v1).";
      }
    }
  }
  // provider "ollama" needs no fields here — it uses the built-in loopback Ollama base
  // + the default Gemma model; any omlx-only draft fields are simply dropped below.

  const ok = Object.keys(errors).length === 0;
  if (!ok) return { ok, errors, value: null };

  // Keep ONLY the fields the provider uses, so the persisted config is minimal and a
  // later provider switch never leaves a stale base/model behind (no stale-field bleed).
  let value: CensorLocalAi;
  if (draft.provider === "appleFm") {
    value = model.length > 0
      ? { provider: "appleFm", model }
      : { provider: "appleFm" };
  } else if (draft.provider === "omlx") {
    // normalizedBaseUrl is non-null here: ok === true means no baseUrl error, and the
    // only ok-with-omlx path sets it via validateOmlxBaseUrl. Non-null assertion (NOT
    // `?? baseUrl`) so a future refactor that breaks the invariant surfaces immediately
    // instead of silently persisting an UNVALIDATED url.
    // oMLX uses `model`; the Ollama-only override is intentionally NOT carried here (the
    // Rust validator likewise drops a stray ollama_model on the oMLX branch).
    value = { provider: "omlx", baseUrl: normalizedBaseUrl!, model };
  } else if (draft.provider === "cloud") {
    // normalizedBaseUrl is non-null here (ok === true ⇒ no baseUrl error ⇒ validateCloudBaseUrl
    // returned a string). The API key is intentionally NOT carried — it lives in the vault.
    value = { provider: "cloud", baseUrl: normalizedBaseUrl!, model };
  } else {
    // Ollama: carry the validated override when present so a provider save preserves the
    // model the CensorModelCard set (split-brain fix). Empty -> omitted, so the bare
    // default stays the minimal {provider:"ollama"} the backend drops entirely (no churn).
    value = ollamaModel.length > 0
      ? { provider: "ollama", ollamaModel }
      : { provider: "ollama" };
  }
  return { ok: true, errors, value };
}
