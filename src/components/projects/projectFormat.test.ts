import { describe, expect, it } from "vitest";
import { relativeTime, formatDate } from "./projectFormat";

describe("relativeTime", () => {
  // Fixed "now" so the buckets are deterministic (no real clock dependency).
  const now = new Date("2026-07-09T12:00:00Z");

  it('buckets seconds as "just now"', () => {
    expect(relativeTime("2026-07-09T12:00:30Z", now)).toBe("just now");
  });

  it('buckets minutes as "Xm ago"', () => {
    expect(relativeTime("2026-07-09T11:55:00Z", now)).toBe("5m ago");
  });

  it('buckets hours as "Xh ago"', () => {
    expect(relativeTime("2026-07-09T10:00:00Z", now)).toBe("2h ago");
  });

  it('buckets days as "Xd ago"', () => {
    expect(relativeTime("2026-07-06T12:00:00Z", now)).toBe("3d ago");
  });

  it("falls back to an absolute short date beyond the threshold (>= 30 days)", () => {
    const iso = "2026-06-01T12:00:00Z"; // ~38 days before now
    expect(relativeTime(iso, now)).toBe(formatDate(iso));
  });

  it("treats a future timestamp as just now (never negative buckets)", () => {
    expect(relativeTime("2026-07-09T13:00:00Z", now)).toBe("just now");
  });
});
