import { useEffect, useState, useRef } from "react";
import { invokeBackendCommand } from "../../context/AppContext";

const ENCODER = new TextEncoder();

export function SkillEditor({
  projectRoot,
  profile,
  content,
  truncated,
  onSaved,
}: {
  projectRoot: string;
  profile: string;
  content: string;
  truncated: boolean;
  onSaved: () => void;
}) {
  const [draft, setDraft] = useState(content);
  const [ackTrunc, setAckTrunc] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const savingRef = useRef(false);
  const mountedRef = useRef(true);
  const prevContentRef = useRef(content);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Reflect EXTERNAL content changes (a save's refetch, or a library "apply") only when the user
  // hasn't diverged from the last server value — so unsaved edits are preserved but applied/saved
  // content is shown. The parent keys this by profile, so this never fires across a profile switch.
  useEffect(() => {
    if (draft === prevContentRef.current) {
      setDraft(content);
    }
    prevContentRef.current = content;
  }, [content, draft]);

  const bytes = ENCODER.encode(draft).length;
  const overCap = bytes > 8192;

  const handleSave = async () => {
    if (savingRef.current) return;
    if (overCap) return;
    if (truncated && !ackTrunc) return;
    savingRef.current = true;
    setSaving(true);
    setError(null);
    try {
      await invokeBackendCommand("skills_save_profile", {
        workingFolderPath: projectRoot,
        profile,
        content: draft,
      });
      if (mountedRef.current) setAckTrunc(false);
      onSaved();
    } catch (e) {
      console.error("skills_save_profile failed", e);
      if (mountedRef.current) setError(e instanceof Error ? e.message : "Failed to save");
    } finally {
      savingRef.current = false;
      if (mountedRef.current) setSaving(false);
    }
  };

  return (
    <div className="flex flex-col gap-2">
      <textarea
        data-testid="skills-tools-skill-editor"
        className="h-48 w-full whitespace-pre-wrap rounded-xl border border-cream-100 bg-cream-50 p-3 text-[12px] text-cream-800 focus:border-teal/40 focus:outline-none"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
      />
      {error && (
        <div data-testid="skills-tools-skill-error" className="text-[11px] text-coral-dark">
          {error}
        </div>
      )}
      <div className="flex items-center justify-between gap-2">
        <span
          data-testid="skills-tools-skill-bytes"
          className={`text-[11px] ${overCap ? "text-coral-dark" : "text-cream-600"}`}
        >
          {bytes} / 8192 bytes
        </span>
        {truncated && (
          <div className="flex items-center gap-2 text-[11px] text-coral-dark">
            <input
              type="checkbox"
              id="skills-tools-skill-ack"
              data-testid="skills-tools-skill-ack"
              checked={ackTrunc}
              onChange={(e) => setAckTrunc(e.target.checked)}
              className="rounded border-cream-300 text-teal focus:ring-teal/50"
            />
            <label htmlFor="skills-tools-skill-ack">Acknowledge truncation</label>
          </div>
        )}
        <button
          type="button"
          data-testid="skills-tools-skill-save"
          onClick={() => void handleSave()}
          disabled={saving || draft === content || (truncated && !ackTrunc) || overCap}
          className="rounded-lg border border-teal/30 bg-teal/10 px-3 py-1 text-[11px] font-semibold text-teal disabled:opacity-50"
        >
          {saving ? "Saving…" : "Save"}
        </button>
      </div>
    </div>
  );
}
