import { describe, expect, it, vi } from "vitest";
import type {
  AgentSession,
  AgentSubagent,
  GitPushRequest,
  ProjectGitStatus,
} from "../../types/backend";
import { isRecentProjectSession } from "../../utils/agentClaims";
import {
  DEFAULT_DOCK_TAB,
  DOCK_TABS,
  commitProjectCall,
  compactWriteCall,
  defaultSelectedAgentId,
  enterWorkMode,
  exitWorkMode,
  isMiniSession,
  miniKillCall,
  projectsViewMode,
  pushProjectCall,
  pullProjectCall,
  cloneProjectCall,
  isLikelyGithubRepoUrl,
  railRows,
  reconcileSelectedAgentId,
  runGitActionGuarded,
  shouldShowCompact,
  subagentRailLabel,
  workspaceGitLine,
  isPendingPushRequest,
  pendingPushRequestsForProject,
  pushRequestSummary,
} from "./projectWorkspaceModel";

// ---- fixtures ---------------------------------------------------------------

function git(partial: Partial<ProjectGitStatus>): ProjectGitStatus {
  return {
    rootPath: null,
    repoRoot: null,
    repoName: null,
    branch: null,
    upstream: null,
    origin: null,
    githubUrl: null,
    cloneCommand: null,
    pullRequestUrl: null,
    commit: null,
    dirtyCount: 0,
    stagedCount: 0,
    unstagedCount: 0,
    untrackedCount: 0,
    aheadCount: 0,
    behindCount: 0,
    isGitRepo: false,
    isGithub: false,
    policyStatus: "ready",
    warnings: [],
    requiredActions: [],
    suggestedRepos: [],
    ...partial,
  };
}

function session(partial: Partial<AgentSession>): AgentSession {
  return {
    agentId: "coder-1",
    role: "coder",
    model: null,
    status: "online",
    message: null,
    currentProjectId: "p1",
    currentTaskId: null,
    firstSeenAt: "2026-06-05T00:00:00Z",
    lastSeenAt: "2026-06-05T00:00:00Z",
    ...partial,
  };
}

// ---- work-mode routing ------------------------------------------------------

describe("work-mode routing", () => {
  it("enterWorkMode selects the project and flips workMode on", () => {
    const next = enterWorkMode({ selectedId: null, workMode: false }, "p9");
    expect(next).toEqual({ selectedId: "p9", workMode: true });
  });

  it("enterWorkMode on a different card switches the selection", () => {
    const next = enterWorkMode({ selectedId: "p1", workMode: true }, "p2");
    expect(next).toEqual({ selectedId: "p2", workMode: true });
  });

  it("exitWorkMode clears workMode but KEEPS the selection", () => {
    const next = exitWorkMode({ selectedId: "p3", workMode: true });
    expect(next).toEqual({ selectedId: "p3", workMode: false });
  });

  it("projectsViewMode requires both the flag and a loaded project", () => {
    expect(projectsViewMode(true, true)).toBe("work");
    expect(projectsViewMode(true, false)).toBe("board"); // flag set, no project
    expect(projectsViewMode(false, true)).toBe("board");
    expect(projectsViewMode(false, false)).toBe("board");
  });
});

// ---- top-bar git line -------------------------------------------------------

describe("workspaceGitLine", () => {
  it("derives committed?/pushed? from dirtyCount/aheadCount", () => {
    const clean = workspaceGitLine(
      git({ isGitRepo: true, branch: "main", dirtyCount: 0, aheadCount: 0 }),
    );
    expect(clean.committed).toBe(true);
    expect(clean.pushed).toBe(true);
    expect(clean.segments).toContain("committed?: yes");
    expect(clean.segments).toContain("pushed?: yes");
    // A clean tree shows NO "modified" segment (committed?: yes already says so).
    expect(clean.segments.some((s) => s.includes("modified"))).toBe(false);
    expect(clean.segments).not.toContain("0 modified");

    const dirty = workspaceGitLine(
      git({ isGitRepo: true, branch: "feat", dirtyCount: 2, aheadCount: 1 }),
    );
    expect(dirty.committed).toBe(false);
    expect(dirty.pushed).toBe(false);
    expect(dirty.segments).toContain("2 modified");
    expect(dirty.segments).toContain("↑1");
    expect(dirty.segments).toContain("committed?: no");
    expect(dirty.segments).toContain("pushed?: no");
  });

  it("shows behind segment only when behind > 0", () => {
    const behind = workspaceGitLine(
      git({ isGitRepo: true, branch: "main", behindCount: 3 }),
    );
    expect(behind.segments).toContain("↓3");
    const ahead = workspaceGitLine(git({ isGitRepo: true, branch: "main" }));
    expect(ahead.segments.some((s) => s.startsWith("↓"))).toBe(false);
  });

  it("is null-safe and never renders a raw value", () => {
    const none = workspaceGitLine(null);
    expect(none.isGitRepo).toBe(false);
    expect(none.branch).toBe("—");
    expect(none.committed).toBe(true); // 0 dirty
    // No commit hash / upstream leaked into the segments.
    expect(none.segments.join(" ")).not.toMatch(/[0-9a-f]{7,}/);
  });

  it("floors/sanitizes negative or NaN counts", () => {
    const line = workspaceGitLine(
      git({
        isGitRepo: true,
        branch: "main",
        dirtyCount: -5,
        aheadCount: Number.NaN,
      }),
    );
    expect(line.dirtyCount).toBe(0);
    expect(line.aheadCount).toBe(0);
    expect(line.committed).toBe(true);
  });
});

// ---- rail model -------------------------------------------------------------

describe("subagentRailLabel", () => {
  it("formats label · model · ×count", () => {
    const sub: AgentSubagent = { label: "search", model: "haiku", count: 3 };
    expect(subagentRailLabel(sub)).toBe("search · haiku · ×3");
  });

  it("omits empty model and non-positive count", () => {
    expect(subagentRailLabel({ label: "x", model: "", count: 0 })).toBe("x");
  });
});

describe("railRows", () => {
  it("marks the selected agent and carries subagents", () => {
    const sub: AgentSubagent = { label: "s", model: "m", count: 1 };
    const rows = railRows(
      [
        session({ agentId: "coder-1", subagents: [sub] }),
        session({ agentId: "verifier-1", role: "verifier" }),
      ],
      "verifier-1",
    );
    expect(rows[0].selected).toBe(false);
    expect(rows[0].subagents).toEqual([sub]);
    expect(rows[1].selected).toBe(true);
    expect(rows[1].role).toBe("verifier");
  });

  it("derives the orchestrator badge from subagents (legacy folds to coder)", () => {
    const rows = railRows(
      [
        session({
          agentId: "a",
          role: "orchestrator",
          subagents: [{ label: "s", model: "m", count: 1 }],
        }),
      ],
      null,
    );
    expect(rows[0].role).toBe("coder");
    expect(rows[0].orchestratorBadge).toBe(true);
  });
});

// ---- mini-coder identity + nesting -----------------------------------------

describe("isMiniSession", () => {
  it("is true when parentAgentId is a non-empty string", () => {
    expect(isMiniSession(session({ parentAgentId: "coder-1" }))).toBe(true);
  });

  it("is false with no parentAgentId, null, or whitespace-only", () => {
    expect(isMiniSession(session({}))).toBe(false);
    expect(isMiniSession(session({ parentAgentId: null }))).toBe(false);
    expect(isMiniSession(session({ parentAgentId: "" }))).toBe(false);
    expect(isMiniSession(session({ parentAgentId: "   " }))).toBe(false);
  });
});

describe("railRows nesting", () => {
  it("nests a mini under the session whose agentId === its parentAgentId", () => {
    const rows = railRows(
      [
        session({ agentId: "coder-1" }),
        session({ agentId: "mini-1", parentAgentId: "coder-1" }),
      ],
      null,
    );
    // Only the parent is a top-level row; the mini is nested under it.
    expect(rows).toHaveLength(1);
    expect(rows[0].agentId).toBe("coder-1");
    expect(rows[0].isMini).toBe(false);
    expect(rows[0].miniChildren).toHaveLength(1);
    expect(rows[0].miniChildren[0].agentId).toBe("mini-1");
    expect(rows[0].miniChildren[0].isMini).toBe(true);
    expect(rows[0].miniChildren[0].orphanedMini).toBe(false);
  });

  it("surfaces a mini whose parent is ABSENT at top level with the orphan flag", () => {
    const rows = railRows(
      [session({ agentId: "mini-1", parentAgentId: "gone-coder" })],
      null,
    );
    expect(rows).toHaveLength(1);
    expect(rows[0].agentId).toBe("mini-1");
    expect(rows[0].isMini).toBe(true);
    expect(rows[0].orphanedMini).toBe(true);
    expect(rows[0].miniChildren).toHaveLength(0);
  });

  it("keeps label-only subagents as info on the parent, separate from miniChildren", () => {
    const sub: AgentSubagent = { label: "search", model: "haiku", count: 2 };
    const rows = railRows(
      [
        session({ agentId: "coder-1", subagents: [sub] }),
        session({ agentId: "mini-1", parentAgentId: "coder-1" }),
      ],
      null,
    );
    expect(rows[0].subagents).toEqual([sub]);
    expect(rows[0].miniChildren.map((c) => c.agentId)).toEqual(["mini-1"]);
  });

  it("marks the selected mini child as selected, parent not", () => {
    const rows = railRows(
      [
        session({ agentId: "coder-1" }),
        session({ agentId: "mini-1", parentAgentId: "coder-1" }),
      ],
      "mini-1",
    );
    expect(rows[0].selected).toBe(false);
    expect(rows[0].miniChildren[0].selected).toBe(true);
  });

  it("never drops a mini whose parent is itself a mini (surfaces it top-level as orphan)", () => {
    const rows = railRows(
      [
        session({ agentId: "coder-1" }),
        session({ agentId: "mini-1", parentAgentId: "coder-1" }),
        // A grandchild mini: parent is a mini, not a real coder. Don't nest 2 deep;
        // don't drop it — surface it at top level as an orphan.
        session({ agentId: "mini-2", parentAgentId: "mini-1" }),
      ],
      null,
    );
    const topIds = rows.map((r) => r.agentId);
    expect(topIds).toContain("coder-1");
    expect(topIds).toContain("mini-2");
    // WARNING 7: mini-1 is NESTED only — it must NOT also appear top-level (no dup).
    expect(topIds).not.toContain("mini-1");
    // mini-1 nested under coder-1; mini-2 surfaced top-level as orphan.
    expect(rows.find((r) => r.agentId === "coder-1")?.miniChildren.map((c) => c.agentId)).toEqual(["mini-1"]);
    const grandchild = rows.find((r) => r.agentId === "mini-2");
    expect(grandchild?.isMini).toBe(true);
    expect(grandchild?.orphanedMini).toBe(true);
    expect(grandchild?.miniChildren).toHaveLength(0);
  });

  it("WARNING 7: two minis under one parent both nest; neither is top-level", () => {
    const rows = railRows(
      [
        session({ agentId: "coder-1" }),
        session({ agentId: "mini-a", parentAgentId: "coder-1" }),
        session({ agentId: "mini-b", parentAgentId: "coder-1" }),
      ],
      null,
    );
    const topIds = rows.map((r) => r.agentId);
    // Only the parent is top-level.
    expect(topIds).toEqual(["coder-1"]);
    expect(topIds).not.toContain("mini-a");
    expect(topIds).not.toContain("mini-b");
    // Both minis nest under the parent, in input order.
    expect(rows[0].miniChildren.map((c) => c.agentId)).toEqual([
      "mini-a",
      "mini-b",
    ]);
    expect(rows[0].miniChildren.every((c) => c.isMini && !c.orphanedMini)).toBe(
      true,
    );
  });

  it("WARNING 3: a finished (status='done') mini is excluded from the project rail", () => {
    // The executor closes a finished mini's SESSION (status -> 'done') so that
    // ProjectsView.sessionsByProject (filtered by isRecentProjectSession) drops it
    // before railRows ever sees it — the stale mini row disappears promptly instead
    // of lingering ~15min. We reproduce that filter here, then build the rail.
    const now = Date.parse("2026-06-05T00:01:00Z");
    const recent = "2026-06-05T00:00:30Z"; // well within the 15min window
    const all = [
      session({ agentId: "coder-1", lastSeenAt: recent }),
      // The finished mini: still has a parent + a recent heartbeat, but status 'done'.
      session({
        agentId: "mini-1",
        parentAgentId: "coder-1",
        status: "done",
        lastSeenAt: recent,
      }),
    ];
    // isRecentProjectSession excludes 'done' regardless of how recent the heartbeat.
    expect(isRecentProjectSession(all[1], now)).toBe(false);
    const railInput = all.filter((s) => isRecentProjectSession(s, now));
    const rows = railRows(railInput, null);
    // Only the coder survives; the done mini is gone from both top-level and children.
    expect(rows.map((r) => r.agentId)).toEqual(["coder-1"]);
    expect(rows[0].miniChildren).toHaveLength(0);
  });

  it("preserves order and produces no duplicate/missing rows", () => {
    const rows = railRows(
      [
        session({ agentId: "coder-1" }),
        session({ agentId: "mini-1a", parentAgentId: "coder-1" }),
        session({ agentId: "verifier-1", role: "verifier" }),
        session({ agentId: "mini-1b", parentAgentId: "coder-1" }),
      ],
      null,
    );
    expect(rows.map((r) => r.agentId)).toEqual(["coder-1", "verifier-1"]);
    // Both minis nested under coder-1, in input order; verifier has none.
    expect(rows[0].miniChildren.map((c) => c.agentId)).toEqual([
      "mini-1a",
      "mini-1b",
    ]);
    expect(rows[1].miniChildren).toHaveLength(0);
    // Every input session appears exactly once across the tree.
    const flat = rows.flatMap((r) => [r.agentId, ...r.miniChildren.map((c) => c.agentId)]);
    expect(flat.sort()).toEqual(["coder-1", "mini-1a", "mini-1b", "verifier-1"]);
  });
});

// ---- selection default + pruning -------------------------------------------

describe("selection lifecycle", () => {
  it("defaults to the freshest session by heartbeat", () => {
    const id = defaultSelectedAgentId([
      session({ agentId: "old", lastSeenAt: "2026-06-05T00:00:00Z" }),
      session({ agentId: "fresh", lastSeenAt: "2026-06-05T01:00:00Z" }),
    ]);
    expect(id).toBe("fresh");
  });

  it("default is null when there are no sessions", () => {
    expect(defaultSelectedAgentId([])).toBeNull();
  });

  it("keeps a still-live selection unchanged (same reference returned)", () => {
    const sessions = [session({ agentId: "coder-1" })];
    expect(reconcileSelectedAgentId("coder-1", sessions)).toBe("coder-1");
  });

  it("prunes a dangling selection to the freshest survivor when the agent exits", () => {
    const sessions = [
      session({ agentId: "survivor", lastSeenAt: "2026-06-05T02:00:00Z" }),
    ];
    expect(reconcileSelectedAgentId("gone", sessions)).toBe("survivor");
  });

  it("prunes to null when the only agent exits", () => {
    expect(reconcileSelectedAgentId("gone", [])).toBeNull();
  });

  it("a reaped mini falls back to its PARENT when the parent survives", () => {
    const prev = [
      session({ agentId: "coder-1" }),
      session({ agentId: "mini-1", parentAgentId: "coder-1" }),
      session({ agentId: "other", lastSeenAt: "2026-06-05T09:00:00Z" }),
    ];
    // The mini is gone now; the parent (and an unrelated, fresher agent) remain.
    const sessions = [
      session({ agentId: "coder-1" }),
      session({ agentId: "other", lastSeenAt: "2026-06-05T09:00:00Z" }),
    ];
    expect(reconcileSelectedAgentId("mini-1", sessions, prev)).toBe("coder-1");
  });

  it("a reaped mini whose parent is ALSO gone falls back to the freshest survivor", () => {
    const prev = [
      session({ agentId: "coder-1" }),
      session({ agentId: "mini-1", parentAgentId: "coder-1" }),
      session({ agentId: "fresh", lastSeenAt: "2026-06-05T09:00:00Z" }),
    ];
    // Both the mini AND its parent are gone; only an unrelated agent remains.
    const sessions = [
      session({ agentId: "fresh", lastSeenAt: "2026-06-05T09:00:00Z" }),
    ];
    expect(reconcileSelectedAgentId("mini-1", sessions, prev)).toBe("fresh");
  });

  it("BLOCKER 3: with NO previousSessions, skips the parent branch (plain freshest)", () => {
    // The selection is gone. Without a prior snapshot the function must NOT consult
    // `sessions` as a stand-in for `previousSessions` (the old default-to-sessions
    // bug): it simply falls back to the freshest survivor. Here a present session even
    // shares the gone id's would-be parent shape, but with no prior snapshot the
    // mini-parent fallback is never attempted.
    const sessions = [
      session({ agentId: "coder-1" }),
      session({ agentId: "fresh", lastSeenAt: "2026-06-05T09:00:00Z" }),
    ];
    // previousSessions omitted -> freshest survivor, not coder-1.
    expect(reconcileSelectedAgentId("mini-1", sessions)).toBe("fresh");
  });

  it("a still-present selected normal agent is unaffected by the parent logic", () => {
    const prev = [
      session({ agentId: "coder-1" }),
      session({ agentId: "mini-1", parentAgentId: "coder-1" }),
    ];
    const sessions = [
      session({ agentId: "coder-1" }),
      session({ agentId: "mini-1", parentAgentId: "coder-1" }),
    ];
    expect(reconcileSelectedAgentId("coder-1", sessions, prev)).toBe("coder-1");
  });
});

// ---- commit / push IPC contract --------------------------------------------

describe("commit/push IPC builders", () => {
  it("commit targets project_git_commit with camelCase projectId + message", () => {
    const call = commitProjectCall("p1", "fix the bug");
    expect(call.command).toBe("project_git_commit");
    expect(call.args).toEqual({ projectId: "p1", message: "fix the bug" });
  });

  it("push targets project_git_push with camelCase projectId, no force flag", () => {
    const call = pushProjectCall("p1");
    expect(call.command).toBe("project_git_push");
    expect(call.args).toEqual({ projectId: "p1" });
    // The UI never threads a force flag; the backend additionally refuses one.
    expect(JSON.stringify(call.args)).not.toMatch(/force|-f/);
  });

  it("a mocked invoke receives exactly the built call", async () => {
    const invoke = vi.fn().mockResolvedValue({});
    const commit = commitProjectCall("proj", "msg");
    await invoke(commit.command, commit.args);
    const push = pushProjectCall("proj");
    await invoke(push.command, push.args);
    expect(invoke).toHaveBeenNthCalledWith(1, "project_git_commit", {
      projectId: "proj",
      message: "msg",
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "project_git_push", {
      projectId: "proj",
    });
  });
});

// ---- GH-P3: pull IPC builder + surfaces errors -----------------------------

describe("pullProjectCall (GH-P3)", () => {
  it("targets project_git_pull with camelCase projectId", () => {
    const call = pullProjectCall("p1");
    expect(call.command).toBe("project_git_pull");
    expect(call.args).toEqual({ projectId: "p1" });
  });

  it("a mocked invoke that rejects surfaces the backend git error", async () => {
    const invoke = vi
      .fn()
      .mockRejectedValue(new Error("Not possible to fast-forward, aborting."));
    const call = pullProjectCall("proj");
    await expect(invoke(call.command, call.args)).rejects.toThrow(
      /fast-forward/,
    );
    expect(invoke).toHaveBeenCalledWith("project_git_pull", {
      projectId: "proj",
    });
  });
});

// ---- GH-P3: clone dialog URL validation + IPC builder ----------------------

describe("isLikelyGithubRepoUrl (clone dialog gate)", () => {
  it("accepts the common GitHub remote shapes", () => {
    expect(
      isLikelyGithubRepoUrl("https://github.com/Saurias92/Aspis-bio"),
    ).toBe(true);
    expect(
      isLikelyGithubRepoUrl("https://github.com/Saurias92/Aspis-bio.git"),
    ).toBe(true);
    expect(
      isLikelyGithubRepoUrl("git@github.com:Saurias92/Aspis-bio.git"),
    ).toBe(true);
    expect(
      isLikelyGithubRepoUrl("ssh://git@github.com/Saurias92/Aspis-bio.git"),
    ).toBe(true);
    expect(
      isLikelyGithubRepoUrl("http://github.com/Saurias92/Aspis-bio"),
    ).toBe(true);
  });

  it("rejects empty, non-github, and incomplete URLs", () => {
    expect(isLikelyGithubRepoUrl("")).toBe(false);
    expect(isLikelyGithubRepoUrl("   ")).toBe(false);
    expect(isLikelyGithubRepoUrl("not a url")).toBe(false);
    expect(isLikelyGithubRepoUrl("https://evil.example/o/r")).toBe(false);
    expect(isLikelyGithubRepoUrl("https://github.com/owner")).toBe(false);
    expect(isLikelyGithubRepoUrl("https://gitlab.com/o/r")).toBe(false);
  });
});

describe("cloneProjectCall (clone dialog)", () => {
  it("targets project_git_clone with the trimmed url and no token", () => {
    const call = cloneProjectCall("  https://github.com/o/r  ");
    expect(call.command).toBe("project_git_clone");
    expect(call.args).toEqual({ url: "https://github.com/o/r" });
    // The token must NEVER be threaded by the UI; only url (and optional dest).
    expect(JSON.stringify(call.args)).not.toMatch(/ghp_|gho_|token/i);
  });

  it("includes destParent only when a non-empty parent is given", () => {
    expect(cloneProjectCall("https://github.com/o/r", "  ").args).toEqual({
      url: "https://github.com/o/r",
    });
    expect(
      cloneProjectCall("https://github.com/o/r", "C:/dev").args,
    ).toEqual({ url: "https://github.com/o/r", destParent: "C:/dev" });
  });

  it("a mocked invoke receives exactly the built clone call", async () => {
    const invoke = vi.fn().mockResolvedValue({ metadata: { id: "r" } });
    const call = cloneProjectCall("https://github.com/o/r");
    await invoke(call.command, call.args);
    expect(invoke).toHaveBeenCalledWith("project_git_clone", {
      url: "https://github.com/o/r",
    });
  });
});

// ---- MC-P5: mini Stop (kill) IPC builder + gating ---------------------------

describe("miniKillCall (Stop safety brake)", () => {
  it("targets mini_coder_kill with the mini's camelCase agentId", () => {
    const call = miniKillCall(
      session({ agentId: "mini-1", parentAgentId: "coder-1" }),
    );
    expect(call).not.toBeNull();
    expect(call?.command).toBe("mini_coder_kill");
    expect(call?.args).toEqual({ agentId: "mini-1" });
  });

  it("returns null for a non-mini agent (gating — no 1-click kill)", () => {
    // No parentAgentId -> not a mini -> no kill call (caller renders no Stop).
    expect(miniKillCall(session({ agentId: "coder-1" }))).toBeNull();
    expect(miniKillCall(session({ agentId: "x", parentAgentId: null }))).toBeNull();
    expect(miniKillCall(session({ agentId: "x", parentAgentId: "" }))).toBeNull();
  });

  it("a mocked invoke receives exactly the built kill call", async () => {
    const invoke = vi.fn().mockResolvedValue(undefined);
    const call = miniKillCall(
      session({ agentId: "mini-7", parentAgentId: "coder-2" }),
    );
    if (call) await invoke(call.command, call.args);
    expect(invoke).toHaveBeenCalledWith("mini_coder_kill", { agentId: "mini-7" });
  });
});

// ---- MC-P7: Compact button gating + write call ------------------------------

describe("shouldShowCompact (claude-only gating)", () => {
  it("is true ONLY for the resolved built-in client exactly 'claude'", () => {
    expect(shouldShowCompact(session({ client: "claude" }))).toBe(true);
    // Case/whitespace-insensitive on the resolved client.
    expect(shouldShowCompact(session({ client: "Claude" }))).toBe(true);
    expect(shouldShowCompact(session({ client: " claude " }))).toBe(true);
  });

  it("is false for codex / powershell / ollama / custom / empty clients", () => {
    expect(shouldShowCompact(session({ client: "codex" }))).toBe(false);
    expect(shouldShowCompact(session({ client: "powershell" }))).toBe(false);
    expect(shouldShowCompact(session({ client: "ollama" }))).toBe(false);
    expect(shouldShowCompact(session({ client: "my-custom-cli" }))).toBe(false);
    expect(shouldShowCompact(session({ client: "" }))).toBe(false);
    expect(shouldShowCompact(session({ client: null }))).toBe(false);
    expect(shouldShowCompact(session({ client: undefined }))).toBe(false);
  });

  it("is NOT a substring match — 'claudex' is false", () => {
    // A custom client id that merely CONTAINS "claude" must not trip the gate.
    expect(shouldShowCompact(session({ client: "claudex" }))).toBe(false);
    expect(shouldShowCompact(session({ client: "claude-x" }))).toBe(false);
    expect(shouldShowCompact(session({ client: "myclaude" }))).toBe(false);
  });
});

describe("compactWriteCall (run /compact via agent_pty_write)", () => {
  it("writes /compact\\n via agent_pty_write for a claude session", () => {
    const call = compactWriteCall(
      session({ agentId: "coder-1", client: "claude" }),
    );
    expect(call).not.toBeNull();
    expect(call?.command).toBe("agent_pty_write");
    expect(call?.args).toEqual({ agentId: "coder-1", data: "/compact\n" });
  });

  it("returns null for a non-claude session (gating — no Compact)", () => {
    expect(compactWriteCall(session({ client: "codex" }))).toBeNull();
    expect(compactWriteCall(session({ client: "claudex" }))).toBeNull();
    expect(compactWriteCall(session({ client: null }))).toBeNull();
  });

  it("a mocked invoke receives exactly the built compact write call", async () => {
    const invoke = vi.fn().mockResolvedValue(undefined);
    const call = compactWriteCall(
      session({ agentId: "coder-9", client: "claude" }),
    );
    if (call) await invoke(call.command, call.args);
    expect(invoke).toHaveBeenCalledWith("agent_pty_write", {
      agentId: "coder-9",
      data: "/compact\n",
    });
  });
});

// ---- git action reentrancy guard (BLOCKER 1) --------------------------------

describe("runGitActionGuarded", () => {
  it("runs the action and clears the flag when free", async () => {
    const flag = { current: false };
    const action = vi.fn().mockResolvedValue(undefined);
    const ran = await runGitActionGuarded(flag, action);
    expect(ran).toBe(true);
    expect(action).toHaveBeenCalledTimes(1);
    expect(flag.current).toBe(false);
  });

  it("a second invocation WHILE busy is a no-op (does not run the action)", async () => {
    const flag = { current: false };
    let resolveFirst!: () => void;
    const firstBody = new Promise<void>((r) => {
      resolveFirst = r;
    });
    const firstAction = vi.fn().mockReturnValue(firstBody);
    const secondAction = vi.fn().mockResolvedValue(undefined);

    // Start the first action but do NOT let it settle yet — the flag is held set.
    const firstCall = runGitActionGuarded(flag, firstAction);
    expect(flag.current).toBe(true);

    // A concurrent (e.g. double-click / Commit-then-Push) call must short-circuit.
    const secondRan = await runGitActionGuarded(flag, secondAction);
    expect(secondRan).toBe(false);
    expect(secondAction).not.toHaveBeenCalled();

    // Let the first finish; the flag clears and a later call works again.
    resolveFirst();
    expect(await firstCall).toBe(true);
    expect(firstAction).toHaveBeenCalledTimes(1);
    expect(flag.current).toBe(false);

    const thirdAction = vi.fn().mockResolvedValue(undefined);
    expect(await runGitActionGuarded(flag, thirdAction)).toBe(true);
    expect(thirdAction).toHaveBeenCalledTimes(1);
  });

  it("clears the flag even when the action throws", async () => {
    const flag = { current: false };
    await expect(
      runGitActionGuarded(flag, () => Promise.reject(new Error("boom"))),
    ).rejects.toThrow("boom");
    expect(flag.current).toBe(false);
  });
});

// ---- dock -------------------------------------------------------------------

describe("dock", () => {
  it("defaults to Censor", () => {
    expect(DEFAULT_DOCK_TAB).toBe("censor");
    expect(DOCK_TABS[0].id).toBe("censor");
  });

  it("exposes the five tabs in order (MCP tab added in Phase A.3)", () => {
    expect(DOCK_TABS.map((t) => t.id)).toEqual([
      "censor",
      "git",
      "plans",
      "console",
      "mcp",
    ]);
    expect(DOCK_TABS.find((t) => t.id === "console")!.label).toBe("Console");
    expect(DOCK_TABS.find((t) => t.id === "mcp")!.label).toBe("MCP");
  });
});

// ---- GH-P4: push-approval gate selection ------------------------------------

function pushReq(partial: Partial<GitPushRequest>): GitPushRequest {
  return {
    id: "r1",
    agentId: "codex",
    projectId: "p1",
    status: "pending_approval",
    createdAt: "2026-06-06T00:00:00Z",
    ...partial,
  };
}

describe("push-approval gate model", () => {
  it("isPendingPushRequest is true only for pending_approval", () => {
    expect(isPendingPushRequest(pushReq({ status: "pending_approval" }))).toBe(true);
    expect(isPendingPushRequest(pushReq({ status: "approved" }))).toBe(false);
    expect(isPendingPushRequest(pushReq({ status: "pushed" }))).toBe(false);
    expect(isPendingPushRequest(pushReq({ status: "denied" }))).toBe(false);
    expect(isPendingPushRequest(pushReq({ status: "timeout" }))).toBe(false);
  });

  it("filters to the project's pending requests, oldest first", () => {
    const requests: GitPushRequest[] = [
      pushReq({ id: "b", projectId: "p1", createdAt: "2026-06-06T00:00:02Z" }),
      pushReq({ id: "a", projectId: "p1", createdAt: "2026-06-06T00:00:01Z" }),
      pushReq({ id: "other", projectId: "p2", createdAt: "2026-06-06T00:00:00Z" }),
      pushReq({ id: "done", projectId: "p1", status: "pushed" }),
    ];
    const pending = pendingPushRequestsForProject(requests, "p1");
    expect(pending.map((r) => r.id)).toEqual(["a", "b"]);
  });

  it("returns empty for null/empty input or unknown project", () => {
    expect(pendingPushRequestsForProject(null, "p1")).toEqual([]);
    expect(pendingPushRequestsForProject([], "p1")).toEqual([]);
    expect(pendingPushRequestsForProject([pushReq({})], "")).toEqual([]);
  });

  it("summary renders branch/remote and a FORCE marker, never a URL", () => {
    expect(pushRequestSummary(pushReq({ branch: "main", remote: "origin" }))).toBe(
      "main → origin",
    );
    expect(
      pushRequestSummary(pushReq({ branch: "feat", remote: "upstream", force: true })),
    ).toBe("feat → upstream (FORCE)");
    // Missing branch/remote fall back to defaults.
    expect(pushRequestSummary(pushReq({ branch: undefined, remote: undefined }))).toBe(
      "current branch → origin",
    );
  });
});
