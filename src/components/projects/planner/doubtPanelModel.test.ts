import { describe, expect, it } from "vitest";
import {
  acceptDoubtAnswerOnce,
  DOUBT_OPTION_FONT_PX,
  DOUBT_QUESTION_FONT_PX,
  openDoubts,
} from "./doubtPanelModel";

describe("acceptDoubtAnswerOnce (F38)", () => {
  it("accepts the first answer and rejects subsequent for the same id", () => {
    const empty = new Set<string>();
    const first = acceptDoubtAnswerOnce(empty, "q1");
    expect(first.accepted).toBe(true);
    expect(first.next.has("q1")).toBe(true);

    const second = acceptDoubtAnswerOnce(first.next, "q1");
    expect(second.accepted).toBe(false);
    expect(second.next.has("q1")).toBe(true);
    expect(second.next.size).toBe(1);
  });

  it("allows different question ids independently", () => {
    let s = new Set<string>();
    const a = acceptDoubtAnswerOnce(s, "q1");
    expect(a.accepted).toBe(true);
    s = a.next;
    const b = acceptDoubtAnswerOnce(s, "q2");
    expect(b.accepted).toBe(true);
    expect(b.next.size).toBe(2);
  });
});

describe("openDoubts (F38 dismiss)", () => {
  it("filters settled questions out of the open list", () => {
    const qs = [{ id: "a" }, { id: "b" }, { id: "c" }];
    expect(openDoubts(qs, new Set(["b"]))).toEqual([{ id: "a" }, { id: "c" }]);
  });
});

describe("readable sizing contract (F37)", () => {
  it("uses font sizes large enough for long Italian copy", () => {
    // Owner: 12.5 / 11.5 was unreadable; floor must stay above those.
    expect(DOUBT_QUESTION_FONT_PX).toBeGreaterThanOrEqual(14);
    expect(DOUBT_OPTION_FONT_PX).toBeGreaterThanOrEqual(13);
  });
});
