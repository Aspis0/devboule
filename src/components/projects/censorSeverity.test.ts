import { describe, expect, it } from "vitest";
import {
  categoryBadgeClass,
  categoryLabel,
  fileLineLabel,
  gemmaStatusNote,
  severityRank,
  severityStyle,
  sourceBadgeClass,
  sourceLabel,
} from "./censorSeverity";

describe("censorSeverity badge mapping", () => {
  it("maps each severity to its RiskFlags palette bundle", () => {
    expect(severityStyle("high").badge).toContain("coral");
    expect(severityStyle("medium").badge).toContain("amber");
    expect(severityStyle("low").badge).toContain("teal");
  });

  it("falls back to the low style for an unknown/missing severity", () => {
    expect(severityStyle(undefined)).toBe(severityStyle("low"));
    expect(severityStyle("nope" as never)).toBe(severityStyle("low"));
  });

  it("ranks high < medium < low < unknown for sorting", () => {
    expect(severityRank("high")).toBeLessThan(severityRank("medium"));
    expect(severityRank("medium")).toBeLessThan(severityRank("low"));
    expect(severityRank("low")).toBeLessThan(severityRank("zzz" as never));
  });

  it("labels every category, defaulting unknown to a neutral 'Finding'", () => {
    expect(categoryLabel("security")).toBe("Security");
    expect(categoryLabel("dead-code")).toBe("Dead code");
    expect(categoryLabel("style")).toBe("Style");
    expect(categoryLabel(undefined)).toBe("Finding");
  });

  it("gives security a coral accent and other categories a neutral pill", () => {
    expect(categoryBadgeClass("security")).toContain("coral");
    expect(categoryBadgeClass("style")).not.toContain("coral");
  });

  it("gives gemma a teal source pill and linters a neutral one", () => {
    expect(sourceBadgeClass("gemma")).toContain("teal");
    expect(sourceBadgeClass("GEMMA")).toContain("teal");
    expect(sourceBadgeClass("clippy")).not.toContain("teal");
  });

  it("labels a source, capping a runaway value and defaulting an empty one", () => {
    expect(sourceLabel("eslint")).toBe("eslint");
    expect(sourceLabel("")).toBe("linter");
    expect(sourceLabel("x".repeat(40)).endsWith("…")).toBe(true);
  });
});

describe("fileLineLabel", () => {
  it("renders file:line for a positive line", () => {
    expect(fileLineLabel("src/app.ts", 12)).toBe("src/app.ts:12");
  });

  it("renders just the file for a null / non-positive line", () => {
    expect(fileLineLabel("src/app.ts", null)).toBe("src/app.ts");
    expect(fileLineLabel("src/app.ts", 0)).toBe("src/app.ts");
  });

  it("handles a missing file gracefully", () => {
    expect(fileLineLabel(undefined, 5)).toBe("(unknown file):5");
  });
});

describe("gemmaStatusNote", () => {
  it("shows the offline banner ONLY when the tier is offline", () => {
    expect(gemmaStatusNote("offline")).toContain("Gemma layer offline");
    expect(gemmaStatusNote("available")).toBeNull();
    expect(gemmaStatusNote("unknown")).toBeNull();
    expect(gemmaStatusNote(undefined)).toBeNull();
  });
});
