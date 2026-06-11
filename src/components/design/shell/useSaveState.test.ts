import { describe, it, expect } from "vitest";
import { deriveSaveState } from "./useSaveState";

describe("deriveSaveState", () => {
  it("is 'saved' when clean", () => {
    expect(deriveSaveState({ writing: false, pendingDirty: false })).toBe("saved");
  });

  it("is 'dirty' when a change is pending but nothing is being written", () => {
    expect(deriveSaveState({ writing: false, pendingDirty: true })).toBe("dirty");
  });

  it("is 'writing' while an IPC save is in flight", () => {
    expect(deriveSaveState({ writing: true, pendingDirty: false })).toBe("writing");
  });

  it("prioritizes 'writing' over 'dirty'", () => {
    expect(deriveSaveState({ writing: true, pendingDirty: true })).toBe("writing");
  });
});
