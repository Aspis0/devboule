import "./work.css";
import { useState } from "react";
import type { ReactNode, KeyboardEvent } from "react";
import {
	useSlashCommands,
	SlashCommandPopup,
} from "../../hooks/useSlashCommands";
import type { SlashResult } from "../../hooks/useSlashCommands";
import { AgentConsole } from "../agents/AgentConsole";
import type { ConsoleActivity } from "../agents/agentConsoleModel";
import type { WorkNode } from "./workConsoleModel";
import { Send } from "lucide-react";

export interface FocusStageProps {
	node: WorkNode;
	activity: ConsoleActivity;
	view: "activity" | "raw";
	onViewChange: (v: "activity" | "raw") => void;
	onSendMessage: (text: string) => void;
	pendingQuestion: string | null;
	onAnswer: (text: string) => void;
	rawSlot?: ReactNode;
	disabled?: boolean;
	onQuickAction?: (a: "redo" | "narrow" | "pause") => void;
	/** Slash-command result (model/agent switch, stop, help). Optional: only
	 *  invoked when the composer intercepts a matched command on Enter. */
	onSlashCommand?: (result: SlashResult) => void;
}

export function FocusStage(props: FocusStageProps) {
	const {
		node,
		activity,
		view,
		onViewChange,
		onSendMessage,
		pendingQuestion,
		onAnswer,
		rawSlot,
		disabled = false,
		onQuickAction,
		onSlashCommand,
	} = props;

	const [value, setValue] = useState("");
	const slash = useSlashCommands();

	const fileBasename = node.file
		? node.file.split("/").filter(Boolean).pop() || node.file
		: node.label;

	const initial = node.label?.[0]?.toUpperCase() || "?";

	const isAnswerMode = pendingQuestion != null && pendingQuestion.trim() !== "";

	const handleSend = () => {
		const trimmed = value.trim();
		if (!trimmed) return;
		if (isAnswerMode) {
			onAnswer(trimmed);
		} else {
			onSendMessage(trimmed);
		}
		setValue("");
	};

	// Resolve a slash command (from Enter OR a popup click) and route it:
	//   action  -> emit to the parent (model/agent switch, stop, help)
	//   message -> forward the literal text as a normal chat message
	//   none    -> unmatched: caller falls through to a normal text send
	const runSlash = (result: SlashResult) => {
		if (result.type === "action") {
			onSlashCommand?.(result);
			setValue("");
			return true;
		}
		if (result.type === "message" && result.message) {
			onSendMessage(result.message);
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
				// Matched command (action/message) is consumed here; an unmatched
				// slash falls through to the normal send below (treated as plain text).
				if (runSlash(slash.onEnter())) return;
			}
		}
		if (e.key === "Enter" && !e.shiftKey) {
			e.preventDefault();
			handleSend();
		}
	};

	const activeTabStyle: React.CSSProperties = {
		background: "#2A2621",
		color: "#FBF8F2",
		padding: "4px 12px",
		borderRadius: "7px",
		fontSize: "12px",
		fontWeight: 500,
		cursor: "default",
	};

	const inactiveTabStyle: React.CSSProperties = {
		color: "#9c9488",
		padding: "4px 12px",
		borderRadius: "7px",
		fontSize: "12px",
		fontWeight: 500,
		cursor: "pointer",
	};

	return (
		<div
			data-view={view}
			style={{
				display: "flex",
				flexDirection: "column",
				height: "100%",
				minHeight: 0,
			}}
		>
			{/* HEADER */}
			<div
				style={{
					flex: "none",
					display: "flex",
					alignItems: "center",
					gap: "11px",
					padding: "13px 18px",
					borderBottom: "1px solid #EFE7DA",
				}}
			>
				<span
					style={{
						width: 28,
						height: 28,
						borderRadius: "50%",
						background: "#E4DDD0",
						display: "flex",
						alignItems: "center",
						justifyContent: "center",
						fontSize: "12px",
						fontWeight: 600,
						color: "#3B362F",
					}}
				>
					{initial}
				</span>
				<div style={{ minWidth: 0 }}>
					<div
						className="pp-mono"
						style={{
							fontSize: "13px",
							color: "#2A2621",
							fontWeight: 500,
							whiteSpace: "nowrap",
							overflow: "hidden",
							textOverflow: "ellipsis",
						}}
					>
						{fileBasename}
					</div>
					<div
						style={{ fontSize: "11.5px", color: "#9c9488", marginTop: "2px" }}
					>
						{node.district} · {node.label} · {node.status}
					</div>
				</div>
				<div
					style={{
						marginLeft: "auto",
						display: "flex",
						alignItems: "center",
						gap: "10px",
					}}
				>
					<div
						style={{
							display: "flex",
							background: "#F1E9DC",
							borderRadius: "9px",
							padding: "3px",
						}}
					>
						<div
							data-tab="activity"
							onClick={() => onViewChange("activity")}
							style={view === "activity" ? activeTabStyle : inactiveTabStyle}
						>
							Activity
						</div>
						<div
							data-tab="raw"
							onClick={() => onViewChange("raw")}
							style={view === "raw" ? activeTabStyle : inactiveTabStyle}
						>
							Raw
						</div>
					</div>
				</div>
			</div>

			{/* BODY */}
			<div
				style={{
					flex: 1,
					minHeight: 0,
					display: "flex",
					flexDirection: "column",
				}}
			>
				{view === "activity" && (
					<div
						className="wc-scroll"
						style={{
							flex: 1,
							minHeight: 0,
							padding: "18px 20px 6px",
							display: "flex",
							flexDirection: "column",
							gap: "12px",
							overflowY: "auto",
						}}
					>
						<AgentConsole activity={activity} />
					</div>
				)}
				{view === "raw" && (
					<div style={{ flex: 1, padding: "16px 18px", minHeight: 0 }}>
						{rawSlot}
					</div>
				)}
			</div>

			{/* COMPOSER */}
			<div
				style={{
					flex: "none",
					borderTop: "1px solid #EFE7DA",
					background: "#FCFAF6",
					padding: "11px 14px",
				}}
			>
				{isAnswerMode && (
					<div
						data-asking="true"
						style={{
							marginBottom: "9px",
							padding: "10px 12px",
							background: "#FDF6EC",
							border: "1px solid #E6D3BB",
							borderRadius: "8px",
							display: "flex",
							alignItems: "flex-start",
							gap: "8px",
						}}
					>
						<span
							style={{
								marginTop: "2px",
								width: "6px",
								height: "6px",
								borderRadius: "50%",
								background: "#9A6A2E",
								animation: "wc-amber 1.8s infinite",
								flexShrink: 0,
							}}
						/>
						<div
							style={{ fontSize: "12px", color: "#2A2621", lineHeight: "1.45" }}
						>
							<span style={{ fontWeight: 600, color: "#9A6A2E" }}>
								coder asks:
							</span>{" "}
							{pendingQuestion}
						</div>
					</div>
				)}

				{!isAnswerMode && (
					<div
						style={{
							display: "flex",
							alignItems: "center",
							gap: "9px",
							marginBottom: "9px",
						}}
					>
						<span
							data-action="redo"
							onClick={disabled ? undefined : () => onQuickAction?.("redo")}
							style={{
								display: "flex",
								alignItems: "center",
								gap: "5px",
								fontSize: "10.5px",
								fontWeight: 600,
								color: "#6f685e",
								background: "#fff",
								border: "1px solid #E4DDD0",
								padding: "4px 9px",
								borderRadius: "7px",
								cursor: disabled ? "default" : "pointer",
								opacity: disabled ? 0.5 : 1,
								pointerEvents: disabled ? "none" : "auto",
							}}
						>
							redo round
						</span>
						<span
							data-action="narrow"
							onClick={disabled ? undefined : () => onQuickAction?.("narrow")}
							style={{
								display: "flex",
								alignItems: "center",
								gap: "5px",
								fontSize: "10.5px",
								fontWeight: 600,
								color: "#6f685e",
								background: "#fff",
								border: "1px solid #E4DDD0",
								padding: "4px 9px",
								borderRadius: "7px",
								cursor: disabled ? "default" : "pointer",
								opacity: disabled ? 0.5 : 1,
								pointerEvents: disabled ? "none" : "auto",
							}}
						>
							narrow scope
						</span>
						<span
							data-action="pause"
							onClick={disabled ? undefined : () => onQuickAction?.("pause")}
							style={{
								display: "flex",
								alignItems: "center",
								gap: "5px",
								fontSize: "10.5px",
								fontWeight: 600,
								color: "#6f685e",
								background: "#fff",
								border: "1px solid #E4DDD0",
								padding: "4px 9px",
								borderRadius: "7px",
								cursor: disabled ? "default" : "pointer",
								opacity: disabled ? 0.5 : 1,
								pointerEvents: disabled ? "none" : "auto",
							}}
						>
							pause
						</span>
					</div>
				)}

				<div
					style={{
						position: "relative",
						display: "flex",
						alignItems: "flex-end",
						gap: "8px",
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
						placeholder={
							isAnswerMode
								? "answer to continue…"
								: `message ${node.label} · arrives next round`
						}
						disabled={disabled}
						style={{
							flex: 1,
							resize: "none",
							border: "1px solid #E4DDD0",
							borderRadius: "10px",
							background: "#fff",
							padding: "10px 13px",
							fontSize: "13px",
							color: "#2A2621",
							outline: "none",
							lineHeight: "1.4",
							maxHeight: "90px",
							opacity: disabled ? 0.6 : 1,
							cursor: disabled ? "default" : "text",
						}}
					/>
					<button
						type="button"
						onClick={handleSend}
						disabled={disabled || !value.trim()}
						data-action="send"
						style={{
							width: "38px",
							height: "38px",
							flex: "none",
							border: "none",
							background: "linear-gradient(150deg,#C8945C,#B07D43)",
							borderRadius: "10px",
							display: "flex",
							alignItems: "center",
							justifyContent: "center",
							color: "#FBF6EF",
							cursor: disabled || !value.trim() ? "default" : "pointer",
							opacity: disabled || !value.trim() ? 0.5 : 1,
						}}
					>
						<Send size={16} strokeWidth={1.9} />
					</button>
				</div>
			</div>
		</div>
	);
}
