// Composer — the assistant panel's input box (panel.jsx Composer).
//
// One textarea drives BOTH real flows: with a node selected it routes to the EXISTING
// per-node edit round-trip; with no selection it routes to the EXISTING generate flow.
// The parent (DesignView) owns that branching — the composer just calls `onSend(text)`
// and clears the draft. Enter sends, Shift+Enter inserts a newline.
//
// Deferred (visual kept, disabled): the paperclip attachment button. The prototype
// supported image attachments; the real pipeline does not yet, so the button is shown
// DISABLED with a "coming soon" title rather than dropped (so the reskin stays faithful
// and the feature is obvious-next).

import { useCallback, useEffect, useRef, useState } from "react";
import { ChevronDown, Loader2, Paperclip, Send, Wand2, X } from "lucide-react";
import { ModelPopover } from "./ModelPopover";
import { providerMeta, effortLabel } from "./types";
import type { DesignLlmBackend } from "../../../types/config";

export interface ComposerProps {
  /** Display name of the node selected for edit, or null (= generate mode). */
  selectedNodeName: string | null;
  /** Clear the edit-target selection (the ctx-chip X). */
  onClearContext: () => void;
  /** Send the trimmed draft (edit when a node is selected, generate otherwise). */
  onSend: (text: string) => void;
  /** True while a run is in flight (disables input + spins the send icon). */
  busy: boolean;
  /** Backend + popover wiring. */
  backend: DesignLlmBackend | null;
  onSaveBackend: (next: DesignLlmBackend) => void;
  onOpenSettings: () => void;
  /** Controlled draft (so suggestions can seed it from the parent). */
  draft: string;
  setDraft: (value: string) => void;
  /** Imperative focus handle bumped by the parent (empty-canvas "Generate a section"). */
  focusSignal: number;
}

export function Composer({
  selectedNodeName,
  onClearContext,
  onSend,
  busy,
  backend,
  onSaveBackend,
  onOpenSettings,
  draft,
  setDraft,
  focusSignal,
}: ComposerProps) {
  const [modelOpen, setModelOpen] = useState(false);
  const taRef = useRef<HTMLTextAreaElement>(null);

  // Focus the textarea when the parent bumps the signal (empty-canvas CTA). Skip the
  // initial mount (signal 0) so we don't steal focus on first render.
  const firstFocus = useRef(true);
  useEffect(() => {
    if (firstFocus.current) {
      firstFocus.current = false;
      return;
    }
    taRef.current?.focus();
  }, [focusSignal]);

  const meta = providerMeta(backend?.kind);

  const send = useCallback(() => {
    const text = draft.trim();
    if (!text || busy) return;
    onSend(text);
    setDraft("");
  }, [draft, busy, onSend, setDraft]);

  const sendDisabled = busy || draft.trim().length === 0;

  return (
    <div className="composer" data-screen-label="Composer">
      <div className="composer-box">
        {selectedNodeName ? (
          <div className="composer-ctx">
            <span className="ctx-chip">
              <Wand2 size={11} />
              Editing {selectedNodeName}
              <button
                type="button"
                className="x"
                onClick={onClearContext}
                title="Clear — generate new instead"
              >
                <X size={11} />
              </button>
            </span>
          </div>
        ) : null}

        <textarea
          ref={taRef}
          rows={2}
          placeholder={
            selectedNodeName
              ? `Describe the change to ${selectedNodeName}…`
              : "Describe what to generate…"
          }
          value={draft}
          disabled={busy}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              send();
            }
          }}
        />

        <div className="composer-bar">
          {/* Attachments are deferred — the visual is kept but disabled. */}
          <button
            type="button"
            className="icon-btn"
            disabled
            title="Attachments — coming soon"
          >
            <Paperclip size={16} />
          </button>

          <div className="pop-wrap">
            <button
              type="button"
              className="model-chip"
              data-open={modelOpen}
              onClick={() => setModelOpen((v) => !v)}
            >
              <span className="dot" />
              {meta.name} · {effortLabel(backend?.effort)}
              <ChevronDown size={12} style={{ color: "var(--muted)" }} />
            </button>
            <ModelPopover
              open={modelOpen}
              onClose={() => setModelOpen(false)}
              backend={backend}
              onSave={onSaveBackend}
              onOpenSettings={onOpenSettings}
            />
          </div>

          <button
            type="button"
            className="send-btn"
            disabled={sendDisabled}
            onClick={send}
            title="Generate (Enter)"
          >
            {busy ? (
              <Loader2 size={16} style={{ animation: "dsgnRot 1s linear infinite" }} />
            ) : (
              <Send size={16} />
            )}
          </button>
        </div>
      </div>
      <div className="composer-hint">
        <b>Enter</b> to send · <b>Shift+Enter</b> for a new line
      </div>
    </div>
  );
}

export default Composer;
