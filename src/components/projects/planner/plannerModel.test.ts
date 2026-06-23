import { describe, it, expect } from "vitest";
import {
  derivePlanCards,
  pageHostname,
  stripLabel,
  pickProjectDesign,
  type PlanCard,
} from "./plannerModel";
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
