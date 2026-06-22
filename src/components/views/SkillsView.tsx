import {
  AlertTriangle,
  CheckCircle2,
  FolderOpen,
  GraduationCap,
  RefreshCw,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { invokeBackendCommand } from "../../context/AppContext";
import {
  MAX_SKILL_BYTES,
  type CatalogEntry,
  type SkillEntry,
  type SkillRole,
} from "../../types/skills";

// P10(b) Step 3 — the top-level "Skills" view: the unified per-project SKILL.md
// editor + on/off toggle + bundled-template installer. Skills are PER-PROJECT
// (there is no global working folder in scope here), so this view owns its own
// native folder picker and only renders the role cards once a folder is chosen.
//
// The backend commands already exist (src-tauri/src/backend/project_skill.rs);
// this is frontend-only and GPU-free. It mirrors MiniWriteBehaviorCard's
// race/robustness shape: a `loaded` flag, a `busy` flag, an inline error banner,
// controls disabled while busy/not-loaded, and an ignore-stale-after-unmount
// guard (`mountedRef`). Backend error strings are surfaced verbatim.

// The three role cards render in this fixed order (the backend returns the same
// set; we drive the order from here so an absent/extra role can't reshuffle it).
const ROLE_ORDER: readonly SkillRole[] = ["mini", "coder", "design", "orchestrator"];

const ROLE_LABELS: Record<SkillRole, string> = {
  mini: "Mini",
  coder: "Coder",
  design: "Design",
  orchestrator: "Orchestrator",
};

const ROLE_DESCRIPTIONS: Record<SkillRole, string> = {
  mini: "House conventions injected for the local mini executor.",
  coder: "House conventions injected for the coder agent.",
  design: "House conventions injected for the design generator.",
  orchestrator: "House conventions injected for the local main-coder orchestrator.",
};

// A blank per-role record, DERIVED from ROLE_ORDER — so adding a role (above) needs no edits at
// the ~8 places that previously hardcoded `{ mini: …, coder: …, design: … }` literals.
function blankRoleRecord<T>(value: T): Record<SkillRole, T> {
  return Object.fromEntries(ROLE_ORDER.map((role) => [role, value])) as Record<
    SkillRole,
    T
  >;
}

// A single shared encoder — `byteLength` runs on every keystroke (per-render
// in each card), so allocating a fresh TextEncoder each call is pure waste.
const SKILL_BYTE_ENCODER = new TextEncoder();

/** Byte length of a draft (the backend caps on bytes, not chars). */
function byteLength(value: string): number {
  return SKILL_BYTE_ENCODER.encode(value).length;
}

export function SkillsView() {
  // The chosen project folder. null == nothing picked yet (empty state).
  const [folder, setFolder] = useState<string | null>(null);
  // The per-role entries from the last skills_list. null == not loaded yet.
  const [entries, setEntries] = useState<SkillEntry[] | null>(null);
  // The bundled catalog (fetched once on mount). null == not loaded / failed.
  const [catalog, setCatalog] = useState<CatalogEntry[] | null>(null);
  // Local editor drafts keyed by role. Seeded from each list, edited freely.
  const [drafts, setDrafts] = useState<Record<SkillRole, string>>(
    blankRoleRecord(""),
  );
  // Per-role explicit acknowledgement that saving a truncated skill drops the tail.
  const [ackTruncated, setAckTruncated] = useState<Record<SkillRole, boolean>>(
    blankRoleRecord(false),
  );

  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedRole, setSavedRole] = useState<SkillRole | null>(null);

  const mountedRef = useRef(true);
  const savedTimer = useRef<number | null>(null);
  // Monotonic generation for `refresh`: two rapid folder picks each kick off a
  // `skills_list`, and the slower (stale) one can resolve last. Each refresh
  // captures its generation up-front and bails after every await if a newer
  // refresh has since started — so folder A's late list can never overwrite
  // folder B's entries/drafts/acks (or clear B's busy/error). `mountedRef`
  // alone only guards unmount, not a superseded-but-still-mounted fetch.
  const refreshGenRef = useRef(0);
  // Synchronous mutation lock. `busy` is React state, so onToggle/onSave/onInstall
  // fired in the same tick all read busy===false and interleave (double-save,
  // toggle+install racing the same skills-state.json). This ref flips
  // synchronously, gating every mutation handler before any await; `busy` state
  // still drives the disabled/rendering. `refresh` itself is NOT gated (it is
  // also called by pickFolder/reloadCurrent) — the gen counter handles its race.
  const busyRef = useRef(false);
  // The per-role content from the LAST list, so a re-list (after toggling/saving
  // ONE role) only reseeds a draft the user has NOT locally diverged from — an
  // unsaved edit in another role's textarea must never be silently clobbered.
  const lastLoadedRef = useRef<Record<SkillRole, string>>(blankRoleRecord(""));
  // Live mirror of the drafts so `refresh` can compare against the user's current
  // text without depending on (and re-creating itself for) every keystroke.
  const draftsRef = useRef<Record<SkillRole, string>>(blankRoleRecord(""));

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      if (savedTimer.current !== null) {
        window.clearTimeout(savedTimer.current);
        savedTimer.current = null;
      }
    };
  }, []);

  // Mirror committed drafts into the ref so `refresh` reads the live text. Done
  // in a layout effect (not the render body) so the ref only updates from
  // COMMITTED state: in React 18 concurrent mode a render can be interrupted and
  // retried, and a render-body write would leave the ref pointing at a draft that
  // was never committed — an awaited `refresh` continuation reading draftsRef in
  // its divergence check could then clobber an unsaved edit. Layout effects run
  // synchronously after commit, before paint, so the ref is consistent with
  // committed state and the next refresh's read.
  useLayoutEffect(() => {
    draftsRef.current = drafts;
  }, [drafts]);

  // Fetch the bundled catalog ONCE on mount. It needs no folder and no lock, so
  // a failure degrades silently (the Install-template control simply shows none).
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const list = await invokeBackendCommand<CatalogEntry[]>("skills_catalog");
        if (!cancelled && mountedRef.current) {
          setCatalog(Array.isArray(list) ? list : []);
        }
      } catch {
        if (!cancelled && mountedRef.current) setCatalog([]);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Re-list the skills for the given folder and reseed the per-role drafts from
  // the fresh content. Clears the truncation ack only for roles whose fresh read
  // is no longer truncated (a stale ack must never carry over a save guard, but a
  // still-truncated role keeps the ack the user already gave).
  const refresh = useCallback(
    async (folderPath: string, forceReseed?: SkillRole) => {
    // Claim this refresh's generation BEFORE any state/await; a later refresh
    // (newer folder pick) bumps it, marking this one superseded.
    const gen = ++refreshGenRef.current;
    setBusy(true);
    setError(null);
    try {
      const list = await invokeBackendCommand<SkillEntry[]>("skills_list", {
        workingFolderPath: folderPath,
      });
      // Bail if unmounted OR superseded by a newer refresh — a stale list must
      // never overwrite the current folder's entries/drafts/acks.
      if (!mountedRef.current || refreshGenRef.current !== gen) return;
      const rows = Array.isArray(list) ? list : [];
      setEntries(rows);
      // Reseed each role's draft from the fresh on-disk content, but PRESERVE a
      // draft the user has locally diverged from (an unsaved edit in one role's
      // textarea must survive a re-list triggered by another role's mutation).
      // "Diverged" = the current draft differs from what we last loaded for that
      // role; in that case keep the user's text. A just-saved/just-installed role
      // re-loads to its new content (its draft already equals the last-loaded
      // value, or the new content, so it is not treated as diverged).
      const prevLoaded = lastLoadedRef.current;
      const liveDrafts = draftsRef.current;
      const nextLoaded: Record<SkillRole, string> = blankRoleRecord("");
      const nextDrafts: Record<SkillRole, string> = { ...liveDrafts };
      for (const role of ROLE_ORDER) {
        const row = rows.find((r) => r.role === role);
        const content = row?.content ?? "";
        nextLoaded[role] = content;
        // Force-reseed the role we just wrote (install replaces content the user
        // can't see in their draft; an explicit overwrite must show the result).
        const diverged =
          role !== forceReseed && liveDrafts[role] !== prevLoaded[role];
        if (!diverged) nextDrafts[role] = content;
      }
      lastLoadedRef.current = nextLoaded;
      draftsRef.current = nextDrafts;
      setDrafts(nextDrafts);
      // Clear the truncation ack ONLY for roles whose fresh read is no longer
      // truncated. A role that is STILL truncated keeps its ack, so toggling/
      // saving an unrelated role doesn't force the user to re-acknowledge a
      // truncation they already accepted.
      setAckTruncated((prev) => {
        const next = { ...prev };
        for (const role of ROLE_ORDER) {
          const row = rows.find((r) => r.role === role);
          if (!row?.truncated) next[role] = false;
        }
        return next;
      });
      setLoaded(true);
    } catch (e) {
      // Only the latest refresh may write error/loaded — a superseded list's
      // failure must not stomp the current folder's banner. Drop the previous
      // folder's entries/acks so a failed re-list can't leave a stale truncated
      // entry that keeps Save enabled on the wrong data; the view then shows the
      // empty/no-cards state plus this error banner, which is correct.
      if (mountedRef.current && refreshGenRef.current === gen) {
        setEntries(null);
        setAckTruncated(blankRoleRecord(false));
        setError(
          e instanceof Error ? e.message : "Could not load skills for this folder.",
        );
        setLoaded(true);
      }
    } finally {
      // Only the latest generation may release the busy flag (a superseded
      // refresh clearing busy would unlock the UI mid-flight for the live one).
      if (mountedRef.current && refreshGenRef.current === gen) setBusy(false);
    }
    },
    [],
  );

  // Open the NATIVE OS directory picker and, on a real pick, store the folder and
  // list its skills. Narrows the plugin's `string | string[] | null` to a single
  // path (mirrors DesignView.pickFolder). A cancel/unavailable dialog is a no-op.
  const pickFolder = useCallback(async () => {
    let picked: string | null = null;
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const result = await open({
        directory: true,
        multiple: false,
        title: "Choose a project folder",
      });
      if (typeof result === "string" && result.trim()) picked = result;
    } catch {
      // Dialog plugin unavailable or user dismissed — no-op.
    }
    if (picked === null || !mountedRef.current) return;
    // A new folder is a clean slate: drop the previous folder's draft/divergence
    // tracking AND its entries/acks so its data can't leak into the newly-picked
    // project even if the new folder's list then fails (a failed list would
    // otherwise leave the old folder's truncated entry + ack, keeping Save
    // enabled on the wrong data).
    const blank = blankRoleRecord("");
    lastLoadedRef.current = { ...blank };
    draftsRef.current = { ...blank };
    setDrafts({ ...blank });
    setEntries(null);
    setAckTruncated(blankRoleRecord(false));
    setFolder(picked);
    setError(null);
    await refresh(picked);
  }, [refresh]);

  // Re-list the currently-selected folder (manual Refresh / post-mutation reload).
  const reloadCurrent = useCallback(async () => {
    if (!folder) return;
    await refresh(folder);
  }, [folder, refresh]);

  const flashSaved = useCallback((role: SkillRole) => {
    setSavedRole(role);
    if (savedTimer.current !== null) window.clearTimeout(savedTimer.current);
    savedTimer.current = window.setTimeout(() => {
      if (mountedRef.current) setSavedRole(null);
    }, 2000);
  }, []);

  // Toggle a role's skill on/off, then re-list so the status line + toggle reflect
  // the persisted state (and so a backend corrupt-state error surfaces verbatim).
  const onToggle = useCallback(
    async (role: SkillRole, enabled: boolean) => {
      if (!folder) return;
      // Synchronous gate: a second mutation fired in the same tick (before
      // `busy` state propagates) is dropped, so concurrent writes can't race
      // the same skills-state.json.
      if (busyRef.current) return;
      busyRef.current = true;
      setBusy(true);
      setError(null);
      try {
        await invokeBackendCommand<void>("skills_set_enabled", {
          workingFolderPath: folder,
          role,
          enabled,
        });
        await refresh(folder);
      } catch (e) {
        if (mountedRef.current) {
          setError(
            e instanceof Error ? e.message : "Could not change the skill toggle.",
          );
        }
      } finally {
        busyRef.current = false;
        if (mountedRef.current) setBusy(false);
      }
    },
    [folder, refresh],
  );

  // Save a role's draft, then re-list. The caller (the card) has already enforced
  // the byte cap + the truncation acknowledgement before enabling Save.
  const onSave = useCallback(
    async (role: SkillRole) => {
      if (!folder) return;
      if (busyRef.current) return;
      busyRef.current = true;
      // Read the LIVE draft from the ref (not the closure-captured `drafts`), so
      // this callback need not depend on `drafts` and re-create on every
      // keystroke (which would re-render all three RoleCards). draftsRef is the
      // canonical live mirror used everywhere else in this component.
      const content = draftsRef.current[role];
      setBusy(true);
      setError(null);
      try {
        await invokeBackendCommand<void>("skills_save", {
          workingFolderPath: folder,
          role,
          content,
        });
        if (mountedRef.current) flashSaved(role);
        // Force-reseed the just-saved role so its editor reflects the new
        // on-disk content (e.g. a backend that normalises/trims on write).
        await refresh(folder, role);
      } catch (e) {
        if (mountedRef.current) {
          setError(e instanceof Error ? e.message : "Could not save the skill.");
        }
      } finally {
        busyRef.current = false;
        if (mountedRef.current) setBusy(false);
      }
    },
    [folder, refresh, flashSaved],
  );

  // Install a bundled template into a role (confirming an overwrite when a skill
  // already exists), then re-list.
  const onInstall = useCallback(
    async (role: SkillRole, catalogId: string, exists: boolean) => {
      if (!folder) return;
      // The confirm prompt (synchronous) precedes the lock so a declined
      // overwrite never leaves the lock stuck.
      if (
        exists &&
        !window.confirm(`Overwrite the existing ${role} skill with this template?`)
      ) {
        return;
      }
      if (busyRef.current) return;
      busyRef.current = true;
      setBusy(true);
      setError(null);
      try {
        await invokeBackendCommand<void>("skills_install_from_catalog", {
          workingFolderPath: folder,
          role,
          catalogId,
        });
        // Force-reseed the installed role so the editor shows the template body
        // even if the user had unsaved edits there (they confirmed the overwrite).
        await refresh(folder, role);
      } catch (e) {
        if (mountedRef.current) {
          setError(
            e instanceof Error ? e.message : "Could not install the template.",
          );
        }
      } finally {
        busyRef.current = false;
        if (mountedRef.current) setBusy(false);
      }
    },
    [folder, refresh],
  );

  // Index entries + catalog by role so each card reads its own row in O(1).
  const entryByRole = useMemo(() => {
    const map = new Map<SkillRole, SkillEntry>();
    for (const e of entries ?? []) {
      if (ROLE_ORDER.includes(e.role)) map.set(e.role, e);
    }
    return map;
  }, [entries]);

  const catalogByRole = useMemo(() => {
    const map = new Map<SkillRole, CatalogEntry[]>();
    for (const c of catalog ?? []) {
      if ((ROLE_ORDER as readonly string[]).includes(c.role)) {
        const role = c.role as SkillRole;
        const list = map.get(role) ?? [];
        list.push(c);
        map.set(role, list);
      }
    }
    return map;
  }, [catalog]);

  const controlsDisabled = busy || !loaded;

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-lg font-semibold text-cream-800">Skills</h1>
        <p className="mt-1 max-w-3xl text-[12px] leading-5 text-cream-500">
          Per-project SKILL.md house conventions for each role. Pick a project
          folder, edit a role&apos;s skill, toggle it on or off, or install a
          starter template. Skills are stored in the project&apos;s{" "}
          <code className="rounded bg-cream-100 px-1 py-0.5 text-[11px]">
            .claude/skills
          </code>{" "}
          folder.
        </p>
      </div>

      {/* Folder picker + current selection + manual refresh. */}
      <section className="rounded-2xl border border-cream-200 bg-white p-4">
        <div className="flex flex-wrap items-center gap-3">
          <button
            type="button"
            onClick={() => void pickFolder()}
            disabled={busy}
            className="inline-flex items-center gap-2 rounded-2xl border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-700 transition-colors hover:border-teal/40 disabled:opacity-60"
          >
            <FolderOpen className="h-4 w-4 text-teal" />
            Choose project folder
          </button>
          {folder ? (
            <button
              type="button"
              onClick={() => void reloadCurrent()}
              disabled={busy}
              className="inline-flex items-center gap-1.5 rounded-2xl border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 transition-colors hover:border-teal/40 disabled:opacity-60"
            >
              <RefreshCw className="h-3.5 w-3.5" />
              Refresh
            </button>
          ) : null}
          {folder ? (
            <span
              className="min-w-0 truncate text-[11px] text-cream-500"
              title={folder}
            >
              {folder}
            </span>
          ) : null}
        </div>
      </section>

      {error ? (
        <p className="flex items-start gap-2 rounded-2xl border border-coral/30 bg-coral/[0.05] px-3 py-2 text-[11px] leading-4 text-coral-dark">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>{error}</span>
        </p>
      ) : null}

      {folder === null ? (
        // Empty state: no folder chosen yet — no role cards, just the prompt.
        <section className="rounded-2xl border border-dashed border-cream-300 bg-cream-50/60 p-8 text-center">
          <GraduationCap className="mx-auto h-8 w-8 text-cream-300" />
          <p className="mt-3 text-[13px] font-semibold text-cream-800">
            Choose a project folder to manage its skills
          </p>
          <p className="mt-1 text-[12px] text-cream-400">
            Each project keeps its own per-role SKILL.md conventions. Pick a
            folder above to view and edit them.
          </p>
        </section>
      ) : (
        <div className="grid gap-4">
          {ROLE_ORDER.map((role) => (
            <RoleCard
              key={role}
              role={role}
              entry={entryByRole.get(role) ?? null}
              templates={catalogByRole.get(role) ?? []}
              draft={drafts[role]}
              onDraftChange={(value) =>
                setDrafts((prev) => ({ ...prev, [role]: value }))
              }
              ackTruncated={ackTruncated[role]}
              onAckChange={(value) =>
                setAckTruncated((prev) => ({ ...prev, [role]: value }))
              }
              disabled={controlsDisabled}
              saved={savedRole === role}
              onToggle={(enabled) => void onToggle(role, enabled)}
              onSave={() => void onSave(role)}
              onInstall={(catalogId, exists) =>
                void onInstall(role, catalogId, exists)
              }
            />
          ))}
        </div>
      )}
    </div>
  );
}

interface RoleCardProps {
  role: SkillRole;
  entry: SkillEntry | null;
  templates: CatalogEntry[];
  draft: string;
  onDraftChange: (value: string) => void;
  ackTruncated: boolean;
  onAckChange: (value: boolean) => void;
  disabled: boolean;
  saved: boolean;
  onToggle: (enabled: boolean) => void;
  onSave: () => void;
  onInstall: (catalogId: string, exists: boolean) => void;
}

function RoleCard({
  role,
  entry,
  templates,
  draft,
  onDraftChange,
  ackTruncated,
  onAckChange,
  disabled,
  saved,
  onToggle,
  onSave,
  onInstall,
}: RoleCardProps) {
  const exists = entry?.exists ?? false;
  const enabled = entry?.enabled ?? false;
  const truncated = entry?.truncated ?? false;

  // Status line: active when present + enabled, disabled when present + off,
  // "no skill yet" when absent.
  const statusLabel = !exists
    ? "no skill yet"
    : enabled
      ? "active"
      : "disabled";
  const statusClass = !exists
    ? "text-cream-400"
    : enabled
      ? "text-sage-dark"
      : "text-cream-500";

  const bytes = byteLength(draft);
  const overCap = bytes > MAX_SKILL_BYTES;
  // A truncated read must be explicitly acknowledged before Save is allowed —
  // saving the head-only draft would permanently discard the on-disk tail.
  const blockedByTruncation = truncated && !ackTruncated;
  const saveDisabled = disabled || overCap || blockedByTruncation;

  return (
    <section className="rounded-2xl border border-cream-200 bg-white p-4">
      <div className="mb-3 flex items-start justify-between gap-3">
        <div className="flex items-center gap-2">
          <GraduationCap className="h-4 w-4 text-teal" />
          <div>
            <h3 className="text-[13px] font-semibold text-cream-800">
              {ROLE_LABELS[role]}
            </h3>
            <p className="text-[11px] leading-4 text-cream-500">
              {ROLE_DESCRIPTIONS[role]}
            </p>
          </div>
        </div>
        <div className="flex items-center gap-3">
          <span className={`text-[11px] font-semibold ${statusClass}`}>
            {statusLabel}
          </span>
          {/* On/off toggle — only meaningful when a skill exists. */}
          <button
            type="button"
            role="switch"
            aria-checked={enabled}
            aria-label={`Toggle the ${role} skill`}
            disabled={disabled || !exists}
            onClick={() => onToggle(!enabled)}
            className={`relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors disabled:opacity-50 ${
              enabled ? "bg-teal" : "bg-cream-300"
            }`}
          >
            <span
              aria-hidden="true"
              className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform ${
                enabled ? "translate-x-4" : "translate-x-0.5"
              }`}
            />
          </button>
        </div>
      </div>

      {/* Truncation / data-loss guard. */}
      {truncated ? (
        <div className="mb-3 rounded-2xl border border-coral/30 bg-coral/[0.05] px-3 py-2.5">
          <p className="flex items-start gap-2 text-[11px] leading-4 text-coral-dark">
            <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
            <span>
              This file is larger than {MAX_SKILL_BYTES} bytes and only its first{" "}
              {MAX_SKILL_BYTES} bytes are shown here.{" "}
              <span className="font-semibold">
                Saving will permanently discard everything past{" "}
                {MAX_SKILL_BYTES} bytes.
              </span>
            </span>
          </p>
          <label className="mt-2 flex items-center gap-2 text-[11px] text-coral-dark">
            <input
              type="checkbox"
              checked={ackTruncated}
              disabled={disabled}
              onChange={(e) => onAckChange(e.target.checked)}
            />
            I understand saving discards the truncated tail
          </label>
        </div>
      ) : null}

      <textarea
        value={draft}
        onChange={(e) => onDraftChange(e.target.value)}
        disabled={disabled}
        spellCheck={false}
        rows={10}
        aria-label={`${ROLE_LABELS[role]} SKILL.md content`}
        placeholder={`Write the ${role} role's SKILL.md house conventions here.`}
        className="w-full resize-y rounded-2xl border border-cream-200 bg-cream-50/40 px-3 py-2 font-mono text-[12px] leading-5 text-cream-800 focus:border-teal/40 focus:outline-none disabled:opacity-60"
      />

      <div className="mt-2 flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-3 text-[11px]">
          <span className={overCap ? "font-semibold text-coral-dark" : "text-cream-500"}>
            {bytes} / {MAX_SKILL_BYTES} bytes
          </span>
          {overCap ? (
            <span className="text-coral-dark">trim to {MAX_SKILL_BYTES} bytes before saving</span>
          ) : null}
          {saved ? (
            <span className="inline-flex items-center gap-1 text-sage-dark">
              <CheckCircle2 className="h-3.5 w-3.5" />
              Saved
            </span>
          ) : null}
        </div>
        <button
          type="button"
          onClick={onSave}
          disabled={saveDisabled}
          className="inline-flex items-center gap-1.5 rounded-2xl bg-teal px-3 py-2 text-[12px] font-semibold text-white transition-colors hover:bg-teal/90 disabled:opacity-50"
        >
          Save
        </button>
      </div>

      {/* Install-template control: the bundled templates for this role. */}
      {templates.length > 0 ? (
        <div className="mt-3 border-t border-cream-200 pt-3">
          <p className="mb-2 text-[11px] font-semibold uppercase tracking-widest text-cream-400">
            Install template
          </p>
          <div className="grid gap-2">
            {templates.map((tpl) => (
              <button
                key={tpl.id}
                type="button"
                disabled={disabled}
                onClick={() => onInstall(tpl.id, exists)}
                className="flex flex-col items-start gap-0.5 rounded-2xl border border-cream-200 bg-white px-3 py-2 text-left transition-colors hover:border-teal/30 disabled:opacity-60"
              >
                <span className="text-[12px] font-semibold text-cream-800">
                  {tpl.name}
                </span>
                <span className="text-[11px] leading-4 text-cream-500">
                  {tpl.description}
                </span>
              </button>
            ))}
          </div>
        </div>
      ) : null}
    </section>
  );
}
