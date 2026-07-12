import { describe, expect, it } from "vitest";
import type { OracleLlmSettingsStatus, SecretStatus } from "../../types/backend";
import { deriveProviderConfigured } from "./oracleProviderState";

// Table-driven test for the shared provider-configured derivation. This pure fn
// is the single source of truth that BOTH the Oracle admin panel (Settings) and
// the Polis ask-panel use, so the two surfaces never disagree on whether an
// answer provider is available.
//
// Rule: apiKeyConfigured || status === "configured" (for local providers).
// A Cloudflare secret alone MUST NOT make Oracle appear configured — Cloudflare
// is unrelated to Oracle LLM.

function secret(
  provider: SecretStatus["provider"],
  configured: boolean,
): SecretStatus {
  return { provider, configured } as SecretStatus;
}

function llm(apiKeyConfigured: boolean): OracleLlmSettingsStatus {
  return {
    settings: {
      provider: "openai",
      model: "gpt-4o-mini",
      baseUrl: null,
      remoteEnabled: true,
    },
    apiKeyConfigured,
    status: apiKeyConfigured ? "configured" : "missing_api_key",
    message: null,
  };
}

function localLlm(
  provider: "omlx" | "ollama",
): OracleLlmSettingsStatus {
  // What the Rust vault returns for a LOCAL provider: keyless, status
  // "configured", a loopback base URL.
  return {
    settings: {
      provider,
      model: "Qwen3.6-35B-A3B-4bit-DWQ",
      baseUrl: "http://127.0.0.1:8000/v1",
      remoteEnabled: false,
    },
    apiKeyConfigured: false,
    status: "configured",
    message: "Local loopback provider — keyless; prompts never leave this machine.",
  };
}

describe("deriveProviderConfigured — local providers are keyless (oMLX/Ollama)", () => {
  it("oMLX local is configured WITHOUT an API key", () => {
    expect(deriveProviderConfigured(localLlm("omlx"), [])).toBe(true);
  });
  it("Ollama local is configured WITHOUT an API key", () => {
    expect(deriveProviderConfigured(localLlm("ollama"), undefined)).toBe(true);
  });
  it("a local provider whose backend status is NOT configured stays false", () => {
    const notReady = { ...localLlm("omlx"), status: "local" };
    expect(deriveProviderConfigured(notReady, [])).toBe(false);
  });
});

describe("deriveProviderConfigured", () => {
  it("is configured when a dedicated Oracle API key is configured", () => {
    expect(deriveProviderConfigured(llm(true), [])).toBe(true);
  });

  it("is configured when status is configured (local keyless provider)", () => {
    expect(
      deriveProviderConfigured({ ...llm(false), status: "configured" }, []),
    ).toBe(true);
  });

  it("is configured when the API key is set even if no secrets exist", () => {
    expect(
      deriveProviderConfigured(llm(true), []),
    ).toBe(true);
  });

  it("is NOT configured when neither a key nor configured status exists", () => {
    expect(
      deriveProviderConfigured(llm(false), []),
    ).toBe(false);
  });

  it("is NOT configured with null settings and no secrets", () => {
    expect(deriveProviderConfigured(null, [])).toBe(false);
  });

  it("is NOT configured with null settings and no secrets", () => {
    expect(
      deriveProviderConfigured(null, [secret("cloudflare", true)]),
    ).toBe(false);
  });

  it("tolerates an undefined secrets list", () => {
    expect(deriveProviderConfigured(llm(false), undefined)).toBe(false);
    expect(deriveProviderConfigured(llm(true), undefined)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Regression: Cloudflare-only secret must NOT configure Oracle
// ---------------------------------------------------------------------------
describe("deriveProviderConfigured — Cloudflare must not bleed (regression)", () => {
  it("apiKeyConfigured=true → true (primary signal)", () => {
    expect(deriveProviderConfigured(llm(true), [])).toBe(true);
  });

  it("only cloudflare configured → false (unrelated provider, must not bleed)", () => {
    expect(
      deriveProviderConfigured(llm(false), [secret("cloudflare", true)]),
    ).toBe(false);
  });

  it("nothing configured → false", () => {
    expect(deriveProviderConfigured(null, [])).toBe(false);
  });
});
