// Regression test for the macOS white-screen bug (2026-06-12): a freshly
// bootstrapped backend config.json contains `{}`; the raw object used to flow
// into the shell unchanged, so `config.navigation.some(...)` threw in Sidebar
// and — with no shell-level error boundary — blanked the whole app.
import { describe, expect, it } from "vitest";
import { normalizeConfig } from "./AppContext";

describe("normalizeConfig", () => {
  it("fills every top-level default for a bootstrapped empty config", () => {
    const cfg = normalizeConfig({});
    expect(Array.isArray(cfg.navigation)).toBe(true);
    expect(cfg.navigation.length).toBeGreaterThan(0);
    expect(cfg.project.name).toBeTruthy();
    expect(Array.isArray(cfg.providers)).toBe(true);
  });

  it("falls back to the full default for non-object payloads", () => {
    for (const raw of [null, undefined, [], "nope", 42]) {
      const cfg = normalizeConfig(raw);
      expect(Array.isArray(cfg.navigation)).toBe(true);
      expect(cfg.navigation.length).toBeGreaterThan(0);
    }
  });

  it("keeps caller-provided keys while defaulting the missing ones", () => {
    const cfg = normalizeConfig({ project: { name: "Custom", version: "9" } });
    expect(cfg.project.name).toBe("Custom");
    expect(cfg.navigation.length).toBeGreaterThan(0);
  });
});
