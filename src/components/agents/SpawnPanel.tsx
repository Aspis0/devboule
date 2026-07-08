// One launch card for the Agents fleet view (and reusable by ProjectAgentPanel).
// Replaces the three large role-rule cards + the per-role launch button grid with
// a single explicit flow: pick role, model (advisory), task, project (when global),
// then choose where it runs — Launch in app (PTY), Launch external, or Copy prompt.
//
// All launch derivation (host + model threading, disabled reason) lives in the
// pure agentRowModel builders so this is the thin JSX shell. Full role rules sit
// behind a CollapsibleSection so the page is not dominated by them.

import { Copy, Play, ShieldQuestion, SquareTerminal } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import type { AgentRoleRule, ProjectTask } from "../../types/backend";
import type { CustomAgentClient } from "../../types/config";
import { CollapsibleSection } from "../projects/CollapsibleSection";
import {
	buildLaunchInput,
	modelSuggestionsForClient,
	orchestratorModelNote,
	spawnDisabledReason,
	type SpawnLaunchInput,
	type SpawnSelection,
} from "./agentRowModel";
import type { SpawnRole } from "./roleDisplay";
import { invokeBackendCommand } from "../../context/AppContext";

// Phase 6 — the FALLBACK seed for the override selector's languages (the core bundle set). The
// live list is loaded at runtime from `skills_lang_catalog` so a newly-added bundle language shows
// up with no code change; this seed only fills the gap while that loads / if it errors. "" in the
// selector = auto-detect (the panel uses the backend-detected primary).
const PERSONA_LANGS = [
	"rust",
	"node",
	"python",
	"go",
	"cpp",
	"kotlin",
] as const;

// Phase B role merge: only coder/verifier are spawnable. A coder PLANS and CODES
// (and may fan out to subagents, which surfaces a derived "orchestrator" badge);
// "orchestrator" is no longer a spawn choice.
export const ROLE_OPTIONS: { id: SpawnRole; label: string; summary: string }[] =
	[
		{
			id: "coder",
			label: "Coder",
			summary: "Plans, edits code, and moves tasks toward Review.",
		},
		{
			id: "verifier",
			label: "Verifier",
			summary: "Audits work read-only and decides when Done is justified.",
		},
	];

// Built-in CLIs always shown in the selector; configured custom clients are
// appended after these (see `clientOptions`). Each carries an explicit label so
// the orchestrator can read "Local (Devboule)" rather than its raw id. "codex" /
// "claude" keep their id as the label (rendered capitalized by the selector).
// L2.4: "orchestrator" selects the LOCAL Devboule main coder (oMLX) — the backend
// dispatches its own binary instead of an external CLI when client ===
// "orchestrator" (normalize_agent_client + RESERVED_CLIENT_IDS in projects.rs).
const BUILTIN_CLIENTS: { id: string; label: string }[] = [
	{ id: "codex", label: "codex" },
	{ id: "claude", label: "claude" },
	{ id: "openai", label: "openai" },
	{ id: "orchestrator", label: "Local (Devboule)" },
];

export interface SpawnPanelProps {
	// Project dropdown source (global use). When `lockedProjectId` is set the
	// dropdown is replaced by a static label (project-scoped use).
	projects: { id: string; title: string }[];
	lockedProjectId?: string | null;
	selectedProjectId: string;
	onSelectProject?: (projectId: string) => void;
	// Tasks of the currently-selected project, and whether it is active.
	tasks: ProjectTask[];
	projectActive: boolean | null;
	isBusy: boolean;
	message: string | null;
	// Role-rule cards content, collapsed behind a section.
	rules?: AgentRoleRule[];
	// Operator-configured extra agent CLIs (Settings → Workspace). Rendered after
	// the built-in codex/claude in the CLI selector. Default [].
	customClients?: CustomAgentClient[];
	// The model the local Devboule orchestrator is configured to run (config
	// .localCoderBackend.model). Surfaced when the "Local (Devboule)" CLI is selected so the
	// launcher is NOT empty (the orchestrator's model is set in Settings, not free-typed).
	// Absent/empty => the orchestrator note tells the user to configure it. Optional so a
	// caller without config still type-checks.
	localCoderModel?: string | null;
	// Launch (app/external) and copy callbacks.
	onLaunch: (input: SpawnLaunchInput) => void;
	onCopyPrompt: (selection: SpawnSelection) => void;
}

export function SpawnPanel({
	projects,
	lockedProjectId = null,
	selectedProjectId,
	onSelectProject,
	tasks,
	projectActive,
	isBusy,
	message,
	rules = [],
	customClients = [],
	localCoderModel = null,
	onLaunch,
	onCopyPrompt,
}: SpawnPanelProps) {
	const [role, setRole] = useState<SpawnRole>("coder");
	// Free-text advisory model hint (the source of truth). Empty = let the agent
	// self-report. Quick-fill suggestion chips (per CLI) write into this field.
	const [model, setModel] = useState<string>("");
	const [taskId, setTaskId] = useState<string>("");
	// The CLI the launch uses: a built-in id ("codex"/"claude") or a configured
	// custom client id. Codex is the default.
	const [client, setClient] = useState<string>("codex");
	// 3b — "Plan first" bias, shown ONLY for the local orchestrator. Default ON:
	// planning-first is the intended UX for the local coder (it plans before acting
	// and surfaces the plan in the Plans tab for approval). The state is kept
	// regardless of the selected client; it is only RENDERED for the orchestrator and
	// only THREADED into the launch for the orchestrator (see `selection` below), so
	// toggling to codex/claude can never carry the flag.
	const [planFirst, setPlanFirst] = useState<boolean>(true);
	// Phase 6 — the project's auto-detected primary persona language (from the backend), an optional
	// per-launch override, and whether the (non-invasive by default) editor row is expanded.
	const [detectedLang, setDetectedLang] = useState<string>("");
	const [langOverride, setLangOverride] = useState<string>("");
	const [langExpanded, setLangExpanded] = useState<boolean>(false);
	// The override selector's languages — DATA-DRIVEN from the persona bundle (skills_lang_catalog)
	// so a newly-added bundle language appears here with no code change. Seeded with the core set as
	// a fallback while the catalog loads / if it errors.
	const [personaLangs, setPersonaLangs] = useState<string[]>([
		...PERSONA_LANGS,
	]);

	// The selector options: built-ins first, then each configured custom client
	// (label shown, id is the value). Deduped/validated upstream; rendered as-is.
	const clientOptions = useMemo(
		() => [
			...BUILTIN_CLIENTS.map((c) => ({ id: c.id, label: c.label })),
			...customClients.map((c) => ({ id: c.id, label: c.label })),
		],
		[customClients],
	);

	// If the selected custom client disappears from config (removed in Settings),
	// fall back to codex so the panel never points at a non-existent client.
	useEffect(() => {
		if (!clientOptions.some((option) => option.id === client)) {
			setClient("codex");
		}
	}, [clientOptions, client]);

	// Phase 6 — detect the project's PRIMARY persona language whenever the project changes, to seed
	// the language indicator + override selector. Best-effort: an error / no working root ⇒ "".
	const activeProjectId = lockedProjectId ?? selectedProjectId;
	useEffect(() => {
		// Reset SYNCHRONOUSLY on project change so a stale override OR a stale detected-language
		// indicator from the previous project can never ride / mislead the new selection (#3); the
		// async detect result then fills in the new project's language.
		setLangOverride("");
		setLangExpanded(false);
		setDetectedLang("");
		if (!activeProjectId) {
			return;
		}
		let alive = true;
		invokeBackendCommand<string>("detect_project_language", {
			projectId: activeProjectId,
		})
			.then((lang) => {
				if (alive) setDetectedLang(lang ?? "");
			})
			.catch(() => {
				if (alive) setDetectedLang("");
			});
		return () => {
			alive = false;
		};
	}, [activeProjectId]);

	// Load the persona-language catalog ONCE (no project needed) so the override selector lists
	// whatever the bundle ships, not a hardcoded set. Best-effort: an error keeps the seeded fallback.
	useEffect(() => {
		let alive = true;
		invokeBackendCommand<{ lang: string }[]>("skills_lang_catalog")
			.then((catalog) => {
				if (alive && Array.isArray(catalog) && catalog.length > 0) {
					setPersonaLangs(catalog.map((entry) => entry.lang));
				}
			})
			.catch(() => {});
		return () => {
			alive = false;
		};
	}, []);

	// Per-CLI quick-fill model suggestions (claude -> opus/sonnet/haiku; orchestrator ->
	// the configured local-coder model when known; everything else -> none — we never
	// invent model names for codex/custom CLIs).
	const modelSuggestions = useMemo(
		() => modelSuggestionsForClient(client, localCoderModel),
		[client, localCoderModel],
	);

	// Switching the CLI clears the model text ONLY if it exactly equals one of the
	// PREVIOUS client's suggestions (so a stale quick-fill suggestion does not stick
	// to a CLI it does not belong to). A hand-typed model is never wiped.
	//
	// L2 — when switching TO the orchestrator and the model field is now empty (either it
	// was blank, or the previous client's suggestion was just cleared), PREFILL it with the
	// configured local-coder model so the launcher surfaces a model instead of being empty.
	// The user can still overwrite it (the orchestrator binary reads the model from config,
	// so this field stays advisory for prompt/fleet display — but it should not read empty).
	//
	// This uses React's documented "adjust state during render" pattern instead of a
	// ref + effect: comparing `client` against the committed `prevClient` STATE during
	// render is correct under StrictMode's double-invoke and under batched setClient
	// calls (a ref written in an effect could desync on a fast claude->A->claude
	// switch and skip the clear). The setState calls run during render and React
	// immediately re-renders without committing the discarded pass, so there is no
	// extra paint and no stale window.
	const [prevClient, setPrevClient] = useState(client);
	if (prevClient !== client) {
		const prevSuggestions = modelSuggestionsForClient(
			prevClient,
			localCoderModel,
		);
		let nextModel = model;
		if (prevSuggestions.includes(model.trim().toLowerCase())) {
			nextModel = "";
		}
		// P1: the orchestrator's model is READ-ONLY = the Settings value (the binary uses that
		// regardless), so force it to the configured local-coder model on switch — not just when
		// empty — so the launch/fleet never carries a stale hand-typed model for the orchestrator.
		const localModel = (localCoderModel ?? "").trim();
		if (client === "orchestrator" && localModel.length > 0) {
			nextModel = localModel;
		}
		if (nextModel !== model) setModel(nextModel);
		setPrevClient(client);
	}

	const launchableTasks = useMemo(
		() => tasks.filter((task) => task.status !== "done"),
		[tasks],
	);
	const selectedTask = useMemo(
		() => tasks.find((task) => task.id === taskId) ?? null,
		[tasks, taskId],
	);

	const disabledReason = spawnDisabledReason({
		projectId: lockedProjectId ?? selectedProjectId,
		projectActive,
		role,
		task: selectedTask,
	});
	const disabled = isBusy || Boolean(disabledReason);

	const selection: SpawnSelection = {
		projectId: lockedProjectId ?? selectedProjectId,
		role,
		model,
		taskId,
		client,
		// 3b — only the orchestrator carries "Plan first"; leave it unset (absent === off
		// per the SpawnSelection contract) for every other client so a stale toggle state
		// never threads into a codex/claude launch, and no consumer mistakes a not-applicable
		// field for a deliberately-disabled one.
		planFirst: client === "orchestrator" ? planFirst : undefined,
		// Phase 6 — the per-launch language-persona override (empty ⇒ the backend auto-detects).
		languageOverride: langOverride.trim().length > 0 ? langOverride : undefined,
	};

	const roleSummary = ROLE_OPTIONS.find((r) => r.id === role)?.summary ?? "";

	return (
		<section
			className="rounded-2xl border border-cream-200 bg-white p-4"
			data-help-title="This launches a new agent."
			data-help-lines="Pick the role, the model (advisory — it seeds the prompt), and the task.|Launch in app runs the agent inside a live in-app terminal you can watch and reply to.|Launch external opens a detached OS console window.|Copy prompt is for a terminal you already have open."
		>
			<div className="mb-3 flex items-center gap-2">
				<Play className="h-4 w-4 text-terracotta" />
				<h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
					Spawn agent
				</h3>
			</div>

			{/* Project (global use only). */}
			{lockedProjectId === null && (
				<div className="mb-3">
					<p className="mb-1.5 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
						Project
					</p>
					<select
						value={selectedProjectId}
						onChange={(e) => onSelectProject?.(e.target.value)}
						data-help-title="This chooses the project the new agent works in."
						data-help-lines="The agent launches at this project's root with its prompt and MCP config.|Only active projects can launch agents.|The task list below is scoped to this project.|Select a specific project to enable launch."
						className="w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-700 outline-none focus:border-terracotta/30"
					>
						<option value="all">Select a project…</option>
						{projects.map((p) => (
							<option key={p.id} value={p.id}>
								{p.title}
							</option>
						))}
					</select>
				</div>
			)}

			{/* Role. */}
			<p className="mb-1.5 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
				Role
			</p>
			<div
				className="mb-1.5 inline-flex rounded-lg border border-cream-200 bg-white p-0.5"
				role="radiogroup"
				aria-label="Agent role"
			>
				{ROLE_OPTIONS.map((option) => {
					const active = role === option.id;
					return (
						<button
							key={option.id}
							type="button"
							role="radio"
							aria-checked={active}
							onClick={() => setRole(option.id)}
							title={option.summary}
							className={`rounded-md px-2.5 py-1 text-[11px] font-semibold transition-colors ${
								active
									? "bg-terracotta/10 text-terracotta"
									: "text-cream-500 hover:text-cream-800"
							}`}
						>
							{option.label}
						</button>
					);
				})}
			</div>
			<p className="mb-3 text-[10px] leading-4 text-cream-400">{roleSummary}</p>

			{/* Model (advisory) — free text is the source of truth; the chips below
          are per-CLI quick-fills. */}
			<p className="mb-1.5 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
				Model <span className="font-normal normal-case">(advisory)</span>
			</p>
			{client === "orchestrator" ? (
				// P1: the local main coder's model is configured in Settings (Local main coder) and the
				// Devboule binary uses THAT model regardless (DEVBOULE_OMLX_MODEL) — so show it READ-ONLY
				// here instead of a free-text field that could drift from the real setting.
				<div className="mb-3 rounded-md border border-cream-200 bg-cream-50 px-3 py-2 text-[12px]">
					<span className="font-mono font-semibold text-cream-800">
						{localCoderModel || "no model configured"}
					</span>
					<span className="ml-2 text-[10px] text-cream-400">
						set in Settings → Local main coder
					</span>
				</div>
			) : (
				<>
					<input
						type="text"
						value={model}
						onChange={(e) => setModel(e.target.value)}
						placeholder="model name (optional)"
						maxLength={64}
						data-help-title="This is the advisory model the new agent should use."
						data-help-lines="It is only a hint that seeds the launch prompt and fleet counts; the agent still reports its real model.|Leave it blank to let the agent self-report.|Quick-fill chips appear for the Claude CLI; for other CLIs, type the model name yourself.|Switching CLI clears the field only if it still held that CLI's suggestion."
						className="mb-2 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-700 outline-none focus:border-terracotta/30"
					/>
					{modelSuggestions.length > 0 && (
						<div className="mb-3 flex flex-wrap gap-1.5">
							{modelSuggestions.map((suggestion) => {
								const active = model.trim().toLowerCase() === suggestion;
								return (
									<button
										key={suggestion}
										type="button"
										onClick={() => setModel(suggestion)}
										className={`rounded-md px-2.5 py-1 text-[11px] font-semibold capitalize transition-colors ${
											active
												? "bg-teal/10 text-teal"
												: "border border-cream-200 bg-white text-cream-500 hover:text-cream-800"
										}`}
									>
										{suggestion}
									</button>
								);
							})}
						</div>
					)}
				</>
			)}

			{/* Task. */}
			<p className="mb-1.5 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
				Task
			</p>
			{launchableTasks.length > 0 ? (
				<select
					value={taskId}
					onChange={(e) => setTaskId(e.target.value)}
					data-help-title="This chooses the exact task the new agent works on."
					data-help-lines="The agent prompt is task-specific, so this decides which work it claims.|Project-level means the agent picks the next task itself via MCP.|Coder targets Todo/WIP/Blocked; verifier targets Review/Blocked.|The selected task id is threaded into the launch and the MCP claim."
					className="mb-3 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-700 outline-none focus:border-terracotta/30"
				>
					<option value="">Project-level (agent picks next task)</option>
					{launchableTasks.map((task) => (
						<option key={task.id} value={task.id}>
							{task.id} / {task.status} / {task.title}
						</option>
					))}
				</select>
			) : (
				<p className="mb-3 text-[10px] leading-4 text-cream-400">
					No open task; the agent will work at project level.
				</p>
			)}

			{/* CLI choice. */}
			<p className="mb-1.5 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
				CLI
			</p>
			<div
				className="mb-3 flex flex-wrap gap-0.5 rounded-lg border border-cream-200 bg-white p-0.5"
				role="radiogroup"
				aria-label="Agent CLI"
			>
				{clientOptions.map((option) => {
					const active = client === option.id;
					return (
						<button
							key={option.id}
							type="button"
							role="radio"
							aria-checked={active}
							onClick={() => setClient(option.id)}
							title={option.label}
							className={`max-w-[180px] truncate rounded-md px-2.5 py-1 text-[11px] font-semibold capitalize transition-colors ${
								active
									? "bg-terracotta/10 text-terracotta"
									: "text-cream-500 hover:text-cream-800"
							}`}
						>
							{option.label}
						</button>
					);
				})}
			</div>
			{client === "orchestrator" && (
				<p className="mb-2 -mt-1.5 text-[10px] leading-4 text-cream-400">
					{orchestratorModelNote(localCoderModel)}
				</p>
			)}
			{/* 3b — "Plan first" toggle, orchestrator-only. Default ON: the local coder
          should plan before acting and surface the plan in the Plans tab for
          approval. Hidden for codex/claude (they have no planner entry). */}
			{client === "orchestrator" && (
				<label className="mb-3 flex cursor-pointer items-start gap-2">
					<input
						type="checkbox"
						checked={planFirst}
						onChange={(e) => setPlanFirst(e.target.checked)}
						aria-label="Plan first"
						data-testid="plan-first-toggle"
						className="mt-0.5 h-3.5 w-3.5 cursor-pointer accent-terracotta"
					/>
					<span className="text-[11px] leading-4 text-cream-600">
						<span className="font-semibold text-cream-800">Plan first</span> —
						the coder produces a task plan and submits it for your approval
						(Plans tab) before doing any other work.
					</span>
				</label>
			)}

			{/* Phase 6 — language-persona indicator. NON-INVASIVE: shown only when a language was
          detected, collapsed by default. Click to expand the override selector when the system
          picked the wrong language. The persona is composed server-side at launch + injected on
          EVERY backend; the override only chooses which language. */}
			{detectedLang && (
				<div className="mb-3">
					<button
						type="button"
						onClick={() => setLangExpanded((v) => !v)}
						aria-expanded={langExpanded}
						data-testid="language-persona-toggle"
						className="inline-flex items-center gap-1 text-[10px] font-semibold uppercase tracking-widest text-cream-400 hover:text-cream-700"
					>
						<span>
							Language persona:{" "}
							<span className="font-mono lowercase text-cream-600">
								{langOverride || detectedLang}
							</span>
							{langOverride && langOverride !== detectedLang
								? " (override)"
								: ""}
						</span>
						<span>{langExpanded ? "▴" : "▾"}</span>
					</button>
					{langExpanded && (
						<div className="mt-1.5 rounded-md border border-cream-200 bg-cream-50 px-3 py-2">
							<label className="block text-[10px] leading-4 text-cream-500">
								The (role × language) coder persona is auto-detected and
								injected on every backend. If it picked the wrong language,
								override it here:
								<select
									value={langOverride}
									onChange={(e) => setLangOverride(e.target.value)}
									data-testid="language-override-select"
									className="mt-1.5 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold lowercase text-cream-700 outline-none focus:border-terracotta/30"
								>
									<option value="">Auto-detected ({detectedLang})</option>
									{personaLangs.map((l) => (
										<option key={l} value={l}>
											{l}
										</option>
									))}
								</select>
							</label>
							<p className="mt-1.5 text-[10px] leading-4 text-cream-400">
								Use “Copy prompt” to see the full prompt with this persona.
							</p>
						</div>
					)}
				</div>
			)}

			{/* Launch actions. */}
			<div className="flex flex-wrap gap-2">
				<button
					type="button"
					onClick={() => onLaunch(buildLaunchInput(selection, "app"))}
					disabled={disabled}
					title={
						disabledReason ?? "Launch the agent inside an in-app terminal."
					}
					data-help-title="This launches the agent inside the app (PTY)."
					data-help-lines="The agent runs under an in-app terminal you can watch live and reply to.|Use it when you want to supervise or answer the agent directly.|The terminal is read-only except via the reply bar.|Stop kills the PTY child cleanly."
					className="inline-flex items-center gap-1.5 rounded-md bg-terracotta px-3 py-1.5 text-[12px] font-semibold text-white hover:bg-terracotta/90 disabled:opacity-60"
				>
					<SquareTerminal className="h-3.5 w-3.5" aria-hidden />
					Launch in app
				</button>
				<button
					type="button"
					onClick={() => onLaunch(buildLaunchInput(selection, "external"))}
					disabled={disabled}
					title={
						disabledReason ?? "Launch the agent in a detached console window."
					}
					data-help-title="This launches the agent in an external console."
					data-help-lines="The app opens a dedicated OS console window for the agent.|Use it when you prefer a standalone terminal outside the app.|The agent still uses MCP for project updates.|Open CLI later focuses that window."
					className="inline-flex items-center gap-1.5 rounded-md bg-teal px-3 py-1.5 text-[12px] font-semibold text-white hover:bg-teal/90 disabled:opacity-60"
				>
					<Play className="h-3.5 w-3.5" aria-hidden />
					Launch external
				</button>
				<button
					type="button"
					onClick={() => onCopyPrompt(selection)}
					disabled={disabled}
					title={
						disabledReason ??
						"Copy the role/task prompt for a terminal you already have open."
					}
					data-help-title="This copies a manual prompt for the selected role and task."
					data-help-lines="Manual prompt copy is for terminals you open yourself.|The role/task prompt tells the agent how to read the project and report through MCP.|It does not start a process or inject token profiles by itself.|Prefer app/external launch when provider tokens or root setup matter."
					className="inline-flex items-center gap-1.5 rounded-md border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-600 hover:text-terracotta disabled:opacity-60"
				>
					<Copy className="h-3.5 w-3.5" aria-hidden />
					Copy prompt
				</button>
			</div>

			{disabledReason && (
				<p className="mt-2 text-[10px] font-semibold text-amber-dark">
					{disabledReason}
				</p>
			)}
			{message && (
				<p className="mt-2 rounded-md bg-sage/10 px-2 py-1 text-[10px] font-semibold text-sage-dark">
					{message}
				</p>
			)}

			{/* Full role rules behind a collapsible (the three big cards are gone). */}
			{rules.length > 0 && (
				<div className="mt-3">
					<CollapsibleSection
						icon={ShieldQuestion}
						title="Role rules"
						purpose="What each role may and may not do."
						helpTitle="Role rules are the contract between the Kanban and CLI agents."
						helpLines="Orchestrators plan and coordinate; coders modify code and scoped provider surfaces; verifiers read and audit only.|Allowed tools decide what MCP/provider actions the role can use.|Forbidden items are safety rails and should block project closure if violated.|The mandate lines are what every agent of that role must DO."
					>
						<div className="space-y-3">
							{rules.map((rule) => (
								<div
									key={rule.role}
									className="rounded-lg border border-cream-200 bg-cream-50 p-3"
								>
									<p className="text-[12px] font-semibold capitalize text-cream-800">
										{rule.role}
									</p>
									<p className="mt-1 text-[11px] leading-5 text-cream-600">
										{rule.summary}
									</p>
									{rule.allowedTools.length > 0 && (
										<p className="mt-1 text-[10px] text-cream-400">
											Tools: {rule.allowedTools.slice(0, 6).join(", ")}
										</p>
									)}
								</div>
							))}
						</div>
					</CollapsibleSection>
				</div>
			)}
		</section>
	);
}

export default SpawnPanel;
