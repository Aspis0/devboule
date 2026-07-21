//! Canonical agent-role taxonomy — the ONE classification fold.
//!
//! ROLE UNTANGLE (2026-07): devboule has FOUR first-class agent roles, mirroring the
//! server-side truth in `oracle/server/aspis_mcp.py` (`VALID_ROLES` / `ROLE_ALIASES` /
//! `CODER_LIKE_ROLES`) — the MCP server is the gate that actually enforces per-role
//! tool grants, so this module follows it, never the other way around:
//!
//! - `orchestrator` — plans + hands off; NEVER writes (no write/mutation tool; every
//!   code change goes through `spawn_main_coder`). Never owns minis — minis are
//!   Main-coder-only. Locally this is the Devboule binary (`client == "orchestrator"`).
//! - `coder` — the MAIN CODER (UI display name "Main coder"): the primary writer;
//!   alone may `spawn_mini_coder`. Externally a claude/codex CLI writing with its
//!   own native tools.
//! - `verifier` — review-only; the ONLY role that sets Kanban `done`.
//! - `mini` — the delegated sub-task worker (a directive, not a launched client).
//!
//! ROLE ≠ CLIENT. A role is a permission/registration identity; a client is which
//! engine runs it (codex/claude/powershell/orchestrator/custom). The launch intent
//! decides the role: launching the Devboule binary — or, later, a cloud CLI as the
//! planner — yields the `orchestrator` role.
//!
//! This module replaces the former trio of half-measures (`normalize_agent_role`'s
//! orchestrator→coder fold, `pending_session_role`'s client special-case, and the
//! `launch_injects_cloudflare_env` token strip-hack): the orchestrator is first-class
//! and simply never selects a write-token profile (see `vault::canonical_agent_role`).

/// Role that plans + delegates and never writes. Also the reserved CLIENT id of the
/// local Devboule binary — same string, two axes (see module doc).
pub const ROLE_ORCHESTRATOR: &str = "orchestrator";
/// The Main coder role (canonical id kept as "coder" for stored-session and
/// cross-language back-compat; the UI renders `display_name`).
pub const ROLE_CODER: &str = "coder";
pub const ROLE_VERIFIER: &str = "verifier";
pub const ROLE_MINI: &str = "mini";

/// Canonicalize an inbound launch `role` FIELD to one of the launchable roles
/// {coder, verifier, orchestrator}. "architect"/"code" are legacy aliases that fold
/// to coder (mirrors Python `ROLE_ALIASES`); "orchestrator" is FIRST-CLASS and no
/// longer folds. `mini` is not launchable via the agent-launch path (it is a
/// directive), so it is rejected here on purpose.
pub fn canonicalize_launch_role(value: &str) -> Result<String, String> {
    let role = value.trim().to_ascii_lowercase();
    match role.as_str() {
        ROLE_CODER | ROLE_VERIFIER | ROLE_ORCHESTRATOR => Ok(role),
        "architect" | "code" => Ok(ROLE_CODER.to_string()),
        _ => Err("Agent role must be coder, verifier or orchestrator.".into()),
    }
}

/// The EFFECTIVE role for a launch, from the launch intent.
///
/// Selecting the Devboule binary (`client == "orchestrator"`) is an orchestrator
/// launch ONLY. Sending `role=coder` or `role=verifier` with that client used to
/// silently coerce to orchestrator (so "Launch Verifier" with Local CLI produced
/// no verifier session). Fail closed with a clear error instead; the UI must not
/// offer coder/verifier when Local is selected. Role already `orchestrator` (or
/// empty, treated as default orchestrator) succeeds. Every other client keeps the
/// canonical role verbatim.
pub fn effective_launch_role(client: &str, canonical_role: &str) -> Result<String, String> {
    if client == ROLE_ORCHESTRATOR {
        let role = canonical_role.trim();
        if role.is_empty() || role == ROLE_ORCHESTRATOR {
            return Ok(ROLE_ORCHESTRATOR.to_string());
        }
        if role == ROLE_CODER || role == ROLE_VERIFIER {
            return Err(
                "Local (Devboule) launches the orchestrator only. Pick Claude/Codex/Gemini for coder or verifier."
                    .into(),
            );
        }
        // Unknown role strings should already have been rejected by canonicalize;
        // still refuse rather than silently coerce.
        return Err(format!(
            "Local (Devboule) launches the orchestrator only (got role '{role}')."
        ));
    }
    Ok(canonical_role.to_string())
}

/// Roles sharing the coder's Kanban transition + claim semantics (mirrors Python
/// `CODER_LIKE_ROLES`): claim todo/wip/blocked, set todo/wip/review/blocked, reopen
/// to todo — never the verifier-only `done`. The orchestrator is strictly
/// tighter-or-equal to the coder everywhere else (no write tools, no write token).
pub fn is_coder_like(role: &str) -> bool {
    matches!(role, ROLE_CODER | ROLE_ORCHESTRATOR)
}

/// UI display name for a canonical role id. The `coder` id renders as "Main coder"
/// (the id string stays "coder" for back-compat; only the label changed in the
/// role untangle).
pub fn display_name(role: &str) -> &'static str {
    match role {
        ROLE_ORCHESTRATOR => "Orchestrator",
        ROLE_CODER => "Main coder",
        ROLE_VERIFIER => "Verifier",
        ROLE_MINI => "Mini coder",
        _ => "Agent",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_keeps_orchestrator_first_class() {
        // The load-bearing untangle assertion: orchestrator no longer folds to coder.
        assert_eq!(
            canonicalize_launch_role("orchestrator").unwrap(),
            "orchestrator"
        );
        assert_eq!(canonicalize_launch_role("ORCHESTRATOR").unwrap(), "orchestrator");
    }

    #[test]
    fn canonicalize_folds_legacy_aliases_to_coder() {
        assert_eq!(canonicalize_launch_role("architect").unwrap(), "coder");
        assert_eq!(canonicalize_launch_role("code").unwrap(), "coder");
        assert_eq!(canonicalize_launch_role(" Coder ").unwrap(), "coder");
        assert_eq!(canonicalize_launch_role("verifier").unwrap(), "verifier");
    }

    #[test]
    fn canonicalize_rejects_unknown_and_mini() {
        assert!(canonicalize_launch_role("mini").is_err());
        assert!(canonicalize_launch_role("").is_err());
        assert!(canonicalize_launch_role("random").is_err());
    }

    #[test]
    fn effective_role_follows_launch_intent() {
        // Devboule-binary client ⇒ orchestrator role when role matches intent.
        assert_eq!(
            effective_launch_role("orchestrator", "orchestrator").unwrap(),
            "orchestrator"
        );
        assert_eq!(
            effective_launch_role("orchestrator", "").unwrap(),
            "orchestrator"
        );
        // Mismatched coder/verifier with Local client fails closed (no silent coerce).
        let err = effective_launch_role("orchestrator", "coder").unwrap_err();
        assert!(err.contains("orchestrator only"), "{err}");
        let err = effective_launch_role("orchestrator", "verifier").unwrap_err();
        assert!(err.contains("orchestrator only"), "{err}");
        // Every other client keeps the canonical role verbatim.
        assert_eq!(effective_launch_role("codex", "coder").unwrap(), "coder");
        assert_eq!(
            effective_launch_role("claude", "verifier").unwrap(),
            "verifier"
        );
        assert_eq!(
            effective_launch_role("custom-cli", "coder").unwrap(),
            "coder"
        );
    }

    #[test]
    fn coder_like_mirrors_python_contract() {
        assert!(is_coder_like("coder"));
        assert!(is_coder_like("orchestrator"));
        assert!(!is_coder_like("verifier"));
        assert!(!is_coder_like("mini"));
    }

    #[test]
    fn display_names_render_main_coder() {
        assert_eq!(display_name("coder"), "Main coder");
        assert_eq!(display_name("orchestrator"), "Orchestrator");
        assert_eq!(display_name("verifier"), "Verifier");
        assert_eq!(display_name("mini"), "Mini coder");
    }
}
