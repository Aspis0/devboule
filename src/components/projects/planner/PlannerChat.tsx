import { useState, useRef, useEffect } from "react";
import type { KeyboardEvent } from "react";
import { Send } from "lucide-react";
import type { PlannerMessage } from "./plannerModel";

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
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = scrollRef.current;
    if (el) {
      el.scrollTop = el.scrollHeight;
    }
    // Depend on the message array identity + last-message length so streaming
    // (text appended in place to the last entry) also keeps the view pinned.
  }, [messages, messages[messages.length - 1]?.text.length, awaitingReply]);

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

      {/* SCROLL AREA */}
      <div
        ref={scrollRef}
        className="pp-scroll"
        style={{
          flex: 1,
          overflowY: "auto",
          padding: "13px 13px 4px",
          display: "flex",
          flexDirection: "column",
        }}
      >
        {messages.length === 0 ? (
          <div
            style={{
              fontSize: 12,
              color: "#B3AB9C",
              margin: "auto",
              textAlign: "center",
            }}
          >
            Describe a goal above, or message the Orchestrator while it plans.
          </div>
        ) : (
          <>
            {messages.map((msg, i) => (
              <div
                key={i}
                style={{
                  display: "flex",
                  justifyContent: msg.role === "user" ? "flex-end" : "flex-start",
                }}
              >
                <div
                  style={{
                    maxWidth: msg.role === "user" ? "82%" : "88%",
                    background:
                      msg.role === "user" ? "#2A2621" : "#FCFAF6",
                    color: msg.role === "user" ? "#F2EEE6" : "#3B362F",
                    padding: "8px 12px",
                    borderRadius:
                      msg.role === "user"
                        ? "13px 13px 4px 13px"
                        : "13px 13px 13px 4px",
                    fontSize: 12.5,
                    lineHeight: 1.5,
                    marginTop: 9,
                    whiteSpace: "pre-wrap",
                    wordBreak: "break-word",
                  }}
                >
                  {msg.text}
                </div>
              </div>
            ))}
            {awaitingReply && (
              <div
                style={{
                  alignSelf: "center",
                  marginTop: 9,
                  fontSize: 11,
                  fontWeight: 600,
                  color: "#9A6A2E",
                  background: "#F6EFE3",
                  border: "1px solid #E6D3BB",
                  borderRadius: 8,
                  padding: "6px 12px",
                }}
              >
                ⏳ Awaiting your reply
              </div>
            )}
          </>
        )}
      </div>

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
