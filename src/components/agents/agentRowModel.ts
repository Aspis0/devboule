// Pure, DOM-free derivations for the rebuilt Agents fleet view.
//
// AgentRow.tsx, FleetSummary.tsx, AgentDetailDrawer.tsx and SpawnPanel.tsx are
// kept thin: every display string / branch decision that does NOT need the DOM
// lives here so it can be unit-tested in node (this repo has no jsdom). The .tsx
// files only map these structs to JSX.
//
// Health vocabulary, thresholds and CLI badges are NOT redefined here — they are
// imported from the single source of truth, ../projects/agentLiveStatus, so the
// Projects panel and the Agents room can never drift.

import type {
  AgentClaim,
  AgentEvent,
  AgentRole,
  AgentSession,
  AgentSubagent,
  ProjectTask,
} from "../../types/backend";
import type { SpawnRole } from "./roleDisplay";
import {
  cliBadge,
  formatHeartbeatAge,
  needsRecovery,
  parseTimestamp,
  sessionAgeMs,
  sessionHealth,
  type CliBadge,
  type SessionHealth,
} from "../projects/agentLiveStatus";
import { isOpenClaim, isWorkingClaim } from "../../utils/agentClaims";

const MODEL_UNKNOWN_LABEL = "model unknown";

// --- shared formatters -------------------------------------------------------

// Absolute, locale-aware short timestamp (e.g. "Jun 04, 10:42"). "never" for a
// missing value; falls back to a raw slice for an unparseable string so a
// malformed timestamp never crashes a row.
export function formatStamp(value: string | null | undefined): string {
  if (!value) return "never";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value.slice(0, 19);
  return date.toLocaleString(undefined, {
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

// --- AgentRow badge model ----------------------------------------------------

// One subagent chip's display text, e.g. "+6 sonnet reviewer". Naive trailing-s
// pluralization mirrors summarizeFleet so the two surfaces read the same. The
// role falls back to the empty string when absent so the chip degrades to
// "+6 sonnet".
export function subagentChipLabel(entry: AgentSubagent): string {
  const model = entry.model && entry.model.length > 0 ? entry.model : "unknown";
  const role = entry.role && entry.role.length > 0 ? ` ${entry.role}` : "";
  return `+${entry.count} ${model}${role}`;
}

export interface RowBadges {
  health: SessionHealth;
  // Coarse heartbeat-age age string for the row, e.g. "12s ago" / "unknown".
  ageLabel: string;
  // Whether this session needs a recovery action (stale/lost/unknown heartbeat).
  recovery: boolean;
  // Model label: the reported model, or a muted "model unknown" sentinel.
  modelLabel: string;
  modelKnown: boolean;
  cli: CliBadge;
  // Subagent chips (already pluralized); empty when the session reported none.
  subagentChips: string[];
  // Total subagents headcount (sum of counts) — used for an at-a-glance number.
  subagentTotal: number;
  // needsUser message preview (single line) when the agent is blocked on a human.
  needsUserMessage: string | null;
}

// Everything AgentRow needs to render its badges/affordances, derived purely.
export function rowBadges(
  session: AgentSession,
  now: number = Date.now(),
): RowBadges {
  const health = sessionHealth(session, now);
  const subagents = session.subagents ?? [];
  const needsUserMessage =
    session.needsUser && session.needsUser.message.trim().length > 0
      ? session.needsUser.message.trim()
      : session.needsUser
        ? session.needsUser.reason
        : null;
  return {
    health,
    ageLabel: formatHeartbeatAge(sessionAgeMs(session, now)),
    recovery: needsRecovery(health),
    modelLabel:
      session.model && session.model.length > 0
        ? session.model
        : MODEL_UNKNOWN_LABEL,
    modelKnown: Boolean(session.model && session.model.length > 0),
    cli: cliBadge(session.client),
    subagentChips: subagents.map(subagentChipLabel),
    subagentTotal: subagents.reduce((sum, entry) => sum + entry.count, 0),
    needsUserMessage,
  };
}

// --- AgentRow action gating --------------------------------------------------

// Which action affordances a row should expose, derived from the session's
// terminal host (read-time stamp) and whether it currently has a live app PTY.
//
// Rules:
//  - Terminal toggle: only when the agent has a live in-app PTY (hasPty).
//  - Open CLI: only meaningful for a NON-app host (external console). It is a
//    silent no-op for an app-hosted agent, so it is hidden whenever host==="app".
//  - When an agent was launched in-app (host==="app") but its PTY is gone, neither
//    button applies; instead show a muted "Terminal exited" hint so the row is not
//    left with dead/clickable-but-useless controls.
//
// A session with host == null/undefined was not launched by the app (no ledger
// entry): Open CLI stays available (legacy/external behavior) and no exited hint
// is shown.
export interface RowActions {
  showTerminalToggle: boolean;
  showOpenCli: boolean;
  // True only for an app-hosted agent whose PTY has exited: show a relaunch hint
  // chip instead of dead Terminal/Open-CLI buttons.
  showExitedHint: boolean;
}

export function rowActions(session: AgentSession, hasPty: boolean): RowActions {
  const isApp = session.host === "app";
  return {
    showTerminalToggle: hasPty,
    // Open CLI focuses an external console; hide it for app-hosted agents.
    showOpenCli: !isApp,
    // App-hosted but no live PTY -> exited; surface a relaunch hint.
    showExitedHint: isApp && !hasPty,
  };
}

// --- FleetSummary health roll-up ---------------------------------------------

export interface FleetHealthRollup {
  online: number; // online + pending (booting agents count as online-ish)
  stale: number; // stale + unknown heartbeat
  lost: number; // lost heartbeat
}

// Compact health counts over the live sessions for the top strip. Closed sessions
// (done/archived/stopped/idle/closed) are NOT counted — they are not part of the
// running fleet. Mirrors agentFleet.fleetCounts' "closed is excluded" rule.
export function fleetHealthRollup(
  sessions: AgentSession[],
  now: number = Date.now(),
): FleetHealthRollup {
  const roll: FleetHealthRollup = { online: 0, stale: 0, lost: 0 };
  for (const session of sessions) {
    const h = sessionHealth(session, now);
    if (h === "online" || h === "pending") roll.online += 1;
    else if (h === "stale" || h === "unknown") roll.stale += 1;
    else if (h === "lost") roll.lost += 1;
    // "closed" falls through: not part of the running fleet.
  }
  return roll;
}

// --- History cap -------------------------------------------------------------

export interface CappedHistory {
  // The most-recent `limit` closed sessions, newest lastSeenAt first.
  sessions: AgentSession[];
  // Total number of closed sessions before capping (for the "showing X of N" line).
  total: number;
  // True when `total > limit` and the list was truncated.
  truncated: boolean;
}

// Bound the rendered History list. The closed/history set is unbounded — a
// long-lived control room can accumulate hundreds of done/stopped sessions, and
// rendering them all churns the DOM and the row ref map for no benefit. Keep only
// the most recent `limit` by lastSeenAt (descending; a missing lastSeenAt sorts
// last), and report the pre-cap total so the UI can show "showing N of M".
//
// Pure and total: never mutates the input array, tolerates an empty list, and a
// non-positive limit yields an empty list (still reporting the real total).
export function capHistory(
  sessions: AgentSession[],
  limit = 20,
): CappedHistory {
  const total = sessions.length;
  if (limit <= 0) {
    return { sessions: [], total, truncated: total > 0 };
  }
  const sorted = [...sessions].sort((a, b) => {
    // Newest lastSeenAt first; null/unparseable timestamps sort to the end.
    const ta = parseTimestamp(a.lastSeenAt) ?? -Infinity;
    const tb = parseTimestamp(b.lastSeenAt) ?? -Infinity;
    return tb - ta;
  });
  return {
    sessions: sorted.slice(0, limit),
    total,
    truncated: total > limit,
  };
}

// --- AgentDetailDrawer model -------------------------------------------------

export interface DrawerData {
  // This agent's claims, split for display. Working = currently held; open =
  // claimed/awaiting; history = closed/expired. All newest-updated first.
  activeClaims: AgentClaim[];
  waitingClaims: AgentClaim[];
  historyClaims: AgentClaim[];
  // This agent's recent events, newest first, capped.
  events: AgentEvent[];
  // Subagent breakdown rows (copied through; the drawer renders the raw entries).
  subagents: AgentSubagent[];
}

// Filter the GLOBAL claim/event arrays down to one agent and shape them for the
// drawer. Sorting is null-safe (a missing updatedAt/timestamp sorts last) so a
// malformed record never throws inside this selector.
export function drawerData(
  session: AgentSession,
  claims: AgentClaim[],
  events: AgentEvent[],
  now: number = Date.now(),
  eventLimit = 12,
): DrawerData {
  const agentId = session.agentId;
  const mine = claims
    .filter((claim) => claim.agentId === agentId)
    .sort((a, b) => (b.updatedAt ?? "").localeCompare(a.updatedAt ?? ""));
  return {
    activeClaims: mine.filter((claim) => isWorkingClaim(claim, now)),
    waitingClaims: mine.filter(
      (claim) => isOpenClaim(claim, now) && !isWorkingClaim(claim, now),
    ),
    historyClaims: mine.filter((claim) => !isOpenClaim(claim, now)),
    events: events
      .filter((event) => event.agentId === agentId)
      .sort((a, b) => (b.timestamp ?? "").localeCompare(a.timestamp ?? ""))
      .slice(0, eventLimit),
    subagents: session.subagents ?? [],
  };
}

// --- SpawnPanel launch builder ----------------------------------------------

export type SpawnHost = "app" | "external" | "copy";

export interface SpawnSelection {
  projectId: string;
  role: SpawnRole;
  // Advisory model hint threaded into the launch prompt. "" / "custom-with-no-
  // value" means "let the agent self-report" and no hint is sent.
  model: string;
  taskId: string;
  // Built-in CLIs ("codex" | "claude" | "powershell" | "orchestrator") or a
  // user-configured custom agent client id (validated [a-z0-9-]{1,32}).
  // "orchestrator" (L2.4) selects the LOCAL Devboule main coder (oMLX); the
  // backend dispatches its own binary instead of an external CLI. Kept as a string
  // so a custom client id threads through the launch pipeline without a type
  // widening at every call site; the Rust boundary re-validates it against
  // config.json (normalize_agent_client).
  client: string;
  // 3b — "Plan first" bias for the LOCAL orchestrator (client === "orchestrator")
  // ONLY. When true the launch sets DEVBOULE_PLAN_FIRST=1 so the orchestrator's
  // system prompt gains a plan-before-acting directive. Meaningless for
  // codex/claude (the toggle is not shown for them); SpawnPanel forces it to false
  // for any non-orchestrator client so the launch input never carries a stale flag.
  // Optional so existing callers (e.g. the Censor final-review path) type-check
  // without setting it; absent is treated as off everywhere downstream.
  planFirst?: boolean;
  // Phase 6 — per-launch LANGUAGE-persona override (the panel's language selector). "" / absent
  // ⇒ the backend auto-detects the project's primary language for the (role × language) persona;
  // a non-empty value (rust/node/python/go/cpp/kotlin) forces that persona instead.
  languageOverride?: string;
}

// The exact ProjectAgentLaunchInput sent over IPC, built from a selection plus a
// host. Mirrors src/types/backend.ts ProjectAgentLaunchInput (camelCase). The
// caller appends agentId; this builder owns the host + model threading only.
export interface SpawnLaunchInput {
  projectId: string;
  // ROLE UNTANGLE: the launch role is the WIDER AgentRole (not just the panel's
  // SpawnRole) — planner launches pass role:"orchestrator" so the ledger stores
  // the truth for local AND cloud-duplex planners alike. The panel still only
  // builds coder/verifier selections.
  role: AgentRole;
  // See SpawnSelection.client: built-in CLI id or a custom agent client id.
  client: string;
  taskId: string | null;
  host: "app" | "external";
  model: string | null;
  // Phase H: true only for the Censor "Run final review" launch (a verifier
  // that should receive the residual-adjudication addendum). Optional; a normal
  // SpawnPanel launch leaves it undefined → the backend treats absent as false
  // and the verifier prompt is unchanged. Threaded to ProjectAgentLaunchInput
  // .censorReview over IPC.
  censorReview?: boolean;
  // 3b — true only for a LOCAL orchestrator launch (client === "orchestrator")
  // with the "Plan first" toggle ON. Threaded to ProjectAgentLaunchInput.planFirst
  // over IPC; the Rust launch wiring turns it into DEVBOULE_PLAN_FIRST=1 for the
  // orchestrator binary (and omits it when false/absent so a non-plan-first launch
  // is byte-identical). Always absent for codex/claude — the toggle is not shown.
  planFirst?: boolean;
  // Phase 6 — the per-launch language-persona override, threaded to ProjectAgentLaunchInput
  // .languageOverride over IPC. Absent ⇒ the backend auto-detects; a non-empty value forces that
  // language's persona. Backend-agnostic (applies on whatever backend the role runs on).
  languageOverride?: string;
  // Orchestrator composer "Plan it": the typed GOAL. Threaded to ProjectAgentLaunchInput.initialGoal
  // → DEVBOULE_GOAL, so the orchestrator runs headless on it (plan-first) instead of waiting for TUI
  // input. Absent ⇒ the interactive launch, unchanged.
  initialGoal?: string;
  // Orchestrator composer auto-create toggle. `false` ⇒ DEVBOULE_AUTO_CREATE=0 (the planner submits
  // the plan but doesn't create its tasks on approval). Absent/`true` ⇒ the default (create on approval).
  autoCreate?: boolean;
  // Phase D — true only when launching a CLOUD orchestrator (claude/codex) AS the planner, so the
  // backend runs it as a piped duplex child whose events feed the Stage (instead of a terminal).
  // Threaded to ProjectAgentLaunchInput.cloudDuplex over IPC; absent ⇒ the existing launch.
  cloudDuplex?: boolean;
}

// Max length of an advisory model hint that rides into the prompt. A model name
// is short; anything longer is almost certainly a paste accident, so cap it
// rather than thread an unbounded string into the launch prompt.
export const MODEL_HINT_MAX_LENGTH = 64;

// Normalize the advisory model hint: trimmed, lowercased; "" yields null (no
// hint). The model field is now a free-text input (the chip set was replaced by
// per-client suggestions), so the UI no longer emits the "custom" literal — but
// "custom" is still mapped to null defensively in case an old caller/value sends
// it. Anything else rides along verbatim (capped) so an arbitrary self-hosted
// model name is preserved.
export function normalizeModelHint(model: string): string | null {
  const trimmed = model.trim().toLowerCase();
  if (trimmed === "" || trimmed === "custom") return null;
  return trimmed.slice(0, MODEL_HINT_MAX_LENGTH);
}

// Quick-fill model suggestions for the Spawn panel's free-text model input,
// scoped to the selected CLI. Only the Claude CLI has a meaningful, stable set of
// model names we can pre-fill (opus/sonnet/haiku); for codex, powershell and any
// user-configured custom client we deliberately invent NOTHING — the operator
// types the model name themselves (or leaves it blank to self-report). Pure so
// SpawnPanel can both render the chips and detect "did the user pick a suggestion"
// when resetting on a client switch.
//
// L2 — the local Devboule orchestrator ("orchestrator") is a special case: its model
// is NOT free-typed, it is the one configured in Settings → Local main coder. When that
// configured model is known (passed by SpawnPanel from config.localCoderBackend), offer
// it as the single quick-fill suggestion so the launcher is NOT empty and the user can
// click it. Absent config => no suggestion (the orchestrator note then tells the user to
// configure it in Settings).
export function modelSuggestionsForClient(
  client: string,
  localCoderModel?: string | null,
): string[] {
  if (client === "claude") return ["opus", "sonnet", "haiku"];
  if (client === "orchestrator") {
    const trimmed = (localCoderModel ?? "").trim();
    return trimmed.length > 0 ? [trimmed] : [];
  }
  return [];
}

// The note SpawnPanel shows under the CLI selector when the local Devboule orchestrator
// is selected. Surfaces the configured local-main-coder model so the launcher always
// communicates which model will run (it is NOT the free-text advisory field — the binary
// reads it from config), and points the user at Settings to change it. Pure + total.
export function orchestratorModelNote(localCoderModel?: string | null): string {
  const trimmed = (localCoderModel ?? "").trim();
  if (trimmed.length > 0) {
    return `Runs Devboule's own local coder using "${trimmed}" (set in Settings → Local main coder).`;
  }
  return "Runs Devboule's own local coder — no model configured yet. Set one in Settings → Local main coder (until then it falls back to a safe local stub).";
}

// Build the IPC launch input for the in-app PTY ("app") or external console
// ("external") path. The "copy" host is NOT a launch and must be handled by the
// caller (prepare_project_agent_prompt) — passing it here throws so a wiring
// mistake is loud.
export function buildLaunchInput(
  selection: SpawnSelection,
  host: SpawnHost,
): SpawnLaunchInput {
  if (host === "copy") {
    throw new Error(
      "buildLaunchInput is for app/external launches only; use the prompt-copy path for host=copy.",
    );
  }
  return {
    projectId: selection.projectId,
    role: selection.role,
    client: selection.client,
    taskId: selection.taskId.trim().length > 0 ? selection.taskId : null,
    host,
    model: normalizeModelHint(selection.model),
    // 3b — "Plan first" is a LOCAL-orchestrator-only bias. Gate it on the client here so a
    // stale flag can never ride a codex/claude launch. A-F1: for the orchestrator emit the
    // EXPLICIT boolean (true OR false) — Rust defaults an ABSENT value to plan-first, so
    // sending `undefined` when the toggle is OFF let that default silently override the
    // user's choice. Non-orchestrator clients carry no plan-first flag (undefined).
    planFirst:
      selection.client === "orchestrator"
        ? selection.planFirst === true
        : undefined,
    // Phase 6 — forward the language-persona override only when the user actually picked one
    // (non-empty). Absent ⇒ the backend auto-detects; applies on any backend the role runs on.
    languageOverride:
      selection.languageOverride && selection.languageOverride.trim().length > 0
        ? selection.languageOverride.trim()
        : undefined,
  };
}

// Whether a given (role, task) launch is allowed — same rules the old AgentsView
// enforced. Pure so SpawnPanel can disable buttons and explain why.
export function canRoleLaunchTask(
  role: SpawnRole,
  task: ProjectTask | null,
): boolean {
  if (!task) return true;
  if (task.status === "done") return false;
  if (role === "verifier")
    return task.status === "review" || task.status === "blocked";
  // coder (the only other spawn role): todo / wip / blocked.
  return (
    task.status === "todo" || task.status === "wip" || task.status === "blocked"
  );
}

// The reason a spawn is blocked, or null when it is allowed. Order of precedence:
// no project selected -> project not active -> role/task rule. Returned as a
// human string so the panel can show it as a disabled-button title + inline note.
export function spawnDisabledReason(args: {
  projectId: string;
  projectActive: boolean | null;
  role: SpawnRole;
  task: ProjectTask | null;
}): string | null {
  if (!args.projectId || args.projectId === "all") {
    return "Select a project before launching an agent.";
  }
  if (args.projectActive === null) {
    // A concrete project is selected but its detail has not loaded yet (active
    // flag unknown). Disable rather than optimistically allowing a launch that
    // the backend might reject for an inactive project.
    return "Project loading…";
  }
  if (args.projectActive === false) {
    return "Only active projects can launch agents.";
  }
  if (!canRoleLaunchTask(args.role, args.task)) {
    // canRoleLaunchTask only ever returns false for a non-null task (a null task
    // is always launchable), so args.task is guaranteed non-null here.
    if (args.role === "coder")
      return "Coder can launch only on Todo, WIP or Blocked tasks.";
    if (args.role === "verifier")
      return "Verifier can launch only on Review or Blocked tasks.";
    return "Done tasks are verifier-locked.";
  }
  return null;
}
