import type { CSSProperties } from "react";
import type { QuestionEntry } from "../../agents/agentConsoleModel";
import { steerPickOption, steerYouDecide } from "./plannerModel";
import { leanIsSoft } from "./leanFieldMath";
import { LeanField } from "./LeanField";

// DoubtPanel — the LEFT half of the reused Plan view. It renders the orchestrator's
// OPEN Kairion doubts as cards: the question, the lean-field graphic (insecurity =
// instability), an HONEST lean line, the "shapes → <task>" link, and the moves.
//
// THREE MOVES, all via the EXISTING `onSend` (no new command): pick an option, press
// "you decide", or — handled by the shared chat composer below — type a plain reply.
// Each routes to orchestrator_steer (local) / project_cloud_orchestrator_send (cloud)
// exactly as PlannerChat's onSend already does.
//
// Degrades calmly: no doubts => a quiet "no open doubts" resting state (Kairion is
// always-on for the orchestrator but silent when it isn't split on anything).

interface DoubtPanelProps {
  questions: QuestionEntry[];
  /** Send a plain steer line (the existing orchestrator_steer / cloud send round-trip). */
  onSend: (text: string) => void;
  /** Doubt ids to highlight because their linked task card is hovered. */
  highlightedDoubtIds: Set<string>;
  /** Hover a doubt (id) / leave (null) — drives the task-card highlight upstream. */
  onHoverDoubt: (id: string | null) => void;
}

const COL_HEADER: CSSProperties = {
  display: "flex",
  justifyContent: "space-between",
  fontSize: 9,
  letterSpacing: "0.16em",
  textTransform: "uppercase",
  color: "#9c9488",
  margin: "0 2px 9px",
};

export function DoubtPanel({
  questions,
  onSend,
  highlightedDoubtIds,
  onHoverDoubt,
}: DoubtPanelProps) {
  return (
    <div
      className="pp-scroll"
      style={{
        flex: 1.3,
        minWidth: 0,
        overflowY: "auto",
        paddingRight: 11,
        borderRight: "1px solid #ECE6DB",
      }}
    >
      <div className="pp-mono" style={COL_HEADER}>
        <span>open doubts</span>
        <span>{questions.length}</span>
      </div>

      {questions.length === 0 ? (
        <div style={{ color: "#9c9488", fontSize: 12, padding: "18px 4px" }}>
          no open doubts — the plan is firming up.
        </div>
      ) : (
        questions.map((q) => (
          <DoubtCard
            key={q.id}
            q={q}
            onSend={onSend}
            linked={highlightedDoubtIds.has(q.id)}
            onHover={onHoverDoubt}
          />
        ))
      )}
    </div>
  );
}

function DoubtCard({
  q,
  onSend,
  linked,
  onHover,
}: {
  q: QuestionEntry;
  onSend: (text: string) => void;
  linked: boolean;
  onHover: (id: string | null) => void;
}) {
  const reopened = q.status === "reopened";
  const soft = q.lean !== null && leanIsSoft(q.directionConfidence);

  const cardStyle: CSSProperties = {
    background: "#fff",
    border: `1px solid ${reopened || linked ? "#C0894F" : "#E4DDD0"}`,
    borderRadius: 9,
    padding: "10px 11px 9px",
    marginBottom: 9,
    boxShadow: linked
      ? "0 0 0 2px #C0894F44"
      : reopened
        ? "0 0 0 1px #C0894F22"
        : "none",
    transition: "box-shadow .15s, border-color .15s",
    animation: "pp-crystal .6s ease-out",
  };

  return (
    <div
      style={cardStyle}
      onMouseEnter={() => onHover(q.id)}
      onMouseLeave={() => onHover(null)}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 7, marginBottom: 7 }}>
        <div style={{ flex: 1, fontSize: 12.5, fontWeight: 600, color: "#2A2621", lineHeight: 1.3 }}>
          {q.text}
        </div>
        {reopened && (
          <span
            className="pp-mono"
            style={{
              fontSize: 8.5,
              fontWeight: 700,
              letterSpacing: "0.08em",
              textTransform: "uppercase",
              padding: "2px 6px",
              borderRadius: 20,
              background: "#F1E4D2",
              color: "#9a6a33",
            }}
          >
            reopened
          </span>
        )}
      </div>

      <LeanField
        unrest={q.unrest}
        candidates={q.candidates}
        lean={q.lean}
        status={q.status}
        directionConfidence={q.directionConfidence}
      />

      {/* HONEST lean line — never a percentage. */}
      <div style={{ fontSize: 10.5, color: "#9c9488", margin: "5px 1px 8px", lineHeight: 1.4 }}>
        {q.lean === null ? (
          <span style={{ color: "#9a6a33", fontWeight: 600 }}>genuinely split</span>
        ) : soft ? (
          <>
            leaning <span style={{ color: "#C0894F", fontWeight: 600 }}>{q.lean}</span>
            <span style={{ color: "#B3AB9C" }}> · a soft hint, not a verdict</span>
          </>
        ) : (
          <>
            leaning <span style={{ color: "#C0894F", fontWeight: 600 }}>{q.lean}</span>
          </>
        )}
      </div>

      {q.affects.length > 0 && (
        <div
          className="pp-mono"
          style={{ fontSize: 9, color: "#9c9488", margin: "6px 1px 0" }}
        >
          shapes → <span style={{ color: "#C0894F" }}>{q.affects.join(", ")}</span>
        </div>
      )}

      {/* MOVE 1: pick an option. */}
      <div style={{ display: "flex", gap: 5, flexWrap: "wrap", marginTop: 8 }}>
        {q.options.map((o) => (
          <button
            key={o.id}
            type="button"
            onClick={() => onSend(steerPickOption(q, o))}
            style={{
              fontSize: 11.5,
              color: "#2A2621",
              background: "#FCFAF6",
              border: "1px solid #E4DDD0",
              borderRadius: 7,
              padding: "5px 9px",
              cursor: "pointer",
            }}
          >
            {o.label}
          </button>
        ))}
      </div>

      {/* MOVE 2: you decide. (MOVE 3 = a plain reply via the shared chat composer below.) */}
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          marginTop: 8,
        }}
      >
        <span className="pp-mono" style={{ fontSize: 9, color: "#B3AB9C" }}>
          your call
        </span>
        <button
          type="button"
          className="pp-mono"
          onClick={() => onSend(steerYouDecide(q))}
          style={{
            fontSize: 10,
            color: "#C0894F",
            background: "none",
            border: "none",
            cursor: "pointer",
            padding: 0,
          }}
        >
          you decide →
        </button>
      </div>
    </div>
  );
}
