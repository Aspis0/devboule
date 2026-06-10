// Pure mappers for the Oracle "Answer LLM" Save feedback + key-status line.
//
// The user saves the Oracle LLM API key in Settings -> Oracle and cannot tell
// whether it worked: there is no save confirmation, no error surfaced when the
// backend returns an error STATUS object, and the "Scaleway token is reused"
// behaviour is invisible. These two pure functions are the single source of
// truth for (a) what the Save button should say after a save resolves, and
// (b) the always-visible key-status line. They are React/DOM-free so they can
// be unit-tested directly (vitest `node` environment).

import type { OracleLlmSettingsStatus } from "../types/backend";

// Transient Save-button result. `saving`/`idle` are driven by the component;
// these mappers only produce the post-resolve `saved` / `error` outcome.
export type SaveFeedbackKind = "saved" | "error";

export interface SaveFeedback {
  kind: SaveFeedbackKind;
  // For `error`, a short human message (backend message when present, else a
  // generic pointer to the global error banner). Empty for `saved`.
  message: string;
}

const GENERIC_SAVE_FAILURE = "Save failed — see the error banner.";

// Backend statuses that mean "the save did NOT take": an explicit error, or a
// privacy/config gate that blocked it. `missing_api_key` /
// `missing_fallback_api_key` are NOT failures here — the settings were saved,
// the key is just (intentionally) absent; the key-status line communicates
// that separately. Anything else (configured, local, ok, unknown additive
// statuses) counts as a successful save.
const ERROR_STATUSES = new Set(["error"]);

/**
 * Decide what the Save button should show once `saveOracleLlmSettings`
 * resolves.
 *
 * @param status the returned `OracleLlmSettingsStatus`, or `null` when the
 *   context hit a hard failure (it set the global error banner in that case).
 */
export function saveFeedback(
  status: OracleLlmSettingsStatus | null,
): SaveFeedback {
  // Hard failure: context returned null and already surfaced the real error on
  // the global banner. Point the user there rather than inventing a message.
  if (status === null) {
    return { kind: "error", message: GENERIC_SAVE_FAILURE };
  }

  if (ERROR_STATUSES.has(status.status)) {
    return {
      kind: "error",
      message: status.message?.trim() || GENERIC_SAVE_FAILURE,
    };
  }

  return { kind: "saved", message: "" };
}

// Always-visible key-status line tone. Maps onto the warm palette in the view:
// `ok` -> sage check, `info` -> amber info, `warn` -> coral warning.
export type KeyStatusTone = "ok" | "info" | "warn";

export interface KeyStatusHint {
  tone: KeyStatusTone;
  label: string;
}

/**
 * Decide the always-visible key-status line from the loaded settings.
 *
 * `usesScalewayProviderToken` is computed by the view (it depends on the
 * separately-fetched Scaleway secret status, not on `OracleLlmSettingsStatus`),
 * so it is passed in rather than re-derived here.
 *
 * The three states:
 *   - ok   : a dedicated Oracle key is saved in the vault.
 *   - info : no dedicated key, but the saved Scaleway provider token is reused.
 *   - warn : remote answers are enabled but there is no key/token at all.
 * When remote answering is disabled there is no key to report, so the line is
 * hidden (returns null).
 */
export function keyStatusHint(
  status: OracleLlmSettingsStatus | null,
  usesScalewayProviderToken: boolean,
): KeyStatusHint | null {
  // No loaded settings yet, or remote answering off -> nothing meaningful to
  // say about a key.
  if (!status || !status.settings.remoteEnabled) return null;

  if (status.apiKeyConfigured) {
    return { tone: "ok", label: "API key saved in the Windows vault" };
  }

  if (usesScalewayProviderToken) {
    return {
      tone: "info",
      label: "Using your saved Scaleway token (no dedicated key needed)",
    };
  }

  return {
    tone: "warn",
    label: "No API key — remote answers are disabled (extractive only)",
  };
}
