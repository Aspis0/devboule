import { describe, expect, it } from "vitest";
import {
  effectiveSandboxMode,
  setSandboxModeArgs,
  shouldAdoptProp,
  SANDBOX_MODES,
  type SandboxMode,
} from "./sandboxModeModel";

// ── shouldAdoptProp ───────────────────────────────────────────────────────────
//
// This helper gates the prop-sync useEffect so a confirmed optimistic value is
// never clobbered by a stale prop arriving before the parent has refreshed.

describe("shouldAdoptProp", () => {
  // ── busy = true: never adopt ─────────────────────────────────────────────
  it("returns false when busy, regardless of pending or incoming", () => {
    expect(shouldAdoptProp("ask", null, true)).toBe(false);
    expect(shouldAdoptProp("ask", "ask", true)).toBe(false);
    expect(shouldAdoptProp("unattended", "ask", true)).toBe(false);
  });

  // ── no pending, not busy: always adopt ───────────────────────────────────
  it("returns true when not busy and no pending value (normal external refresh)", () => {
    expect(shouldAdoptProp("ask", null, false)).toBe(true);
    expect(shouldAdoptProp("unattended", null, false)).toBe(true);
    expect(shouldAdoptProp("autoAcceptInWorkspace", null, false)).toBe(true);
  });

  // ── pending set, not busy: stale prop must NOT clobber confirmed value ────
  it("returns false when incoming differs from the pending confirmed value", () => {
    // User just confirmed "unattended"; parent still sends the old "ask" prop.
    expect(shouldAdoptProp("ask", "unattended", false)).toBe(false);
    // User confirmed "autoAcceptInWorkspace"; parent sends "ask".
    expect(shouldAdoptProp("ask", "autoAcceptInWorkspace", false)).toBe(false);
  });

  it("returns true when the incoming prop matches the pending confirmed value (parent caught up)", () => {
    expect(shouldAdoptProp("unattended", "unattended", false)).toBe(true);
    expect(shouldAdoptProp("autoAcceptInWorkspace", "autoAcceptInWorkspace", false)).toBe(true);
    expect(shouldAdoptProp("ask", "ask", false)).toBe(true);
  });

  // ── exhaustive: every (incoming, pending) pair where they differ returns false ──
  const modes: SandboxMode[] = ["ask", "autoAcceptInWorkspace", "unattended"];
  for (const incoming of modes) {
    for (const pending of modes) {
      if (incoming !== pending) {
        it(`returns false for incoming="${incoming}" vs pending="${pending}" (stale prop)`, () => {
          expect(shouldAdoptProp(incoming, pending, false)).toBe(false);
        });
      }
    }
  }
});

// ── effectiveSandboxMode ──────────────────────────────────────────────────────

describe("effectiveSandboxMode", () => {
  it("returns 'ask' when input is undefined (absent from IPC JSON)", () => {
    expect(effectiveSandboxMode(undefined)).toBe("ask");
  });

  it("returns 'ask' when explicitly provided", () => {
    expect(effectiveSandboxMode("ask")).toBe("ask");
  });

  it("returns 'autoAcceptInWorkspace' unchanged", () => {
    expect(effectiveSandboxMode("autoAcceptInWorkspace")).toBe(
      "autoAcceptInWorkspace",
    );
  });

  it("returns 'unattended' unchanged", () => {
    expect(effectiveSandboxMode("unattended")).toBe("unattended");
  });

  // Exhaustive coverage: every defined mode round-trips through the helper.
  const modes: SandboxMode[] = ["ask", "autoAcceptInWorkspace", "unattended"];
  for (const m of modes) {
    it(`round-trips mode "${m}" unchanged`, () => {
      expect(effectiveSandboxMode(m)).toBe(m);
    });
  }
});

// ── setSandboxModeArgs ────────────────────────────────────────────────────────

describe("setSandboxModeArgs", () => {
  it("emits the exact camelCase JSON shape the Rust backend expects (ask)", () => {
    const args = setSandboxModeArgs("proj-abc", "ask");
    expect(args).toEqual({ projectId: "proj-abc", mode: "ask" });
  });

  it("emits camelCase mode string for autoAcceptInWorkspace", () => {
    const args = setSandboxModeArgs("proj-abc", "autoAcceptInWorkspace");
    expect(args).toEqual({
      projectId: "proj-abc",
      mode: "autoAcceptInWorkspace",
    });
  });

  it("emits camelCase mode string for unattended", () => {
    const args = setSandboxModeArgs("proj-abc", "unattended");
    expect(args).toEqual({ projectId: "proj-abc", mode: "unattended" });
  });

  it("passes projectId unchanged", () => {
    const args = setSandboxModeArgs("my-project-id", "ask");
    expect(args.projectId).toBe("my-project-id");
  });

  it("only contains projectId and mode keys (no extras)", () => {
    const args = setSandboxModeArgs("p", "ask");
    expect(Object.keys(args).sort()).toEqual(["mode", "projectId"]);
  });

  // All three modes produce different 'mode' values.
  it("all three modes produce distinct mode values", () => {
    const modes: SandboxMode[] = ["ask", "autoAcceptInWorkspace", "unattended"];
    const results = modes.map((m) => setSandboxModeArgs("p", m).mode);
    expect(new Set(results).size).toBe(3);
  });
});

// ── SANDBOX_MODES descriptor table ───────────────────────────────────────────

describe("SANDBOX_MODES", () => {
  it("contains exactly three entries", () => {
    expect(SANDBOX_MODES).toHaveLength(3);
  });

  it("first entry is 'ask' (least autonomous)", () => {
    expect(SANDBOX_MODES[0].value).toBe("ask");
  });

  it("second entry is 'autoAcceptInWorkspace'", () => {
    expect(SANDBOX_MODES[1].value).toBe("autoAcceptInWorkspace");
  });

  it("third entry is 'unattended' (most autonomous)", () => {
    expect(SANDBOX_MODES[2].value).toBe("unattended");
  });

  it("every entry has a non-empty label", () => {
    for (const d of SANDBOX_MODES) {
      expect(d.label.length).toBeGreaterThan(0);
    }
  });

  it("every entry has a non-empty description", () => {
    for (const d of SANDBOX_MODES) {
      expect(d.description.length).toBeGreaterThan(0);
    }
  });

  it("all values are distinct", () => {
    const values = SANDBOX_MODES.map((d) => d.value);
    expect(new Set(values).size).toBe(SANDBOX_MODES.length);
  });

  it("all labels are distinct", () => {
    const labels = SANDBOX_MODES.map((d) => d.label);
    expect(new Set(labels).size).toBe(SANDBOX_MODES.length);
  });
});
