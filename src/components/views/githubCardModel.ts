import type { GithubConnectionStatus } from "../../types/backend";

// Pure presentation/logic helpers for the GitHub provider card. Extracted so the
// status -> pill mapping and the "show import button" gate are unit-testable in
// this repo's node-env vitest (no jsdom, no events). The .tsx wires these to the
// write-only token input + the github.rs Tauri commands.

export type GithubPillTone = "valid" | "error" | "missing" | "checking";

export interface GithubCardPill {
  tone: GithubPillTone;
  label: string;
}

/**
 * Map a GithubConnectionStatus into the status pill (label + tone bucket).
 * Reuses the same wording idiom as projectFormat's githubAuthLabel so the
 * Settings card and the Projects panel stay consistent.
 *  - null status  -> "Checking auth" (initial load, before first fetch)
 *  - "valid"      -> "Connected[ as <login>]"
 *  - "error"      -> "Auth needs fix"
 *  - anything else (incl. "missing") -> "Not connected"
 */
export function githubCardPill(status: GithubConnectionStatus | null): GithubCardPill {
  if (!status) return { tone: "checking", label: "Checking auth" };
  if (status.status === "valid") {
    return {
      tone: "valid",
      label: `Connected${status.login ? ` as ${status.login}` : ""}`,
    };
  }
  if (status.status === "error") return { tone: "error", label: "Auth needs fix" };
  return { tone: "missing", label: "Not connected" };
}

/**
 * The "Import from GitHub CLI" button is only meaningful when the `gh` CLI is
 * available on this machine. Before the first status load (status null) we
 * hide it — we don't yet know whether the CLI exists, and offering it would
 * surface a guaranteed failure.
 */
export function shouldShowGithubImportButton(
  status: GithubConnectionStatus | null,
): boolean {
  return status?.cliAvailable === true;
}

/**
 * Disconnect/Remove is only meaningful once a token is actually stored. The
 * backend marks `configured` true whenever a token sits in the vault, even if
 * it later turned invalid (so the user can clear a broken token).
 */
export function shouldShowGithubRemoveButton(
  status: GithubConnectionStatus | null,
): boolean {
  return status?.configured === true;
}

// The four IPC actions, pulled out of the .tsx so the exact command name +
// args are unit-testable in node-env vitest (the repo has no jsdom, so we can't
// fire click events). The component injects `invokeBackendCommand`; tests
// inject a mock. The TOKEN is only ever passed INTO save_github_token — it is
// never returned to the UI by any of these.
export type GithubInvoke = <T>(
  command: string,
  args?: Record<string, unknown>,
) => Promise<T>;

export function loadGithubStatus(invoke: GithubInvoke): Promise<GithubConnectionStatus> {
  return invoke<GithubConnectionStatus>("get_github_connection_status");
}

export function saveGithubToken(
  invoke: GithubInvoke,
  token: string,
): Promise<GithubConnectionStatus> {
  // Pass the token through verbatim; the backend trims + validates it. Use the
  // `token` arg key exactly (camelCase IPC parity with the Rust command).
  return invoke<GithubConnectionStatus>("save_github_token", { token });
}

export function importGithubTokenFromCli(
  invoke: GithubInvoke,
): Promise<GithubConnectionStatus> {
  return invoke<GithubConnectionStatus>("import_github_token_from_cli");
}

export function deleteGithubToken(invoke: GithubInvoke): Promise<GithubConnectionStatus> {
  return invoke<GithubConnectionStatus>("delete_github_token");
}
