// netConsentModel — pure model for the net-consent broker (Slice 0).
//
// Keeps all non-UI logic (type aliases, arg builders, filtering) in a
// separate, vitest-friendly module — the same split used by censorPanelModel.ts.

// ─────────────────────────────────────────────────────────────
// Wire types (mirrors src-tauri/src/backend/broker/mod.rs)
// ─────────────────────────────────────────────────────────────

/**
 * Category of permission the agent is requesting. Slice 0 handles Net only;
 * later slices add FolderWrite / Exec / Patch via the same listener.
 *
 * Serde on the Rust side: `rename_all = "camelCase"` on the enum, so
 * Net → "net", FolderWrite → "folderWrite", etc.
 */
export type ConsentKind = "net" | "folderWrite" | "exec" | "patch";

/**
 * Payload of the `sandbox://consent-request` Tauri event.
 * All fields are camelCase (Rust struct has `rename_all = "camelCase"`).
 */
export interface ConsentRequest {
  kind: ConsentKind;
  projectId: string;
  agentId: string;
  /**
   * Human-readable context displayed to the user (e.g. "A sandboxed command attempted
   * to write outside the project to ..."). Never raw secrets.
   *
   * IMPORTANT: do NOT pass `detail` as the `folder` argument to `grant_folder_consent`.
   * It is display-only prose and will be rejected by the backend's path validator.
   * Use `path` (the machine-readable field) for any backend call.
   */
  detail: string;
  /**
   * Machine-readable absolute path for FolderWrite consent requests.
   * Contains the canonical folder path that triggered the block — pass this
   * (not `detail`) to `grant_folder_consent` as the `folder` argument.
   * Absent (`undefined`) for Net and other kinds that have no associated path.
   */
  path?: string;
  /**
   * Correlation id for LIVE cloud-agent requests (Slice 5, Exec/Patch from Claude/Codex).
   * When present, the decision must be answered via `respond_cloud_consent` (which
   * round-trips it back to the blocked agent) — NOT the fire-and-forget grant_* commands.
   * Format `"<agentId>:<requestId>"`. Absent for the local seatbelt path (Net/FolderWrite).
   */
  approvalId?: string;
}

/**
 * Decision sent back to `grant_net_consent`.
 *
 * Rust enum `ConsentDecision` has `rename_all = "camelCase"`:
 *   AllowRemember → "allowRemember"
 *   AllowOnce     → "allowOnce"
 *   Deny          → "deny"
 */
export type ConsentDecision = "allowRemember" | "allowOnce" | "deny";

// ─────────────────────────────────────────────────────────────
// Arg builders (pure, no side-effects, easily tested)
// ─────────────────────────────────────────────────────────────

/**
 * Build the args object for `invokeBackendCommand("grant_net_consent", ...)`.
 *
 * The Tauri JS bridge passes camelCase keys; the Rust `#[tauri::command]`
 * handler receives them as snake_case (`project_id`, `decision`). Passing
 * camelCase here is the correct and documented pattern (see CensorPanel.tsx
 * passing `projectId` to `set_censor_trusted`).
 */
export function grantNetConsentArgs(params: {
  projectId: string;
  decision: ConsentDecision;
}): Record<string, unknown> {
  return { projectId: params.projectId, decision: params.decision };
}

/**
 * Build the args object for `invokeBackendCommand("grant_folder_consent", ...)`.
 *
 * Matches the `grant_folder_consent` Tauri command signature:
 *   `project_id: String, folder: String, decision: ConsentDecision`
 * All keys camelCase per the Tauri IPC bridge convention.
 */
export function grantFolderConsentArgs(params: {
  projectId: string;
  folder: string;
  decision: ConsentDecision;
}): Record<string, unknown> {
  return {
    projectId: params.projectId,
    folder: params.folder,
    decision: params.decision,
  };
}

/**
 * Build the args object for `invokeBackendCommand("respond_cloud_consent", ...)`.
 *
 * Slice 5: used for LIVE cloud-agent (Claude/Codex) Exec/Patch requests that carry an
 * `approvalId`. The decision round-trips back to the blocked agent (vs the grant_*
 * commands which only persist for the next spawn). Matches the Tauri command signature:
 *   `approval_id: String, decision: ConsentDecision`  (camelCase over the JS bridge).
 */
export function respondCloudConsentArgs(params: {
  approvalId: string;
  decision: ConsentDecision;
}): Record<string, unknown> {
  return { approvalId: params.approvalId, decision: params.decision };
}

// ─────────────────────────────────────────────────────────────
// Slice 5b: Claude consent file-bridge (mirrors ConsentBridgeRequest in
// src-tauri/src/backend/consent_bridge.rs)
// ─────────────────────────────────────────────────────────────

/** Lifecycle of a Claude file-bridge consent request (snake_case over the wire). */
export type ConsentBridgeStatus =
  | "pending_approval"
  | "allowed"
  | "denied"
  | "timeout"
  // A newer ask for the same (project, kind, path) replaced this row
  // (append_superseding, consent_bridge.rs) — terminal, never rendered as
  // pending. Present on the wire since the executor write-through landed.
  | "superseded";

/**
 * One Claude consent request in the `.aspis-agents.json` `consentRequests` queue. Written by
 * the `claude_consent_hook` binary (a PreToolUse hook), polled by the frontend via
 * `consent_requests_list`, stamped terminal by `respond_cloud_consent`.
 */
export interface ConsentBridgeRequest {
  id: string;
  agentId: string;
  projectId: string;
  kind: ConsentKind;
  detail: string;
  path?: string;
  status: ConsentBridgeStatus;
  createdAt: string;
}

/**
 * Map the `pending_approval` file-bridge requests for `projectId` into the SAME `ConsentRequest`
 * shape the event-driven modal uses, carrying the file-bridge id as `approvalId` so a decision
 * round-trips via `respond_cloud_consent`. Terminal (allowed/denied/timeout) entries are dropped.
 */
export function pendingConsentBridgeForProject(
  all: ConsentBridgeRequest[],
  projectId: string,
): ConsentRequest[] {
  return all
    .filter((r) => r.status === "pending_approval" && r.projectId === projectId)
    .map((r) => ({
      kind: r.kind,
      projectId: r.projectId,
      agentId: r.agentId,
      detail: r.detail,
      path: r.path,
      approvalId: r.id,
    }));
}

// ─────────────────────────────────────────────────────────────
// Filtering helpers (pure, tested)
// ─────────────────────────────────────────────────────────────

/**
 * Returns true if this request is a Net kind and belongs to the given project.
 * Used by the listener in ProjectWorkspace to ignore irrelevant events.
 */
export function isNetRequestForProject(
  request: ConsentRequest,
  projectId: string,
): boolean {
  return request.kind === "net" && request.projectId === projectId;
}

/**
 * Returns true when the request belongs to the given project, regardless of kind.
 * Used by the Slice 2 listener extension in ProjectWorkspace to accept BOTH
 * `Net` and `FolderWrite` (and any future kinds) in a single subscription.
 * Earlier, kind-specific call sites keep using `isNetRequestForProject` directly.
 */
export function isConsentRequestForProject(
  request: ConsentRequest,
  projectId: string,
): boolean {
  return request.projectId === projectId;
}

// ─────────────────────────────────────────────────────────────
// FIFO queue helpers (pure, tested)
// ─────────────────────────────────────────────────────────────

/**
 * Returns true when two ConsentRequests have the same logical identity:
 * same projectId, agentId, AND kind. Used to deduplicate rapid duplicate events
 * from the Tauri backend before they enter the pending queue.
 *
 * Kind is included because an agent can legitimately be blocked on BOTH a net
 * AND a folderWrite request simultaneously — treating them as the same identity
 * would silently drop the second request and leave the agent permanently stuck.
 */
export function sameConsentRequest(a: ConsentRequest, b: ConsentRequest): boolean {
  // Slice 5: cloud requests carry an approvalId. A single agent can be blocked on
  // several DISTINCT live requests over its lifetime (e.g. two sequential Exec
  // approvals), so when an approvalId is present it is part of the identity —
  // otherwise the second request would be silently dropped and the agent would hang.
  // When BOTH lack an approvalId (local seatbelt path) we keep the original
  // project+agent+kind identity so rapid duplicate net/folder events still dedupe.
  if (a.approvalId !== undefined || b.approvalId !== undefined) {
    return (
      a.projectId === b.projectId &&
      a.agentId === b.agentId &&
      a.kind === b.kind &&
      a.approvalId === b.approvalId
    );
  }
  return a.projectId === b.projectId && a.agentId === b.agentId && a.kind === b.kind;
}

/**
 * Append `req` to `list` only if no entry with the same identity already exists.
 * Returns the original array unchanged when the request is already queued, so
 * React can skip unnecessary re-renders (referential stability on no-op).
 */
export function enqueueConsent(
  list: ConsentRequest[],
  req: ConsentRequest,
): ConsentRequest[] {
  if (list.some((r) => sameConsentRequest(r, req))) return list;
  return [...list, req];
}
