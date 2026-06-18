// Agent Activity Console — the framework-free data CONTRACT (Step A of the
// console feature). This is a structured timeline for the orchestration loop:
// the complement to Devboule's raw xterm terminal, surfaced as the "Console" tab
// in the Work-mode bottom dock.
//
// CONTRACT: this file is the single source of truth that the BACKEND (Step B)
// MUST match. The Rust `mini_activity_snapshot` command returns a `ConsoleActivity`
// (camelCase JSON), and the per-agent `mini-activity://<agentId>` event channel
// emits incremental updates whose applied result is again a `ConsoleActivity`
// (see `MiniActivityEvent` in useAgentConsole.ts). Keep the field names + shapes
// here in lockstep with the Rust serde structs; a drift breaks the wire silently.
//
// Framework-free + DOM-free on purpose: the types + the tiny pure helpers below are
// unit-testable in the repo's node vitest env without React. AgentConsole.tsx is a
// thin, prop-driven render of exactly these shapes.
//
// PRIVACY: every string here is already a redacted, human-readable summary the
// engine produced (verb labels, file targets, diff hunks, finding messages). This
// model defines no raw transcript / token / secret field — the component renders
// only what the backend chose to surface.

// ---- diff -------------------------------------------------------------------

/** One line of a unified-diff hunk shown under an expanded write action.
 *  `t` drives the row color: meta=muted header, add=sage, del=coral, ctx=neutral. */
export interface DiffLine {
  t: "meta" | "add" | "del" | "ctx";
  /** The line text WITHOUT the leading +/-/space sigil (the view renders the sigil). */
  s: string;
}

// ---- action -----------------------------------------------------------------

/** What an action did, for icon mapping. Anything unknown renders the `run` glyph. */
export type ActionKind = "read" | "write" | "run" | "search";

/** A single tool action inside a round (read/write/run/search). Collapsed by
 *  default; expandable only when it carries a `diff` or `output`. */
export interface Action {
  kind: ActionKind;
  /** Short capitalized label, e.g. "Read" / "Write" / "Run" / "Search". */
  verb: string;
  /** Optional indigo pill, e.g. "emit-edits", shown before the target. */
  emit?: string;
  /** The file path / command / query the action operated on (rendered monospace). */
  target?: string;
  /** Terminal success of the action. `false` => coral "fail" pill. Ignored when
   *  `status` is "run" (the running pill wins). Default (undefined) => sage "ok". */
  ok?: boolean;
  /** "run" => a neutral "running" pill (action still in flight). */
  status?: "run";
  /** A unified-diff hunk to reveal on expand (write actions). */
  diff?: DiffLine[];
  /** Generic monospace output to reveal on expand (read/search/run). May contain a
   *  single `<span class="ok-ln">…</span>` marker that the view renders in sage —
   *  the ONLY HTML token honored, matching the mock; everything else is text. */
  output?: string;
}

// ---- verdict ----------------------------------------------------------------

/** Severity of one Censor finding. `med` renders the label "medium". */
export type FindingSeverity = "high" | "med" | "low";

/** One Censor finding under a DIRTY verdict. */
export interface Finding {
  sev: FindingSeverity;
  /** A monospace teal location, e.g. "auth.rs:42". */
  loc: string;
  /** The human-readable finding message. */
  msg: string;
}

/** The Censor verdict that closes a round.
 *  clean => sage CLEAN shield + "No policy violations[ — X reviewed]".
 *  dirty => coral DIRTY shield + "N finding(s)[ across X]" + the findings list. */
export interface Verdict {
  state: "clean" | "dirty";
  /** A human files summary, e.g. "2 files". When ABSENT the view OMITS the files
   *  clause entirely (it never fabricates a count) — so the meta line reads just
   *  "No policy violations" / "N finding(s)". */
  files?: string;
  /** Only meaningful (and only rendered) when `state==="dirty"`. */
  findings?: Finding[];
}

// ---- round ------------------------------------------------------------------

/** One fix-loop round inside a mini run: a "ROUND n" marker, its actions, and an
 *  optional closing Censor verdict. */
export interface Round {
  n: number;
  actions: Action[];
  verdict?: Verdict;
}

// ---- banner -----------------------------------------------------------------

/** The terminal status banner of a mini run.
 *  done => sage check, esc => amber alert, stop => neutral stop glyph. */
export interface Banner {
  kind: "done" | "esc" | "stop";
  /** Override the default title ("Done" / "Escalated" / "Stopped"). */
  title?: string;
  /** A muted trailing sub-line, e.g. "2 files · 1 round · edits applied". */
  sub?: string;
}

// ---- mini run ---------------------------------------------------------------

/** A delegated mini-coder run, nested under a `spawn` entry. */
export interface MiniRun {
  /** Monospace model label, e.g. "mini · sonnet-4". */
  model: string;
  /** The scope files the mini was given (rendered as right-aligned chips). */
  scope: string[];
  rounds: Round[];
  /** A live shimmer line (e.g. "working — compiling edits…") shown after the last
   *  round while the mini is mid-flight. Absent once the run is terminal. */
  working?: string;
  /** The terminal status banner. Absent while the run is still in flight. */
  banner?: Banner;
}

// ---- top-level timeline entries ---------------------------------------------

/** A coder milestone row (teal "Coder" chip + text + right-aligned time). `text`
 *  MAY contain a single `<span class="mono">…</span>` marker that the view renders
 *  monospace+muted — the ONLY HTML token honored (matching the mock); the rest is
 *  plain text. `node` colors the timeline node dot. */
export interface CoderEntry {
  type: "coder";
  /** Timeline node style: ""=hollow teal, dot=filled teal, sage/terra=colored ring. */
  node?: "" | "dot" | "sage" | "terra";
  text: string;
  time: string;
}

/** A spawn row: a coder milestone ("spawned mini-coder") that OWNS a nested
 *  MiniRun card. */
export interface SpawnEntry {
  type: "spawn";
  node?: "" | "dot" | "sage" | "terra";
  text: string;
  time: string;
  mini: MiniRun;
}

/** A single top-level row of the timeline. */
export type ConsoleEntry = CoderEntry | SpawnEntry;

// ---- the activity snapshot --------------------------------------------------

/** The whole console state for ONE agent. This is the exact shape the backend
 *  `mini_activity_snapshot` returns and the shape every `MiniActivityEvent` is
 *  applied INTO (useAgentConsole.ts owns the apply). All fields optional so an
 *  absent/partial snapshot degrades to the calm empty state. */
export interface ConsoleActivity {
  /** A run is in flight => the Console tab shows a spinner + `runCount`, and a
   *  running timeline auto-scrolls to newest. */
  running?: boolean;
  /** How many mini runs are active (shown in the tab pill). Defaults to 1 in the
   *  tab when `running` and this is absent. */
  runCount?: number;
  /** Explicit calm resting state: render the centered empty state. Implied when
   *  there are no `entries`. */
  empty?: boolean;
  /** The timeline, oldest-first (the view marks the first/last for the gutter). */
  entries?: ConsoleEntry[];
  /** Estimated USD cost for the current task (P2). Null/absent when model is unpriced. */
  taskCostEstimateUsd?: number | null;
}

// ---- pure helpers (unit-tested in node) -------------------------------------

/** Whether the activity should render the calm empty state: explicitly flagged,
 *  or simply carrying no entries. Centralized so the component + the hook + tests
 *  agree on the single emptiness rule. */
export function isEmptyActivity(
  activity: ConsoleActivity | null | undefined,
): boolean {
  if (!activity) return true;
  if (activity.empty) return true;
  return !activity.entries || activity.entries.length === 0;
}

/** The integer run-count to show in the Console tab pill. The pill only renders
 *  while `running`, so this is 0 whenever NOT running (a stray `runCount` on a
 *  resting state never shows). When running: a valid positive integer count, or 1
 *  as the floor for an absent/invalid count (mirrors the mock's `s.runCount || 1`). */
export function consoleRunCount(
  activity: ConsoleActivity | null | undefined,
): number {
  if (!activity?.running) return 0;
  const raw = activity.runCount;
  const valid =
    typeof raw === "number" && Number.isFinite(raw) && raw > 0
      ? Math.floor(raw)
      : 0;
  return valid > 0 ? valid : 1;
}

/** Whether an action can be expanded: it has a diff hunk OR an output block. A
 *  static action (neither) shows no chevron and is not a toggle button. */
export function actionHasDetail(action: Action): boolean {
  return (
    (Array.isArray(action.diff) && action.diff.length > 0) ||
    (typeof action.output === "string" && action.output.length > 0)
  );
}

/** The status-pill descriptor for an action: a running action wins (neutral),
 *  else an explicit `ok===false` is a coral fail, else sage ok. Mirrors the mock's
 *  precedence exactly so the view stays a dumb switch on this. */
export function actionStatus(action: Action): {
  kind: "run" | "ok" | "fail";
  label: string;
} {
  if (action.status === "run") return { kind: "run", label: "running" };
  if (action.ok === false) return { kind: "fail", label: "fail" };
  return { kind: "ok", label: "ok" };
}
