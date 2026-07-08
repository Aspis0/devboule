// AgentConsole — the structured Agent Activity timeline (Step A). The complement
// to the raw xterm terminal, rendered as the "Console" tab in the Work-mode dock.
//
// PURE + PROP-DRIVEN: it renders exactly the `ConsoleActivity` it is handed and owns
// no IO. The live wiring (snapshot + event channel) lives in useAgentConsole.ts; the
// data contract in agentConsoleModel.ts. This is a faithful React/Tailwind port of
// the approved mock (design/project/agent-console/Agent Activity Console.html) using
// the app's cream/teal/sage/amber/coral/indigo tokens.
//
// Collapse state is React `useState` per action (NOT direct DOM): every expandable
// action is a `<button aria-expanded>` toggling its detail; a static action (no
// diff/output) renders as a non-interactive row with no chevron.
//
// PRIVACY: renders only the already-redacted summaries in `ConsoleActivity`.
// The two HTML markers the mock used (`<span class="mono">` in coder text,
// `<span class="ok-ln">` in output) are parsed into JSX by a small, allowlist-only
// splitter below — NEVER dangerouslySetInnerHTML, so no markup the backend sends can
// inject (CSP-strict app).

import {
	ChevronRight,
	ChevronsRight,
	Eye,
	Inbox,
	Pencil,
	Search,
	Shield,
} from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";
import {
	type Action,
	type ActionKind,
	type Banner,
	type ConsoleActivity,
	type ConsoleEntry,
	type DiffLine,
	type MiniRun,
	type QuestionEntry,
	type ThinkingEntry,
	type Round,
	type Verdict,
	actionHasDetail,
	actionStatus,
	isEmptyActivity,
} from "./agentConsoleModel";

// ---- safe inline-marker parsing --------------------------------------------
//
// The mock embedded ONE inline HTML token per field, never nested:
//   - coder text:  <span class="mono">…</span>  → monospace + muted span
//   - output:      <span class="ok-ln">…</span>  → sage span (a passing test line)
// We parse exactly those (and nothing else) into React nodes — no innerHTML, so a
// stray "<script>" or any other tag is rendered as literal text, not markup.

/** Split `text` on `<span class="<cls>">…</span>` markers, wrapping the inner text
 *  of each match in `wrap`. Everything else is emitted as plain text. Greedy-safe:
 *  uses a non-global manual scan so adjacent/multiple markers all parse. */
function parseMarkers(
	text: string,
	cls: string,
	wrap: (inner: string, key: string) => ReactNode,
): ReactNode[] {
	const open = `<span class="${cls}">`;
	const close = `</span>`;
	const out: ReactNode[] = [];
	let rest = text;
	let key = 0;
	while (rest.length > 0) {
		const start = rest.indexOf(open);
		if (start === -1) {
			out.push(rest);
			break;
		}
		if (start > 0) out.push(rest.slice(0, start));
		const after = rest.slice(start + open.length);
		const end = after.indexOf(close);
		if (end === -1) {
			// Unterminated marker: emit the remainder literally (never drop text).
			out.push(rest.slice(start));
			break;
		}
		out.push(wrap(after.slice(0, end), `m${key++}`));
		rest = after.slice(end + close.length);
	}
	return out;
}

/** Coder milestone text with `<span class="mono">…</span>` → mono+muted span. The
 *  parse is memoized on `text`: the input string is immutable, so a streaming
 *  re-render (e.g. a sibling action appended) never re-parses an unchanged string. */
function CoderText({ text }: { text: string }) {
	const parsed = useMemo(
		() =>
			parseMarkers(text, "mono", (inner, key) => (
				<span key={key} className="font-mono text-[11px] text-cream-500">
					{inner}
				</span>
			)),
		[text],
	);
	return <span className="text-[12px] text-cream-800">{parsed}</span>;
}

// ---- icons ------------------------------------------------------------------

/** lucide-react map for action kinds (write=Pencil, read=Eye, run=ChevronsRight,
 *  search=Search). An unknown kind falls back to the run glyph, mirroring the mock. */
function ActionIcon({ kind }: { kind: ActionKind }) {
	const cls = "h-3.5 w-3.5";
	switch (kind) {
		case "write":
			return <Pencil className={cls} aria-hidden />;
		case "read":
			return <Eye className={cls} aria-hidden />;
		case "search":
			return <Search className={cls} aria-hidden />;
		case "run":
		default:
			return <ChevronsRight className={cls} aria-hidden />;
	}
}

// ---- diff / output ----------------------------------------------------------

function DiffBlock({ lines }: { lines: DiffLine[] }) {
	const sigil: Record<DiffLine["t"], string> = {
		add: "+",
		del: "-",
		ctx: " ",
		meta: "",
	};
	return (
		<div className="overflow-hidden rounded-lg border border-cream-200 bg-white font-mono text-[10.5px] leading-[1.7]">
			{lines.map((line, i) => {
				if (line.t === "meta") {
					return (
						<div
							key={i}
							className="border-b border-cream-200 bg-cream-50 px-2.5 py-[3px] text-cream-400"
						>
							{line.s}
						</div>
					);
				}
				const rowClass =
					line.t === "add"
						? "bg-sage/10 text-sage-dark"
						: line.t === "del"
							? "bg-coral/10 text-coral-dark"
							: "text-cream-500";
				const sigClass =
					line.t === "add"
						? "text-sage-dark"
						: line.t === "del"
							? "text-coral-dark"
							: "text-cream-400";
				return (
					<div key={i} className={`flex whitespace-pre px-2.5 ${rowClass}`}>
						<span className={`w-3.5 shrink-0 ${sigClass}`}>
							{sigil[line.t]}
						</span>
						{line.s}
					</div>
				);
			})}
		</div>
	);
}

function OutputBlock({ output }: { output: string }) {
	// Memoized on `output` (immutable) so streaming re-renders never re-parse it.
	const parsed = useMemo(
		() =>
			parseMarkers(output, "ok-ln", (inner, key) => (
				<span key={key} className="text-sage-dark">
					{inner}
				</span>
			)),
		[output],
	);
	return (
		<div className="whitespace-pre-wrap rounded-lg border border-cream-200 bg-cream-50 px-2.5 py-2 font-mono text-[10.5px] leading-[1.65] text-cream-500">
			{parsed}
		</div>
	);
}

// ---- action row -------------------------------------------------------------

function ActionRow({ action }: { action: Action }) {
	const [open, setOpen] = useState(false);
	const hasDetail = actionHasDetail(action);
	const status = actionStatus(action);

	const statusClass =
		status.kind === "ok"
			? "bg-sage/15 text-sage-dark"
			: status.kind === "fail"
				? "bg-coral/15 text-coral-dark"
				: "bg-cream-100 text-cream-500";

	const target = action.target
		? action.kind === "write"
			? `(${action.target})`
			: action.target
		: null;

	const inner = (
		<>
			<span className="flex h-4 w-4 shrink-0 items-center justify-center text-cream-500">
				<ActionIcon kind={action.kind} />
			</span>
			<span className="shrink-0 text-[11.5px] font-semibold text-cream-800">
				{action.verb}
			</span>
			{action.emit ? (
				<span className="shrink-0 rounded border border-indigo/40 bg-indigo/10 px-1.5 py-px font-mono text-[9.5px] font-semibold text-indigo-dark">
					{action.emit}
				</span>
			) : null}
			{target ? (
				<span className="min-w-0 truncate font-mono text-[11px] text-cream-500">
					{target}
				</span>
			) : null}
			<span className="min-w-[8px] flex-1" />
			<span
				className={`shrink-0 rounded-full px-1.5 py-px text-[9px] font-bold uppercase tracking-wider ${statusClass}`}
			>
				{status.label}
			</span>
			{hasDetail ? (
				<ChevronRight
					className={`h-3 w-3 shrink-0 text-cream-400 transition-transform ${
						open ? "rotate-90" : ""
					}`}
					aria-hidden
				/>
			) : null}
		</>
	);

	if (!hasDetail) {
		return (
			<div className="flex w-full items-center gap-2 rounded-lg px-2 py-[5px] text-left">
				{inner}
			</div>
		);
	}

	return (
		<>
			<button
				type="button"
				aria-expanded={open}
				onClick={() => setOpen((v) => !v)}
				className="flex w-full items-center gap-2 rounded-lg px-2 py-[5px] text-left transition-colors hover:bg-cream-100"
			>
				{inner}
			</button>
			{open ? (
				<div className="ml-8 mb-[5px] mt-px">
					{action.diff ? (
						<DiffBlock lines={action.diff} />
					) : action.output ? (
						<OutputBlock output={action.output} />
					) : null}
				</div>
			) : null}
		</>
	);
}

// ---- verdict ----------------------------------------------------------------

function severityLabel(sev: "high" | "med" | "low"): string {
	return sev === "med" ? "medium" : sev;
}

function VerdictBlock({ verdict }: { verdict: Verdict }) {
	const clean = verdict.state === "clean";
	// The files count is shown ONLY when the backend supplied it — we never fabricate
	// one (a hardcoded "2 files" would assert a wrong count for any 1- or 3-file run).
	// Absent => render the verdict line WITHOUT the "… reviewed" / "across …" clause.
	const files = verdict.files;
	const findings = verdict.findings ?? [];
	return (
		<div className="mx-0.5 mb-1 mt-2">
			<div className="flex items-center gap-2">
				<span
					className={`inline-flex h-[21px] items-center gap-1.5 rounded-full px-2.5 text-[10.5px] font-bold tracking-wide ${
						clean
							? "border border-sage/40 bg-sage/15 text-sage-dark"
							: "border border-coral/40 bg-coral/15 text-coral-dark"
					}`}
				>
					<Shield className="h-3 w-3" aria-hidden />
					{clean ? "CLEAN" : "DIRTY"}
				</span>
				<span className="text-[11px] text-cream-500">
					{clean ? (
						<>
							No policy violations
							{files ? (
								<>
									{" — "}
									<b className="font-semibold text-cream-800">{files}</b>{" "}
									reviewed
								</>
							) : null}
						</>
					) : (
						<>
							<b className="font-semibold text-cream-800">{findings.length}</b>{" "}
							{findings.length === 1 ? "finding" : "findings"}
							{files ? <> across {files}</> : null}
						</>
					)}
				</span>
			</div>

			{findings.length > 0 ? (
				<div className="mt-[7px] flex flex-col gap-px">
					{findings.map((finding, i) => {
						const sevClass =
							finding.sev === "high"
								? "bg-coral/15 text-coral-dark"
								: finding.sev === "med"
									? "bg-amber/20 text-amber-dark"
									: "bg-cream-100 text-cream-500";
						return (
							<div
								key={i}
								className="flex items-baseline gap-2.5 rounded-md px-2 py-1 hover:bg-cream-100"
							>
								<span
									className={`w-14 shrink-0 rounded px-0 py-0.5 text-center text-[9px] font-bold uppercase tracking-wide ${sevClass}`}
								>
									{severityLabel(finding.sev)}
								</span>
								<span className="shrink-0 font-mono text-[10.5px] text-teal-dark">
									{finding.loc}
								</span>
								<span className="min-w-0 text-[11px] text-cream-800">
									{finding.msg}
								</span>
							</div>
						);
					})}
				</div>
			) : null}
		</div>
	);
}

// ---- banner -----------------------------------------------------------------

function BannerBlock({ banner }: { banner: Banner }) {
	const map = {
		done: {
			title: "Done",
			cls: "border border-sage/40 bg-sage/15 text-sage-dark",
		},
		esc: {
			title: "Escalated",
			cls: "border border-amber/40 bg-amber/20 text-amber-dark",
		},
		stop: {
			title: "Stopped",
			cls: "border border-cream-200 bg-cream-100 text-cream-500",
		},
	} as const;
	const meta = map[banner.kind];
	return (
		<div
			className={`mx-0.5 mb-0.5 mt-2.5 flex items-center gap-2 rounded-[10px] px-3 py-2 text-[11.5px] font-semibold ${meta.cls}`}
		>
			<span className="flex h-4 w-4 shrink-0 items-center justify-center">
				<BannerIcon kind={banner.kind} />
			</span>
			<span>{banner.title ?? meta.title}</span>
			{banner.sub ? (
				<span className="font-normal text-cream-500">· {banner.sub}</span>
			) : null}
		</div>
	);
}

function BannerIcon({ kind }: { kind: Banner["kind"] }) {
	// Inline-styled tiny strokes matching the mock's check/alert/stop glyphs, drawn
	// with currentColor so each inherits its banner color.
	const common = {
		viewBox: "0 0 24 24",
		width: 14,
		height: 14,
		fill: "none",
		stroke: "currentColor",
		strokeLinecap: "round" as const,
		strokeLinejoin: "round" as const,
		"aria-hidden": true,
	};
	if (kind === "done") {
		return (
			<svg {...common} strokeWidth={2.1}>
				<path d="M20 6 9 17l-5-5" />
			</svg>
		);
	}
	if (kind === "esc") {
		return (
			<svg {...common} strokeWidth={2}>
				<path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z" />
				<path d="M12 9v4M12 17h.01" />
			</svg>
		);
	}
	return (
		<svg {...common} strokeWidth={1.8}>
			<circle cx="12" cy="12" r="9" />
			<rect x="9" y="9" width="6" height="6" rx="1" />
		</svg>
	);
}

// ---- working shimmer --------------------------------------------------------
//
// The pulse dot + shimmering text. `prefers-reduced-motion` is honored with
// Tailwind's `motion-reduce:` variants: NO animation + plain muted text (mirrors
// the mock's @media block). The shimmer is a clipped gradient on text; under
// reduced motion it falls back to a flat muted color.

function WorkingLine({ text }: { text: string }) {
	return (
		<div className="flex items-center gap-2.5 px-2 pb-[3px] pt-[7px]">
			<span className="h-2 w-2 shrink-0 animate-pulse rounded-full bg-indigo motion-reduce:animate-none" />
			<span className="bg-gradient-to-r from-cream-400 via-indigo to-cream-400 bg-[length:200%_100%] bg-clip-text text-[11px] font-medium text-transparent motion-reduce:bg-none motion-reduce:text-cream-500">
				{text}
			</span>
		</div>
	);
}

// ---- round ------------------------------------------------------------------

function RoundBlock({ round }: { round: Round }) {
	return (
		<>
			<div className="mx-0.5 mb-1 mt-[9px] flex items-center gap-2.5 first:mt-0.5">
				<span className="text-[10px] font-bold uppercase tracking-[0.12em] text-cream-400">
					Round {round.n}
				</span>
				<span className="h-px flex-1 bg-cream-200" />
			</div>
			{/* Index keys are CORRECT here: the MiniActivityEvent contract is APPEND-ONLY
          (appendAction only pushes to the end; actions never reorder or get removed),
          so a given index always maps to the same action. And an agentId change fully
          resets the hook (the effect re-runs, the timeline is rebuilt), so an
          ActionRow's local open/collapse state can never mis-bind to a different
          agent's action. A keyed-by-content scheme would buy nothing. */}
			{round.actions.map((action, i) => (
				<ActionRow key={i} action={action} />
			))}
			{round.verdict ? <VerdictBlock verdict={round.verdict} /> : null}
		</>
	);
}

// ---- mini run card ----------------------------------------------------------

function MiniCard({ mini }: { mini: MiniRun }) {
	return (
		<div className="mb-2 mt-1.5 overflow-hidden rounded-xl border border-cream-200 border-l-[2.5px] border-l-indigo-light bg-indigo/[0.03]">
			<div className="flex flex-wrap items-center gap-2 border-b border-cream-200 bg-white px-3 py-2.5">
				<span className="inline-flex h-[19px] items-center gap-1.5 rounded-full border border-indigo/40 bg-indigo/15 px-2.5 text-[10.5px] font-semibold text-indigo-dark">
					<span className="h-[5px] w-[5px] rounded-full bg-current" />
					Mini
				</span>
				<span className="font-mono text-[11px] text-cream-500">
					{mini.model}
				</span>
				<span className="ml-auto flex items-center gap-1.5">
					{mini.scope.map((file, i) => (
						<span
							key={i}
							className="rounded-full border border-cream-200 bg-cream-100 px-2 py-0.5 font-mono text-[10px] text-cream-500"
						>
							{file}
						</span>
					))}
				</span>
			</div>
			<div className="px-3 pb-2.5 pt-[7px]">
				{mini.rounds.map((round) => (
					<RoundBlock key={round.n} round={round} />
				))}
				{mini.working ? <WorkingLine text={mini.working} /> : null}
				{mini.banner ? <BannerBlock banner={mini.banner} /> : null}
			</div>
		</div>
	);
}

function nodeRingClass(
	node: "" | "dot" | "sage" | "terra" | undefined,
): string {
	switch (node) {
		case "dot":
			return "border-teal bg-teal";
		case "sage":
			return "border-sage bg-white";
		case "terra":
			return "border-terracotta bg-white";
		default:
			return "border-teal bg-white";
	}
}

/** Part A: a model `thinking` block, rendered COLLAPSED (one muted preview row).
 *  Per-row `useState` toggle expands to the full thinking text in a pre-wrapped,
 *  muted block. The gutter node uses the default hollow style; time sits on the right. */
function ThinkingRow({
	entry,
	first,
	last,
}: {
	entry: ThinkingEntry;
	first: boolean;
	last: boolean;
}) {
	const [open, setOpen] = useState(false);
	const preview = entry.text.split("\n")[0] ?? "";
	return (
		<div className="relative flex gap-2.5 py-[5px]">
			{/* gutter line + node (default hollow style) */}
			<div className="relative flex w-[18px] shrink-0 justify-center">
				{!last ? (
					<span
						className={`absolute bottom-[-10px] w-[1.5px] bg-cream-200 ${
							first ? "top-[11px]" : "top-0"
						}`}
						aria-hidden
					/>
				) : null}
				<span
					className={`relative z-[1] mt-1 h-[9px] w-[9px] rounded-full border-2 ${nodeRingClass(
						undefined,
					)}`}
					aria-hidden
				/>
			</div>

			{/* content */}
			<div className="min-w-0 flex-1 pt-px">
				<div className="flex flex-wrap items-center gap-2">
					<button
						type="button"
						onClick={() => setOpen((v) => !v)}
						className="inline-flex h-[19px] items-center gap-1.5 rounded-full border border-cream-300 bg-cream-100 px-2.5 text-[10.5px] font-normal italic text-cream-400"
						title={open ? "Collapse thinking" : "Expand thinking"}
					>
						<span className="h-[5px] w-[5px] rounded-full bg-current" />
						Thinking
					</button>
					<span className="min-w-0 truncate text-[12px] italic text-cream-400">
						{preview}
					</span>
					<span className="ml-auto whitespace-nowrap font-mono text-[10.5px] text-cream-400">
						{entry.time}
					</span>
				</div>
				{open ? (
					<pre className="mt-1 whitespace-pre-wrap break-words text-[12px] italic text-cream-500">
						{entry.text}
					</pre>
				) : null}
			</div>
		</div>
	);
}

function TimelineRow({
	entry,
	first,
	last,
}: {
	// Kairion `question` entries are rendered in the planner Plan panel, not this raw
	// timeline (filtered out by the caller), so this row never narrows over a doubt.
	entry: Exclude<ConsoleEntry, QuestionEntry>;
	first: boolean;
	last: boolean;
}) {
	// Part A: a `thinking` entry renders as its own (per-row, expandable) row.
	if (entry.type === "thinking") {
		return <ThinkingRow entry={entry} first={first} last={last} />;
	}
	return (
		<div className="relative flex gap-2.5 py-[5px]">
			{/* gutter line + node */}
			<div className="relative flex w-[18px] shrink-0 justify-center">
				{!last ? (
					<span
						className={`absolute bottom-[-10px] w-[1.5px] bg-cream-200 ${
							first ? "top-[11px]" : "top-0"
						}`}
						aria-hidden
					/>
				) : null}
				<span
					className={`relative z-[1] mt-1 h-[9px] w-[9px] rounded-full border-2 ${nodeRingClass(
						entry.type === "webSearch" ||
							entry.type === "chat" ||
							entry.type === "banner"
							? undefined
							: entry.node,
					)}`}
					aria-hidden
				/>
			</div>

			{/* content */}
			<div className="min-w-0 flex-1 pt-px">
				<div className="flex flex-wrap items-center gap-2">
					{entry.type === "webSearch" ? (
						<span className="inline-flex h-[19px] items-center gap-1.5 rounded-full border border-terracotta/40 bg-terracotta/10 px-2.5 text-[10.5px] font-semibold text-terracotta">
							<span className="h-[5px] w-[5px] rounded-full bg-current" />
							Web
						</span>
					) : entry.type === "chat" ? (
						<span className="inline-flex h-[19px] items-center gap-1.5 rounded-full border border-cream-300 bg-cream-100 px-2.5 text-[10.5px] font-semibold text-cream-700">
							<span className="h-[5px] w-[5px] rounded-full bg-current" />
							{entry.role === "user" ? "You" : "Orchestrator"}
						</span>
					) : entry.type === "banner" ? (
						<span className="inline-flex h-[19px] items-center gap-1.5 rounded-full border border-cream-300 bg-cream-100 px-2.5 text-[10.5px] font-semibold text-cream-500">
							<span className="h-[5px] w-[5px] rounded-full bg-current" />
							Notice
						</span>
					) : (
						<span className="inline-flex h-[19px] items-center gap-1.5 rounded-full border border-teal/40 bg-teal/15 px-2.5 text-[10.5px] font-semibold text-teal-dark">
							<span className="h-[5px] w-[5px] rounded-full bg-current" />
							Coder
						</span>
					)}
					{entry.type === "webSearch" ? (
						<span className="min-w-0 truncate text-[12px] text-cream-700">
							{entry.query}
							<span className="ml-1.5 text-cream-400">
								· {entry.pages.length} page{entry.pages.length === 1 ? "" : "s"}
							</span>
						</span>
					) : (
						<CoderText text={entry.text} />
					)}
					<span className="ml-auto whitespace-nowrap font-mono text-[10.5px] text-cream-400">
						{entry.time}
					</span>
				</div>
				{entry.type === "spawn" ? <MiniCard mini={entry.mini} /> : null}
			</div>
		</div>
	);
}

// ---- empty state ------------------------------------------------------------

function EmptyState() {
	return (
		<div className="flex h-full flex-col items-center justify-center gap-2.5 px-6 py-6 text-center">
			<span className="flex h-[46px] w-[46px] items-center justify-center rounded-xl border border-cream-200 bg-cream-100 text-cream-500">
				<Inbox className="h-[22px] w-[22px]" aria-hidden />
			</span>
			<b className="text-[13px] font-semibold text-cream-800">
				No agent activity yet
			</b>
			<p className="m-0 max-w-[280px] text-[11.5px] leading-[1.55] text-cream-500">
				When a coder claims a task or spawns a mini-coder, its loop will stream
				here — rounds, edits, and Censor verdicts.
			</p>
			<span className="mt-1 font-mono text-[10.5px] text-cream-400">
				waiting on orchestrator…
			</span>
		</div>
	);
}

// ---- the console ------------------------------------------------------------

export interface AgentConsoleProps {
	/** The console state for the selected agent (from useAgentConsole). */
	activity: ConsoleActivity;
}

/** The structured agent-activity timeline. PURE + prop-driven. */
export function AgentConsole({ activity }: AgentConsoleProps) {
	if (isEmptyActivity(activity)) {
		return (
			<div className="flex min-h-[256px] flex-1 flex-col">
				<EmptyState />
			</div>
		);
	}

	// Kairion `question` entries are surfaced in the planner Plan panel (DoubtPanel), not
	// in this raw timeline — filter them out here so the console keeps its action/chat
	// semantics (and TimelineRow never narrows over a doubt shape).
	const entries = (activity.entries ?? []).filter(
		(e): e is Exclude<ConsoleEntry, QuestionEntry> => e.type !== "question",
	);

	return (
		<div
			className="relative"
			data-running={activity.running ? "true" : undefined}
		>
			{entries.map((entry, i) => (
				<TimelineRow
					// Fix2: stable key across FIFO eviction. `entries` is front-evicted
					// at MAX_CONSOLE_ENTRIES, so a bare 0-based `i` shifts and bleeds
					// per-row state (e.g. an expanded ThinkingRow). `entriesBase` is the
					// live mapper's monotonic eviction count; base + i is a stable id.
					key={(activity.entriesBase ?? 0) + i}
					entry={entry}
					first={i === 0}
					last={i === entries.length - 1}
				/>
			))}
		</div>
	);
}
