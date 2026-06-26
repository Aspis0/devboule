//! Slice 5b: generate a per-project Claude Code `settings.json` (as serializable structs)
//! from the project's sandbox knobs. We do NOT sandbox Claude's process; instead we drive
//! its native permission system + a PreToolUse hook that bridges every tool call to OUR
//! consent UI over the `.aspis-agents.json` file-bridge.
//!
//! ⚠️ Claude settings.json has NO `sandbox.filesystem`/`sandbox.network` block (verified —
//! not in the official schema). Filesystem/network confinement is enforced by the hook +
//! `permissions.deny` rules only. The keys here are the LITERAL Claude settings keys
//! (`permissions`/`deny`/`hooks`/`PreToolUse`/`type`/`command`/`timeout`) — not camelCase.

use crate::backend::broker::SandboxMode;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClaudeSettings {
    #[serde(skip_serializing_if = "ClaudePermissions::is_empty")]
    pub permissions: ClaudePermissions,
    #[serde(skip_serializing_if = "ClaudeHooks::is_empty")]
    pub hooks: ClaudeHooks,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct ClaudePermissions {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ask: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
}

impl ClaudePermissions {
    fn is_empty(&self) -> bool {
        self.deny.is_empty() && self.ask.is_empty() && self.allow.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct ClaudeHooks {
    #[serde(rename = "PreToolUse", skip_serializing_if = "Vec::is_empty")]
    pub pre_tool_use: Vec<HookMatcher>,
}

impl ClaudeHooks {
    fn is_empty(&self) -> bool {
        self.pre_tool_use.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HookMatcher {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    pub hooks: Vec<HookCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HookCommand {
    #[serde(rename = "type")]
    pub kind: String,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

/// Build the per-project Claude settings from the sandbox knobs.
/// - `helper_path`: absolute path to our compiled PreToolUse hook binary.
/// - `hook_timeout_secs`: the hook `timeout` (must exceed our consent poll cap so the CLI
///   doesn't kill the hook before the human answers).
///
/// `mode` does not change the settings STRUCTURE (the hook is registered for every mode,
/// including Unattended where the helper answers `deny` without prompting — a runtime
/// decision, not a config one). It is part of the signature for future per-mode deny tuning.
pub fn build_claude_settings(
    mode: SandboxMode,
    net_enabled: bool,
    helper_path: Option<&str>,
    hook_timeout_secs: u64,
) -> ClaudeSettings {
    let _ = mode;

    let deny = if !net_enabled {
        vec![
            "Bash(curl *)".to_string(),
            "Bash(wget *)".to_string(),
            "Bash(nc *)".to_string(),
            "WebFetch".to_string(),
            "WebSearch".to_string(),
        ]
    } else {
        Vec::new()
    };

    // 5b max-recall F11: the PreToolUse hook is registered ONLY when the helper binary is
    // available. When it is missing we still emit the `permissions.deny` net rules (deny-only
    // settings) so a net-disabled project never loses its network gating just because the hook
    // could not be located.
    let hooks = match helper_path {
        Some(helper_path) => ClaudeHooks {
            pre_tool_use: vec![HookMatcher {
                matcher: None,
                hooks: vec![HookCommand {
                    kind: "command".to_string(),
                    command: helper_path.to_string(),
                    timeout: Some(hook_timeout_secs),
                }],
            }],
        },
        None => ClaudeHooks::default(),
    };

    ClaudeSettings {
        permissions: ClaudePermissions {
            deny,
            ask: Vec::new(),
            allow: Vec::new(),
        },
        hooks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_settings_registers_pretooluse_hook() {
        let settings =
            build_claude_settings(SandboxMode::Unattended, true, Some("/usr/local/bin/claude-hook"), 30);
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("\"PreToolUse\""));
        assert!(json.contains("\"command\""));
        assert!(json.contains("/usr/local/bin/claude-hook"));
        assert!(json.contains("\"timeout\""));
    }

    #[test]
    fn net_disabled_emits_net_deny_rules() {
        let settings =
            build_claude_settings(SandboxMode::Unattended, false, Some("/usr/local/bin/claude-hook"), 30);
        let json = serde_json::to_string(&settings).unwrap();
        assert!(settings.permissions.deny.contains(&"Bash(curl *)".to_string()));
        assert!(settings.permissions.deny.contains(&"WebFetch".to_string()));
        assert!(json.contains("\"deny\""));
    }

    #[test]
    fn net_enabled_omits_net_deny_rules() {
        let settings =
            build_claude_settings(SandboxMode::Ask, true, Some("/usr/local/bin/claude-hook"), 30);
        assert!(settings.permissions.deny.is_empty());
        let json = serde_json::to_string(&settings).unwrap();
        assert!(!json.contains("\"deny\""));
    }

    #[test]
    fn settings_serializes_pretooluse_and_type_keys() {
        let settings =
            build_claude_settings(SandboxMode::Ask, true, Some("/usr/local/bin/claude-hook"), 30);
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("\"PreToolUse\""));
        assert!(json.contains("\"type\":\"command\""));
    }

    #[test]
    fn default_minimal_json_when_net_enabled() {
        let settings =
            build_claude_settings(SandboxMode::Ask, true, Some("/usr/local/bin/claude-hook"), 30);
        let json = serde_json::to_string(&settings).unwrap();
        assert!(!json.contains("\"permissions\""));
    }

    #[test]
    fn hook_registered_for_unattended_mode_too() {
        let settings =
            build_claude_settings(SandboxMode::Unattended, true, Some("/usr/local/bin/claude-hook"), 30);
        assert!(!settings.hooks.pre_tool_use.is_empty());
    }

    #[test]
    fn deny_only_settings_when_helper_missing_still_keeps_net_rules() {
        // F11: no hook binary → no PreToolUse hook, but the net deny rules MUST remain.
        let settings = build_claude_settings(SandboxMode::Ask, false, None, 30);
        assert!(settings.hooks.pre_tool_use.is_empty(), "no hook when helper is None");
        assert!(settings.permissions.deny.contains(&"Bash(curl *)".to_string()));
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("\"deny\""));
        assert!(!json.contains("\"PreToolUse\""));
    }

    #[test]
    fn deny_only_with_net_enabled_is_empty_no_churn() {
        // Helper missing AND net enabled → nothing to emit (empty settings object).
        let settings = build_claude_settings(SandboxMode::Ask, true, None, 30);
        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(json, "{}");
    }
}
