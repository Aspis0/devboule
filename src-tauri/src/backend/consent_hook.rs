//! Slice 5b: the runtime of the Claude Code `PreToolUse` hook helper.
//!
//! This module is the body of the standalone `claude_consent_hook` binary (see
//! `src/bin/claude_consent_hook.rs`), kept inside the lib so it can reuse the private
//! `backend` file-bridge primitives (`agents::mutate_agent_live_state_at_path`,
//! `consent_bridge`) and the shared `broker::ConsentKind`. The bin is a thin shim that
//! calls `run()`.
//!
//! FLOW (per the Slice 5b plan, mirroring the Python git-push gate):
//!   1. Read the PreToolUse hook JSON from STDIN (tolerant parse).
//!   2. Resolve context from the launcher-injected env (`ASPIS_CONSENT_BRIDGE`,
//!      `ASPIS_CONSENT_AGENT_ID`, `ASPIS_CONSENT_PROJECT_ID`).
//!   3. Map `tool_name` → `ConsentKind`. Read-only / safe tools (Read/Glob/Grep/…/mcp__*)
//!      ALLOW immediately with no file round-trip. Write/Edit → Patch; Bash → Exec.
//!   4. For Patch/Exec: append a `pending_approval` `ConsentBridgeRequest` to
//!      `.aspis-agents.json` + light the session's `needs_user`, then BOUNDED-poll the
//!      verdict (re-read under the lock, ~250ms interval, cap from
//!      `ASPIS_CONSENT_TIMEOUT_SECS`, default 300s).
//!   5. Print Claude's `hookSpecificOutput.permissionDecision` (allow/deny) and exit 0.
//!      On ANY error/timeout → print `deny` (FAIL-CLOSED) and exit 0.
//!
//! ⚠️ UNVERIFIED (live e2e later, the owner's eyes): the exact PreToolUse stdin field names
//! (`tool_name`/`tool_input`/`cwd`/`session_id`) AND that hooks fire at all in `--print`
//! headless mode. We parse tolerantly (missing fields degrade, never panic) and the
//! output shape follows the documented `hookSpecificOutput` schema. If a field name is
//! wrong the request still forms (with an empty detail) and fails safe.

use crate::backend::agents;
use crate::backend::broker::ConsentKind;
use crate::backend::consent_bridge::{
    cap_consent_requests, ConsentBridgeRequest, ConsentBridgeStatus, MAX_CONSENT_REQUESTS,
};
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

/// Default bounded-poll cap (seconds) if `ASPIS_CONSENT_TIMEOUT_SECS` is unset/garbage.
const DEFAULT_TIMEOUT_SECS: u64 = 300;
/// Poll interval between re-reads of the verdict.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

// ───────────────────────────────────────────────────────────────────────────
// PURE: tool_name → ConsentKind mapping (None = allow-fast, no round-trip)
// ───────────────────────────────────────────────────────────────────────────

/// Map a Claude tool name to the consent `ConsentKind` that gates it, or `None` when the
/// tool is read-only / safe and must be ALLOWED immediately with no human round-trip.
///
/// - `Write`/`Edit`/`MultiEdit`/`NotebookEdit` → `Patch` (a file mutation).
/// - `Bash`/`KillShell` → `Exec` (arbitrary command execution).
/// - `Read`/`Glob`/`Grep`/`LS`/`Task`/`WebFetch`/`WebSearch`/`TodoWrite` and any
///   `mcp__*` tool → `None` (allow-fast). Network confinement for WebFetch/WebSearch
///   (and curl/wget via Bash) is enforced separately by the generated settings'
///   `permissions.deny` rules, NOT by this consent prompt, so they allow-fast here.
///   `TodoWrite` only mutates the agent's in-session todo list (not the repo), so it is
///   treated as benign.
/// - Any UNKNOWN tool → `Exec` (fail-safe: an unrecognized tool is treated as the most
///   privileged category, so it is gated rather than silently allowed).
///
/// ⚠️ max-recall note (ACCEPTED, by design): `mcp__*` tools are allow-fast because the MCP
/// server set is a USER trust boundary — it is built from the app config + the user's own MCP
/// settings (`--mcp-config`), and a repo CANNOT inject an MCP server. The consent gate exists
/// to mediate the agent's use of the built-in filesystem/exec tools, not to re-litigate the
/// user's explicit choice of MCP servers. A future tightening could allow-fast only an
/// allow-list of known read-only MCP tools and gate the rest as `Exec`; deferred so the app's
/// own (heavily-used, read-only) Oracle MCP isn't prompt-spammed / silently denied in Unattended.
pub fn hook_tool_to_kind(tool_name: &str) -> Option<ConsentKind> {
    match tool_name {
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => Some(ConsentKind::Patch),
        "Bash" | "KillShell" => Some(ConsentKind::Exec),
        "Read" | "Glob" | "Grep" | "LS" | "Task" | "WebFetch" | "WebSearch" | "TodoWrite" => None,
        name if name.starts_with("mcp__") => None,
        // Unknown tool: gate it as Exec (most privileged) — fail-safe, never allow-fast.
        _ => Some(ConsentKind::Exec),
    }
}

// ───────────────────────────────────────────────────────────────────────────
// PURE: Claude hook output JSON
// ───────────────────────────────────────────────────────────────────────────

/// Build the Claude `PreToolUse` hook output JSON for an allow/deny decision.
///
/// `allow == true` → `permissionDecision: "allow"`; `false` → `"deny"`. The schema is
/// `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":...,
/// "permissionDecisionReason":...}}`. The reason is short, already-safe prose (it never
/// echoes a secret — the detail it might mention is the agent's own command/file path,
/// which the agent already knows).
pub fn hook_output_json(allow: bool, reason: &str) -> String {
    let decision = if allow { "allow" } else { "deny" };
    // Build via serde_json so the reason is correctly JSON-escaped (quotes/newlines).
    let value = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": decision,
            "permissionDecisionReason": reason,
        }
    });
    value.to_string()
}

// ───────────────────────────────────────────────────────────────────────────
// PURE: detail/path extraction from the tool_input object
// ───────────────────────────────────────────────────────────────────────────

/// Extract a `(detail, path)` pair from the tool_input for the card.
///
/// - Exec: detail = the `command` string (the bash command). No path.
/// - Patch: detail + path = the `file_path` (the edited file). NotebookEdit uses
///   `notebook_path`; we fall back to it.
///
/// All lookups are tolerant: a missing/wrong-typed field yields an empty detail and a
/// `None` path — the request still forms and fails safe.
fn extract_detail_path(
    kind: &ConsentKind,
    tool_input: &serde_json::Value,
) -> (String, Option<String>) {
    let str_field = |key: &str| -> Option<String> {
        tool_input
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|s| !s.is_empty())
    };
    match kind {
        ConsentKind::Exec => (str_field("command").unwrap_or_default(), None),
        ConsentKind::Patch => {
            let path = str_field("file_path").or_else(|| str_field("notebook_path"));
            (path.clone().unwrap_or_default(), path)
        }
        // Net/FolderWrite are not produced by hook_tool_to_kind today; degrade safely.
        _ => (String::new(), None),
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Runtime entrypoint (the binary's body)
// ───────────────────────────────────────────────────────────────────────────

/// Run the hook end-to-end: read stdin, decide, print the Claude hook output, and
/// return the process exit code (ALWAYS 0 — Claude reads the decision from STDOUT, and
/// a non-zero exit would be interpreted as a hook execution error, not a deny).
pub fn run() -> i32 {
    // FAIL-CLOSED: any error below prints `deny` and exits 0. We never panic.
    let decision = decide();
    let (allow, reason) = match decision {
        Ok(true) => (
            true,
            "Allowed by the human via Devboule consent.".to_string(),
        ),
        Ok(false) => (
            false,
            "Denied by the human via Devboule consent.".to_string(),
        ),
        Err(reason) => (false, reason), // fail-closed deny with the cause.
    };
    println!("{}", hook_output_json(allow, &reason));
    0
}

/// The decision core, returning `Ok(true)`=allow / `Ok(false)`=deny / `Err(reason)`=a
/// fail-closed deny with a human-readable cause. Split out so `run()` stays trivial.
fn decide() -> Result<bool, String> {
    // 1) Read STDIN (tolerant — empty/garbage degrades to an empty object).
    let mut buf = String::new();
    let _ = std::io::stdin().read_to_string(&mut buf);
    let payload: serde_json::Value =
        serde_json::from_str(buf.trim()).unwrap_or(serde_json::Value::Null);

    let tool_name = payload
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tool_input = payload
        .get("tool_input")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    // 2) Map the tool to a kind. None → allow-fast (read-only/safe), no round-trip.
    let kind = match hook_tool_to_kind(tool_name) {
        None => return Ok(true),
        Some(k) => k,
    };

    // 3) Resolve the launcher-injected context. The bridge path is REQUIRED; without it
    //    we cannot reach the consent UI, so fail-closed deny.
    let bridge = std::env::var("ASPIS_CONSENT_BRIDGE").unwrap_or_default();
    if bridge.trim().is_empty() {
        return Err("Devboule consent bridge is not configured (no ASPIS_CONSENT_BRIDGE).".into());
    }
    let projects_dir = Path::new(&bridge);
    let agent_id = std::env::var("ASPIS_CONSENT_AGENT_ID").unwrap_or_default();
    let project_id = std::env::var("ASPIS_CONSENT_PROJECT_ID").unwrap_or_default();
    let timeout_secs = std::env::var("ASPIS_CONSENT_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(DEFAULT_TIMEOUT_SECS);

    let (detail, path) = extract_detail_path(&kind, &tool_input);

    // 4) Append the pending request + light the session's needs_user, under the lock.
    let request_id = new_request_id();
    let created_at = chrono::Utc::now().to_rfc3339();
    let request = ConsentBridgeRequest {
        id: request_id.clone(),
        agent_id: agent_id.clone(),
        project_id,
        kind,
        detail,
        path,
        status: ConsentBridgeStatus::PendingApproval,
        created_at,
    };
    agents::mutate_agent_live_state_at_path(projects_dir, |live| {
        live.consent_requests.push(request);
        cap_consent_requests(&mut live.consent_requests, MAX_CONSENT_REQUESTS);
        // Light the existing needs-you bell on the requesting session (mirrors the
        // Python git-push gate). A missing session is a no-op — the request still
        // shows in the card by agent_id; the app clears the bell on respond.
        if !agent_id.is_empty() {
            if let Some(session) = live.sessions.iter_mut().find(|s| s.agent_id == agent_id) {
                session.needs_user = Some(crate::backend::model::AgentNeedsUser {
                    reason: "needs_consent".to_string(),
                    message: "Awaiting your approval for a tool action.".to_string(),
                    since: chrono::Utc::now().to_rfc3339(),
                });
            }
        }
    })
    .map_err(|e| format!("Could not record the consent request: {e}"))?;

    // 5) BOUNDED-poll the verdict. Re-read under the lock each pass; sleep OUTSIDE it.
    //    On the hard cap, stamp the still-pending request `timeout` (best-effort) and
    //    fail-closed deny. A vanished request (capped out) is also a fail-closed deny.
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    // 5b reviewer F6: a PERMANENTLY broken bridge (lock file deleted, projects dir unmounted)
    // would otherwise spin for the full timeout. Fail-closed after a short run of consecutive
    // read errors instead of waiting the whole cap. A single transient error still retries.
    let mut consecutive_errors: u32 = 0;
    const MAX_CONSECUTIVE_ERRORS: u32 = 10; // ~2.5s at the 250ms poll interval
    loop {
        match poll_status(projects_dir, &request_id) {
            Ok(Some(ConsentBridgeStatus::Allowed)) => return Ok(true),
            Ok(Some(ConsentBridgeStatus::Denied)) => return Ok(false),
            Ok(Some(ConsentBridgeStatus::Timeout)) => {
                return Err("Consent request timed out.".into());
            }
            Ok(Some(ConsentBridgeStatus::PendingApproval)) => {
                consecutive_errors = 0; /* healthy read — keep polling */
            }
            Ok(Some(ConsentBridgeStatus::Superseded)) => {
                // A newer ask for the same (project,kind,path) superseded this one.
                // The user may still answer this stale ask — keep polling (same as
                // PendingApproval) so a late answer still resolves the hook.
                consecutive_errors = 0;
            }
            Ok(None) => {
                // Vanished (evicted / hand-edited away) with no verdict → fail-closed.
                return Err("Consent request vanished before a decision.".into());
            }
            Err(_) => {
                consecutive_errors += 1;
                if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                    return Err("Consent bridge is unreadable; failing closed.".into());
                }
            }
        }
        if Instant::now() >= deadline {
            // Best-effort: stamp the still-pending request timeout so a late human
            // answer no-ops, and clear the bell. Swallow errors — we deny regardless.
            let _ = agents::mutate_agent_live_state_at_path(projects_dir, |live| {
                if let Some(req) = live
                    .consent_requests
                    .iter_mut()
                    .find(|r| r.id == request_id)
                {
                    // claim_terminal only transitions a still-pending request, so a race
                    // with the human's answer never clobbers a recorded verdict.
                    crate::backend::consent_bridge::claim_terminal(
                        req,
                        ConsentBridgeStatus::Timeout,
                    );
                }
                if !agent_id.is_empty() {
                    if let Some(session) = live.sessions.iter_mut().find(|s| s.agent_id == agent_id)
                    {
                        session.needs_user = None;
                    }
                }
            });
            return Err("Consent request timed out.".into());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Read the live state under the lock and return the request's status, or `None` if the
/// request is no longer present. Mirrors the Python `_git_push_request_result` read.
fn poll_status(
    projects_dir: &Path,
    request_id: &str,
) -> Result<Option<ConsentBridgeStatus>, String> {
    let state = agents::read_agent_live_state_at_path(projects_dir)?;
    Ok(state
        .consent_requests
        .iter()
        .find(|r| r.id == request_id)
        .map(|r| r.status))
}

/// Generate a random hex request id WITHOUT pulling in the app's launch-token machinery
/// (the bin must stay light). `getrandom` is already a dependency; a failure falls back
/// to a time+pid id (collision-resistant enough for a per-process queue key).
fn new_request_id() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_ok() {
        return hex::encode(bytes);
    }
    format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- hook_tool_to_kind --------------------------------------------------

    #[test]
    fn edits_map_to_patch() {
        for t in ["Write", "Edit", "MultiEdit", "NotebookEdit"] {
            assert_eq!(hook_tool_to_kind(t), Some(ConsentKind::Patch), "tool {t}");
        }
    }

    #[test]
    fn bash_and_killshell_map_to_exec() {
        assert_eq!(hook_tool_to_kind("Bash"), Some(ConsentKind::Exec));
        assert_eq!(hook_tool_to_kind("KillShell"), Some(ConsentKind::Exec));
    }

    #[test]
    fn read_only_tools_allow_fast() {
        for t in [
            "Read",
            "Glob",
            "Grep",
            "LS",
            "Task",
            "WebFetch",
            "WebSearch",
            "TodoWrite",
        ] {
            assert_eq!(hook_tool_to_kind(t), None, "tool {t} must allow-fast");
        }
    }

    #[test]
    fn mcp_tools_allow_fast() {
        assert_eq!(hook_tool_to_kind("mcp__oracle__ask"), None);
        assert_eq!(hook_tool_to_kind("mcp__whatever"), None);
    }

    #[test]
    fn unknown_tool_is_gated_as_exec_fail_safe() {
        // An unrecognized tool must be GATED (most privileged), never allow-fast.
        assert_eq!(hook_tool_to_kind("SomeFutureTool"), Some(ConsentKind::Exec));
        assert_eq!(hook_tool_to_kind(""), Some(ConsentKind::Exec));
    }

    // -- hook_output_json ---------------------------------------------------

    #[test]
    fn output_allow_shape() {
        let json = hook_output_json(true, "ok");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "allow");
        assert_eq!(v["hookSpecificOutput"]["permissionDecisionReason"], "ok");
    }

    #[test]
    fn output_deny_shape() {
        let json = hook_output_json(false, "nope");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(v["hookSpecificOutput"]["permissionDecisionReason"], "nope");
    }

    #[test]
    fn output_reason_is_json_escaped() {
        // A reason with quotes/newlines must produce valid JSON (no manual concatenation bug).
        let json = hook_output_json(false, "bad \"quote\"\nand newline");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(
            v["hookSpecificOutput"]["permissionDecisionReason"],
            "bad \"quote\"\nand newline"
        );
    }

    // -- extract_detail_path ------------------------------------------------

    #[test]
    fn exec_detail_is_the_command() {
        let input = serde_json::json!({ "command": "cargo build" });
        let (detail, path) = extract_detail_path(&ConsentKind::Exec, &input);
        assert_eq!(detail, "cargo build");
        assert_eq!(path, None);
    }

    #[test]
    fn patch_detail_and_path_is_file_path() {
        let input = serde_json::json!({ "file_path": "/repo/src/a.rs", "content": "x" });
        let (detail, path) = extract_detail_path(&ConsentKind::Patch, &input);
        assert_eq!(detail, "/repo/src/a.rs");
        assert_eq!(path.as_deref(), Some("/repo/src/a.rs"));
    }

    #[test]
    fn patch_notebook_path_fallback() {
        let input = serde_json::json!({ "notebook_path": "/repo/nb.ipynb" });
        let (detail, path) = extract_detail_path(&ConsentKind::Patch, &input);
        assert_eq!(detail, "/repo/nb.ipynb");
        assert_eq!(path.as_deref(), Some("/repo/nb.ipynb"));
    }

    #[test]
    fn missing_fields_degrade_to_empty() {
        let input = serde_json::Value::Null;
        let (detail, path) = extract_detail_path(&ConsentKind::Exec, &input);
        assert!(detail.is_empty());
        assert_eq!(path, None);
        let (detail2, path2) = extract_detail_path(&ConsentKind::Patch, &input);
        assert!(detail2.is_empty());
        assert_eq!(path2, None);
    }
}
