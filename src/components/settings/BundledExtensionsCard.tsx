// Bundled extensions settings card — visibility + essential management for the
// must-have pi extensions the app ships. Each extension is its own collapsible
// row with a one-line status header (compact pattern from WebSearchCard).
//
// Four npm-installed rows: Subagents (with agent list panel), pi-lens,
// Compactor, Web search (status-only + hint to the Web search settings above).

import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Puzzle,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { invokeBackendCommand } from "../../context/AppContext";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface ExtensionStatus {
  agentDir: string;
  mode: "global" | "appManaged" | "envOverride";
  bootstrap: "idle" | "running" | "done" | "failed";
  bootstrapError: string | null;
}

interface InstalledExtension {
  source: string;
  name: string;
  version: string;
  description: string;
  author: string;
  installedOk: boolean;
}

interface AgentDefinition {
  name: string;
  description: string;
  model: string;
  file: string;
}

// ---------------------------------------------------------------------------
// Known bundled extensions
// ---------------------------------------------------------------------------

interface BundledSpec {
  /** npm source used in settings.json. */
  source: string;
  /** Display name. */
  label: string;
  /** One-liner description. */
  description: string;
}

const BUNDLED_EXTENSIONS: BundledSpec[] = [
  {
    source: "npm:@tintinweb/pi-subagents",
    label: "Subagents",
    description: "Multi-agent orchestration with custom agent definitions.",
  },
  {
    source: "npm:pi-lens",
    label: "pi-lens",
    description: "Real-time code feedback (LSP, linters). Zero-config.",
  },
  {
    source: "npm:@pi-unipi/compactor",
    label: "Compactor",
    description: "Deterministic context compaction — automatic, no LLM.",
  },
  {
    source: "npm:pi-web-access",
    label: "Web search",
    description:
      "Web search integration for pi sessions. Configure providers and keys in the Web search section above.",
  },
];

// ---------------------------------------------------------------------------
// Collapsible sub-row
// ---------------------------------------------------------------------------

function ExtensionRow({
  spec,
  installed,
  version,
  children,
}: {
  spec: BundledSpec;
  installed: boolean;
  version: string;
  children?: React.ReactNode;
}) {
  const [isOpen, setIsOpen] = useState(false);
  const contentId = `ext-${spec.source.replace(/[^a-z0-9]/gi, "-")}`;

  return (
    <div className="rounded-xl border border-cream-100 bg-cream-50/40">
      <button
        type="button"
        onClick={() => setIsOpen(!isOpen)}
        aria-expanded={isOpen}
        aria-controls={contentId}
        className="flex w-full items-center justify-between gap-2 px-3 py-2.5 text-left"
      >
        <div className="flex min-w-0 flex-1 items-center gap-2">
          {installed ? (
            <CheckCircle2 className="h-3.5 w-3.5 shrink-0 text-sage-dark" />
          ) : (
            <AlertTriangle className="h-3.5 w-3.5 shrink-0 text-amber-dark" />
          )}
          <span className="truncate text-[12px] font-semibold text-cream-800">
            {spec.label}
          </span>
          {version && (
            <span className="shrink-0 rounded-full bg-cream-200 px-1.5 py-0.5 text-[10px] text-cream-600">
              {version}
            </span>
          )}
          {!installed && (
            <span className="shrink-0 rounded-full bg-amber/10 px-1.5 py-0.5 text-[10px] font-semibold text-amber-dark">
              not installed
            </span>
          )}
        </div>
        {isOpen ? (
          <ChevronDown className="h-4 w-4 shrink-0 text-cream-400" />
        ) : (
          <ChevronRight className="h-4 w-4 shrink-0 text-cream-400" />
        )}
      </button>
      <div id={contentId} className={isOpen ? "border-t border-cream-100 px-3 py-2" : "hidden"}>
        <p className="mb-1 text-[11px] leading-4 text-cream-500">{spec.description}</p>
        {!installed && (
          <p className="text-[10px] text-cream-400">
            First launch installs them on app-managed setups.
          </p>
        )}
        {children}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Subagents detail panel
// ---------------------------------------------------------------------------

function SubagentsPanel({ agents, agentDir }: { agents: AgentDefinition[]; agentDir: string }) {
  if (agents.length === 0) {
    return (
      <div className="mt-2 rounded-lg bg-white p-2">
        <p className="text-[11px] text-cream-400">No agent definitions yet.</p>
        <p className="mt-1 font-mono text-[10px] text-cream-300">{agentDir}/agents/</p>
      </div>
    );
  }

  return (
    <ul className="mt-2 space-y-1">
      {agents.map((agent) => (
        <li
          key={agent.file}
          className="flex items-center justify-between gap-2 rounded-lg bg-white px-2.5 py-1.5"
        >
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="text-[11px] font-semibold text-cream-800">{agent.name}</span>
              {agent.model && (
                <span className="rounded-full bg-teal/10 px-1.5 py-0.5 text-[10px] font-medium text-teal">
                  {agent.model}
                </span>
              )}
            </div>
            {agent.description && (
              <p className="mt-0.5 truncate text-[10px] text-cream-400">{agent.description}</p>
            )}
          </div>
        </li>
      ))}
    </ul>
  );
}

// ---------------------------------------------------------------------------
// Main card
// ---------------------------------------------------------------------------

export function BundledExtensionsCard() {
  const [status, setStatus] = useState<ExtensionStatus | null>(null);
  const [installed, setInstalled] = useState<InstalledExtension[]>([]);
  const [agents, setAgents] = useState<AgentDefinition[]>([]);
  const [loading, setLoading] = useState(true);
  const mountedRef = useRef(true);
  const loadSeqRef = useRef(0);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const loadAll = useCallback(async () => {
    const seq = loadSeqRef.current + 1;
    loadSeqRef.current = seq;
    setLoading(true);
    try {
      const [s, list, agentList] = await Promise.all([
        invokeBackendCommand<ExtensionStatus>("pi_extensions_status"),
        invokeBackendCommand<InstalledExtension[]>("pi_extensions_list"),
        invokeBackendCommand<AgentDefinition[]>("pi_agents_list"),
      ]);
      if (!mountedRef.current || loadSeqRef.current !== seq) return;
      setStatus(s);
      setInstalled(Array.isArray(list) ? list : []);
      setAgents(Array.isArray(agentList) ? agentList : []);
    } catch {
      // Non-fatal: card renders with partial data.
    } finally {
      if (mountedRef.current && loadSeqRef.current === seq) setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  // Helper: find an installed extension by npm source.
  const findInstalled = useCallback(
    (source: string) => installed.find((ext) => ext.source === source),
    [installed],
  );

  return (
    <section
      className="rounded-2xl border border-cream-200 bg-white p-4"
      data-help-title="Bundled extensions — status and visibility for the must-have pi extensions."
      data-help-lines="These extensions ship with the app and are installed on first launch in app-managed mode.|Each row shows installed/missing status and essential details.|Subagents lists your custom agent definitions.|Web search links to the Web search settings section above."
    >
      <div className="mb-3 flex items-center gap-2">
        <Puzzle className="h-4 w-4 text-teal" />
        <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          Bundled extensions
        </h3>
      </div>

      <p className="mb-3 max-w-3xl text-[12px] leading-5 text-cream-500">
        Extensions that ship with the app. They are installed automatically on
        first launch in app-managed mode.
      </p>

      {loading && !status ? (
        <p className="text-[11px] text-cream-400">Loading extension info…</p>
      ) : (
        <div className="space-y-2">
          {/* Subagents */}
          <ExtensionRow
            spec={BUNDLED_EXTENSIONS[0]}
            installed={!!findInstalled(BUNDLED_EXTENSIONS[0].source)}
            version={findInstalled(BUNDLED_EXTENSIONS[0].source)?.version ?? ""}
          >
            <SubagentsPanel agents={agents} agentDir={status?.agentDir ?? ""} />
          </ExtensionRow>

          {/* pi-lens */}
          <ExtensionRow
            spec={BUNDLED_EXTENSIONS[1]}
            installed={!!findInstalled(BUNDLED_EXTENSIONS[1].source)}
            version={findInstalled(BUNDLED_EXTENSIONS[1].source)?.version ?? ""}
          />

          {/* Compactor */}
          <ExtensionRow
            spec={BUNDLED_EXTENSIONS[2]}
            installed={!!findInstalled(BUNDLED_EXTENSIONS[2].source)}
            version={findInstalled(BUNDLED_EXTENSIONS[2].source)?.version ?? ""}
          />

          {/* Web search — status-only, hint points to Web search settings above */}
          <ExtensionRow
            spec={BUNDLED_EXTENSIONS[3]}
            installed={!!findInstalled(BUNDLED_EXTENSIONS[3].source)}
            version={findInstalled(BUNDLED_EXTENSIONS[3].source)?.version ?? ""}
          />
        </div>
      )}
    </section>
  );
}

export default BundledExtensionsCard;
