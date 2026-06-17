// Pure, DOM-free validation + normalization for the single global LOCAL MAIN-CODER
// backend config (Settings → Providers & Models "Local main coder (Devboule)" card).
//
// TIER DISTINCTION: this is the ORCHESTRATOR / local-main-coder tier — a SEPARATE,
// INDEPENDENT value from the MINI-coder backend (miniCoderBackend.ts). The mini is the
// small delegated worker a coder spawns; the orchestrator is the local main coder itself.
//
// The local-coder kinds (ollama/omlx) are a STRICT SUBSET of the mini's kinds with
// IDENTICAL per-kind rules, so — exactly like designLlmBackend.ts — this module does NOT
// re-implement any rule: it REUSES the shared validator (`validateMiniBackend`) and the
// shared char/loopback primitives from miniCoderBackend.ts, then re-maps the normalized
// value to a LocalCoderBackend. The accept/reject SET is byte-for-byte the same as the
// mini-coder card and the Rust `validate_local_coder_backend` boundary.

import {
  MINI_BASE_URL_MAX_LENGTH,
  MINI_MODEL_MAX_LENGTH,
  MODEL_PATTERN,
  validateCloudBaseUrl,
  validateMiniBackend,
  validateOmlxBaseUrl,
} from "./miniCoderBackend";
import type {
  LocalCoderBackend,
  LocalCoderBackendKind,
} from "../../types/config";

// Re-export the shared caps under local-coder names so the card imports a single module
// (no drift: these are the SAME caps the mini + Rust use).
export const LOCAL_MODEL_MAX_LENGTH = MINI_MODEL_MAX_LENGTH;
export const LOCAL_BASE_URL_MAX_LENGTH = MINI_BASE_URL_MAX_LENGTH;

// The kinds the local coder offers, in selector order. A strict subset of the mini's kinds
// (only the two LOCAL HTTP runtimes the orchestrator binary can drive). Mirrors the Rust
// `LocalCoderBackendKind`.
export const LOCAL_BACKEND_KINDS: readonly LocalCoderBackendKind[] = [
  "ollama",
  "omlx",
  "cloud",
] as const;

export interface LocalBackendDraft {
  kind: LocalCoderBackendKind;
  model: string;
  // The base URL field. REQUIRED + validated for kind "omlx". OPTIONAL + EDITABLE for kind
  // "ollama": empty/absent => the launch uses the OLLAMA_OPENAI_BASE_URL default
  // (http://localhost:11434/v1); a non-empty value is validated to the SAME loopback-http
  // rule omlx uses (Ollama on a non-default port, no hardcode lock-in). Treated as "" when
  // absent.
  baseUrl?: string;
}

export interface LocalBackendValidation {
  ok: boolean;
  // Field-keyed inline messages; absent key == that field is valid.
  errors: Partial<Record<"model" | "baseUrl", string>>;
  // The normalized backend when ok (only the fields the kind uses are kept).
  value: LocalCoderBackend | null;
}

// Validate one draft. Pure and total: never throws. Delegates the MODEL (and omlx's
// REQUIRED baseUrl) to `validateMiniBackend` (the local kinds are a subset of the mini's,
// with the same per-kind rules), then re-maps the normalized mini value to a
// LocalCoderBackend (dropping `command`/`maxConcurrent`, which the local coder does not
// have). The error keys are narrowed to the two fields this card surfaces (model/baseUrl) —
// `command` can never appear because no local kind uses it.
//
// OLLAMA baseUrl is handled HERE, not by the mini validator: the mini's ollama arm IGNORES
// baseUrl entirely (the mini never had a configurable Ollama endpoint). For the local coder
// the field is OPTIONAL + EDITABLE — empty => omit (the launch uses the :11434 default); a
// non-empty value is validated with the SAME `validateOmlxBaseUrl` (loopback http only) the
// Rust `validate_local_coder_backend` ollama arm enforces, so TS/Rust accept/reject the same
// set and a custom port round-trips.
export function validateLocalBackend(
  draft: LocalBackendDraft,
): LocalBackendValidation {
  // CLOUD (opt-in) is NOT a mini kind, so handle it HERE before delegating: validate the
  // model (the same bare-tag rule) + an HTTPS non-loopback base URL (validateCloudBaseUrl,
  // mirroring the Rust boundary). The API KEY is NOT validated here — it lives in the OS
  // vault on a separate status surface; this pure helper validates the config SHAPE only, so
  // a saved Cloud config and a separately-saved key stay independent (the card gates the Save
  // button on the key's present/absent status from the vault).
  if (draft.kind === "cloud") {
    const errors: LocalBackendValidation["errors"] = {};
    const model = draft.model.trim();
    if (model.length === 0) {
      errors.model = "A model id is required.";
    } else if (model.length > LOCAL_MODEL_MAX_LENGTH) {
      errors.model = `Model must be at most ${LOCAL_MODEL_MAX_LENGTH} characters.`;
    } else if (!MODEL_PATTERN.test(model)) {
      errors.model = "Model must be a bare tag (letters, digits, . _ : / -).";
    }

    const rawBase = (draft.baseUrl ?? "").trim();
    let normalizedBase: string | null = null;
    if (rawBase.length === 0) {
      errors.baseUrl = "An https base URL is required for Cloud.";
    } else if (rawBase.length > LOCAL_BASE_URL_MAX_LENGTH) {
      errors.baseUrl = `Base URL must be at most ${LOCAL_BASE_URL_MAX_LENGTH} characters.`;
    } else {
      normalizedBase = validateCloudBaseUrl(rawBase);
      if (normalizedBase === null) {
        errors.baseUrl =
          "Base URL must be an https public host (e.g. https://openrouter.ai/api/v1) — not loopback, not an IP.";
      }
    }

    if (Object.keys(errors).length > 0 || normalizedBase === null) {
      return { ok: false, errors, value: null };
    }
    return {
      ok: true,
      errors,
      value: { kind: "cloud", model, baseUrl: normalizedBase },
    };
  }

  const mini = validateMiniBackend({
    kind: draft.kind,
    model: draft.model,
    // No command for any local kind; pass empty so the mini validator's api-arm (never
    // reached for ollama/omlx) has a defined input.
    command: "",
    baseUrl: draft.baseUrl,
  });

  // Carry only the two fields this card can show; the mini validator never emits a
  // `command` error for ollama/omlx, so this loses nothing.
  const errors: LocalBackendValidation["errors"] = {};
  if (mini.errors.model) errors.model = mini.errors.model;
  if (mini.errors.baseUrl) errors.baseUrl = mini.errors.baseUrl;

  // Optional ollama baseUrl: validate it ourselves (the mini ignores it). Empty/absent is
  // fine (use the default); a non-empty value must pass the loopback-http rule, else surface
  // an inline error. Computed up front so both the error branch and the value branch agree.
  let ollamaBaseUrl: string | undefined;
  if (draft.kind === "ollama") {
    const raw = (draft.baseUrl ?? "").trim();
    if (raw.length > 0) {
      if (raw.length > LOCAL_BASE_URL_MAX_LENGTH) {
        errors.baseUrl = `Base URL must be at most ${LOCAL_BASE_URL_MAX_LENGTH} characters.`;
      } else {
        const normalized = validateOmlxBaseUrl(raw);
        if (normalized === null) {
          errors.baseUrl =
            "Base URL must be a loopback http origin (localhost, 127.0.0.1 or [::1]).";
        } else {
          ollamaBaseUrl = normalized;
        }
      }
    }
  }

  if (!mini.ok || !mini.value || Object.keys(errors).length > 0) {
    return { ok: false, errors, value: null };
  }

  // Re-map the normalized mini value to a LocalCoderBackend, keeping ONLY the fields the
  // kind uses (so a kind switch never leaves a stale model/baseUrl behind).
  let value: LocalCoderBackend;
  if (mini.value.kind === "omlx") {
    // For omlx the mini validator guarantees both model + a normalized baseUrl.
    value = {
      kind: "omlx",
      model: mini.value.model!,
      baseUrl: mini.value.baseUrl!,
    };
  } else {
    // ollama: model always; baseUrl ONLY when the user set a (validated, normalized) custom
    // endpoint — omitted otherwise so the launch falls back to the editable :11434 default.
    value = { kind: "ollama", model: mini.value.model! };
    if (ollamaBaseUrl !== undefined) value.baseUrl = ollamaBaseUrl;
  }
  return { ok: true, errors, value };
}
