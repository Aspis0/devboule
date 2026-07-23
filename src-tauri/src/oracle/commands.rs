use super::model::*;
use super::oracle_error::{
    merge_live_server_check, merge_provider_check, OracleDoctorReport, OracleError,
};
use super::python_oracle::{
    find_oracle_package_root, probe_oracle_live_server, run_python_oracle_http_get,
    run_python_oracle_http_post, strip_windows_verbatim_prefix,
};
use crate::backend::state::BackendState;
use crate::backend::vault;

/// P2: number of distinct suspect files seeded onto a card from Oracle retrieval.
const SUSPECT_FILE_TOP_K: usize = 6;
/// P2: how many `/context` chunks to retrieve before aggregating to top-K files.
/// Larger than `SUSPECT_FILE_TOP_K` because several chunks usually map to the same
/// file; this gives the max-score-per-file aggregation enough spread to pick the
/// best `SUSPECT_FILE_TOP_K` distinct files.
const SUSPECT_CONTEXT_LIMIT: usize = 24;

fn require_graph_auth(auth_state: &BackendState) -> Result<(), String> {
    auth_state.ensure_unlocked()
}

/// PURE: the actual disabled-gate check, parameterized on the already-read
/// enabled flag so it's independently testable without depending on
/// process-global state.
fn oracle_enabled_gate(enabled: bool) -> Result<(), &'static str> {
    if !enabled {
        return Err("Oracle is disabled — enable it in Settings.");
    }
    Ok(())
}

/// Same lock check as [`require_graph_auth`], PLUS the Oracle-disabled gate.
/// Used by every graph-auth-gated command EXCEPT the setup/install flow
/// (`get_oracle_runtime_setup`, `install_oracle_runtime`), which must keep
/// working while Oracle is disabled — that's how a user installs/repairs the
/// runtime before or while it's turned off.
fn require_graph_auth_and_enabled(auth_state: &BackendState) -> Result<(), String> {
    require_graph_auth(auth_state)?;
    oracle_enabled_gate(crate::backend::oracle_service::oracle_is_enabled())
        .map_err(|e| e.to_string())
}

/// Auth gate for the Oracle query/snapshot commands. Same lock check as
/// [`require_graph_auth`], but maps a locked/auth failure to a typed
/// [`OracleError`] so these commands keep a single error type end-to-end.
fn require_oracle_auth(auth_state: &BackendState) -> Result<(), OracleError> {
    auth_state.ensure_unlocked().map_err(OracleError::internal)?;
    oracle_enabled_gate(crate::backend::oracle_service::oracle_is_enabled())
        .map_err(OracleError::internal)
}

/// Report whether the local Oracle retrieval runtime (Python venv + LanceDB +
/// Qwen3 embedder) is installed. Runs subprocess probes off the UI thread.
#[tauri::command]
pub async fn get_oracle_runtime_setup(
    auth_state: tauri::State<'_, BackendState>,
) -> Result<super::oracle_setup::OracleRuntimeSetup, String> {
    require_graph_auth(&auth_state)?;
    tauri::async_runtime::spawn_blocking(super::oracle_setup::current_oracle_runtime_setup)
        .await
        .map_err(|e| format!("Oracle runtime status task failed: {e}"))
}

/// Install/repair the local Oracle retrieval runtime: create the venv, install
/// LanceDB + the Qwen3 embedder, and warm the model. Long-running and network-
/// bound, so it runs on a blocking worker, not the UI thread.
#[tauri::command]
pub async fn install_oracle_runtime(
    auth_state: tauri::State<'_, BackendState>,
) -> Result<super::oracle_setup::OracleRuntimeSetup, String> {
    require_graph_auth(&auth_state)?;
    tauri::async_runtime::spawn_blocking(super::oracle_setup::install_oracle_runtime)
        .await
        .map_err(|e| format!("Oracle runtime setup task failed: {e}"))?
}

fn oracle_root_query(root: &std::path::PathBuf) -> String {
    format!("root={}", urlencoding::encode(&root.to_string_lossy()))
}

/// Resolve the user-selected indexed workspace root from a preferences value.
///
/// Pure seam (no I/O on preferences, no `AppState`) so it is deterministically
/// unit-testable. The previous silent fallback to the management/graph root is
/// GONE: an unset, blank, missing, or non-directory `prefs_root` is now a hard
/// `OracleError::no_workspace_root()`, never a fallback. This is the regression
/// invariant Step 1 locks: "workspace root unset ⇒ typed NoWorkspaceRoot error,
/// no silent fallback."
fn resolve_oracle_index_root(prefs_root: Option<&str>) -> Result<std::path::PathBuf, OracleError> {
    let Some(root) = prefs_root.map(str::trim).filter(|root| !root.is_empty()) else {
        return Err(OracleError::no_workspace_root());
    };
    let path = std::path::PathBuf::from(root);
    if !path.exists() || !path.is_dir() {
        return Err(OracleError::no_workspace_root());
    }
    // Canonicalize for symlink/`..` safety, then strip the Windows `\\?\`
    // verbatim prefix it adds so the path string sent to the Python server and
    // recorded as the index identity matches the pre-canonicalize form — a
    // `\\?\`-prefixed root would otherwise look like a different workspace and
    // trigger a needless full re-index.
    let canonical = path
        .canonicalize()
        .map_err(|_| OracleError::no_workspace_root())?;
    Ok(strip_windows_verbatim_prefix(canonical))
}

/// Read the persisted Oracle index preferences and resolve the workspace root,
/// hard-erroring (no graph-root fallback) when it is unset/invalid.
///
/// THE single source of truth for "which workspace root does Oracle address?".
/// Both the operator query path (via [`oracle_index_root`]) and the resident-
/// server supervisor (`backend::oracle_service::index_root`) call this, so the
/// server is started against — and `oracle_server_ready` matched against — the
/// exact same root the UI queries. Diverging roots would make the supervisor and
/// the operator path fight over the single server/port in a kill-restart loop.
///
/// Takes no `AppState`: the root comes only from the vault preferences, never the
/// graph root (that fallback is gone — see [`resolve_oracle_index_root`]).
pub(crate) fn current_oracle_index_root() -> Result<std::path::PathBuf, OracleError> {
    // A genuine vault read/keyring/deserialize failure must NOT be swallowed by
    // substituting a guessed default workspace (the old `unwrap_or_else(default)`
    // could silently point the Oracle at an auto-detected root). The first-run
    // "nothing saved yet" case still returns Ok(default) from the vault (NoEntry
    // ⇒ Ok), so it flows through to the NoWorkspaceRoot branch normally. Only a
    // real failure surfaces here, as a SAFE static Internal message (no raw error
    // string crosses the IPC boundary).
    let preferences = vault::read_oracle_index_preferences()
        .map_err(|_| OracleError::internal("Could not read Oracle preferences."))?;
    // Multi-root: primary is first of index_roots, else legacy index_root.
    resolve_oracle_index_root(preferences.primary_index_root().as_deref())
}

/// Operator-path alias for [`current_oracle_index_root`]. Kept as a thin wrapper
/// so the many call sites below read intent-fully; the resolution logic lives in
/// the shared fn above.
fn oracle_index_root() -> Result<std::path::PathBuf, OracleError> {
    current_oracle_index_root()
}

/// P2: shared bounded path for the READ-ONLY Oracle status/detail GET endpoints
/// (`/snapshot`, `/coverage`, `/runtime`, `/node/…`, `/similar/…`,
/// `/duplicate-labels`). It deliberately does NOT go through `try_python_oracle`:
///
/// * The previous P13 ask-permit machinery (a 2-slot RAII guard around the HTTP
///   ask path) was removed when the Python subprocess / permit dance went away;
///   the HTTP path now goes straight through. The boot-poll surge on unlock
///   (runtime/snapshot/coverage "Checking vector runtime") hits this readonly
///   gate instead of the ask path, so a burst of boot polls can never starve a
///   real ask; ask-side backpressure is left to the in-process reqwest client.
/// * It does NOT fall through to the heavy `oracle.cli` subprocess fallback. The
///   underlying `run_python_oracle_http_get` first runs the cheap, bounded (5s)
///   readiness probe ([`require_oracle_server_ready`]); a not-ready server returns
///   the fast typed "starting" error instead of a 165s stall or a model-loading
///   subprocess that would compete with the server the supervisor is bringing up.
///
/// The resident server is single-root per session, so these endpoints need no
/// `?root=` query; the readiness probe confirms the server is serving this exact
/// workspace root before the request is sent.
/// URL-encode a node/cluster id for use as a path segment in a read-only Oracle
/// GET (`/node/{id}`, `/similar/{id}`). Mirrors the server-addressing convention
/// the resident HTTP layer uses: backslashes are normalized to `/`, the value is
/// percent-encoded, and `/` is kept literal so a hierarchical id maps to nested
/// path segments. Kept local (and tested) so it cannot drift from the id shape.
fn encode_oracle_path_segment(value: &str) -> String {
    urlencoding::encode(&value.replace('\\', "/"))
        .replace("%2F", "/")
        .replace("%2f", "/")
}

async fn oracle_readonly_get<T: serde::de::DeserializeOwned + Send + 'static>(
    path: &str,
) -> Result<T, OracleError> {
    let index_root = oracle_index_root()?;
    oracle_http_get_blocking::<T>(index_root, path.to_string())
        .await
        .map_err(OracleError::from_python)
}

/// Run a blocking Oracle HTTP GET off the tokio async worker. The blocking
/// reqwest client is constructed, used and (eventually) returned to its static
/// home entirely on a `spawn_blocking` thread, never on the async executor.
async fn oracle_http_get_blocking<T: serde::de::DeserializeOwned + Send + 'static>(
    root: std::path::PathBuf,
    path: String,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || run_python_oracle_http_get::<T>(&root, &path))
        .await
        .map_err(|e| format!("Oracle HTTP task failed: {e}"))?
}

/// Run a blocking Oracle HTTP POST off the tokio async worker. The blocking
/// reqwest client is constructed, used and (eventually) returned to its static
/// home entirely on a `spawn_blocking` thread, never on the async executor.
async fn oracle_http_post_blocking<T: serde::de::DeserializeOwned + Send + 'static>(
    root: std::path::PathBuf,
    path: String,
    body: Option<serde_json::Value>,
    llm_config: Option<super::python_oracle::OracleLlmRuntimeConfig>,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || {
        run_python_oracle_http_post::<T>(&root, &path, body.as_ref(), llm_config.as_ref())
    })
    .await
    .map_err(|e| format!("Oracle HTTP task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oracle::oracle_error::{OracleDoctorCheck, OracleErrorKind};

    #[test]
    fn graph_command_gate_blocks_when_backend_is_locked() {
        let state = BackendState::new();
        assert!(require_graph_auth(&state).is_err());
    }

    #[test]
    fn resolve_index_root_errors_when_prefs_root_unset() {
        // REGRESSION INVARIANT: workspace root unset ⇒ typed NoWorkspaceRoot
        // error, NEVER a silent fallback to the management/graph root.
        let err = resolve_oracle_index_root(None).expect_err("must hard-error");
        assert_eq!(err.kind, OracleErrorKind::NoWorkspaceRoot);
    }

    #[test]
    fn resolve_index_root_errors_when_prefs_root_blank() {
        let err = resolve_oracle_index_root(Some("   ")).expect_err("blank must hard-error");
        assert_eq!(err.kind, OracleErrorKind::NoWorkspaceRoot);
    }

    #[test]
    fn resolve_index_root_errors_when_prefs_root_does_not_exist() {
        let missing = "C:\\__aspis_nonexistent_workspace_root__\\nope";
        let err = resolve_oracle_index_root(Some(missing)).expect_err("missing must hard-error");
        assert_eq!(err.kind, OracleErrorKind::NoWorkspaceRoot);
    }

    #[test]
    fn resolve_index_root_returns_canonical_path_for_existing_dir() {
        // An existing directory (the crate manifest dir) must resolve, proving
        // the happy path still works after killing the fallback.
        let dir = env!("CARGO_MANIFEST_DIR");
        let resolved = resolve_oracle_index_root(Some(dir)).expect("existing dir resolves");
        assert!(resolved.is_dir());
    }

    #[test]
    fn http_command_root_resolver_is_the_workspace_resolver() {
        // FIX 2 invariant: the HTTP-path operator commands
        // (get_oracle_index_status / get_oracle_indexed_files /
        // start|stop_oracle_index_watcher / start_oracle_index_job /
        // sync_oracle_text_chunks) resolve the server root they address via
        // `oracle_index_root()`, which MUST be the exact same WORKSPACE-root
        // resolver the resident-server supervisor and `ask_oracle` use
        // (`current_oracle_index_root`). If these diverged (the old bug passed the
        // package/code root from `require_oracle_root`), the commands would address
        // a server at the wrong root, never match `oracle_server_ready`, and fight
        // the supervisor in a 60s kill/restart loop. Asserting both resolvers agree
        // for the same vault state locks the one-index-root invariant. We compare
        // the typed result shapes so the test holds whether or not a workspace root
        // is configured in the test environment.
        let command_path = oracle_index_root().map_err(|e| e.kind);
        let supervisor_path = current_oracle_index_root().map_err(|e| e.kind);
        assert_eq!(
            command_path, supervisor_path,
            "HTTP operator commands and the supervisor must resolve the SAME root"
        );
    }

    fn sample_doctor_report(ok: bool) -> OracleDoctorReport {
        OracleDoctorReport {
            ok,
            checks: vec![OracleDoctorCheck {
                id: "runtime".to_string(),
                ok,
                detail: String::new(),
                remediation: String::new(),
            }],
        }
    }

    #[test]
    fn fresh_cached_report_is_reused_no_rerun() {
        // The coalesce seam: a report younger than the TTL is returned as-is, so a
        // second caller that acquires the lock does NOT re-run the heavy doctor.
        let now = chrono::Utc::now();
        let cache = Some((
            now - chrono::Duration::seconds(1),
            sample_doctor_report(true),
        ));
        let hit = cached_doctor_if_fresh(&cache, now, ORACLE_DOCTOR_CACHE_TTL);
        assert!(
            hit.is_some(),
            "a 1s-old report must be reused within the 5s TTL"
        );
        assert_eq!(hit.unwrap(), sample_doctor_report(true));
    }

    #[test]
    fn stale_cached_report_forces_rerun() {
        // Past the TTL the cache must miss, so the next caller re-runs the doctor
        // (reflecting any fix made since the last run).
        let now = chrono::Utc::now();
        let cache = Some((
            now - (ORACLE_DOCTOR_CACHE_TTL + chrono::Duration::seconds(1)),
            sample_doctor_report(true),
        ));
        assert!(
            cached_doctor_if_fresh(&cache, now, ORACLE_DOCTOR_CACHE_TTL).is_none(),
            "a report older than the TTL must NOT be reused"
        );
    }

    #[test]
    fn empty_cache_and_backwards_clock_force_rerun() {
        let now = chrono::Utc::now();
        // No prior run → miss.
        assert!(cached_doctor_if_fresh(&None, now, ORACLE_DOCTOR_CACHE_TTL).is_none());
        // Clock skew (stored timestamp in the future) is treated as stale, never
        // as infinitely fresh.
        let future = Some((
            now + chrono::Duration::seconds(10),
            sample_doctor_report(true),
        ));
        assert!(
            cached_doctor_if_fresh(&future, now, ORACLE_DOCTOR_CACHE_TTL).is_none(),
            "a future timestamp must not count as fresh"
        );
    }

    #[test]
    fn coalesce_runs_heavy_work_once_when_cache_fresh() {
        // Model the exact decision the command makes: check cache; on a hit return
        // it WITHOUT invoking the run-fn; on a miss invoke it and store. Two rapid
        // calls against a fresh cache must invoke the heavy run-fn exactly once.
        use std::cell::Cell;
        let runs = Cell::new(0u32);
        let mut cache: Option<(chrono::DateTime<chrono::Utc>, OracleDoctorReport)> = None;

        let mut call = |now: chrono::DateTime<chrono::Utc>| -> OracleDoctorReport {
            if let Some(hit) = cached_doctor_if_fresh(&cache, now, ORACLE_DOCTOR_CACHE_TTL) {
                return hit;
            }
            // "heavy work"
            runs.set(runs.get() + 1);
            let report = sample_doctor_report(true);
            cache = Some((now, report.clone()));
            report
        };

        let t0 = chrono::Utc::now();
        let _first = call(t0);
        let _second = call(t0 + chrono::Duration::seconds(1)); // within TTL → cache hit
        assert_eq!(
            runs.get(),
            1,
            "heavy work must run once when the cache is fresh"
        );

        // Past the TTL the next call re-runs.
        let _third = call(t0 + ORACLE_DOCTOR_CACHE_TTL + chrono::Duration::seconds(1));
        assert_eq!(runs.get(), 2, "a stale cache must trigger a re-run");
    }

    #[test]
    fn resolve_index_root_strips_windows_verbatim_prefix() {
        // FIX D: the resolved root must NOT carry the `\\?\` Extended-length
        // prefix that canonicalize adds on Windows, or the Python side treats an
        // existing index as a new workspace and re-indexes. No-op assertion on
        // non-Windows (canonicalize never adds the prefix there).
        let dir = env!("CARGO_MANIFEST_DIR");
        let resolved = resolve_oracle_index_root(Some(dir)).expect("existing dir resolves");
        let text = resolved.to_string_lossy();
        assert!(
            !text.starts_with(r"\\?\"),
            "verbatim prefix leaked into resolved root: {text}"
        );
    }

    #[test]
    fn encode_oracle_path_segment_keeps_slashes_and_encodes_specials() {
        // P2: node/cluster ids become path segments in the read-only GETs
        // (`/node/{id}`). Hierarchical ids keep their `/` separators while other
        // unsafe characters are percent-encoded, and backslashes normalize to `/`.
        assert_eq!(encode_oracle_path_segment("src/worker.ts"), "src/worker.ts");
        assert_eq!(
            encode_oracle_path_segment(r"src\worker.ts"),
            "src/worker.ts"
        );
        assert_eq!(encode_oracle_path_segment("a b/c?d"), "a%20b/c%3Fd");
    }

    #[test]
    fn python_failure_maps_to_typed_error_not_ok() {
        // The old code swallowed Python failures into Ok(None) + graph fallback.
        // Now a connection-style failure must surface as ServerUnavailable and a
        // generic failure as PythonError — never silently succeed.
        let server = OracleError::from_python("Connection refused (os error 10061)");
        assert_eq!(server.kind, OracleErrorKind::ServerUnavailable);

        let python = OracleError::from_python("Traceback: ValueError");
        assert_eq!(python.kind, OracleErrorKind::PythonError);
    }

    #[test]
    fn clamp_oracle_result_limit_defaults_and_caps() {
        assert_eq!(clamp_oracle_result_limit(None), ORACLE_RESULT_DEFAULT_LIMIT);
        assert_eq!(clamp_oracle_result_limit(Some(0)), 1);
        assert_eq!(clamp_oracle_result_limit(Some(8)), 8);
        assert_eq!(clamp_oracle_result_limit(Some(32)), 32);
        assert_eq!(
            clamp_oracle_result_limit(Some(10_000)),
            ORACLE_RESULT_MAX_LIMIT
        );
    }

    #[test]
    fn indexed_files_query_clamps_limit_and_encodes_filter() {
        let root = std::path::PathBuf::from("/tmp/workspace");

        // Over the cap clamps to 500; a filter is appended URL-encoded.
        let query = oracle_indexed_files_query(&root, Some(10_000), Some(40), Some("src/ a"));
        assert!(query.contains("limit=500"), "limit not clamped: {query}");
        assert!(query.contains("offset=40"), "offset missing: {query}");
        assert!(
            query.contains("filter=src%2F%20a"),
            "filter not encoded: {query}"
        );

        // Zero/None limit falls back to a sane default within [1, MAX]; a blank
        // filter is omitted entirely (no empty `filter=` on the wire).
        let defaulted = oracle_indexed_files_query(&root, None, None, Some("   "));
        assert!(
            defaulted.contains("limit=100"),
            "default limit wrong: {defaulted}"
        );
        assert!(
            defaulted.contains("offset=0"),
            "default offset wrong: {defaulted}"
        );
        assert!(
            !defaulted.contains("filter="),
            "blank filter should be omitted: {defaulted}"
        );

        // limit=0 is clamped up to 1 (never a zero-size page request).
        let zero = oracle_indexed_files_query(&root, Some(0), None, None);
        assert!(
            zero.contains("limit=1"),
            "zero limit not clamped up: {zero}"
        );
    }

    #[test]
    fn indexed_files_payload_deserializes_camel_case_and_relative_paths() {
        // Mirrors the Python `GET /index/files` response. Confirms the serde
        // shape the TS side relies on, and that a relative path round-trips.
        let payload = serde_json::json!({
            "total": 2,
            "limit": 100,
            "offset": 0,
            "files": [
                {"path": "src/worker.ts", "chunks": 3, "updatedAt": "2026-06-01T00:00:00Z"},
                {"path": "docs/readme.md", "chunks": 1, "updatedAt": "2026-06-01T00:00:01Z"}
            ]
        });
        let parsed: OracleIndexedFiles =
            serde_json::from_value(payload).expect("camelCase payload deserializes");
        assert_eq!(parsed.total, 2);
        assert_eq!(parsed.limit, 100);
        assert_eq!(parsed.offset, 0);
        assert_eq!(parsed.files.len(), 2);
        assert_eq!(parsed.files[0].path, "src/worker.ts");
        assert_eq!(parsed.files[0].chunks, 3);
        assert_eq!(parsed.files[0].updated_at, "2026-06-01T00:00:00Z");
        assert!(!std::path::Path::new(&parsed.files[0].path).is_absolute());

        // Re-serialize and confirm the camelCase key the UI consumes is emitted.
        let out = serde_json::to_value(&parsed).expect("serializes");
        assert!(out["files"][0].get("updatedAt").is_some());
        assert!(out["files"][0].get("updated_at").is_none());
    }

    /// A valid key resolves into the runtime config; a legitimately-absent key
    /// leaves `api_key` None (the answerer then returns an extractive answer — the
    /// only fallback). The non-secret fields always carry through from settings.
    #[test]
    fn valid_key_is_used_and_absent_key_stays_none() {
        let settings = vault::default_oracle_llm_settings();

        let config = assemble_oracle_llm_runtime_config(
            &settings,
            Ok(Some("valid-primary-key".to_string())),
        );
        assert_eq!(
            config.api_key.as_deref(),
            Some("valid-primary-key"),
            "a valid key must be used"
        );
        assert_eq!(config.provider, settings.provider);
        assert_eq!(config.model, settings.model);

        let config_none = assemble_oracle_llm_runtime_config(&settings, Ok(None));
        assert!(
            config_none.api_key.is_none(),
            "a legitimately-absent key stays None"
        );
    }

    /// FINDING 4: a keyring error degrades to no key (extractive) — never aborts —
    /// but distinctly from `Ok(None)` (the warn-log side effect is best-effort and
    /// not asserted).
    #[test]
    fn keyring_error_and_no_key_both_degrade_to_none() {
        let settings = vault::default_oracle_llm_settings();

        let on_error = assemble_oracle_llm_runtime_config(
            &settings,
            Err("transient keyring failure".to_string()),
        );
        assert!(
            on_error.api_key.is_none(),
            "a keyring error must degrade to no key, not abort"
        );

        let on_no_key = assemble_oracle_llm_runtime_config(&settings, Ok(None));
        assert!(
            on_no_key.api_key.is_none(),
            "a legitimately-absent key stays None"
        );
    }

    fn chunk(file: &str, score: f64) -> super::super::python_oracle::ContextChunk {
        super::super::python_oracle::ContextChunk {
            file_source: file.to_string(),
            score,
        }
    }

    #[test]
    fn aggregate_suspect_files_takes_max_score_per_file() {
        // Several chunks from the SAME file collapse to one entry keyed on the
        // file's BEST chunk score (not a sum that would bias toward big files).
        let chunks = vec![
            chunk("src/a.ts", 0.30),
            chunk("src/a.ts", 0.90), // a.ts's best chunk
            chunk("src/b.ts", 0.50),
        ];
        let out = aggregate_suspect_files(&chunks, 6);
        // a.ts (max 0.90) ranks above b.ts (0.50); each file appears once.
        assert_eq!(out, vec!["src/a.ts".to_string(), "src/b.ts".to_string()]);
    }

    #[test]
    fn aggregate_suspect_files_caps_at_top_k() {
        let chunks: Vec<_> = (0..10)
            .map(|i| chunk(&format!("src/f{i}.ts"), i as f64))
            .collect();
        let out = aggregate_suspect_files(&chunks, 3);
        // Highest three scores: f9 (9), f8 (8), f7 (7), in score-desc order.
        assert_eq!(
            out,
            vec![
                "src/f9.ts".to_string(),
                "src/f8.ts".to_string(),
                "src/f7.ts".to_string()
            ]
        );
    }

    #[test]
    fn aggregate_suspect_files_is_deterministic_on_ties() {
        // Equal scores must tie-break by path ASC, regardless of chunk arrival
        // order — no HashMap iteration order leaks in.
        let order_one = vec![
            chunk("src/zeta.ts", 0.5),
            chunk("src/alpha.ts", 0.5),
            chunk("src/mid.ts", 0.5),
        ];
        let order_two = vec![
            chunk("src/mid.ts", 0.5),
            chunk("src/zeta.ts", 0.5),
            chunk("src/alpha.ts", 0.5),
        ];
        let expected = vec![
            "src/alpha.ts".to_string(),
            "src/mid.ts".to_string(),
            "src/zeta.ts".to_string(),
        ];
        assert_eq!(aggregate_suspect_files(&order_one, 6), expected);
        assert_eq!(aggregate_suspect_files(&order_two, 6), expected);
    }

    #[test]
    fn aggregate_suspect_files_empty_and_blank_inputs() {
        // No chunks ⇒ no suspects. A blank/whitespace file_source is skipped (never
        // a phantom suspect). k == 0 yields nothing even with real chunks.
        assert!(aggregate_suspect_files(&[], 6).is_empty());
        let blanks = vec![chunk("   ", 0.9), chunk("", 0.8)];
        assert!(aggregate_suspect_files(&blanks, 6).is_empty());
        assert!(aggregate_suspect_files(&[chunk("src/a.ts", 0.9)], 0).is_empty());
    }

    #[test]
    fn aggregate_suspect_files_ranks_nan_last_and_never_displaces_real_scores() {
        // FIX 3: a file whose ONLY chunk scores are NaN must rank LAST (the doc says
        // "NaN sorts last") and must NEVER displace a real-scored file from top-K.
        // The old comparator used `b.total_cmp(a)`, which ranks +NaN ABOVE every
        // finite score — so a garbage NaN file would have stolen the #1 slot.
        let chunks = vec![
            chunk("src/nan_only.ts", f64::NAN),
            chunk("src/nan_only.ts", f64::NAN), // still only NaN for this file
            chunk("src/real_low.ts", 0.10),
            chunk("src/real_high.ts", 0.90),
        ];
        // Top-2: the two REAL files (high before low); the NaN-only file is excluded.
        let top2 = aggregate_suspect_files(&chunks, 2);
        assert_eq!(
            top2,
            vec![
                "src/real_high.ts".to_string(),
                "src/real_low.ts".to_string()
            ],
            "a NaN-only file must never displace a real-scored file from top-K"
        );
        // With room for all three, the NaN-only file appears LAST.
        let all = aggregate_suspect_files(&chunks, 6);
        assert_eq!(
            all,
            vec![
                "src/real_high.ts".to_string(),
                "src/real_low.ts".to_string(),
                "src/nan_only.ts".to_string(),
            ],
            "NaN scores must sort last, as the doc comment claims"
        );
    }

    #[test]
    fn context_chunk_parses_oracle_payload_keeping_only_path_and_score() {
        // FAIL-CLOSED parse contract: the `/context` chunk shape from
        // `query_engine.chunk_context_payload` deserializes into the pared-down
        // ContextChunk; the `text` (code body) is IGNORED so it can never reach the
        // card store. A missing score defaults to 0.0 rather than failing the parse.
        let payload = serde_json::json!({
            "query": "worker 500 cold start",
            "chunks": [
                {
                    "chunk_id": "c1",
                    "file_source": "src/worker.ts",
                    "chunk_index": 0,
                    "start_char": 0,
                    "end_char": 40,
                    "score": 0.87,
                    "retrieval": "dense",
                    "text": "export function handler() { /* secret code */ }",
                    "last_modified": "2026-06-01T00:00:00Z"
                },
                { "file_source": "src/db.ts" }
            ]
        });
        #[derive(serde::Deserialize)]
        struct Env {
            chunks: Vec<super::super::python_oracle::ContextChunk>,
        }
        let env: Env = serde_json::from_value(payload).expect("context payload deserializes");
        assert_eq!(env.chunks.len(), 2);
        assert_eq!(env.chunks[0].file_source, "src/worker.ts");
        assert!((env.chunks[0].score - 0.87).abs() < 1e-9);
        // Missing score defaults to 0.0; aggregation then ranks it last.
        assert_eq!(env.chunks[1].file_source, "src/db.ts");
        assert_eq!(env.chunks[1].score, 0.0);
        let ranked = aggregate_suspect_files(&env.chunks, 6);
        assert_eq!(
            ranked,
            vec!["src/worker.ts".to_string(), "src/db.ts".to_string()]
        );
    }

    #[test]
    fn oracle_failure_yields_empty_suspects_and_honest_note_never_errors() {
        // FAIL-CLOSED INVARIANT: an Oracle retrieval error must NOT propagate as a
        // command error (that would break the already-created card). It maps to a
        // `Failed` outcome → empty suspects + an honest, sanitized note. The card
        // stays intact.
        // Feed a raw error that carries a Windows user PATH (a real leak risk if the
        // note stored the raw string). The note's reason MUST be the CLASSIFIED,
        // sanitized `OracleError` message — not the raw text — so (a) it equals what
        // `from_python` produces and (b) the leaked path is redacted out of it.
        const RAW_WITH_PATH: &str =
            "Oracle HTTP request failed: Connection refused at C:\\Users\\gualt\\secret.json";
        let down = suspect_outcome_from_retrieval(Err(RAW_WITH_PATH.to_string()));
        match down {
            SuspectOutcome::Failed(reason) => {
                // The note carries EXACTLY the classified+sanitized message (pins the
                // production boundary, far stronger than a non-empty check).
                let expected =
                    crate::oracle::oracle_error::OracleError::from_python(RAW_WITH_PATH).message;
                assert_eq!(
                    reason, expected,
                    "the note reason must be the classified+sanitized Oracle message"
                );
                // It is a connection failure → classified as server-unavailable, and
                // the raw path/username is scrubbed (never reaches the project note).
                assert!(
                    !reason.contains("gualt") && !reason.contains("secret.json"),
                    "the raw path/username must be redacted from the note reason: {reason}"
                );
                assert!(!reason.is_empty(), "failure must carry an honest reason");
            }
            SuspectOutcome::Files(_) => panic!("a retrieval error must NOT yield suspect files"),
        }

        // A successful-but-empty retrieval is a clean "no suspects", NOT a failure
        // (so no scary note is recorded).
        let empty = suspect_outcome_from_retrieval(Ok(Vec::new()));
        match empty {
            SuspectOutcome::Files(files) => assert!(files.is_empty()),
            SuspectOutcome::Failed(_) => panic!("an empty index is not a failure"),
        }

        // A successful retrieval seeds the top-K paths.
        let seeded = suspect_outcome_from_retrieval(Ok(vec![
            chunk("src/worker.ts", 0.9),
            chunk("src/db.ts", 0.4),
        ]));
        match seeded {
            SuspectOutcome::Files(files) => assert_eq!(
                files,
                vec!["src/worker.ts".to_string(), "src/db.ts".to_string()]
            ),
            SuspectOutcome::Failed(_) => panic!("a successful retrieval must seed files"),
        }
    }

    // ------------------------------------------------------------------
    // Oracle-disabled gate tests (Step 4)
    // ------------------------------------------------------------------

    #[test]
    fn oracle_enabled_gate_rejects_when_disabled() {
        // When Oracle is disabled, the real gate must return the disabled error.
        let err = super::oracle_enabled_gate(false).expect_err("must error when disabled");
        assert!(err.contains("Oracle is disabled"), "unexpected message: {err}");
    }

    #[test]
    fn oracle_enabled_gate_passes_when_enabled() {
        // When Oracle is enabled, the real gate must not short-circuit.
        assert!(super::oracle_enabled_gate(true).is_ok());
    }



    /// The setup/install flow commands (`get_oracle_runtime_setup`,
    /// `install_oracle_runtime`) MUST keep calling the plain `require_graph_auth`
    /// (NOT `require_graph_auth_and_enabled`) so they work even when Oracle is
    /// disabled — that's how a user installs/repairs the runtime. This is a
    /// code-verification test: it reads the source to confirm the two setup
    /// command functions call `require_graph_auth`, not
    /// `require_graph_auth_and_enabled`. (A runtime test would require mocking
    //  subprocess/filesystem calls; the re-grep in Step 3 plus this static
    //  assertion is sufficient.)
    #[test]
    fn setup_commands_use_plain_require_graph_auth() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/oracle/commands.rs"),
        )
        .expect("must read commands.rs source");
        let lines: Vec<&str> = src.lines().collect();

        // Find a function by name, then search the next 20 lines for its auth call.
        let find_auth_call = |fn_name: &str| -> Option<&str> {
            for (i, line) in lines.iter().enumerate() {
                if line.contains(&format!("pub async fn {fn_name}")) {
                    // Scan the next 20 lines for the first `require_graph_auth` call.
                    for j in i..(i + 20).min(lines.len()) {
                        if lines[j].contains("require_graph_auth") {
                            return Some(lines[j]);
                        }
                    }
                }
            }
            None
        };

        let setup_call = find_auth_call("get_oracle_runtime_setup")
            .expect("get_oracle_runtime_setup not found in source");
        assert!(
            setup_call.contains("require_graph_auth(&auth_state)?;"),
            "get_oracle_runtime_setup must call plain require_graph_auth, got: {setup_call}"
        );
        assert!(
            !setup_call.contains("require_graph_auth_and_enabled"),
            "get_oracle_runtime_setup must NOT call require_graph_auth_and_enabled, got: {setup_call}"
        );

        let install_call = find_auth_call("install_oracle_runtime")
            .expect("install_oracle_runtime not found in source");
        assert!(
            install_call.contains("require_graph_auth(&auth_state)?;"),
            "install_oracle_runtime must call plain require_graph_auth, got: {install_call}"
        );
        assert!(
            !install_call.contains("require_graph_auth_and_enabled"),
            "install_oracle_runtime must NOT call require_graph_auth_and_enabled, got: {install_call}"
        );
    }
}

#[tauri::command]
pub async fn get_oracle_snapshot(
    auth_state: tauri::State<'_, BackendState>,
) -> Result<OracleSnapshot, OracleError> {
    require_oracle_auth(&auth_state)?;
    // P2: read-only status poll. Route through the bounded HTTP-only GET path
    // (probe → request) instead of `try_python_oracle`, so it NEVER consumes an
    // `/ask` permit and NEVER falls through to the heavy CLI subprocess. A burst of
    // boot polls therefore cannot exhaust the ask permits ("Oracle is busy") and a
    // not-yet-ready server returns a fast typed "starting" error, not a 165s stall.
    oracle_readonly_get::<OracleSnapshot>("/snapshot").await
}

/// Default / max for operator result-limit parameters (`ask_oracle`,
/// `get_oracle_similar`). Mirrors `ORACLE_INDEXED_FILES_MAX_LIMIT` style.
const ORACLE_RESULT_DEFAULT_LIMIT: usize = 8;
const ORACLE_RESULT_MAX_LIMIT: usize = 32;

/// Clamp an optional operator result limit to `[1, ORACLE_RESULT_MAX_LIMIT]`,
/// defaulting to `ORACLE_RESULT_DEFAULT_LIMIT`. Pure seam for unit tests.
pub(crate) fn clamp_oracle_result_limit(limit: Option<usize>) -> usize {
    limit
        .unwrap_or(ORACLE_RESULT_DEFAULT_LIMIT)
        .clamp(1, ORACLE_RESULT_MAX_LIMIT)
}

#[tauri::command]
pub async fn ask_oracle(
    auth_state: tauri::State<'_, BackendState>,
    query: String,
    limit: Option<usize>,
) -> Result<OracleAnswer, OracleError> {
    require_oracle_auth(&auth_state)?;

    // FIX: HTTP-only path. The Python subprocess `run_python_oracle` was deleted in M3;
    // all operator commands now address the in-process resident server via its HTTP
    // interface. The `try_python_oracle_with_llm` wrapper (which spawned a detached
    // worker around the deleted fn) is also gone — `ask_oracle` now POSTs directly to
    // `/ask` (the OPERATOR endpoint: full-corpus, operator token) on the resident
    // server, failing loud (STARTING / unreachable) when the server is not ready,
    // instead of silently falling through to a subprocess. NOT `/ask-bounded`: that
    // is the AGENT endpoint whose `allowed_file_ids` scope FAIL-CLOSES to empty when
    // absent — the operator ask would always answer "not found in corpus".
    let index_root = oracle_index_root()?;
    let limit = clamp_oracle_result_limit(limit);
    let body = serde_json::json!({
        "query": query,
        "limit": limit,
    });
    let llm_config = resolve_oracle_llm_runtime_config();
    let result: OracleAnswer = oracle_http_post_blocking::<OracleAnswer>(
        index_root,
        "/ask".to_string(),
        Some(body),
        llm_config,
    )
    .await
    .map_err(OracleError::from_python)?;
    Ok(result)
}

/// PURE aggregation of Oracle `/context` retrieval chunks into the top-`k` DISTINCT
/// suspect file paths. Deterministic by construction:
///
/// 1. Group chunks by `file_source`, keeping the MAX score seen for each file (a
///    file's relevance is its single best-matching chunk, not a sum that would bias
///    toward larger files with more chunks).
/// 2. Sort by score DESCENDING, then by file path ASCENDING as a stable, total
///    tie-break (so equal-score files have a reproducible order regardless of chunk
///    arrival order — no `HashMap` iteration order leaks in).
/// 3. Return the first `k` file paths.
///
/// Empty input (or `k == 0`) ⇒ empty output. NaN scores sort last (treated as the
/// lowest). Stores only the PATHS — the chunk `text` is never read here.
fn aggregate_suspect_files(chunks: &[super::python_oracle::ContextChunk], k: usize) -> Vec<String> {
    use std::collections::BTreeMap;
    // BTreeMap keeps the grouping itself deterministic (path-ordered) before the
    // score sort, so even the pre-sort state has no hash-order nondeterminism.
    let mut best: BTreeMap<&str, f64> = BTreeMap::new();
    for chunk in chunks {
        let file = chunk.file_source.trim();
        if file.is_empty() {
            continue;
        }
        best.entry(file)
            .and_modify(|score| {
                // `max` here treats NaN conservatively: if the stored score is NaN
                // a real score replaces it; a NaN incoming never overwrites a real
                // score (f64::max returns the non-NaN operand).
                *score = score.max(chunk.score);
            })
            .or_insert(chunk.score);
    }
    // Treat NaN as the LOWEST possible score so it sorts LAST in the DESC order
    // below (FIX 3). The bare `total_cmp` ranks +NaN ABOVE every finite value, which
    // contradicted the "NaN sorts last" contract and let a garbage NaN-only file
    // steal a top-K slot from a real-scored file. Mapping NaN → NEG_INFINITY before
    // the (still total, still deterministic) `total_cmp` fixes that. A file only
    // carries NaN here if EVERY one of its chunks scored NaN — `f64::max` in the
    // per-file fold already prefers any real score over NaN.
    let sort_score = |score: f64| {
        if score.is_nan() {
            f64::NEG_INFINITY
        } else {
            score
        }
    };
    let mut ranked: Vec<(&str, f64)> = best.into_iter().collect();
    ranked.sort_by(|(a_path, a_score), (b_path, b_score)| {
        // Score DESC (NaN sorts last), then path ASC. `total_cmp` over the
        // NaN-flattened scores gives a total order so the sort is deterministic.
        sort_score(*b_score)
            .total_cmp(&sort_score(*a_score))
            .then_with(|| a_path.cmp(b_path))
    });
    ranked
        .into_iter()
        .take(k)
        .map(|(path, _score)| path.to_string())
        .collect()
}

/// P2: localize a card's suspect files via Oracle RETRIEVAL (`/context`, NO LLM)
/// and persist the top-K distinct file paths onto the task. Called by the frontend
/// as a SEPARATE async best-effort step right AFTER `create_project_task` returns,
/// so card creation itself stays sync/fast and never waits on Oracle.
///
/// Runs for EVERY category (feature/hardening/bug/other) — every new card gets an
/// Oracle head-start; the bug-vs-other distinction only matters for the P3 Polis
/// smoke. `query` is the card's title (plus its description when present), supplied
/// by the caller.
///
/// FAIL-CLOSED, by design the card is ALREADY created before this runs:
/// * empty/blank query                → `Ok` with empty suspects, no Oracle call;
/// * Oracle down / no index / no key  → `Ok` with empty suspects + an honest
///   project note "Oracle could not localize suspects for task <id> (…)";
/// * retrieval returns zero chunks    → `Ok` with empty suspects (no note — the
///   index simply had nothing relevant; that is not a failure).
///
/// It NEVER returns an `Err` for an Oracle-side problem, so the UI can fire it
/// best-effort and ignore the outcome. The only `Err` paths are auth (locked vault)
/// and a project-store write failure, which the UI also tolerates.
///
/// PRIVACY: `/context` reuses the SAME gated path as the blurb/dossier; the query
/// text + retrieved code chunks go to the already-accepted Scaleway-GDPR Oracle. We
/// store ONLY file paths in `suspect_file_ids`, never any code text.
#[tauri::command]
pub async fn localize_card_suspects(
    app: tauri::AppHandle,
    auth_state: tauri::State<'_, BackendState>,
    project_id: String,
    task_id: String,
    query: String,
) -> Result<crate::backend::model::ProjectDetail, OracleError> {
    require_oracle_auth(&auth_state)?;

    // Resolve the suspect file paths via retrieval, fail-closed. `Files(..)` means
    // "persist these paths" (possibly empty when the index had nothing relevant, in
    // which case no failure note is warranted); `Failed(reason)` means "leave
    // suspects empty + record an honest failure note".
    let outcome = resolve_card_suspects(query).await;

    // Persist on a blocking worker (the project store does locked file I/O). The
    // suspects are file paths only.
    let app_for_write = app.clone();
    let project_id_for_write = project_id.clone();
    let task_id_for_write = task_id.clone();
    let detail = tauri::async_runtime::spawn_blocking(move || {
        // SAFETY: the command holds the unlock gate above; re-fetch the backend
        // state from the app handle inside the blocking closure (it is not Send via
        // the `State` guard). `state` lives on the `Manager` trait.
        use tauri::Manager as _;
        let state = app_for_write.state::<BackendState>();
        match &outcome {
            SuspectOutcome::Files(files) => crate::backend::projects::set_task_suspect_files(
                &app_for_write,
                &state,
                &project_id_for_write,
                &task_id_for_write,
                files.clone(),
            ),
            SuspectOutcome::Failed(reason) => {
                // Leave suspects empty; record an honest note. If even the note
                // write fails, fall back to returning the current detail so the
                // card is never reported as broken.
                crate::backend::projects::append_oracle_localization_failure_note(
                    &app_for_write,
                    &state,
                    &project_id_for_write,
                    &task_id_for_write,
                    reason,
                )
            }
        }
    })
    .await
    .map_err(|e| {
        OracleError::internal_sanitized(format!("Suspect localization task failed: {e}"))
    })?;

    detail.map_err(OracleError::internal_sanitized)
}

/// The fail-closed result of the retrieval step, mapped to a project-store action.
enum SuspectOutcome {
    /// Persist these file paths (possibly empty when the index had nothing relevant
    /// — that is a clean "no suspects", NOT a failure, so no note is recorded).
    Files(Vec<String>),
    /// Oracle could not be reached / had no index / no key: leave suspects empty and
    /// record this (sanitized) reason as an honest project note.
    Failed(String),
}

/// Run the `/context` retrieval + aggregation off the async worker, fail-closed.
/// Pure-ish seam around the blocking Oracle call so `localize_card_suspects` stays
/// readable. A blank query short-circuits to `Files(empty)` with no Oracle call.
async fn resolve_card_suspects(query: String) -> SuspectOutcome {
    let query = query.trim().to_string();
    if query.is_empty() {
        return SuspectOutcome::Files(Vec::new());
    }
    let root = match oracle_index_root() {
        Ok(root) => root,
        // No indexed workspace selected: honest note, empty suspects.
        Err(err) => return SuspectOutcome::Failed(err.message),
    };
    let chunks = tauri::async_runtime::spawn_blocking(move || {
        super::python_oracle::oracle_context_chunks(&root, &query, SUSPECT_CONTEXT_LIMIT)
    })
    .await;
    match chunks {
        // Join error (worker panic): treat as a fail-closed Oracle failure. Reuse
        // the sanitizing `Internal` constructor so no path/secret reaches the note.
        Err(e) => SuspectOutcome::Failed(
            OracleError::internal_sanitized(format!("retrieval task failed: {e}")).message,
        ),
        Ok(retrieval) => suspect_outcome_from_retrieval(retrieval),
    }
}

/// PURE fail-closed mapping from a `/context` retrieval result to a
/// [`SuspectOutcome`], extracted so the "Oracle error ⇒ empty suspects + honest
/// note, NEVER a hard error" contract is unit-testable without a Tauri runtime:
///
/// * `Ok(chunks)` → `Files(top-K)` (possibly empty when the index had nothing
///   relevant — a clean "no suspects", not a failure);
/// * `Err(raw)`   → `Failed(sanitized reason)` (suspects stay empty; the caller
///   records an honest project note). The reason is the classified
///   [`OracleError`] message, already path/secret-safe.
fn suspect_outcome_from_retrieval(
    retrieval: Result<Vec<super::python_oracle::ContextChunk>, String>,
) -> SuspectOutcome {
    match retrieval {
        Ok(chunks) => SuspectOutcome::Files(aggregate_suspect_files(&chunks, SUSPECT_FILE_TOP_K)),
        Err(raw) => SuspectOutcome::Failed(OracleError::from_python(raw).message),
    }
}

/// Resolve the Oracle LLM runtime config (provider/model/base_url/api_key) from
/// the vault, for injection into BOTH the resident-server spawn env and the
/// CLI-subprocess path.
///
/// Returns `None` when remote answering is disabled, when the vault is locked, or
/// when any vault read fails — it must NEVER panic and NEVER propagate an `Err`,
/// because it runs on the server-spawn path (under the start lock) where a failure
/// to read a key must simply degrade to "no key" (extractive), not abort the spawn.
///
/// Note: a `None` return and a returned config whose `api_key` is `None` are both
/// valid "no key" outcomes; the answerer treats a missing key as extractive. The
/// only difference is the resident server still inherits provider/model/flags when
/// a config is returned, which is harmless.
pub(crate) fn resolve_oracle_llm_runtime_config(
) -> Option<super::python_oracle::OracleLlmRuntimeConfig> {
    // LIFECYCLE EDGE CASE (server respawn while the vault is LOCKED): the resident
    // server now survives a vault lock and the supervisor may respawn it while the
    // vault is locked. In that state the vault read below fails and we DEGRADE to
    // defaults / no key (never panic), so the respawned server still serves
    // retrieval + bounded endpoints; only LLM-backed answers degrade to extractive
    // until the next unlock. The respawn is NOT blocked on an unlocked vault.
    // A vault read error (e.g. locked) must degrade to defaults, never panic.
    let llm_settings =
        vault::read_oracle_llm_settings().unwrap_or_else(|_| vault::default_oracle_llm_settings());
    if !llm_settings.remote_enabled {
        return None;
    }
    // Read the key from the vault, then apply the pure degradation policy below.
    // Keeping the policy in a separate, I/O-free helper makes the FINDING 4
    // ("a primary keyring error is observable, not silently identical to no-key")
    // invariant unit-testable without any vault state.
    let primary = resolve_oracle_llm_api_key(&llm_settings);
    Some(assemble_oracle_llm_runtime_config(&llm_settings, primary))
}

/// Pure degradation policy for the resident-server LLM config. Given the settings
/// and the vault-read result, build the runtime config without aborting the spawn:
///
/// * FINDING 4 — key: `Ok(Some)` → use it; `Ok(None)` → no key (silent); `Err` →
///   degrade to no key BUT warn-log the error (NEVER the key value) so a transient
///   keyring failure is observable rather than indistinguishable from "no key
///   configured". A missing key makes the answerer return an extractive answer —
///   the ONLY fallback (there is no LLM-to-LLM fallback).
///
/// I/O-free (no vault access) so the policy can be unit-tested directly.
fn assemble_oracle_llm_runtime_config(
    llm_settings: &crate::backend::model::OracleLlmSettings,
    primary: Result<Option<String>, String>,
) -> super::python_oracle::OracleLlmRuntimeConfig {
    let llm_api_key = match primary {
        Ok(key) => key,
        Err(e) => {
            eprintln!(
                "oracle: failed to read the LLM API key from the vault, degrading to \
                 extractive (no key): {e}"
            );
            None
        }
    };
    super::python_oracle::OracleLlmRuntimeConfig {
        provider: llm_settings.provider.clone(),
        model: llm_settings.model.clone(),
        base_url: llm_settings.base_url.clone(),
        api_key: llm_api_key,
    }
}

fn resolve_oracle_llm_api_key(
    llm_settings: &crate::backend::model::OracleLlmSettings,
) -> Result<Option<String>, String> {
    if !llm_settings.remote_enabled {
        return Ok(None);
    }
    resolve_llm_api_key_for_settings(llm_settings)
}

fn resolve_llm_api_key_for_settings(
    llm_settings: &crate::backend::model::OracleLlmSettings,
) -> Result<Option<String>, String> {
    if let Some(key) =
        vault::read_oracle_llm_api_key_for_settings(llm_settings).map_err(|e| e.to_string())?
    {
        return Ok(Some(key));
    }
    vault::read_llm_provider_token(&llm_settings.provider).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_oracle_node(
    auth_state: tauri::State<'_, BackendState>,
    node_id: String,
) -> Result<OracleNodeCard, OracleError> {
    require_oracle_auth(&auth_state)?;
    // P2: read-only detail lookup — bounded HTTP-only GET, no ask permit, no CLI
    // fallback. The node id is path-encoded server-side (`/node/{id}`).
    let path = format!("/node/{}", encode_oracle_path_segment(&node_id));
    oracle_readonly_get::<OracleNodeCard>(&path).await
}

#[tauri::command]
pub async fn get_oracle_similar(
    auth_state: tauri::State<'_, BackendState>,
    node_id: String,
    limit: Option<usize>,
) -> Result<Vec<OracleResult>, OracleError> {
    require_oracle_auth(&auth_state)?;

    // P2: read-only similarity lookup — bounded HTTP-only GET, no ask permit, no
    // CLI fallback. `/similar/{id}?limit=N`. Clamp limit server-side so a large
    // UI value cannot inflate the response.
    let limit = clamp_oracle_result_limit(limit);
    let path = format!(
        "/similar/{}?limit={}",
        encode_oracle_path_segment(&node_id),
        limit
    );
    oracle_readonly_get::<Vec<OracleResult>>(&path).await
}

#[tauri::command]
pub async fn get_oracle_duplicates(
    auth_state: tauri::State<'_, BackendState>,
) -> Result<Vec<OracleDuplicateLabel>, OracleError> {
    require_oracle_auth(&auth_state)?;
    // P2: read-only lookup — bounded HTTP-only GET, no ask permit, no CLI fallback.
    oracle_readonly_get::<Vec<OracleDuplicateLabel>>("/duplicate-labels").await
}

#[tauri::command]
pub async fn get_oracle_coverage(
    auth_state: tauri::State<'_, BackendState>,
) -> Result<OracleCoverage, OracleError> {
    require_oracle_auth(&auth_state)?;
    // P2: read-only status poll — bounded HTTP-only GET, no ask permit, no CLI
    // fallback. Part of the boot-poll burst, so keeping it off the ask permit is
    // what stops a refresh storm from starving a real ask.
    oracle_readonly_get::<OracleCoverage>("/coverage").await
}

#[tauri::command]
pub async fn get_oracle_runtime(
    auth_state: tauri::State<'_, BackendState>,
) -> Result<OracleRuntime, String> {
    require_graph_auth_and_enabled(&auth_state)?;

    // P2: this is the "Checking vector runtime…" boot step. Route it through the
    // bounded HTTP-only GET (probe → request): a not-ready server returns the fast
    // typed "starting" error within a few seconds instead of hanging on the 165s
    // hard cap, and it never consumes an `/ask` permit. Not part of the typed-
    // OracleError surface, so flatten the typed error into its message string.
    oracle_readonly_get::<OracleRuntime>("/runtime")
        .await
        .map_err(|e| e.message)
}

#[tauri::command]
pub async fn get_oracle_index_status(
    auth_state: tauri::State<'_, BackendState>,
) -> Result<serde_json::Value, String> {
    require_graph_auth_and_enabled(&auth_state)?;
    // FIX: address the SAME resident server the supervisor and `ask_oracle` use —
    // the one started against the WORKSPACE index root. Passing the package/code
    // root here (the old `require_oracle_root(&state)`) made
    // `ensure_rust_oracle_server` / `oracle_server_ready` never match the
    // supervisor-started server (whose `server_root` is the workspace), waiting
    // 60s then killing+respawning against the wrong root — a kill/restart loop
    // fighting the supervisor.
    let index_root = oracle_index_root().map_err(|e| e.message)?;
    oracle_http_get_blocking(
        index_root.clone(),
        format!("/index/status?{}", oracle_root_query(&index_root)),
    )
    .await
}

/// Server-side cap on the page size requested for `get_oracle_indexed_files`.
/// Mirrors the Python `MAX_INDEXED_FILES_LIMIT`; clamping here too keeps the
/// request URL bounded even if the UI sends a larger value.
const ORACLE_INDEXED_FILES_MAX_LIMIT: u32 = 500;

/// Build the `/index/files` query string from the (already-resolved) index root
/// and the optional UI paging/filter inputs. Pure seam (no I/O) so the clamp +
/// encoding can be unit-tested without a live Python server.
///
/// PRIVACY: the only path placed on the wire is the index ROOT (the workspace
/// the user selected), URL-encoded; the manifest file ids returned by the
/// server are already workspace-relative.
fn oracle_indexed_files_query(
    index_root: &std::path::PathBuf,
    limit: Option<u32>,
    offset: Option<u32>,
    filter: Option<&str>,
) -> String {
    let limit = limit
        .unwrap_or(100)
        .clamp(1, ORACLE_INDEXED_FILES_MAX_LIMIT);
    let offset = offset.unwrap_or(0);
    let mut query = format!(
        "{}&limit={}&offset={}",
        oracle_root_query(index_root),
        limit,
        offset,
    );
    if let Some(filter) = filter.map(str::trim).filter(|f| !f.is_empty()) {
        query.push_str(&format!("&filter={}", urlencoding::encode(filter)));
    }
    query
}

/// List the files recorded in the chunk-index manifest (bounded + paginated)
/// for the Oracle UI. Operator-gated (this is the app UI path, not the agent
/// token). Reads only the manifest server-side (no vectors loaded); the
/// returned `path`s are workspace-relative file ids, never absolute paths.
#[tauri::command]
pub async fn get_oracle_indexed_files(
    auth_state: tauri::State<'_, BackendState>,
    limit: Option<u32>,
    offset: Option<u32>,
    filter: Option<String>,
) -> Result<OracleIndexedFiles, OracleError> {
    require_oracle_auth(&auth_state)?;
    // FIX: the resident server is addressed by the WORKSPACE index root (the root
    // the supervisor + `ask_oracle` use), NOT the package/code root. Passing the
    // code root here made `oracle_server_ready` never match the supervisor-started
    // server and triggered a 60s-then-kill/respawn loop against the wrong root.
    // The same `index_root` is both the server address and the `?root=` query.
    let index_root = oracle_index_root()?;
    let query = oracle_indexed_files_query(&index_root, limit, offset, filter.as_deref());
    oracle_http_get_blocking::<OracleIndexedFiles>(index_root, format!("/index/files?{query}"))
        .await
        .map_err(OracleError::from_python)
}

/// Process-wide single-flight lock for the Oracle doctor. The doctor loads the
/// real ~1-2GB embedding model; a double-click (or two windows) firing
/// `get_oracle_doctor` concurrently would spawn two model loads and OOM a weak
/// machine. Uses the async `tauri::async_runtime::Mutex` (a tokio mutex re-export)
/// so the held guard can be held across the `.await` of the blocking doctor task.
///
/// COALESCE (not reject): concurrent callers serialize on `.lock().await` rather
/// than being rejected with a "already running" error. The second caller waits
/// for the in-flight run, then finds a fresh cached report and returns it without
/// re-running the heavy work (see `ORACLE_DOCTOR_CACHE` + `cached_doctor_if_fresh`).
static ORACLE_DOCTOR_LOCK: std::sync::OnceLock<tauri::async_runtime::Mutex<()>> =
    std::sync::OnceLock::new();

fn oracle_doctor_lock() -> &'static tauri::async_runtime::Mutex<()> {
    ORACLE_DOCTOR_LOCK.get_or_init(|| tauri::async_runtime::Mutex::new(()))
}

/// How long a completed doctor report is reused. Short enough that a manual
/// re-run after a fix reflects reality quickly, long enough that a burst of
/// concurrent callers (double-click, two windows) coalesce onto one heavy run.
const ORACLE_DOCTOR_CACHE_TTL: chrono::Duration = chrono::Duration::seconds(5);

/// Last completed doctor report + when it was produced. A plain `std::sync::Mutex`
/// (not the async one): it only guards a cheap clone/store and is never held
/// across an `.await`, so it cannot deadlock the runtime.
static ORACLE_DOCTOR_CACHE: std::sync::OnceLock<
    std::sync::Mutex<Option<(chrono::DateTime<chrono::Utc>, OracleDoctorReport)>>,
> = std::sync::OnceLock::new();

fn oracle_doctor_cache(
) -> &'static std::sync::Mutex<Option<(chrono::DateTime<chrono::Utc>, OracleDoctorReport)>> {
    ORACLE_DOCTOR_CACHE.get_or_init(|| std::sync::Mutex::new(None))
}

/// Pure freshness check over the cache state. Returns a clone of the cached
/// report iff it exists and is younger than `ttl` relative to `now`. Extracted as
/// a seam so the coalesce/cache decision is unit-testable without a live runtime
/// or a real clock. A negative `now - stored` (clock went backwards) is treated
/// as stale, never as "infinitely fresh".
fn cached_doctor_if_fresh(
    cache: &Option<(chrono::DateTime<chrono::Utc>, OracleDoctorReport)>,
    now: chrono::DateTime<chrono::Utc>,
    ttl: chrono::Duration,
) -> Option<OracleDoctorReport> {
    cache.as_ref().and_then(|(stored_at, report)| {
        let age = now - *stored_at;
        if age >= chrono::Duration::zero() && age < ttl {
            Some(report.clone())
        } else {
            None
        }
    })
}

/// The single source of truth for "is Oracle healthy?". Runs the Python doctor
/// (runtime / embedder / workspace / index checks) under the venv interpreter
/// off the UI thread, then OVERWRITES the placeholder `provider` check with a
/// boolean key-presence result resolved from the OS vault (the Python side
/// cannot read it). The merged report's overall `ok` is recomputed afterwards.
///
/// Single-flight (COALESCE, not reject): concurrent callers serialize on the
/// process-wide async mutex via `.lock().await` instead of being rejected. A
/// double-click / second window WAITS for the in-flight run, then finds the fresh
/// cached report and returns it WITHOUT spawning a second ~1-2GB model load (OOM
/// risk). The cache TTL (`ORACLE_DOCTOR_CACHE_TTL`) bounds how stale a coalesced
/// answer can be; past the TTL the next caller re-runs. The guard is held for the
/// whole run and released on drop — including every early-return / error path.
///
/// Privacy: the doctor's strings carry no paths/usernames, and the provider
/// merge records only whether a key resolved — never the key value.
#[tauri::command]
pub async fn get_oracle_doctor(
    auth_state: tauri::State<'_, BackendState>,
) -> Result<OracleDoctorReport, OracleError> {
    require_oracle_auth(&auth_state)?;

    // Serialize concurrent callers (don't reject): the second caller blocks here
    // until the in-flight run releases the guard, then proceeds — at which point
    // the cache below is fresh and it returns without re-running the heavy work.
    // Held for the whole run; released on drop, including every error path below.
    let _doctor_guard = oracle_doctor_lock().lock().await;

    // Coalesce: if a recent run already produced a report (within the TTL), reuse
    // it instead of paying for another model load. This is the payoff of waiting
    // on the lock above — a burst of callers collapses to ONE heavy run.
    if let Some(cached) = {
        let guard = oracle_doctor_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cached_doctor_if_fresh(&guard, chrono::Utc::now(), ORACLE_DOCTOR_CACHE_TTL)
    } {
        return Ok(cached);
    }

    // The indexed workspace root (the thing the doctor inspects). An unset/invalid
    // root surfaces as a typed NoWorkspaceRoot error — itself a useful doctor
    // signal the UI already handles.
    let index_root = oracle_index_root()?;

    // The code root that owns the `oracle` package + venv. Use the package finder
    // (not the data-ready finder) so the doctor runs even before the first index
    // exists. Thread the INDEX root in as the extra candidate hint, exactly like
    // `try_python_oracle_with_llm` does on the ask path — otherwise (debug builds
    // only, release ignores the hint) the doctor's candidate search can resolve a
    // DIFFERENT package root than ask when the dev cwd isn't the repo root,
    // reporting "runtime not installed" while asks succeed.
    let code_root = find_oracle_package_root(Some(index_root.clone())).ok_or_else(|| {
        OracleError::server_unavailable("The Oracle Python runtime is not installed.")
    })?;

    // The doctor is now Rust-native (M3): `model_present` probes the bundled
    // embedder ONCE up-front (cheap, no model-load), then a live `/runtime`
    // probe confirms the resident server is ready-to-answer. This is the "truthful
    // shape" — a green doctor means Oracle can actually answer `/ask` right now.
    // Run off the async worker (it touches the filesystem and does a blocking
    // reqwest probe).
    let probe_root = index_root.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
        super::python_oracle::run_python_oracle_doctor(&code_root, &index_root)
    })
    .await
    .map_err(|e| OracleError::internal_sanitized(format!("Oracle doctor task failed: {e}")))??;

    // Fill the LIVE-server check the Python placeholder left for us: probe the
    // resident server's (now-fast) /health + /runtime for a ready CHUNK store.
    // This is what makes a green doctor HONEST — the data-layer checks can be
    // green while the live server is unreachable or its index is not ready, in
    // which case Oracle cannot actually answer. Blocking reqwest, so off the
    // async worker. A task-join failure degrades to "unreachable" (red) rather
    // than crashing the doctor.
    let live_probe =
        tauri::async_runtime::spawn_blocking(move || probe_oracle_live_server(&probe_root))
            .await
            .unwrap_or(crate::oracle::oracle_error::LiveServerProbe::Unreachable);
    let report = merge_live_server_check(report, live_probe);

    // Fill the provider check the Python placeholder left for us: resolve (but
    // never surface) whether an Oracle LLM API key is configured for the active
    // provider.
    let key_present = oracle_provider_key_present();
    let merged = merge_provider_check(report, key_present);

    // Cache the FINAL merged report so the next coalesced caller returns the exact
    // shape the UI consumes (provider check included). Cheap clone under a plain
    // mutex never held across an await.
    {
        let mut guard = oracle_doctor_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Some((chrono::Utc::now(), merged.clone()));
    }

    Ok(merged)
}

/// Resolve whether an Oracle LLM API key is configured for the active provider.
/// Returns only a boolean — the key value never leaves the vault layer.
///
/// Bug B fix: this MUST be the same computation the Settings status command uses,
/// otherwise the doctor can report "no provider key" while the status reports
/// configured. It now delegates to the single source of truth
/// `vault::oracle_llm_api_key_present` (derived from `oracle_llm_settings_status`)
/// instead of the divergent `resolve_oracle_llm_api_key` path. Any vault read
/// failure degrades to `false` (treated as "not configured"), which the UI
/// surfaces as an actionable remediation rather than a crash.
fn oracle_provider_key_present() -> bool {
    vault::oracle_llm_api_key_present()
}

#[tauri::command]
pub async fn sync_oracle_text_chunks(
    auth_state: tauri::State<'_, BackendState>,
) -> Result<serde_json::Value, String> {
    require_graph_auth_and_enabled(&auth_state)?;
    // FIX: address the workspace-root resident server (supervisor/`ask_oracle`),
    // not the package/code root, to avoid the wrong-root kill/respawn loop.
    let index_root = oracle_index_root().map_err(|e| e.message)?;
    oracle_http_post_blocking(
        index_root.clone(),
        format!("/index/sync?{}", oracle_root_query(&index_root)),
        None,
        None,
    )
    .await
}

/// Bring up the resident Oracle HTTP server (if needed) before operator index
/// actions. Runs on a blocking pool so a cold Metal/CPU model load does not
/// stall the async runtime. Surfaces a clear error instead of a dead POST.
async fn ensure_oracle_http_ready() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        crate::backend::oracle_service::ensure_resident_server_now()
    })
    .await
    .map_err(|e| format!("Oracle ensure task join: {e}"))?
}

#[tauri::command]
pub async fn start_oracle_index_job(
    auth_state: tauri::State<'_, BackendState>,
    force: Option<bool>,
    max_batches: Option<usize>,
    idle: Option<bool>,
    manual: Option<bool>,
) -> Result<serde_json::Value, String> {
    require_graph_auth_and_enabled(&auth_state)?;
    // FIX: address the workspace-root resident server (supervisor/`ask_oracle`),
    // not the package/code root, to avoid the wrong-root kill/respawn loop.
    let index_root = oracle_index_root().map_err(|e| e.message)?;
    // Do not POST into a dead ghost server — start (or restart) first.
    ensure_oracle_http_ready().await?;
    // FIX 2: when the user clicks "Index now" (manual=true) the server forces
    // idle=false + unbounded batches so the whole workspace indexes immediately
    // instead of being deferred by the idle RAM floor or capped to one batch
    // (which left the UI stuck at 0%). The auto warm-on-unlock path passes
    // manual=false and keeps the opportunistic idle/single-batch behavior.
    let path = format!(
        "/index/run?{}&force={}&max_batches={}&idle={}&manual={}&background=true",
        oracle_root_query(&index_root),
        force.unwrap_or(false),
        max_batches.unwrap_or(1),
        idle.unwrap_or(true),
        manual.unwrap_or(false),
    );
    oracle_http_post_blocking(index_root, path, None, None).await
}

#[tauri::command]
pub async fn start_oracle_index_watcher(
    auth_state: tauri::State<'_, BackendState>,
) -> Result<serde_json::Value, String> {
    require_graph_auth_and_enabled(&auth_state)?;
    // FIX: address the workspace-root resident server (supervisor/`ask_oracle`),
    // not the package/code root, to avoid the wrong-root kill/respawn loop.
    let index_root = oracle_index_root().map_err(|e| e.message)?;
    ensure_oracle_http_ready().await?;
    oracle_http_post_blocking(
        index_root.clone(),
        format!("/index/watch/start?{}", oracle_root_query(&index_root)),
        None,
        None,
    )
    .await
}

#[tauri::command]
pub async fn stop_oracle_index_watcher(
    auth_state: tauri::State<'_, BackendState>,
) -> Result<serde_json::Value, String> {
    require_graph_auth_and_enabled(&auth_state)?;
    // FIX: address the workspace-root resident server (supervisor/`ask_oracle`),
    // not the package/code root, to avoid the wrong-root kill/respawn loop.
    let index_root = oracle_index_root().map_err(|e| e.message)?;
    let result = oracle_http_post_blocking(index_root, "/index/watch/stop".to_string(), None, None).await;
    // FIX: re-arm the supervisor's one-shot watcher flag on a successful manual
    // stop, so the next supervisor tick can re-start the watcher (respecting the
    // autoWatch pref). Without this, the one-shot stays armed and the watcher is
    // never restarted for the rest of the session.
    if result.is_ok() {
        crate::backend::oracle_service::reset_watcher_armed();
    }
    result
}
