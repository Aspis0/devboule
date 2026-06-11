import { describe, it, expect } from "vitest";
import {
  formatThousands,
  relativeTime,
  colorSwatches,
  thumbColorFromId,
} from "./format";

describe("formatThousands", () => {
  it("groups by 3 digits", () => {
    expect(formatThousands(0)).toBe("0");
    expect(formatThousands(7)).toBe("7");
    expect(formatThousands(999)).toBe("999");
    expect(formatThousands(1000)).toBe("1,000");
    expect(formatThousands(1284)).toBe("1,284");
    expect(formatThousands(1234567)).toBe("1,234,567");
  });

  it("is null/NaN/Infinity-safe", () => {
    expect(formatThousands(undefined)).toBe("0");
    expect(formatThousands(null)).toBe("0");
    expect(formatThousands(NaN)).toBe("0");
    expect(formatThousands(Infinity)).toBe("0");
  });

  it("handles negatives and truncates fractions", () => {
    expect(formatThousands(-2500)).toBe("-2,500");
    expect(formatThousands(1999.9)).toBe("1,999");
  });
});

describe("relativeTime", () => {
  const now = Date.parse("2026-06-10T12:00:00Z");

  it("returns 'never' for empty/invalid", () => {
    expect(relativeTime("", now)).toBe("never");
    expect(relativeTime(undefined, now)).toBe("never");
    expect(relativeTime("not-a-date", now)).toBe("never");
  });

  it("buckets recent instants", () => {
    expect(relativeTime("2026-06-10T11:59:50Z", now)).toBe("just now"); // 10s
    expect(relativeTime("2026-06-10T11:58:00Z", now)).toBe("2m ago");
    expect(relativeTime("2026-06-10T09:00:00Z", now)).toBe("3h ago");
    expect(relativeTime("2026-06-05T12:00:00Z", now)).toBe("5d ago");
  });

  it("collapses future instants to 'just now'", () => {
    expect(relativeTime("2026-06-10T12:05:00Z", now)).toBe("just now");
  });

  it("falls back to a date string past a week", () => {
    const label = relativeTime("2026-05-01T12:00:00Z", now);
    expect(label).not.toMatch(/ago|just now|never/);
    expect(label.length).toBeGreaterThan(0);
  });
});

describe("colorSwatches", () => {
  it("extracts up to N color $values in key-sorted order", () => {
    const doc = {
      color: {
        brand: { $value: "#C14B1B", $type: "color" },
        accent: { $value: "#3B2D1D", $type: "color" },
        ink: { $value: "#F3E3CB", $type: "color" },
      },
    };
    // accent < brand < ink alphabetically.
    expect(colorSwatches(doc, 4)).toEqual(["#3B2D1D", "#C14B1B", "#F3E3CB"]);
  });

  it("respects the max", () => {
    const doc = {
      c: {
        a: { $value: "#111111", $type: "color" },
        b: { $value: "#222222", $type: "color" },
        c: { $value: "#333333", $type: "color" },
      },
    };
    expect(colorSwatches(doc, 2)).toEqual(["#111111", "#222222"]);
  });

  it("skips non-color, composite, and reference values", () => {
    const doc = {
      spacing: { md: { $value: "8px", $type: "dimension" } },
      color: {
        shadow: { $value: { x: 1 }, $type: "color" }, // composite — skipped
        alias: { $value: "{color.brand}", $type: "color" }, // ref string — kept (literal string)
        real: { $value: "#abcabc", $type: "color" },
      },
    };
    // alias is a string so it passes the string test (we don't resolve refs here),
    // real is a literal. shadow (object) is skipped. Order: alias < real.
    expect(colorSwatches(doc, 4)).toEqual(["{color.brand}", "#abcabc"]);
  });

  it("returns [] for empty/invalid docs", () => {
    expect(colorSwatches({}, 4)).toEqual([]);
    expect(colorSwatches(null, 4)).toEqual([]);
    expect(colorSwatches([], 4)).toEqual([]);
    expect(colorSwatches("nope", 4)).toEqual([]);
  });
});

describe("thumbColorFromId", () => {
  it("is deterministic", () => {
    expect(thumbColorFromId("demo")).toBe(thumbColorFromId("demo"));
  });

  it("produces a valid hsl string", () => {
    expect(thumbColorFromId("pricing")).toMatch(/^hsl\(\d+ 32% 84%\)$/);
  });

  it("differs across ids (usually)", () => {
    expect(thumbColorFromId("a")).not.toBe(thumbColorFromId("zzzz"));
  });
});
