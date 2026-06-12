// Pure, DOM-free model + event-driven plumbing for the project-card chips
// (Phase C of the Projects/Agents IA redesign).
//
// Two concerns live here so ProjectCard.tsx stays a thin JSX mapper and the
// logic is unit-testable in node (this repo has no jsdom):
//
//   1. Chip formatters — `censorChipLabel`/`censorChipAria` (open Censor finding
//      count) and `gitChipModel` (ahead/behind/dirty). All are total and
//      null-safe: a missing/zero/negative/NaN input yields null so the chip is
//      simply hidden. They render COUNTS ONLY — never a path, branch name, diff,
//      or any raw value that could leak a secret.
//
//   2. `CensorCountsTracker` — the NO-NEW-POLLER count source. It fetches one
//      `censor_count_open(root)` per project once on start, then REFETCHES only
//      when the backend emits `censor://findings-updated` (an event listener,
//      not a poll loop). A burst of events coalesces into a single re-sweep, and
//      `stop()` unsubscribes the listener so nothing leaks. Both `invoke` and
//      `listen` are injected so the orchestration is testable without Tauri.

import {
  CENSOR_FINDINGS_UPDATED_EVENT,
  type CensorFindingsUpdatedPayload,
  type ProjectGitStatus,
} from "../../types/backend";

// ---- censor chip ------------------------------------------------------------

/** Sanitize an open-finding count to a non-negative integer, or null when the
 *  chip must be hidden (undefined / zero / negative / NaN). */
function normalizeCount(count: number | undefined | null): number | null {
  if (count === undefined || count === null) return null;
  if (!Number.isFinite(count)) return null;
  const floored = Math.floor(count);
  return floored > 0 ? floored : null;
}

/** Compact censor chip label, e.g. "⚠3"; null when there are no open findings
 *  (or the count is not yet known) so the chip is hidden. */
export function censorChipLabel(count: number | undefined | null): string | null {
  const n = normalizeCount(count);
  return n === null ? null : `⚠${n}`;
}

/** Accessible label for the censor chip, e.g. "3 open Censor findings"; null
 *  when hidden. */
export function censorChipAria(count: number | undefined | null): string | null {
  const n = normalizeCount(count);
  if (n === null) return null;
  return `${n} open Censor finding${n === 1 ? "" : "s"}`;
}

// ---- git chip ---------------------------------------------------------------

export interface GitChipModel {
  /** Compact segments in a stable order, e.g. ["↑1", "↓2", "5∆"]. */
  segments: string[];
  /** Full accessible label, e.g. "Git: 1 ahead, 2 behind, 5 uncommitted changes". */
  ariaLabel: string;
}

/** Sanitize a git counter to a non-negative integer (0 for missing/negative/NaN). */
function safeCounter(value: number | undefined | null): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return 0;
  const floored = Math.floor(value);
  return floored > 0 ? floored : 0;
}

/** Build the git chip model from a (possibly partial / null) ProjectGitStatus.
 *  Returns null — i.e. HIDE the chip — when the project is not a git repo or the
 *  working tree is clean AND in sync (ahead/behind/dirty all zero). Renders only
 *  counts; never the branch name, commit, or upstream (no secret/raw value).
 *  `dirtyCount` is the backend's already-derived total uncommitted-change count
 *  (staged + unstaged + untracked), so the chip shows it directly rather than
 *  re-summing the per-bucket counters. */
export function gitChipModel(
  gitStatus: ProjectGitStatus | null | undefined,
): GitChipModel | null {
  if (!gitStatus || !gitStatus.isGitRepo) return null;
  const ahead = safeCounter(gitStatus.aheadCount);
  const behind = safeCounter(gitStatus.behindCount);
  const dirty = safeCounter(gitStatus.dirtyCount);
  if (ahead === 0 && behind === 0 && dirty === 0) return null;

  const segments: string[] = [];
  const ariaParts: string[] = [];
  if (ahead > 0) {
    segments.push(`↑${ahead}`);
    ariaParts.push(`${ahead} ahead`);
  }
  if (behind > 0) {
    segments.push(`↓${behind}`);
    ariaParts.push(`${behind} behind`);
  }
  if (dirty > 0) {
    segments.push(`${dirty}∆`);
    ariaParts.push(`${dirty} uncommitted changes`);
  }
  return { segments, ariaLabel: `Git: ${ariaParts.join(", ")}` };
}

// ---- event-driven count tracker (NO new poller) -----------------------------

/** Minimal project shape the tracker needs: an id and its agent root path. */
export interface CensorTrackedProject {
  id: string;
  rootPath: string | null;
}

/** The map of open-finding counts keyed by project id. */
export type CensorCountByProject = Record<string, number>;

/** Sentinel for "a full sweep is the coalesced pending work". A unique symbol so
 *  it can never collide with a real project id in the pending slot. */
const FULL_SWEEP: unique symbol = Symbol("censor-full-sweep");

/**
 * Build the stable signature the host effect keys the tracker on. Two project
 * sets that differ in any id or root MUST yield different strings, while the
 * SAME set (re-fetched as a fresh array on every poll) yields the SAME string so
 * the tracker is not needlessly re-bound. Entries are sorted (order-independent)
 * and joined with control characters that can never appear in a project id or a
 * filesystem path, so no id/path boundary can be aliased into a different one
 * (the separator-less join bug). The id and root are themselves delimited by a
 * record separator so `{id:"a", root:"b"}` and `{id:"ab", root:""}` differ.
 */
export function censorTrackedSignature(
  projects: ReadonlyArray<CensorTrackedProject>,
): string {
  return projects
    .map((p) => `${p.id}${p.rootPath ?? ""}`)
    // Locale-INDEPENDENT ordering: a bare comparator over UTF-16 code units, not
    // String#localeCompare, so the signature is stable across locales/ICU builds.
    .sort((a, b) => (a < b ? -1 : a > b ? 1 : 0))
    .join("");
}

// The tracker only ever calls `censor_count_open`, which returns a number, so we
// type the injected invoker concretely (the generic invokeBackendCommand is
// assignable to this narrower shape). Keeping it non-generic also makes the test
// mocks (`async () => number`) type-check without a phantom <T>.
type InvokeFn = (
  command: string,
  args?: Record<string, unknown>,
) => Promise<number>;
type ListenFn = (
  channel: string,
  handler: (event: { payload: unknown }) => void,
) => Promise<() => void>;

/** Shallow value-equality for two count maps (same keys, same numbers). Used to
 *  suppress no-op publishes so an event that doesn't change any open count never
 *  re-renders the board. */
function sameCounts(
  a: CensorCountByProject,
  b: CensorCountByProject,
): boolean {
  const aKeys = Object.keys(a);
  const bKeys = Object.keys(b);
  if (aKeys.length !== bKeys.length) return false;
  for (const key of aKeys) {
    if (a[key] !== b[key]) return false;
  }
  return true;
}

/** True when every value in the map is 0 (or the map is empty). */
function allZero(map: CensorCountByProject): boolean {
  for (const key of Object.keys(map)) {
    if (map[key] !== 0) return false;
  }
  return true;
}

/** Extract a projectId from an arbitrary findings-updated payload, defensively.
 *  Returns null when the payload is absent or not the expected shape (so the
 *  caller falls back to a full sweep). */
function payloadProjectId(payload: unknown): string | null {
  if (!payload || typeof payload !== "object") return null;
  const candidate = (payload as Partial<CensorFindingsUpdatedPayload>).projectId;
  return typeof candidate === "string" && candidate.length > 0
    ? candidate
    : null;
}

export interface CensorCountsTrackerOptions {
  /** Backend invoker (e.g. invokeBackendCommand); injected for testability. */
  invoke: InvokeFn;
  /** Tauri event subscriber returning an unlisten fn; injected for testability. */
  listen: ListenFn;
  /** Called with the fresh map after every successful (re)sweep. */
  onChange?: (counts: CensorCountByProject) => void;
}

/** Maintains a per-project open-Censor-finding count map, refreshed on start and
 *  on every `censor://findings-updated` event. NOT a poller — the only repeating
 *  trigger is the backend event. Resilient: a project with no root or a failing
 *  `censor_count_open` contributes 0 and never throws out of `start`.
 *
 *  Refetch is TARGETED when the event payload carries a known tracked projectId:
 *  only that one project's count is re-fetched and merged, so a file-save on a
 *  30-project board is ONE IPC call, not 30. An absent or unknown projectId
 *  falls back to the full sweep. */
export class CensorCountsTracker {
  private readonly invoke: InvokeFn;
  private readonly listen: ListenFn;
  private readonly onChange?: (counts: CensorCountByProject) => void;

  private projects: CensorTrackedProject[] = [];
  private unlisten: (() => void) | null = null;
  // Bumped on every stop()/start() so a late listener callback or an in-flight
  // sweep from a superseded generation is dropped instead of writing stale data.
  private epoch = 0;
  // Coalesce a burst of events while a sweep is in flight. `null` means nothing
  // pending; a non-null Set means "refetch ONLY these project ids when the
  // current sweep finishes"; the FULL_SWEEP sentinel means "do a full sweep".
  private pending: Set<string> | typeof FULL_SWEEP | null = null;
  private sweeping = false;

  private _counts: CensorCountByProject = {};
  // Whether we have ever published a map. Lets the first sweep skip an all-zero
  // publish (no chips to show) without suppressing a later all-zero map that
  // genuinely cleared previously-shown counts.
  private published = false;

  constructor(options: CensorCountsTrackerOptions) {
    this.invoke = options.invoke;
    this.listen = options.listen;
    this.onChange = options.onChange;
  }

  /** The current count map (live; treat as read-only). */
  get counts(): CensorCountByProject {
    return this._counts;
  }

  /** (Re)bind to the given projects: tear down any prior listener, do an initial
   *  sweep, then subscribe to the findings-updated event. Idempotent and safe to
   *  call on every board refresh. Never throws. */
  async start(projects: CensorTrackedProject[]): Promise<void> {
    // New generation: drops any prior listener + supersedes in-flight sweeps.
    this.teardown();
    const epoch = ++this.epoch;
    this.projects = projects.map((p) => ({ id: p.id, rootPath: p.rootPath }));

    await this.sweep(epoch);
    if (epoch !== this.epoch) return; // superseded mid-sweep

    try {
      const unlisten = await this.listen(
        CENSOR_FINDINGS_UPDATED_EVENT,
        (event) => this.scheduleRefetch(epoch, event?.payload),
      );
      if (epoch !== this.epoch) {
        // Superseded while subscribing: drop the listener we just attached so a
        // newer generation owns the single subscription.
        unlisten();
        return;
      }
      this.unlisten = unlisten;
    } catch {
      // Event subscription unavailable (e.g. not in Tauri): the initial sweep
      // map still stands; we just won't get live refetches.
    }
  }

  /** Drop the event listener and supersede any pending/in-flight work. */
  stop(): void {
    this.teardown();
  }

  private teardown(): void {
    this.epoch += 1;
    this.pending = null;
    this.sweeping = false;
    // Abort all prior-generation work: a sweep still in flight belongs to the
    // OLD epoch now and must not leave `sweeping` true for the next generation
    // (otherwise the next sweep's finally — epoch-guarded — never runs and a
    // later event would see sweeping=true and silently coalesce forever, or the
    // stale sweep's finally would flip a current-generation flag).
    if (this.unlisten) {
      this.unlisten();
      this.unlisten = null;
    }
  }

  /** Coalesce event bursts. When the event names a KNOWN tracked project we want
   *  a targeted refetch of just that project; otherwise a full sweep. While a
   *  sweep is in flight we record the desired work and let the current sweep's
   *  finally drain exactly one coalesced follow-up. Ignores callbacks from a
   *  superseded generation. */
  private scheduleRefetch(epoch: number, payload?: unknown): void {
    if (epoch !== this.epoch) return;
    const projectId = payloadProjectId(payload);
    const targeted =
      projectId !== null && this.projects.some((p) => p.id === projectId);

    if (this.sweeping) {
      this.mergePending(targeted ? projectId! : FULL_SWEEP);
      return;
    }
    if (targeted) {
      void this.refetchProject(epoch, projectId!);
    } else {
      void this.sweep(epoch);
    }
  }

  /** Merge a wanted refetch into the pending coalesce slot. A full sweep
   *  subsumes any targeted set; targeted ids accumulate into a Set. */
  private mergePending(want: string | typeof FULL_SWEEP): void {
    if (this.pending === FULL_SWEEP) return; // already the broadest
    if (want === FULL_SWEEP) {
      this.pending = FULL_SWEEP;
      return;
    }
    if (this.pending === null) this.pending = new Set();
    this.pending.add(want);
  }

  /** Drain the single coalesced follow-up recorded while a sweep ran. Called
   *  only from a current-generation sweep's epoch-guarded finally. */
  private drainPending(epoch: number): void {
    const pending = this.pending;
    this.pending = null;
    if (pending === null) return;
    if (pending === FULL_SWEEP) {
      void this.sweep(epoch);
      return;
    }
    // A small set of targeted projects: refetch them together (still far fewer
    // invokes than a full board sweep), merging the results in one publish.
    void this.refetchProjects(epoch, [...pending]);
  }

  /** Fetch the count for one project's root (degrading to 0 on failure/no root). */
  private async countFor(project: CensorTrackedProject): Promise<number> {
    const root = project.rootPath?.trim();
    if (!root) return 0;
    try {
      return safeCounter(await this.invoke("censor_count_open", { root }));
    } catch {
      return 0;
    }
  }

  /** Publish a candidate next map, applying the equality + first-all-zero guards.
   *  Returns nothing; mutates `_counts` and fires onChange only when it changed. */
  private publish(next: CensorCountByProject): void {
    // Only publish (and trigger a board re-render) when a count actually
    // changed. A findings-updated event for a file whose OPEN count is the same
    // (e.g. an edit that neither adds nor clears a finding) must not churn the
    // board — this is the re-render-storm guard for event bursts.
    if (sameCounts(this._counts, next)) return;
    // First publish of an all-zero map is a no-op for the board (no chips), so
    // skip it: it would otherwise force one render of an empty-chip board on
    // mount. A LATER all-zero map (after counts were shown) still publishes
    // because by then `published` is true — it must clear the stale chips.
    if (!this.published && allZero(next)) {
      this._counts = next;
      return;
    }
    this._counts = next;
    this.published = true;
    this.onChange?.(this._counts);
  }

  /** Full sweep: fetch the count for every tracked project and publish the new
   *  map. A failed or root-less project counts as 0. Drains a coalesced
   *  follow-up on completion. */
  private async sweep(epoch: number): Promise<void> {
    if (epoch !== this.epoch) return;
    this.sweeping = true;
    try {
      const entries = await Promise.all(
        this.projects.map(
          async (project) => [project.id, await this.countFor(project)] as const,
        ),
      );
      if (epoch !== this.epoch) return; // superseded: drop this result
      this.publish(Object.fromEntries(entries));
    } finally {
      // Epoch-guard the shared-flag writes: a stale-generation sweep (whose
      // teardown already reset `sweeping`/`pending` for the NEW generation) must
      // never flip the current generation's flags or kick its work.
      if (epoch === this.epoch) {
        this.sweeping = false;
        this.drainPending(epoch);
      }
    }
  }

  /** Targeted refetch of a single project, merged into the existing map. One IPC
   *  call instead of a full N-project sweep. */
  private refetchProject(epoch: number, projectId: string): Promise<void> {
    return this.refetchProjects(epoch, [projectId]);
  }

  /** Targeted refetch of a small set of projects, merged into the existing map. */
  private async refetchProjects(
    epoch: number,
    projectIds: string[],
  ): Promise<void> {
    if (epoch !== this.epoch) return;
    const targets = this.projects.filter((p) => projectIds.includes(p.id));
    if (targets.length === 0) return; // none still tracked
    this.sweeping = true;
    try {
      const entries = await Promise.all(
        targets.map(
          async (project) => [project.id, await this.countFor(project)] as const,
        ),
      );
      if (epoch !== this.epoch) return; // superseded: drop this result
      this.publish({ ...this._counts, ...Object.fromEntries(entries) });
    } finally {
      if (epoch === this.epoch) {
        this.sweeping = false;
        this.drainPending(epoch);
      }
    }
  }
}
