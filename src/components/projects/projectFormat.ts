import type { ProjectGitStatus, GithubConnectionStatus, GithubRepoAccessStatus } from "../../types/backend";

// Shared formatting/tone helpers for the Projects view and its extracted
// presentational sub-components. These were lifted verbatim from ProjectsView
// so both the macro board and the detail panel render identical labels/tones.

export function fileName(path: string) {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

export function formatDate(value: string | null | undefined) {
  if (!value) return "no date";
  return value.slice(0, 10);
}

// How long ago a project was last updated, in coarse buckets.
const RELATIVE_THRESHOLD_DAYS = 30;

function differenceMs(iso: string, now: Date): number {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return 0;
  return now.getTime() - then;
}

function fmt(diff: number, now: Date): string {
  // Negative diff (a future timestamp) is treated as "just now" — never a
  // negative bucket.
  if (diff < 0) return "just now";
  if (diff < 60_000) return "just now";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
  if (diff < RELATIVE_THRESHOLD_DAYS * 86_400_000) {
    return `${Math.floor(diff / 86_400_000)}d ago`;
  }
  // Beyond the threshold: fall back to the existing absolute short date so we
  // don't invent a different long-format renderer.
  return formatDate(new Date(now.getTime() - diff).toISOString());
}

/** Coarse relative-time label for an ISO timestamp. `now` is injectable so the
 *  result is deterministic in tests; defaults to the current time. Buckets:
 *  seconds→"just now", minutes→"Xm ago", hours→"Xh ago", days→"Xd ago";
 *  beyond `RELATIVE_THRESHOLD_DAYS` it falls back to the absolute short date. */
export function relativeTime(iso: string, now: Date = new Date()): string {
  return fmt(differenceMs(iso, now), now);
}

export function formatDateTime(value: string | null | undefined) {
  if (!value) return "never";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value.slice(0, 19);
  return date.toLocaleString(undefined, {
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function gitPolicyTone(status: string) {
  if (status === "ready") return "bg-sage/10 text-sage-dark";
  if (status === "warning") return "bg-amber/10 text-amber-dark";
  return "bg-coral/10 text-coral-dark";
}

export function gitPolicyLabel(status: string) {
  if (status === "ready") return "GitHub ready";
  if (status === "warning") return "Needs Git review";
  return "Git blocked";
}

export function gitRepoLabel(git: ProjectGitStatus) {
  return git.repoName ?? (git.rootPath ? fileName(git.rootPath) : "No repo");
}

export function branchLabel(git: ProjectGitStatus) {
  if (!git.branch) return "no branch";
  const drift = [
    git.aheadCount > 0 ? `ahead ${git.aheadCount}` : null,
    git.behindCount > 0 ? `behind ${git.behindCount}` : null,
  ].filter(Boolean);
  return drift.length ? `${git.branch} / ${drift.join(" / ")}` : git.branch;
}

export function githubAuthTone(status: string | null | undefined) {
  if (status === "valid") return "bg-sage/10 text-sage-dark";
  if (status === "error") return "bg-coral/10 text-coral-dark";
  return "bg-amber/10 text-amber-dark";
}

export function githubAuthLabel(status: GithubConnectionStatus | null) {
  if (!status) return "Checking auth";
  if (status.status === "valid")
    return `Connected${status.login ? ` as ${status.login}` : ""}`;
  if (status.status === "error") return "Auth needs fix";
  return "Not connected";
}

export function repoAccessTone(access: GithubRepoAccessStatus | null) {
  if (access?.accessible) return "bg-sage/10 text-sage-dark";
  if (access?.status === "not_accessible" || access?.status === "error")
    return "bg-coral/10 text-coral-dark";
  return "bg-cream-100 text-cream-500";
}

export function repoAccessLabel(access: GithubRepoAccessStatus | null) {
  if (!access) return "Repo access unknown";
  if (access.accessible)
    return access.private ? "Private repo access OK" : "Public repo access OK";
  if (access.status === "not_accessible") return "Repo not accessible";
  if (access.status === "invalid") return "Invalid repo URL";
  return "Repo check failed";
}

export function githubRepoUrl(git: ProjectGitStatus) {
  return git.githubUrl ?? null;
}

export function githubRepoSubpage(git: ProjectGitStatus, path: string) {
  const base = githubRepoUrl(git);
  return base ? `${base}/${path}` : null;
}
