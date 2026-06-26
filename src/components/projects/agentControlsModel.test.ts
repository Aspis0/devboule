import { describe, expect, it } from "vitest";
import {
  controlsEqual,
  effectiveAgentControls,
  setAgentControlsArgs,
} from "./agentControlsModel";

describe("effectiveAgentControls", () => {
  it("maps undefined to an empty object", () => {
    expect(effectiveAgentControls(undefined)).toEqual({});
  });
  it("passes a present object through", () => {
    expect(effectiveAgentControls({ effort: "high" })).toEqual({ effort: "high" });
  });
});

describe("setAgentControlsArgs", () => {
  it("keeps set fields and wraps them under controls + projectId", () => {
    const args = setAgentControlsArgs("p1", {
      effort: "high",
      systemPrompt: "be terse",
      maxTurns: 5,
      maxBudgetUsd: 2.5,
    });
    expect(args).toEqual({
      projectId: "p1",
      controls: { effort: "high", systemPrompt: "be terse", maxTurns: 5, maxBudgetUsd: 2.5 },
    });
  });

  it("strips empty / whitespace strings to undefined (NO-CHURN)", () => {
    const args = setAgentControlsArgs("p1", { effort: "  ", systemPrompt: "" });
    expect(args).toEqual({ projectId: "p1", controls: {} });
  });

  it("drops non-positive / non-finite numbers", () => {
    expect(setAgentControlsArgs("p1", { maxTurns: 0, maxBudgetUsd: -1 })).toEqual({
      projectId: "p1",
      controls: {},
    });
    expect(setAgentControlsArgs("p1", { maxTurns: NaN })).toEqual({
      projectId: "p1",
      controls: {},
    });
  });

  it("floors a fractional maxTurns but keeps fractional budget", () => {
    const args = setAgentControlsArgs("p1", { maxTurns: 5.9, maxBudgetUsd: 1.25 });
    expect(args.controls).toEqual({ maxTurns: 5, maxBudgetUsd: 1.25 });
  });

  it("trims a system prompt", () => {
    const args = setAgentControlsArgs("p1", { systemPrompt: "  hello  " });
    expect(args.controls).toEqual({ systemPrompt: "hello" });
  });
});

describe("controlsEqual", () => {
  it("treats undefined / empty-string / whitespace effort as equal (normalized)", () => {
    expect(controlsEqual({}, { effort: "" })).toBe(true);
    expect(controlsEqual({ effort: "  " }, {})).toBe(true);
    expect(controlsEqual({ systemPrompt: "" }, {})).toBe(true);
  });
  it("treats non-positive numbers as equal to unset", () => {
    expect(controlsEqual({ maxTurns: 0 }, {})).toBe(true);
    expect(controlsEqual({ maxBudgetUsd: -1 }, {})).toBe(true);
  });
  it("distinguishes genuinely different values", () => {
    expect(controlsEqual({ effort: "high" }, { effort: "low" })).toBe(false);
    expect(controlsEqual({ maxTurns: 5 }, {})).toBe(false);
  });
  it("floors maxTurns when comparing", () => {
    expect(controlsEqual({ maxTurns: 5.9 }, { maxTurns: 5 })).toBe(true);
  });
});
