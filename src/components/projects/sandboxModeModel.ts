// sandboxModeModel — pure model for the per-project sandbox-mode selector (Slice 1).
//
// Keeps all non-UI logic (type re-exports, arg builders, pure helpers) in a
// separate, vitest-friendly module — the same split used by netConsentModel.ts
// and censorPanelModel.ts.

import type { SandboxMode } from "../../types/backend";

// ─────────────────────────────────────────────────────────────
// Re-export the canonical type so component imports stay local
// ─────────────────────────────────────────────────────────────

export type { SandboxMode };

// ─────────────────────────────────────────────────────────────
// Static descriptor table
// ─────────────────────────────────────────────────────────────

export interface SandboxModeDescriptor {
  value: SandboxMode;
  label: string;
  description: string;
}

/**
 * Ordered list of sandbox mode descriptors for rendering a selector UI.
 * Order: least autonomous → most autonomous.
 */
export const SANDBOX_MODES: readonly SandboxModeDescriptor[] = [
  {
    value: "ask",
    label: "Ask",
    description: "Prompt before network access and out-of-workspace writes.",
  },
  {
    value: "autoAcceptInWorkspace",
    label: "Auto-accept in workspace",
    description:
      "Auto-allow writes inside the project; still prompt for network and new folders.",
  },
  {
    value: "unattended",
    label: "Unattended (fail-closed)",
    description: "Never prompt; anything not already granted is denied.",
  },
] as const;

// ─────────────────────────────────────────────────────────────
// Pure helpers
// ─────────────────────────────────────────────────────────────

/**
 * Normalise a potentially-absent sandbox mode to the effective value.
 *
 * The backend skips serializing `sandboxMode` when it equals the default
 * ("ask"), so `undefined` MUST be treated as "ask" on the frontend.
 */
export function effectiveSandboxMode(m: SandboxMode | undefined): SandboxMode {
  return m ?? "ask";
}

/**
 * Pure guard: should the prop-sync effect adopt an incoming prop value?
 *
 * Rules (applied in order):
 * 1. When a write is in-flight (`busy`), never clobber the optimistic value.
 * 2. When a confirmed write is pending (pendingMode set), only adopt the prop
 *    once it has caught up to that confirmed value — i.e. `incoming ===
 *    pendingMode`. Any other incoming value is a stale prop arriving before
 *    the parent has refreshed; discard it to prevent the UI from snapping back.
 * 3. With no pending value and not busy, always adopt the prop (external
 *    refresh path).
 *
 * @param incoming     The effective value derived from the new prop.
 * @param pendingMode  The mode confirmed by the last successful write, or null
 *                     if no write is pending parent acknowledgement.
 * @param busy         Whether an IPC call is currently in flight.
 */
export function shouldAdoptProp(
  incoming: SandboxMode,
  pendingMode: SandboxMode | null,
  busy: boolean,
): boolean {
  if (busy) return false;
  if (pendingMode !== null) return incoming === pendingMode;
  return true;
}

/**
 * Build the args object for
 * `invokeBackendCommand("set_project_sandbox_mode_cmd", ...)`.
 *
 * The Tauri JS bridge passes camelCase keys; the Rust `#[tauri::command]`
 * handler receives them as snake_case (`project_id`, `mode`).  Passing
 * camelCase here is the correct and documented pattern (see CensorPanel.tsx
 * passing `projectId` to `set_censor_trusted`).
 */
export function setSandboxModeArgs(
  projectId: string,
  mode: SandboxMode,
): Record<string, unknown> {
  return { projectId, mode };
}
