// AgentQuestionCard: shown when the selected agent session has a pendingQuestion
// (i.e. needsUser.reason === "question" or pendingQuestion is set).
// The human types a reply (max 4096 chars) and clicks Send → reply_to_agent.
// On success the box clears (the next poll will drop the card when pendingQuestion
// is gone). On error (e.g. agent no longer waiting) the error is shown inline.
//
// PRIVACY: renders only the sanitized agentId + question text (stripSpoofChars).
// No dangerouslySetInnerHTML — plain text nodes only.

import { useCallback, useEffect, useRef, useState } from "react";
import { MessageCircle } from "lucide-react";

import { invokeBackendCommand } from "../../context/AppContext";
import type { AgentSession } from "../../types/backend";
import { stripSpoofChars } from "../agents/attentionNotifier";

const MAX_REPLY_LENGTH = 4096;

export interface AgentQuestionCardProps {
  session: AgentSession;
  /** Called after a successful reply, so the parent can trigger a refresh. */
  onReplied?: () => void;
}

export function AgentQuestionCard({ session, onReplied }: AgentQuestionCardProps) {
  const [reply, setReply] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sent, setSent] = useState(false);

  const mountedRef = useRef(true);
  const busyRef = useRef(false);

  // Track mount state so async completions after unmount don't setState.
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // After a successful send the card lingers briefly until the parent poll drops it;
  // we show a "Sent." confirmation instead of re-submitting.
  const send = useCallback(async () => {
    if (busyRef.current) return;
    const text = reply.trim();
    if (!text || text.length > MAX_REPLY_LENGTH) return;
    busyRef.current = true;
    setBusy(true);
    setError(null);
    try {
      await invokeBackendCommand("reply_to_agent", {
        agentId: session.agentId,
        replyText: text,
      });
      if (!mountedRef.current) return;
      setSent(true);
      setReply("");
      onReplied?.();
    } catch (e) {
      if (mountedRef.current) {
        setError(e instanceof Error ? e.message : "Failed to send reply.");
      }
    } finally {
      busyRef.current = false;
      if (mountedRef.current) setBusy(false);
    }
  }, [reply, session.agentId, onReplied]);

  // Show only when there is an active question waiting for a reply.
  // This early return MUST come after every hook above (Rules of Hooks).
  const question = resolveQuestion(session);
  if (!question) return null;

  const agentId = stripSpoofChars(session.agentId);
  const questionText = stripSpoofChars(question);
  const charCount = reply.length;
  const overLimit = charCount > MAX_REPLY_LENGTH;

  return (
    <div className="flex flex-col gap-2 rounded-2xl border border-terracotta/30 bg-terracotta/[0.05] p-3">
      <div className="flex items-center gap-2">
        <MessageCircle className="h-4 w-4 text-terracotta" aria-hidden />
        <h3 className="text-[12px] font-semibold text-terracotta">
          {agentId} is asking you a question
        </h3>
      </div>

      <div className="rounded-lg border border-cream-200 bg-white p-2.5">
        <p className="text-[12px] leading-relaxed text-cream-800">{questionText}</p>
      </div>

      {sent ? (
        <p className="rounded-lg bg-sage/10 px-3 py-2 text-[11px] font-semibold text-sage-dark">
          Reply sent. Waiting for next update…
        </p>
      ) : (
        <>
          <div className="relative">
            <textarea
              value={reply}
              onChange={(e) => {
                setReply(e.target.value);
                setError(null);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                  e.preventDefault();
                  void send();
                }
              }}
              placeholder="Type your reply… (⌘↵ to send)"
              rows={3}
              maxLength={MAX_REPLY_LENGTH}
              disabled={busy}
              className="w-full resize-none rounded-lg border border-cream-200 bg-white px-2.5 py-2 text-[12px] text-cream-800 placeholder:text-cream-400 focus:outline-none focus:ring-1 focus:ring-teal/40 disabled:opacity-60"
            />
          </div>

          <div className="flex items-center justify-between gap-2">
            <span
              className={`text-[10px] ${overLimit ? "text-coral-dark font-semibold" : "text-cream-400"}`}
            >
              {charCount} / {MAX_REPLY_LENGTH}
            </span>
            <button
              type="button"
              onClick={() => void send()}
              disabled={busy || overLimit || reply.trim().length === 0}
              className="rounded-lg bg-teal px-3 py-1.5 text-[12px] font-semibold text-white hover:bg-teal/90 disabled:opacity-60"
            >
              {busy ? "Sending…" : "Send"}
            </button>
          </div>

          {error && (
            <p className="rounded-lg bg-coral/[0.06] px-3 py-2 text-[11px] font-semibold text-coral-dark">
              {error}
            </p>
          )}
        </>
      )}
    </div>
  );
}

/** Resolve the question text from the session.
 *  The card shows ONLY when `pendingQuestion` is present — this is the authority.
 *  The `needsUser` field drives the bell/attention indicator; the question text
 *  itself must come from the typed `pendingQuestion.question` field. */
function resolveQuestion(session: AgentSession): string | null {
  if (session.pendingQuestion?.question) {
    return session.pendingQuestion.question;
  }
  return null;
}

export default AgentQuestionCard;
