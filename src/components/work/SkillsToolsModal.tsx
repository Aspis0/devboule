import {
  useEffect,
  useRef,
  useState,
  useCallback,
  type MouseEvent,
} from "react";
import { invokeBackendCommand } from "../../context/AppContext";
import { Sparkles, X } from "lucide-react";
import { ToolsPicker } from "./ToolsPicker";
import { LibrarySearch } from "./LibrarySearch";

// Work Console ASSIGNMENT PROFILES (capability tiers), mirroring the backend's
// `ASSIGNMENT_PROFILES`. The single legacy `mini` role splits into two tiers here:
// `mini-big` (capable local model) and `mini-small` (8B, edits-only). This is the
// assignment layer — separate from the backend injection/traversal `KNOWN_ROLES` gate.
type SkillProfile =
  "coder" | "mini-big" | "mini-small" | "design" | "orchestrator";

interface SkillEntry {
  role: SkillProfile;
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

// `coder` + both mini tiers are active now; `design`/`orchestrator` are predisposed but
// disabled ("coming soon", managed in the sidebar for now). `label` carries the nice
// human form since the tier names aren't a simple capitalize.
const PROFILES: { profile: SkillProfile; label: string; enabled: boolean }[] = [
  { profile: "coder", label: "Coder", enabled: true },
  { profile: "mini-big", label: "Mini · big", enabled: true },
  { profile: "mini-small", label: "Mini · small", enabled: true },
  { profile: "design", label: "Design", enabled: false },
  { profile: "orchestrator", label: "Orchestrator", enabled: false },
];

export function SkillsToolsModal({ projectRoot, onClose }: Props) {
  const [entries, setEntries] = useState<SkillEntry[]>([]);
  const [active, setActive] = useState<SkillProfile>("coder");
  const [status, setStatus] = useState<Status>("loading");
  // Bumped after a library skill is applied so the active profile's content refreshes.
  const [reload, setReload] = useState(0);
  const dialogRef = useRef<HTMLDivElement>(null);

  // Reset the active tab to the always-enabled default when the project changes.
  useEffect(() => {
    setActive("coder");
  }, [projectRoot]);

  // Fetch per-role skills for the project. Re-runs on project change OR after an
  // apply (reload). The cancelled flag prevents setState after unmount / superseded fetch.
  useEffect(() => {
    let cancelled = false;
    setStatus("loading");
    setEntries([]); // clear stale content before the new fetch resolves
    (async () => {
      try {
        const result = await invokeBackendCommand("skills_list_profiles", {
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
  }, [projectRoot, reload]);

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
  const activeLabel =
    PROFILES.find((p) => p.profile === active)?.label ?? active;

  const handleTabClick = useCallback((profile: SkillProfile) => {
    const profileDef = PROFILES.find((p) => p.profile === profile);
    if (profileDef?.enabled) setActive(profile);
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
          {PROFILES.map(({ profile, label, enabled }) => (
            <button
              key={profile}
              type="button"
              data-testid={`skills-tools-tab-${profile}`}
              onClick={() => handleTabClick(profile)}
              disabled={!enabled}
              className={`rounded-lg border px-3 py-1.5 text-[12px] font-semibold transition-colors ${
                active === profile
                  ? "border-teal/30 bg-teal/10 text-teal"
                  : enabled
                    ? "border-cream-200 bg-white text-cream-600 hover:border-cream-300"
                    : "cursor-not-allowed border-cream-200 text-cream-400 opacity-50"
              }`}
            >
              {label}
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
          <pre
            data-testid="skills-tools-skill-content"
            className="whitespace-pre-wrap rounded-xl border border-cream-100 bg-cream-50 p-3 text-[12px] text-cream-800"
          >
            {skillBody}
          </pre>
          <div className="mb-2 mt-4 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
            From your global library
          </div>
          <LibrarySearch
            projectRoot={projectRoot}
            profile={active}
            onApplied={() => setReload((r) => r + 1)}
          />
          <div className="mb-2 mt-5 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
            Tools
          </div>
          <ToolsPicker
            key={active}
            projectRoot={projectRoot}
            profile={active}
          />
        </div>

        <div className="border-t border-cream-100 px-5 py-3 text-[11px] text-cream-500">
          The active skill manual for the{" "}
          <span className="font-semibold">{activeLabel}</span> profile.
        </div>
      </div>
    </div>
  );
}
