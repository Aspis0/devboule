// OS-notification logic for "an agent needs you" (Phase 5).
//
// Two layers, kept separate so the decision logic is pure and unit-testable and
// only the thin wrapper touches the Tauri plugin:
//   1. shouldNotify(...)        — pure predicate: fire ONLY on a needsUser.since
//      transition (enter or re-raise with a new `since`), never on repeat ticks,
//      with a per-minute global cap. Clock is injectable.
//   2. notifyAgentsNeedYou(...) — dynamic-imports @tauri-apps/plugin-notification,
//      requests OS permission on first use, sends one notification per session.
//      Denied/unsupported/error -> silent no-op (the in-app Header pill remains).
//   3. startAttentionWatcher(store) — app-level glue: subscribes to the
//      agentAttentionStore, computes attention via attentionSessions (REUSED, not
//      duplicated), and drives shouldNotify + notifyAgentsNeedYou.
//
// PRIVACY: the notification body is the AGENT'S OWN message (already control-char
// stripped server-side by clean_text). We still cap it client-side and never add
// file paths or token-like strings — only `<agentId>: <message>`.

import type { AgentSession } from "../../types/backend";
import { attentionSessions } from "./agentFleet";
import type { AgentAttentionStore } from "../../store/agentAttentionStore";

/** Hard cap on the notification body so a long agent message can't spam the OS
 *  toast. The server already strips control chars; this only bounds length. */
export const NOTIFICATION_BODY_MAX = 140;

/** Per-minute cap on OS notifications, so a fleet that all re-raises at once (or
 *  a flapping agent) cannot flood the OS notification center. */
export const NOTIFICATION_PER_MINUTE_CAP = 5;
const ONE_MINUTE_MS = 60_000;

export interface ShouldNotifyDeps {
  /** Last needsUser.since we notified for, per agentId. MUTATED by shouldNotify
   *  on a positive decision so the next repeat tick is suppressed. Callers that
   *  also want leave-tracking should prune this map when an agent stops needing
   *  attention (see startAttentionWatcher). */
  prevSinceByAgent: Map<string, string>;
  /** Monotonic-ish timestamps (ms) of recent fired notifications, for the
   *  per-minute cap. MUTATED by shouldNotify (entries older than 60s pruned, a
   *  new entry pushed on a positive decision). */
  recentFiresMs: number[];
  /** Injectable clock for deterministic tests. */
  now: () => number;
}

export type NotificationDecision = "fire" | "inactive" | "duplicate" | "capped";

/**
 * Decide whether to fire an OS notification for `session`.
 *
 * Returns true ONLY when:
 *   - the session is actually flagged needsUser (with a non-blank `since`), AND
 *   - that `since` differs from the last one we notified for this agent (a fresh
 *     ENTER, or a RE-RAISE with a new transition timestamp), AND
 *   - we are under the per-minute cap.
 *
 * On a positive decision it records the new `since` and the fire timestamp so the
 * next identical tick returns false. A session WITHOUT needsUser returns false
 * but does NOT clear the recorded `since` here — leave-tracking (pruning departed
 * agents) is the watcher's job, since shouldNotify only sees one session at a time.
 */
export function notificationDecision(
  session: AgentSession,
  deps: ShouldNotifyDeps,
): NotificationDecision {
  const needs = session.needsUser;
  const since = needs?.since?.trim();
  if (!needs || !since) return "inactive";

  const recorded = deps.prevSinceByAgent.get(session.agentId);
  if (recorded === since) return "duplicate";

  const nowMs = deps.now();
  // Prune fire timestamps older than the rolling 60s window, then enforce the cap.
  const cutoff = nowMs - ONE_MINUTE_MS;
  // In-place filter to keep the same array reference the caller owns.
  for (let i = deps.recentFiresMs.length - 1; i >= 0; i -= 1) {
    if (deps.recentFiresMs[i] < cutoff) deps.recentFiresMs.splice(i, 1);
  }
  if (deps.recentFiresMs.length >= NOTIFICATION_PER_MINUTE_CAP) {
    // Capped: do NOT record the since, so once the window frees up a still-open
    // (unchanged-since) request can finally fire instead of being lost forever.
    return "capped";
  }

  deps.prevSinceByAgent.set(session.agentId, since);
  deps.recentFiresMs.push(nowMs);
  return "fire";
}

export function shouldNotify(
  session: AgentSession,
  deps: ShouldNotifyDeps,
): boolean {
  return notificationDecision(session, deps) === "fire";
}

export function msUntilNotificationWindowFrees(
  recentFiresMs: number[],
  nowMs: number,
): number {
  const oldest = Math.min(...recentFiresMs);
  if (!Number.isFinite(oldest)) return 0;
  return Math.max(0, oldest + ONE_MINUTE_MS - nowMs + 1);
}

export function reserveNotificationSlot(deps: ShouldNotifyDeps): boolean {
  const nowMs = deps.now();
  const cutoff = nowMs - ONE_MINUTE_MS;
  for (let i = deps.recentFiresMs.length - 1; i >= 0; i -= 1) {
    if (deps.recentFiresMs[i] < cutoff) deps.recentFiresMs.splice(i, 1);
  }
  if (deps.recentFiresMs.length >= NOTIFICATION_PER_MINUTE_CAP) return false;
  deps.recentFiresMs.push(nowMs);
  return true;
}

/**
 * Strip bidi/zero-width spoofing code points from untrusted agent-supplied text.
 *
 * A malicious agentId or needsUser.message could embed RTL overrides (Trojan-
 * Source style) to visually reorder text, or zero-width chars to hide content.
 * The server already strips control chars in clean_text, but the client must
 * defend independently — this is the last gate before the string reaches the OS
 * toast and the Header attention list. Removes:
 *   - bidi marks & embeddings/overrides  U+200E,200F, U+202A–U+202E
 *   - bidi isolates                       U+2066–U+2069
 *   - zero-width space/joiners            U+200B–U+200D
 *   - byte-order mark / zero-width nbsp   U+FEFF
 */
const SPOOF_CHARS_RE = /[​-‏‪-‮⁦-⁩﻿]/g;

export function stripSpoofChars(value: string | null | undefined): string {
  if (!value) return "";
  return value.replace(SPOOF_CHARS_RE, "");
}

/** Build the privacy-safe notification body: "<agentId>: <message>", capped.
 *  No file paths, no tokens — only the agent's own (server-cleaned) message,
 *  with bidi/zero-width spoofing chars stripped client-side. */
export function buildNotificationBody(session: AgentSession): string {
  const agentId = stripSpoofChars(session.agentId);
  const message = stripSpoofChars(session.needsUser?.message).trim();
  const base = message ? `${agentId}: ${message}` : agentId;
  return base.length > NOTIFICATION_BODY_MAX
    ? `${base.slice(0, NOTIFICATION_BODY_MAX - 1)}…`
    : base;
}

export function buildSummaryNotificationBody(count: number): string {
  const safeCount = Math.max(1, Math.floor(count));
  return `${safeCount} agents need you`;
}

export function isTerminalOutcomeSession(session: AgentSession): boolean {
  // Only app-hosted PTY agents get an OS "finished" toast: their terminal lives inside the
  // app and the user cannot see it end. CLI-hosted agents run in their own terminal window
  // where the outcome is already visible, so a toast would be redundant noise.
  if (session.host !== "app") return false;
  return isTerminalOutcomeStatus(session.status);
}

export function isTerminalOutcomeStatus(status: string | null | undefined): boolean {
  const normalized = (status ?? "").toLowerCase();
  return normalized === "done" || normalized === "failed" || normalized === "timeout";
}

export function buildOutcomeNotificationBody(session: AgentSession): string {
  const agentId = stripSpoofChars(session.agentId);
  const status = stripSpoofChars(session.status).toLowerCase();
  const label = status === "done" ? "done" : "failed";
  return `${agentId}: ${label}`;
}

/**
 * Thin Tauri-plugin wrapper: send ONE OS notification per session in `sessions`.
 *
 * Lazily imports the plugin (so the chunk is not pulled into the initial bundle),
 * checks/requests OS permission once, and on any failure (denied, unsupported,
 * not in Tauri, thrown error) becomes a silent no-op — the in-app Header pill is
 * the always-available fallback. Errors are swallowed with a console.warn.
 *
 * `isCancelled` is checked AFTER the (possibly long-pending) permission prompt
 * resolves and before each send: the OS permission request can outlive the
 * watcher (e.g. the app locks while the prompt is open), and without this guard
 * a stale toast would fire after teardown. Defaults to never-cancelled.
 */
export async function notifyAgentsNeedYou(
  sessions: AgentSession[],
  isCancelled: () => boolean = () => false,
): Promise<void> {
  if (sessions.length === 0) return;
  await notifyPlain(
    sessions.map((session) => ({
      title: "Agent needs you",
      body: buildNotificationBody(session),
    })),
    isCancelled,
  );
}

export async function notifyAgentsSummary(
  count: number,
  isCancelled: () => boolean = () => false,
): Promise<void> {
  if (count <= 0) return;
  await notifyPlain(
    [{ title: "Agents need you", body: buildSummaryNotificationBody(count) }],
    isCancelled,
  );
}

export async function notifyAgentOutcomes(
  sessions: AgentSession[],
  isCancelled: () => boolean = () => false,
): Promise<void> {
  if (sessions.length === 0) return;
  await notifyPlain(
    sessions.map((session) => ({
      title: "Agent finished",
      body: buildOutcomeNotificationBody(session),
    })),
    isCancelled,
  );
}

async function notifyPlain(
  notifications: { title: string; body: string }[],
  isCancelled: () => boolean,
): Promise<void> {
  if (notifications.length === 0) return;
  try {
    const plugin = await import("@tauri-apps/plugin-notification");
    let granted = await plugin.isPermissionGranted();
    if (!granted) {
      const permission = await plugin.requestPermission();
      granted = permission === "granted";
    }
    if (!granted) return; // Denied/unsupported -> in-app pill only.
    // The permission prompt may have resolved long after the watcher was torn
    // down; bail before emitting any stale toast.
    if (isCancelled()) return;
    for (const notification of notifications) {
      if (isCancelled()) return;
      plugin.sendNotification(notification);
    }
  } catch (error) {
    // Plugin missing, not in Tauri, or OS notification subsystem failed: the
    // in-app pill still surfaces the request, so this is non-fatal.
    console.warn("OS notification skipped:", error);
  }
}

/**
 * App-level glue, mounted ONCE (see App.tsx). Subscribes to the attention store
 * and, on every store update, computes the sessions needing the human (REUSING
 * attentionSessions — the single attention predicate), runs shouldNotify per
 * session (dedup on since + per-minute cap), prunes tracking for agents that no
 * longer need attention, and fires the surviving OS notifications.
 *
 * Returns an unsubscribe function for symmetric teardown.
 */
// Module-level singleton guard. React StrictMode double-invokes effects in dev,
// so App.tsx's startAttentionWatcher effect runs twice on mount; without this a
// second live subscription fires DOUBLE OS notifications. While a watcher is
// active, a second start returns a no-op teardown and creates no subscription.
// The flag is reset by the active watcher's own teardown so a real unmount/lock
// cycle can cleanly restart it.
let watcherActive = false;

export interface AttentionWatcherDeps extends Partial<ShouldNotifyDeps> {
  loadPrevSinceByAgent?: () => Promise<Record<string, string>>;
  savePrevSinceByAgent?: (value: Record<string, string>) => Promise<void>;
  notifyNeeds?: typeof notifyAgentsNeedYou;
  notifySummary?: typeof notifyAgentsSummary;
  notifyOutcomes?: typeof notifyAgentOutcomes;
  setTimeoutFn?: typeof setTimeout;
  clearTimeoutFn?: typeof clearTimeout;
}

export function startAttentionWatcher(
  store: AgentAttentionStore,
  deps?: AttentionWatcherDeps,
): () => void {
  if (watcherActive) {
    // Already watching (e.g. StrictMode's second invocation): do nothing and
    // return a no-op so the duplicate effect's cleanup cannot touch the real one.
    return () => {};
  }
  watcherActive = true;
  const prevSinceByAgent = deps?.prevSinceByAgent ?? new Map<string, string>();
  const recentFiresMs = deps?.recentFiresMs ?? [];
  const now = deps?.now ?? (() => Date.now());
  const setTimeoutFn = deps?.setTimeoutFn ?? setTimeout;
  const clearTimeoutFn = deps?.clearTimeoutFn ?? clearTimeout;
  const loadPrevSinceByAgent =
    deps?.loadPrevSinceByAgent ?? defaultLoadPrevSinceByAgent;
  const savePrevSinceByAgent =
    deps?.savePrevSinceByAgent ?? defaultSavePrevSinceByAgent;
  const notifyNeeds = deps?.notifyNeeds ?? notifyAgentsNeedYou;
  const notifySummary = deps?.notifySummary ?? notifyAgentsSummary;
  const notifyOutcomes = deps?.notifyOutcomes ?? notifyAgentOutcomes;
  // Flipped by teardown: an in-flight notifyAgentsNeedYou (whose OS permission
  // prompt may still be pending) checks this before sending so a toast cannot
  // fire after the watcher is gone (e.g. the app locked mid-prompt).
  let cancelled = false;
  let ready = false;
  let pendingSessions = store.getState().sessions;
  let persistTimer: ReturnType<typeof setTimeout> | null = null;
  let summaryTimer: ReturnType<typeof setTimeout> | null = null;
  const suppressedNeedsAgents = new Set<string>();
  const previousStatusByAgent = new Map<string, string>();

  const snapshotPrevSince = () => Object.fromEntries(prevSinceByAgent.entries());

  const schedulePersist = () => {
    if (cancelled) return;
    if (persistTimer) clearTimeoutFn(persistTimer);
    persistTimer = setTimeoutFn(() => {
      persistTimer = null;
      void savePrevSinceByAgent(snapshotPrevSince()).catch(() => {});
    }, 300);
  };

  const scheduleSummary = () => {
    if (summaryTimer || suppressedNeedsAgents.size === 0 || cancelled) return;
    const delay = msUntilNotificationWindowFrees(recentFiresMs, now());
    summaryTimer = setTimeoutFn(() => {
      summaryTimer = null;
      if (cancelled || suppressedNeedsAgents.size === 0) return;
      const count = suppressedNeedsAgents.size;
      if (!reserveNotificationSlot({ prevSinceByAgent, recentFiresMs, now })) {
        scheduleSummary();
        return;
      }
      suppressedNeedsAgents.clear();
      void notifySummary(count, () => cancelled);
    }, delay);
  };

  const handle = (sessions: AgentSession[]): void => {
    // A torn-down watcher does NOTHING: a late store emission (or a queued
    // microtask) must not mutate previousStatusByAgent / prevSinceByAgent or
    // consume a per-minute slot in the needs-you / outcome paths after teardown.
    if (cancelled) return;
    if (!ready) {
      pendingSessions = sessions;
      return;
    }
    const attention = attentionSessions(sessions, now());
    // Notify only for sessions actually flagged needsUser (stale/lost ring the
    // in-app bell but are NOT an OS toast — they're a recovery hint, not a
    // human-blocking question).
    const needsUser = attention.filter((session) => session.needsUser);

    const toFire: AgentSession[] = [];
    for (const session of needsUser) {
      const decision = notificationDecision(session, {
        prevSinceByAgent,
        recentFiresMs,
        now,
      });
      if (decision === "fire") {
        toFire.push(session);
        schedulePersist();
      } else if (decision === "capped") {
        suppressedNeedsAgents.add(session.agentId);
        scheduleSummary();
      }
    }

    // Leave-tracking: drop recorded `since` for any agent that is no longer
    // flagged needsUser, so a future re-raise (even with the SAME since string)
    // notifies again. Without this, an agent that resolves then re-blocks under
    // an unchanged timestamp would be silently suppressed.
    const stillNeeds = new Set(needsUser.map((s) => s.agentId));
    for (const agentId of [...prevSinceByAgent.keys()]) {
      if (!stillNeeds.has(agentId)) {
        prevSinceByAgent.delete(agentId);
        schedulePersist();
      }
    }

    if (toFire.length > 0) void notifyNeeds(toFire, () => cancelled);

    const outcomeToFire: AgentSession[] = [];
    for (const session of sessions) {
      const previous = previousStatusByAgent.get(session.agentId);
      if (
        previous !== undefined &&
        !isTerminalOutcomeStatus(previous) &&
        isTerminalOutcomeSession(session) &&
        // Final cancellation gate BEFORE consuming a rate-limit slot: the watcher may have
        // been torn down earlier in this same handle() pass (e.g. a needs-you notify path
        // flipped nothing, but a concurrent teardown set `cancelled`). reserveNotificationSlot
        // mutates recentFiresMs, so consuming a slot for a toast that will never fire would
        // waste a slot from the rolling per-minute budget. Skip the reservation if cancelled.
        !cancelled
      ) {
        if (reserveNotificationSlot({ prevSinceByAgent, recentFiresMs, now })) {
          outcomeToFire.push(session);
        }
      }
      previousStatusByAgent.set(session.agentId, session.status);
    }
    const liveIds = new Set(sessions.map((s) => s.agentId));
    for (const agentId of [...previousStatusByAgent.keys()]) {
      if (!liveIds.has(agentId)) previousStatusByAgent.delete(agentId);
    }
    if (outcomeToFire.length > 0) {
      void notifyOutcomes(outcomeToFire, () => cancelled);
    }
  };

  void loadPrevSinceByAgent()
    .then((record) => {
      if (cancelled) return;
      for (const [agentId, since] of Object.entries(record)) {
        if (typeof agentId === "string" && typeof since === "string" && since.trim()) {
          prevSinceByAgent.set(agentId, since);
        }
      }
    })
    .catch(() => {})
    .finally(() => {
      if (cancelled) return;
      ready = true;
      handle(pendingSessions);
    });

  // Fire once for the current snapshot, then on every change.
  const unsubscribe = store.subscribe((state) => handle(state.sessions));
  // Teardown clears the singleton guard so a later unmount→remount (e.g. lock
  // then unlock) can start a fresh watcher.
  return () => {
    cancelled = true;
    if (persistTimer) clearTimeoutFn(persistTimer);
    if (summaryTimer) clearTimeoutFn(summaryTimer);
    void savePrevSinceByAgent(snapshotPrevSince()).catch(() => {});
    unsubscribe();
    watcherActive = false;
  };
}

async function defaultLoadPrevSinceByAgent(): Promise<Record<string, string>> {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    const state = await invoke<{ prevSinceByAgent?: Record<string, string> }>(
      "read_agent_notification_state",
    );
    return state.prevSinceByAgent ?? {};
  } catch {
    return {};
  }
}

async function defaultSavePrevSinceByAgent(
  value: Record<string, string>,
): Promise<void> {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("write_agent_notification_state", {
      state: { prevSinceByAgent: value },
    });
  } catch {
    // In-browser dev and locked/unsupported runtimes keep the in-memory guard.
  }
}
