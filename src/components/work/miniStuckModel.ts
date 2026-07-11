/**
 * Mini-stuck report model.
 *
 * Mirrors the backend `StuckReport` (stuck_report.rs), serialized as camelCase
 * over the `mini://stuck` Tauri event channel. Before this module existed, a stuck
 * mini silently vanished with no UI signal — the human had no way to know a directive
 * timed out or failed. This model + the companion hook + banner component surface
 * that information so the user can act on it.
 */

/** Wire shape emitted by the Rust backend on `mini://stuck`. */
export interface MiniStuckReport {
  taskId: string;
  agentId: string;
  reason: string;
  attempts: number;
  lastOutput: string;
  filesTouched: string[];
}

/** A short, human label for the banner row, e.g. "timeout" -> "timed out". */
export function stuckReasonLabel(reason: string): string {
  switch (reason) {
    case "timeout":
      return "timed out";
    case "failed":
      return "failed";
    default:
      return reason || "stuck";
  }
}
