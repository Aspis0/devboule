import type { ProjectTask } from "../../../types/backend";
import type { DesignProjectEntry } from "../../../types/design";
import type {
  ConsoleEntry,
  QuestionEntry,
  QuestionOption,
} from "../../agents/agentConsoleModel";

export type StageView = 'exa' | 'plan' | 'design';

export type PlanCardState = 'done' | 'forming' | 'pending' | 'skipped';

export interface PlanCard {
  n: number;
  title: string;
  state: PlanCardState;
}

export interface StagePage {
  url: string;
  title: string;
  summary: string;
}

export interface StageFinding {
  text: string;
  task?: number;
}

export type ChatRole = 'user' | 'assistant';

export interface PlannerMessage {
  /** 'milestone' = a tiny system ROW (tool use / activity), not a chat bubble. */
  role: ChatRole | 'milestone';
  text: string;
  /** B14b: this bubble is the live, in-progress reply being streamed token-by-token (render a
   *  caret / "typing" affordance). Absent/false for finalized turns. */
  streaming?: boolean;
  /** D3: the client-generated send id (user rows only) — the echo carries it back
   *  through the bridge and the pending copy drains by identity. */
  msgId?: string;
  /** Optional hover detail (e.g. full list of steps folded into a compressed milestone). */
  title?: string;
}

/** Compress a consecutive run of milestones when longer than this (inclusive tail kept). */
export const MILESTONE_RUN_COMPRESS_AFTER = 4;
/** How many of the newest milestones in a long run stay expanded. */
export const MILESTONE_RUN_KEEP_TAIL = 3;

/** D3: one optimistic user send awaiting its bridge echo. `msgId` is the
 *  client-generated identity the echo carries back. */
export interface PendingSend {
  text: string;
  msgId: string;
}

/** D3 (planner-chat demolition): merge the bridge's REAL conversation with the
 *  optimistic pending sends — BY IDENTITY, not by counting user rows (the old
 *  `echoedUserCount` watermark broke on every restart/relaunch/generation change).
 *
 *  A pending is DRAINED (already visible in `real`, so not re-appended) when:
 *  - a real user row carries its exact `msgId` (the app-written cloud echo), or
 *  - an ID-LESS real user row matches its text — each id-less row consumes exactly
 *    ONE pending, oldest first (the local binary echoes steers without a msgId).
 *  Everything still pending rides at the END in send order. Pure + total. */
export function mergePendingSends(
  real: PlannerMessage[],
  pending: PendingSend[],
): PlannerMessage[] {
  const still = drainPendingSends(real, pending);
  return still.length === 0
    ? real
    : [...real, ...still.map((p) => ({ role: 'user' as const, text: p.text, msgId: p.msgId }))];
}

/** D3: the subset of `pending` the bridge has NOT echoed yet (the drain rules of
 *  [mergePendingSends]). Exposed separately so the view can also garbage-collect its
 *  pending state once echoes land (the merge alone would just re-hide them forever). */
export function drainPendingSends(
  real: PlannerMessage[],
  pending: PendingSend[],
): PendingSend[] {
  if (pending.length === 0) return pending;
  const echoedIds = new Set<string>();
  const idlessTexts = new Map<string, number>();
  for (const m of real) {
    if (m.role !== 'user') continue;
    if (m.msgId) echoedIds.add(m.msgId);
    else idlessTexts.set(m.text, (idlessTexts.get(m.text) ?? 0) + 1);
  }
  const still: PendingSend[] = [];
  for (const p of pending) {
    if (echoedIds.has(p.msgId)) continue;
    const idless = idlessTexts.get(p.text) ?? 0;
    if (idless > 0) {
      idlessTexts.set(p.text, idless - 1);
      continue;
    }
    still.push(p);
  }
  return still;
}

/** D3: guarded message-id generator (the repo's defaultNewId pattern —
 *  useDesignStream.ts): crypto.randomUUID when available, a time+random fallback
 *  otherwise. A send must never throw synchronously inside a click/keydown handler. */
export function newMsgId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return `msg-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

/** D1: the STABLE planner-orchestrator agent id for a project — the frontend mirror
 *  of Rust `stable_orchestrator_agent_id` (projects.rs). MUST stay byte-identical:
 *  charset `[A-Za-z0-9._-]` (everything else → `_`), project id capped at 100 chars.
 *  Stable ⇒ the planner console binds the moment a project opens (live session or
 *  not) and the transcript survives relaunches/restarts/backend switches. */
export function stableOrchestratorAgentId(projectId: string): string {
  const clean = Array.from(projectId)
    .slice(0, 100)
    .map((c) => (/[A-Za-z0-9._-]/.test(c) ? c : '_'))
    .join('');
  return `orchestrator-${clean}`;
}

export interface StatusPill {
  text: string;
}

/** Maps backend tasks to plan cards with derived states and titles. */
export function derivePlanCards(tasks: ProjectTask[]): PlanCard[] {
  return tasks.map((task, index) => {
    const n = index + 1;
    const state: PlanCardState =
      task.status === 'done'
        ? 'done'
        : task.status === 'wip'
          ? 'forming'
          : 'pending';
    const title = task.title.trim() || `Task ${n}`;
    return { n, title, state };
  });
}

/** Extracts the hostname from a URL string, handling missing protocols and invalid inputs. */
export function pageHostname(url: string): string {
  try {
    const parseable = url.startsWith('http') ? url : `https://${url}`;
    return new URL(parseable).hostname;
  } catch {
    return url.trim();
  }
}

/**
 * Selects the most-recently opened design entry matching the given project root path.
 * Matches a design at the root or in a folder UNDER it (exact path or `root + '/'`
 * prefix — never a bare sibling like '/proj2' for root '/proj'). Pure + total.
 */
export function pickProjectDesign(
  entries: DesignProjectEntry[],
  rootPath: string | null,
): DesignProjectEntry | null {
  if (rootPath == null || rootPath === "" || entries.length === 0) {
    return null;
  }

  let best: DesignProjectEntry | null = null;
  const prefix = rootPath + "/";

  for (const entry of entries) {
    if (
      entry.workingFolderPath === rootPath ||
      entry.workingFolderPath.startsWith(prefix)
    ) {
      if (best === null || entry.lastOpenedAt > best.lastOpenedAt) {
        best = entry;
      }
    }
  }

  return best;
}

/** Short tool name from a milestone line for compressed-run previews. */
export function milestoneToolLabel(text: string): string {
  const tick = text.match(/`([^`]+)`/);
  if (tick?.[1]) {
    return tick[1]
      .replace(/^mcp_devboule_/, "")
      .replace(/^mcp_/, "");
  }
  const cleaned = text.replace(/^[🔧✅❌⏳·\s]+/u, "").trim();
  return cleaned.length > 48 ? `${cleaned.slice(0, 45)}…` : cleaned;
}

/**
 * Collapse long consecutive milestone runs so tool spam does not fill the chat.
 * Runs of length ≤ {@link MILESTONE_RUN_COMPRESS_AFTER} stay expanded. Longer runs
 * keep the last {@link MILESTONE_RUN_KEEP_TAIL} rows and fold the rest into one
 * summary row (full text in `title` for hover).
 */
export function compressMilestoneRuns(messages: PlannerMessage[]): PlannerMessage[] {
  const out: PlannerMessage[] = [];
  let i = 0;
  while (i < messages.length) {
    const msg = messages[i];
    if (msg.role !== "milestone") {
      out.push(msg);
      i += 1;
      continue;
    }
    const start = i;
    while (i < messages.length && messages[i].role === "milestone") i += 1;
    const run = messages.slice(start, i);
    if (run.length <= MILESTONE_RUN_COMPRESS_AFTER) {
      out.push(...run);
      continue;
    }
    const head = run.slice(0, run.length - MILESTONE_RUN_KEEP_TAIL);
    const tail = run.slice(-MILESTONE_RUN_KEEP_TAIL);
    const labels = head.map((m) => milestoneToolLabel(m.text)).filter(Boolean);
    const unique: string[] = [];
    for (const label of labels) {
      if (!unique.includes(label)) unique.push(label);
      if (unique.length >= 4) break;
    }
    const preview = unique.length > 0 ? ` · ${unique.join(", ")}` : "";
    out.push({
      role: "milestone",
      text: `… ${head.length} earlier tool steps${preview}`,
      title: head.map((m) => m.text).join("\n"),
    });
    out.push(...tail);
  }
  return out;
}

/** Map the orchestrator's console entries to planner chat bubbles, interleaving
 *  coder milestones (tool use: Bash, reads, spawns…) as tiny 'milestone' rows, in
 *  timeline order. The milestone rows are the planner chat's visibility into WHAT
 *  the orchestrator is doing between replies — without them a cloud orchestrator
 *  running tools looks identical to one that hung ("thinking" forever).
 *
 *  role:"plan" chat entries are skipped: they are structured data for the Plan stage
 *  (consumed via `latestPlan` / `planCardsFromPiPlan`), never a chat bubble.
 *
 *  Long consecutive tool-call runs are compressed (see {@link compressMilestoneRuns}). */
export function chatMessagesWithMilestones(
  entries: ConsoleEntry[] | undefined,
): PlannerMessage[] {
  const out: PlannerMessage[] = [];
  for (const e of entries ?? []) {
    if (e.type === "chat") {
      // role:"plan" entries are structured plan payloads for the Plan stage, not
      // conversation — never render them as a chat bubble.
      if (e.role === "plan") continue;
      // never render a blank chat bubble
      if (e.text.trim().length === 0) continue;
      out.push(
        // D3: thread the echoed msgId through so `mergePendingSends` can drain
        // the optimistic copy by identity (omit the key entirely when absent).
        e.msgId
          ? { role: e.role, text: e.text, msgId: e.msgId }
          : { role: e.role, text: e.text },
      );
    } else if (e.type === "coder" || e.type === "spawn") {
      const text = e.text;
      // Tool-call `args: {...}` lines are noise at chat altitude: fold them into
      // the preceding "Calling `x`" milestone as a short preview instead of their
      // own 10px row — a long tool loop otherwise floods the chat with an
      // unreadable wall of arg dumps (live e2e finding 2026-07-16). The row's
      // tooltip (title={msg.text}) still carries the merged text in full.
      const prev = out[out.length - 1];
      if (/^args:\s*/.test(text.trim()) && prev?.role === "milestone") {
        const args = text.trim().replace(/^args:\s*/, "");
        const preview = args.length > 80 ? `${args.slice(0, 77)}…` : args;
        prev.text = `${prev.text} · ${preview}`;
        continue;
      }
      // skip blank milestone rows
      if (text.trim().length === 0) continue;
      out.push({ role: "milestone", text });
    }
  }
  return compressMilestoneRuns(out);
}

/** Extract the orchestrator's OPEN Kairion doubts from a console timeline, upserted by
 *  `id` so a later `reopened` (or refreshed) event replaces the earlier one IN PLACE while
 *  keeping first-seen order (stable card positions). Empty when the orchestrator surfaced no
 *  doubt — Kairion degrades to a plain question / no left panel. ORCHESTRATOR-ONLY: this only
 *  reads `question` entries, which the contract emits for the orchestrator alone. Pure + total. */
export function openQuestions(entries: ConsoleEntry[] | undefined): QuestionEntry[] {
  if (!entries) return [];
  const order: string[] = [];
  const byId = new Map<string, QuestionEntry>();
  for (const e of entries) {
    if (e.type !== "question") continue;
    if (!byId.has(e.id)) order.push(e.id);
    byId.set(e.id, e);
  }
  return order.map((id) => byId.get(id)!);
}

/** The plain steer line a picked option rides — the SAME transport as a typed reply
 *  (orchestrator_steer / project_cloud_orchestrator_send). Plain words the model already
 *  parses from an ask_user reply; no new command, no sugar required. Pure + total. */
export function steerPickOption(question: QuestionEntry, option: QuestionOption): string {
  return `For "${question.text}" — go with ${option.label}.`;
}

/** The orchestrator's `plan` tool payload (the `devboule_plan` wire contract).
 *  `status` is always normalized by the sidecar to one of the four values below;
 *  `notes` is omitted from the payload when empty. Pure + total. */
export type PiPlanStepStatus = 'pending' | 'in_progress' | 'done' | 'skipped';
export interface PiPlanStep {
	text: string;
	status: PiPlanStepStatus;
}
export interface PiPlan {
	title: string;
	steps: PiPlanStep[];
	notes?: string;
}

/** Scan a console timeline BACKWARDS for the newest role:"plan" chat entry (last call
 *  wins — the tool contract). JSON.parse(e.text) inside try/catch; a malformed payload
 *  or a shape that fails validation (title not a non-empty string, steps not an array)
 *  returns null — a newer corrupt payload must NOT resurrect a stale plan. Normalizes
 *  each step: non-string/empty text → drop; status outside the 4 values → 'pending'.
 *  `notes` only when a non-empty string. Pure + total, never throws. */
export function latestPlan(entries: ConsoleEntry[] | undefined): PiPlan | null {
	if (!entries) return null;
	for (let i = entries.length - 1; i >= 0; i--) {
		const e = entries[i];
		if (e.type !== 'chat' || e.role !== 'plan') continue;
		let parsed: unknown;
		try {
			parsed = JSON.parse(e.text);
		} catch {
			return null;
		}
		if (
			typeof parsed !== 'object' ||
			parsed === null ||
			Array.isArray(parsed)
		) {
			return null;
		}
		const p = parsed as Record<string, unknown>;
		const title = p['title'];
		const steps = p['steps'];
		if (typeof title !== 'string' || title.trim().length === 0) return null;
		if (!Array.isArray(steps)) return null;
		const normalizedSteps: PiPlanStep[] = [];
		for (const raw of steps as unknown[]) {
			if (
				typeof raw !== 'object' ||
				raw === null ||
				Array.isArray(raw)
			) {
				continue;
			}
			const s = raw as Record<string, unknown>;
			const text = s['text'];
			if (typeof text !== 'string' || text.trim().length === 0) continue;
			const status = s['status'];
			const statusNorm:
				| 'pending'
				| 'in_progress'
				| 'done'
				| 'skipped' =
				status === 'pending'
					? 'pending'
					: status === 'in_progress'
						? 'in_progress'
						: status === 'done'
							? 'done'
							: status === 'skipped'
								? 'skipped'
								: 'pending';
			normalizedSteps.push({ text, status: statusNorm });
		}
		const notes = p['notes'];
		const out: PiPlan = {
			title: title.trim(),
			steps: normalizedSteps,
		};
		if (typeof notes === 'string' && notes.trim().length > 0) {
			out.notes = notes.trim();
		}
		return out;
	}
	return null;
}

/** Turn a pi plan (title + steps) into PlanCards for the Plan stage. Steps are 1-based;
 *  status 'done'→'done', 'in_progress'→'forming', 'pending'→'pending', 'skipped'→'skipped'.
 *  A plan with no steps yields []. Pure + total. */
export function planCardsFromPiPlan(plan: PiPlan): PlanCard[] {
	return plan.steps.map((step, index) => {
		const n = index + 1;
		const state: PlanCardState =
			step.status === 'done'
				? 'done'
				: step.status === 'in_progress'
					? 'forming'
					: step.status === 'skipped'
						? 'skipped'
						: 'pending';
		return { n, title: step.text, state };
	});
}

/** The plain steer line "you decide" rides: hand the fork back to the orchestrator to
 *  resolve on its own lean. Same transport as a pick. Pure + total. */
export function steerYouDecide(question: QuestionEntry): string {
  const dir = question.lean ? ` (your lean — ${question.lean})` : "";
  return `For "${question.text}" — you decide${dir}.`;
}

/** Whether a doubt's `affects[]` touches a given plan card — by exact task title (trimmed,
 *  case-insensitive) OR by 1-based task number. Drives the doubt<->task hover link. Pure + total. */
export function doubtTouchesCard(affects: string[], card: PlanCard): boolean {
  const title = card.title.trim().toLowerCase();
  for (const a of affects) {
    const key = a.trim().toLowerCase();
    if (key.length === 0) continue;
    if (key === title) return true;
    if (key === String(card.n)) return true;
  }
  return false;
}

export interface PlannerWeb {
  pages: StagePage[];
  findings: StageFinding[];
}

/** Extract the LATEST websearch row's real pages + derived findings from a console
 *  timeline (the orchestrator's `useAgentConsole` entries). Findings are the per-page
 *  summaries. Empty when the orchestrator hasn't searched yet. Pure + total. */
export function latestWeb(entries: ConsoleEntry[] | undefined): PlannerWeb {
  if (!entries) return { pages: [], findings: [] };
  for (let i = entries.length - 1; i >= 0; i--) {
    const e = entries[i];
    if (e.type === 'webSearch') {
      const pages: StagePage[] = e.pages.map((p) => ({
        url: p.url,
        title: p.title,
        summary: p.summary,
      }));
      const findings: StageFinding[] = pages
        .filter((p) => p.summary.trim().length > 0)
        .map((p) => ({ text: p.summary }));
      return { pages, findings };
    }
  }
  return { pages: [], findings: [] };
}

/** Returns the human-readable label for a given stage view. */
export function stripLabel(view: StageView): 'searching' | 'planning' | 'designing' {
  switch (view) {
    case 'exa':
      return 'searching';
    case 'plan':
      return 'planning';
    case 'design':
      return 'designing';
  }
}

/**
 * F48: when open doubts arrive (or are already open on first observation), the stage
 * must show the plan view — DoubtPanel only mounts under `view === "plan"`.
 *
 * - question arrival (`len > prevLen`) → `'plan'`
 * - mount / first-obs with open questions (`prevLen === 0 && len > 0`) → `'plan'`
 *   (callers init the prev ref to `0` so remount-with-open-doubts is covered)
 * - no change → `null` (leave the current view alone)
 *
 * `currentView` is unused for the decision (kept for call-site clarity / future).
 */
export function stageViewOnDoubts(
  prevLen: number,
  len: number,
  _currentView: StageView,
): "plan" | null {
  if (len > prevLen) return "plan";
  return null;
}
