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
  DesignEffort,
  DesignLlmBackend,
  DesignLlmBackendKind,
  MiniCoderBackend,
} from "../../types/config";

// Per-run timeout bounds. MUST match the Rust `DESIGN_TIMEOUT_SECS_MIN/MAX` exactly
// (out-of-range is REJECTED on both sides, mirroring the reject-not-normalize posture).
export const DESIGN_TIMEOUT_SECS_MIN = 60;
export const DESIGN_TIMEOUT_SECS_MAX = 600;

// The accepted reasoning-effort values, in selector order. Mirrors the Rust accept set.
export const DESIGN_EFFORTS: readonly DesignEffort[] = ["low", "medium", "high"] as const;

// Validate + normalize the OPTIONAL effort knob. Mirrors the Rust `validate_design_effort`:
// trims + lowercases, accepts ONLY low/medium/high, and treats absent/empty as "no
// override" (returns `{ value: undefined }`). An illegal value is REJECTED (ok:false), not
// silently dropped. Pure + total: never throws.
export type DesignEffortValidation =
  | { ok: true; value: DesignEffort | undefined }
  | { ok: false; value: undefined };

export function validateDesignEffort(
  effort: string | null | undefined,
): DesignEffortValidation {
  const raw = (effort ?? "").trim().toLowerCase();
  if (raw === "") return { ok: true, value: undefined };
  if (raw === "low" || raw === "medium" || raw === "high") {
    return { ok: true, value: raw };
  }
  return { ok: false, value: undefined };
}

// Validate the OPTIONAL per-run timeoutSecs. Mirrors the Rust `validate_design_timeout_secs`:
// absent => no override; a present value must be an integer within [60, 600] — out-of-range
// (or non-finite/non-integer) is REJECTED (ok:false), never clamped. Pure + total.
export function validateDesignTimeoutSecs(
  timeoutSecs: number | null | undefined,
): { ok: boolean; value: number | undefined } {
  if (timeoutSecs === null || timeoutSecs === undefined) {
    return { ok: true, value: undefined };
  }
  if (
    !Number.isFinite(timeoutSecs) ||
    !Number.isInteger(timeoutSecs) ||
    timeoutSecs < DESIGN_TIMEOUT_SECS_MIN ||
    timeoutSecs > DESIGN_TIMEOUT_SECS_MAX
  ) {
    return { ok: false, value: undefined };
  }
  return { ok: true, value: timeoutSecs };
}

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
  // OPTIONAL generation knobs owned by the composer's model popover (NOT the Settings
  // card). When present they are validated + carried through to the normalized value;
  // when absent they are simply omitted (the card path passes them through unchanged so a
  // save from the card never DROPS them — see DesignLlmBackendCard).
  effort?: string;
  timeoutSecs?: number;
}

export interface DesignBackendValidation {
  ok: boolean;
  // Field-keyed inline messages; absent key == that field is valid.
  errors: Partial<Record<"kind" | "model" | "command" | "baseUrl" | "effort" | "timeoutSecs", string>>;
  // The normalized backend when ok (only the fields the kind uses are kept).
  value: DesignLlmBackend | null;
}

// Merge the validated OPTIONAL effort/timeout knobs onto a normalized backend value,
// returning a NEW object (never mutating the input). Shared by every kind arm so the two
// surfaces (codex/claude special-case + the generic delegation) never drift. Returns the
// per-field error map (empty when valid), whether both knobs validated, and the
// resulting value. When a knob is INVALID the value is forced to `null` so the overall
// result is never a partially-valid object (a rejected save must carry no value). A `null`
// input value (an upstream kind error) is left null.
function applyEffortAndTimeout(
  draft: DesignBackendDraft,
  value: DesignLlmBackend | null,
): { errors: DesignBackendValidation["errors"]; ok: boolean; value: DesignLlmBackend | null } {
  const errors: DesignBackendValidation["errors"] = {};
  const effort = validateDesignEffort(draft.effort);
  const timeout = validateDesignTimeoutSecs(draft.timeoutSecs);
  if (!effort.ok) errors.effort = "Effort must be one of: low, medium, high.";
  if (!timeout.ok) {
    errors.timeoutSecs = `Timeout must be between ${DESIGN_TIMEOUT_SECS_MIN} and ${DESIGN_TIMEOUT_SECS_MAX} seconds.`;
  }
  const ok = effort.ok && timeout.ok;
  // A knob failure invalidates the whole save: emit no value, not a half-applied one.
  if (!ok || value === null) return { errors, ok, value: null };
  const merged: DesignLlmBackend = { ...value };
  if (effort.value !== undefined) merged.effort = effort.value;
  if (timeout.value !== undefined) merged.timeoutSecs = timeout.value;
  return { errors, ok, value: merged };
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
    const baseValue: DesignLlmBackend | null = codexResult.value
      ? { kind: "claude", ...(codexResult.value.model !== undefined ? { model: codexResult.value.model } : {}) }
      : null;
    const knobs = applyEffortAndTimeout(draft, baseValue);
    return {
      ok: codexResult.ok && knobs.ok,
      errors: { ...codexResult.errors, ...knobs.errors },
      value: knobs.value,
    };
  }

  if ((draft.kind as string) === "appleFm") {
    return {
      ok: false,
      errors: { kind: "Apple on-device is not supported for Design LLM." },
      value: null,
    };
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
  const baseValue: DesignLlmBackend | null = result.value
    ? remapValue(result.value as MiniCoderBackend & { kind: Exclude<MiniCoderBackend["kind"], "appleFm"> })
    : null;
  const knobs = applyEffortAndTimeout(draft, baseValue);
  return {
    ok: result.ok && knobs.ok,
    errors: { ...result.errors, ...knobs.errors },
    value: knobs.value,
  };
}

// Re-map a normalized MiniCoderBackend onto the DesignLlmBackend shape. They are
// structurally identical; this preserves only the fields the kind kept (no churn).
function remapValue(v: MiniCoderBackend & { kind: Exclude<MiniCoderBackend["kind"], "appleFm"> }): DesignLlmBackend {
  const out: DesignLlmBackend = { kind: v.kind };
  if (v.model !== undefined) out.model = v.model;
  if (v.command !== undefined) out.command = v.command;
  if (v.baseUrl !== undefined) out.baseUrl = v.baseUrl;
  return out;
}
