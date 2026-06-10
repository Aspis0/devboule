// Pure, DOM-free validation + normalization for the single global design-LLM backend
// config (Settings → Workspace "Design LLM backend" card).
//
// The design-LLM backend is a 1:1 MIRROR of the mini-coder backend: the SAME four kinds
// (ollama/api/codex/omlx) with the SAME per-kind rules. To guarantee the two NEVER drift
// (and to match the Rust side, where `validate_design_llm_backend` reuses the mini-coder
// primitives), this module does NOT re-implement any rule — it REUSES the shared
// validator (`validateMiniBackend`) and the shared char/loopback primitives
// (`validateOmlxBaseUrl`, `MODEL_PATTERN`) from miniCoderBackend.ts, then re-maps the
// normalized value to a DesignLlmBackend. The accept/reject SET is byte-for-byte the
// same as the mini-coder card and the Rust boundary.

import {
  MINI_BASE_URL_MAX_LENGTH,
  MINI_COMMAND_MAX_LENGTH,
  MINI_MODEL_MAX_LENGTH,
  MODEL_PATTERN,
  validateMiniBackend,
  validateOmlxBaseUrl,
  type MiniBackendDraft,
} from "../agents/miniCoderBackend";
import type {
  DesignLlmBackend,
  DesignLlmBackendKind,
  MiniCoderBackend,
} from "../../types/config";

// Re-export the shared caps + primitives under design-flavored names so the card can
// import everything from this one module. These are the SAME values the mini-coder uses
// (no separate caps) — re-exporting (not redefining) keeps them in lockstep forever.
export {
  MODEL_PATTERN,
  validateOmlxBaseUrl,
  MINI_MODEL_MAX_LENGTH as DESIGN_MODEL_MAX_LENGTH,
  MINI_COMMAND_MAX_LENGTH as DESIGN_COMMAND_MAX_LENGTH,
  MINI_BASE_URL_MAX_LENGTH as DESIGN_BASE_URL_MAX_LENGTH,
};

// The design backend kinds. SUPERSET of MINI_BACKEND_KINDS: adds "claude" (validated like
// "codex" — optional model only). The mini-coder card does NOT offer "claude".
export const DESIGN_BACKEND_KINDS: readonly DesignLlmBackendKind[] = [
  "ollama",
  "api",
  "codex",
  "claude",
  "omlx",
] as const;

export interface DesignBackendDraft {
  kind: DesignLlmBackendKind;
  model: string;
  command: string;
  // The oMLX base URL field. Required+validated only for kind "omlx"; treated as "" when
  // absent.
  baseUrl?: string;
}

export interface DesignBackendValidation {
  ok: boolean;
  // Field-keyed inline messages; absent key == that field is valid.
  errors: Partial<Record<"model" | "command" | "baseUrl", string>>;
  // The normalized backend when ok (only the fields the kind uses are kept).
  value: DesignLlmBackend | null;
}

// Validate one draft. Pure and total: never throws, returns inline messages for each
// invalid field. DELEGATES to the shared `validateMiniBackend` (the kinds + rules are
// identical) and re-maps its result to the DesignLlmBackend shape. `DesignLlmBackendKind`
// and `MiniCoderBackendKind` are the SAME string-union, so the kind passes through
// unchanged; the normalized `MiniCoderBackend` value is structurally identical to a
// `DesignLlmBackend` (same optional fields), so we simply retype it.
export function validateDesignBackend(
  draft: DesignBackendDraft,
): DesignBackendValidation {
  // "claude" is design-only (not a mini-coder kind) and follows the EXACT same rules as
  // "codex" (optional model, no command/baseUrl). Validate it THROUGH the shared codex arm
  // (so the model rule never drifts) then re-stamp the kind back to "claude". This must run
  // BEFORE the generic delegation below, otherwise the mini validator's catch-all would
  // mis-label it as "codex".
  if (draft.kind === "claude") {
    const codexResult = validateMiniBackend({
      kind: "codex",
      model: draft.model,
      command: draft.command,
      baseUrl: draft.baseUrl,
    });
    const value: DesignLlmBackend | null = codexResult.value
      ? { kind: "claude", ...(codexResult.value.model !== undefined ? { model: codexResult.value.model } : {}) }
      : null;
    return { ok: codexResult.ok, errors: codexResult.errors, value };
  }

  // DesignBackendDraft is structurally a MiniBackendDraft (the remaining kinds are equal),
  // so this is a safe widening for the shared validator.
  const miniDraft: MiniBackendDraft = {
    kind: draft.kind,
    model: draft.model,
    command: draft.command,
    baseUrl: draft.baseUrl,
  };
  const result = validateMiniBackend(miniDraft);
  // The normalized MiniCoderBackend is structurally a DesignLlmBackend (identical fields,
  // same kind union). Re-map explicitly so the public type is DesignLlmBackend, not a
  // structural alias of the mini-coder type.
  const value: DesignLlmBackend | null = result.value
    ? remapValue(result.value)
    : null;
  return { ok: result.ok, errors: result.errors, value };
}

// Re-map a normalized MiniCoderBackend onto the DesignLlmBackend shape. They are
// structurally identical; this preserves only the fields the kind kept (no churn).
function remapValue(v: MiniCoderBackend): DesignLlmBackend {
  const out: DesignLlmBackend = { kind: v.kind };
  if (v.model !== undefined) out.model = v.model;
  if (v.command !== undefined) out.command = v.command;
  if (v.baseUrl !== undefined) out.baseUrl = v.baseUrl;
  return out;
}
