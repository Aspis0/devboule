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
  validateMiniBackend,
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
] as const;

export interface LocalBackendDraft {
  kind: LocalCoderBackendKind;
  model: string;
  // The oMLX base URL field. Optional so the ollama caller need not supply it; treated as
  // "" when absent. Required + validated only for kind "omlx".
  baseUrl?: string;
}

export interface LocalBackendValidation {
  ok: boolean;
  // Field-keyed inline messages; absent key == that field is valid.
  errors: Partial<Record<"model" | "baseUrl", string>>;
  // The normalized backend when ok (only the fields the kind uses are kept).
  value: LocalCoderBackend | null;
}

// Validate one draft. Pure and total: never throws. Delegates to `validateMiniBackend`
// (the local kinds are a subset of the mini's, with the same per-kind rules), then re-maps
// the normalized mini value to a LocalCoderBackend (dropping `command`/`maxConcurrent`,
// which the local coder does not have). The error keys are narrowed to the two fields this
// card surfaces (model/baseUrl) — `command` can never appear because no local kind uses it.
export function validateLocalBackend(
  draft: LocalBackendDraft,
): LocalBackendValidation {
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

  if (!mini.ok || !mini.value) {
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
    // ollama: model only (baseUrl is resolved by the launch, not stored).
    value = { kind: "ollama", model: mini.value.model! };
  }
  return { ok: true, errors, value };
}
