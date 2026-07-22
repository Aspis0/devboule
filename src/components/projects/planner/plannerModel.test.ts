import { describe, it, expect } from "vitest";
import {
  derivePlanCards,
  pageHostname,
  stripLabel,
  pickProjectDesign,
  chatMessagesWithMilestones,
  openQuestions,
  latestPlan,
  planCardsFromPiPlan,
  steerPickOption,
  steerYouDecide,
  doubtTouchesCard,
  stageViewOnDoubts,
  type PlanCard,
  type PiPlan,
} from "./plannerModel";
import type { ConsoleEntry, QuestionEntry } from "../../agents/agentConsoleModel";
import type { ProjectTask } from "../../../types/backend";
import type { DesignProjectEntry } from "../../../types/design";

function planEntry(time: string, payload: unknown): ConsoleEntry {
  // Matches the Rust `serde_json::to_string_pretty` contract: pretty-printed JSON.
  return { type: "chat", role: "plan", text: JSON.stringify(payload, null, 2), time };
}

function designEntry(over: Partial<DesignProjectEntry>): DesignProjectEntry {
  return {
    id: "d",
    name: "D",
    workingFolderPath: "/proj",
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    lastOpenedAt: "2026-01-01T00:00:00Z",
    ...over,
  };
}

function question(over: Partial<QuestionEntry> & { id: string }): QuestionEntry {
  return {
    type: "question",
    text: "How are sessions kept?",
    options: [
      { id: "server", label: "Server" },
      { id: "jwt", label: "JWT" },
    ],
    unrest: 0.8,
    candidates: [
      { label: "Server", pull: 0.6 },
      { label: "JWT", pull: 0.4 },
    ],
    lean: "Server",
    directionConfidence: 0.7,
    status: "open",
    affects: ["Session / token layer"],
    time: "00:01",
    ...over,
  };
}

describe("openQuestions (Kairion doubt extraction)", () => {
  it("keeps only question entries, in first-seen order", () => {
    const entries: ConsoleEntry[] = [
      { type: "coder", text: "reading repo", time: "1" },
      question({ id: "q1" }),
      { type: "chat", role: "user", text: "hi", time: "2" },
      question({ id: "q2", text: "Who owns identity?" }),
    ];
    const out = openQuestions(entries);
    expect(out.map((q) => q.id)).toEqual(["q1", "q2"]);
    expect(out[1].text).toBe("Who owns identity?");
  });

  it("upserts by id IN PLACE so a reopened event replaces the earlier one", () => {
    const entries: ConsoleEntry[] = [
      question({ id: "q1", status: "open", lean: "Server" }),
      question({ id: "q2", text: "Who owns identity?" }),
      // q1 comes back, reopened — the orchestrator changed its own mind.
      question({ id: "q1", status: "reopened", lean: null, unrest: 0.95 }),
    ];
    const out = openQuestions(entries);
    // still two cards, q1 keeps its original slot, carrying the latest (reopened) data.
    expect(out.map((q) => q.id)).toEqual(["q1", "q2"]);
    expect(out[0].status).toBe("reopened");
    expect(out[0].lean).toBeNull();
    expect(out[0].unrest).toBe(0.95);
  });

  it("returns [] for undefined / no question entries", () => {
    expect(openQuestions(undefined)).toEqual([]);
    expect(openQuestions([{ type: "coder", text: "x", time: "1" }])).toEqual([]);
  });
});

describe("steer moves (pick / you-decide ride the existing steer line)", () => {
  it("phrases a picked option as a plain steer line naming the label", () => {
    const q = question({ id: "q1" });
    expect(steerPickOption(q, { id: "jwt", label: "JWT" })).toBe(
      'For "How are sessions kept?" — go with JWT.',
    );
  });

  it("phrases you-decide with the lean as a hint when present", () => {
    expect(steerYouDecide(question({ id: "q1", lean: "Server" }))).toBe(
      'For "How are sessions kept?" — you decide (your lean — Server).',
    );
  });

  it("phrases you-decide without a lean when genuinely split", () => {
    expect(steerYouDecide(question({ id: "q1", lean: null }))).toBe(
      'For "How are sessions kept?" — you decide.',
    );
  });
});

describe("doubtTouchesCard (doubt <-> task link)", () => {
  const card: PlanCard = { n: 2, title: "Session / token layer", state: "pending" };

  it("matches by exact task title, case/space-insensitive", () => {
    expect(doubtTouchesCard(["Session / token layer"], card)).toBe(true);
    expect(doubtTouchesCard(["  session / TOKEN layer  "], card)).toBe(true);
  });

  it("matches by 1-based task number", () => {
    expect(doubtTouchesCard(["2"], card)).toBe(true);
  });

  it("does not match an unrelated task / empty affects", () => {
    expect(doubtTouchesCard(["Login screen"], card)).toBe(false);
    expect(doubtTouchesCard([], card)).toBe(false);
    expect(doubtTouchesCard([""], card)).toBe(false);
  });
});

describe("chatMessagesWithMilestones", () => {
  it("maps chat entries + coder milestones in timeline order, threading msgId", () => {
    const entries: ConsoleEntry[] = [
      { type: "coder", text: "Planning: 3 files", time: "1" },
      { type: "chat", role: "user", text: "build OAuth", time: "2", msgId: "m1" },
      { type: "webSearch", query: "oauth", pages: [], time: "3" },
      { type: "chat", role: "assistant", text: "On it — drafting a plan.", time: "4" },
    ];
    expect(chatMessagesWithMilestones(entries)).toEqual([
      { role: "milestone", text: "Planning: 3 files" },
      { role: "user", text: "build OAuth", msgId: "m1" },
      { role: "assistant", text: "On it — drafting a plan." },
    ]);
  });

  it("returns [] for undefined / no mappable entries", () => {
    expect(chatMessagesWithMilestones(undefined)).toEqual([]);
    expect(
      chatMessagesWithMilestones([
        { type: "webSearch", query: "x", pages: [], time: "1" },
      ]),
    ).toEqual([]);
  });

  it("drops whitespace-only chat entries (defense-in-depth: never emit a blank bubble)", () => {
    // The local Qwen model emits whitespace-only text blocks between thinking/tool
    // segments; even if a future backend regression let one through, the model
    // must never surface as a blank pill.
    const entries: ConsoleEntry[] = [
      { type: "chat", role: "user", text: "hi", time: "1" },
      { type: "chat", role: "assistant", text: "   ", time: "2" },
      { type: "chat", role: "assistant", text: "\n", time: "3" },
      { type: "chat", role: "assistant", text: "\t \n", time: "4" },
    ];
    expect(chatMessagesWithMilestones(entries)).toEqual([
      { role: "user", text: "hi" },
    ]);
  });

  it("drops whitespace-only coder/spawn milestone entries", () => {
    // Same root cause on the milestone side — a blank milestone row is just
    // a blank pill in disguise. Skip empty coder/spawn text.
    const entries: ConsoleEntry[] = [
      { type: "coder", text: "", time: "1" },
      { type: "coder", text: "   ", time: "2" },
      { type: "coder", text: "Planning: 3 files", time: "3" },
    ];
    expect(chatMessagesWithMilestones(entries)).toEqual([
      { role: "milestone", text: "Planning: 3 files" },
    ]);
  });

  it("keeps short consecutive milestone runs fully expanded (≤4)", () => {
    const entries: ConsoleEntry[] = [
      { type: "coder", text: "🔧 Calling `a`", time: "1" },
      { type: "coder", text: "✅ `a` → ok", time: "2" },
      { type: "coder", text: "🔧 Calling `b`", time: "3" },
      { type: "coder", text: "✅ `b` → ok", time: "4" },
    ];
    const msgs = chatMessagesWithMilestones(entries);
    expect(msgs).toHaveLength(4);
    expect(msgs.every((m) => m.role === "milestone")).toBe(true);
    expect(msgs.some((m) => m.text.startsWith("…"))).toBe(false);
  });

  it("compresses long tool-call spam: summary + last 3 milestones", () => {
    // Live OpenRouter smoke flooded the chat with 15+ MCP call/result rows.
    const tools = [
      "agent_register",
      "provider_credentials_status",
      "project_get",
      "oracle_context",
      "project_next_task",
      "project_claim_task",
      "agent_heartbeat",
      "spawn_mini_coder",
    ];
    const entries: ConsoleEntry[] = [];
    for (const t of tools) {
      entries.push({
        type: "coder",
        text: `🔧 Calling \`mcp_devboule_${t}\``,
        time: String(entries.length),
      });
      entries.push({
        type: "coder",
        text: `✅ \`mcp_devboule_${t}\` → ok`,
        time: String(entries.length),
      });
    }
    // 16 milestones → 1 summary + last 3 = 4 rows
    const msgs = chatMessagesWithMilestones(entries);
    expect(msgs).toHaveLength(4);
    expect(msgs[0].role).toBe("milestone");
    expect(msgs[0].text).toMatch(/^… 13 earlier tool steps/);
    expect(msgs[0].text).toContain("agent_register");
    expect(msgs[0].title).toContain("agent_register");
    expect(msgs.slice(1).map((m) => m.text)).toEqual([
      "✅ `mcp_devboule_agent_heartbeat` → ok",
      "🔧 Calling `mcp_devboule_spawn_mini_coder`",
      "✅ `mcp_devboule_spawn_mini_coder` → ok",
    ]);
  });

  it("does not compress across chat bubbles (only consecutive runs)", () => {
    const entries: ConsoleEntry[] = [
      { type: "coder", text: "🔧 Calling `a`", time: "1" },
      { type: "coder", text: "✅ `a`", time: "2" },
      { type: "coder", text: "🔧 Calling `b`", time: "3" },
      { type: "coder", text: "✅ `b`", time: "4" },
      { type: "coder", text: "🔧 Calling `c`", time: "5" },
      { type: "chat", role: "assistant", text: "mid reply", time: "6" },
      { type: "coder", text: "🔧 Calling `d`", time: "7" },
      { type: "coder", text: "✅ `d`", time: "8" },
    ];
    // First run is 5 (>4) → summary + 3; chat; last run is 2 → keep.
    const msgs = chatMessagesWithMilestones(entries);
    expect(msgs[0].text).toMatch(/^… 2 earlier tool steps/);
    expect(msgs.some((m) => m.role === "assistant" && m.text === "mid reply")).toBe(
      true,
    );
    expect(msgs.filter((m) => m.role === "milestone").length).toBe(1 + 3 + 2);
  });
});

describe("pickProjectDesign", () => {
  it("returns null for no root or no entries", () => {
    expect(pickProjectDesign([], "/proj")).toBeNull();
    expect(pickProjectDesign([designEntry({})], null)).toBeNull();
    expect(pickProjectDesign([designEntry({})], "")).toBeNull();
  });

  it("matches a design at the project root OR under it", () => {
    const atRoot = designEntry({ id: "root", workingFolderPath: "/proj" });
    const under = designEntry({ id: "under", workingFolderPath: "/proj/design" });
    const other = designEntry({ id: "other", workingFolderPath: "/elsewhere" });
    expect(pickProjectDesign([other, atRoot], "/proj")?.id).toBe("root");
    expect(pickProjectDesign([other, under], "/proj")?.id).toBe("under");
    expect(pickProjectDesign([other], "/proj")).toBeNull();
  });

  it("picks the most-recent match by lastOpenedAt", () => {
    const older = designEntry({
      id: "older",
      workingFolderPath: "/proj",
      lastOpenedAt: "2026-01-01T00:00:00Z",
    });
    const newer = designEntry({
      id: "newer",
      workingFolderPath: "/proj",
      lastOpenedAt: "2026-06-01T00:00:00Z",
    });
    expect(pickProjectDesign([older, newer], "/proj")?.id).toBe("newer");
    expect(pickProjectDesign([newer, older], "/proj")?.id).toBe("newer");
  });

  it("does not match a sibling whose path merely shares a prefix string", () => {
    // '/proj2' must NOT match root '/proj' (prefix-string trap).
    const sibling = designEntry({ id: "s", workingFolderPath: "/proj2" });
    expect(pickProjectDesign([sibling], "/proj")).toBeNull();
  });

  it("still returns a sole match with an empty lastOpenedAt (legacy entry)", () => {
    const legacy = designEntry({
      id: "legacy",
      workingFolderPath: "/proj",
      lastOpenedAt: "",
    });
    expect(pickProjectDesign([legacy], "/proj")?.id).toBe("legacy");
  });
});

// Minimal ProjectTask factory — only the fields derivePlanCards reads matter.
function task(over: Partial<ProjectTask>): ProjectTask {
  return {
    id: "t",
    title: "T",
    status: "todo",
    priority: null,
    assignee: null,
    due: null,
    linkedResources: [],
    updatedAt: "",
    suspectFileIds: [],
    ...over,
  };
}

describe("derivePlanCards", () => {
  it("numbers cards from 1 in array order", () => {
    const cards = derivePlanCards([
      task({ title: "a" }),
      task({ title: "b" }),
      task({ title: "c" }),
    ]);
    expect(cards.map((c) => c.n)).toEqual([1, 2, 3]);
    expect(cards.map((c) => c.title)).toEqual(["a", "b", "c"]);
  });

  it("maps status to state: done->done, wip->forming, rest->pending", () => {
    const states = derivePlanCards([
      task({ status: "done" }),
      task({ status: "wip" }),
      task({ status: "todo" }),
      task({ status: "review" }),
      task({ status: "blocked" }),
    ]).map((c) => c.state);
    expect(states).toEqual([
      "done",
      "forming",
      "pending",
      "pending",
      "pending",
    ]);
  });

  it("falls back to 'Task N' for an empty/whitespace title", () => {
    const cards = derivePlanCards([task({ title: "   " }), task({ title: "" })]);
    expect(cards[0].title).toBe("Task 1");
    expect(cards[1].title).toBe("Task 2");
  });

  it("trims a padded title", () => {
    expect(derivePlanCards([task({ title: "  hi  " })])[0].title).toBe("hi");
  });

  it("returns [] for an empty list", () => {
    expect(derivePlanCards([])).toEqual([] as PlanCard[]);
  });
});

describe("pageHostname", () => {
  it("strips protocol + path", () => {
    expect(pageHostname("https://stripe.com/docs/billing/usage")).toBe(
      "stripe.com",
    );
  });

  it("handles a url with no protocol", () => {
    expect(pageHostname("github.com/arlyon/async-stripe")).toBe("github.com");
  });

  it("returns the trimmed input on garbage", () => {
    expect(pageHostname("  not a url  ")).toBe("not a url");
  });
});

describe("stripLabel", () => {
  it("maps each view to its label", () => {
    expect(stripLabel("exa")).toBe("searching");
    expect(stripLabel("plan")).toBe("planning");
    expect(stripLabel("design")).toBe("designing");
  });
});

// ---- D3 (planner-chat demolition): identity-based pending reconciliation ------

import { mergePendingSends, stableOrchestratorAgentId, type PendingSend } from "./plannerModel";

describe("mergePendingSends", () => {
  const pending = (text: string, msgId: string): PendingSend => ({ text, msgId });

  it("appends pendings the bridge has not echoed yet", () => {
    const real = [{ role: "assistant" as const, text: "hi" }];
    const out = mergePendingSends(real, [pending("do it", "m1")]);
    expect(out).toEqual([
      { role: "assistant", text: "hi" },
      { role: "user", text: "do it", msgId: "m1" },
    ]);
  });

  it("drains a pending BY ID when the echo carries its msgId", () => {
    const real = [
      { role: "user" as const, text: "do it", msgId: "m1" },
      { role: "assistant" as const, text: "done" },
    ];
    const out = mergePendingSends(real, [pending("do it", "m1")]);
    expect(out).toEqual(real);
  });

  it("repeated identical sends drain one-by-one by their own ids", () => {
    // The exact case the old count-watermark existed for: "yes" sent twice.
    const real = [
      { role: "user" as const, text: "yes", msgId: "m1" },
      { role: "assistant" as const, text: "ok" },
    ];
    const out = mergePendingSends(real, [pending("yes", "m1"), pending("yes", "m2")]);
    expect(out).toEqual([...real, { role: "user", text: "yes", msgId: "m2" }]);
  });

  it("falls back to consuming one id-less echo per text match (local binary echoes)", () => {
    // The local orchestrator binary echoes user steers WITHOUT a msgId. Each id-less
    // user row consumes exactly ONE text-matching pending (oldest first).
    const real = [
      { role: "user" as const, text: "yes" },
      { role: "assistant" as const, text: "ok" },
      { role: "user" as const, text: "yes" },
    ];
    const out = mergePendingSends(real, [
      pending("yes", "m1"),
      pending("yes", "m2"),
      pending("yes", "m3"),
    ]);
    expect(out).toEqual([...real, { role: "user", text: "yes", msgId: "m3" }]);
  });

  it("an id-less echo never consumes a pending with a DIFFERENT text", () => {
    const real = [{ role: "user" as const, text: "first message" }];
    const out = mergePendingSends(real, [pending("second message", "m2")]);
    expect(out).toEqual([...real, { role: "user", text: "second message", msgId: "m2" }]);
  });

  it("milestone rows ride through untouched and never match pendings", () => {
    const real = [
      { role: "milestone" as const, text: "Bash: ls" },
      { role: "user" as const, text: "go", msgId: "m1" },
    ];
    const out = mergePendingSends(real, [pending("go", "m1"), pending("next", "m2")]);
    expect(out).toEqual([...real, { role: "user", text: "next", msgId: "m2" }]);
  });

  it("empty inputs are total", () => {
    expect(mergePendingSends([], [])).toEqual([]);
    expect(mergePendingSends([], [pending("a", "m1")])).toEqual([
      { role: "user", text: "a", msgId: "m1" },
    ]);
  });
});

describe("stableOrchestratorAgentId", () => {
  it("mirrors the backend id: orchestrator-<sanitized project id>", () => {
    // MUST stay byte-identical to Rust `stable_orchestrator_agent_id` (projects.rs):
    // charset [A-Za-z0-9._-] (others -> '_'), capped at 100 chars of project id.
    expect(stableOrchestratorAgentId("my-project.v2")).toBe(
      "orchestrator-my-project.v2",
    );
    expect(stableOrchestratorAgentId("we ird/../id")).toBe(
      "orchestrator-we_ird_.._id",
    );
    expect(stableOrchestratorAgentId("x".repeat(500))).toBe(
      `orchestrator-${"x".repeat(100)}`,
    );
  });
});

// ---- pi plan (orchestrator `plan` tool payload) ------------------------------

function chatEntry(time: string, role: "user" | "assistant", text: string): ConsoleEntry {
  return { type: "chat", role, text, time };
}

function validPlanPayload(): PiPlan {
  return {
    title: "Build the auth layer",
    steps: [
      { text: "Design the token schema", status: "done" },
      { text: "Implement the refresh flow", status: "in_progress" },
      { text: "Write integration tests", status: "pending" },
      { text: "Skip the legacy path", status: "skipped" },
    ],
    notes: "See RFC-42 for the token TTL decision",
  };
}

describe("latestPlan", () => {
  it("returns null for undefined / empty / no plan entries", () => {
    expect(latestPlan(undefined)).toBeNull();
    expect(latestPlan([])).toBeNull();
    expect(latestPlan([chatEntry("1", "user", "hi")])).toBeNull();
  });

  it("parses a valid payload (title / steps / notes)", () => {
    const p = validPlanPayload();
    const out = latestPlan([planEntry("1", p)]);
    expect(out).toEqual(p);
  });

  it("last plan entry wins over an earlier one", () => {
    const first = { title: "A", steps: [{ text: "a", status: "pending" }] };
    const second = { title: "B", steps: [{ text: "b", status: "done" }] };
    const out = latestPlan([
      planEntry("1", first),
      planEntry("2", second),
    ]);
    expect(out!.title).toBe("B");
    expect(out!.steps[0].text).toBe("b");
  });

  it("malformed JSON in the newest plan entry returns null (no fallback)", () => {
    const older = validPlanPayload();
    const out = latestPlan([
      planEntry("1", older),
      planEntry("2", "NOT JSON"),
    ]);
    expect(out).toBeNull();
  });

  it("steps with unknown status normalized to 'pending'; empty text dropped", () => {
    const p = {
      title: "T",
      steps: [
        { text: "a", status: "unknown_status" },
        { text: "", status: "pending" },
        { text: "b", status: "done" },
      ],
    };
    const out = latestPlan([planEntry("1", p)]);
    expect(out!.steps.map((s) => s.status)).toEqual(["pending", "done"]);
    expect(out!.steps.map((s) => s.text)).toEqual(["a", "b"]);
  });

  it("notes absent when missing from the payload", () => {
    const p = { title: "T", steps: [{ text: "a", status: "pending" }] };
    const out = latestPlan([planEntry("1", p)]);
    expect(out!.notes).toBeUndefined();
  });

  it("notes absent when empty string", () => {
    const p = {
      title: "T",
      steps: [{ text: "a", status: "pending" }],
      notes: "",
    };
    const out = latestPlan([planEntry("1", p)]);
    expect(out!.notes).toBeUndefined();
  });

  it("returns null when title is not a non-empty string", () => {
    expect(latestPlan([planEntry("1", { title: "", steps: [] })])).toBeNull();
    expect(latestPlan([planEntry("1", { title: 42, steps: [] })])).toBeNull();
    expect(latestPlan([planEntry("1", { title: null, steps: [] })])).toBeNull();
  });

  it("returns null for a whitespace-only title", () => {
    expect(latestPlan([planEntry("1", { title: "   ", steps: [] })])).toBeNull();
    expect(latestPlan([planEntry("1", { title: "\t\n", steps: [] })])).toBeNull();
  });

  it("returns null when steps is not an array", () => {
    expect(latestPlan([planEntry("1", { title: "T", steps: "nope" })])).toBeNull();
    expect(latestPlan([planEntry("1", { title: "T" })])).toBeNull();
  });

  it("drops non-object step entries, keeps only the valid one (status -> pending)", () => {
    const p = {
      title: "T",
      steps: [null, 42, "string", { text: "ok" }] as unknown[],
    };
    const out = latestPlan([planEntry("1", p)]);
    expect(out!.steps).toHaveLength(1);
    expect(out!.steps[0].text).toBe("ok");
    expect(out!.steps[0].status).toBe("pending");
  });
});

describe("planCardsFromPiPlan", () => {
  it("maps all 4 statuses to the 4 states", () => {
    const plan: PiPlan = {
      title: "T",
      steps: [
        { text: "a", status: "done" },
        { text: "b", status: "in_progress" },
        { text: "c", status: "pending" },
        { text: "d", status: "skipped" },
      ],
    };
    const cards = planCardsFromPiPlan(plan);
    expect(cards.map((c) => c.state)).toEqual(["done", "forming", "pending", "skipped"]);
  });

  it("cards are 1-based and title = step text", () => {
    const plan: PiPlan = {
      title: "T",
      steps: [{ text: "alpha", status: "pending" }, { text: "beta", status: "done" }],
    };
    const cards = planCardsFromPiPlan(plan);
    expect(cards.map((c) => c.n)).toEqual([1, 2]);
    expect(cards.map((c) => c.title)).toEqual(["alpha", "beta"]);
  });

  it("returns [] for a plan with no steps", () => {
    expect(planCardsFromPiPlan({ title: "T", steps: [] })).toEqual([]);
  });
});

describe("chatMessagesWithMilestones (plan-role filtering)", () => {
  it("a role:plan chat entry is NOT in the output while surrounding user/assistant entries are", () => {
    const entries: ConsoleEntry[] = [
      chatEntry("1", "user", "build auth"),
      planEntry("2", validPlanPayload()),
      chatEntry("3", "assistant", "On it"),
    ];
    expect(chatMessagesWithMilestones(entries)).toEqual([
      { role: "user", text: "build auth" },
      { role: "assistant", text: "On it" },
    ]);
  });
});

describe("stageViewOnDoubts (F48 — force plan when doubts arrive)", () => {
  it("question arrival (len grows) → plan", () => {
    expect(stageViewOnDoubts(0, 1, "exa")).toBe("plan");
    expect(stageViewOnDoubts(2, 3, "design")).toBe("plan");
  });

  it("no change in open-question count → null", () => {
    expect(stageViewOnDoubts(0, 0, "exa")).toBeNull();
    expect(stageViewOnDoubts(3, 3, "exa")).toBeNull();
    expect(stageViewOnDoubts(2, 1, "plan")).toBeNull();
  });

  it("mount / first-obs with open questions (prevLen 0, len > 0) → plan", () => {
    // Callers init prevQuestionsLen to 0 so remount-with-open-doubts is an arrival.
    expect(stageViewOnDoubts(0, 3, "exa")).toBe("plan");
  });
});
