import { describe, expect, it } from "vitest";
import { displayRole } from "./roleDisplay";
import { ROLE_OPTIONS } from "./SpawnPanel";

describe("displayRole", () => {
  it("maps a stored legacy 'orchestrator' role to coder + badge", () => {
    expect(displayRole({ role: "orchestrator" })).toEqual({
      role: "coder",
      orchestratorBadge: true,
    });
  });

  it("raises the badge for a coder that currently has subagents", () => {
    expect(
      displayRole({
        role: "coder",
        subagents: [{ label: "helpers", model: "haiku", count: 2 }],
      }),
    ).toEqual({ role: "coder", orchestratorBadge: true });
  });

  it("plain coder with no subagents -> coder, no badge", () => {
    expect(displayRole({ role: "coder" })).toEqual({
      role: "coder",
      orchestratorBadge: false,
    });
  });

  it("verifier -> verifier, no badge (even with no subagents field)", () => {
    expect(displayRole({ role: "verifier" })).toEqual({
      role: "verifier",
      orchestratorBadge: false,
    });
  });

  it("is null-safe on an empty subagents array (no badge)", () => {
    expect(displayRole({ role: "coder", subagents: [] })).toEqual({
      role: "coder",
      orchestratorBadge: false,
    });
  });

  it("a verifier WITH subagents is NEVER promoted (no orchestrator badge)", () => {
    // NITPICK 1: a verifier is never an orchestrator, even when it fans out. This
    // mirrors the Rust Polis `derived_agent_type`, which only promotes a coder.
    expect(
      displayRole({
        role: "verifier",
        subagents: [{ label: "scouts", model: "haiku", count: 1 }],
      }),
    ).toEqual({ role: "verifier", orchestratorBadge: false });
  });

  it("folds an unknown/empty stored role to coder without a badge", () => {
    expect(displayRole({ role: "" })).toEqual({
      role: "coder",
      orchestratorBadge: false,
    });
    expect(displayRole({ role: "augur" })).toEqual({
      role: "coder",
      orchestratorBadge: false,
    });
  });

  it("normalizes case and surrounding whitespace on the stored role", () => {
    expect(displayRole({ role: " Orchestrator " })).toEqual({
      role: "coder",
      orchestratorBadge: true,
    });
    expect(displayRole({ role: "VERIFIER" })).toEqual({
      role: "verifier",
      orchestratorBadge: false,
    });
  });
});

describe("SpawnPanel ROLE_OPTIONS", () => {
  it("offers only the two spawnable roles (no orchestrator)", () => {
    expect(ROLE_OPTIONS.map((option) => option.id)).toEqual([
      "coder",
      "verifier",
    ]);
  });

  it("never offers 'orchestrator' as a spawn choice", () => {
    expect(
      ROLE_OPTIONS.some((option) => String(option.id) === "orchestrator"),
    ).toBe(false);
  });
});
