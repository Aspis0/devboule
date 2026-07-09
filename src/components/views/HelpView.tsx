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
      "Devboule is a local coordinator that turns a folder of code into a managed project. An orchestrator plans the work, coders (CLI agents) implement it, the Censor reviews the changes, and you approve before anything ships.",
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
    title: "The projects board",
    body: [
      "The Projects board is a Kanban of every project, grouped into six stage columns: Planned, Launching, Active, Review, Blocked, and Verified.",
      "Each project card shows quick chips: a git drift indicator (↑/↓/∆), an open Censor-finding count (⚠N), a task-state breakdown (done vs total, plus blocked), and the next milestone. These update live as agents work.",
      "A Calendar toggle button reveals a single view of every project deadline and release date.",
    ],
    link: { label: "Open the Projects board", view: "projects" },
  },
  {
    id: "work-console",
    title: "Inside a project (Work console)",
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
      "Four agent roles do the work. The orchestrator turns your goal into a plan and coordinates the others. A coder implements the plan by driving an external CLI or the local mini-coder. A verifier reviews completed work and closes tasks. A mini-coder is a small delegated session a coder spawns to handle a sub-task.",
      "AI models are configured in Settings → Providers & Models. External CLIs (codex / claude / openai) must be installed on your machine; Devboule auto-detects them and lists them as launch options.",
    ],
  },
  {
    id: "censor",
    title: "Censor",
    body: [
      "The Censor is Devboule's local review gate. It runs deterministic linters and applies AI review tiers as agents write code, then raises findings you can triage inside the project.",
      "Each project has a trust gate: the Censor only starts enforcing once you trust the project, so it stays inert until then.",
    ],
  },
  {
    id: "oracle",
    title: "Oracle",
    body: [
      "Oracle answers questions about your codebase using the indexed repository, so agents and you can recover real context before acting.",
      "Configure and use it from the standalone Oracle page, reachable from the sidebar.",
    ],
    link: { label: "Open Oracle", view: "oracle" },
  },
  {
    id: "skills-tools",
    title: "Skills & Tools",
    body: [
      "Skills are reusable playbooks (SKILL.md manuals) and tools (MCP servers) that make the agents smarter across every project. Open the Skills page to browse the library and install tools.",
    ],
    link: { label: "Open the Skills page", view: "skills" },
  },
  {
    id: "keys-providers",
    title: "Keys & providers",
    body: [
      "API tokens and provider keys live in Settings → Security (Secrets).",
      "AI models and provider connections are configured in Settings → Providers & Models.",
    ],
  },
  {
    id: "tips",
    title: "Tips",
    body: [
      "Hold Alt anywhere to enter Help mode: a floating overlay explains what each control does and why it matters for Devboule.",
      "Use the header search (the magnifying-glass jump-search) to jump straight to any page by typing part of its name.",
      "The bell / notification icon in the header shows agents that need you — a question, an approve/deny decision, or a risk flag — so you never miss a hand-off.",
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
              <p
                key={index}
                className="mb-2 text-sm leading-6 text-cream-600"
              >
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
