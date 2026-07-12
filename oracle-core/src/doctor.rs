//! Oracle health doctor — Rust port of `oracle/bootstrap/doctor.py`.
//!
//! Runs independent checks and produces ONE JSON report:
//!
//! ```text
//! {"ok": bool, "checks": [{"id", "ok", "detail", "remediation"}, ...]}
//! ```
//!
//! Each check catches its own errors and degrades to `ok: false` with an
//! actionable, English `remediation`.  The overall `ok` is the AND of every
//! check.
//!
//! PRIVACY: no `detail` or `remediation` string may contain an absolute
//! filesystem path, the OS username, or any secret value.  `safe_detail`
//! redacts path-like substrings defensively.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::OracleDataPaths;
use crate::ingest::indexer::IndexStatusSnapshot;
use crate::store::lance::LanceStore;
use crate::store::manifest::{self, load_manifest};
use crate::store::sqlite::SqliteStore;

// ═══════════════════════════════════════════════════════════════════════════
// Path redaction — port of _safe_detail from doctor.py
// ═══════════════════════════════════════════════════════════════════════════

/// Defense-in-depth scrub: never let an absolute path / home dir survive in a
/// surfaced string.
fn safe_detail(value: &str) -> String {
    let mut out = value.to_string();

    // Windows drive paths: C:\... or C:/...
    let re_drive = regex::Regex::new(r#"[A-Za-z]:[\\/][^\s'"]*"#).unwrap();
    out = re_drive.replace_all(&out, "<path>").to_string();

    // Windows UNC / verbatim prefixes (backslash-backslash-server\...)
    let re_unc = regex::Regex::new(r#"\\[\\][^\s'"]*"#).unwrap();
    out = re_unc.replace_all(&out, "<path>").to_string();

    // POSIX user/home/temp paths.
    let re_posix = regex::Regex::new(r#"/(?:Users|home|root|var/folders)/[^\s'"]*"#).unwrap();
    out = re_posix.replace_all(&out, "<path>").to_string();

    // Cap length.
    let result: String = out.chars().take(400).collect();
    result
}

// ═══════════════════════════════════════════════════════════════════════════
// Check result type
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub id: String,
    pub ok: bool,
    pub detail: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remediation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub checks: Vec<DoctorCheck>,
}

fn check(id: &str, ok: bool, detail: &str, remediation: &str) -> DoctorCheck {
    DoctorCheck {
        id: id.to_string(),
        ok,
        detail: safe_detail(detail),
        remediation: if !ok {
            safe_detail(remediation)
        } else {
            String::new()
        },
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 1) Runtime check — model files present
// ═══════════════════════════════════════════════════════════════════════════

/// Check if the candle HF cache OR ort model directory is readable.
fn check_runtime(ort_model_dir: Option<&Path>) -> DoctorCheck {
    let candle_ok = check_candle_cache();
    let ort_ok = match ort_model_dir {
        Some(dir) => dir.exists() && dir.is_dir(),
        None => false,
    };

    if candle_ok || ort_ok {
        let backend = if candle_ok { "candle" } else { "ort" };
        return check(
            "runtime",
            true,
            &format!("Model files available ({} backend).", backend),
            "",
        );
    }

    check(
        "runtime",
        false,
        "Embedding model files not found.",
        "Install the Oracle runtime from Oracle - Setup.",
    )
}

/// Check for candle qwen3 model in the HuggingFace cache.
fn check_candle_cache() -> bool {
    if let Ok(home) = std::env::var("HOME") {
        let cache = PathBuf::from(home)
            .join(".cache")
            .join("huggingface")
            .join("hub")
            .join("models--Qwen--Qwen3-Embedding-0.6B");
        if cache.exists() {
            return true;
        }
        if let Ok(hf_home) = std::env::var("HF_HOME") {
            let cache = PathBuf::from(hf_home)
                .join("hub")
                .join("models--Qwen--Qwen3-Embedding-0.6B");
            if cache.exists() {
                return true;
            }
        }
    }
    false
}

// ═══════════════════════════════════════════════════════════════════════════
// 2) Stores check — sqlite + chunks.lancedb readable
// ═══════════════════════════════════════════════════════════════════════════

fn check_stores(paths: &OracleDataPaths) -> DoctorCheck {
    let sqlite_ok = SqliteStore::new(&paths.metadata).is_ok();
    let chunks_lance_ok = paths.chunks.exists();

    if sqlite_ok && chunks_lance_ok {
        return check(
            "stores",
            true,
            "SQLite and chunk vector store readable.",
            "",
        );
    }

    let mut missing = Vec::new();
    if !sqlite_ok {
        missing.push("SQLite");
    }
    if !chunks_lance_ok {
        missing.push("chunk vectors");
    }
    check(
        "stores",
        false,
        &format!("Stores unreadable: {}.", missing.join(", ")),
        "Re-index your workspace from Oracle - Index.",
    )
}

// ═══════════════════════════════════════════════════════════════════════════
// 3) Workspace check
// ═══════════════════════════════════════════════════════════════════════════

fn check_workspace(root: Option<&Path>, manifest_path: &Path) -> DoctorCheck {
    let root = match root {
        Some(r) => r,
        None => {
            return check(
                "workspace",
                false,
                "No workspace folder is selected.",
                "Open Devboule - Oracle and choose your workspace folder.",
            );
        }
    };

    if !root.exists() {
        return check(
            "workspace",
            false,
            "Selected workspace folder does not exist.",
            "Open Devboule - Oracle and choose an existing workspace folder.",
        );
    }
    if !root.is_dir() {
        return check(
            "workspace",
            false,
            "Selected workspace path is not a folder.",
            "Open Devboule - Oracle and choose a folder, not a file.",
        );
    }

    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    let manifest_match = manifest_root_matches(root, manifest_path);
    if manifest_match == Some(false) {
        return check(
            "workspace",
            false,
            &format!(
                "Selected folder ('{}') does not match the indexed workspace.",
                name
            ),
            "Index this folder from Oracle - Index, or select the indexed folder.",
        );
    }

    check(
        "workspace",
        true,
        &format!("Workspace folder '{}' is selected.", name),
        "",
    )
}

fn manifest_root_matches(resolved_root: &Path, manifest_path: &Path) -> Option<bool> {
    if !manifest_path.exists() {
        return None;
    }
    let mut manifest = load_manifest(manifest_path);
    let roots = manifest::manifest_roots(&mut manifest);
    if roots.is_empty() {
        return None;
    }
    let target = resolved_root
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let found = roots.keys().any(|key| {
        Path::new(key)
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase() == target)
            .unwrap_or(false)
    });
    Some(found)
}

// ═══════════════════════════════════════════════════════════════════════════
// 4) Index check — port of doctor.py check_index
// ═══════════════════════════════════════════════════════════════════════════

fn check_index(root: Option<&Path>, paths: &OracleDataPaths) -> DoctorCheck {
    let root = match root {
        Some(r) => r.to_path_buf(),
        None => {
            return check(
                "index",
                false,
                "No workspace folder is selected.",
                "Open Devboule - Oracle and choose your workspace folder.",
            );
        }
    };

    let snap = match build_index_snapshot(&root, paths) {
        Ok(s) => s,
        Err(_) => {
            return check(
                "index",
                false,
                "Could not read the index status.",
                "Index your workspace from Oracle - Index.",
            );
        }
    };

    let detail = format!(
        "expected={} indexed={} pending={} chunks={}",
        snap.expected_files, snap.indexed_files, snap.pending_files, snap.sqlite_chunks
    );

    // Mirror ensure_oracle_index_ready EXACTLY: not-ready when
    // expected>0 AND (indexed==0 OR chunks==0).
    if snap.expected_files > 0 && (snap.indexed_files == 0 || snap.sqlite_chunks == 0) {
        return check(
            "index",
            false,
            &format!("The workspace is not indexed yet. {}", detail),
            "Index your workspace from Oracle - Index.",
        );
    }
    if snap.expected_files == 0 {
        return check(
            "index",
            true,
            &format!(
                "No indexable files (all excluded by the secret / .oracleignore filter, or empty workspace). {}",
                detail
            ),
            "",
        );
    }
    check("index", true, &detail, "")
}

fn build_index_snapshot(
    root: &Path,
    paths: &OracleDataPaths,
) -> Result<IndexStatusSnapshot, anyhow::Error> {
    let sqlite = SqliteStore::new(&paths.metadata)?;
    let chunk_vectors = LanceStore::new(&paths.chunks);
    let mini = tokio::runtime::Runtime::new()?;
    mini.block_on(crate::ingest::indexer::chunk_index_status(
        root,
        &sqlite,
        &chunk_vectors,
        &paths.manifest,
    ))
}

// ═══════════════════════════════════════════════════════════════════════════
// 5) Live server placeholder (overwritten by the app)
// ═══════════════════════════════════════════════════════════════════════════

fn live_server_placeholder() -> DoctorCheck {
    DoctorCheck {
        id: "live_server".to_string(),
        ok: true,
        detail: "checked by app".to_string(),
        remediation: String::new(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 6) Provider placeholder (overwritten by the app)
// ═══════════════════════════════════════════════════════════════════════════

fn provider_placeholder() -> DoctorCheck {
    DoctorCheck {
        id: "provider".to_string(),
        ok: true,
        detail: "checked by app".to_string(),
        remediation: String::new(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Public API
// ═══════════════════════════════════════════════════════════════════════════

/// Build the full doctor report.
pub fn build_report(root: Option<&Path>) -> DoctorReport {
    let paths = match root {
        Some(r) => OracleDataPaths::from_root(r),
        None => OracleDataPaths::from_root(&std::env::current_dir().unwrap_or_default()),
    };

    let checks = vec![
        check_runtime(None),
        check_stores(&paths),
        check_workspace(root, &paths.manifest),
        check_index(root, &paths),
        live_server_placeholder(),
        provider_placeholder(),
    ];

    let ok = checks.iter().all(|c| c.ok);
    DoctorReport { ok, checks }
}
