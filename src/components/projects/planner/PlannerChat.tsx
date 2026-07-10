import { useState } from "react";
import type { KeyboardEvent } from "react";
import { Send, RotateCcw } from "lucide-react";
import type { PlannerMessage } from "./plannerModel";
import { ChatThread } from "../../activity/ChatThread";
import {
	useSlashCommands,
	SlashCommandPopup,
} from "../../../hooks/useSlashCommands";
import type { SlashResult } from "../../../hooks/useSlashCommands";

interface PlannerChatProps {
	messages: PlannerMessage[];
	modelLabel: string;
	live: boolean;
	awaitingReply: boolean;
	/** D4 (planner-chat demolition): composer CHROME for delivery failures, launch
	 *  guidance and the 60s silence watchdog — rendered as an amber strip ABOVE the
	 *  composer, never spliced into the transcript as a fake assistant message.
	 *  While set it also supersedes the "thinking…" pill (the strip explains why
	 *  there is no reply; a spinning pill next to it would contradict it). */
	banner?: string | null;
	onSend: (text: string) => void;
	/** Esc while the orchestrator works: interrupt the IN-FLIGHT turn (the agent
	 *  and its context stay alive). Absent = no interrupt surface (e.g. no live
	 *  cloud orchestrator bound). */
	onInterrupt?: () => void;
	/** Slash-command result (model/agent switch, stop, help). Optional: only
	 *  invoked when the composer intercepts a matched command on Enter. */
	onSlashCommand?: (result: SlashResult) => void;
	/** Reset the orchestrator chat: stop the session, wipe the transcript, start
	 *  clean. Absent when no orchestrator agent id is bound. */
	onResetChat?: () => void;
	/** S4: optional orchestrator backend selector (WHO YOU TALK TO). When supplied it
	 *  renders as a compact segmented control inside the chat header, next to the
	 *  CHAT label, before the live/reset controls. The active entry pulses while
	 *  `live`. Absent => no selector (the planner renders it standalone instead). */
	orchestrators?: { id: string; label: string; disabled?: boolean }[];
	orchestratorId?: string;
	onOrchestratorChange?: (id: string) => void;
}

export function PlannerChat({
	messages,
	modelLabel,
	live,
	awaitingReply,
	banner,
	onSend,
	onInterrupt,
	onSlashCommand,
	onResetChat,
	orchestrators,
	orchestratorId,
	onOrchestratorChange,
}: PlannerChatProps) {
	const [value, setValue] = useState("");
	const slash = useSlashCommands();

	const send = () => {
		const trimmed = value.trim();
		if (!trimmed) return;
		onSend(trimmed);
		setValue("");
	};

	// Resolve a slash command (from Enter OR a popup click) and route it:
	//   action  -> emit to the parent (model/agent switch, stop, help)
	//   message -> forward the literal text to the orchestrator (steer)
	//   none    -> unmatched: caller falls through to a normal text send
	const runSlash = (result: SlashResult) => {
		if (result.type === "action") {
			onSlashCommand?.(result);
			setValue("");
			return true;
		}
		if (result.type === "message" && result.message) {
			onSend(result.message);
			setValue("");
			return true;
		}
		return false;
	};

	const onPopupSelect = (index: number) => {
		runSlash(slash.selectIndex(index));
	};

	const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
		if (slash.isSlashActive) {
			if (e.key === "ArrowDown") {
				e.preventDefault();
				slash.moveActive(1);
				return;
			}
			if (e.key === "ArrowUp") {
				e.preventDefault();
				slash.moveActive(-1);
				return;
			}
			if (e.key === "Tab") {
				e.preventDefault();
				slash.moveActive(1);
				return;
			}
			if (e.key === "Escape") {
				e.preventDefault();
				slash.onEscape();
				return;
			}
			if (e.key === "Enter" && !e.shiftKey) {
				e.preventDefault();
				// Matched command (action/message) is consumed here; an unmatched slash
				// falls through to the normal send below (treated as plain text).
				if (runSlash(slash.onEnter())) return;
			}
		}
		if (e.key === "Enter" && !e.shiftKey) {
			e.preventDefault();
			send();
			return;
		}
		if (e.key === "Escape" && onInterrupt && live) {
			e.preventDefault();
			onInterrupt();
		}
	};

	return (
		<div
			style={{
				background: "#fff",
				border: "1px solid #E4DDD0",
				borderRadius: 12,
				overflow: "hidden",
				display: "flex",
				flexDirection: "column",
				minHeight: 420,
				maxHeight: "clamp(460px, 62vh, 1200px)",
				flex: 1,
			}}
		>
			{/* HEADER */}
			<div
				style={{
					display: "flex",
					alignItems: "center",
					gap: 8,
					padding: "9px 13px",
					borderBottom: "1px solid #EFE7DA",
					background: "#FCFAF6",
				}}
			>
				<span
					className="pp-mono"
					style={{ fontSize: 9.5, letterSpacing: 0.14, color: "#A89F90" }}
				>
					CHAT
				</span>
				<span className="pp-mono" style={{ fontSize: 9.5, color: "#9c9488" }}>
					{modelLabel}
				</span>
				{orchestrators && orchestrators.length > 0 && (
					<div
						style={{
							display: "flex",
							alignItems: "center",
							gap: 4,
							marginLeft: 6,
						}}
					>
						{orchestrators.map((o) => {
							const isActive = o.id === orchestratorId;
							const disabled = o.disabled === true;
							return (
								<button
									type="button"
									key={o.id}
									className="pp-mono"
									onClick={() => {
										if (!disabled) onOrchestratorChange?.(o.id);
									}}
									disabled={disabled}
									title={
										disabled
											? `${o.label} CLI is not installed on this machine`
											: undefined
									}
									style={{
										display: "flex",
										alignItems: "center",
										gap: 5,
										fontSize: 9.5,
										borderRadius: 7,
										padding: "4px 8px",
										cursor: disabled ? "not-allowed" : "pointer",
										opacity: disabled ? 0.45 : 1,
										fontWeight: isActive ? 600 : 400,
										color: isActive ? "#9A6A2E" : "#A89F90",
										background: isActive ? "#F1E4D2" : "transparent",
										border: isActive
											? "1px solid #E6D3BB"
											: "1px solid transparent",
										animation:
											isActive && live ? "pp-pulse 1.9s infinite" : "none",
									}}
								>
									<div
										style={{
											width: 6,
											height: 6,
											borderRadius: "50%",
											background: isActive ? "#C0894F" : "#CFC6B6",
										}}
									/>
									<span>{o.label}</span>
								</button>
							);
						})}
					</div>
				)}
				<span
					style={{
						marginLeft: "auto",
						display: "flex",
						alignItems: "center",
						gap: 5,
					}}
				>
					<span
						style={{
							width: 6,
							height: 6,
							borderRadius: "50%",
							background: live ? "#7FA468" : "#CFC6B6",
						}}
					/>
					<span
						style={{
							fontSize: 10,
							fontWeight: 600,
							color: live ? "#5e8a4d" : "#9c9488",
						}}
					>
						{live ? "live" : "idle"}
					</span>
					<button
						onClick={() => onResetChat?.()}
						disabled={!onResetChat}
						title={onResetChat ? "Reset chat" : "No orchestrator bound"}
						style={{
							width: 22,
							height: 22,
							border: "none",
							background: "transparent",
							borderRadius: 6,
							display: "flex",
							alignItems: "center",
							justifyContent: "center",
							cursor: onResetChat ? "pointer" : "default",
							opacity: onResetChat ? 0.7 : 0.25,
							transition: "opacity 0.15s",
							padding: 0,
						}}
					>
						<RotateCcw size={12} />
					</button>
				</span>
			</div>

			<ChatThread
				messages={messages}
				live={live && !banner}
				awaitingReply={awaitingReply}
			/>

			{/* D4 BANNER: delivery/stall/launch feedback as chrome above the composer. */}
			{banner ? (
				<div
					data-testid="planner-banner"
					style={{
						display: "flex",
						alignItems: "center",
						gap: 8,
						padding: "8px 13px",
						borderTop: "1px solid #EED9B7",
						background: "#FBF3E2",
						color: "#8A6B33",
						fontSize: 12,
						lineHeight: 1.45,
					}}
				>
					<span aria-hidden style={{ flex: "none" }}>
						⚠︎
					</span>
					<span style={{ flex: 1 }}>{banner}</span>
					{onResetChat ? (
						<button
							type="button"
							data-testid="planner-banner-restart"
							onClick={() => onResetChat?.()}
							style={{ flex: "none", cursor: "pointer", border: "1px solid #D9B673", background: "#F3E4C4", color: "#7A5A20", fontSize: 11, fontWeight: 600, borderRadius: 6, padding: "4px 10px", display: "flex", alignItems: "center", gap: 5 }}
						>
							<RotateCcw size={11} />
							Restart orchestrator
						</button>
					) : null}
				</div>
			) : null}

			{/* COMPOSER */}
			<div
				style={{
					position: "relative",
					display: "flex",
					alignItems: "flex-end",
					gap: 8,
					padding: "9px 10px",
					borderTop: "1px solid #EFE7DA",
					background: "#FCFAF6",
				}}
			>
				{slash.showPopup && (
					<SlashCommandPopup
						commands={slash.filteredCommands}
						activeIndex={slash.activeIndex}
						onSelect={onPopupSelect}
						onHover={slash.setActive}
					/>
				)}
				<textarea
					value={value}
					onChange={(e) => {
						setValue(e.target.value);
						slash.handleInput(e.target.value);
					}}
					onKeyDown={handleKeyDown}
					rows={1}
					placeholder="Message the Orchestrator…  (Enter to send)"
					style={{
						flex: 1,
						resize: "none",
						border: "1px solid #E4DDD0",
						borderRadius: 10,
						background: "#fff",
						padding: "10px 12px",
						fontSize: 13,
						color: "#2A2621",
						outline: "none",
						lineHeight: 1.4,
						maxHeight: 80,
					}}
				/>
				<button
					onClick={send}
					style={{
						width: 38,
						height: 38,
						flex: "none",
						border: "none",
						background: "linear-gradient(150deg,#C8945C,#B07D43)",
						borderRadius: 10,
						display: "flex",
						alignItems: "center",
						justifyContent: "center",
						color: "#FBF6EF",
						cursor: value.trim() ? "pointer" : "default",
						opacity: value.trim() ? 1 : 0.5,
						transition: "opacity 0.15s",
					}}
				>
					<Send size={16} />
				</button>
			</div>
		</div>
	);
}
