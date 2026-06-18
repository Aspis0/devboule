import { Network } from "lucide-react";
import { McpServerList } from "../settings/UserMcpServersCard";

// Project-scope MCP servers card. Renders inside the project workspace dock's
// "MCP" tab. Passes scope="project" + the active projectRoot to McpServerList,
// which handles all list / toggle / remove / add-dialog logic.
//
// projectRoot MUST be the absolute path to the project root (the same value as
// `project.metadata.rootPath`). The backend resolves
// `<projectRoot>/.devboule/mcp-servers.json` from this value and rejects
// path-traversal attempts.

export interface ProjectMcpServersCardProps {
  /** Absolute path to the project root. Required for project-scope commands. */
  projectRoot: string;
}

export function ProjectMcpServersCard({ projectRoot }: ProjectMcpServersCardProps) {
  return (
    <section
      data-help-title="Project-scoped MCP servers — available in this project only."
      data-help-lines="Stored in .devboule/mcp-servers.json in the project root, so they can be committed to the repo.|On name collision with a global server, the project entry wins.|MCP servers run as your user account and may have network access.|The Devboule Oracle is always present separately and never appears here."
    >
      <div className="mb-3 flex items-center gap-2">
        <Network className="h-4 w-4 text-amber-dark" />
        <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          MCP servers (project)
        </h3>
      </div>
      <p className="mb-4 text-[12px] leading-5 text-cream-500">
        Project-scoped servers live in{" "}
        <span className="font-mono">.devboule/mcp-servers.json</span> and are
        available in this project only. On name collision with a global server,
        this project entry wins.
      </p>
      <McpServerList scope="project" projectRoot={projectRoot} />
    </section>
  );
}

export const __test_ProjectMcpServersCard = ProjectMcpServersCard;
