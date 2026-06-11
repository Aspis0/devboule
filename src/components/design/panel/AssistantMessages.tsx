// AssistantMessages — the scrollable transcript (panel.jsx AssistantMessages).
//
// Empty state: the prototype's intro copy + three suggestion buttons. Clicking a
// suggestion SEEDS the composer draft (does NOT send) — matching app.jsx onSuggest.
//
// Messages:
//   - user: a right-aligned bubble with an optional ctx-chip ("Editing <node>").
//   - assistant: a card whose icon/title/desc reflect the run lifecycle
//     (working/done/error), an optional source list (HTTP grounding) OR a muted
//     "grounds agentically via MCP" note for CLI providers (B4), and a foot of
//     actions: Stop while working; Select-on-canvas + Regenerate when done; Retry on
//     error.

import {
  AlertTriangle,
  CheckCircle2,
  FileCode,
  Loader2,
  RotateCcw,
  Square,
  Wand2,
} from "lucide-react";
import { SUGGESTIONS, type AssistantMessage } from "./types";

export interface AssistantMessagesProps {
  messages: AssistantMessage[];
  /** Seed the composer draft from a suggestion (does not send). */
  onSuggest: (text: string) => void;
  /** Re-run the instruction behind an assistant card (Regenerate / Retry). */
  onRerun: (msg: AssistantMessage) => void;
  /** Select the node(s) a done card created/edited, on the canvas. */
  onLocate: (nodeIds: string[]) => void;
  /** Cancel the in-flight run (the working card's Stop). */
  onStop: () => void;
  /** Scroll container ref (parent auto-scrolls to bottom on new messages). */
  scrollRef: React.RefObject<HTMLDivElement>;
}

export function AssistantMessages({
  messages,
  onSuggest,
  onRerun,
  onLocate,
  onStop,
  scrollRef,
}: AssistantMessagesProps) {
  return (
    <div className="assist-scroll" ref={scrollRef}>
      {messages.length === 0 ? (
        <>
          <p className="sugg-intro">
            Describe a section and it&apos;s generated{" "}
            <b style={{ fontWeight: 600, color: "var(--ink-2)" }}>
              grounded in your real codebase
            </b>{" "}
            — components, palette and copy included. Select a node on the canvas to edit
            just that node.
          </p>
          {SUGGESTIONS.map((s, i) => {
            const Icon = s.icon;
            return (
              <button
                key={i}
                type="button"
                className="sugg"
                onClick={() => onSuggest(s.text)}
              >
                <Icon size={16} />
                {s.text}
              </button>
            );
          })}
        </>
      ) : null}

      {messages.map((m) =>
        m.role === "user" ? (
          <div key={m.id} className="msg-user">
            {m.ctx ? (
              <div className="ctx">
                <span className="ctx-chip">
                  <Wand2 size={11} />
                  {m.ctx}
                </span>
              </div>
            ) : null}
            <div className="bubble">{m.text}</div>
          </div>
        ) : (
          <div
            key={m.id}
            className={"msg-ai" + (m.status === "error" ? " err" : "")}
          >
            <div className="card">
              <div className="head">
                <span
                  className={
                    "ic" +
                    (m.status === "working" ? " spin" : "") +
                    (m.status === "error" ? " alert" : "")
                  }
                >
                  {m.status === "working" ? (
                    <Loader2 size={15} />
                  ) : m.status === "error" ? (
                    <AlertTriangle size={15} />
                  ) : (
                    <CheckCircle2 size={15} />
                  )}
                </span>
                {m.title}
              </div>
              {m.desc ? <div className="desc">{m.desc}</div> : null}

              {/* HTTP providers: the fetched grounding sources. CLI providers (B4):
                  no fetched sources — a single muted note that grounding is agentic. */}
              {m.agentic ? (
                <div className="src-list">
                  <span className="src-chip" data-agentic="true">
                    <FileCode size={11} />
                    grounds agentically via MCP
                  </span>
                </div>
              ) : m.sources && m.sources.length > 0 ? (
                <div className="src-list">
                  {m.sources.map((s, j) => (
                    <span key={j} className="src-chip">
                      <FileCode size={11} />
                      {s}
                    </span>
                  ))}
                </div>
              ) : null}

              {m.status === "working" ? (
                <div className="foot">
                  <button
                    type="button"
                    className="mini-btn"
                    onClick={onStop}
                  >
                    <Square size={12} />
                    Stop
                  </button>
                </div>
              ) : m.status === "done" ? (
                <div className="foot">
                  {m.nodeIds && m.nodeIds.length > 0 ? (
                    <button
                      type="button"
                      className="mini-btn"
                      onClick={() => onLocate(m.nodeIds!)}
                    >
                      Select on canvas
                    </button>
                  ) : null}
                  {m.instruction ? (
                    <button
                      type="button"
                      className="mini-btn"
                      onClick={() => onRerun(m)}
                    >
                      Regenerate
                    </button>
                  ) : null}
                </div>
              ) : m.status === "error" ? (
                <div className="foot">
                  <button
                    type="button"
                    className="mini-btn"
                    onClick={() => onRerun(m)}
                  >
                    <RotateCcw size={12} />
                    Retry
                  </button>
                </div>
              ) : null}
            </div>
          </div>
        ),
      )}
    </div>
  );
}

export default AssistantMessages;
