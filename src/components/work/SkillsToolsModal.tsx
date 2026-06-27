import {
  useEffect,
  useRef,
  useState,
  useCallback,
  type MouseEvent,
} from "react";
import { invokeBackendCommand } from "../../context/AppContext";
import { Sparkles, X } from "lucide-react";

type SkillRole = "mini" | "coder" | "design" | "orchestrator";

interface SkillEntry {
  role: SkillRole;
  exists: boolean;
  enabled: boolean;
  content: string;
  bytes: number;
  truncated: boolean;
}

type Props = {
  projectRoot: string;
  onClose: () => void;
};

type Status = "loading" | "ok" | "error";

const ROLES: { role: SkillRole; enabled: boolean }[] = [
  { role: "coder", enabled: true },
  { role: "mini", enabled: true },
  { role: "design", enabled: false },
  { role: "orchestrator", enabled: false },
];

export function SkillsToolsModal({ projectRoot, onClose }: Props) {
  const [entries, setEntries] = useState<SkillEntry[]>([]);
  const [active, setActive] = useState<SkillRole>("coder");
  const [status, setStatus] = useState<Status>("loading");
  const dialogRef = useRef<HTMLDivElement>(null);

  // Fetch per-role skills for the project. Re-runs if the root changes; the
  // cancelled flag prevents setState after unmount or on a superseded fetch.
  useEffect(() => {
    let cancelled = false;
    setStatus("loading");
    setEntries([]); // clear stale content before the new fetch resolves
    (async () => {
      try {
        const result = await invokeBackendCommand("skills_list", {
          workingFolderPath: projectRoot,
        });
        if (cancelled) return;
        setEntries(result as SkillEntry[]);
        setStatus("ok");
      } catch {
        if (cancelled) return;
        setStatus("error");
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [projectRoot]);

  // Escape-to-close (WAI-ARIA modal requirement).
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onClose]);

  // Move focus into the dialog on open; restore to the trigger on close.
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    dialogRef.current?.focus();
    return () => prev?.focus?.();
  }, []);

  const activeEntry = entries.find((e) => e.role === active);

  const handleTabClick = useCallback((role: SkillRole) => {
    const roleDef = ROLES.find((r) => r.role === role);
    if (roleDef?.enabled) setActive(role);
  }, []);

  const handleCardClick = useCallback((e: MouseEvent) => {
    e.stopPropagation();
  }, []);

  const skillBody =
    status === "loading"
      ? "Loading skills…"
      : status === "error"
        ? "Couldn't load skills for this project."
        : activeEntry?.exists
          ? activeEntry.content
          : "No skill manual for this role yet.";

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-cream-900/40 p-4"
      onClick={onClose}
    >
      <div
        ref={dialogRef}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-label="Skills and Tools"
        data-testid="skills-tools-modal"
        className="flex max-h-[85vh] w-full max-w-2xl flex-col rounded-2xl border border-cream-200 bg-white shadow-xl outline-none"
        onClick={handleCardClick}
      >
        <div className="flex items-center justify-between border-b border-cream-100 px-5 py-4">
          <div className="flex items-center gap-2">
            <Sparkles className="h-4 w-4 text-teal" />
            <span className="text-[13px] font-semibold text-cream-800">
              Skills &amp; Tools
            </span>
          </div>
          <button
            type="button"
            data-testid="skills-tools-close"
            aria-label="Close"
            onClick={onClose}
            className="rounded-lg p-1 text-cream-400 transition-colors hover:bg-cream-100 hover:text-cream-700"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="flex gap-2 border-b border-cream-100 px-5 py-3">
          {ROLES.map(({ role, enabled }) => (
            <button
              key={role}
              type="button"
              data-testid={`skills-tools-tab-${role}`}
              onClick={() => handleTabClick(role)}
              disabled={!enabled}
              className={`rounded-lg border px-3 py-1.5 text-[12px] font-semibold transition-colors ${
                active === role
                  ? "border-teal/30 bg-teal/10 text-teal"
                  : enabled
                    ? "border-cream-200 bg-white text-cream-600 hover:border-cream-300"
                    : "cursor-not-allowed border-cream-200 text-cream-400 opacity-50"
              }`}
            >
              {role.charAt(0).toUpperCase() + role.slice(1)}
              {!enabled && (
                <span className="ml-1 text-[10px] font-normal opacity-70">
                  · coming soon
                </span>
              )}
            </button>
          ))}
        </div>

        <div className="overflow-y-auto px-5 py-4">
          <div className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
            Skills
          </div>
          <input
            data-testid="skills-tools-search"
            type="text"
            disabled
            title="Library search — coming soon"
            placeholder="Search the library — skills and tools… (coming soon)"
            className="mb-3 w-full max-w-xs rounded-2xl border border-cream-200 bg-cream-50 px-3 py-1.5 text-[12px] text-cream-400 focus:outline-none"
          />
          <pre
            data-testid="skills-tools-skill-content"
            className="whitespace-pre-wrap rounded-xl border border-cream-100 bg-cream-50 p-3 text-[12px] text-cream-800"
          >
            {skillBody}
          </pre>
          <div className="mb-2 mt-5 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
            Tools
          </div>
          <div
            data-testid="skills-tools-tools-placeholder"
            className="text-[12px] text-cream-500"
          >
            Per-role tools — coming soon.
          </div>
        </div>

        <div className="border-t border-cream-100 px-5 py-3 text-[11px] text-cream-500">
          The active skill manual for the{" "}
          <span className="font-semibold">{active}</span> role.
        </div>
      </div>
    </div>
  );
}
