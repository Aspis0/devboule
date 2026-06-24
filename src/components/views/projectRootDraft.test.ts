import { afterEach, beforeEach, describe, expect, test } from "vitest";
import {
  clearPersistedProjectRootDraft,
  persistProjectRootDraft,
  readPersistedProjectRootDraft,
} from "./projectRootDraft";

// Finding 5: the per-project root-editor draft must survive the idle auto-lock
// (which unmounts ProjectsView and wipes useState), mirroring B5's create-flow
// folder draft. These pure helpers own the localStorage round-trip; the env is
// "node" so we stub a minimal localStorage.

class MemoryStorage {
  private map = new Map<string, string>();
  getItem(k: string): string | null {
    return this.map.has(k) ? (this.map.get(k) as string) : null;
  }
  setItem(k: string, v: string): void {
    this.map.set(k, v);
  }
  removeItem(k: string): void {
    this.map.delete(k);
  }
  clear(): void {
    this.map.clear();
  }
  key(i: number): string | null {
    return Array.from(this.map.keys())[i] ?? null;
  }
  get length(): number {
    return this.map.size;
  }
  get size(): number {
    return this.map.size;
  }
  raw(k: string): string | null {
    return this.getItem(k);
  }
}

let store: MemoryStorage;

beforeEach(() => {
  store = new MemoryStorage();
  (globalThis as unknown as { localStorage: MemoryStorage }).localStorage = store;
});

afterEach(() => {
  delete (globalThis as unknown as { localStorage?: MemoryStorage }).localStorage;
});

describe("projectRootDraft persistence", () => {
  test("persists an unsaved edit and restores it (survives remount)", () => {
    persistProjectRootDraft("proj-1", "/home/user/new-folder", null);
    // simulate remount: a fresh read sees the stored draft
    expect(readPersistedProjectRootDraft("proj-1")).toBe("/home/user/new-folder");
  });

  test("uses a project-scoped key (drafts do not bleed across projects)", () => {
    persistProjectRootDraft("proj-1", "/a", null);
    persistProjectRootDraft("proj-2", "/b", null);
    expect(readPersistedProjectRootDraft("proj-1")).toBe("/a");
    expect(readPersistedProjectRootDraft("proj-2")).toBe("/b");
  });

  test("does NOT persist a draft equal to the saved rootPath (no noise)", () => {
    persistProjectRootDraft("proj-1", "/saved", "/saved");
    expect(readPersistedProjectRootDraft("proj-1")).toBeNull();
    expect(store.size).toBe(0);
  });

  test("does NOT persist an empty/whitespace draft, and clears any prior one", () => {
    persistProjectRootDraft("proj-1", "/typed", null);
    persistProjectRootDraft("proj-1", "   ", null);
    expect(readPersistedProjectRootDraft("proj-1")).toBeNull();
    persistProjectRootDraft("proj-1", "/typed2", null);
    persistProjectRootDraft("proj-1", "", null);
    expect(readPersistedProjectRootDraft("proj-1")).toBeNull();
  });

  test("a draft equal to the saved rootPath after trim is not persisted (Finding 2)", () => {
    // trailing whitespace only — setProjectRoot would save the trimmed value, so
    // this is "nothing unsaved" and must be removed, not persisted forever.
    persistProjectRootDraft("proj-1", "/saved  ", "/saved");
    expect(readPersistedProjectRootDraft("proj-1")).toBeNull();
    // a genuinely different value (beyond whitespace) IS persisted
    persistProjectRootDraft("proj-1", "/saved/x", "/saved");
    expect(readPersistedProjectRootDraft("proj-1")).toBe("/saved/x");
  });

  test("a later edit overwrites the stored draft", () => {
    persistProjectRootDraft("proj-1", "/first", null);
    persistProjectRootDraft("proj-1", "/second", null);
    expect(readPersistedProjectRootDraft("proj-1")).toBe("/second");
  });

  test("clear removes the stored draft (used after a successful save)", () => {
    persistProjectRootDraft("proj-1", "/typed", null);
    clearPersistedProjectRootDraft("proj-1");
    expect(readPersistedProjectRootDraft("proj-1")).toBeNull();
  });

  test("a null/empty projectId is a no-op and never throws", () => {
    expect(() => persistProjectRootDraft(null, "/x", null)).not.toThrow();
    expect(() => persistProjectRootDraft(undefined, "/x", null)).not.toThrow();
    expect(readPersistedProjectRootDraft(null)).toBeNull();
    expect(readPersistedProjectRootDraft(undefined)).toBeNull();
    expect(() => clearPersistedProjectRootDraft(null)).not.toThrow();
    expect(store.size).toBe(0);
  });

  test("storage throwing (private mode / disabled) never propagates", () => {
    const throwing = {
      getItem() {
        throw new Error("denied");
      },
      setItem() {
        throw new Error("denied");
      },
      removeItem() {
        throw new Error("denied");
      },
    };
    (globalThis as unknown as { localStorage: unknown }).localStorage = throwing;
    expect(() => persistProjectRootDraft("p", "/x", null)).not.toThrow();
    expect(readPersistedProjectRootDraft("p")).toBeNull();
    expect(() => clearPersistedProjectRootDraft("p")).not.toThrow();
  });
});
