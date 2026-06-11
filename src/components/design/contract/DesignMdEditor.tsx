// DesignMdEditor — the design.md contract editor modal (Phase C).
//
// Opened either from the ProjectPopover "Design contract…" row (loads current
// design.md or a fresh draft) OR automatically after create/open when design.md is
// MISSING (the seed flow). It shows the draft/current contract in a big textarea, a
// 3-card preset picker, a byte/char counter vs the 64 KiB on-disk cap, and Save / Skip.
//
// HARD INVARIANT (trust model): NOTHING here writes to disk except the Save button.
//   - Save -> design_write_design_md(content); AND when the content came from a chosen
//     PRESET or from token EXTRACTION, also design_write_tokens(tokensJson) so the
//     swatches + tokenNamesForPrompt pick up the real values.
//   - Skip -> closes, writes NOTHING (the seed flow handles its own legacy fallback).
// The draft may quote target source; it is only ever written on this explicit Save, so
// post-save it is user-curated content (same trust class as the user's own instruction).

import { useEffect, useMemo, useRef, useState } from "react";
import { X, Save, FileText } from "lucide-react";
import { PRESET_CATALOG, type DesignTokensDoc } from "./presets";

/** The on-disk byte cap enforced by the Rust `design_write_design_md` command. */
const MAX_DESIGN_MD_BYTES = 64 * 1024;

export interface DesignMdEditorProps {
  open: boolean;
  /** Initial textarea content: the current design.md, or a freshly-built draft. */
  initialContent: string;
  /**
   * Tokens to write alongside design.md on Save when the content is the EXTRACTED
   * draft (not a preset). Undefined when there is nothing extracted to persist (e.g.
   * editing an existing contract). A chosen preset's tokens override this.
   */
  draftTokens?: DesignTokensDoc;
  /**
   * Review notice (Fix 3): shown as a banner when the editor opened because the on-disk
   * design.md changed outside the editor and must be reviewed/re-approved before use.
   */
  notice?: string;
  /**
   * Inline save error (Fix 5): when a Save WRITE failed the parent keeps the editor open
   * and passes the error here; the user's content is intact and Save can be retried.
   */
  saveError?: string;
  /** Persist the contract (+ tokens). Returns when the writes settle. */
  onSave: (content: string, tokens: DesignTokensDoc | undefined) => Promise<void> | void;
  /** Close WITHOUT writing anything. */
  onSkip: () => void;
}

function byteLen(s: string): number {
  return new TextEncoder().encode(s).length;
}

export function DesignMdEditor({
  open,
  initialContent,
  draftTokens,
  notice,
  saveError,
  onSave,
  onSkip,
}: DesignMdEditorProps) {
  const [content, setContent] = useState(initialContent);
  // Tokens to write on Save: the extracted draft tokens by default; a chosen preset
  // REPLACES both the text and this token set (so swatches match the preset).
  const [pendingTokens, setPendingTokens] = useState<DesignTokensDoc | undefined>(
    draftTokens,
  );
  const [saving, setSaving] = useState(false);
  // Re-seed when the modal (re)opens with new content — but not on every render.
  const lastInitial = useRef<string | null>(null);
  // Fix 7: capture the LATEST draftTokens in a ref so the reseed effect can read it
  // WITHOUT listing draftTokens as a dependency. A parent re-render that hands us a new
  // draftTokens REFERENCE (same modal, same initialContent — e.g. after the user already
  // picked a preset inside the editor) must NOT re-run the seed and clobber pendingTokens.
  // The reseed is gated SOLELY on (re)open with new initialContent.
  const draftTokensRef = useRef(draftTokens);
  draftTokensRef.current = draftTokens;

  useEffect(() => {
    if (!open) {
      lastInitial.current = null;
      return;
    }
    if (lastInitial.current !== initialContent) {
      lastInitial.current = initialContent;
      setContent(initialContent);
      setPendingTokens(draftTokensRef.current);
      setSaving(false);
    }
  }, [open, initialContent]);

  const bytes = useMemo(() => byteLen(content), [content]);
  const overCap = bytes > MAX_DESIGN_MD_BYTES;

  if (!open) return null;

  const pickPreset = (id: string) => {
    const preset = PRESET_CATALOG.find((p) => p.id === id);
    if (!preset) return;
    // Picking a preset REPLACES the textarea content and remembers its tokens for Save.
    setContent(preset.designMd);
    setPendingTokens(preset.tokens);
  };

  const doSave = async () => {
    if (overCap || saving) return;
    setSaving(true);
    try {
      await onSave(content, pendingTokens);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div
      className="modal-scrim"
      role="dialog"
      aria-modal="true"
      aria-label="Design contract editor"
      data-testid="design-md-editor"
    >
      <div className="handoff dsgn-contract">
        <div className="ho-head">
          <div className="ho-ic">
            <FileText size={20} />
          </div>
          <div className="ho-head-t">
            <b>Design contract</b>
            <span>design.md · grounds every generation in your rules</span>
          </div>
          <button
            type="button"
            className="ho-close"
            onClick={onSkip}
            aria-label="Skip — close without saving"
          >
            <X size={16} />
          </button>
        </div>

        {notice ? (
          <div className="dc-notice" role="status" data-testid="dc-notice">
            {notice}
          </div>
        ) : null}

        <div className="dc-presets" data-testid="dc-presets">
          {PRESET_CATALOG.map((p) => (
            <button
              key={p.id}
              type="button"
              className="dc-preset"
              data-preset={p.id}
              onClick={() => pickPreset(p.id)}
              title={p.description}
            >
              <b>{p.name}</b>
              <span>{p.description}</span>
            </button>
          ))}
        </div>

        <div className="dc-body">
          <textarea
            className="dc-textarea"
            aria-label="Design contract markdown"
            value={content}
            onChange={(e) => setContent(e.target.value)}
            spellCheck={false}
          />
        </div>

        <div className="ho-foot">
          {saveError ? (
            <span className="dc-save-error" role="alert" data-testid="dc-save-error">
              Couldn’t save: {saveError}
            </span>
          ) : null}
          <span
            className="ho-foot-note"
            data-over={overCap || undefined}
            data-testid="dc-counter"
          >
            {bytes.toLocaleString()} / {MAX_DESIGN_MD_BYTES.toLocaleString()} bytes
            {overCap ? " — too large to save" : ""}
          </span>
          <button type="button" className="btn btn-ghost" onClick={onSkip}>
            Skip
          </button>
          <button
            type="button"
            className="btn btn-primary"
            onClick={() => void doSave()}
            disabled={overCap || saving}
          >
            <Save size={15} />
            {saving ? "Saving…" : "Save contract"}
          </button>
        </div>
      </div>
    </div>
  );
}

export default DesignMdEditor;
