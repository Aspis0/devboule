import { useEffect, useState, useCallback } from "react";
import { invokeBackendCommand } from "../../context/AppContext";

interface McpServer {
  name: string;
  transport: string;
  command: string;
  args: string[];
  env: Record<string, string>;
  enabled: boolean;
}

const MAX_TOOLS = 5;

export function ToolsPicker({
  projectRoot,
  profile,
}: {
  projectRoot: string;
  profile: string;
}) {
  const [available, setAvailable] = useState<McpServer[]>([]);
  const [assigned, setAssigned] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);

  // Fetch Effect
  useEffect(() => {
    if (profile === "mini-small") return;

    let cancelled = false;

    const fetchTools = async () => {
      try {
        const [catalog, assignedNames] = await Promise.all([
          invokeBackendCommand("tools_library_list", {
            workingFolderPath: projectRoot,
          }),
          invokeBackendCommand("tools_assignment_list", {
            workingFolderPath: projectRoot,
            profile,
          }),
        ]);

        if (cancelled) return;

        setAvailable(catalog as McpServer[]);
        setAssigned(assignedNames as string[]);
      } catch {
        if (!cancelled) setError("Failed to load MCP tools.");
      }
    };

    fetchTools();

    return () => {
      cancelled = true;
    };
  }, [projectRoot, profile]);

  // Toggle Handler
  const toggleTool = useCallback(
    async (server: McpServer) => {
      const isAssigned = assigned.includes(server.name);
      const next = isAssigned
        ? assigned.filter((n) => n !== server.name)
        : [...assigned, server.name];

      setAssigned(next);

      try {
        await invokeBackendCommand("tools_assignment_set", {
          workingFolderPath: projectRoot,
          profile,
          names: next,
        });
        setError(null);
      } catch {
        // Undo ONLY this toggle (functional form) so a concurrent toggle isn't clobbered.
        setAssigned((prev) =>
          isAssigned
            ? [...prev, server.name]
            : prev.filter((n) => n !== server.name),
        );
        setError("Failed to update tools.");
      }
    },
    [assigned, projectRoot, profile],
  );

  // Render Mini-small
  if (profile === "mini-small") {
    return (
      <div
        data-testid="tools-picker-disabled"
        className="text-cream-400 text-sm"
      >
        This tier is edits-only — no tools.
      </div>
    );
  }

  // Render Empty
  if (available.length === 0) {
    return (
      <div data-testid="tools-empty" className="text-cream-400 text-sm">
        No MCP servers configured for this project.
      </div>
    );
  }

  // Render List
  return (
    <div className="flex flex-col gap-2">
      <div
        data-testid="tools-count"
        className="text-[10px] uppercase tracking-widest text-cream-400"
      >
        {assigned.length} / {MAX_TOOLS}
      </div>
      {error && (
        <div
          data-testid="tools-error"
          className="rounded-lg border border-coral/30 bg-coral/[0.05] px-2 py-1 text-[11px] text-coral-dark"
        >
          {error}
        </div>
      )}
      {available.map((server) => {
        const isAssigned = assigned.includes(server.name);
        const isDisabled = !isAssigned && assigned.length >= MAX_TOOLS;

        return (
          <button
            key={server.name}
            data-testid={`tools-row-${server.name}`}
            type="button"
            aria-pressed={isAssigned}
            disabled={isDisabled}
            onClick={() => toggleTool(server)}
            className={`rounded-xl border px-3 py-2 text-[12px] ${
              isAssigned
                ? "border-teal/30 bg-teal/10 text-teal"
                : isDisabled
                  ? "border-cream-200 bg-white text-cream-700 opacity-50 cursor-not-allowed"
                  : "border-cream-200 bg-white text-cream-700"
            }`}
          >
            <div className="font-medium">{server.name}</div>
            <div className="opacity-70">{server.command}</div>
          </button>
        );
      })}
    </div>
  );
}
