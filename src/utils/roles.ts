import type { Role } from "../types/backend";

// Views ONLY the admin may open. A collaborator works on everything else —
// including Cloudflare/Scaleway/Compute, because their agents operate that infra
// (limited by the SCOPED cloud token they hold, which the provider enforces —
// not by hiding the UI). The admin-only surface is roster management: approving
// devices and issuing role grants.
//
// NOTE: this is a DENYLIST, so every other view (Polis, the generative "design"
// module, etc.) is reachable by BOTH roles automatically — design is a normal
// collaborative view, so it is intentionally absent from this set.
const ADMIN_ONLY_VIEWS = new Set<string>(["devices"]);

/**
 * Whether a role may open a view. NOTE: this is cosmetic UX only — the backend
 * enforces the few truly admin-only commands. The real cloud-permission boundary
 * is the collaborator's scoped token, not this filter.
 */
export function isViewAllowedForRole(
  role: Role | null | undefined,
  viewId: string,
): boolean {
  if (role === "admin") return true;
  // Collaborator (or null/loading): everything except the admin-only surfaces.
  return !ADMIN_ONLY_VIEWS.has(viewId);
}

/** Filter a list of nav ids down to those the role may see. */
export function navIdsForRole(
  role: Role | null | undefined,
  allIds: string[],
): string[] {
  return allIds.filter((id) => isViewAllowedForRole(role, id));
}
