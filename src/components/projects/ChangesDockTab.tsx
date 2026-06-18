import { useCallback, useEffect, useRef, useState } from "react";
import { invokeBackendCommand } from "../../context/AppContext";
import { safeOpenExternal } from "../../utils/safeOpenExternal";
import type { ProjectDetail } from "../../types/backend";
import { RefreshCw, ExternalLink, SquareArrowOutUpRight } from "lucide-react";

export interface ChangesDockTabProps {
  project: ProjectDetail;
}

const EDITOR_LABELS: Record<string, string> = {
  code: "VS Code",
  cursor: "Cursor",
  zed: "Zed",
  idea: "IntelliJ IDEA",
};

/// WARNING 7: hard cap on how many diff lines we render. One <div> per line means a
/// 200KB diff (~5000 lines) would create thousands of DOM nodes and jank on paint.
/// We render the first MAX_RENDERED_LINES and show a note pointing the user to an
/// editor for the full diff. The backend's 200KB byte cap still applies on top.
const MAX_RENDERED_LINES = 800;

function lineClass(line: string): string {
  if (
    line.startsWith("+++ ") ||
    line.startsWith("--- ") ||
    line.startsWith("diff ") ||
    line.startsWith("index ")
  ) {
    return "text-cream-400";
  }
  if (line.startsWith("@@")) {
    return "text-indigo-dark";
  }
  if (line.startsWith("+")) {
    return "text-sage-dark bg-sage/10";
  }
  if (line.startsWith("-")) {
    return "text-coral-dark bg-coral/[0.06]";
  }
  // Untracked-files section markers from the backend.
  if (line.startsWith("# Untracked") || line.startsWith("?? ")) {
    return "text-cream-400";
  }
  return "text-cream-600";
}

export function ChangesDockTab({ project }: ChangesDockTabProps) {
  const projectId = project.metadata.id;
  const rootPath = project.metadata.rootPath ?? "";
  const prUrl = project.gitStatus?.pullRequestUrl ?? null;

  const [diff, setDiff] = useState<string>("");
  const [loading, setLoading] = useState<boolean>(false);
  const [error, setError] = useState<string | null>(null);
  const [editors, setEditors] = useState<string[]>([]);
  const [editorError, setEditorError] = useState<string | null>(null);

  // WARNING 5: request token so only the LATEST diff fetch (effect or Refresh) is
  // allowed to write state. Incremented at the start of every fetch and on unmount;
  // a fetch whose token no longer matches `requestToken.current` is stale and drops
  // its result. Shared by the mount effect and the Refresh button.
  const requestToken = useRef(0);

  const fetchDiff = useCallback(async () => {
    if (!rootPath) return;
    const token = ++requestToken.current;
    setLoading(true);
    setError(null);
    try {
      const out = await invokeBackendCommand<string>("git_working_diff", { projectId });
      if (token !== requestToken.current) return; // a newer fetch superseded us
      setDiff(out ?? "");
    } catch (e) {
      if (token !== requestToken.current) return;
      setError(e instanceof Error ? e.message : String(e));
      setDiff("");
    } finally {
      // WARNING 6: only the latest fetch clears the loading flag.
      if (token === requestToken.current) setLoading(false);
    }
  }, [projectId, rootPath]);

  useEffect(() => {
    if (!rootPath) {
      // No root: cancel any in-flight fetch and reset.
      requestToken.current++;
      setDiff("");
      setEditors([]);
      setError(null);
      setLoading(false);
      return;
    }

    const token = ++requestToken.current;
    setLoading(true);
    setError(null);

    (async () => {
      try {
        const out = await invokeBackendCommand<string>("git_working_diff", { projectId });
        if (token !== requestToken.current) return;
        setDiff(out ?? "");
      } catch (e) {
        if (token !== requestToken.current) return;
        setError(e instanceof Error ? e.message : String(e));
        setDiff("");
      }

      // WARNING 6: clear loading only after BOTH the diff and the editor probe have
      // resolved, so the spinner doesn't stop mid-render (brief layout shift).
      try {
        const eds = await invokeBackendCommand<string[]>("list_external_editors", {});
        if (token !== requestToken.current) return;
        setEditors(eds ?? []);
        setEditorError(null);
      } catch (e) {
        if (token !== requestToken.current) return;
        setEditorError(e instanceof Error ? e.message : String(e));
        setEditors([]);
      } finally {
        if (token === requestToken.current) setLoading(false);
      }
    })();

    // On unmount / rootPath change: invalidate any in-flight fetch so it cannot set
    // stale state (WARNING 5).
    return () => {
      requestToken.current++;
    };
  }, [projectId, rootPath]);

  const handleOpenEditor = useCallback(
    async (editor: string) => {
      try {
        await invokeBackendCommand<void>("open_in_editor", { projectId, editor });
      } catch (e) {
        setEditorError(e instanceof Error ? e.message : String(e));
      }
    },
    [projectId]
  );

  if (!rootPath) {
    return (
      <div className="px-3 py-2 text-[11px] text-cream-500">Set a project root to see changes.</div>
    );
  }

  const allLines = diff.length > 0 ? diff.split("\n") : [];
  const truncated = allLines.length > MAX_RENDERED_LINES;
  const lines = truncated ? allLines.slice(0, MAX_RENDERED_LINES) : allLines;

  return (
    <div className="flex flex-col gap-2 px-3 py-2">
      <div className="flex items-center justify-between gap-2">
        <span className="text-[11px] font-medium text-cream-500">Working changes</span>
        <button
          type="button"
          onClick={() => void fetchDiff()}
          disabled={loading}
          className="flex items-center gap-1 rounded border border-cream-200 px-1.5 py-0.5 text-[11px] text-cream-600 hover:bg-cream-100 disabled:opacity-50"
        >
          <RefreshCw size={11} className={loading ? "animate-spin" : ""} />
          Refresh
        </button>
      </div>

      <div className="flex flex-wrap items-center gap-1.5">
        {editors.map((ed) => (
          <button
            key={ed}
            type="button"
            onClick={() => handleOpenEditor(ed)}
            className="flex items-center gap-1 rounded border border-cream-200 px-1.5 py-0.5 text-[11px] text-cream-600 hover:bg-cream-100"
          >
            <ExternalLink size={11} />
            Open in {EDITOR_LABELS[ed] ?? ed}
          </button>
        ))}
        {prUrl && (
          <button
            type="button"
            onClick={() => safeOpenExternal(prUrl)}
            className="flex items-center gap-1 rounded border border-cream-200 px-1.5 py-0.5 text-[11px] text-terracotta hover:bg-cream-100"
          >
            <SquareArrowOutUpRight size={11} />
            Open PR ↗
          </button>
        )}
      </div>

      {editorError && (
        <div className="text-[11px] text-coral-dark">{editorError}</div>
      )}

      <div className="max-h-[360px] overflow-auto rounded border border-cream-200 bg-cream-50/50 font-mono text-[12px]">
        {loading ? (
          <div className="px-2 py-1.5 text-[11px] text-cream-500">Loading changes…</div>
        ) : error ? (
          <div className="px-2 py-1.5 text-[11px] text-coral-dark">{error}</div>
        ) : lines.length === 0 ? (
          <div className="px-2 py-1.5 text-[11px] text-cream-500">No uncommitted changes.</div>
        ) : (
          <>
            {lines.map((line, i) => (
              <div key={i} className={`whitespace-pre px-2 ${lineClass(line)}`}>
                {line}
              </div>
            ))}
            {truncated && (
              <div className="px-2 py-1.5 text-[11px] text-cream-500">
                View truncated — open in an editor for the full diff.
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}
