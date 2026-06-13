import { describe, expect, it } from "vitest";
import type { OracleLlmSettingsStatus, SecretStatus } from "../../types/backend";
import { deriveProviderConfigured } from "./oracleProviderState";

// Table-driven test for the shared provider-configured derivation. This pure fn
// is the single source of truth that BOTH the Oracle admin panel (Settings) and
// the Polis ask-panel use, so the two surfaces never disagree on whether an
// answer provider is available.
//
// Rule after fix-2: apiKeyConfigured || (scaleway secret configured).
// A Cloudflare secret alone MUST NOT make Oracle appear configured — Cloudflare
// is unrelated to Oracle LLM (which runs on Scaleway).

function secret(
  provider: SecretStatus["provider"],
  configured: boolean,
): SecretStatus {
  return { provider, configured } as SecretStatus;
}

function llm(apiKeyConfigured: boolean): OracleLlmSettingsStatus {
  return {
    settings: {
      provider: "scaleway",
      model: "voxtral-small-24b-2507",
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

  it("is configured when a reused provider (Scaleway) token exists", () => {
    expect(
      deriveProviderConfigured(llm(false), [secret("scaleway", true)]),
    ).toBe(true);
  });

  it("is configured when the API key is set even if no secrets are", () => {
    expect(
      deriveProviderConfigured(llm(true), [secret("scaleway", false)]),
    ).toBe(true);
  });

  it("is NOT configured when neither a key nor any configured secret exists", () => {
    expect(
      deriveProviderConfigured(llm(false), [secret("scaleway", false)]),
    ).toBe(false);
  });

  it("is NOT configured with null settings and no secrets", () => {
    expect(deriveProviderConfigured(null, [])).toBe(false);
  });

  it("is configured with null settings but a configured Scaleway secret (pre-load fallback)", () => {
    expect(
      deriveProviderConfigured(null, [secret("scaleway", true)]),
    ).toBe(true);
  });

  it("tolerates an undefined secrets list", () => {
    expect(deriveProviderConfigured(llm(false), undefined)).toBe(false);
    expect(deriveProviderConfigured(llm(true), undefined)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Regression: fix-2 — Cloudflare-only secret must NOT configure Oracle
// ---------------------------------------------------------------------------
describe("deriveProviderConfigured — Scaleway-only reuse path (regression)", () => {
  it("apiKeyConfigured=true → true (primary signal)", () => {
    expect(deriveProviderConfigured(llm(true), [])).toBe(true);
  });

  it("only scaleway configured → true (valid reuse path)", () => {
    expect(
      deriveProviderConfigured(llm(false), [secret("scaleway", true)]),
    ).toBe(true);
  });

  it("only cloudflare configured → false (unrelated provider, must not bleed)", () => {
    expect(
      deriveProviderConfigured(llm(false), [secret("cloudflare", true)]),
    ).toBe(false);
  });

  it("nothing configured → false", () => {
    expect(deriveProviderConfigured(null, [])).toBe(false);
  });

  it("both cloudflare and scaleway configured → true (scaleway match)", () => {
    expect(
      deriveProviderConfigured(llm(false), [
        secret("cloudflare", true),
        secret("scaleway", true),
      ]),
    ).toBe(true);
  });

  it("cloudflare configured + scaleway NOT configured → false", () => {
    expect(
      deriveProviderConfigured(llm(false), [
        secret("cloudflare", true),
        secret("scaleway", false),
      ]),
    ).toBe(false);
  });
});
