import { useRef, useEffect, useState, useMemo, useCallback } from "react";
import { invokeBackendCommand } from "../../../context/AppContext";
import gsap from "gsap";
import { Search, ListOrdered, LayoutDashboard, ChevronDown, ChevronRight } from "lucide-react";
import "./planner.css";
import { useStageRotation } from "./useStageRotation";
import type {
	PlanCard,
	StagePage,
	StageFinding,
	PlannerMessage,
} from "./plannerModel";
import { doubtTouchesCard } from "./plannerModel";
import type { QuestionEntry } from "../../agents/agentConsoleModel";
import { StageWebsearch } from "./StageWebsearch";
import { StagePlan } from "./StagePlan";
import { StageDesign } from "./StageDesign";
import { DoubtPanel } from "./DoubtPanel";
import { PlannerChat } from "./PlannerChat";
import { PlannerControls } from "./PlannerControls";
import type { SlashResult } from "../../../hooks/useSlashCommands";

interface PlannerPlanModeProps {
	goal: string | null;
	contextLabel: string;
	plannerModelLabel: string;
	live: boolean;
	planCards: PlanCard[];
	/** Title of the pi plan (when no project tasks exist yet). Forwarded to StagePlan.
	 *  Absent when real project tasks are driving the view. */
	planTitle?: string;
	/** Optional notes from the pi plan (when no project tasks exist yet). Forwarded to
	 *  StagePlan. Absent when real project tasks are driving the view. */
	planNotes?: string;
	// Kairion (ORCHESTRATOR-ONLY): the orchestrator's open doubts. Empty => the Plan view
	// renders task-cards only (degrades to a plain plan with no left doubt panel).
	questions: QuestionEntry[];
	pages: StagePage[];
	findings: StageFinding[];
	webMode: "auto" | "manual";
	onWebModeChange: (m: "auto" | "manual") => void;
	onManualSearch: (q: string) => void;
	design: {
		name: string;
		version: string | null;
		ago: string | null;
		thumbnailUri: string | null;
		/** Registry entry id — forwarded to StageDesign for the task-link command. */
		id?: string;
		/** Phase 3: present when the registry entry is an interactive artifact. */
		kind?: import("../../../types/design").ArtifactKind;
		/** Phase 3: the registry entry id — used to build the artifact:// URL. */
		artifactId?: string;
		/** Phase 4 (Fix 3): device-frame skin stored on the registry entry. Forwarded
		 *  to StageDesign so effectiveFrameKind resolves the user's stored frame before
		 *  the heuristic. Absent ⇒ inferred. */
		frame?: import("../../../types/design").ArtifactFrameKind;
		/** OPTIONAL linked plan-task number (1-based). Absent ⇒ unlinked. */
		linkedTaskN?: number;
	} | null;
	linkedTask: number | null;
	onOpenInDesign: () => void;
	/** Phase 3: absolute project root path forwarded to StageDesign for generation. */
	projectRoot: string | null;
	/** Phase 3: called when a user-triggered design generation completes. */
	onGenerated: () => void;
	messages: PlannerMessage[];
	awaitingReply: boolean;
	/** D4: composer chrome (delivery/stall/launch feedback) — see PlannerChat.banner. */
	banner?: string | null;
	onSend: (text: string) => void;
	/** Esc in the chat: interrupt the orchestrator's in-flight turn (cloud duplex). */
	onInterrupt?: () => void;
	/** Slash-command result (model/agent switch, stop, help), forwarded to PlannerChat. */
	onSlashCommand?: (result: SlashResult) => void;
	/** Reset the orchestrator chat: stop session, wipe transcript, start clean. */
	onResetChat?: () => void;
	// Orchestrator backend selector — who you TALK TO (the planner). Replaces the redundant
	// status strip (searching/planning/designing duplicated the view tabs). The active one
	// pulses. Local = Devboule engine (WHO); Claude/Codex/OpenAI = external CLIs.
	// OpenRouter/oMLX placement is Settings → Roles, not these chips.
	orchestrators: { id: string; label: string; disabled?: boolean }[];
	orchestratorId: string;
	onOrchestratorChange: (id: string) => void;
	// Hand-off + auto-create controls (preserved from the old composer — never strip choices).
	// Role untangle (P6b): the hand-off targets the Main coder ROLE; `coders` are the engines
	// the PER-PROJECT override can pick, `mainCoderOverride` is this project's override (null =
	// the Settings → Roles default labelled `defaultCoderLabel`).
	coders: { id: string; label: string }[];
	mainCoderOverride: string | null;
	defaultCoderLabel: string;
	onCoderChange: (id: string) => void;
	autoCreate: boolean;
	onAutoCreateToggle: () => void;
	// B10: explicit "Create plan" trigger (discuss-first; plan on demand).
	onCreatePlan?: () => void;
	canCreatePlan?: boolean;
}

export function PlannerPlanMode(props: PlannerPlanModeProps) {
	const {
		goal,
		contextLabel,
		plannerModelLabel,
		live,
		planCards,
		planTitle,
		planNotes,
		questions,
		pages,
		findings,
		webMode,
		onWebModeChange,
		onManualSearch,
		design,
		linkedTask,
		onOpenInDesign,
		projectRoot,
		onGenerated,
		messages,
		awaitingReply,
		banner,
		onSend,
		onInterrupt,
		onSlashCommand,
	onResetChat,
		orchestrators,
		orchestratorId,
		onOrchestratorChange,
		coders,
		mainCoderOverride,
		defaultCoderLabel,
		onCoderChange,
		autoCreate,
		onAutoCreateToggle,
		onCreatePlan,
		canCreatePlan,
	} = props;

	// Phase 5: hold rotation while an interactive artifact is actively shown inside
	// StageDesign. The signal originates inside StageDesign and never propagates
	// to PlannerPlanModeProps — it is fully self-contained here.
	const [artifactActive, setArtifactActive] = useState(false);
	const handleArtifactActiveChange = useCallback((active: boolean) => {
		setArtifactActive(active);
	}, []);

	// Stage drawer: collapsed by default when idle AND empty; opens itself when there
	// is something to show (live, artifact, or any stage content). A manual
	// collapse/expand always wins over the auto-expand — until fresh content arrives.
	// `stageHasContent`: true if ANY of the three stage views carries content.
	const stageHasContent =
		pages.length > 0 ||
		findings.length > 0 ||
		planCards.length > 0 ||
		questions.length > 0 ||
		design != null;
	const [stageExpanded, setStageExpanded] = useState(
		live || artifactActive || stageHasContent,
	);
	// Whether the user has taken manual control of the drawer. Once true, live/artifact
	// auto-expands no-op — but an incoming DOUBT still forces open (see below).
	const userToggled = useRef(false);

	const toggleStage = useCallback(() => {
		userToggled.current = true;
		setStageExpanded((v) => !v);
	}, []);

	// Task-link: derive { n, title } list from planCards for the StageDesign selector.
	const taskOptions = useMemo(
		() => planCards.map((c) => ({ n: c.n, title: c.title })),
		[planCards],
	);

	// onLinkTask: invoke the backend command and trigger a design reload via onGenerated.
	// design?.id is the registry entry id (set in ProjectsView when loading the entry).
	// Errors propagate to the caller (StageDesign) so it can surface them near the control.
	// onGenerated is only called on success; a failure leaves the UI consistent without reload.
	const handleLinkTask = useCallback(
		async (n: number | null): Promise<void> => {
			const entryId = design?.id;
			if (!entryId) return;
			await invokeBackendCommand("design_registry_set_linked_task", {
				id: entryId,
				linkedTaskN: n,
			});
			// Re-load the design entry so the new linkedTaskN is reflected in the UI.
			onGenerated();
		},
		[design?.id, onGenerated],
	);

	const { view, auto, pick, toggleAuto } = useStageRotation(
		3800,
		live,
		artifactActive,
	);
	const ref = useRef<HTMLDivElement>(null);

	// Click a collapsed tab label: select that view AND open the drawer (a click on an
	// inert-looking collapsed label must not silently do nothing).
	const selectAndExpand = useCallback(
		(v: "exa" | "plan" | "design") => {
			pick(v);
			userToggled.current = true;
			setStageExpanded(true);
		},
		[pick],
	);

	// Auto-expand the drawer when something worth showing appears. Doubts are the HARD
	// exception: an incoming doubt MUST always surface — even if the user collapsed the
	// drawer by hand — unanswered doubts are never hidden by a collapsed drawer.
	// Same convention for plan-content: a fresh pi plan (or any stage content) arriving
	// mid-session forces the drawer open, unless the user has manually toggled (userToggled).
	const prevLive = useRef(live);
	const prevArtifact = useRef(artifactActive);
	const prevQuestionsLen = useRef(questions.length);
	const prevHasContent = useRef(stageHasContent);
	useEffect(() => {
		if (questions.length > prevQuestionsLen.current) {
			setStageExpanded(true);
		} else if (!userToggled.current) {
			if (live && !prevLive.current) setStageExpanded(true);
			if (artifactActive && !prevArtifact.current) setStageExpanded(true);
			if (stageHasContent && !prevHasContent.current) setStageExpanded(true);
		}
		prevLive.current = live;
		prevArtifact.current = artifactActive;
		prevQuestionsLen.current = questions.length;
		prevHasContent.current = stageHasContent;
	}, [live, artifactActive, questions.length, stageHasContent]);

	// Kairion doubt<->task link: hovering a doubt highlights its task card(s) and vice-versa.
	// One source of hover at a time; the derived Sets feed both panels.
	const [hoveredDoubtId, setHoveredDoubtId] = useState<string | null>(null);
	const [hoveredCardN, setHoveredCardN] = useState<number | null>(null);

	const highlightedTaskNums = useMemo(() => {
		const out = new Set<number>();
		if (hoveredDoubtId == null) return out;
		const q = questions.find((x) => x.id === hoveredDoubtId);
		if (!q) return out;
		for (const card of planCards) {
			if (doubtTouchesCard(q.affects, card)) out.add(card.n);
		}
		return out;
	}, [hoveredDoubtId, questions, planCards]);

	const highlightedDoubtIds = useMemo(() => {
		const out = new Set<string>();
		if (hoveredCardN == null) return out;
		const card = planCards.find((c) => c.n === hoveredCardN);
		if (!card) return out;
		for (const q of questions) {
			if (doubtTouchesCard(q.affects, card)) out.add(q.id);
		}
		return out;
	}, [hoveredCardN, questions, planCards]);

	useEffect(() => {
		const el = ref.current;
		if (!el) return;
		// fromTo with EXPLICIT end values (not gsap.from, which reads the current value
		// as the destination): under React StrictMode the effect runs twice and the
		// cleanup kills the first tween mid-flight at opacity:0 — gsap.from would then
		// animate 0 -> 0 and leave the panel invisible. fromTo always ends visible.
		const tween = gsap.fromTo(
			el,
			{ scaleY: 0.6, opacity: 0 },
			{
				scaleY: 1,
				opacity: 1,
				transformOrigin: "top",
				duration: 0.35,
				ease: "power2.out",
			},
		);
		return () => {
			tween.kill();
			// Guarantee the panel is left visible even if killed mid-flight.
			gsap.set(el, { clearProps: "opacity,transform" });
		};
	}, []);

	return (
		<div
			ref={ref}
			className="pp-root rounded-2xl border border-cream-200 bg-white shadow-sm"
			style={{ padding: 16 }}
		>
			<div
				style={{
					display: "flex",
					flexDirection: "column",
					gap: 13,
				}}
			>
				{/* 1) Goal echo — shown only while a REAL goal is being planned. No fake
        placeholder when idle (the chat composer carries the "describe a goal" affordance). */}
				{goal && (
					<div
						style={{
							display: "flex",
							alignItems: "center",
							gap: 10,
							padding: "10px 12px",
							background: "#FCFAF6",
							border: "1px solid #E4DDD0",
							borderRadius: 12,
						}}
					>
						<div
							className="pp-mono"
							style={{
								fontSize: 12,
								fontWeight: 600,
								color: "#9A6A2E",
								background: "#F1E4D2",
								border: "1px solid #E6D3BB",
								padding: "4px 11px",
								borderRadius: 8,
							}}
						>
							plan
						</div>
						<span
							style={{
								flex: 1,
								fontSize: 13.5,
								color: "#2A2621",
								whiteSpace: "nowrap",
								overflow: "hidden",
								textOverflow: "ellipsis",
							}}
						>
							{goal}
						</span>
						{contextLabel && (
							<div
								className="pp-mono"
								style={{
									fontSize: 11,
									color: "#7c766b",
									background: "#fff",
									border: "1px solid #E9E3D8",
									padding: "4px 8px",
									borderRadius: 7,
								}}
							>
								{contextLabel}
							</div>
						)}
					</div>
				)}

				{/* 2) Chat (local Stage / pre-launch composer) — the first substantial block.
            The orchestrator selector now lives inside PlannerChat's own header. */}
				<PlannerChat
					messages={messages}
					modelLabel={`Orchestrator · ${plannerModelLabel}`}
					live={live}
					awaitingReply={awaitingReply}
					banner={banner}
					onSend={onSend}
					onInterrupt={onInterrupt}
					onSlashCommand={onSlashCommand}
					onResetChat={onResetChat}
					orchestrators={orchestrators}
					orchestratorId={orchestratorId}
					onOrchestratorChange={onOrchestratorChange}
				/>

				{/* 3) Stage Container — cloud duplex orchestrators drive the SAME Stage as the
            local one. Collapsed by default; opens itself when there is something to show
            (live, artifact, doubts, or any stage content). The collapsed slim header still
            exposes the tab labels (with live content-count badges) and the Auto toggle. */}
				{stageExpanded ? (
					<div
						style={{
							background: "#FAF7F1",
							border: "1px solid #ECE6DB",
							borderRadius: 12,
							padding: 13,
							height: 316,
							display: "flex",
							flexDirection: "column",
						}}
					>
						{/* Tab Row */}
						<div
							style={{
								display: "flex",
								alignItems: "center",
								gap: 9,
								marginBottom: 12,
							}}
						>
							<div
								style={{
									display: "flex",
									background: "#F1E9DC",
									borderRadius: 10,
									padding: 3,
								}}
							>
								{[
									{ v: "exa" as const, icon: Search, label: "Websearch" },
									{ v: "plan" as const, icon: ListOrdered, label: "Plan" },
									{
										v: "design" as const,
										icon: LayoutDashboard,
										label: "Design",
									},
								].map(({ v, icon: Icon, label }) => {
									const isActive = view === v;
									return (
										<div
											key={v}
											onClick={() => pick(v)}
											className="pp-mono"
											style={{
												display: "flex",
												alignItems: "center",
												gap: 6,
												padding: "6px 12px",
												borderRadius: 8,
												fontSize: 12,
												fontWeight: 600,
												cursor: "pointer",
												background: isActive ? "#C8945C" : "transparent",
												color: isActive ? "#FBF6EF" : "#9c9488",
											}}
										>
											<Icon size={13} />
											<span>{label}</span>
										</div>
									);
								})}
							</div>

							{/* Auto Toggle */}
							<div
								onClick={toggleAuto}
								className="pp-mono"
								style={{
									marginLeft: "auto",
									fontSize: 9,
									cursor: "pointer",
									padding: "3px 8px",
									borderRadius: 7,
									display: "flex",
									alignItems: "center",
									gap: 5,
									...(auto
										? { color: "#B3AB9C" }
										: {
												color: "#9A6A2E",
												background: "#F1E4D2",
												border: "1px solid #E6D3BB",
											}),
								}}
							>
								<div
									style={{
										width: 5,
										height: 5,
										borderRadius: "50%",
										background: auto ? "#B3AB9C" : "#C0894F",
									}}
								/>
								<span>{auto ? "auto" : "paused · resume"}</span>
							</div>

							{/* Collapse chevron */}
							<button
								type="button"
								onClick={toggleStage}
								title="Collapse stage panels"
								style={{
									marginLeft: 8,
									border: "1px solid #E6D3BB",
									background: "#F1E4D2",
									borderRadius: 7,
									padding: "3px 6px",
									cursor: "pointer",
									display: "flex",
									alignItems: "center",
								}}
							>
								<ChevronDown size={13} />
							</button>
						</div>

						{/* Active View */}
						<div style={{ flex: 1, overflow: "hidden" }}>
							{view === "exa" && (
								<StageWebsearch
									pages={pages}
									findings={findings}
									mode={webMode}
									live={live}
									onModeChange={onWebModeChange}
									onManualSearch={onManualSearch}
								/>
							)}
							{view === "plan" &&
								(questions.length > 0 ? (
									// Kairion two-panel Plan view: LEFT = open doubts, RIGHT = the firming plan.
									<div style={{ display: "flex", height: "100%", minHeight: 0 }}>
										<DoubtPanel
											questions={questions}
											onSend={onSend}
											highlightedDoubtIds={highlightedDoubtIds}
											onHoverDoubt={setHoveredDoubtId}
										/>
										<div
											className="pp-scroll"
											style={{
												flex: 1,
												minWidth: 0,
												overflowY: "auto",
												paddingLeft: 11,
											}}
										>
											<StagePlan
												cards={planCards}
												singleColumn
												highlightedTaskNums={highlightedTaskNums}
												onHoverTask={setHoveredCardN}
												planTitle={planTitle}
												planNotes={planNotes}
											/>
										</div>
									</div>
								) : (
									// Degrade: no doubts -> the plan task-cards alone, exactly as before.
									<StagePlan cards={planCards} planTitle={planTitle} planNotes={planNotes} />
								))}
							{view === "design" && (
								<StageDesign
									design={design}
									linkedTask={linkedTask}
									onOpenInDesign={onOpenInDesign}
									projectRoot={projectRoot}
									onGenerated={onGenerated}
									onArtifactActiveChange={handleArtifactActiveChange}
									tasks={taskOptions}
									onLinkTask={handleLinkTask}
								/>
							)}
						</div>
					</div>
				) : (
					/* Collapsed slim header: tab labels with live content-count badges, the
					   Auto toggle, and an expand chevron. Clicking a tab both selects it and
					   opens the drawer (selectAndExpand), so no click silently does nothing.
					   Badge counts: Websearch = pages + findings (combined source yield);
					   Plan = planCards (+ " · N doubts" when Kairion has open doubts, the most
					   urgent content); Design = 1 when an artifact is active, else 0. */
					<div
						style={{
							display: "flex",
							alignItems: "center",
							gap: 9,
							background: "#FAF7F1",
							border: "1px solid #ECE6DB",
							borderRadius: 12,
							padding: "8px 12px",
						}}
					>
						<div
							style={{
								display: "flex",
								background: "#F1E9DC",
								borderRadius: 10,
								padding: 3,
								gap: 2,
							}}
						>
							<button
								type="button"
								onClick={() => selectAndExpand("exa")}
								className="pp-mono"
								style={{
									display: "flex",
									alignItems: "center",
									gap: 5,
									padding: "5px 10px",
									borderRadius: 8,
									fontSize: 11,
									fontWeight: 600,
									cursor: "pointer",
									background: view === "exa" ? "#C8945C" : "transparent",
									color: view === "exa" ? "#FBF6EF" : "#9c9488",
									border: "1px solid transparent",
								}}
							>
								Websearch ({pages.length + findings.length})
							</button>
							<button
								type="button"
								onClick={() => selectAndExpand("plan")}
								className="pp-mono"
								style={{
									display: "flex",
									alignItems: "center",
									gap: 5,
									padding: "5px 10px",
									borderRadius: 8,
									fontSize: 11,
									fontWeight: 600,
									cursor: "pointer",
									background: view === "plan" ? "#C8945C" : "transparent",
									color: view === "plan" ? "#FBF6EF" : "#9c9488",
									border: "1px solid transparent",
								}}
							>
								{questions.length > 0
									? `Plan (${planCards.length} · ${questions.length} doubts)`
									: `Plan (${planCards.length})`}
							</button>
							<button
								type="button"
								onClick={() => selectAndExpand("design")}
								className="pp-mono"
								style={{
									display: "flex",
									alignItems: "center",
									gap: 5,
									padding: "5px 10px",
									borderRadius: 8,
									fontSize: 11,
									fontWeight: 600,
									cursor: "pointer",
									background: view === "design" ? "#C8945C" : "transparent",
									color: view === "design" ? "#FBF6EF" : "#9c9488",
									border: "1px solid transparent",
								}}
							>
								Design ({design ? 1 : 0})
							</button>
						</div>

						{/* Auto Toggle */}
						<div
							onClick={toggleAuto}
							className="pp-mono"
							style={{
								marginLeft: "auto",
								fontSize: 9,
								cursor: "pointer",
								padding: "3px 8px",
								borderRadius: 7,
								display: "flex",
								alignItems: "center",
								gap: 5,
								...(auto
									? { color: "#B3AB9C" }
									: {
											color: "#9A6A2E",
											background: "#F1E4D2",
											border: "1px solid #E6D3BB",
										}),
							}}
						>
							<div
								style={{
									width: 5,
									height: 5,
									borderRadius: "50%",
									background: auto ? "#B3AB9C" : "#C0894F",
								}}
							/>
							<span>{auto ? "auto" : "paused · resume"}</span>
						</div>

						{/* Expand chevron */}
						<button
							type="button"
							onClick={toggleStage}
							title="Expand stage panels"
							style={{
								marginLeft: 8,
								border: "1px solid #E6D3BB",
								background: "#F1E4D2",
								borderRadius: 7,
								padding: "3px 6px",
								cursor: "pointer",
								display: "flex",
								alignItems: "center",
							}}
						>
							<ChevronRight size={13} />
						</button>
					</div>
				)}

				{/* 4) Hand-off + auto-create controls (preserved choices) */}
				<PlannerControls
					coders={coders}
					mainCoderOverride={mainCoderOverride}
					defaultCoderLabel={defaultCoderLabel}
					onCoderChange={onCoderChange}
					autoCreate={autoCreate}
					onAutoCreateToggle={onAutoCreateToggle}
					onCreatePlan={onCreatePlan}
					canCreatePlan={canCreatePlan}
				/>
			</div>
		</div>
	);
}
