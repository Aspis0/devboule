import { describe, expect, it } from "vitest";
// Vite `?raw` imports inline the file's text at transform time (no node:fs, which
// this tsconfig has no types for). We assert against the raw component source.
import projectsViewSrc from "../views/ProjectsView.tsx?raw";
import appSrc from "../../App.tsx?raw";
import headerSrc from "../Header.tsx?raw";
import sidebarSrc from "../Sidebar.tsx?raw";
import deepLinkSrc from "../../utils/deepLink.ts?raw";

// Invariant (Phase G): the standalone Agents page is dissolved. Work mode (Phase D)
// + the Censor dock (Phase E) replace it. After removal there must be NO lingering
// reference to the deleted AgentsView component, the removed `projectsTab` machinery,
// or the dead `projects#agents` deep-link. These are cheap static source-string
// assertions (no runtime) so a regression that re-introduces the page is caught at
// test time even before `npm run build` fails on the deleted file.

// Strip block/line comments so a doc-comment MENTION (e.g. "formerly AgentsView")
// doesn't trip the check — we only care about actual code references.
function codeOnly(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/[^\n]*/g, "");
}

describe("Agents page is dissolved (Phase G)", () => {
  // Extended beyond ProjectsView/App to the files most likely to RE-INTRODUCE the
  // dissolved page: Header.tsx + Sidebar.tsx (an "Agents" nav/jump target) and
  // deepLink.ts (a `projects#agents` route or AgentsView wiring). Comment mentions
  // (e.g. "the Agents page was dissolved") are stripped by codeOnly first, so a
  // legitimate doc note never trips these checks; only real code references do.
  const cases: [string, string][] = [
    ["ProjectsView.tsx", projectsViewSrc],
    ["App.tsx", appSrc],
    ["Header.tsx", headerSrc],
    ["Sidebar.tsx", sidebarSrc],
    ["deepLink.ts", deepLinkSrc],
  ];

  for (const [file, src] of cases) {
    const code = codeOnly(src);

    it(`${file} has no AgentsView reference`, () => {
      expect(code, `${file} must not reference AgentsView`).not.toMatch(
        /\bAgentsView\b/,
      );
    });

    it(`${file} has no dead projects#agents deep-link`, () => {
      expect(code, `${file} must not contain projects#agents`).not.toMatch(
        /projects#agents/,
      );
    });

    it(`${file} has no projectsTab machinery`, () => {
      expect(code, `${file} must not reference projectsTab`).not.toMatch(
        /\bprojectsTab\b/,
      );
    });
  }
});
