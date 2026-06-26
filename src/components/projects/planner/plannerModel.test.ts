import { describe, it, expect } from "vitest";
import {
  derivePlanCards,
  pageHostname,
  stripLabel,
  pickProjectDesign,
  chatMessages,
  openQuestions,
  steerPickOption,
  steerYouDecide,
  doubtTouchesCard,
  type PlanCard,
} from "./plannerModel";
import type { ConsoleEntry, QuestionEntry } from "../../agents/agentConsoleModel";
import type { ProjectTask } from "../../../types/backend";
import type { DesignProjectEntry } from "../../../types/design";

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

describe("chatMessages", () => {
  it("keeps only chat entries, in order, mapping role+text", () => {
    const entries: ConsoleEntry[] = [
      { type: "coder", text: "Planning: 3 files", time: "1" },
      { type: "chat", role: "user", text: "build OAuth", time: "2" },
      { type: "webSearch", query: "oauth", pages: [], time: "3" },
      { type: "chat", role: "assistant", text: "On it — drafting a plan.", time: "4" },
    ];
    expect(chatMessages(entries)).toEqual([
      { role: "user", text: "build OAuth" },
      { role: "assistant", text: "On it — drafting a plan." },
    ]);
  });

  it("returns [] for undefined / no chat entries", () => {
    expect(chatMessages(undefined)).toEqual([]);
    expect(
      chatMessages([{ type: "coder", text: "x", time: "1" }]),
    ).toEqual([]);
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
