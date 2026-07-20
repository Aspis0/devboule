//! Launch / session token hashing (sha256 hex) and unmanaged kill-switch.

use chrono::Duration as ChronoDuration;
use sha2::{Digest, Sha256};
use std::env;
use uuid::Uuid;

pub const LAUNCH_TOKEN_WINDOW: ChronoDuration = ChronoDuration::hours(2);
pub const SESSION_TOKEN_WINDOW: ChronoDuration = ChronoDuration::hours(12);

fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn hash_launch_token(token: &str) -> String {
    sha256_hex(token.trim())
}

pub fn hash_session_token(token: &str) -> String {
    sha256_hex(token.trim())
}

/// 64 hex chars (two uuid4 hexes), same as Python `uuid.uuid4().hex * 2`.
pub fn generate_session_token() -> String {
    format!(
        "{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}

/// Dual-read: Devboule first, Aspis legacy second.
pub fn unmanaged_privileged_agents_allowed() -> bool {
    for key in [
        "DEVBOULE_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS",
        "ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS",
    ] {
        if env::var(key).map(|v| v.trim() == "1").unwrap_or(false) {
            return true;
        }
    }
    false
}
