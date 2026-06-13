import { describe, it, expect } from "vitest";
import { saveFeedback, keyStatusHint } from "./oracleLlmFeedback";
import type {
  OracleLlmSettings,
  OracleLlmSettingsStatus,
} from "../types/backend";

function baseSettings(over: Partial<OracleLlmSettings> = {}): OracleLlmSettings {
  return {
    provider: "scaleway",
    model: "voxtral-small-24b-2507",
    baseUrl: null,
    remoteEnabled: true,
    ...over,
  };
}

function baseStatus(
  over: Partial<OracleLlmSettingsStatus> = {},
): OracleLlmSettingsStatus {
  return {
    settings: baseSettings(over.settings),
    apiKeyConfigured: false,
    status: "configured",
    message: null,
    ...over,
  };
}

describe("saveFeedback", () => {
  it("maps a configured status to a saved outcome", () => {
    expect(saveFeedback(baseStatus({ status: "configured" }))).toEqual({
      kind: "saved",
      message: "",
    });
  });

  it("treats local/ok and unknown additive statuses as saved", () => {
    expect(saveFeedback(baseStatus({ status: "local" })).kind).toBe("saved");
    expect(saveFeedback(baseStatus({ status: "ok" })).kind).toBe("saved");
    // A missing key still means the SETTINGS were saved — not a save failure.
    expect(saveFeedback(baseStatus({ status: "missing_api_key" })).kind).toBe(
      "saved",
    );
  });

  it("surfaces an error status with the backend message", () => {
    const fb = saveFeedback(
      baseStatus({
        status: "error",
        message: "API key is too short or contains whitespace.",
      }),
    );
    expect(fb.kind).toBe("error");
    expect(fb.message).toBe("API key is too short or contains whitespace.");
  });

  it("falls back to a generic message for an error status without a message", () => {
    const fb = saveFeedback(baseStatus({ status: "error", message: null }));
    expect(fb.kind).toBe("error");
    expect(fb.message).toBe("Save failed — see the error banner.");
  });

  it("maps a null return (hard failure) to a generic error pointing at the banner", () => {
    expect(saveFeedback(null)).toEqual({
      kind: "error",
      message: "Save failed — see the error banner.",
    });
  });
});

describe("keyStatusHint", () => {
  it("returns null when settings are not loaded yet", () => {
    expect(keyStatusHint(null, false)).toBeNull();
  });

  it("returns null when remote answering is disabled", () => {
    const status = baseStatus({ settings: baseSettings({ remoteEnabled: false }) });
    expect(keyStatusHint(status, false)).toBeNull();
  });

  it("reports a dedicated key saved in the vault (ok tone)", () => {
    const status = baseStatus({ apiKeyConfigured: true });
    expect(keyStatusHint(status, false)).toEqual({
      tone: "ok",
      label: "API key saved in the Windows vault",
    });
  });

  it("prefers the dedicated key over the reused Scaleway token", () => {
    const status = baseStatus({ apiKeyConfigured: true });
    // Even when the Scaleway token would be reused, a dedicated key wins.
    expect(keyStatusHint(status, true)?.tone).toBe("ok");
  });

  it("reports the reused Scaleway token when no dedicated key is set (info tone)", () => {
    const status = baseStatus({ apiKeyConfigured: false });
    expect(keyStatusHint(status, true)).toEqual({
      tone: "info",
      label: "Using your saved Scaleway token (no dedicated key needed)",
    });
  });

  it("warns when remote is enabled but there is no key or token (warn tone)", () => {
    const status = baseStatus({ apiKeyConfigured: false });
    expect(keyStatusHint(status, false)).toEqual({
      tone: "warn",
      label: "No API key — remote answers are disabled (extractive only)",
    });
  });

  it("never reports a key line for a LOCAL provider (oMLX/Ollama are keyless)", () => {
    // Robust even when a stale saved config still has remoteEnabled=true (the
    // old changeProvider bug): a local provider has no key to report.
    for (const provider of ["omlx", "ollama"] as const) {
      const status = baseStatus({
        settings: baseSettings({ provider, remoteEnabled: true }),
        apiKeyConfigured: false,
      });
      expect(keyStatusHint(status, false)).toBeNull();
    }
  });
});
