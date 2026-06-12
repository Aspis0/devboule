// Pure, DOM-free model + event-driven plumbing for the Censor dock panel (E2).
//
// Keeps CensorPanel.tsx / CensorFindingRow.tsx thin JSX mappers and the logic
// unit-testable in node (this repo has no jsdom). Three concerns:
//
//   1. Grouping/sort — `groupFindingsByFile` buckets OPEN findings per file and
//      sorts files by their worst severity, findings within a file by severity.
//   2. Command-arg builders — the EXACT payloads for the Tauri commands the row /
//      strip invoke (`censor_dispose_finding`, `censor_open_in_editor`,
//      `censor_review_now`) and the `launch_project_agent_terminal` verifier input
//      for "Run final review". Pure → the wiring is tested without a DOM.
//   3. `CensorFindingsTracker` — fetches `censor_get_findings(root)` once and
//      REFETCHES only on `censor://findings-updated` for THIS project (a listener,
//      not a poll). Mirrors `CensorCountsTracker`'s epoch/coalesce/cleanup design.

import {
  CENSOR_FINDINGS_UPDATED_EVENT,
  type CensorDisposition,
  type CensorFinding,
  type CensorFindingsUpdatedPayload,
} from "../../types/backend";
import type { SpawnLaunchInput } from "../agents/agentRowModel";
import { severityRank } from "./censorSeverity";

// ---- grouping ---------------------------------------------------------------

export interface CensorFileGroup {
  /** Project-relative file path (the group key). */
  file: string;
  /** This file's open findings, sorted by severity (high → low). */
  findings: CensorFinding[];
  /** Best (lowest-rank) severity present, used to sort + style the group header. */
  worstRank: number;
}

/** Bucket open findings by file and sort: files by worst severity then path,
 *  findings within a file by severity then line. Stable + total (an empty or
 *  malformed array yields []). Never inspects anything but the safe fields. */
export function groupFindingsByFile(
  findings: ReadonlyArray<CensorFinding>,
): CensorFileGroup[] {
  const byFile = new Map<string, CensorFinding[]>();
  for (const f of findings ?? []) {
    const key = (f?.file ?? "").trim() || "(unknown file)";
    const list = byFile.get(key);
    if (list) list.push(f);
    else byFile.set(key, [f]);
  }
  const groups: CensorFileGroup[] = [];
  for (const [file, list] of byFile) {
    const sorted = [...list].sort((a, b) => {
      const r = severityRank(a.severity) - severityRank(b.severity);
      if (r !== 0) return r;
      return (a.line ?? Number.MAX_SAFE_INTEGER) - (b.line ?? Number.MAX_SAFE_INTEGER);
    });
    const worstRank = sorted.reduce(
      (best, f) => Math.min(best, severityRank(f.severity)),
      Number.MAX_SAFE_INTEGER,
    );
    groups.push({ file, findings: sorted, worstRank });
  }
  groups.sort((a, b) => {
    if (a.worstRank !== b.worstRank) return a.worstRank - b.worstRank;
    return a.file.localeCompare(b.file);
  });
  return groups;
}

// ---- command-arg builders (the wiring contract, DOM-free + tested) ----------

/** Args for `censor_dispose_finding` (the row's mark-FP / wontfix action). */
export function disposeArgs(params: {
  projectId: string;
  root: string;
  file: string;
  id: string;
  disposition: CensorDisposition;
}): Record<string, string> {
  return {
    projectId: params.projectId,
    root: params.root,
    file: params.file,
    id: params.id,
    disposition: params.disposition,
  };
}

/** Args for `censor_open_in_editor` (the row's clickable file:line → open). The
 *  Rust command confines `root` to THIS project's configured root (WARNING D) and
 *  re-validates `file` is inside `root` (resolve_editor_target), so this forwards
 *  the project id + project-relative path + chosen editor. */
export function openInEditorArgs(params: {
  projectId: string;
  root: string;
  file: string;
  editor: string;
}): Record<string, string> {
  return {
    projectId: params.projectId,
    root: params.root,
    file: params.file,
    editor: params.editor,
  };
}

/** Args for `censor_review_now` whole-project sweep (the strip's "Review now"). */
export function reviewNowArgs(params: {
  projectId: string;
  root: string;
}): Record<string, unknown> {
  return { projectId: params.projectId, root: params.root, file: null };
}

/** Args for `set_censor_trusted` (BLOCKER B trust gate): opt a project in/out of
 *  running its OWN linter/Gemma configs. The "Trust & enable Censor" button sends
 *  `{ projectId, trusted: true }`. */
export function setCensorTrustedArgs(params: {
  projectId: string;
  trusted: boolean;
}): Record<string, unknown> {
  return { projectId: params.projectId, trusted: params.trusted };
}

// ---- panel view decision (pure, tested) -------------------------------------

/** The mutually-exclusive top-level states the Censor panel can render. */
export type CensorPanelViewState =
  | "loading" // status not read yet (can't tell trusted vs untrusted)
  | "no-root" // project has no working root → Censor cannot review it
  | "untrusted" // trusted === false → show the trust gate (no findings/actions)
  | "findings"; // trusted → the normal findings UI

/**
 * Decide which top-level view the Censor panel renders, given the project's root
 * and its one-shot `censor_status`.
 *
 * Order matters and is security-relevant:
 *   1. no-root wins — without a root there is nothing (and no project) to trust.
 *   2. While `status` is still null we do NOT assume trusted (would flash the
 *      findings/actions UI for an untrusted repo); show a neutral loading state.
 *   3. `trusted === false` → the gate: the engine runs NO linters/Gemma until the
 *      user explicitly opts in, so showing findings/actions would be misleading.
 *   4. Otherwise the normal findings UI.
 *
 * Pure (no React) so every branch is unit-tested without a DOM.
 */
export function censorPanelViewState(args: {
  hasRoot: boolean;
  status: { trusted: boolean } | null | undefined;
}): CensorPanelViewState {
  if (!args.hasRoot) return "no-root";
  if (!args.status) return "loading";
  return args.status.trusted ? "findings" : "untrusted";
}

/**
 * The `SpawnLaunchInput` for "Run final review": launch a `verifier` in-app on the
 * project, using the existing project-agent launch path. Phase H sets
 * `censorReview: true` so the launch threads through to
 * `ProjectAgentLaunchInput.censorReview` and the backend appends the verifier's
 * Censor residual-adjudication addendum to the launch prompt. A normal verifier
 * spawn from the Spawn panel leaves `censorReview` unset, so its prompt is
 * unchanged (back-compat).
 */
export function finalReviewLaunchInput(projectId: string): SpawnLaunchInput {
  return {
    projectId,
    role: "verifier",
    client: "claude",
    taskId: null,
    host: "app",
    model: null,
    censorReview: true,
  };
}

// ---- event-driven findings tracker (NO new poller) --------------------------

type InvokeFindingsFn = (
  command: string,
  args?: Record<string, unknown>,
) => Promise<CensorFinding[]>;
type ListenFn = (
  channel: string,
  handler: (event: { payload: unknown }) => void,
) => Promise<() => void>;

export interface CensorFindingsTrackerOptions {
  /** The project id whose findings-updated events trigger a refetch. */
  projectId: string;
  /** The project root passed to `censor_get_findings`. */
  root: string;
  invoke: InvokeFindingsFn;
  listen: ListenFn;
  /** Called with the fresh open-findings array after every successful (re)fetch. */
  onChange: (findings: CensorFinding[]) => void;
  /** Optional: called when a fetch fails (so the panel can surface a soft error). */
  onError?: (message: string) => void;
}

/** Does a findings-updated payload concern this project? The Rust emitter ALWAYS
 *  includes a `projectId` (see backend/censor/watch.rs), so a missing/empty/non-
 *  object payload is MALFORMED, not a wildcard — treat it as a NON-match (skip the
 *  refetch) rather than a spurious "refetch everything" (WARNING 5). Only a payload
 *  whose `projectId` equals this project's triggers a refetch. */
function payloadMatchesProject(payload: unknown, projectId: string): boolean {
  if (!payload || typeof payload !== "object") {
    if (typeof console !== "undefined") {
      console.debug("censor: ignoring findings-updated event with no payload");
    }
    return false;
  }
  const candidate = (payload as Partial<CensorFindingsUpdatedPayload>).projectId;
  if (typeof candidate !== "string" || candidate.length === 0) {
    if (typeof console !== "undefined") {
      console.debug("censor: ignoring findings-updated event with no projectId");
    }
    return false;
  }
  return candidate === projectId;
}

/**
 * Maintains the OPEN-findings list for ONE project, refreshed on start and on
 * every matching `censor://findings-updated` event. NOT a poller — the only
 * repeating trigger is the backend event. Epoch-guarded so a late callback or an
 * in-flight fetch from a superseded generation is dropped; `stop()` unsubscribes
 * so no listener leaks. A burst of events coalesces into one follow-up fetch.
 */
export class CensorFindingsTracker {
  private readonly projectId: string;
  private readonly root: string;
  private readonly invoke: InvokeFindingsFn;
  private readonly listen: ListenFn;
  private readonly onChange: (findings: CensorFinding[]) => void;
  private readonly onError?: (message: string) => void;

  private unlisten: (() => void) | null = null;
  private epoch = 0;
  private fetching = false;
  private pending = false;

  constructor(options: CensorFindingsTrackerOptions) {
    this.projectId = options.projectId;
    this.root = options.root;
    this.invoke = options.invoke;
    this.listen = options.listen;
    this.onChange = options.onChange;
    this.onError = options.onError;
  }

  /** Initial fetch + subscribe to the findings-updated event. Never throws. */
  async start(): Promise<void> {
    this.teardown();
    const epoch = ++this.epoch;
    await this.fetch(epoch);
    if (epoch !== this.epoch) return; // superseded mid-fetch

    try {
      const unlisten = await this.listen(
        CENSOR_FINDINGS_UPDATED_EVENT,
        (event) => this.scheduleRefetch(epoch, event?.payload),
      );
      if (epoch !== this.epoch) {
        unlisten();
        return;
      }
      this.unlisten = unlisten;
    } catch {
      // No event bus (e.g. not in Tauri): the initial list still stands.
    }
  }

  /** Drop the listener + supersede any in-flight / pending work. */
  stop(): void {
    this.teardown();
  }

  private teardown(): void {
    this.epoch += 1;
    this.fetching = false;
    this.pending = false;
    if (this.unlisten) {
      this.unlisten();
      this.unlisten = null;
    }
  }

  private scheduleRefetch(epoch: number, payload?: unknown): void {
    if (epoch !== this.epoch) return;
    if (!payloadMatchesProject(payload, this.projectId)) return;
    if (this.fetching) {
      this.pending = true;
      return;
    }
    void this.fetch(epoch);
  }

  private async fetch(epoch: number): Promise<void> {
    if (epoch !== this.epoch) return;
    const root = this.root.trim();
    if (!root) {
      this.onChange([]);
      return;
    }
    this.fetching = true;
    try {
      const findings = await this.invoke("censor_get_findings", { root });
      if (epoch !== this.epoch) return; // superseded: drop this result
      this.onChange(Array.isArray(findings) ? findings : []);
    } catch (e) {
      if (epoch !== this.epoch) return;
      this.onError?.(e instanceof Error ? e.message : "Could not load Censor findings.");
    } finally {
      if (epoch === this.epoch) {
        this.fetching = false;
        if (this.pending) {
          this.pending = false;
          void this.fetch(epoch);
        }
      }
    }
  }
}
