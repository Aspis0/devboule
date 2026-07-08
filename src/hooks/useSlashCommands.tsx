import { useCallback, useMemo, useState } from "react";

export interface SlashCommand {
	command: string; // e.g. "model", "agent", "websearch"
	description: string; // tooltip
	args: string; // e.g. "[local|claude|openai] [model]"
	handler: (args: string) => SlashResult;
}

export interface SlashResult {
	type: "action" | "message" | "none";
	action?: string; // what action to perform
	payload?: any; // action data
	message?: string; // if type=message, inject into chat
}

export interface SlashApi {
	showPopup: boolean;
	filteredCommands: SlashCommand[];
	isSlashActive: boolean;
	activeIndex: number;
	handleInput: (value: string) => void;
	onEnter: () => SlashResult;
	onEscape: () => void;
	moveActive: (delta: number) => void;
	setActive: (index: number) => void;
	selectIndex: (index: number) => SlashResult;
}

// Static command registry. Handlers are pure (no component state), so this lives
// module-level — the hook just wires filtering/navigation around it.
const COMMANDS: SlashCommand[] = [
	{
		command: "model",
		description: "Switch the orchestrator model/provider",
		args: "[local|claude|openai|codex] [model]",
		handler: (args) => {
			const parts = args.trim().split(/\s+/).filter(Boolean);
			const provider = parts[0];
			if (!provider) return { type: "none" };
			return {
				type: "action",
				action: "switchModel",
				payload: { provider, model: parts.slice(1).join(" ") || undefined },
			};
		},
	},
	{
		command: "agent",
		description: "Switch the working agent role",
		args: "[role]",
		handler: (args) => {
			const role = args.trim();
			if (!role) return { type: "none" };
			return { type: "action", action: "switchAgent", payload: { role } };
		},
	},
	{
		command: "websearch",
		description: "Steer the orchestrator to run a web search",
		args: "[query]",
		handler: (args) => ({
			type: "message",
			message: "/websearch " + args.trim(),
		}),
	},
	{
		command: "plan",
		description: "Draft and submit the plan now",
		args: "[goal]",
		handler: (args) => ({ type: "message", message: "/plan " + args.trim() }),
	},
	{
		command: "review",
		description: "Request a review of the current plan",
		args: "",
		handler: () => ({ type: "message", message: "/review" }),
	},
	{
		command: "oracle",
		description: "Ask the Oracle",
		args: "[query]",
		handler: (args) => ({ type: "message", message: "/oracle " + args.trim() }),
	},
	{
		command: "stop",
		description: "Stop the current session",
		args: "",
		handler: () => ({ type: "action", action: "stopSession" }),
	},
	{
		command: "help",
		description: "Show help",
		args: "",
		handler: () => ({ type: "action", action: "showHelp" }),
	},
];

export function useSlashCommands(): SlashApi {
	const [input, setInput] = useState("");
	const [activeIndex, setActiveIndex] = useState(0);

	// The index of the slash that begins a potential command. Slash commands
	// activate ONLY when that slash is at the very start of the buffer or is
	// preceded solely by whitespace — a slash mid-sentence (e.g. "hello /foo")
	// must NOT open the popup (audit finding #4: the old `!input.includes("\n") `
	// guard let a slash after other text on the same line stay active).
	const slashIndex = input.indexOf("/");
	const isSlashActive =
		slashIndex >= 0 && input.slice(0, slashIndex).trim() === "";
	const query = isSlashActive
		? input
				.slice(slashIndex + 1)
				.split(/\s+/)[0]
				.toLowerCase()
		: "";

	const filteredCommands = useMemo(
		() =>
			isSlashActive ? COMMANDS.filter((c) => c.command.startsWith(query)) : [],
		[isSlashActive, query],
	);

	const showPopup = isSlashActive && filteredCommands.length > 0;

	const clampIndex = useCallback(
		(i: number) =>
			filteredCommands.length
				? Math.max(0, Math.min(i, filteredCommands.length - 1))
				: 0,
		[filteredCommands],
	);

	const handleInput = useCallback((value: string) => {
		setInput(value);
		setActiveIndex(0);
	}, []);

	const moveActive = useCallback(
		(delta: number) => setActiveIndex((i) => clampIndex(i + delta)),
		[clampIndex],
	);

	const setActive = useCallback(
		(index: number) => setActiveIndex(clampIndex(index)),
		[clampIndex],
	);

	const onEscape = useCallback(() => {
		setInput("");
		setActiveIndex(0);
	}, []);

	// Resolve the args AFTER the command token (everything past the first word).
	// Uses the command-starting slash (slashIndex) rather than a blind slice(1) so
	// a whitespace-prefixed command like "  /model codex" parses correctly.
	const argsFor = useCallback(() => {
		const i = input.indexOf("/");
		return i >= 0
			? input
					.slice(i + 1)
					.split(/\s+/)
					.slice(1)
					.join(" ")
			: "";
	}, [input]);

	const runAt = useCallback(
		(index: number): SlashResult => {
			// No silent fallback to the first command on an out-of-bounds index
			// (audit finding #3): an invalid index is a no-op, not a surprising run.
			const cmd = filteredCommands[index];
			if (!cmd) return { type: "none" };
			const result = cmd.handler(argsFor().trim());
			setInput("");
			setActiveIndex(0);
			return result;
		},
		[filteredCommands, argsFor],
	);

	const onEnter = useCallback((): SlashResult => {
		if (!isSlashActive || filteredCommands.length === 0) {
			// An unmatched slash (e.g. "/zzz") is treated as ordinary text: clear the
			// slash buffer so isSlashActive does not stay stuck true (audit finding
			// #2). The caller's normal-send path still fires for the raw text.
			setInput("");
			setActiveIndex(0);
			return { type: "none" };
		}
		return runAt(activeIndex);
	}, [isSlashActive, filteredCommands, activeIndex, runAt]);

	const selectIndex = useCallback(
		(index: number): SlashResult => runAt(index),
		[runAt],
	);

	return {
		showPopup,
		filteredCommands,
		isSlashActive,
		activeIndex,
		handleInput,
		onEnter,
		onEscape,
		moveActive,
		setActive,
		selectIndex,
	};
}

interface SlashCommandPopupProps {
	commands: SlashCommand[];
	activeIndex: number;
	onSelect: (index: number) => void;
	onHover: (index: number) => void;
}

// Presentational command list. Positioned ABSOLUTELY above the input — the parent
// composer container must be `position: relative`. Clicking uses onMouseDown +
// preventDefault so the textarea doesn't blur before the selection resolves.
export function SlashCommandPopup(props: SlashCommandPopupProps) {
	const { commands, activeIndex, onSelect, onHover } = props;
	return (
		<div
			role="listbox"
			style={{
				position: "absolute",
				bottom: "100%",
				left: 0,
				right: 0,
				marginBottom: 8,
				background: "#fff",
				border: "1px solid #EDE8E3",
				borderRadius: 10,
				boxShadow: "0 8px 32px rgba(45,42,38,0.10)",
				maxHeight: 240,
				overflowY: "auto",
				zIndex: 50,
				padding: 4,
			}}
		>
			{commands.map((c, i) => (
				<button
					key={c.command}
					type="button"
					role="option"
					aria-selected={i === activeIndex}
					onMouseEnter={() => onHover(i)}
					onMouseDown={(e) => {
						e.preventDefault();
						onSelect(i);
					}}
					style={{
						display: "flex",
						flexDirection: "column",
						gap: 1,
						width: "100%",
						textAlign: "left",
						padding: "6px 10px",
						border: "none",
						borderRadius: 7,
						background: i === activeIndex ? "#F5F0EB" : "transparent",
						cursor: "pointer",
					}}
				>
					<span style={{ fontSize: 12.5, fontWeight: 600, color: "#2D2A26" }}>
						/{c.command} {c.args}
					</span>
					<span style={{ fontSize: 11, color: "#8A8580" }}>
						{c.description}
					</span>
				</button>
			))}
		</div>
	);
}
