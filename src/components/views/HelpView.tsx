import { LifeBuoy } from "lucide-react";
import { useAppActions } from "../../context/AppContext";

/** A single Help page section. Kept as plain, testable data so the page can be
 *  rendered data-driven and asserted in tests without reaching into JSX. */
export interface HelpSectionLink {
	label: string;
	view: string;
	tab?: string;
}

export interface HelpSection {
	id: string;
	title: string;
	/** One or more plain-text paragraphs (no markdown). Optional when `steps` is used. */
	body?: string[];
	/** An optional ordered list (used by the Quick start section). */
	steps?: string[];
	/** An optional cross-link button that jumps to another view. */
	link?: HelpSectionLink;
}

export const HELP_SECTIONS: HelpSection[] = [
	{
		id: "what-is-devboule",
		title: "What is Devboule",
		body: [
			"Devboule is a local-first desktop app that turns a folder of code into a managed project. An orchestrator plans the work, coders (AI agents) implement it, the Censor reviews every change, and you approve before anything ships.",
			"It runs on your own machine. Your code and your secrets stay local — agents talk to models you choose, and nothing leaves your computer unless you push it. If you're new, start with Quick start just below.",
		],
	},
	{
		id: "quick-start",
		title: "Quick start (5 steps)",
		steps: [
			"Projects → type a title (or pick a folder / clone from GitHub) → Create.",
			"Tell the orchestrator your goal in the chat and let it plan (websearch/plan/design panels appear under the chat — collapsed until there's something to show).",
			"Approve the plan → tasks land on the project board.",
			"Open the project and Launch a coder (choose codex / claude / openai / Local).",
			"Watch the console, answer its questions, let the Censor review, then Commit/Push from the top bar.",
		],
	},
	{
		id: "projects-board",
		title: "Projects board",
		body: [
			"The Projects board is a Kanban of every project, grouped into six stage columns: Planned, Launching, Active, Review, Blocked, and Verified.",
			"Each project card shows quick chips: a git drift indicator (↑/↓/∆), an open Censor-finding count (⚠N), a task-state breakdown (done vs total, plus blocked), and the next milestone. These update live as agents work.",
			"A Calendar toggle button reveals a single view of every project deadline and release date. Creating a project leaves it in the Planned stage with an empty draft plan — you fill that plan in by talking to the orchestrator.",
		],
		link: { label: "Open the Projects board", view: "projects" },
	},
	{
		id: "planner",
		title: "Planner & the orchestrator",
		body: [
			"The Planner is the chat where you tell Devboule what you want. Describe your goal in plain language and the orchestrator turns it into a structured plan of tasks you can approve.",
			"Each role has three placement options: Local runs an on-device engine (Ollama, oMLX, or Apple on-device) with nothing leaving the machine. Cloud API sends prompts to a remote OpenAI-compatible provider — you set the model, base URL, and a shared API key stored in your OS keychain, and tick a consent checkbox because content leaves the machine. Cloud CLI hands off to an external CLI (Claude Code, Codex, or OpenAI) which uses its own login — Devboule does not manage that key.",
			"As it works, collapsible panels appear under the chat showing web-search results, design ideas, and the plan outline. Approve the plan and its tasks land on the project board.",
			"Tip: the local orchestrator on a big reasoning model can take a while to answer your first message — it's loading the model and thinking, not stuck. Give it 30–90 seconds before assuming something's wrong.",
		],
	},
	{
		id: "work-console",
		title: "Work mode & the console",
		body: [
			"Open a project to enter its Work console. The top of the console switches between the Activity view (structured, human-readable agent activity) and the Raw terminal view (the agent's actual PTY).",
			"One consolidated tab bar runs along the Work console: Tasks, Censor, Git, Changes, Plans, Notes, MCP, and Project. Use it to move between the Kanban, the reviewer, the git line, the diff, the plan, your notes, project MCP servers, and project details.",
			"You can stop an agent from here, and plan-approval and push-approval requests surface as badges on the relevant tabs (such as Plans) so you can approve or deny them.",
		],
	},
	{
		id: "agents-coders",
		title: "Agents & coders",
		body: [
			"Four agent roles do the work. The orchestrator turns your goal into a plan and coordinates the others. A coder implements the plan by driving an external CLI or a local engine. A verifier reviews completed work and closes tasks. A mini-coder is a small delegated session a coder spawns to handle a sub-task. Each role's placement is configured independently in Settings → Roles, so different roles can run on different backends.",
			"To use Claude you have two routes: a subscription via Cloud CLI (Claude Code, which uses its own login), or an API key via Cloud API with an aggregator like OpenRouter and an anthropic model id — Devboule speaks the OpenAI dialect, so Anthropic's native endpoint is not used directly. Cloud API backends are per-role: what you configure on a row is what that role uses, and the main coder does not inherit another role's cloud backend.",
			"The Mini row offers local backends (Ollama, oMLX, Apple on-device), Cloud API with a shared key, or external CLIs including a Custom command that runs a shell command verbatim with the prompt piped to its stdin.",
		],
	},
	{
		id: "censor",
		title: "Censor",
		body: [
			"The Censor is Devboule's local review gate. It runs deterministic linters and applies AI review tiers (including an optional local model) as agents write code, then raises findings you can triage inside the project.",
			"Each project has a trust gate: the Censor only starts enforcing once you trust the project, so it stays inert until then. You can mark findings as false-positive, wontfix, or accepted, and run a final review sweep before anything ships.",
		],
	},
	{
		id: "oracle",
		title: "Oracle",
		body: [
			"Oracle answers questions about your codebase using the indexed repository, so you and the agents can recover real context before acting.",
			"It indexes your workspace and keeps a watcher running so the index stays fresh. If you see an 'indexing' badge, that's a normal busy state — it's just catching up on recent changes.",
			"Configure and use it from the standalone Oracle page, reachable from the sidebar — or ask it directly from the Polis map.",
		],
		link: { label: "Open Oracle", view: "oracle" },
	},
	{
		id: "skills-tools",
		title: "Skills & Tools",
		body: [
			"Skills are reusable playbooks (SKILL.md manuals) that teach agents how your project works, and Tools are MCP (Model Context Protocol) servers the agents can call to reach external systems like databases or APIs.",
			"Open the Skills page to browse the library and install tools. It has two tabs: Library (your personal skill manuals, shared across projects) and Tools (your MCP servers, which can be scoped global or to a single project).",
		],
		link: { label: "Open the Skills page", view: "skills" },
	},
	{
		id: "providers-models",
		title: "Providers & Models settings",
		body: [
			"This Settings tab is where you connect every AI provider and pick the model each agent role uses. It has four sub-tabs: Models, Gates & helpers, Extensions, and Design.",
			"The Roles card is the main surface: each row (Orchestrator, Main coder, Mini, Verifier) has a three-way placement switch (Local / Cloud API / Cloud CLI) plus the backend fields for that placement. What you configure on a row is what that role uses — the main coder does not inherit another role's cloud backend. Gates & helpers sets the Censor's local AI backend, web search, and mini-write behavior. Extensions manages pi extensions and MCP servers; Design picks the Design module's LLM.",
			"At the top, Devboule auto-detects on-device engines (Ollama, oMLX, Apple on-device) and cloud APIs installed on your machine — it shows what's available without exposing CLI paths.",
		],
		link: {
			label: "Open Providers & Models",
			view: "settings",
			tab: "providers",
		},
	},
	{
		id: "workspace-index",
		title: "Workspace & Index settings",
		body: [
			"The Workspace & Index tab is for hygiene and setup. Run a hygiene scan to spot dirty git repos, large files, and overall workspace health; review your repos and classification policies; and build bootstrap packages so collaborators can decrypt and open your exact workspace.",
			"The Censor local AI provider (oMLX / Ollama / Apple on-device / cloud) is also configured here.",
		],
		link: {
			label: "Open Workspace & Index",
			view: "settings",
			tab: "workspace",
		},
	},
	{
		id: "dependencies",
		title: "Dependencies settings",
		body: [
			"Dependencies lists every external command-line tool Devboule relies on — things like Git, Codex, or Claude — with its purpose, install status, resolved path, and version.",
			"Missing tools show a red X but only disable the features that need them; the rest of the app keeps working. Open this tab whenever something won't launch to see what's missing.",
		],
		link: { label: "Open Dependencies", view: "settings", tab: "dependencies" },
	},
	{
		id: "security-lock",
		title: "Security & lock",
		body: [
			"Devboule locks behind device auth — Touch ID on macOS or Windows Hello on Windows — and encrypts all secrets at rest, so nothing is readable without unlocking your device.",
			"Auto-lock isn't instant: it waits a short grace period while the window is hidden, so a quick Space switch or minimizing won't lock you out.",
			"API tokens and provider secrets live in the OS keychain and are shown only as ••••••••, with per-provider status and scope pinning so the app can't write to the wrong account.",
		],
		link: { label: "Open Security", view: "settings", tab: "security" },
	},
	{
		id: "labs",
		title: "Labs",
		body: [
			"Labs holds experimental features that aren't stable yet. Flip a toggle to try them; some apply immediately, others need an app restart.",
			"Today you can enable the Design view (generative UI mockups) and Pigeon (a persistent agent mailbox for handoffs). More experiments — SkillOpt and ORPO Night — are on the way.",
		],
		link: { label: "Open Labs", view: "labs" },
	},
	{
		id: "polis",
		title: "Polis — the city of your code",
		body: [
			"Polis is an isometric living map of your codebase. Files are buildings, dependencies are roads and trade routes, agents are walking figures, and the Censor's findings show up as fires.",
			"Click a building to inspect the file it represents, or a trade route to trace a dependency edge. Use the bottom bar to ask Oracle a question about the code directly from the map, and to filter agents and anomalies.",
		],
		link: { label: "Open Polis", view: "polis" },
	},
	{
		id: "github-push",
		title: "GitHub & push approval",
		body: [
			"Git lives inside Devboule. Agents commit freely as they work, but every push to a remote must be approved by you — nothing reaches GitHub without your OK.",
			"When an agent wants to push, a compact amber card appears in the Work console showing which agent, which branch, and whether it's a force push. Approve to let Devboule perform the push, or deny to hold it back.",
		],
		link: { label: "Open the Projects board", view: "projects" },
	},
	{
		id: "tips",
		title: "Tips",
		body: [
			"Hold Alt anywhere to enter Help mode: a floating overlay explains what each control does and why it matters for Devboule.",
			"Use the header search (the magnifying-glass jump-search) to jump straight to any page by typing part of its name.",
			"The bell / notification icon in the header shows agents that need you — a question, an approve/deny decision, or a risk flag — so you never miss a hand-off.",
			"The Planner's panels (web search, design, plan) stay collapsed until there's something to show — expand them as the orchestrator works.",
		],
	},
	{
		id: "acknowledgments",
		title: "Acknowledgments",
		body: [
			"Devboule is built on the shoulders of giants — first among them is pi, the open-source coding-agent runtime by earendil-works (@earendil-works/pi-coding-agent). Our orchestrator and coders run on pi; it bundles the pi agent loop, AI, and TUI pieces that our own agents are wired into every day, and we're deeply grateful to earendil-works for releasing it as open source.",
			"Under the hood, Devboule leans on an incredible open-source stack (the genuine npm/cargo dependencies). The desktop app is powered by Tauri, with a React + Vite frontend styled with Tailwind CSS and icons from lucide-react, state managed with Zustand, and in-app terminals rendered with xterm.js. The living Polis map is drawn with PixiJS (pixi.js, with pixi-filters and pixi-viewport), its dependency arrows laid out with d3-force and perfect-arrows, and UI motion from GSAP. The Censor's deterministic review is grounded in tree-sitter's multi-language parsing, agent terminals ride on portable-pty, and DOMPurify keeps rendered HTML safe. Separately, Devboule integrates at runtime with a local retrieval/inference stack — the Oracle knowledge layer and its semantic search run on LanceDB, with local Qwen embeddings and on-device MLX (oMLX) inference — but these are external runtime services and models the app talks to, not npm/cargo dependencies.",
			"Polis isn't just code — it's also open art. The isometric city is built from open-licensed sprite work we're thrilled to credit: seamless terrain and material textures from Screaming Brain Studios' Tiny Texture Pack (CC0); the trees and walking-crowd walk cycles from the Unknown Horizons team (CC-BY-SA 3.0); and the burning-building fire animation by FoshyTakashi (CC-BY 3.0). Thank you to these artists for sharing their work.",
			"And, of course, the many other open-source projects — from the Rust crates in our Tauri backend to the npm packages in our frontend — listed in our dependency manifests. Thank you.",
		],
	},
];

export function HelpView() {
	const { requestView } = useAppActions();

	return (
		<div className="mx-auto max-w-3xl px-6 py-8">
			<div className="mb-6 flex items-center gap-3">
				<div className="flex h-8 w-8 items-center justify-center rounded-lg bg-teal/10">
					<LifeBuoy className="h-4 w-4 text-teal-dark" />
				</div>
				<div>
					<h1 className="text-lg font-semibold text-cream-900">Help</h1>
					<p className="text-sm text-cream-500">
						How Devboule works, in plain English — start here if you're new.
					</p>
				</div>
			</div>

			<nav className="sticky top-0 z-10 mb-4 flex flex-wrap gap-2 rounded-2xl border border-cream-200 bg-cream-50/90 p-3 backdrop-blur">
				{HELP_SECTIONS.map((section) => (
					<a
						key={section.id}
						href={`#${section.id}`}
						className="rounded-full bg-cream-100 px-3 py-1 text-[12px] font-medium text-cream-600 transition-colors hover:text-cream-800"
					>
						{section.title}
					</a>
				))}
			</nav>

			<div className="grid gap-4">
				{HELP_SECTIONS.map((section) => (
					<section
						key={section.id}
						id={section.id}
						className="scroll-mt-4 rounded-2xl border border-cream-200 bg-white p-5"
					>
						<h2 className="mb-2 text-base font-semibold text-cream-900">
							{section.title}
						</h2>
						{section.body?.map((paragraph, index) => (
							<p key={index} className="mb-2 text-sm leading-6 text-cream-600">
								{paragraph}
							</p>
						))}
						{section.steps && (
							<ol className="my-2 list-decimal space-y-1.5 pl-5 text-sm leading-6 text-cream-600">
								{section.steps.map((step, index) => (
									<li key={index}>{step}</li>
								))}
							</ol>
						)}
						{section.link && (
							<button
								type="button"
								onClick={() =>
									section.link!.tab
										? requestView(section.link!.view, section.link!.tab)
										: requestView(section.link!.view)
								}
								data-testid={`help-link-${section.link.view}`}
								className="mt-2 inline-flex items-center gap-1.5 rounded-2xl border border-cream-200 bg-cream-50 px-3 py-1.5 text-[12px] font-medium text-cream-700 transition-colors hover:bg-cream-100"
							>
								{section.link.label}
							</button>
						)}
					</section>
				))}
			</div>
		</div>
	);
}

export default HelpView;
