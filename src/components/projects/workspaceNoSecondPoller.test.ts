import { describe, expect, it } from "vitest";
// Vite `?raw` imports inline the file's text at transform time (no node:fs, which
// this tsconfig has no types for). We assert against the raw component source.
import projectWorkspaceSrc from "./ProjectWorkspace.tsx?raw";
import projectWorkspaceAgentRailSrc from "./ProjectWorkspaceAgentRail.tsx?raw";

// Invariant (Phase D): the Work-mode shell reuses the SINGLE agent-state poller
// owned by ProjectsView. ProjectWorkspace / ProjectWorkspaceAgentRail must NOT
// start their own poller or call live-state IPC directly — a second poller would
// double the list_projects / get_agent_live_state load and could race the canonical
// one. This is a cheap static source-string assertion (no runtime), matching the
// "no live-state IPC here" contract documented in those components.

// Strip block/line comments so a doc-comment MENTION of a command name (e.g.
// "from the board's agent_pty_list") doesn't trip the check — we only care about
// actual code: a real `setInterval(` call or a quoted IPC command literal.
function codeOnly(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/[^\n]*/g, "");
}

// Forbidden ACTUAL usages (not comment text):
//  - `setInterval(` — a self-owned poller.
//  - a quoted live-state IPC command — calling the backend poll directly.
const FORBIDDEN: RegExp[] = [
  /\bsetInterval\s*\(/,
  /["'`]get_agent_live_state["'`]/,
  /["'`]agent_pty_list["'`]/,
];

describe("Work-mode shell holds no second poller", () => {
  const cases: [string, string][] = [
    ["ProjectWorkspace.tsx", projectWorkspaceSrc],
    ["ProjectWorkspaceAgentRail.tsx", projectWorkspaceAgentRailSrc],
  ];
  for (const [file, src] of cases) {
    it(`${file} contains no own poller / live-state IPC`, () => {
      const code = codeOnly(src);
      for (const pattern of FORBIDDEN) {
        expect(code, `${file} must not match ${pattern}`).not.toMatch(pattern);
      }
    });
  }
});
