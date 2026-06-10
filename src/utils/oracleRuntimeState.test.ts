import { describe, it, expect } from "vitest";
import { classifyRuntimeStage } from "./oracleRuntimeState";
import type { OracleRuntimeSetup } from "../types/backend";

// Minimal not-ready setup; individual tests override the relevant fields.
function baseSetup(over: Partial<OracleRuntimeSetup> = {}): OracleRuntimeSetup {
  return {
    pythonFound: false,
    pythonCommand: null,
    pythonVersion: null,
    venvReady: false,
    depsReady: false,
    embedderReady: false,
    ready: false,
    embedModel: "Qwen3-Embedding",
    messages: [],
    ...over,
  };
}

describe("classifyRuntimeStage", () => {
  it("returns needsInstall when Python is found (even with a stale checking hint)", () => {
    expect(
      classifyRuntimeStage(
        baseSetup({ pythonFound: true, messages: ["still checking…"] }),
      ),
    ).toBe("needsInstall");
    // Also when checking flag would otherwise be true: pythonFound wins.
    expect(
      classifyRuntimeStage(baseSetup({ pythonFound: true, checking: true })),
    ).toBe("needsInstall");
  });

  it("returns checking when the additive flag is true", () => {
    expect(
      classifyRuntimeStage(baseSetup({ pythonFound: false, checking: true })),
    ).toBe("checking");
  });

  it("returns missingPython when the flag is explicitly false (probe finished)", () => {
    expect(
      classifyRuntimeStage(baseSetup({ pythonFound: false, checking: false })),
    ).toBe("missingPython");
  });

  it("ignores a stale checking MESSAGE when the flag authoritatively says false", () => {
    expect(
      classifyRuntimeStage(
        baseSetup({
          pythonFound: false,
          checking: false,
          messages: ["still checking the runtime…"],
        }),
      ),
    ).toBe("missingPython");
  });

  it("falls back to message sniffing when the flag is absent (older backend)", () => {
    expect(
      classifyRuntimeStage(
        baseSetup({
          pythonFound: false,
          messages: ["Checking the local runtime, this can be slow…"],
        }),
      ),
    ).toBe("checking");
    expect(
      classifyRuntimeStage(
        baseSetup({
          pythonFound: false,
          messages: ["Python probe timed out on a busy machine"],
        }),
      ),
    ).toBe("checking");
  });

  it("returns missingPython when the flag is absent and no message hints checking", () => {
    expect(
      classifyRuntimeStage(
        baseSetup({
          pythonFound: false,
          messages: ["No interpreter on PATH"],
        }),
      ),
    ).toBe("missingPython");
    // Empty messages, no flag -> genuinely missing.
    expect(classifyRuntimeStage(baseSetup({ pythonFound: false }))).toBe(
      "missingPython",
    );
  });
});
