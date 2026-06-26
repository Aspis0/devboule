import { describe, it, expect } from "vitest";
import { installHintFor } from "./censorInstallHints";

describe("installHintFor", () => {
  it("has an install hint for the common deterministic runners", () => {
    for (const tool of [
      "cargo",
      "eslint",
      "ruff",
      "gitleaks",
      "semgrep",
      "tsc",
      "shellcheck",
      "hadolint",
      "pyright",
    ]) {
      expect(installHintFor(tool), `missing install hint for ${tool}`).toBeTruthy();
    }
  });

  it("returns undefined for an unknown tool", () => {
    expect(installHintFor("definitely-not-a-real-tool")).toBeUndefined();
  });
});
