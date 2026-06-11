// AssistantPanel — the design module's right column (panel.jsx AssistantPanel).
//
// A presentational shell: header (Sparkles + "Assistant" + "<N> generations" once any
// run has completed), the auto-scrolling transcript, and the composer. All behavior is
// driven by props from DesignView, which owns the real generation pipeline; this panel
// adds NO pipeline logic.

import { useEffect, useRef } from "react";
import { Sparkles, ScanEye } from "lucide-react";
import { AssistantMessages } from "./AssistantMessages";
import { Composer } from "./Composer";
import type { AssistantMessage } from "./types";
import type { DesignLlmBackend } from "../../../types/config";

export interface AssistantPanelProps {
  width: number;
  messages: AssistantMessage[];
  /** Count of completed generations this session (the header subtitle). */
  doneCount: number;
  selectedNodeName: string | null;
  onClearContext: () => void;
  onSend: (text: string) => void;
  onSuggest: (text: string) => void;
  onRerun: (msg: AssistantMessage) => void;
  onLocate: (nodeIds: string[]) => void;
  onStop: () => void;
  busy: boolean;
  backend: DesignLlmBackend | null;
  onSaveBackend: (next: DesignLlmBackend) => void;
  onOpenSettings: () => void;
  draft: string;
  setDraft: (value: string) => void;
  focusSignal: number;
  /** Out-of-band status line (load/save/export feedback not tied to a generation). */
  notice?: string | null;
  /** Out-of-band error (folder/load/save failures) shown as a muted error strip. */
  error?: string | null;
  /** Run a visual check (capture the preview + critique). The assist-head icon-button. */
  onVisualCheck: () => void;
  /** Disable the visual-check button (no project open). */
  visualCheckDisabled: boolean;
  /** True while a visual check is in flight (shows a spinner + disables the button). */
  visualChecking: boolean;
}

export function AssistantPanel({
  width,
  messages,
  doneCount,
  selectedNodeName,
  onClearContext,
  onSend,
  onSuggest,
  onRerun,
  onLocate,
  onStop,
  busy,
  backend,
  onSaveBackend,
  onOpenSettings,
  draft,
  setDraft,
  focusSignal,
  notice,
  error,
  onVisualCheck,
  visualCheckDisabled,
  visualChecking,
}: AssistantPanelProps) {
  const scrollRef = useRef<HTMLDivElement>(null);

  // Auto-scroll to the bottom whenever the transcript changes (prototype idiom).
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages]);

  const hasAssistant = messages.some((m) => m.role === "assistant");

  return (
    <aside
      className="assist"
      style={{ width }}
      data-screen-label="Assistant panel"
    >
      <div className="assist-head">
        <span className="spark">
          <Sparkles size={17} />
        </span>
        <span className="ttl">Assistant</span>
        <span className="sub">
          {hasAssistant ? `${doneCount} generations` : ""}
        </span>
        <span className="visual-check">
          <button
            type="button"
            className="icon-btn"
            onClick={onVisualCheck}
            disabled={visualCheckDisabled || visualChecking}
            title="Visual check"
            aria-label="Visual check"
            aria-busy={visualChecking}
          >
            <ScanEye size={16} className={visualChecking ? "spin" : undefined} />
          </button>
        </span>
      </div>
      <AssistantMessages
        messages={messages}
        onSuggest={onSuggest}
        onRerun={onRerun}
        onLocate={onLocate}
        onStop={onStop}
        scrollRef={scrollRef}
      />
      {error ? (
        <p
          className="assist-notice err"
          style={{
            flex: "none",
            margin: 0,
            padding: "0 var(--pad) 8px",
            fontSize: "11.5px",
            lineHeight: 1.4,
            color: "#A33715",
          }}
        >
          {error}
        </p>
      ) : notice ? (
        <p
          className="assist-notice"
          style={{
            flex: "none",
            margin: 0,
            padding: "0 var(--pad) 8px",
            fontSize: "11.5px",
            lineHeight: 1.4,
            color: "var(--muted)",
          }}
        >
          {notice}
        </p>
      ) : null}
      <Composer
        selectedNodeName={selectedNodeName}
        onClearContext={onClearContext}
        onSend={onSend}
        busy={busy}
        backend={backend}
        onSaveBackend={onSaveBackend}
        onOpenSettings={onOpenSettings}
        draft={draft}
        setDraft={setDraft}
        focusSignal={focusSignal}
      />
    </aside>
  );
}

export default AssistantPanel;
