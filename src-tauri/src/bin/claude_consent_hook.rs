//! Slice 5b: the Claude Code `PreToolUse` hook helper binary.
//!
//! A standalone compiled process (NO Tauri `AppHandle`) that Claude Code spawns before
//! every tool call when registered as a `PreToolUse` hook in the per-project
//! `settings.json` (built by `cloud_claude_config::build_claude_settings`). It reads the
//! hook JSON from STDIN, routes Patch/Exec tool calls through the `.aspis-agents.json`
//! consent file-bridge into the app's existing consent UI, and prints the
//! `permissionDecision` to STDOUT. All the logic lives in the lib
//! (`run_claude_consent_hook`) so it reuses the same file-lock + serializer the app
//! uses; this shim only forwards the exit code.

fn main() {
    std::process::exit(aspis_management_lib::run_claude_consent_hook());
}
