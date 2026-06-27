import { describe, expect, it } from "vitest";

import { validateCensorLocalAi } from "./censorLocalAi";

// Pure validation for the Censor local-AI provider config. Mirrors the mini-coder
// backend validation tests; the loopback/http rules are the SHARED validateOmlxBaseUrl
// (covered exhaustively in agents/miniCoderBackend.test.ts), so here we focus on the
// provider-specific shape: ollama default carries no fields, omlx requires base+model
// and a loopback http base, and a provider switch drops the now-unused fields.

describe("validateCensorLocalAi", () => {
  it("accepts the ollama default and persists only the provider (no base/model)", () => {
    const v = validateCensorLocalAi({ provider: "ollama", baseUrl: "", model: "" });
    expect(v.ok).toBe(true);
    expect(v.value).toEqual({ provider: "ollama" });
  });

  it("accepts appleFm without baseUrl/model and persists only the provider", () => {
    const v = validateCensorLocalAi({
      provider: "appleFm",
      baseUrl: "",
      model: "",
    });
    expect(v.ok).toBe(true);
    expect(v.value).toEqual({ provider: "appleFm" });
  });

  it("preserves an optional appleFm model when provided", () => {
    const v = validateCensorLocalAi({
      provider: "appleFm",
      baseUrl: "",
      model: "apple-default",
    });
    expect(v.ok).toBe(true);
    expect(v.value).toEqual({ provider: "appleFm", model: "apple-default" });
  });

  it("drops omlx-only draft fields when switching back to ollama (no stale bleed)", () => {
    // A draft that still carries an omlx base/model but provider==ollama must persist
    // as the bare ollama default — the omlx fields are dropped from the payload.
    const v = validateCensorLocalAi({
      provider: "ollama",
      baseUrl: "http://localhost:8000/v1",
      model: "mlx-community/gemma",
    });
    expect(v.ok).toBe(true);
    expect(v.value).toEqual({ provider: "ollama" });
    expect(v.value).not.toHaveProperty("baseUrl");
    expect(v.value).not.toHaveProperty("model");
  });

  it("drops omlx-only base URL when switching to appleFm but preserves its model", () => {
    const v = validateCensorLocalAi({
      provider: "appleFm",
      baseUrl: "http://localhost:8000/v1",
      model: "mlx-community/gemma",
    });
    expect(v.ok).toBe(true);
    expect(v.value).toEqual({ provider: "appleFm", model: "mlx-community/gemma" });
    expect(v.value).not.toHaveProperty("baseUrl");
  });

  it("accepts a valid omlx config and normalizes the base (trailing slash stripped)", () => {
    const v = validateCensorLocalAi({
      provider: "omlx",
      baseUrl: "http://localhost:8000/v1/",
      model: "mlx-community/gemma",
    });
    expect(v.ok).toBe(true);
    expect(v.value).toEqual({
      provider: "omlx",
      baseUrl: "http://localhost:8000/v1",
      model: "mlx-community/gemma",
    });
  });

  it("requires a base URL AND a model for omlx", () => {
    const v = validateCensorLocalAi({ provider: "omlx", baseUrl: "", model: "" });
    expect(v.ok).toBe(false);
    expect(v.value).toBeNull();
    expect(v.errors.baseUrl).toMatch(/Enter the oMLX server base URL/);
    expect(v.errors.model).toMatch(/Enter the oMLX model id/);
  });

  it("rejects a non-loopback omlx base (privacy: code must stay on-device)", () => {
    const v = validateCensorLocalAi({
      provider: "omlx",
      baseUrl: "http://evil.com/v1",
      model: "m",
    });
    expect(v.ok).toBe(false);
    expect(v.errors.baseUrl).toMatch(/loopback http origin/);
  });

  it("rejects an https omlx base (self-signed-TLS silent-degrade trap)", () => {
    const v = validateCensorLocalAi({
      provider: "omlx",
      baseUrl: "https://localhost:8000/v1",
      model: "m",
    });
    expect(v.ok).toBe(false);
    expect(v.errors.baseUrl).toMatch(/loopback http origin/);
  });

  it("rejects a bad port on an otherwise-loopback omlx base", () => {
    const v = validateCensorLocalAi({
      provider: "omlx",
      baseUrl: "http://localhost:99999/v1",
      model: "m",
    });
    expect(v.ok).toBe(false);
    expect(v.errors.baseUrl).toMatch(/loopback http origin/);
  });

  it("rejects the userinfo loopback-spoof trick (127.0.0.1@evil.com)", () => {
    const v = validateCensorLocalAi({
      provider: "omlx",
      baseUrl: "http://127.0.0.1@evil.com/v1",
      model: "m",
    });
    expect(v.ok).toBe(false);
    expect(v.errors.baseUrl).toMatch(/loopback http origin/);
  });

  it("accepts org/name HF-path omlx models (the / is a valid char)", () => {
    // PARITY (max-recall FIX 3): an `org/name` HF-style model id is a valid bare tag.
    const v = validateCensorLocalAi({
      provider: "omlx",
      baseUrl: "http://localhost:8000/v1",
      model: "mlx-community/gemma-2-2b-it",
    });
    expect(v.ok).toBe(true);
    expect(v.value?.model).toBe("mlx-community/gemma-2-2b-it");
  });

  it("rejects an omlx model with whitespace / control / metachars (bare-tag rule)", () => {
    // PARITY (max-recall FIX 3): same char-class as the mini-coder oMLX model and the
    // Rust is_valid_model. Whitespace, shell metachars, a non-alnum first char and
    // control/bidi chars are all rejected so all oMLX model validators agree.
    for (const model of [
      "model name",
      "model;rm -rf",
      "-leading-dash",
      ".dotfirst",
      "model@host",
      "model\\path",
      "model‮evil",
      "model\ttab",
    ]) {
      const v = validateCensorLocalAi({
        provider: "omlx",
        baseUrl: "http://localhost:8000/v1",
        model,
      });
      expect(v.ok, `model ${JSON.stringify(model)} must be rejected`).toBe(false);
      expect(v.errors.model).toMatch(/bare tag/);
    }
  });

  it("preserves a present ollamaModel override on the ollama branch (split-brain fix)", () => {
    // BLOCKER 1: the provider card round-trips the existing ollamaModel so saving the
    // provider never wipes the override set by the CensorModelCard. A present override
    // makes the value NON-bare so the backend keeps the key.
    const v = validateCensorLocalAi({
      provider: "ollama",
      baseUrl: "",
      model: "",
      ollamaModel: "  gemma4:x  ",
    });
    expect(v.ok).toBe(true);
    expect(v.value).toEqual({ provider: "ollama", ollamaModel: "gemma4:x" });
  });

  it("omits an empty/whitespace ollamaModel (stays the bare ollama default)", () => {
    const v = validateCensorLocalAi({
      provider: "ollama",
      baseUrl: "",
      model: "",
      ollamaModel: "   ",
    });
    expect(v.ok).toBe(true);
    expect(v.value).toEqual({ provider: "ollama" });
    expect(v.value).not.toHaveProperty("ollamaModel");
  });

  it("validates ollamaModel with the bare-tag rule + length cap", () => {
    const bad = validateCensorLocalAi({
      provider: "ollama",
      baseUrl: "",
      model: "",
      ollamaModel: "bad name;rm",
    });
    expect(bad.ok).toBe(false);
    expect(bad.value).toBeNull();
    expect(bad.errors.ollamaModel).toMatch(/bare tag/);

    const overCap = validateCensorLocalAi({
      provider: "ollama",
      baseUrl: "",
      model: "",
      ollamaModel: "a".repeat(201),
    });
    expect(overCap.ok).toBe(false);
    expect(overCap.errors.ollamaModel).toMatch(/at most 200 characters/);
  });

  it("drops ollamaModel on the omlx branch without validating it (omlx uses model)", () => {
    // Mirrors the Rust validator: a stray ollamaModel on an oMLX config is dropped WITHOUT
    // validation, so even a structurally-bad override can never block an oMLX save.
    const v = validateCensorLocalAi({
      provider: "omlx",
      baseUrl: "http://localhost:8000/v1",
      model: "mlx-community/gemma",
      ollamaModel: "bad override;rm -rf",
    });
    expect(v.ok).toBe(true);
    expect(v.errors.ollamaModel).toBeUndefined();
    expect(v.value).toEqual({
      provider: "omlx",
      baseUrl: "http://localhost:8000/v1",
      model: "mlx-community/gemma",
    });
    expect(v.value).not.toHaveProperty("ollamaModel");
  });

  it("accepts a valid cloud config and normalizes the https base (trailing slash stripped)", () => {
    const v = validateCensorLocalAi({
      provider: "cloud",
      baseUrl: "https://openrouter.ai/api/v1/",
      model: "openai/gpt-4o-mini",
    });
    expect(v.ok).toBe(true);
    expect(v.value).toEqual({
      provider: "cloud",
      baseUrl: "https://openrouter.ai/api/v1",
      model: "openai/gpt-4o-mini",
    });
  });

  it("requires a base URL AND a model for cloud", () => {
    const v = validateCensorLocalAi({ provider: "cloud", baseUrl: "", model: "" });
    expect(v.ok).toBe(false);
    expect(v.errors.baseUrl).toBeTruthy();
    expect(v.errors.model).toBeTruthy();
  });

  it("rejects a non-https cloud base (TLS required for off-device egress)", () => {
    const v = validateCensorLocalAi({
      provider: "cloud",
      baseUrl: "http://openrouter.ai/api/v1",
      model: "m",
    });
    expect(v.ok).toBe(false);
    expect(v.errors.baseUrl).toMatch(/https origin/);
  });

  it("rejects a loopback cloud base (cloud is remote — validateCloudBaseUrl denies loopback)", () => {
    const v = validateCensorLocalAi({
      provider: "cloud",
      baseUrl: "https://localhost:8000/v1",
      model: "m",
    });
    expect(v.ok).toBe(false);
    expect(v.errors.baseUrl).toBeTruthy();
  });

  it("NEVER carries an api key in the cloud value (the key lives in the vault)", () => {
    const v = validateCensorLocalAi({
      provider: "cloud",
      baseUrl: "https://openrouter.ai/api/v1",
      model: "openai/gpt-4o-mini",
    });
    expect(v.ok).toBe(true);
    // Only provider/baseUrl/model — no apiKey field ever leaks into the persisted config.
    expect(Object.keys(v.value ?? {}).sort()).toEqual(["baseUrl", "model", "provider"]);
  });

  it("rejects an omlx model over the 200-char cap (matches Rust CENSOR_OMLX_MODEL_MAX_LEN)", () => {
    // PARITY (max-recall FIX 2): the model cap is 200, matching the Rust constant.
    const atCap = "a".repeat(200);
    const okAtCap = validateCensorLocalAi({
      provider: "omlx",
      baseUrl: "http://localhost:8000/v1",
      model: atCap,
    });
    expect(okAtCap.ok).toBe(true);

    const overCap = "a".repeat(201);
    const v = validateCensorLocalAi({
      provider: "omlx",
      baseUrl: "http://localhost:8000/v1",
      model: overCap,
    });
    expect(v.ok).toBe(false);
    expect(v.errors.model).toMatch(/at most 200 characters/);
  });
});
