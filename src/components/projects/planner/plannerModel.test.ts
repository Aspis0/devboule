import { describe, it, expect } from "vitest";
import {
  derivePlanCards,
  pageHostname,
  stripLabel,
  type PlanCard,
} from "./plannerModel";
import type { ProjectTask } from "../../../types/backend";

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
