import { describe, it, expect } from "vitest";
import {
  parseViewTarget,
  formatViewTarget,
  parseWorkTab,
  attentionBellTarget,
  shouldClearWorkEntryBridge,
  shouldExitWorkMode,
} from "./deepLink";

describe("parseViewTarget", () => {
  it("parses a bare view with no tab", () => {
    expect(parseViewTarget("projects")).toEqual({
      view: "projects",
      tab: null,
    });
  });

  it("parses a view#tab target", () => {
    expect(parseViewTarget("settings#secrets")).toEqual({
      view: "settings",
      tab: "secrets",
    });
  });

  it("parses a settings#secrets target", () => {
    expect(parseViewTarget("settings#secrets")).toEqual({
      view: "settings",
      tab: "secrets",
    });
  });

  it("trims surrounding whitespace", () => {
    expect(parseViewTarget("  settings#devices  ")).toEqual({
      view: "settings",
      tab: "devices",
    });
  });

  it("treats an empty tab after # as no tab", () => {
    expect(parseViewTarget("settings#")).toEqual({
      view: "settings",
      tab: null,
    });
  });

  it("ignores extra # segments, keeping only the first tab", () => {
    expect(parseViewTarget("settings#secrets#extra")).toEqual({
      view: "settings",
      tab: "secrets",
    });
  });

  it("returns an empty view for an empty string (caller decides fallback)", () => {
    expect(parseViewTarget("")).toEqual({ view: "", tab: null });
  });

  it("parses a projects#work:<id> Work-mode target (Phase G bell deep-link)", () => {
    expect(parseViewTarget("projects#work:abc")).toEqual({
      view: "projects",
      tab: "work:abc",
    });
  });
});

describe("parseWorkTab", () => {
  it("maps a work:<id> token to a Work-mode selection", () => {
    expect(parseWorkTab("work:abc")).toEqual({
      selectedId: "abc",
      workMode: true,
    });
  });

  it("trims whitespace around the token and the id", () => {
    expect(parseWorkTab("  work: abc ")).toEqual({
      selectedId: "abc",
      workMode: true,
    });
  });

  it("returns null for the plain agents token (dissolved page)", () => {
    expect(parseWorkTab("agents")).toBeNull();
  });

  it("returns null for a non-work tab", () => {
    expect(parseWorkTab("board")).toBeNull();
  });

  it("returns null for an empty work token (no project id)", () => {
    expect(parseWorkTab("work:")).toBeNull();
    expect(parseWorkTab("work:   ")).toBeNull();
  });

  it("returns null for null/undefined/empty", () => {
    expect(parseWorkTab(null)).toBeNull();
    expect(parseWorkTab(undefined)).toBeNull();
    expect(parseWorkTab("")).toBeNull();
  });

  it("keeps a colon in the id (permissive parse; caller no-ops on unknown id)", () => {
    // Only the leading `work:` prefix is stripped, so the rest — colons and all —
    // is the id verbatim. The caller's load simply finds no such project.
    expect(parseWorkTab("work:foo:bar")).toEqual({
      selectedId: "foo:bar",
      workMode: true,
    });
  });
});

describe("attentionBellTarget", () => {
  it("targets Work mode for a known project id", () => {
    expect(attentionBellTarget("abc")).toEqual({
      view: "projects",
      tab: "work:abc",
    });
  });

  it("falls back to the Projects Board when no project id is known", () => {
    expect(attentionBellTarget(null)).toEqual({ view: "projects", tab: null });
    expect(attentionBellTarget(undefined)).toEqual({
      view: "projects",
      tab: null,
    });
    expect(attentionBellTarget("   ")).toEqual({ view: "projects", tab: null });
  });

  it("round-trips through parseViewTarget + parseWorkTab into a selection", () => {
    const built = attentionBellTarget("p1");
    const parsed = parseViewTarget(`${built.view}#${built.tab}`);
    expect(parsed).toEqual({ view: "projects", tab: "work:p1" });
    expect(parseWorkTab(parsed.tab)).toEqual({
      selectedId: "p1",
      workMode: true,
    });
  });
});

describe("shouldExitWorkMode (Phase G BLOCKER: deep-link must survive the load)", () => {
  it("never exits when work mode is off", () => {
    expect(
      shouldExitWorkMode({
        workMode: false,
        hasCurrentProject: false,
        selectedId: "p1",
        loadingProjectId: null,
      }),
    ).toBe(false);
  });

  it("never exits when the project is resolved", () => {
    expect(
      shouldExitWorkMode({
        workMode: true,
        hasCurrentProject: true,
        selectedId: "p1",
        loadingProjectId: null,
      }),
    ).toBe(false);
  });

  it("HOLDS work mode on the first render after a bell deep-link (sync bridge, no loadingProjectId yet)", () => {
    // enterWorkMode("p1") just ran: selectedId set, pendingWorkEntryId set, but the
    // detail-load effect has NOT yet set loadingProjectId (lands a render later).
    expect(
      shouldExitWorkMode({
        workMode: true,
        hasCurrentProject: false,
        selectedId: "p1",
        loadingProjectId: null,
        pendingWorkEntryId: "p1",
      }),
    ).toBe(false);
  });

  it("HOLDS work mode while the selected project is loading", () => {
    expect(
      shouldExitWorkMode({
        workMode: true,
        hasCurrentProject: false,
        selectedId: "p1",
        loadingProjectId: "p1",
        pendingWorkEntryId: null,
      }),
    ).toBe(false);
  });

  it("a project that loads AFTER entry stays in work mode through the whole sequence", () => {
    // R1: sync bridge holds (no loadingProjectId yet).
    expect(
      shouldExitWorkMode({
        workMode: true,
        hasCurrentProject: false,
        selectedId: "p1",
        loadingProjectId: null,
        pendingWorkEntryId: "p1",
      }),
    ).toBe(false);
    // R2: loadingProjectId caught up, bridge cleared — still loading, still holds.
    expect(
      shouldExitWorkMode({
        workMode: true,
        hasCurrentProject: false,
        selectedId: "p1",
        loadingProjectId: "p1",
        pendingWorkEntryId: null,
      }),
    ).toBe(false);
    // R3: detail landed — resolved, no exit.
    expect(
      shouldExitWorkMode({
        workMode: true,
        hasCurrentProject: true,
        selectedId: "p1",
        loadingProjectId: null,
        pendingWorkEntryId: null,
      }),
    ).toBe(false);
  });

  it("a truly missing/archived deep-link falls back to Board after the load resolves empty", () => {
    // Load settled (loadingProjectId cleared) but no detail ever arrived and the
    // sync bridge has been cleared — genuine missing id, so exit to the Board.
    expect(
      shouldExitWorkMode({
        workMode: true,
        hasCurrentProject: false,
        selectedId: "ghost",
        loadingProjectId: null,
        pendingWorkEntryId: null,
      }),
    ).toBe(true);
  });

  it("exits when work mode is on but the selection was cleared", () => {
    expect(
      shouldExitWorkMode({
        workMode: true,
        hasCurrentProject: false,
        selectedId: null,
        loadingProjectId: null,
        pendingWorkEntryId: null,
      }),
    ).toBe(true);
  });

  it("a stale load for a DIFFERENT id does not keep work mode for the current selection", () => {
    expect(
      shouldExitWorkMode({
        workMode: true,
        hasCurrentProject: false,
        selectedId: "ghost",
        loadingProjectId: "other",
        pendingWorkEntryId: "other",
      }),
    ).toBe(true);
  });
});

describe("shouldClearWorkEntryBridge", () => {
  it("never clears when nothing is pending", () => {
    expect(
      shouldClearWorkEntryBridge({
        pendingWorkEntryId: null,
        currentProjectId: "a",
        loadingProjectId: "a",
        currentSelectedId: "a",
      }),
    ).toBe(false);
  });

  it("clears once the bridge TARGET has resolved", () => {
    expect(
      shouldClearWorkEntryBridge({
        pendingWorkEntryId: "b",
        currentProjectId: "b",
        loadingProjectId: null,
        currentSelectedId: "b",
      }),
    ).toBe(true);
  });

  it("clears once the bridge target's load is in flight (loadingProjectId caught up)", () => {
    expect(
      shouldClearWorkEntryBridge({
        pendingWorkEntryId: "b",
        currentProjectId: null,
        loadingProjectId: "b",
        currentSelectedId: "b",
      }),
    ).toBe(true);
  });

  it("clears when the selection genuinely moved off the bridge (compared to the LIVE ref)", () => {
    expect(
      shouldClearWorkEntryBridge({
        pendingWorkEntryId: "b",
        currentProjectId: null,
        loadingProjectId: null,
        currentSelectedId: "c", // user moved on to c
      }),
    ).toBe(true);
  });

  it("does NOT clear when the previously-selected project A is still resolved during the A→B render", () => {
    // The bug: enterWorkMode(B) ran, selectedIdRef.current === "b", but the stale
    // `currentProject` is still A (id "a") and selectedId STATE is still "a". A bare
    // hasCurrentProject / stale-state comparison would clear here a tick too early.
    expect(
      shouldClearWorkEntryBridge({
        pendingWorkEntryId: "b",
        currentProjectId: "a", // project A still resolved/showing
        loadingProjectId: null, // B's load not started yet
        currentSelectedId: "b", // live ref already points at B
      }),
    ).toBe(false);
  });
});

describe("bell deep-link A→B end-to-end (clear bridge + work-mode coherence)", () => {
  // Simulate the render sequence ProjectsView goes through when a bell deep-link
  // switches from project A (currently selected + resolved + in work mode) to a
  // DIFFERENT project B. The bridge must NOT clear early and work mode must NOT
  // bounce to the Board at any render until B genuinely resolves.
  function step(s: {
    currentProjectId: string | null;
    loadingProjectId: string | null;
    selectedIdState: string | null;
    selectedIdRef: string | null;
    bridge: string | null;
  }): { bridge: string | null; exits: boolean } {
    const cleared = shouldClearWorkEntryBridge({
      pendingWorkEntryId: s.bridge,
      currentProjectId: s.currentProjectId,
      loadingProjectId: s.loadingProjectId,
      currentSelectedId: s.selectedIdRef,
    });
    const bridge = cleared ? null : s.bridge;
    const exits = shouldExitWorkMode({
      workMode: true,
      hasCurrentProject: s.currentProjectId !== null,
      selectedId: s.selectedIdState,
      loadingProjectId: s.loadingProjectId,
      pendingWorkEntryId: bridge,
    });
    return { bridge, exits };
  }

  it("a bell deep-link from A to B STAYS in work mode through the whole load", () => {
    // R0: enterWorkMode("b") just ran. selectedId STATE still "a" (stale), ref "b",
    // bridge "b", project A still resolved.
    let r = step({
      currentProjectId: "a",
      loadingProjectId: null,
      selectedIdState: "a",
      selectedIdRef: "b",
      bridge: "b",
    });
    expect(r.bridge).toBe("b"); // not cleared by stale A
    expect(r.exits).toBe(false);

    // R1: selectedId state now "b"; currentProject memo nulls (project A id !== b);
    // B's load not started yet.
    r = step({
      currentProjectId: null,
      loadingProjectId: null,
      selectedIdState: "b",
      selectedIdRef: "b",
      bridge: r.bridge,
    });
    expect(r.bridge).toBe("b"); // sync bridge still holds
    expect(r.exits).toBe(false);

    // R2: B's load is in flight (loadingProjectId === "b").
    r = step({
      currentProjectId: null,
      loadingProjectId: "b",
      selectedIdState: "b",
      selectedIdRef: "b",
      bridge: r.bridge,
    });
    expect(r.bridge).toBeNull(); // bridge cleared now that real load caught up
    expect(r.exits).toBe(false); // held by loadingProjectId === selectedId

    // R3: B's detail landed.
    r = step({
      currentProjectId: "b",
      loadingProjectId: null,
      selectedIdState: "b",
      selectedIdRef: "b",
      bridge: r.bridge,
    });
    expect(r.exits).toBe(false); // resolved, stays in work mode
  });

  it("a truly-missing deep-link target still falls back to the Board", () => {
    // R0: enterWorkMode("ghost"); A still resolved/stale.
    let r = step({
      currentProjectId: "a",
      loadingProjectId: null,
      selectedIdState: "a",
      selectedIdRef: "ghost",
      bridge: "ghost",
    });
    expect(r.exits).toBe(false);

    // R1: selection state catches up; load not started.
    r = step({
      currentProjectId: null,
      loadingProjectId: null,
      selectedIdState: "ghost",
      selectedIdRef: "ghost",
      bridge: r.bridge,
    });
    expect(r.exits).toBe(false);

    // R2: load in flight.
    r = step({
      currentProjectId: null,
      loadingProjectId: "ghost",
      selectedIdState: "ghost",
      selectedIdRef: "ghost",
      bridge: r.bridge,
    });
    expect(r.bridge).toBeNull();
    expect(r.exits).toBe(false);

    // R3: load settled EMPTY (no detail) — genuine missing id → exit to Board.
    r = step({
      currentProjectId: null,
      loadingProjectId: null,
      selectedIdState: "ghost",
      selectedIdRef: "ghost",
      bridge: r.bridge,
    });
    expect(r.exits).toBe(true);
  });
});

describe("formatViewTarget", () => {
  it("formats a view with no tab as the bare view", () => {
    expect(formatViewTarget("projects")).toBe("projects");
    expect(formatViewTarget("projects", null)).toBe("projects");
    expect(formatViewTarget("projects", "")).toBe("projects");
  });

  it("formats a view#tab target", () => {
    expect(formatViewTarget("settings", "secrets")).toBe("settings#secrets");
  });

  it("round-trips with parseViewTarget", () => {
    const target = formatViewTarget("settings", "secrets");
    expect(parseViewTarget(target)).toEqual({
      view: "settings",
      tab: "secrets",
    });
  });
});
