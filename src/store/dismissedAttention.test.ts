// Unit tests for the dismissed-attention store + dismiss-key design.
//
// Verifies:
//   (a) empty set by default when localStorage is missing
//   (b) dismissAttention(key) adds + persists and get contains it
//   (c) clearAttentions(keys) adds many at once and persists
//   (d) dismissed set persists across fresh module import (reload)
//   (e) malformed storage ⇒ empty set
//   (f) subscribers notified on change
//   (g) attentionDismissKey: same agent + same since stays dismissed;
//       genuinely-new since resurfaces; health fallback for stale/lost
//   (h) filtering: dismissed key is hidden; new key from same agent is not
//   (i) "Clear all" (clearAttentions of all visible keys) hides everything
//   (j) cap drops oldest when exceeding max
//
// Environment: vitest `environment: "node"` — inject minimal fake window/
// localStorage (same pattern as dismissedRisks.test.ts).

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import type { AgentSession } from "../types/backend";

function makeFakeLocalStorage(): Storage {
  const store = new Map<string, string>();
  return {
    getItem: (k: string) => (store.has(k) ? (store.get(k) as string) : null),
    setItem: (k: string, v: string) => void store.set(k, String(v)),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
    key: (i: number) => Array.from(store.keys())[i] ?? null,
    get length() {
      return store.size;
    },
  } as Storage;
}

function session(partial: Partial<AgentSession> & { agentId: string }): AgentSession {
  return {
    role: "coder",
    model: null,
    status: "running",
    message: null,
    currentProjectId: "p1",
    currentTaskId: null,
    firstSeenAt: null,
    lastSeenAt: "2026-06-04T09:59:50.000Z",
    ...partial,
  };
}

describe("dismissedAttention store", () => {
  beforeEach(() => {
    vi.resetModules();
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (globalThis as any).window = globalThis;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (globalThis as any).localStorage = makeFakeLocalStorage();
  });

  afterEach(() => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    delete (globalThis as any).localStorage;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    delete (globalThis as any).window;
  });

  it("starts empty by default", async () => {
    const { getDismissedAttention } = await import("./dismissedAttention");
    expect(getDismissedAttention().size).toBe(0);
  });

  it("dismissAttention(key) adds + persists the key", async () => {
    const { getDismissedAttention, dismissAttention } = await import(
      "./dismissedAttention"
    );
    dismissAttention("agent-1::2026-06-04T10:00:00.000Z");

    expect(
      getDismissedAttention().has("agent-1::2026-06-04T10:00:00.000Z"),
    ).toBe(true);
    expect(getDismissedAttention().size).toBe(1);
    expect(
      window.localStorage.getItem("notifications:dismissedAttention"),
    ).toBe(JSON.stringify(["agent-1::2026-06-04T10:00:00.000Z"]));
  });

  it("dismissAttention is a no-op when the key is already dismissed", async () => {
    const { dismissAttention, getDismissedAttention } = await import(
      "./dismissedAttention"
    );
    dismissAttention("a");
    dismissAttention("a");

    expect(getDismissedAttention().size).toBe(1);
    expect(
      window.localStorage.getItem("notifications:dismissedAttention"),
    ).toBe(JSON.stringify(["a"]));
  });

  it("clearAttentions(keys) adds many at once", async () => {
    const { getDismissedAttention, clearAttentions } = await import(
      "./dismissedAttention"
    );
    clearAttentions(["a", "b"]);

    expect(getDismissedAttention().has("a")).toBe(true);
    expect(getDismissedAttention().has("b")).toBe(true);
    expect(getDismissedAttention().size).toBe(2);
    expect(
      window.localStorage.getItem("notifications:dismissedAttention"),
    ).toBe(JSON.stringify(["a", "b"]));
  });

  it("dismissed keys survive a fresh module import (re-reads persisted value)", async () => {
    const { dismissAttention } = await import("./dismissedAttention");
    dismissAttention("ghost::t1");

    vi.resetModules();
    const fresh = await import("./dismissedAttention");
    expect(fresh.getDismissedAttention().has("ghost::t1")).toBe(true);
  });

  it("malformed JSON storage falls back to empty set", async () => {
    window.localStorage.setItem("notifications:dismissedAttention", "not-json");
    const { getDismissedAttention } = await import("./dismissedAttention");
    expect(getDismissedAttention().size).toBe(0);
  });

  it("non-array JSON storage falls back to empty set", async () => {
    window.localStorage.setItem(
      "notifications:dismissedAttention",
      JSON.stringify({ not: "an array" }),
    );
    const { getDismissedAttention } = await import("./dismissedAttention");
    expect(getDismissedAttention().size).toBe(0);
  });

  it("notifies subscribers when keys are added", async () => {
    const { subscribe, dismissAttention, clearAttentions } = await import(
      "./dismissedAttention"
    );
    let calls = 0;
    const unsub = subscribe(() => {
      calls += 1;
    });

    dismissAttention("a");
    expect(calls).toBe(1);

    dismissAttention("a");
    expect(calls).toBe(1);

    clearAttentions(["b", "c"]);
    expect(calls).toBe(2);

    clearAttentions(["a", "b", "c"]);
    expect(calls).toBe(2);

    unsub();
  });

  it("caps the persisted set to the most-recent keys and drops the oldest", async () => {
    const { dismissAttention, getDismissedAttention } = await import(
      "./dismissedAttention"
    );
    for (let i = 0; i < 305; i += 1) {
      dismissAttention(`id-${i}`);
    }

    const stored = getDismissedAttention();
    expect(stored.size).toBe(300);
    expect(stored.has("id-304")).toBe(true);
    expect(stored.has("id-0")).toBe(false);
    const raw = window.localStorage.getItem("notifications:dismissedAttention");
    const parsed = JSON.parse(raw ?? "[]") as string[];
    expect(parsed).toHaveLength(300);
    expect(parsed).toContain("id-304");
    expect(parsed).not.toContain("id-0");
  });
});

describe("attentionDismissKey + filtering (resurface genuinely-new attention)", () => {
  beforeEach(() => {
    vi.resetModules();
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (globalThis as any).window = globalThis;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (globalThis as any).localStorage = makeFakeLocalStorage();
  });

  afterEach(() => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    delete (globalThis as any).localStorage;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    delete (globalThis as any).window;
  });

  it("keys needsUser attention on agentId + since + message fingerprint", async () => {
    const { attentionDismissKey } = await import("./dismissedAttention");
    const s = session({
      agentId: "coder-1",
      needsUser: {
        reason: "question",
        message: "Which?",
        since: "2026-06-04T10:00:00.000Z",
      },
    });
    // Collision-free JSON-array key: [agentId, since, fingerprint].
    const parsed = JSON.parse(attentionDismissKey(s)) as string[];
    expect(parsed[0]).toBe("coder-1");
    expect(parsed[1]).toBe("2026-06-04T10:00:00.000Z");
    expect(typeof parsed[2]).toBe("string");
    expect(parsed[2].length).toBeGreaterThan(0);
  });

  it("keys stale/lost (no needsUser) on agentId + lastSeenAt", async () => {
    const { attentionDismissKey } = await import("./dismissedAttention");
    const s = session({
      agentId: "ghost-1",
      lastSeenAt: "2026-06-04T09:00:00.000Z",
      needsUser: null,
    });
    expect(JSON.parse(attentionDismissKey(s))).toEqual([
      "ghost-1",
      "health",
      "2026-06-04T09:00:00.000Z",
    ]);
  });

  it("BLOCKER regression: same agent + SAME since but a NEW message resurfaces", async () => {
    // The backend PRESERVES `since` across consecutive needs_user reports while refreshing
    // the message (e.g. approve push branch A, then branch B, uninterrupted). Keying on since
    // alone would permanently mute the agent after the first dismiss. The message fingerprint
    // must make the second, genuinely-new request produce a different key → resurface.
    const {
      attentionDismissKey,
      dismissAttention,
      getDismissedAttention,
    } = await import("./dismissedAttention");

    const since = "2026-06-04T10:00:00.000Z";
    const first = session({
      agentId: "coder-1",
      needsUser: { reason: "needs_push_approval", message: "Approve push to branch A", since },
    });
    dismissAttention(attentionDismissKey(first));

    // Same agent, SAME since (uninterrupted needs_user), DIFFERENT message.
    const second = session({
      agentId: "coder-1",
      needsUser: { reason: "needs_push_approval", message: "Approve push to branch B", since },
    });
    expect(attentionDismissKey(first)).not.toBe(attentionDismissKey(second));
    const dismissed = getDismissedAttention();
    const visible = [second].filter((s) => !dismissed.has(attentionDismissKey(s)));
    expect(visible.map((s) => s.agentId)).toEqual(["coder-1"]);
  });

  it("collision-free: agentId containing '::' cannot forge another session's key", async () => {
    const { attentionDismissKey } = await import("./dismissedAttention");
    const a = session({
      agentId: "foo::bar",
      needsUser: { reason: "q", message: "m", since: "baz" },
    });
    const b = session({
      agentId: "foo",
      needsUser: { reason: "q", message: "m", since: "bar::baz" },
    });
    expect(attentionDismissKey(a)).not.toBe(attentionDismissKey(b));
  });

  it("a dismissed attention is hidden; same key stays hidden", async () => {
    const {
      attentionDismissKey,
      dismissAttention,
      getDismissedAttention,
    } = await import("./dismissedAttention");

    const live = session({
      agentId: "coder-1",
      needsUser: {
        reason: "question",
        message: "Pick one",
        since: "2026-06-04T10:00:00.000Z",
      },
    });
    const key = attentionDismissKey(live);
    dismissAttention(key);

    const dismissed = getDismissedAttention();
    const visible = [live].filter((s) => !dismissed.has(attentionDismissKey(s)));
    expect(visible).toEqual([]);
  });

  it("a genuinely-new attention from the same agent resurfaces (new since)", async () => {
    const {
      attentionDismissKey,
      dismissAttention,
      getDismissedAttention,
    } = await import("./dismissedAttention");

    const first = session({
      agentId: "coder-1",
      needsUser: {
        reason: "question",
        message: "Pick one",
        since: "2026-06-04T10:00:00.000Z",
      },
    });
    dismissAttention(attentionDismissKey(first));

    const second = session({
      agentId: "coder-1",
      needsUser: {
        reason: "question",
        message: "New question?",
        since: "2026-06-04T11:00:00.000Z",
      },
    });
    const dismissed = getDismissedAttention();
    const visible = [second].filter(
      (s) => !dismissed.has(attentionDismissKey(s)),
    );
    expect(visible.map((s) => s.agentId)).toEqual(["coder-1"]);
    expect(attentionDismissKey(first)).not.toBe(attentionDismissKey(second));
  });

  it("Clear all (clearAttentions of all visible keys) hides everything", async () => {
    const {
      attentionDismissKey,
      clearAttentions,
      getDismissedAttention,
    } = await import("./dismissedAttention");

    const items = [
      session({
        agentId: "a1",
        needsUser: {
          reason: "question",
          message: "Q1",
          since: "2026-06-04T10:00:00.000Z",
        },
      }),
      session({
        agentId: "a2",
        lastSeenAt: "2026-06-04T08:00:00.000Z",
        needsUser: null,
      }),
    ];
    const keys = items.map(attentionDismissKey);
    clearAttentions(keys);

    const dismissed = getDismissedAttention();
    const visible = items.filter((s) => !dismissed.has(attentionDismissKey(s)));
    expect(visible).toEqual([]);
    expect(dismissed.size).toBe(2);
  });
});
