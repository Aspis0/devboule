import { Check } from "lucide-react";

interface PlannerControlsProps {
	// Role untangle (P6b): the hand-off is AGNOSTIC — it always targets the Main coder
	// ROLE. `coders` is the list of Main-coder-capable engines the per-project override can
	// pick from; the ENGINE is a per-project choice, not a role pick. `mainCoderOverride` is
	// the project's override (null = use the global Settings default, whose label is
	// `defaultCoderLabel`). onCoderChange("") clears the override back to Default.
	coders: { id: string; label: string }[];
	mainCoderOverride: string | null;
	defaultCoderLabel: string;
	onCoderChange: (id: string) => void;
	autoCreate: boolean;
	onAutoCreateToggle: () => void;
	// B10: explicit "Create plan" trigger. The orchestrator discusses first (no
	// auto-plan on turn 1); the user clicks this when the conversation has converged
	// to draft the plan + create the tasks. Enabled only while an orchestrator is live.
	onCreatePlan?: () => void;
	canCreatePlan?: boolean;
}

export function PlannerControls(props: PlannerControlsProps) {
	const {
		coders,
		mainCoderOverride,
		defaultCoderLabel,
		onCoderChange,
		autoCreate,
		onAutoCreateToggle,
		onCreatePlan,
		canCreatePlan,
	} = props;

	return (
		<div
			style={{
				display: "flex",
				alignItems: "center",
				gap: 8,
				flexWrap: "wrap",
				padding: "9px 12px",
				background: "#F4F0E9",
				border: "1px solid #E4DDD0",
				borderRadius: 10,
			}}
		>
			<span
				className="pp-mono"
				style={{
					fontSize: 9.5,
					letterSpacing: ".14em",
					color: "#A89F90",
					fontWeight: 600,
					cursor: "help",
				}}
				data-help-title="After the plan, the orchestrator hands the tasks off to the Main coder."
				data-help-lines="The Main coder is the ROLE that builds the plan into code — configure its engine in Settings → Roles.|This dropdown is a PER-PROJECT override: one project can build with Codex, another with Claude or a local model.|Default follows the Settings → Roles Main coder; pick an engine here to override it for THIS project only.|OpenAI is also available as a cloud CLI option.|The Main coder delegates one-shot edits to minis and moves tasks toward Review."
			>
				HAND OFF TO
			</span>

			{/* Agnostic target: always the Main coder role. */}
			<span
				style={{
					display: "inline-flex",
					alignItems: "center",
					gap: 6,
					padding: "5px 11px",
					borderRadius: 8,
					fontSize: 11.5,
					fontWeight: 600,
					border: "1px solid #C0894F",
					background: "#fff",
					color: "#2A2621",
				}}
			>
				<span
					style={{
						width: 16,
						height: 16,
						background: "#F1E4D2",
						color: "#9A6A2E",
						borderRadius: 4,
						fontSize: 9,
						fontWeight: 700,
						display: "flex",
						alignItems: "center",
						justifyContent: "center",
					}}
				>
					M
				</span>
				Main coder
			</span>

			{/* Small per-project engine dropdown (Default = the Settings → Roles Main coder). */}
			<select
				value={mainCoderOverride ?? ""}
				onChange={(e) => onCoderChange(e.target.value)}
				title="Which engine the Main coder runs for THIS project. Default follows Settings → Roles."
				style={{
					padding: "5px 9px",
					borderRadius: 8,
					fontSize: 11.5,
					fontWeight: 600,
					border: "1px solid #E4DDD0",
					background: "#fff",
					color: "#2A2621",
					cursor: "pointer",
				}}
			>
				<option value="">Default · {defaultCoderLabel}</option>
				{coders.map((c) => (
					<option key={c.id} value={c.id}>
						{c.label}
					</option>
				))}
			</select>

			{onCreatePlan && (
				<button
					onClick={onCreatePlan}
					disabled={!canCreatePlan}
					title={
						canCreatePlan
							? "Draft the plan from the conversation and create the tasks on the board."
							: "Start the orchestrator (describe a goal) before creating the plan."
					}
					data-help-title="Create the plan when the conversation has converged."
					data-help-lines="The orchestrator discusses the goal with you first — it does NOT plan on the first message.|Click this when you're happy with the direction: it asks the orchestrator to draft the plan and create the Kanban tasks.|Use the auto-create toggle instead if you want it to plan + create eagerly without waiting for this click."
					style={{
						marginLeft: "auto",
						display: "inline-flex",
						alignItems: "center",
						gap: 6,
						padding: "5px 13px",
						borderRadius: 8,
						fontSize: 11.5,
						fontWeight: 700,
						cursor: canCreatePlan ? "pointer" : "not-allowed",
						border: "1px solid #C0894F",
						background: canCreatePlan ? "#C0894F" : "#EDE6DA",
						color: canCreatePlan ? "#fff" : "#B3AB9C",
					}}
				>
					Create plan
				</button>
			)}

			<button
				onClick={onAutoCreateToggle}
				style={{
					marginLeft: onCreatePlan ? 0 : "auto",
					display: "inline-flex",
					alignItems: "center",
					gap: 6,
					padding: "5px 11px",
					borderRadius: 8,
					fontSize: 11,
					fontWeight: 600,
					cursor: "pointer",
					...(autoCreate
						? {
								border: "1px solid #B7D9A8",
								background: "#F0F6EC",
								color: "#4E7C3C",
							}
						: {
								border: "1px solid #E4DDD0",
								background: "#fff",
								color: "#9c9488",
							}),
				}}
				title={
					autoCreate
						? "When you approve the plan, its tasks are created on the board automatically."
						: "The plan is drafted but its tasks are not created — you create them."
				}
			>
				{autoCreate && <Check size={12} />}
				auto-create tasks: {autoCreate ? "on" : "off"}
			</button>
		</div>
	);
}
