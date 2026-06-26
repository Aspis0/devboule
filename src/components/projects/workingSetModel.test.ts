import { describe, expect, it } from "vitest";
import { shouldAdoptWorkingSet } from "./workingSetModel";

// ── shouldAdoptWorkingSet ─────────────────────────────────────────────────────
//
// Guards the prop-sync useEffect so a stale background poll never clobbers the
// canonical list returned by the last successful add/remove IPC.

describe("shouldAdoptWorkingSet", () => {
  // ── busy = true: never adopt ─────────────────────────────────────────────
  it("returns false when busy, regardless of pending or incoming", () => {
    expect(shouldAdoptWorkingSet([], null, true)).toBe(false);
    expect(shouldAdoptWorkingSet(["/a"], null, true)).toBe(false);
    expect(shouldAdoptWorkingSet(["/a"], ["/a"], true)).toBe(false);
    expect(shouldAdoptWorkingSet(["/b"], ["/a"], true)).toBe(false);
  });

  // ── no pending write, not busy: always adopt ─────────────────────────────
  it("returns true when not busy and no pending write (normal external refresh)", () => {
    expect(shouldAdoptWorkingSet([], null, false)).toBe(true);
    expect(shouldAdoptWorkingSet(["/a"], null, false)).toBe(true);
    expect(shouldAdoptWorkingSet(["/a", "/b"], null, false)).toBe(true);
  });

  // ── stale poll: incoming does NOT match lastWritten → skip ───────────────
  it("returns false when incoming differs from lastWritten (stale poll)", () => {
    // Our write added /b; poll still sends the old list without /b.
    expect(shouldAdoptWorkingSet(["/a"], ["/a", "/b"], false)).toBe(false);
    // Our write removed /b; poll still sends the old list with /b.
    expect(shouldAdoptWorkingSet(["/a", "/b"], ["/a"], false)).toBe(false);
    // Completely different paths.
    expect(shouldAdoptWorkingSet(["/c"], ["/a", "/b"], false)).toBe(false);
    // Empty incoming vs non-empty pending.
    expect(shouldAdoptWorkingSet([], ["/a"], false)).toBe(false);
    // Non-empty incoming vs empty pending.
    expect(shouldAdoptWorkingSet(["/a"], [], false)).toBe(false);
  });

  // ── parent caught up: incoming SET-EQUALS lastWritten → adopt ────────────
  it("returns true when incoming is set-equal to lastWritten (parent caught up)", () => {
    // Same single folder.
    expect(shouldAdoptWorkingSet(["/a"], ["/a"], false)).toBe(true);
    // Same two folders, same order.
    expect(shouldAdoptWorkingSet(["/a", "/b"], ["/a", "/b"], false)).toBe(true);
    // Same two folders, different order (order-independent set equality).
    expect(shouldAdoptWorkingSet(["/b", "/a"], ["/a", "/b"], false)).toBe(true);
    // Both empty.
    expect(shouldAdoptWorkingSet([], [], false)).toBe(true);
  });

  // ── macOS /tmp → /private/tmp: set equality handles canonical paths ───────
  it("returns false when paths differ due to canonicalization (stale prop with wrong path)", () => {
    // Old prop has the non-canonical /tmp path; lastWritten has canonical /private/tmp.
    expect(shouldAdoptWorkingSet(["/tmp/foo"], ["/private/tmp/foo"], false)).toBe(false);
  });

  it("returns true when both use the canonical /private/tmp path (parent caught up)", () => {
    expect(shouldAdoptWorkingSet(["/private/tmp/foo"], ["/private/tmp/foo"], false)).toBe(true);
  });
});
