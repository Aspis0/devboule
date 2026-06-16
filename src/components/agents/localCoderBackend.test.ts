// Tests for validateLocalBackend — the pure validation helper for the LOCAL MAIN-CODER
// backend (Settings → "Local main coder (Devboule)"). The local kinds (ollama/omlx) are a
// strict subset of the mini's with the SAME rules (this helper delegates to
// validateMiniBackend), so here we focus on: the two kinds validate + normalize, the
// re-map keeps only the kind's fields, and a kind switch never leaves a stale baseUrl.

import { describe, it, expect } from "vitest";
import { validateLocalBackend } from "./localCoderBackend";

describe("validateLocalBackend — ollama", () => {
  it("requires a model and keeps only the model (drops baseUrl)", () => {
    const bad = validateLocalBackend({ kind: "ollama", model: "" });
    expect(bad.ok).toBe(false);
    expect(bad.errors.model).toBeTruthy();
    expect(bad.value).toBeNull();

    const ok = validateLocalBackend({
      kind: "ollama",
      model: "  qwen2.5-coder  ",
      baseUrl: "http://localhost:8000/v1",
    });
    expect(ok.ok).toBe(true);
    expect(ok.value).toEqual({ kind: "ollama", model: "qwen2.5-coder" });
    // baseUrl is dropped for ollama (resolved by the launch, not stored).
    expect(ok.value && "baseUrl" in ok.value).toBe(false);
  });

  it("rejects a model with whitespace/metachars", () => {
    for (const m of ["has space", "with;semi", "pipe|x", "$(sub)"]) {
      expect(validateLocalBackend({ kind: "ollama", model: m }).ok).toBe(false);
    }
  });
});

describe("validateLocalBackend — omlx", () => {
  it("requires both a model and a loopback http base URL", () => {
    expect(
      validateLocalBackend({ kind: "omlx", model: "m", baseUrl: "" }).ok,
    ).toBe(false);
    expect(
      validateLocalBackend({ kind: "omlx", model: "", baseUrl: "http://localhost:8000/v1" })
        .ok,
    ).toBe(false);
  });

  it("normalizes a loopback base URL (trailing slash stripped) and trims the model", () => {
    const ok = validateLocalBackend({
      kind: "omlx",
      model: "  mlx-qwen  ",
      baseUrl: "  http://127.0.0.1:8000/v1/  ",
    });
    expect(ok.ok).toBe(true);
    expect(ok.value).toEqual({
      kind: "omlx",
      model: "mlx-qwen",
      baseUrl: "http://127.0.0.1:8000/v1",
    });
  });

  it("rejects https / non-loopback / userinfo tricks (same set as the mini + Rust)", () => {
    for (const bad of [
      "https://localhost:8000/v1",
      "http://evil.com:8000/v1",
      "http://127.0.0.1.evil.com/v1",
      "http://127.0.0.1@evil.com/v1",
      "ftp://localhost/v1",
    ]) {
      const v = validateLocalBackend({ kind: "omlx", model: "m", baseUrl: bad });
      expect(v.ok, `${bad} must be rejected`).toBe(false);
      expect(v.errors.baseUrl).toBeTruthy();
    }
  });
});

describe("validateLocalBackend — error keys are narrowed to model/baseUrl", () => {
  it("never surfaces a command error (no local kind uses a command)", () => {
    const v = validateLocalBackend({ kind: "ollama", model: "" });
    // The narrowed error map only ever has model/baseUrl keys.
    expect(Object.keys(v.errors).every((k) => k === "model" || k === "baseUrl")).toBe(
      true,
    );
  });
});
