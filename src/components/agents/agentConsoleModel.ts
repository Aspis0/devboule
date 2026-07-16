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

/** One real web page the orchestrator read (Exa): source url + title + a distilled
 *  summary (the "finding"). Mirrors the backend `PageEntry`/`ExaPage`. */
export interface ConsolePage {
	url: string;
	title: string;
	summary: string;
}

/** A websearch row: the query + the REAL pages just read. The planner panel's
 *  Websearch view renders these as live sources + distilled findings. */
export interface WebSearchEntry {
	type: "webSearch";
	query: string;
	pages: ConsolePage[];
	time: string;
}

/** A standalone notice banner (e.g. a web search that completed but returned
 *  no extractable results). Rendered as a muted system line in the timeline.
 *  Mirrors the backend `ConsoleEntry::Banner`. */
export interface BannerEntry {
	type: "banner";
	text: string;
	time: string;
}

/** A model thinking block (pi sessions). Rendered collapsed; expands on click.
 *  Mirrors the backend `ConsoleEntry::Thinking`: `text` is the full thinking content. */
export interface ThinkingEntry {
	type: "thinking";
	text: string;
	time: string;
}

/** A conversational chat turn surfaced into the planner chat: the orchestrator's own
 *  words (`assistant`) or a steer echoed back (`user`). Mirrors the backend `Chat` entry.
 *
 *  role "plan": a structured plan payload emitted by the orchestrator's `plan` tool
 *  (see `devboule_plan` wire contract). `text` is the COMPACT JSON of the payload
 *  `{title, steps:[{text,status}], notes?}` (compact so it fits the bridge-file
 *  caps; JSON.parse is whitespace-agnostic) — consumed by the Plan stage via
 *  `latestPlan` / `planCardsFromPiPlan`, NEVER rendered as a chat bubble. Mirrors the
 *  backend `ConsoleEntry::Chat { role: "plan", text: <compact JSON>, ... }`. */
export interface ChatEntry {
	type: "chat";
	role: "assistant" | "user" | "plan";
	text: string;
	time: string;
	/** D3 (planner-chat demolition): the client-generated send id echoed back through
	 *  the bridge (cloud-duplex user echoes). The planner drains its optimistic pending
	 *  copy BY this id; absent for local-binary echoes and historical lines. */
	msgId?: string;
}

// ---- Kairion doubt (orchestrator-only) --------------------------------------

/** One pickable answer to a Kairion doubt. `id` is the stable option key; `label`
 *  is the human word shown on the button and used to phrase the steer line. */
export interface QuestionOption {
	id: string;
	label: string;
}

/** One candidate direction the doubt is pulled toward, with its current `pull`
 *  (0..1). This is the visual tension of the lean-field — NOT a percentage shown to
 *  the user, only the relative gravitation of the marker. Mirrors `DoubtSignal.candidates`. */
export interface DoubtCandidate {
	label: string;
	pull: number;
}

/** A Kairion doubt surfaced into the planner Plan view (ORCHESTRATOR-ONLY): the
 *  orchestrator is genuinely split on a fork and shows its insecurity as instability
 *  rather than guessing. Mirrors the frozen QUESTION wire line (serde camelCase) +
 *  the embedded DoubtSignal fields. Degrades to a plain question when no thinking /
 *  doubt is present (empty candidates + null lean). No persistence: present-tense only.
 *
 *  Wire contract (must match the Rust encoder byte-for-byte; Rust tags via
 *  #[serde(tag="type")], so the discriminant is `type`, not `kind`):
 *  { type:"question", id, text, options:[{id,label}], unrest, candidates:[{label,pull}],
 *    lean:string|null, directionConfidence, status:"open"|"reopened", affects:[str], time } */
export interface QuestionEntry {
	type: "question";
	/** Stable within the session, so a `reopened` event updates the doubt in place. */
	id: string;
	/** The fork the orchestrator is split on (the question text). */
	text: string;
	/** The pickable answers. */
	options: QuestionOption[];
	/** Overall instability 0..1 — how unsure the orchestrator is (drives the tremor). */
	unrest: number;
	/** The competing directions with their pulls (drives the marker gravitation). */
	candidates: DoubtCandidate[];
	/** The leaned option label, or `null` for "genuinely split" (honest — never faked). */
	lean: string | null;
	/** Confidence in the `lean` direction 0..1. Low => the lean reads as a SOFT hint,
	 *  not a verdict (the tremor stays). */
	directionConfidence: number;
	/** `open` = first asked; `reopened` = the orchestrator changed its own mind (destabilises). */
	status: "open" | "reopened";
	/** The plan task(s) this doubt shapes — drives the doubt<->task hover link. */
	affects: string[];
	time: string;
}

/** A single top-level row of the timeline. */
export type ConsoleEntry =
	| CoderEntry
	| SpawnEntry
	| WebSearchEntry
	| ChatEntry
	| BannerEntry
	| ThinkingEntry
	| QuestionEntry;

/** B14b: the live, in-progress assistant reply tail — a SEPARATE slot from `entries` (so it
 *  is immune to FIFO eviction / interleaved events), rendered as the last in-progress assistant
 *  bubble until the final `chat` turn lands the real entry. `seq` ties it to one turn. */
export interface StreamingChat {
	seq: number;
	text: string;
}

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
	/** Fix2: stable base offset = how many timeline entries were front-evicted from
	 *  the live mapper's history. The view computes a STABLE React key =
	 *  `entriesBase + i` so a row keeps its identity across FIFO eviction (a plain
	 *  0-based `i` would shift after eviction and bleed per-row state, e.g. an
	 *  expanded ThinkingRow adopting a different block). Absent/0 means no eviction. */
	entriesBase?: number;
	/** Estimated USD cost for the current task (P2). Null/absent when model is unpriced. */
	taskCostEstimateUsd?: number | null;
	/** B14b: the live in-progress assistant reply (token streaming). Absent when no reply is
	 *  currently streaming; cleared when the final `chat` turn lands. */
	streamingChat?: StreamingChat | null;
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
