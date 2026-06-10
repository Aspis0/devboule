// PURE tests for extractMarkup (no DOM needed).

import { describe, it, expect } from "vitest";
import { extractMarkup } from "./parseNodes";

describe("extractMarkup", () => {
  it("returns trimmed text when there are no fences", () => {
    expect(extractMarkup("  <div>hi</div>  ")).toBe("<div>hi</div>");
  });

  it("strips a single ```html fence and surrounding prose", () => {
    const text =
      "Here is your section:\n```html\n<section data-node-id=\"hero\">Hi</section>\n```\nHope it helps!";
    expect(extractMarkup(text)).toBe(
      '<section data-node-id="hero">Hi</section>',
    );
  });

  it("strips a bare ``` fence (no info string)", () => {
    expect(extractMarkup("```\n<button>Go</button>\n```")).toBe(
      "<button>Go</button>",
    );
  });

  it("concatenates multiple fenced blocks in order", () => {
    const text =
      "First:\n```html\n<section data-node-id=\"a\">A</section>\n```\nSecond:\n```html\n<button data-node-id=\"b\">B</button>\n```";
    expect(extractMarkup(text)).toBe(
      '<section data-node-id="a">A</section>\n<button data-node-id="b">B</button>',
    );
  });

  it("tolerates an unterminated fence (no closing ```)", () => {
    expect(extractMarkup("```html\n<div>partial</div>")).toBe(
      "<div>partial</div>",
    );
  });

  it("ignores empty fenced blocks", () => {
    expect(extractMarkup("```html\n\n```")).toBe("");
  });

  it("returns '' for non-string input", () => {
    expect(extractMarkup(undefined)).toBe("");
    expect(extractMarkup(null)).toBe("");
    expect(extractMarkup(42)).toBe("");
  });

  it("does not false-trigger on inline backticks in prose", () => {
    // No newline-anchored opening fence here, so the whole text is returned.
    expect(extractMarkup("use `code` like <span>x</span>")).toBe(
      "use `code` like <span>x</span>",
    );
  });
});
