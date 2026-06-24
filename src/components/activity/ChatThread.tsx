import { useRef, useEffect } from "react";
import type { PlannerMessage } from "../projects/planner/plannerModel";

export interface ChatThreadProps {
  messages: PlannerMessage[];
  live: boolean;
  awaitingReply: boolean;
  emptyHint?: string;
}

export function ChatThread({ messages, live, awaitingReply, emptyHint }: ChatThreadProps) {
  const scrollRef = useRef<HTMLDivElement>(null);

  const lastMsgLen = messages[messages.length - 1]?.text.length ?? 0;

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    if (nearBottom) {
      el.scrollTop = el.scrollHeight;
    }
  }, [messages, lastMsgLen, awaitingReply]);

  return (
    <div
      ref={scrollRef}
      className="pp-scroll"
      style={{
        flex: 1,
        minHeight: 0,
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
          {emptyHint ?? "Describe a goal above, or message the Orchestrator while it plans."}
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
                  background: msg.role === "user" ? "#2A2621" : "#FCFAF6",
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
                {msg.streaming && (
                  <span
                    aria-hidden
                    style={{
                      display: "inline-block",
                      width: 7,
                      marginLeft: 1,
                      animation: "pp-blink 1s step-start infinite",
                    }}
                  >
                    ▌
                  </span>
                )}
              </div>
            </div>
          ))}
          {live &&
            !awaitingReply &&
            messages[messages.length - 1]?.role === "user" && (
              <div
                style={{
                  alignSelf: "flex-start",
                  marginTop: 9,
                  fontSize: 11,
                  fontWeight: 600,
                  color: "#7c766b",
                  display: "flex",
                  alignItems: "center",
                  gap: 7,
                }}
              >
                <span
                  style={{
                    width: 6,
                    height: 6,
                    borderRadius: "50%",
                    background: "#C0894F",
                    animation: "pp-pulse 1.2s ease-in-out infinite",
                  }}
                />
                Planner is thinking…
              </div>
            )}
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
  );
}

