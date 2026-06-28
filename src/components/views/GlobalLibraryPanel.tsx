import { useEffect, useState, useCallback, useMemo, useRef } from "react";
import { invokeBackendCommand } from "../../context/AppContext";
import { commandScore } from "../../vendor/commandScore";

interface GlobalSkill { name: string; content: string; bytes: number; truncated: boolean; }
interface BundledEntry { name: string; description: string; }

export function GlobalLibraryPanel() {
  const [library, setLibrary] = useState<GlobalSkill[]>([]);
  const [bundled, setBundled] = useState<BundledEntry[]>([]);
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [newName, setNewName] = useState("");
  const [newContent, setNewContent] = useState("");
  const [editing, setEditing] = useState<string | null>(null);
  const [editDrafts, setEditDrafts] = useState<Record<string, string>>({});
  const [ackTruncated, setAckTruncated] = useState<Record<string, boolean>>({});

  const busyRef = useRef(false);
  const mountedRef = useRef(true);
  const genRef = useRef(0);

  const refresh = useCallback(async () => {
    const gen = ++genRef.current;
    try {
      const [a, b] = await Promise.all([
        invokeBackendCommand("global_skills_list", {}),
        invokeBackendCommand("skills_library_catalog", {})
      ]);
      if (!mountedRef.current || gen !== genRef.current) return;
      setLibrary(Array.isArray(a) ? (a as GlobalSkill[]) : []);
      setBundled(Array.isArray(b) ? (b as BundledEntry[]) : []);
      setError(null);
    } catch (e: unknown) {
      if (!mountedRef.current || gen !== genRef.current) return;
      setError(e instanceof Error ? e.message : "Failed to refresh library");
    }
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    refresh();
    return () => {
      mountedRef.current = false;
    };
  }, [refresh]);

  const filteredLibrary = useMemo(() => {
    if (query.trim() === "") {
      return [...library].sort((a, b) => a.name.localeCompare(b.name));
    }
    return library
      .map(s => ({ s, score: commandScore(s.name, query, [s.content.slice(0, 200)]) }))
      .filter(x => x.score > 0)
      .sort((a, b) => b.score - a.score)
      .map(x => x.s);
  }, [library, query]);

  const bundledNotInstalled = useMemo(() => {
    let result = bundled.filter(b => !library.some(l => l.name.toLowerCase() === b.name.toLowerCase()));
    if (query.trim() !== "") {
      result = result
        .map(b => ({ b, score: commandScore(b.name, query, [b.description]) }))
        .filter(x => x.score > 0)
        .sort((a, b) => b.score - a.score)
        .map(x => x.b);
    }
    return result;
  }, [bundled, library, query]);

  const handleDelete = async (name: string) => {
    if (busyRef.current) return;
    if (!window.confirm(`Delete "${name}" from your global library?`)) return;
    busyRef.current = true;
    setBusy(name);
    try {
      await invokeBackendCommand("global_skills_delete", { name });
      await refresh();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : "Failed to delete skill");
    } finally {
      busyRef.current = false;
      setBusy(null);
    }
  };

  const handleAddBundled = async (name: string) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(name);
    try {
      await invokeBackendCommand("global_skills_install_bundled", { skillName: name });
      await refresh();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : "Failed to add bundled skill");
    } finally {
      busyRef.current = false;
      setBusy(null);
    }
  };

  const handleCreate = async () => {
    const n = newName.trim();
    if (busyRef.current) return;
    if (!n || !newContent) return;
    busyRef.current = true;
    setBusy(n);
    try {
      await invokeBackendCommand("global_skills_save", { name: n, content: newContent });
      setNewName("");
      setNewContent("");
      await refresh();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : "Failed to create skill");
    } finally {
      busyRef.current = false;
      setBusy(null);
    }
  };

  const handleEditSave = async (name: string) => {
    const content = editDrafts[name] ?? "";
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(name);
    try {
      await invokeBackendCommand("global_skills_save", { name, content });
      setEditing(null);
      setEditDrafts(prev => {
        const c = { ...prev };
        delete c[name];
        return c;
      });
      setAckTruncated(prev => {
        const c = { ...prev };
        delete c[name];
        return c;
      });
      await refresh();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : "Failed to save skill");
    } finally {
      busyRef.current = false;
      setBusy(null);
    }
  };

  const openEdit = (s: GlobalSkill) => {
    if (editing === s.name) {
      setEditing(null);
    } else {
      setEditing(s.name);
      setEditDrafts(prev => prev[s.name] === undefined ? { ...prev, [s.name]: s.content } : prev);
    }
  };

  return (
    <div data-testid="global-library-panel" className="flex flex-col gap-3">
      <input
        data-testid="global-library-search"
        className="w-full rounded-2xl border border-cream-200 bg-white px-3 py-1.5 text-[12px] text-cream-800 focus:border-teal/40 focus:outline-none"
        placeholder="Search skills..."
        value={query}
        onChange={e => setQuery(e.target.value)}
      />
      {error && <div data-testid="global-library-error" className="text-[11px] text-coral-dark">{error}</div>}

      <div className="flex flex-col gap-2">
        <input
          data-testid="global-library-new-name"
          className="w-full rounded-2xl border border-cream-200 bg-white px-3 py-1.5 text-[12px] text-cream-800 focus:border-teal/40 focus:outline-none"
          placeholder="skill-name"
          value={newName}
          onChange={e => setNewName(e.target.value)}
          disabled={busy !== null}
        />
        <textarea
          data-testid="global-library-new-content"
          className="w-full rounded-2xl border border-cream-200 bg-white px-3 py-1.5 text-[12px] text-cream-800 focus:border-teal/40 focus:outline-none min-h-[60px]"
          placeholder="SKILL.md content…"
          value={newContent}
          onChange={e => setNewContent(e.target.value)}
          disabled={busy !== null}
        />
        <button
          type="button"
          data-testid="global-library-new-save"
          onClick={() => void handleCreate()}
          disabled={busy !== null || !newName.trim() || !newContent}
          className="self-end rounded-lg border border-teal/30 bg-teal/10 px-3 py-1 text-[12px] font-semibold text-teal disabled:opacity-50"
        >
          Create skill
        </button>
      </div>

      <h3 className="text-sm font-semibold text-cream-900">Your library</h3>
      <div className="flex flex-col gap-2">
        {filteredLibrary.map(s => (
          <div key={s.name} data-testid={`global-skill-row-${s.name}`} className="rounded-xl border border-cream-200 bg-white p-2">
            <div className="flex items-center justify-between gap-2">
              <div className="flex-1">
                <div className="text-[12px] font-medium text-cream-900">
                  {s.name} <span className="text-[10px] text-cream-500">{s.bytes} bytes</span>
                  {s.truncated && <span className="text-[10px] text-coral-dark"> (truncated)</span>}
                </div>
              </div>
              <div className="flex gap-1">
                <button
                  type="button"
                  data-testid={`global-skill-edit-${s.name}`}
                  onClick={() => openEdit(s)}
                  disabled={busy === s.name}
                  className="rounded-lg border border-cream-200 bg-cream-50 px-2 py-1 text-[11px] font-semibold text-cream-700 hover:bg-cream-100 disabled:opacity-50"
                >
                  Edit
                </button>
                <button
                  type="button"
                  data-testid={`global-skill-delete-${s.name}`}
                  onClick={() => void handleDelete(s.name)}
                  disabled={busy === s.name}
                  className="rounded-lg border border-coral/30 bg-coral/10 px-2 py-1 text-[11px] font-semibold text-coral-dark disabled:opacity-50"
                >
                  Delete
                </button>
              </div>
            </div>
            {editing === s.name && (
              <div className="mt-2 flex flex-col gap-2">
                {s.truncated && (
                  <div className="flex items-center gap-2 rounded-lg border border-coral/30 bg-coral/10 p-2 text-[11px] text-coral-dark">
                    <input
                      type="checkbox"
                      data-testid={`global-skill-ack-${s.name}`}
                      checked={!!ackTruncated[s.name]}
                      onChange={e => setAckTruncated(prev => ({ ...prev, [s.name]: e.target.checked }))}
                      className="mr-1"
                    />
                    <label>This skill was truncated on read — saving will discard everything past the cap. I understand, save anyway.</label>
                  </div>
                )}
                <textarea
                  data-testid={`global-skill-textarea-${s.name}`}
                  value={editDrafts[s.name] ?? s.content}
                  onChange={e => setEditDrafts(prev => ({ ...prev, [s.name]: e.target.value }))}
                  className="w-full rounded-xl border border-cream-200 bg-cream-50 px-2 py-1 text-[11px] text-cream-800 focus:border-teal/40 focus:outline-none min-h-[80px]"
                />
                <button
                  type="button"
                  data-testid={`global-skill-save-${s.name}`}
                  onClick={() => void handleEditSave(s.name)}
                  disabled={busy === s.name || (s.truncated && !ackTruncated[s.name])}
                  className="self-end rounded-lg border border-teal/30 bg-teal/10 px-3 py-1 text-[11px] font-semibold text-teal disabled:opacity-50"
                >
                  Save
                </button>
              </div>
            )}
          </div>
        ))}
      </div>

      <h3 className="text-sm font-semibold text-cream-900">From the bundled catalog</h3>
      <div className="flex flex-col gap-2">
        {bundledNotInstalled.map(b => (
          <div key={b.name} data-testid={`bundled-skill-row-${b.name}`} className="flex items-center justify-between gap-2 rounded-xl border border-cream-200 bg-white p-2">
            <div className="flex-1">
              <div className="text-[12px] font-medium text-cream-900">{b.name}</div>
              <div className="text-[11px] text-cream-500">{b.description}</div>
            </div>
            <button
              type="button"
              data-testid={`bundled-skill-add-${b.name}`}
              onClick={() => void handleAddBundled(b.name)}
              disabled={busy === b.name}
              className="rounded-lg border border-teal/30 bg-teal/10 px-2 py-1 text-[11px] font-semibold text-teal disabled:opacity-50"
            >
              Add to my library
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}