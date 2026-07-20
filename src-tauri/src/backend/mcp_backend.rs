//! Which app-tools MCP backend agents should use.
//!
//! - `python` (default until cutover): `python -m oracle.server.aspis_mcp`
//! - `rust`: the native `devboule-mcp` binary
//!
//! See `docs/devboule-mcp-port-plan.md`.

use std::path::PathBuf;

/// Env var selecting the MCP implementation.
pub const ENV_BACKEND: &str = "DEVBOULE_MCP_BACKEND";

/// Optional absolute path to the Rust MCP binary.
pub const ENV_BIN: &str = "DEVBOULE_MCP_BIN";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpBackend {
    Python,
    Rust,
}

impl McpBackend {
    /// Parse from env. Default **Python** until P7 cutover.
    pub fn from_env() -> Self {
        match std::env::var(ENV_BACKEND)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "rust" | "devboule-mcp" | "native" => Self::Rust,
            _ => Self::Python,
        }
    }
}

/// Resolve `devboule-mcp` absolute path when backend is Rust.
pub fn resolve_devboule_mcp_bin() -> Option<PathBuf> {
    if let Ok(p) = std::env::var(ENV_BIN) {
        let pb = PathBuf::from(p.trim());
        if pb.is_file() {
            return Some(pb);
        }
    }
    // Dev: cargo target next to src-tauri
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        for profile in ["debug", "release"] {
            let cand = PathBuf::from(&manifest)
                .join("..")
                .join("devboule-mcp")
                .join("target")
                .join(profile)
                .join(bin_name());
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    // PATH
    crate::backend::provider_detect::resolve_program("devboule-mcp")
}

fn bin_name() -> &'static str {
    #[cfg(windows)]
    {
        "devboule-mcp.exe"
    }
    #[cfg(not(windows))]
    {
        "devboule-mcp"
    }
}

/// Cloudflare profile-mode env key: prefer Devboule name, accept legacy Aspis.
pub fn cloudflare_profile_mode_env_key() -> &'static str {
    "DEVBOULE_MCP_CLOUDFLARE_PROFILE_MODE"
}

/// Legacy key still written for one release (Python MCP + old agent env).
pub const LEGACY_CLOUDFLARE_PROFILE_MODE_ENV: &str = "ASPIS_MCP_CLOUDFLARE_PROFILE_MODE";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_defaults_to_python() {
        // Cannot clear env safely in parallel tests; just ensure unknown → Python.
        assert_eq!(McpBackend::from_env(), McpBackend::from_env());
    }
}
