import { useEffect, useRef, useState } from "react";
import { useAppContext } from "../../context/AppContext";

type HelpTooltip = {
	top: number;
	left: number;
	title: string;
	lines: string[];
};

const HELP_WIDTH = 380;
const HELP_HEIGHT = 238;
const HELP_MAX_LINES = 8;

/** Only author-annotated elements are help targets (no generic button/input fallback). */
export const HELP_TARGET_SELECTOR =
	"[data-help-title], [data-help-lines]";

const pageUseLines: Record<string, string> = {
	secrets:
		"For Devboule, this page keeps GitHub tokens and vault credentials out of code and project notes.",
	oracle:
		"For Devboule, Oracle is the local memory agents should query before touching code, plans, or project notes.",
	projects:
		"For Devboule, Projects is the mini-Notion control board where human plans, agent claims, evidence, and verifier gates meet.",
	agents:
		"For Devboule, Agents is the bridge between the Kanban, CLI terminals, MCP tools, and Oracle.",
	graph:
		"For Devboule, Graph is a structural map of code relationships; Oracle is the stronger source for semantic answers.",
	design:
		"For Devboule, Design is the generative UI lab: preview layouts, tokens, and component variants before they land in product views.",
	polis:
		"For Devboule, Polis is the spatial map of the product surface — orientation for humans and agents navigating the app structure.",
	skills:
		"For Devboule, Skills is the per-project skill catalog agents and humans use to stay aligned on how work should be done.",
	labs:
		"For Devboule, Labs holds experimental feature toggles (Pigeon, Oracle gates, etc.) so risky switches stay explicit.",
	help:
		"For Devboule, Help is the getting-started / how-it-works page for humans onboarding onto the control plane.",
	settings:
		"For Devboule, Settings is where workspace roots, roles, models, and app preferences are configured safely.",
	devices:
		"For Devboule, Devices tracks local endpoints and machine context so agent and sync actions know where they run.",
	workspace:
		"For Devboule, Workspace is the indexed project root and file memory agents and Oracle share for reliable retrieval.",
};

/** Page-context line for the active view (exported for tests). */
export function pageUseLineFor(activeView: string): string {
	return (
		pageUseLines[activeView] ??
		"For Devboule, use this only when it makes the local project, agents, or Oracle memory more reliable."
	);
}

/**
 * Walk up from `el` to the nearest author-annotated help element.
 * Returns null when there is no `[data-help-title]` / `[data-help-lines]` ancestor,
 * or when that target is under `[data-help-skip='true']`.
 */
export function findHelpTarget(el: Element | null): HTMLElement | null {
	if (!el || !(el instanceof Element)) return null;
	const target = el.closest(HELP_TARGET_SELECTOR);
	if (!(target instanceof HTMLElement)) return null;
	if (target.closest("[data-help-skip='true']")) return null;
	return target;
}

function cleanText(value: string | null | undefined) {
	return (value ?? "").replace(/\s+/g, " ").trim();
}

function fallbackLabel(element: HTMLElement) {
	const explicit =
		element.getAttribute("aria-label") ||
		element.getAttribute("title") ||
		element.getAttribute("placeholder") ||
		cleanText(element.textContent);
	return cleanText(explicit) || element.tagName.toLowerCase();
}

function fallbackTitle(element: HTMLElement) {
	const tag = element.tagName.toLowerCase();
	const label = fallbackLabel(element);
	if (tag === "input" || tag === "textarea")
		return "This field is where you type a value.";
	if (tag === "select")
		return "This menu chooses which thing the app works on.";
	if (tag === "button") return `This button runs "${label}".`;
	return `This part controls "${label}".`;
}

function areaText(element: HTMLElement) {
	const section = element.closest("section, article, aside, header, main, div");
	return cleanText(section?.textContent).slice(0, 600);
}

function semanticLines(element: HTMLElement, title: string) {
	const haystack =
		`${title} ${fallbackLabel(element)} ${element.dataset.helpLines ?? ""} ${areaText(element)}`.toLowerCase();
	const lines: string[] = [];

	if (haystack.includes("secret")) {
		lines.push(
			"For Devboule, secrets keep model and vault credentials out of code, Markdown, Oracle chunks, and agent prompts.",
		);
	}
	if (
		haystack.includes("token") ||
		haystack.includes("api key") ||
		haystack.includes("key")
	) {
		lines.push(
			"For Devboule, tokens decide whether the app can access GitHub, query remote models, or give agents scoped vault access.",
		);
		lines.push(
			"Temporary keys expire: replace them in the app vault instead of hardcoding them or pasting them into project notes.",
		);
	}
	if (
		haystack.includes("oracle") ||
		haystack.includes("index") ||
		haystack.includes("chunk") ||
		haystack.includes("embedding") ||
		haystack.includes("lancedb")
	) {
		lines.push(
			"For Devboule, Oracle must retrieve real files and project evidence before any local or remote model answer is trusted.",
		);
	}
	if (
		haystack.includes("agent") ||
		haystack.includes("mcp") ||
		haystack.includes("codex") ||
		haystack.includes("claude") ||
		haystack.includes("verifier") ||
		haystack.includes("coder")
	) {
		lines.push(
			"For Devboule, agents should work through MCP: read project state, ask Oracle, claim tasks, then update status.",
		);
	}
	if (
		haystack.includes("project") ||
		haystack.includes("task") ||
		haystack.includes("kanban") ||
		haystack.includes("note")
	) {
		lines.push(
			"For Devboule, project Markdown is the durable source of truth that the UI, Oracle, and CLI agents can all read.",
		);
	}
	if (
		haystack.includes("dry") ||
		haystack.includes("smoke") ||
		haystack.includes("audit")
	) {
		lines.push(
			"For Devboule, dry runs and audits are proof steps: they should show scope and evidence before a real write.",
		);
	}

	return lines;
}

function fallbackLines(element: HTMLElement) {
	const disabled =
		element.hasAttribute("disabled") ||
		element.getAttribute("aria-disabled") === "true";
	const tag = element.tagName.toLowerCase();
	const label = fallbackLabel(element);
	const lowerLabel = label.toLowerCase();
	if (tag === "select") {
		return [
			`This menu chooses ${label === "select" ? "which item the next action uses" : label}.`,
			"A menu is usually safe by itself: it changes context, not cloud state.",
			"The important part is the next button you press, because it may use this selection.",
			"For Devboule, check project, provider, account, model, and role selections before launching agents or cloud actions.",
			disabled
				? "It is disabled because the required data is not ready yet."
				: "If the list looks empty, sync or reload the page first.",
		];
	}
	if (tag === "input" || tag === "textarea") {
		if (
			lowerLabel.includes("password") ||
			lowerLabel.includes("token") ||
			lowerLabel.includes("key") ||
			element.getAttribute("type") === "password"
		) {
			return [
				"This field is for a private credential or key-like value.",
				"For Devboule, credentials should live in the Windows vault, not in code, project Markdown, Oracle chunks, or agent prompts.",
				"The app should save the value only when you press the matching Save/Rotate action.",
				"Temporary provider keys expire; replace them here when sync, model calls, or agent operations start failing.",
				disabled
					? "It is disabled because another required condition is missing."
					: "Before saving, check that the token belongs to the pinned Devboule account or project.",
			];
		}
		return [
			`This field lets you type ${label === "input" || label === "textarea" ? "a value for this page" : label}.`,
			"It normally changes only local form state until you press the matching action.",
			"For Devboule, prefer concrete names: project title, task goal, provider id, model, root path, or evidence note.",
			"Do not type raw secrets in ordinary notes, search fields, or project text.",
			disabled
				? "It is disabled because another required value or job is missing."
				: "If the value controls agents or cloud resources, verify it before saving.",
		];
	}
	if (tag === "button") {
		return [
			`This button runs "${label}".`,
			"Disabled usually means a required token, project, selection, sync result, or confirmation is missing.",
			"For Devboule, cloud and agent actions should run through the Tauri backend so permissions, scopes, and audit evidence are controlled.",
			"Provider writes should show the provider scope, token role, API equivalent, and project evidence when wired.",
			"For destructive actions, read the confirmation text before accepting.",
		];
	}
	return [
		"This area affects what you see, what is selected, or what the next action will use.",
		"Provider data comes from live sync when tokens and scopes are configured.",
		"For Devboule, project evidence should be written to local Markdown so Oracle and agents can recover context.",
		"If something looks stale, refresh the page section before acting.",
	];
}

function uniqueLines(lines: string[]) {
	const seen = new Set<string>();
	const result: string[] = [];
	for (const line of lines.map(cleanText).filter(Boolean)) {
		const key = line.toLowerCase();
		if (seen.has(key)) continue;
		seen.add(key);
		result.push(line);
	}
	return result;
}

export function readHelp(element: HTMLElement, activeView: string) {
	const title = cleanText(element.dataset.helpTitle) || fallbackTitle(element);
	const rawLines = element.dataset.helpLines;
	const baseLines = rawLines
		? rawLines
				.split("|")
				.map((line) => cleanText(line))
				.filter(Boolean)
		: fallbackLines(element);
	const lines = uniqueLines([
		...baseLines,
		...semanticLines(element, title),
		pageUseLineFor(activeView),
	]);
	return { title, lines: lines.slice(0, HELP_MAX_LINES) };
}

/** Place the tooltip next to `rect`, clamped to the viewport. */
function positionTooltip(rect: DOMRect): { top: number; left: number } {
	const viewportWidth = window.innerWidth;
	const viewportHeight = window.innerHeight;
	const canPlaceRight = rect.right + HELP_WIDTH + 12 < viewportWidth;
	const canPlaceLeft = rect.left - HELP_WIDTH - 12 > 0;
	const left = canPlaceRight
		? rect.right + 8
		: canPlaceLeft
			? rect.left - HELP_WIDTH - 8
			: Math.min(
					Math.max(8, rect.left),
					Math.max(8, viewportWidth - HELP_WIDTH - 8),
				);
	const below =
		rect.bottom + HELP_HEIGHT < viewportHeight || rect.top < HELP_HEIGHT;
	const top =
		canPlaceRight || canPlaceLeft
			? Math.min(
					Math.max(8, rect.top),
					Math.max(8, viewportHeight - HELP_HEIGHT - 8),
				)
			: below
				? Math.min(
						rect.bottom + 6,
						Math.max(8, viewportHeight - HELP_HEIGHT - 8),
					)
				: Math.max(8, rect.top - HELP_HEIGHT - 6);
	return { top, left };
}

function buildTooltip(
	element: HTMLElement | null,
	activeView: string,
): HelpTooltip | null {
	if (!element) return null;
	const rect = element.getBoundingClientRect();
	if (rect.width < 3 || rect.height < 3) return null;
	const { title, lines } = readHelp(element, activeView);
	const { top, left } = positionTooltip(rect);
	return { top, left, title, lines };
}

/**
 * Help mode should engage ONLY on a BARE Alt (Option on macOS) keydown.
 *
 * A bare Alt keydown is `event.key === "Alt"` with no other modifier held.
 * We must NOT trigger on `event.altKey === true` for other keys: on macOS,
 * Option is a dead-key used to compose special characters (Option+8 = •,
 * Option+e = accents, etc.). If help mode fired on any Alt+char combo, it would
 * hijack those keystrokes and block typing in inputs/textareas.
 */
export function shouldEnterHelpMode(event: KeyboardEvent): boolean {
	return (
		event.key === "Alt" &&
		!event.shiftKey &&
		!event.ctrlKey &&
		!event.metaKey
	);
}

export function HelpModeOverlay() {
	const { activeView } = useAppContext();
	// helpMode lives here, not in the global AppContext: holding/releasing Alt
	// would otherwise re-render the whole app. Only this overlay needs it.
	const [helpMode, setHelpMode] = useState(false);
	const [tooltip, setTooltip] = useState<HelpTooltip | null>(null);

	// Last help-bearing target under the pointer / focus; kept so scroll/resize
	// can recompute position without another hit-test.
	const targetRef = useRef<HTMLElement | null>(null);

	// Cache the active view in a ref so pointer/focus handlers always read the
	// current value without re-subscribing listeners.
	const activeViewRef = useRef(activeView);
	activeViewRef.current = activeView;

	// Alt key drives help mode. Kept local so the global provider never re-renders
	// on Alt press/release.
	useEffect(() => {
		const onKeyDown = (event: KeyboardEvent) => {
			// Only a lone Alt (Option) keydown reveals help mode. Alt combined with a
			// character or another modifier is left alone so Option+char typing on
			// macOS reaches inputs/textareas normally.
			if (shouldEnterHelpMode(event)) setHelpMode(true);
		};
		const onKeyUp = (event: KeyboardEvent) => {
			// Hide when the bare Alt key is released, or whenever Alt is no longer
			// held. Never blocks input because we never call preventDefault.
			if (event.key === "Alt" || !event.altKey) setHelpMode(false);
		};
		const onBlur = () => setHelpMode(false);

		window.addEventListener("keydown", onKeyDown);
		window.addEventListener("keyup", onKeyUp);
		window.addEventListener("blur", onBlur);
		return () => {
			window.removeEventListener("keydown", onKeyDown);
			window.removeEventListener("keyup", onKeyUp);
			window.removeEventListener("blur", onBlur);
		};
	}, []);

	// Pointer + focus targeting: one contextual tooltip under cursor / focus.
	useEffect(() => {
		if (!helpMode) {
			targetRef.current = null;
			setTooltip(null);
			return;
		}

		const applyTarget = (element: HTMLElement | null) => {
			targetRef.current = element;
			setTooltip(buildTooltip(element, activeViewRef.current));
		};

		const onPointerMove = (event: PointerEvent | MouseEvent) => {
			const under = document.elementFromPoint(event.clientX, event.clientY);
			const next = findHelpTarget(under);
			// Skip re-render when still over the same annotated element.
			if (next === targetRef.current) return;
			applyTarget(next);
		};

		const onFocusIn = (event: FocusEvent) => {
			const next = findHelpTarget(
				event.target instanceof Element ? event.target : null,
			);
			if (next === targetRef.current) return;
			applyTarget(next);
		};

		// Recompute position when layout moves (scroll/resize) without changing
		// the target. Throttled to one rAF so bursts collapse.
		let frame = 0;
		let scheduled = false;
		const recomputePosition = () => {
			scheduled = false;
			frame = 0;
			setTooltip(buildTooltip(targetRef.current, activeViewRef.current));
		};
		const scheduleRecompute = () => {
			if (scheduled) return;
			scheduled = true;
			frame = window.requestAnimationFrame(recomputePosition);
		};

		// Seed from current focus if the user entered help mode via keyboard.
		const focused =
			document.activeElement instanceof Element
				? document.activeElement
				: null;
		applyTarget(findHelpTarget(focused));

		window.addEventListener("pointermove", onPointerMove);
		// mousemove fallback for environments that do not emit pointer events.
		window.addEventListener("mousemove", onPointerMove);
		window.addEventListener("focusin", onFocusIn);
		window.addEventListener("resize", scheduleRecompute);
		window.addEventListener("scroll", scheduleRecompute, true);

		return () => {
			if (frame) window.cancelAnimationFrame(frame);
			window.removeEventListener("pointermove", onPointerMove);
			window.removeEventListener("mousemove", onPointerMove);
			window.removeEventListener("focusin", onFocusIn);
			window.removeEventListener("resize", scheduleRecompute);
			window.removeEventListener("scroll", scheduleRecompute, true);
		};
	}, [helpMode]);

	// Rebuild tooltip copy when the active view changes while help mode is held
	// (page line is part of readHelp).
	useEffect(() => {
		if (!helpMode || !targetRef.current) return;
		const frame = window.requestAnimationFrame(() =>
			setTooltip(buildTooltip(targetRef.current, activeView)),
		);
		return () => window.cancelAnimationFrame(frame);
	}, [activeView, helpMode]);

	if (!helpMode) return null;

	const pageLine = pageUseLineFor(activeView);

	return (
		<div className="pointer-events-none fixed inset-0 z-[120]">
			<div className="absolute right-4 top-3 max-w-xs rounded-xl border border-terracotta/20 bg-white/95 px-3 py-2 text-[11px] font-semibold text-cream-700 shadow-soft-lg">
				<p>
					Help mode: hold Alt and hover a control with authored help. Move the
					pointer (or focus with the keyboard) to read one tip at a time.
				</p>
				<p className="mt-1 font-normal text-cream-600">{pageLine}</p>
			</div>
			{tooltip ? (
				<div
					style={{
						top: tooltip.top,
						left: tooltip.left,
						width: HELP_WIDTH,
					}}
					className="absolute max-h-60 overflow-hidden rounded-xl border border-terracotta/20 bg-white/95 px-3 py-2 text-left shadow-soft-lg backdrop-blur"
				>
					<p className="text-[12px] font-semibold leading-4 text-cream-900">
						{tooltip.title}
					</p>
					<div className="mt-1 space-y-0.5">
						{tooltip.lines.map((line, index) => (
							<p
								key={`${index}:${line.slice(0, 24)}`}
								className="text-[10.5px] leading-4 text-cream-600"
							>
								{line}
							</p>
						))}
					</div>
				</div>
			) : null}
		</div>
	);
}
