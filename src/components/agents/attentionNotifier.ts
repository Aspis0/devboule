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
export function shouldNotify(
  session: AgentSession,
  deps: ShouldNotifyDeps,
): boolean {
  const needs = session.needsUser;
  const since = needs?.since?.trim();
  if (!needs || !since) return false;

  const recorded = deps.prevSinceByAgent.get(session.agentId);
  if (recorded === since) return false;

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
    return false;
  }

  deps.prevSinceByAgent.set(session.agentId, since);
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
    for (const session of sessions) {
      if (isCancelled()) return;
      plugin.sendNotification({
        title: "Agent needs you",
        body: buildNotificationBody(session),
      });
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

export function startAttentionWatcher(
  store: AgentAttentionStore,
  deps?: Partial<ShouldNotifyDeps>,
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
  // Flipped by teardown: an in-flight notifyAgentsNeedYou (whose OS permission
  // prompt may still be pending) checks this before sending so a toast cannot
  // fire after the watcher is gone (e.g. the app locked mid-prompt).
  let cancelled = false;

  const handle = (sessions: AgentSession[]): void => {
    const attention = attentionSessions(sessions, now());
    // Notify only for sessions actually flagged needsUser (stale/lost ring the
    // in-app bell but are NOT an OS toast — they're a recovery hint, not a
    // human-blocking question).
    const needsUser = attention.filter((session) => session.needsUser);

    const toFire: AgentSession[] = [];
    for (const session of needsUser) {
      if (shouldNotify(session, { prevSinceByAgent, recentFiresMs, now })) {
        toFire.push(session);
      }
    }

    // Leave-tracking: drop recorded `since` for any agent that is no longer
    // flagged needsUser, so a future re-raise (even with the SAME since string)
    // notifies again. Without this, an agent that resolves then re-blocks under
    // an unchanged timestamp would be silently suppressed.
    const stillNeeds = new Set(needsUser.map((s) => s.agentId));
    for (const agentId of [...prevSinceByAgent.keys()]) {
      if (!stillNeeds.has(agentId)) prevSinceByAgent.delete(agentId);
    }

    if (toFire.length > 0) void notifyAgentsNeedYou(toFire, () => cancelled);
  };

  // Fire once for the current snapshot, then on every change.
  handle(store.getState().sessions);
  const unsubscribe = store.subscribe((state) => handle(state.sessions));
  // Teardown clears the singleton guard so a later unmount→remount (e.g. lock
  // then unlock) can start a fresh watcher.
  return () => {
    cancelled = true;
    unsubscribe();
    watcherActive = false;
  };
}
