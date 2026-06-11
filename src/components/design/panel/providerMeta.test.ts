import { describe, it, expect } from "vitest";
import { providerMeta, DESIGN_PROVIDERS } from "./types";

describe("providerMeta", () => {
  it("returns the matching provider metadata for a known kind", () => {
    expect(providerMeta("ollama").name).toBe("Ollama");
    expect(providerMeta("claude").name).toBe("Claude Code");
  });

  it("returns an explicit 'Unknown provider' for an unknown/legacy kind (not the first entry)", () => {
    const meta = providerMeta("totally-unknown");
    expect(meta.name).toBe("Unknown provider");
    // It must NOT masquerade as the first real provider.
    expect(meta.name).not.toBe(DESIGN_PROVIDERS[0].name);
  });

  it("returns 'Unknown provider' for undefined", () => {
    expect(providerMeta(undefined).name).toBe("Unknown provider");
  });
});
