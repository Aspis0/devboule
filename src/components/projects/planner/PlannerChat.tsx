import { useState } from "react";
import type { KeyboardEvent } from "react";
import { Send } from "lucide-react";
import type { PlannerMessage } from "./plannerModel";
import { ChatThread } from "../../activity/ChatThread";

interface PlannerChatProps {
  messages: PlannerMessage[];
  modelLabel: string;
  live: boolean;
  awaitingReply: boolean;
  onSend: (text: string) => void;
}

export function PlannerChat({
  messages,
  modelLabel,
  live,
  awaitingReply,
  onSend,
}: PlannerChatProps) {
  const [value, setValue] = useState("");

  const send = () => {
    const trimmed = value.trim();
    if (!trimmed) return;
    onSend(trimmed);
    setValue("");
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  };

  return (
    <div
      style={{
        background: "#fff",
        border: "1px solid #E4DDD0",
        borderRadius: 12,
        overflow: "hidden",
        display: "flex",
        flexDirection: "column",
        minHeight: 340,
        maxHeight: 460,
        flex: 1,
      }}
    >
      {/* HEADER */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          padding: "9px 13px",
          borderBottom: "1px solid #EFE7DA",
          background: "#FCFAF6",
        }}
      >
        <span className="pp-mono" style={{ fontSize: 9.5, letterSpacing: 0.14, color: "#A89F90" }}>
          CHAT
        </span>
        <span className="pp-mono" style={{ fontSize: 9.5, color: "#9c9488" }}>
          {modelLabel}
        </span>
        <span style={{ marginLeft: "auto", display: "flex", alignItems: "center", gap: 5 }}>
          <span
            style={{
              width: 6,
              height: 6,
              borderRadius: "50%",
              background: live ? "#7FA468" : "#CFC6B6",
            }}
          />
          <span
            style={{
              fontSize: 10,
              fontWeight: 600,
              color: live ? "#5e8a4d" : "#9c9488",
            }}
          >
            {live ? "live" : "idle"}
          </span>
        </span>
      </div>

      <ChatThread messages={messages} live={live} awaitingReply={awaitingReply} />

      {/* COMPOSER */}
      <div
        style={{
          display: "flex",
          alignItems: "flex-end",
          gap: 8,
          padding: "9px 10px",
          borderTop: "1px solid #EFE7DA",
          background: "#FCFAF6",
        }}
      >
        <textarea
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={handleKeyDown}
          rows={1}
          placeholder="Message the Orchestrator…  (Enter to send)"
          style={{
            flex: 1,
            resize: "none",
            border: "1px solid #E4DDD0",
            borderRadius: 10,
            background: "#fff",
            padding: "10px 12px",
            fontSize: 13,
            color: "#2A2621",
            outline: "none",
            lineHeight: 1.4,
            maxHeight: 80,
          }}
        />
        <button
          onClick={send}
          style={{
            width: 38,
            height: 38,
            flex: "none",
            border: "none",
            background: "linear-gradient(150deg,#C8945C,#B07D43)",
            borderRadius: 10,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            color: "#FBF6EF",
            cursor: value.trim() ? "pointer" : "default",
            opacity: value.trim() ? 1 : 0.5,
            transition: "opacity 0.15s",
          }}
        >
          <Send size={16} />
        </button>
      </div>
    </div>
  );
}
