//! Typed Oracle error surfaced to the frontend.
//!
//! Every Oracle command returns `Result<T, OracleError>` so the UI can react to
//! the *kind* of failure (no workspace, server down, missing API key, empty
//! index, …) instead of parsing an opaque string. Each kind carries an
//! actionable, English `remediation` hint the UI can show verbatim.
//!
//! Serde uses camelCase to match the rest of the Tauri/TS boundary (see the
//! `#[serde(rename_all = "camelCase")]` structs in `oracle/model.rs`), so the
//! kinds serialize as `noWorkspaceRoot`, `serverUnavailable`, … and the struct
//! fields as `kind`, `message`, `remediation`.

use serde::Serialize;

/// Maximum length of a sanitized external detail string. External error text
/// (Python tracebacks, HTTP bodies, OS errors) can be huge; we cap it after
/// redaction so the IPC payload stays small and bounded.
const MAX_SANITIZED_DETAIL_LEN: usize = 300;

/// Redact privacy-sensitive substrings from a raw external error string before
/// it crosses the Tauri IPC boundary into `OracleError.message`.
///
/// Raw Python tracebacks, HTTP response bodies and OS error strings routinely
/// embed absolute filesystem paths (which leak the OS username and machine
/// layout) and, in the worst case, secret-like tokens echoed from a crash. This
/// is the single choke point that scrubs them. We deliberately keep the FULL raw
/// detail in the app log (`eprintln!` at the call sites) for debuggability — only
/// the user-facing `message` is sanitized here.
///
/// No regex dependency: a conservative whitespace-token scan redacts
/// path-looking and secret-looking tokens. We err toward over-redaction of
/// suspicious tokens, but the digit requirement on secrets avoids eating
/// ordinary prose words.
fn sanitize_detail(raw: &str) -> String {
    let redacted = raw
        .split_inclusive(char::is_whitespace)
        .map(|piece| {
            // Split the trailing whitespace off so it is preserved verbatim.
            let trimmed_len = piece.trim_end_matches(char::is_whitespace).len();
            let (token, trailing) = piece.split_at(trimmed_len);
            if token.is_empty() {
                return piece.to_string();
            }
            let replacement = if looks_like_path(token) {
                "<path>"
            } else if looks_like_secret(token) {
                "<redacted>"
            } else {
                token
            };
            format!("{replacement}{trailing}")
        })
        .collect::<String>();

    if redacted.chars().count() > MAX_SANITIZED_DETAIL_LEN {
        redacted
            .chars()
            .take(MAX_SANITIZED_DETAIL_LEN)
            .collect::<String>()
    } else {
        redacted
    }
}

/// Heuristic: does this token look like an absolute filesystem path that would
/// leak the OS username / machine layout? Covers Windows drive paths
/// (`C:\Users\...`), Windows verbatim/UNC paths (`\\?\...`, `\\server\...`) and
/// POSIX user/home paths (`/Users/...`, `/home/...`, `/root/...`).
fn looks_like_path(token: &str) -> bool {
    let bytes = token.as_bytes();
    // Windows drive-letter path: `X:\` or `X:/`.
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        return true;
    }
    // Windows UNC / verbatim path: `\\?\...`, `\\server\share`.
    if token.starts_with(r"\\") {
        return true;
    }
    // POSIX paths that expose a user identity.
    if token.starts_with("/Users/")
        || token.starts_with("/home/")
        || token.starts_with("/root/")
        || token.starts_with("/var/folders/")
    {
        return true;
    }
    false
}

/// Heuristic: does this token look like a secret (API key, hash, bearer token)?
/// A run of >=24 chars from `[A-Za-z0-9_-]` that also contains at least one
/// digit. The digit requirement keeps ordinary long words (no digits) intact
/// while still catching keys/hashes which almost always mix letters and digits.
fn looks_like_secret(token: &str) -> bool {
    if token.chars().count() < 24 {
        return false;
    }
    let mut has_digit = false;
    for ch in token.chars() {
        if ch.is_ascii_digit() {
            has_digit = true;
        } else if !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-') {
            return false;
        }
    }
    has_digit
}

/// Discriminant for the class of Oracle failure, so the frontend can branch on
/// `kind` rather than the human-readable `message`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OracleErrorKind {
    /// No indexed workspace folder has been selected/resolved. The previous
    /// silent fallback to the management/graph root is gone; this is now a hard
    /// error.
    NoWorkspaceRoot,
    /// The local Oracle HTTP server / sidecar is not reachable (not started,
    /// crashed, connection refused, timed out).
    ServerUnavailable,
    /// The Python Oracle process ran but returned an error.
    PythonError,
    /// The embedding model/runtime is not installed or failed to load.
    EmbedderUnavailable,
    /// The vector index exists but contains no rows for this query/workspace.
    IndexEmpty,
    /// A remote LLM is required but no API key is configured.
    MissingApiKey,
    /// An unexpected internal error (lock poisoned, auth gate, serialization…).
    Internal,
}

impl OracleErrorKind {
    /// Default, actionable remediation hint for this kind. English, imperative,
    /// safe to show directly in the UI.
    fn default_remediation(self) -> &'static str {
        match self {
            OracleErrorKind::NoWorkspaceRoot => {
                "Open Devboule → Oracle and choose your workspace folder."
            }
            OracleErrorKind::ServerUnavailable => {
                "Start the Oracle runtime (Oracle → Run doctor), then try again."
            }
            OracleErrorKind::PythonError => "Run Oracle doctor (Oracle → Run doctor) to diagnose.",
            OracleErrorKind::EmbedderUnavailable => {
                "Install or repair the Oracle runtime from Oracle → Setup."
            }
            OracleErrorKind::IndexEmpty => {
                "Index your workspace from Oracle → Index before asking."
            }
            OracleErrorKind::MissingApiKey => "Add your provider API key in Oracle → Settings.",
            OracleErrorKind::Internal => {
                "Try again; if it persists, run Oracle doctor (Oracle → Run doctor)."
            }
        }
    }
}

/// Typed error returned by every Oracle command. Serialized to the frontend by
/// Tauri as the `Err` variant of the command result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OracleError {
    pub kind: OracleErrorKind,
    pub message: String,
    pub remediation: String,
}

impl OracleError {
    /// Build an error of `kind` with a custom `message` and the kind's default
    /// remediation.
    pub fn new(kind: OracleErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            remediation: kind.default_remediation().to_string(),
        }
    }

    pub fn no_workspace_root() -> Self {
        Self::new(
            OracleErrorKind::NoWorkspaceRoot,
            "No indexed workspace folder is selected.",
        )
    }

    pub fn server_unavailable(message: impl Into<String>) -> Self {
        Self::new(OracleErrorKind::ServerUnavailable, message)
    }

    // The following constructors are part of the typed-error API surface but
    // have no production call sites yet (these kinds are produced by the Python
    // classifier in `from_python`; the constructors are reserved for direct use
    // and exercised in tests). Allow dead_code so the API surface stays without
    // warnings; drop the attribute when a real call site lands (as happened to
    // `server_unavailable` above).
    #[allow(dead_code)]
    pub fn python_error(message: impl Into<String>) -> Self {
        Self::new(OracleErrorKind::PythonError, message)
    }

    #[allow(dead_code)]
    pub fn embedder_unavailable(message: impl Into<String>) -> Self {
        Self::new(OracleErrorKind::EmbedderUnavailable, message)
    }

    #[allow(dead_code)]
    pub fn index_empty(message: impl Into<String>) -> Self {
        Self::new(OracleErrorKind::IndexEmpty, message)
    }

    #[allow(dead_code)]
    pub fn missing_api_key(message: impl Into<String>) -> Self {
        Self::new(OracleErrorKind::MissingApiKey, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(OracleErrorKind::Internal, message)
    }

    /// Build an `Internal` error from a RAW external string (e.g. a join error
    /// that embeds `{e}`), sanitizing it first so no path/secret leaks across
    /// the IPC boundary. Use this instead of [`OracleError::internal`] whenever
    /// the message interpolates untrusted/system-origin text.
    pub fn internal_sanitized(raw: impl Into<String>) -> Self {
        Self::new(OracleErrorKind::Internal, sanitize_detail(&raw.into()))
    }

    /// Classify a raw Python/HTTP error string into the most specific kind we
    /// can infer, keeping the original text as the `message`. Connection-level
    /// failures map to `ServerUnavailable`, embedder/model load failures to
    /// `EmbedderUnavailable`, empty-index signals to `IndexEmpty`; everything
    /// else is a generic `PythonError`.
    pub fn from_python(message: impl Into<String>) -> Self {
        let message = message.into();
        let lower = message.to_ascii_lowercase();
        // Classification order matters: embedder/model-load failures are checked
        // BEFORE the generic connection phrases so an embedder error worded as
        // "could not connect to embedder backend" maps to EmbedderUnavailable,
        // not ServerUnavailable.
        let kind = if lower.contains("embedder") || lower.contains("embedding model") {
            OracleErrorKind::EmbedderUnavailable
        } else if lower.contains("connection refused")
            || lower.contains("could not connect to")
            || lower.contains("could not reach")
            || lower.contains("timed out")
            || lower.contains("timeout")
            || lower.contains("server is not")
            || lower.contains("not running")
            || lower.contains("unreachable")
        {
            // NB: the bare "connect" substring was removed — it spuriously
            // matched "disconnect"/"reconnect" and stole embedder errors.
            OracleErrorKind::ServerUnavailable
        } else if lower.contains("index is empty") || lower.contains("empty index") {
            OracleErrorKind::IndexEmpty
        } else {
            OracleErrorKind::PythonError
        };
        // Sanitize at this single choke point so EVERY Python/HTTP-origin error
        // is scrubbed of paths/secrets before it reaches the frontend.
        Self::new(kind, sanitize_detail(&message))
    }
}

impl std::fmt::Display for OracleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for OracleError {}

// --- Oracle doctor report ---------------------------------------------------

/// One health check in the Oracle doctor report. Mirrors the Python doctor's
/// per-check JSON shape (`oracle/bootstrap/doctor.py`). Serialized to the
/// frontend in camelCase to match the rest of the Tauri/TS boundary.
///
/// The five stable check ids are: `runtime`, `embedder`, `workspace`, `index`,
/// `provider`. The `provider` check is a placeholder the Rust side overwrites
/// with a boolean key-presence result (see [`merge_provider_check`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OracleDoctorCheck {
    pub id: String,
    pub ok: bool,
    pub detail: String,
    pub remediation: String,
}

/// The full Oracle doctor report: `ok` is the AND of every check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OracleDoctorReport {
    pub ok: bool,
    pub checks: Vec<OracleDoctorCheck>,
}

/// Stable id of the provider check the Python placeholder emits and the Rust
/// side overwrites.
const PROVIDER_CHECK_ID: &str = "provider";

/// Stable id of the live-server check the Python placeholder emits and the Rust
/// side overwrites with the result of probing the resident server's `/runtime`.
const LIVE_SERVER_CHECK_ID: &str = "live_server";

/// The authoritative outcome of probing the LIVE resident Oracle server, decided
/// by the Rust side (the only layer that holds the session port + auth token).
/// Pure data so [`merge_live_server_check`] is unit-testable without a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveServerProbe {
    /// Reachable AND `/runtime` reports the chunk store ready (records > 0).
    Ready,
    /// Reachable, but the chunk store is not ready (no chunks indexed yet).
    ChunkStoreNotReady,
    /// The resident server could not be reached / did not answer readiness.
    Unreachable,
}

impl OracleDoctorReport {
    /// Recompute the overall `ok` as the AND of all checks. Call after mutating
    /// any check (e.g. the provider merge).
    pub(crate) fn recompute_ok(&mut self) {
        self.ok = self.checks.iter().all(|check| check.ok);
    }
}

/// Pure seam: overwrite the placeholder `provider` check with a boolean
/// key-presence result, then recompute the report's overall `ok`.
///
/// The app/Rust side is the only layer that can read the OS vault, so the Python
/// doctor emits a stable `provider` placeholder (`ok: true`, "checked by app")
/// that this function find/replaces by id. NEVER pass or embed the key itself —
/// only whether one resolved. If no `provider` check exists (e.g. an older
/// Python doctor), one is appended so the UI always has a provider row.
pub fn merge_provider_check(
    mut report: OracleDoctorReport,
    key_present: bool,
) -> OracleDoctorReport {
    let merged = OracleDoctorCheck {
        id: PROVIDER_CHECK_ID.to_string(),
        ok: key_present,
        detail: if key_present {
            "Provider API key is configured.".to_string()
        } else {
            "No provider API key is configured.".to_string()
        },
        remediation: if key_present {
            String::new()
        } else {
            "Add your provider API key in Oracle - Settings.".to_string()
        },
    };
    match report
        .checks
        .iter_mut()
        .find(|check| check.id == PROVIDER_CHECK_ID)
    {
        Some(existing) => *existing = merged,
        None => report.checks.push(merged),
    }
    report.recompute_ok();
    report
}

/// Pure seam: overwrite the placeholder `live_server` check with the result of
/// probing the resident server, then recompute the report's overall `ok`.
///
/// This is what makes a fully-green doctor HONEST: the Python data-layer checks
/// (index/manifest/embedder) can all be green while the live server is
/// unreachable or its retrieval index is not ready — in which case Oracle cannot
/// actually answer. Only the app/Rust side holds the session port + auth token,
/// so the Python doctor emits a stable `live_server` placeholder that this
/// function find/replaces by id. A missing placeholder (older Python doctor) is
/// appended so the UI always has a live-server row. NEVER embeds a port/token.
pub fn merge_live_server_check(
    mut report: OracleDoctorReport,
    probe: LiveServerProbe,
) -> OracleDoctorReport {
    let merged = match probe {
        LiveServerProbe::Ready => OracleDoctorCheck {
            id: LIVE_SERVER_CHECK_ID.to_string(),
            ok: true,
            detail: "Resident Oracle server is answering and its retrieval index is ready."
                .to_string(),
            remediation: String::new(),
        },
        LiveServerProbe::ChunkStoreNotReady => OracleDoctorCheck {
            id: LIVE_SERVER_CHECK_ID.to_string(),
            ok: false,
            detail: "Resident Oracle server is up, but its retrieval index has no chunks yet."
                .to_string(),
            remediation: "Index your workspace from Oracle - Index, then retry.".to_string(),
        },
        LiveServerProbe::Unreachable => OracleDoctorCheck {
            id: LIVE_SERVER_CHECK_ID.to_string(),
            ok: false,
            detail: "The resident Oracle server is not reachable.".to_string(),
            remediation: "Open the Oracle view to start the server, or reinstall the runtime from Oracle - Setup."
                .to_string(),
        },
    };
    match report
        .checks
        .iter_mut()
        .find(|check| check.id == LIVE_SERVER_CHECK_ID)
    {
        Some(existing) => *existing = merged,
        None => report.checks.push(merged),
    }
    report.recompute_ok();
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_with_camel_case_field_and_kind_names() {
        let err = OracleError::no_workspace_root();
        let json = serde_json::to_value(&err).expect("serialize OracleError");
        assert_eq!(json["kind"], "noWorkspaceRoot");
        assert!(json.get("message").is_some(), "message field present");
        assert!(
            json.get("remediation").is_some(),
            "remediation field present"
        );
        // No snake_case leakage.
        assert!(json.get("no_workspace_root").is_none());
    }

    #[test]
    fn each_kind_serializes_to_expected_camel_case_token() {
        let cases = [
            (OracleErrorKind::NoWorkspaceRoot, "noWorkspaceRoot"),
            (OracleErrorKind::ServerUnavailable, "serverUnavailable"),
            (OracleErrorKind::PythonError, "pythonError"),
            (OracleErrorKind::EmbedderUnavailable, "embedderUnavailable"),
            (OracleErrorKind::IndexEmpty, "indexEmpty"),
            (OracleErrorKind::MissingApiKey, "missingApiKey"),
            (OracleErrorKind::Internal, "internal"),
        ];
        for (kind, expected) in cases {
            let json = serde_json::to_value(kind).expect("serialize kind");
            assert_eq!(json, serde_json::Value::String(expected.to_string()));
        }
    }

    #[test]
    fn constructors_set_a_non_empty_default_remediation() {
        let err = OracleError::python_error("boom");
        assert_eq!(err.kind, OracleErrorKind::PythonError);
        assert_eq!(err.message, "boom");
        assert!(!err.remediation.is_empty());
    }

    #[test]
    fn from_python_classifies_connection_failures_as_server_unavailable() {
        let err = OracleError::from_python("Connection refused (os error 10061)");
        assert_eq!(err.kind, OracleErrorKind::ServerUnavailable);

        let err = OracleError::from_python("request timed out after 90s");
        assert_eq!(err.kind, OracleErrorKind::ServerUnavailable);
    }

    #[test]
    fn from_python_defaults_to_python_error() {
        let err = OracleError::from_python("traceback: KeyError 'foo'");
        assert_eq!(err.kind, OracleErrorKind::PythonError);
    }

    // ---- FIX C: classifier precision -------------------------------------

    #[test]
    fn from_python_could_not_connect_to_embedder_is_embedder_unavailable() {
        // Embedder classification must win over the generic connect phrase.
        let err = OracleError::from_python("could not connect to embedder backend");
        assert_eq!(err.kind, OracleErrorKind::EmbedderUnavailable);
    }

    #[test]
    fn from_python_disconnected_is_not_server_unavailable() {
        // The bare "connect" arm used to (wrongly) match "disconnected".
        let err = OracleError::from_python("stream disconnected mid-stream");
        assert_eq!(err.kind, OracleErrorKind::PythonError);
    }

    #[test]
    fn from_python_could_not_connect_to_server_is_server_unavailable() {
        let err = OracleError::from_python("could not connect to http://127.0.0.1:8765");
        assert_eq!(err.kind, OracleErrorKind::ServerUnavailable);
    }

    // ---- FIX A: sanitization ---------------------------------------------

    #[test]
    fn sanitize_detail_redacts_windows_path() {
        let out = sanitize_detail(r"failed to open C:\Users\gualt\Desktop\data.db");
        assert!(!out.contains(r"C:\Users"), "windows path leaked: {out}");
        assert!(out.contains("<path>"), "no redaction marker: {out}");
        assert!(out.starts_with("failed to open"));
    }

    #[test]
    fn sanitize_detail_redacts_posix_user_path() {
        let out = sanitize_detail("cannot read /Users/user/secret/index.lance");
        assert!(!out.contains("/Users/user"), "posix path leaked: {out}");
        assert!(out.contains("<path>"));
    }

    #[test]
    fn sanitize_detail_redacts_home_path() {
        let out = sanitize_detail("open /home/user/.config/key failed");
        assert!(!out.contains("/home/user"), "home path leaked: {out}");
        assert!(out.contains("<path>"));
    }

    #[test]
    fn sanitize_detail_redacts_secret_like_token() {
        // A fake 40-char API key (letters + digits).
        let key = "sk1234567890abcdef1234567890abcdefghij42";
        assert_eq!(key.len(), 40);
        let out = sanitize_detail(&format!("auth failed with key {key}"));
        assert!(!out.contains(key), "secret leaked: {out}");
        assert!(out.contains("<redacted>"));
    }

    #[test]
    fn sanitize_detail_leaves_normal_sentence_intact() {
        let sentence = "The Oracle index could not be loaded for this workspace.";
        assert_eq!(sanitize_detail(sentence), sentence);
    }

    #[test]
    fn sanitize_detail_does_not_redact_long_word_without_digit() {
        // 30 letters, no digit → ordinary word, must NOT be redacted.
        let word = "internationalizationmatters!!";
        let out = sanitize_detail(word);
        assert_eq!(out, word, "over-redacted a plain word: {out}");
    }

    #[test]
    fn sanitize_detail_caps_length() {
        let long = "x".repeat(1000);
        let out = sanitize_detail(&long);
        assert!(out.chars().count() <= 300, "not capped: {}", out.len());
    }

    #[test]
    fn from_python_output_has_no_windows_user_path() {
        let traceback = "Traceback: FileNotFoundError: C:\\Users\\gualt\\AppData\\oracle.sqlite";
        let err = OracleError::from_python(traceback);
        assert!(
            !err.message.contains("C:\\Users"),
            "from_python leaked a path: {}",
            err.message
        );
    }

    #[test]
    fn internal_sanitized_redacts_path() {
        let err = OracleError::internal_sanitized(r"join error at C:\Users\gualt\tmp");
        assert_eq!(err.kind, OracleErrorKind::Internal);
        assert!(
            !err.message.contains(r"C:\Users"),
            "leaked: {}",
            err.message
        );
    }

    // ---- Oracle doctor report --------------------------------------------

    fn placeholder_report(checks_ok: bool) -> OracleDoctorReport {
        OracleDoctorReport {
            ok: checks_ok,
            checks: vec![
                OracleDoctorCheck {
                    id: "embedder".into(),
                    ok: checks_ok,
                    detail: "ok".into(),
                    remediation: String::new(),
                },
                OracleDoctorCheck {
                    id: "provider".into(),
                    ok: true,
                    detail: "checked by app".into(),
                    remediation: String::new(),
                },
            ],
        }
    }

    #[test]
    fn doctor_types_serialize_camel_case() {
        let report = OracleDoctorReport {
            ok: false,
            checks: vec![OracleDoctorCheck {
                id: "runtime".into(),
                ok: false,
                detail: "d".into(),
                remediation: "r".into(),
            }],
        };
        let json = serde_json::to_value(&report).expect("serialize report");
        assert_eq!(json["ok"], false);
        assert_eq!(json["checks"][0]["id"], "runtime");
        assert_eq!(json["checks"][0]["ok"], false);
        assert_eq!(json["checks"][0]["detail"], "d");
        assert_eq!(json["checks"][0]["remediation"], "r");
    }

    #[test]
    fn merge_provider_check_present_keeps_overall_ok() {
        let report = merge_provider_check(placeholder_report(true), true);
        let provider = report
            .checks
            .iter()
            .find(|c| c.id == "provider")
            .expect("provider check present");
        assert!(provider.ok);
        assert!(provider.remediation.is_empty());
        // Every check ok + key present ⇒ overall ok.
        assert!(report.ok);
    }

    #[test]
    fn merge_provider_check_absent_key_fails_overall() {
        let report = merge_provider_check(placeholder_report(true), false);
        let provider = report
            .checks
            .iter()
            .find(|c| c.id == "provider")
            .expect("provider check present");
        assert!(!provider.ok);
        assert!(!provider.remediation.is_empty());
        // A failing provider check drags the recomputed overall ok to false.
        assert!(!report.ok);
        // Exactly one provider check (overwritten, not duplicated).
        assert_eq!(
            report.checks.iter().filter(|c| c.id == "provider").count(),
            1
        );
    }

    #[test]
    fn merge_provider_check_does_not_revive_other_failures() {
        // A non-provider check already failed; a present key must NOT flip overall.
        let report = merge_provider_check(placeholder_report(false), true);
        assert!(!report.ok);
    }

    #[test]
    fn merge_provider_check_appends_when_missing() {
        let report = OracleDoctorReport {
            ok: true,
            checks: vec![OracleDoctorCheck {
                id: "runtime".into(),
                ok: true,
                detail: "ok".into(),
                remediation: String::new(),
            }],
        };
        let merged = merge_provider_check(report, true);
        assert_eq!(
            merged.checks.iter().filter(|c| c.id == "provider").count(),
            1,
            "provider check appended"
        );
        assert!(merged.ok);
    }

    // ---- live-server check ----------------------------------------------

    fn report_with_live_placeholder() -> OracleDoctorReport {
        OracleDoctorReport {
            ok: true,
            checks: vec![
                OracleDoctorCheck {
                    id: "index".into(),
                    ok: true,
                    detail: "ok".into(),
                    remediation: String::new(),
                },
                // The Python placeholder: ok:true, "checked by app".
                OracleDoctorCheck {
                    id: "live_server".into(),
                    ok: true,
                    detail: "checked by app".into(),
                    remediation: String::new(),
                },
            ],
        }
    }

    #[test]
    fn merge_live_server_ready_keeps_overall_ok() {
        let report =
            merge_live_server_check(report_with_live_placeholder(), LiveServerProbe::Ready);
        let live = report
            .checks
            .iter()
            .find(|c| c.id == "live_server")
            .expect("live_server check present");
        assert!(live.ok);
        assert!(live.remediation.is_empty());
        assert!(report.ok);
        // Overwritten, not duplicated.
        assert_eq!(
            report
                .checks
                .iter()
                .filter(|c| c.id == "live_server")
                .count(),
            1
        );
    }

    #[test]
    fn merge_live_server_unreachable_fails_overall_with_remediation() {
        // The CRITICAL honesty case: the data-layer checks are green, but the
        // resident server is unreachable -> the live_server check goes RED and
        // drags the overall report to false, so a green doctor cannot lie.
        let report =
            merge_live_server_check(report_with_live_placeholder(), LiveServerProbe::Unreachable);
        let live = report
            .checks
            .iter()
            .find(|c| c.id == "live_server")
            .expect("live_server check present");
        assert!(!live.ok);
        assert!(!live.remediation.is_empty());
        assert!(!report.ok);
    }

    #[test]
    fn merge_live_server_chunk_store_not_ready_fails_overall() {
        let report = merge_live_server_check(
            report_with_live_placeholder(),
            LiveServerProbe::ChunkStoreNotReady,
        );
        let live = report
            .checks
            .iter()
            .find(|c| c.id == "live_server")
            .expect("live_server check present");
        assert!(!live.ok);
        assert!(!live.remediation.is_empty());
        assert!(!report.ok);
    }

    #[test]
    fn merge_live_server_appends_when_missing() {
        // An older Python doctor that omits the placeholder still gets a row.
        let report = OracleDoctorReport {
            ok: true,
            checks: vec![OracleDoctorCheck {
                id: "runtime".into(),
                ok: true,
                detail: "ok".into(),
                remediation: String::new(),
            }],
        };
        let merged = merge_live_server_check(report, LiveServerProbe::Unreachable);
        assert_eq!(
            merged
                .checks
                .iter()
                .filter(|c| c.id == "live_server")
                .count(),
            1,
            "live_server check appended"
        );
        assert!(!merged.ok, "appended red check drags overall ok false");
    }
}
