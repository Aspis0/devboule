import { describe, expect, it } from "vitest";

import {
  availabilityLabel,
  baseLabel,
  buildProviderStatusMap,
  isKindBlocked,
  offlineHttpHint,
  selectedUnavailableHint,
  selectorLabel,
  type ProviderStatus,
} from "./designProviderDetection";
import type { DetectedProvider } from "../../types/config";

const status = (over: Partial<ProviderStatus>): ProviderStatus => ({
  kind: "codex",
  available: false,
  detail: "",
  models: [],
  ...over,
});

describe("buildProviderStatusMap", () => {
  it("returns an all-unavailable map (except api) for null/undefined input", () => {
    const map = buildProviderStatusMap(null);
    expect(map.claude.available).toBe(false);
    expect(map.codex.available).toBe(false);
    expect(map.ollama.available).toBe(false);
    expect(map.omlx.available).toBe(false);
    // api is always configurable.
    expect(map.api.available).toBe(true);
    expect(buildProviderStatusMap(undefined).codex.available).toBe(false);
  });

  it("maps detected availability + detail + models per kind", () => {
    const detected: DetectedProvider[] = [
      { kind: "claude", available: true, detail: "cli only", models: [] },
      { kind: "codex", available: false, models: [] },
      { kind: "ollama", available: true, detail: "running", models: ["qwen2.5-coder", "llama3.1"] },
    ];
    const map = buildProviderStatusMap(detected);
    expect(map.claude.available).toBe(true);
    expect(map.claude.detail).toBe("cli only");
    expect(map.codex.available).toBe(false);
    expect(map.ollama.models).toEqual(["qwen2.5-coder", "llama3.1"]);
  });

  it("W2: ignores any stray `path` field on a raw entry (no path in the status)", () => {
    // Even if a stale/hand-edited IPC payload carries a path, the status map must not
    // surface it — there is no `path` on ProviderStatus.
    const detected = [
      { kind: "claude", available: true, path: "/usr/bin/claude", models: [] },
    ] as unknown as DetectedProvider[];
    const map = buildProviderStatusMap(detected);
    expect(map.claude.available).toBe(true);
    expect("path" in map.claude).toBe(false);
  });

  it("ignores unknown kinds and duplicate entries (first wins)", () => {
    const detected = [
      { kind: "bogus", available: true, models: [] },
      { kind: "ollama", available: true, models: ["first"] },
      { kind: "ollama", available: false, models: ["second"] },
    ] as unknown as DetectedProvider[];
    const map = buildProviderStatusMap(detected);
    expect(map.ollama.available).toBe(true);
    expect(map.ollama.models).toEqual(["first"]);
  });

  it("coerces untrusted shapes (non-string fields, huge/dirty model arrays)", () => {
    const detected = [
      {
        kind: "ollama",
        available: 1, // not strictly true -> false
        detail: 42,
        models: ["ok", "", "ok", 7, "  spaced  "],
      },
    ] as unknown as DetectedProvider[];
    const map = buildProviderStatusMap(detected);
    expect(map.ollama.available).toBe(false); // only === true counts
    expect(map.ollama.detail).toBe("");
    // empty/duplicate/non-string dropped; whitespace trimmed.
    expect(map.ollama.models).toEqual(["ok", "spaced"]);
  });

  it("caps the model count at 100", () => {
    const many = Array.from({ length: 250 }, (_, i) => `m${i}`);
    const map = buildProviderStatusMap([
      { kind: "ollama", available: true, models: many },
    ] as DetectedProvider[]);
    expect(map.ollama.models).toHaveLength(100);
  });
});

describe("labels", () => {
  it("baseLabel marks claude as a subscription option", () => {
    expect(baseLabel("claude")).toBe("Claude (subscription)");
    expect(baseLabel("codex")).toBe("Codex (subscription)");
  });

  it("availabilityLabel: api is always configurable", () => {
    expect(availabilityLabel(status({ kind: "api", available: true }))).toBe(
      "configure a command",
    );
  });

  it("availabilityLabel: unavailable CLI is 'not found'", () => {
    expect(availabilityLabel(status({ kind: "claude", available: false }))).toBe(
      "not found",
    );
  });

  it("availabilityLabel: detected CLI shows detail when present", () => {
    expect(
      availabilityLabel(status({ kind: "codex", available: true, detail: "cli only" })),
    ).toBe("detected (cli only)");
    expect(availabilityLabel(status({ kind: "codex", available: true }))).toBe(
      "detected",
    );
  });

  it("availabilityLabel: HTTP provider shows running + model count (singular/plural)", () => {
    expect(
      availabilityLabel(status({ kind: "ollama", available: true, models: ["a"] })),
    ).toBe("running (1 model)");
    expect(
      availabilityLabel(status({ kind: "ollama", available: true, models: ["a", "b"] })),
    ).toBe("running (2 models)");
    expect(availabilityLabel(status({ kind: "omlx", available: true }))).toBe(
      "running",
    );
  });

  it("selectorLabel composes base + availability", () => {
    expect(selectorLabel(status({ kind: "claude", available: false }))).toBe(
      "Claude (subscription) — not found",
    );
  });
});

describe("selectedUnavailableHint / isKindBlocked", () => {
  it("an unavailable CLI provider yields a hint and blocks save", () => {
    const map = buildProviderStatusMap([
      { kind: "claude", available: false, models: [] },
    ] as DetectedProvider[]);
    const hint = selectedUnavailableHint("claude", map);
    expect(hint).toContain("Claude was not found");
    expect(hint).toContain("PATH");
    expect(isKindBlocked("claude", map)).toBe(true);
  });

  it("an available CLI provider has no hint and does not block", () => {
    const map = buildProviderStatusMap([
      { kind: "codex", available: true, models: [] },
    ] as DetectedProvider[]);
    expect(selectedUnavailableHint("codex", map)).toBeNull();
    expect(isKindBlocked("codex", map)).toBe(false);
  });

  it("api is never blocked, even though not 'detected'", () => {
    const map = buildProviderStatusMap(null);
    expect(selectedUnavailableHint("api", map)).toBeNull();
    expect(isKindBlocked("api", map)).toBe(false);
  });

  it("ollama/omlx are never HARD-blocked even when unavailable (soft hint instead)", () => {
    const map = buildProviderStatusMap(null);
    expect(isKindBlocked("ollama", map)).toBe(false);
    expect(isKindBlocked("omlx", map)).toBe(false);
    expect(selectedUnavailableHint("ollama", map)).toBeNull();
  });
});

describe("offlineHttpHint", () => {
  it("returns a soft hint for an offline ollama/omlx", () => {
    const map = buildProviderStatusMap(null);
    expect(offlineHttpHint("ollama", map)).toContain("Ollama was not detected");
    expect(offlineHttpHint("omlx", map)).toContain("oMLX server was not detected");
  });

  it("returns null when the HTTP provider is available, or for non-HTTP kinds", () => {
    const map = buildProviderStatusMap([
      { kind: "ollama", available: true, models: ["a"] },
    ] as DetectedProvider[]);
    expect(offlineHttpHint("ollama", map)).toBeNull();
    expect(offlineHttpHint("claude", map)).toBeNull();
    expect(offlineHttpHint("api", map)).toBeNull();
  });
});
