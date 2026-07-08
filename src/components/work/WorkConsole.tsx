import "./work.css";
import {
	Suspense,
	lazy,
	useCallback,
	useEffect,
	useMemo,
	useRef,
	useState,
} from "react";
import { buildWorkConsoleModel, findWorkNode } from "./workConsoleModel";
import { LivingPlan } from "./LivingPlan";
import { FocusStage } from "./FocusStage";
import { agentChannel, type CommsDirection } from "./agentChannel";
import { isMiniManagedSession } from "../projects/projectWorkspaceModel";
import { stripSpoofChars } from "../agents/attentionNotifier";
import { useAgentConsole } from "../agents/useAgentConsole";
import { invokeBackendCommand } from "../../context/AppContext";
import type { AgentSession, ProjectTask } from "../../types/backend";
import type { SlashResult } from "../../hooks/useSlashCommands";
import { Bot, Square } from "lucide-react";

const AgentTerminalViewer = lazy(() =>
	import("../agents/AgentTerminalViewer").then((m) => ({
		default: m.AgentTerminalViewer,
	})),
);

export interface WorkConsoleProps {
	sessions: AgentSession[];
	tasks: ProjectTask[];
	projectId: string;
	ptyAgentIds: Set<string>;
	selectedAgentId: string | null;
	onSelectAgent: (agentId: string) => void;
	readOnly?: boolean;
	dirtyAgentIds?: Set<string>;
}

export function WorkConsole(props: WorkConsoleProps) {
	const {
		sessions,
		tasks,
		projectId,
		ptyAgentIds,
		selectedAgentId,
		onSelectAgent,
		readOnly,
		dirtyAgentIds,
	} = props;

	const model = useMemo(
		() => buildWorkConsoleModel({ sessions, tasks, projectId }),
		[sessions, tasks, projectId],
	);

	const selectedNode = selectedAgentId
		? findWorkNode(model, selectedAgentId)
		: null;

	const [view, setView] = useState<"activity" | "raw">("activity");
	// Reset to Activity when the selection changes, so a Raw view never leaks across agents.
	useEffect(() => {
		setView("activity");
	}, [selectedAgentId]);

	const activity = useAgentConsole(selectedAgentId);

	// isPty gates the RAW terminal mount (any app-hosted PTY agent); the COMMS channel is
	// chosen by whether the agent is mini_coder-managed (local) vs a cloud PTY worker.
	const isPty = selectedNode ? ptyAgentIds.has(selectedNode.agentId) : false;
	const selectedSession = selectedAgentId
		? (sessions.find((s) => s.agentId === selectedAgentId) ?? null)
		: null;
	const miniManaged = selectedSession
		? isMiniManagedSession(selectedSession)
		: true;
	const pendingQuestion = selectedNode?.pendingQuestion
		? stripSpoofChars(selectedNode.pendingQuestion)
		: null;

	const dispatch = (text: string, dir: CommsDirection) => {
		const t = text.trim();
		if (!t || !selectedNode) return;
		const ch = agentChannel(selectedNode, { miniManaged }, dir);
		if (!ch) return;
		void invokeBackendCommand(ch.command, ch.buildArgs(t)).catch(() => {});
	};

	const onSendMessage = (t: string) => dispatch(t, "message");
	const onAnswer = (t: string) => dispatch(t, "answer");

	const QUICK = {
		redo: "Redo this round.",
		narrow: "Narrow the scope to the current file only.",
		pause: "Pause after the current step.",
	};
	const onQuickAction = (a: "redo" | "narrow" | "pause") =>
		dispatch(QUICK[a], "message");

	// Slash-command results from the FocusStage composer. The focused-work composer
	// only owns one meaningful action here — stopping the focused agent — so stop
	// maps to stop_agent; model/agent/help switches are Orchestrator concerns
	// handled in ProjectsView's PlannerChat and are no-ops in this console.
	const onSlashCommand = useCallback(
		(result: SlashResult) => {
			switch (result.action) {
				case "stopSession":
					if (selectedNode) {
						void invokeBackendCommand("stop_agent", {
							agentId: selectedNode.agentId,
						}).catch(() => {});
					}
					break;
				default:
					break;
			}
		},
		[selectedNode],
	);

	const rawSlot =
		isPty && selectedNode ? (
			<Suspense
				fallback={
					<div
						style={{
							padding: 24,
							textAlign: "center",
							color: "#9c9488",
							fontSize: 12,
						}}
					>
						Loading terminal…
					</div>
				}
			>
				<AgentTerminalViewer
					key={selectedNode.agentId}
					agentId={selectedNode.agentId}
				/>
			</Suspense>
		) : (
			<div
				style={{
					display: "flex",
					height: "100%",
					alignItems: "center",
					justifyContent: "center",
					color: "#9c9488",
					fontSize: 12,
					textAlign: "center",
					padding: 16,
				}}
			>
				This agent runs in an external console — no in-app terminal to show.
			</div>
		);

	// ---- Phase 1: pi sidecar alpha trigger ----------------------------------
	const [piSessionId, setPiSessionId] = useState<string | null>(null);
	const [piPrompt, setPiPrompt] = useState("");
	const [piLoading, setPiLoading] = useState(false);
	const piActivity = useAgentConsole(piSessionId);
	const piInputRef = useRef<HTMLInputElement>(null);

	const handleStartPiSession = useCallback(async () => {
		const text = piPrompt.trim();
		if (!text || piLoading) return;
		setPiLoading(true);
		try {
			const result = await invokeBackendCommand<{
				sessionId: string;
				isNew: boolean;
			}>("spike_pi_prompt", { text, sessionId: piSessionId });
			setPiSessionId(result.sessionId);
			setPiPrompt("");
		} catch (err) {
			console.error("[pi-sidecar] Failed to start session:", err);
		} finally {
			setPiLoading(false);
		}
	}, [piPrompt, piSessionId, piLoading]);

	const handleStopPiSession = useCallback(async () => {
		if (!piSessionId) return;
		try {
			await invokeBackendCommand<boolean>("spike_pi_stop", {
				sessionId: piSessionId,
			});
		} catch {
			// best-effort
		}
		setPiSessionId(null);
	}, [piSessionId]);

	const handlePiKeyDown = useCallback(
		(e: React.KeyboardEvent<HTMLInputElement>) => {
			if (e.key === "Enter" && !e.shiftKey) {
				e.preventDefault();
				void handleStartPiSession();
			}
		},
		[handleStartPiSession],
	);

	// Cleanup: stop pi session on unmount (F3: strict-mode safe).
	// React 18 Strict Mode mounts → cleanup → mounts → ... → real unmount.
	// The empty-deps cleanup must NOT fire spike_pi_stop on the dummy cleanup.
	// Pattern: track mount status via ref; only stop on genuine final unmount.
	const piSessionRef = useRef<string | null>(null);
	const isMountedRef = useRef(false);
	useEffect(() => {
		piSessionRef.current = piSessionId;
	}, [piSessionId]);
	useEffect(() => {
		isMountedRef.current = true;
		return () => {
			// In strict mode: sets false, then component re-mounts synchronously
			// setting it back to true before the timeout fires.
			isMountedRef.current = false;
			// Defer stop: if still unmounted after tick, it's the real unmount.
			const sid = piSessionRef.current;
			if (sid) {
				setTimeout(() => {
					if (!isMountedRef.current) {
						void invokeBackendCommand<boolean>("spike_pi_stop", {
							sessionId: sid,
						}).catch(() => {});
					}
				}, 0);
			}
		};
	}, []);

	// When a pi session is active, show its console instead of the selected agent.
	const showingPi = piSessionId != null;
	const effectiveActivity = showingPi ? piActivity : activity;

	const piPanel = (
		<div
			style={{
				borderTop: "1px solid #EFE7DA",
				padding: "8px 12px",
				background: "#FDF8F0",
				display: "flex",
				flexDirection: "column",
				gap: 6,
			}}
		>
			<div style={{ display: "flex", alignItems: "center", gap: 6 }}>
				<Bot size={14} style={{ color: "#8B7355" }} />
				<span
					style={{
						fontSize: 11,
						fontWeight: 600,
						color: "#8B7355",
						letterSpacing: "0.05em",
					}}
				>
					Phase 1 — pi sidecar (alpha)
				</span>
				{piSessionId && (
					<span style={{ fontSize: 10, color: "#B8A88A", marginLeft: 4 }}>
						{piSessionId}
					</span>
				)}
				{piSessionId && (
					<button
						type="button"
						onClick={handleStopPiSession}
						style={{
							marginLeft: "auto",
							background: "none",
							border: "1px solid #D4C4A8",
							borderRadius: 4,
							padding: "2px 6px",
							cursor: "pointer",
							display: "flex",
							alignItems: "center",
							gap: 3,
							fontSize: 10,
							color: "#8B7355",
						}}
						title="Stop pi session"
					>
						<Square size={10} /> Stop
					</button>
				)}
			</div>
			<div style={{ display: "flex", gap: 6 }}>
				<input
					ref={piInputRef}
					type="text"
					value={piPrompt}
					onChange={(e) => setPiPrompt(e.target.value)}
					onKeyDown={handlePiKeyDown}
					placeholder={
						piSessionId ? "Send follow-up to pi…" : "Start a pi session…"
					}
					disabled={piLoading}
					style={{
						flex: 1,
						padding: "4px 8px",
						border: "1px solid #D4C4A8",
						borderRadius: 6,
						fontSize: 12,
						background: "#fff",
						color: "#3D3425",
						outline: "none",
					}}
				/>
				<button
					type="button"
					onClick={handleStartPiSession}
					disabled={piLoading || !piPrompt.trim()}
					style={{
						padding: "4px 12px",
						background: piLoading || !piPrompt.trim() ? "#E4DDD0" : "#8B7355",
						color: "#fff",
						border: "none",
						borderRadius: 6,
						fontSize: 11,
						fontWeight: 600,
						cursor: piLoading || !piPrompt.trim() ? "default" : "pointer",
					}}
				>
					{piLoading ? "Starting…" : piSessionId ? "Send" : "Start"}
				</button>
			</div>
		</div>
	);

	return (
		<div
			style={{
				height: "clamp(600px, calc(100vh - 210px), 1400px)",
				border: "1px solid #E4DDD0",
				borderRadius: 12,
				overflow: "hidden",
				display: "flex",
				flexDirection: "row",
			}}
		>
			<div
				style={{
					flex: "none",
					width: 480,
					borderRight: "1px solid #EFE7DA",
					display: "flex",
					flexDirection: "column",
				}}
			>
				<div style={{ flex: 1, overflow: "auto" }}>
					<LivingPlan
						model={model}
						selectedAgentId={selectedAgentId}
						onSelect={onSelectAgent}
						dirtyAgentIds={dirtyAgentIds}
					/>
				</div>
				{piPanel}
			</div>
			<div style={{ flex: 1, minWidth: 0 }}>
				{showingPi ? (
					// Show pi session console when active.
					<FocusStage
						node={{
							agentId: piSessionId!,
							label: `pi · ${piSessionId}`,
							type: "coder" as const,
							status: piActivity?.running ? "running" : "idle",
							file: null,
							district: "pi-sidecar",
							parentAgentId: null,
							pendingQuestion: null,
							taskId: null,
							live: true,
							children: [],
							orphaned: false,
							subagents: [],
						}}
						activity={effectiveActivity}
						view={view}
						onViewChange={setView}
						onSendMessage={(t) => {
							void invokeBackendCommand("spike_pi_prompt", {
								text: t,
								sessionId: piSessionId,
							}).catch(() => {});
						}}
						pendingQuestion={null}
						onAnswer={() => {}}
						rawSlot={null}
						disabled={false}
						onSlashCommand={onSlashCommand}
					/>
				) : selectedNode ? (
					<FocusStage
						node={selectedNode}
						activity={activity}
						view={view}
						onViewChange={setView}
						onSendMessage={onSendMessage}
						pendingQuestion={pendingQuestion}
						onAnswer={onAnswer}
						rawSlot={rawSlot}
						disabled={!!readOnly}
						onQuickAction={onQuickAction}
						onSlashCommand={onSlashCommand}
					/>
				) : (
					<div
						style={{
							display: "flex",
							height: "100%",
							alignItems: "center",
							justifyContent: "center",
							color: "#9c9488",
							fontSize: 14,
							textAlign: "center",
							padding: 16,
						}}
					>
						Select an agent on the left to focus it.
					</div>
				)}
			</div>
		</div>
	);
}
