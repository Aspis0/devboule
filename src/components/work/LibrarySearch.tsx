import { useEffect, useState, useCallback, useMemo } from "react";
import { invokeBackendCommand } from "../../context/AppContext";
import { commandScore } from "../../vendor/commandScore";

interface GlobalSkill {
  name: string;
  content: string;
  bytes: number;
  truncated: boolean;
}

export function LibrarySearch({
  projectRoot,
  profile,
  onApplied,
}: {
  projectRoot: string;
  profile: string;
  onApplied?: () => void;
}) {
  const [library, setLibrary] = useState<GlobalSkill[]>([]);
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [applying, setApplying] = useState<string | null>(null);

  // Fetch effect
  useEffect(() => {
    let cancelled = false;
    const fetchLibrary = async () => {
      try {
        const result = await invokeBackendCommand("global_skills_list", {});
        if (!cancelled) {
          // Guard against a non-array result (null/() IPC response) — library.length
          // is read during render and would throw on null.
          setLibrary(Array.isArray(result) ? (result as GlobalSkill[]) : []);
        }
      } catch (err: unknown) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
        }
      }
    };
    fetchLibrary();
    return () => {
      cancelled = true;
    };
  }, [projectRoot]);

  // Derived filtered/sorted list
  const filteredLibrary = useMemo(() => {
    if (query.trim() === "") {
      return [...library].sort((a, b) => a.name.localeCompare(b.name));
    }
    return library
      .map((s) => ({
        skill: s,
        score: commandScore(s.name, query, [s.content.slice(0, 200)]),
      }))
      .filter((item) => item.score > 0)
      .sort((a, b) => b.score - a.score)
      .map((item) => item.skill);
  }, [library, query]);

  // Apply function
  const apply = useCallback(
    async (s: GlobalSkill) => {
      // Block applying a truncated library skill (it would write only the head and
      // silently lose the tail).
      if (s.truncated) {
        setError(
          `"${s.name}" is truncated — expand it in the global library before applying.`,
        );
        return;
      }
      // Apply OVERWRITES the profile's current SKILL.md — confirm to avoid data loss.
      if (
        !window.confirm(
          `Apply "${s.name}" to the ${profile} profile? This overwrites its current skill manual.`,
        )
      ) {
        return;
      }
      setApplying(s.name);
      setError(null);
      try {
        await invokeBackendCommand("skills_save_profile", {
          workingFolderPath: projectRoot,
          profile,
          content: s.content,
        });
        onApplied?.();
      } catch (err: unknown) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setApplying(null);
      }
    },
    [projectRoot, profile, onApplied],
  );

  // Render logic
  if (library.length === 0 && !error) {
    return (
      <div className="flex flex-col gap-2">
        <input
          data-testid="library-search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search the library — skills & tools…"
          className="w-full rounded-2xl border border-cream-200 bg-white px-3 py-1.5 text-[12px] text-cream-800 focus:border-teal/40 focus:outline-none"
        />
        <div data-testid="library-empty">Your global library is empty.</div>
        {error && (
          <div
            data-testid="library-error"
            className="text-[11px] text-coral-dark"
          >
            {error}
          </div>
        )}
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2">
      <input
        data-testid="library-search"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        placeholder="Search the library — skills & tools…"
        className="w-full rounded-2xl border border-cream-200 bg-white px-3 py-1.5 text-[12px] text-cream-800 focus:border-teal/40 focus:outline-none"
      />
      {error && (
        <div
          data-testid="library-error"
          className="text-[11px] text-coral-dark"
        >
          {error}
        </div>
      )}
      {filteredLibrary.length === 0 && query.trim() !== "" && (
        <div data-testid="library-no-matches">No matches.</div>
      )}
      {filteredLibrary.map((s) => (
        <div
          key={s.name}
          data-testid={`library-row-${s.name}`}
          className="flex items-center justify-between rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px]"
        >
          <div className="flex flex-col">
            <span className="font-medium">{s.name}</span>
            <span className="text-[10px] text-cream-500">
              {s.bytes} bytes{s.truncated ? " (truncated)" : ""}
            </span>
          </div>
          <button
            type="button"
            data-testid={`library-apply-${s.name}`}
            disabled={applying === s.name}
            onClick={() => apply(s)}
            className="rounded-lg border border-teal/30 bg-teal/10 px-2 py-1 text-[11px] font-semibold text-teal disabled:opacity-50"
          >
            {applying === s.name ? "Applying…" : "Apply"}
          </button>
        </div>
      ))}
    </div>
  );
}
