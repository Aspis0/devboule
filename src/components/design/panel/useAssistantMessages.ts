// useAssistantMessages — the assistant transcript state for the design panel.
//
// A thin, capped list manager driven by DesignView's EXISTING flow control points.
// It owns ONLY presentation state: pushing a user row + a "working" assistant card at
// the start of a run, patching that card to done/error as the pipeline resolves, and
// updating its desc while self-repair runs. The pipeline itself is unchanged — this
// hook never calls the backend.
//
// Concurrency note: a generation run pushes exactly one assistant card and remembers
// its id (the caller stores it in a ref). The done-effect patches THAT id. Because the
// existing pipeline already serializes runs (the `preparing` guard + a single
// `pendingRunRef`), there is at most one live assistant card at a time, so patch-by-id
// is unambiguous.

import { useCallback, useRef, useState } from "react";
import { type AssistantMessage, MAX_MESSAGES } from "./types";

export interface AssistantMessagesApi {
  messages: AssistantMessage[];
  /** Append a row, dropping the oldest when over the cap. Returns its assigned id. */
  push: (msg: Omit<AssistantMessage, "id">) => number;
  /** Merge a patch into the message with `id` (no-op if it was already dropped). */
  patch: (id: number, patch: Partial<AssistantMessage>) => void;
  /** Count of assistant cards that reached `done` this session (the header `.sub`). */
  doneCount: number;
  /** Clear the transcript (e.g. on project load). */
  reset: () => void;
}

export function useAssistantMessages(): AssistantMessagesApi {
  const [messages, setMessages] = useState<AssistantMessage[]>([]);
  const idRef = useRef(0);

  const push = useCallback((msg: Omit<AssistantMessage, "id">): number => {
    const id = ++idRef.current;
    setMessages((prev) => {
      const next = [...prev, { ...msg, id }];
      // Bound memory: drop oldest rows beyond the cap.
      return next.length > MAX_MESSAGES
        ? next.slice(next.length - MAX_MESSAGES)
        : next;
    });
    return id;
  }, []);

  const patch = useCallback((id: number, patch: Partial<AssistantMessage>) => {
    setMessages((prev) =>
      prev.map((m) => (m.id === id ? { ...m, ...patch } : m)),
    );
  }, []);

  const reset = useCallback(() => setMessages([]), []);

  const doneCount = messages.reduce(
    (n, m) => (m.role === "assistant" && m.status === "done" ? n + 1 : n),
    0,
  );

  return { messages, push, patch, doneCount, reset };
}
