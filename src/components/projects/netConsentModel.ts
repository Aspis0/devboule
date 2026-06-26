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
  /** Human-readable context (e.g. the command that hit the block). Never raw secrets. */
  detail: string;
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
