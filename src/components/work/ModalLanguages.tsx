import { useEffect, useState, useCallback, useRef } from "react";
import { invokeBackendCommand } from "../../context/AppContext";

interface LangEntry {
  role: string;
  lang: string;
  source: "project" | "bundled";
  content: string;
  bytes: number;
  truncated: boolean;
}

export function ModalLanguages({ projectRoot, profile }: { projectRoot: string; profile: string }) {
  const [langs, setLangs] = useState<LangEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [drafts, setDrafts] = useState<Record<string, string>>({});

  const busyRef = useRef(false);
  const mountedRef = useRef(true);
  const genRef = useRef(0);

  const refresh = useCallback(async () => {
    const gen = ++genRef.current;
    try {
      const r = await invokeBackendCommand("skills_list_langs_profile", {
        workingFolderPath: projectRoot,
        profile,
      });
      if (!mountedRef.current || gen !== genRef.current) return;
      setLangs(Array.isArray(r) ? (r as LangEntry[]) : []);
      setError(null);
    } catch (e: unknown) {
      if (!mountedRef.current || gen !== genRef.current) return;
      setError(e instanceof Error ? e.message : "Failed to load languages");
    }
  }, [projectRoot, profile]);

  useEffect(() => {
    mountedRef.current = true;
    void refresh();
    return () => {
      mountedRef.current = false;
    };
  }, [refresh]);

  const openEdit = (l: LangEntry) => {
    if (editing === l.lang) {
      setEditing(null);
    } else {
      setEditing(l.lang);
      setDrafts((prev) =>
        prev[l.lang] === undefined ? { ...prev, [l.lang]: l.content } : prev,
      );
    }
  };

  const handleSave = async (lang: string) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(lang);
    try {
      await invokeBackendCommand("skills_save_lang_profile", {
        workingFolderPath: projectRoot,
        profile,
        lang,
        content: drafts[lang] ?? "",
      });
      if (mountedRef.current) {
        setEditing(null);
        setDrafts((prev) => {
          const c = { ...prev };
          delete c[lang];
          return c;
        });
        await refresh();
      }
    } catch (e: unknown) {
      if (mountedRef.current) {
        setError(e instanceof Error ? e.message : "Failed to save");
      }
    } finally {
      busyRef.current = false;
      if (mountedRef.current) setBusy(null);
    }
  };

  const handleReset = async (lang: string) => {
    if (busyRef.current) return;
    const confirmed = window.confirm(`Reset the ${lang} persona to the bundled default?`);
    if (!confirmed) return;
    busyRef.current = true;
    setBusy(lang);
    try {
      await invokeBackendCommand("skills_reset_lang_profile", {
        workingFolderPath: projectRoot,
        profile,
        lang,
      });
      if (mountedRef.current) {
        await refresh();
      }
    } catch (e: unknown) {
      if (mountedRef.current) {
        setError(e instanceof Error ? e.message : "Failed to reset");
      }
    } finally {
      busyRef.current = false;
      if (mountedRef.current) setBusy(null);
    }
  };

  return (
    <div data-testid="modal-languages" className="flex flex-col gap-2">
      {error && (
        <div data-testid="modal-languages-error" className="text-[11px] text-coral-dark">
          {error}
        </div>
      )}
      {langs.map((l) => (
        <div
          key={l.lang}
          data-testid={`ml-row-${l.lang}`}
          className="rounded-lg border border-cream-200 bg-white p-2 text-[12px]"
        >
          <div className="flex items-center justify-between">
            <span className="font-medium">{l.lang}</span>
            <span
              data-testid={`ml-badge-${l.lang}`}
              className="rounded bg-cream-100 px-1 text-[10px] text-cream-600"
            >
              {l.source}
            </span>
          </div>
          <div className="mt-1 flex gap-2">
            <button
              type="button"
              data-testid={`ml-edit-${l.lang}`}
              onClick={() => openEdit(l)}
              disabled={busy !== null}
              className="rounded-lg border border-teal/30 bg-teal/10 px-2 py-1 text-[11px] font-semibold text-teal disabled:opacity-50"
            >
              Edit
            </button>
            {l.source === "project" && (
              <button
                type="button"
                data-testid={`ml-reset-${l.lang}`}
                onClick={() => void handleReset(l.lang)}
                disabled={busy !== null}
                className="rounded-lg border border-cream-200 bg-cream-50 px-2 py-1 text-[11px] font-semibold text-cream-700 hover:bg-cream-100 disabled:opacity-50"
              >
                Reset
              </button>
            )}
          </div>
          {editing === l.lang && (
            <div className="mt-2">
              <textarea
                data-testid={`ml-textarea-${l.lang}`}
                value={drafts[l.lang] ?? l.content}
                onChange={(e) =>
                  setDrafts((prev) => ({ ...prev, [l.lang]: e.target.value }))
                }
                disabled={busy !== null}
                className="min-h-[80px] w-full rounded-xl border border-cream-100 bg-cream-50 p-2 text-[11px] disabled:opacity-50"
              />
              <button
                type="button"
                data-testid={`ml-save-${l.lang}`}
                onClick={() => void handleSave(l.lang)}
                disabled={busy !== null}
                className="mt-2 rounded-lg border border-teal/30 bg-teal/10 px-2 py-1 text-[11px] font-semibold text-teal disabled:opacity-50"
              >
                Save
              </button>
            </div>
          )}
        </div>
      ))}
    </div>
  );
}
