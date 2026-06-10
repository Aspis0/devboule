import { describe, it, expect } from "vitest";
import { oracleIndexPhaseHint } from "./oracleIndexPhase";

describe("oracleIndexPhaseHint", () => {
  it("returns null for the normal running / unknown / missing phase", () => {
    expect(oracleIndexPhaseHint("running", undefined)).toBeNull();
    expect(oracleIndexPhaseHint(undefined, undefined)).toBeNull();
    expect(oracleIndexPhaseHint(null, "anything")).toBeNull();
    expect(oracleIndexPhaseHint("complete", "done")).toBeNull();
  });

  it("prefers the server message (with live numbers) for cooling_gpu", () => {
    const hint = oracleIndexPhaseHint(
      "cooling_gpu",
      "GPU cooling (85°C), resuming…",
    );
    expect(hint).toEqual({
      phase: "cooling_gpu",
      label: "GPU cooling (85°C), resuming…",
    });
  });

  it("prefers the server message for waiting_memory", () => {
    const hint = oracleIndexPhaseHint(
      "waiting_memory",
      "Waiting for memory (1.4 GB free), resuming…",
    );
    expect(hint).toEqual({
      phase: "waiting_memory",
      label: "Waiting for memory (1.4 GB free), resuming…",
    });
  });

  it("falls back to a static label when the server message is absent or blank", () => {
    expect(oracleIndexPhaseHint("cooling_gpu", undefined)).toEqual({
      phase: "cooling_gpu",
      label: "GPU cooling — resuming…",
    });
    expect(oracleIndexPhaseHint("waiting_memory", "   ")).toEqual({
      phase: "waiting_memory",
      label: "Waiting for memory — resuming…",
    });
    expect(oracleIndexPhaseHint("cooling_gpu", 123)).toEqual({
      phase: "cooling_gpu",
      label: "GPU cooling — resuming…",
    });
  });
});
