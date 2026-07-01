import { describe, expect, it } from "vitest";
import { displayRole } from "./roleDisplay";
import { ROLE_OPTIONS } from "./SpawnPanel";

// ROLE UNTANGLE (2026-07): displayRole is a PASS-THROUGH of the stored role for
// the four first-class roles. The former "derived orchestrator badge from
// subagent count" heuristic is DEAD: the ledger stores role:"orchestrator"
// truthfully for every planner launch (local binary AND cloud duplex), so the
// badge simply reflects the stored role.
describe("displayRole", () => {
  it("keeps a stored 'orchestrator' role first-class (with badge)", () => {
    expect(displayRole({ role: "orchestrator" })).toEqual({
      role: "orchestrator",
      orchestratorBadge: true,
    });
  });

  it("does NOT promote a coder with subagents (the heuristic is dead)", () => {
    expect(
      displayRole({
        role: "coder",
        subagents: [{ label: "helpers", model: "haiku", count: 2 }],
      }),
    ).toEqual({ role: "coder", orchestratorBadge: false });
  });

  it("plain coder with no subagents -> coder, no badge", () => {
    expect(displayRole({ role: "coder" })).toEqual({
      role: "coder",
      orchestratorBadge: false,
    });
  });

  it("verifier -> verifier, no badge", () => {
    expect(displayRole({ role: "verifier" })).toEqual({
      role: "verifier",
      orchestratorBadge: false,
    });
  });

  it("a verifier WITH subagents is never promoted", () => {
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
      role: "orchestrator",
      orchestratorBadge: true,
    });
    expect(displayRole({ role: "VERIFIER" })).toEqual({
      role: "verifier",
      orchestratorBadge: false,
    });
  });
});

describe("SpawnPanel ROLE_OPTIONS", () => {
  // The SpawnPanel role picker stays {coder, verifier}: an orchestrator is
  // launched via the planner ("Plan it") or the Devboule client selection, not
  // via the role radio. (The Roles-table phase may revisit this.)
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
