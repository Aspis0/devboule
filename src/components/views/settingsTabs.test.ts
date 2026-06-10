import { describe, expect, it } from "vitest";

import { mapLegacySettingsTab, type SettingsTabId } from "./settingsTabs";

describe("mapLegacySettingsTab", () => {
  // The FULL legacy → new contract. Every old Settings tab id must land on a
  // valid Phase-5 tab. oracle→providers and secrets/devices→security are the
  // load-bearing redirects (AskErrorCard provider link, Secrets/Devices re-home).
  const cases: Array<[string, SettingsTabId]> = [
    ["account", "account"],
    ["secrets", "security"],
    ["devices", "security"],
    ["workspace", "workspace"],
    ["oracle", "providers"],
    // New ids are idempotent (already-migrated links pass through unchanged).
    ["providers", "providers"],
    ["security", "security"],
  ];

  it.each(cases)("maps %s → %s", (old, expected) => {
    expect(mapLegacySettingsTab(old)).toBe(expected);
  });

  it("falls back to account for an unknown id", () => {
    expect(mapLegacySettingsTab("totally-bogus")).toBe("account");
  });

  it("falls back to account for an empty / whitespace id", () => {
    expect(mapLegacySettingsTab("")).toBe("account");
    expect(mapLegacySettingsTab("   ")).toBe("account");
  });

  it("trims surrounding whitespace before mapping", () => {
    expect(mapLegacySettingsTab("  oracle  ")).toBe("providers");
  });
});
