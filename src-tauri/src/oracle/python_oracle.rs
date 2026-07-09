use serde::de::DeserializeOwned;
use sha2::Digest;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const PYTHON_ORACLE_TIMEOUT: Duration = Duration::from_secs(90);
/// Absolute upper bound the async command layer enforces on a SINGLE Oracle call
/// (`try_python_oracle_with_llm`). The blocking `run_python_oracle` is internally
/// bounded by `ensure_oracle_server` (one `ORACLE_SERVER_START_TIMEOUT` wait, a
/// spawn, then another wait) followed by a `PYTHON_ORACLE_TIMEOUT` request, so a
/// well-behaved call returns well within this. The async wrapper still wraps the
/// JoinHandle in this cap so a pathological combination (repeated readiness
/// failures, a stalled fallback, a wedged child) can NEVER leave the UI on
/// "Querying Oracle…" forever — it surfaces a typed `ServerUnavailable` instead.
///
/// Sized as one server-start wait + one request PLUS a fixed headroom: a VALID
/// cold path (model load near `ORACLE_SERVER_START_TIMEOUT` + a remote-LLM answer
/// near `PYTHON_ORACLE_TIMEOUT`) must NOT be falsely killed. F5: the +15s headroom
/// covers the readiness-poll cadence, process spawn/handshake and clock slack so a
/// legitimately slow-but-progressing call still returns. This MUST stay ABOVE the
/// cooperative-cancellation worst case in [`run_python_oracle`]: once this cap
/// fires the outer wrapper sets `cancel=true`, and the worker bails before the
/// expensive CLI-subprocess fallback (and the second in-lock HTTP retry was
/// removed). W1: the worker's HTTP `/ask` requests are non-interruptible
/// `reqwest::blocking` calls, but each one's timeout is BUDGETED against this same
/// deadline (`min(PYTHON_ORACLE_TIMEOUT, remaining)`), so a doomed worker overruns
/// the cap only by clock slack — not by a full `PYTHON_ORACLE_TIMEOUT`. The
/// CLI-subprocess fallback is cancel-aware (killed within one poll tick), so it too
/// stops promptly. See `try_python_oracle_with_llm`.
pub const ORACLE_CALL_HARD_TIMEOUT: Duration = Duration::from_secs(
    ORACLE_SERVER_START_TIMEOUT.as_secs() + PYTHON_ORACLE_TIMEOUT.as_secs() + 15,
);
/// The doctor loads the REAL embedding model (check 2), which can take ~30-60s
/// cold. Give it a wider budget than the regular data calls.
const ORACLE_DOCTOR_TIMEOUT: Duration = Duration::from_secs(120);
const ORACLE_SERVER_START_TIMEOUT: Duration = Duration::from_secs(60);
const ORACLE_SERVER_HEALTH_TIMEOUT: Duration = Duration::from_secs(5);
const ORACLE_STOP_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Finding 1: absolute upper bound on the TOTAL wall-clock AGE of a resident-server
/// child while it is still `StillStarting` (alive, no RootMismatch, but `/health`
/// has never answered). Distinct from `ORACLE_SERVER_START_TIMEOUT` (the per-wait
/// budget for ONE [`wait_for_oracle_server_ready`] episode): a healthy-but-slow cold
/// boot legitimately overruns the per-wait timeout across several supervisor ticks,
/// and we must NOT kill it (that is the restart loop this whole design avoids). But
/// a child that is genuinely WEDGED — alive, maybe holding the port, yet never
/// answering `/health` (e.g. a CUDA-init deadlock or a hung model load) — would
/// otherwise be kept FOREVER, leaving the app permanently on "Oracle is starting"
/// with no recovery.
///
/// 300s is deliberately GENEROUS: a pre-installed embedding model cold-loads well
/// under this even on a cold disk / contended CPU (the regular per-call cold budget
/// is ~60s start + ~90s answer); only a truly stuck child reaches 300s. So a
/// progressing boot (<300s) is never force-killed, while a permanently-stuck child
/// is force-replaced once past 300s instead of wedging "starting" indefinitely.
///
/// Measured against a MONOTONIC [`Instant`] spawn stamp (see [`ORACLE_CHILD_SPAWN`]),
/// NOT wall-clock time, so a system sleep/resume cannot make a healthy child look
/// hung (the monotonic clock does not advance while suspended on the platforms we
/// target).
const ORACLE_HUNG_CHILD_TIMEOUT: Duration = Duration::from_secs(300);

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug)]
pub(crate) struct ProcessOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

struct OracleHttpSession {
    base_url: String,
    port: u16,
    auth_token: String,
}

static ORACLE_HTTP_SESSION: OnceLock<OracleHttpSession> = OnceLock::new();
/// The AGENT auth token (`ORACLE_AGENT_AUTH_TOKEN`), distinct from the OPERATOR
/// token in [`OracleHttpSession`]. It authorizes ONLY the `/ask-bounded` and
/// `/context-bounded` endpoints and is the token published to MCP thin-clients
/// via the discovery file — never the operator token. Generated once per process
/// (same RNG/length as the operator token) and injected into the server's spawn
/// env so both tiers are live for the unlocked session. See `oracle_service`.
static ORACLE_AGENT_TOKEN: OnceLock<String> = OnceLock::new();
/// One shared blocking HTTP client for every Oracle call. A `reqwest::blocking`
/// client owns an internal runtime; dropping it inside a tokio async context
/// panics ("Cannot drop a runtime in a context where blocking is not allowed").
/// Storing it in a `static` means it is only ever dropped at process exit on the
/// main thread, never on an async worker. Per-call timeouts are applied on the
/// `RequestBuilder` instead of the client, so a single shared client serves the
/// 90s data calls, the readiness probe and the 2s stop request alike.
static ORACLE_HTTP_CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
static ORACLE_CHILD: OnceLock<Mutex<Option<Child>>> = OnceLock::new();
/// Finding 1: the MONOTONIC wall-clock stamp of when the tracked resident-server
/// child in [`ORACLE_CHILD`] was spawned. Set in [`spawn_oracle_server`] right after
/// the child handle is stored and cleared in [`kill_oracle_child`] alongside the
/// child, so it is always in lock-step with the child slot. Used to bound the TOTAL
/// age of a `StillStarting` child (see [`ORACLE_HUNG_CHILD_TIMEOUT`]): an `Instant`
/// (NOT a wall-clock `Date`) so a system suspend/resume cannot spuriously age a
/// healthy child past the hung timeout.
static ORACLE_CHILD_SPAWN: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
static ORACLE_SERVER_START_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static ORACLE_CLI_FALLBACK_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
#[cfg(test)]
static ORACLE_SERVER_SPAWN_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Env var that tells the resident Oracle server NOT to self-exit on its idle
/// timer (the app supervises its lifecycle). FIX 1: this MUST match the key the
/// Python config reads verbatim — `oracle/config.py` does
/// `os.getenv("ORACLE_DISABLE_IDLE_EXIT", ...)`. A mismatch (the old, wrong
/// `DISABLE_IDLE_EXIT`) silently left idle-exit enabled, so the resident server
/// self-terminated on its idle timer and the supervisor restart-looped, causing
/// spurious ServerUnavailable. Defined as a const so the set-site and the
/// Python-name assertion test cannot drift apart.
const ORACLE_DISABLE_IDLE_EXIT_ENV: &str = "ORACLE_DISABLE_IDLE_EXIT";

/// FIX 2: surfaced (instead of an opaque "exited before it became ready") when the
/// `oracle/` package/import root cannot be located, so the UI shows the real cause
/// rather than a doomed-process symptom. Shared by the server spawn, the CLI
/// fallback and the warmup probe so the message cannot drift between them.
const MISSING_PACKAGE_ROOT_ERROR: &str = "Could not locate the Oracle package (PYTHONPATH).";

/// P1: returned by the NON-spawning command-path readiness gate
/// ([`require_oracle_server_ready`]) when the resident server is not (yet) ready.
/// The SUPERVISOR is the sole spawner of the resident server, so a command must
/// never bring it up itself (that was the second racing spawn that collided on
/// the held session port and drove the restart loop). The phrasing contains
/// "server is not" so [`crate::oracle::oracle_error::OracleError::from_python`]
/// classifies it as `ServerUnavailable` — a fast, typed "try again" rather than a
/// 165s hard-cap stall. The supervisor brings the server up within seconds.
pub(crate) const ORACLE_SERVER_STARTING_ERROR: &str =
    "Oracle is starting — the server is not ready yet. Try again in a moment.";

/// Returned by [`ensure_oracle_server`] when the calling supervisor's stop flag was
/// observed mid bring-up: a NEWER supervisor (set the old one's stop flag in
/// [`crate::backend::oracle_service::start_supervisor`]) or a lock teardown has taken
/// over, so the stopping supervisor abandons the (re)spawn and releases the start
/// lock PROMPTLY rather than running to completion and racing a second spawn. This is
/// not a user-facing error — only the supervisor calls `ensure_oracle_server`, and it
/// ignores the result; the constant exists so the abort is an explicit, greppable
/// outcome distinct from a genuine startup failure.
pub(crate) const ORACLE_SERVER_ABORTED_ERROR: &str =
    "Oracle server bring-up aborted: a newer supervisor took over.";

#[derive(Clone)]
pub struct OracleLlmRuntimeConfig {
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
}

/// FINDING 2: a MANUAL `Debug` that renders `api_key` as `"[redacted]"` (when
/// `Some`) / `None` (when absent), never the secret. The derived `Debug` would dump
/// the plaintext key into the world-readable `oracle-server.stderr.log` on any
/// `format!("{:?}", …)` or panic. Non-secret fields are shown normally.
impl std::fmt::Debug for OracleLlmRuntimeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OracleLlmRuntimeConfig")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("api_key", &redacted_api_key(&self.api_key))
            .finish()
    }
}

/// Render an optional API key for `Debug` WITHOUT exposing the secret: the literal
/// `[redacted]` placeholder when a key is present, `None` when absent. Returned as a
/// type whose `Debug` matches `Option<&str>` so the field reads naturally.
fn redacted_api_key(api_key: &Option<String>) -> Option<&'static str> {
    api_key.as_ref().map(|_| "[redacted]")
}

/// Whether `root` holds BOTH the `oracle/` package source AND a complete index
/// (sqlite + vectors). Test-only: Bug A removed the production caller (it was the
/// upfront gate in `run_python_oracle` that wrongly blocked Ask, since the
/// workspace/index root never holds the package). The live integration tests
/// still use it to decide whether the local checkout can serve a real snapshot,
/// so it is gated to `#[cfg(test)]` to avoid a dead-code warning in app builds.
#[cfg(test)]
pub fn python_oracle_available(root: &Path) -> bool {
    root.join("oracle").join("cli.py").exists()
        && root.join("oracle-data").join("metadata.sqlite").exists()
        && oracle_vector_path(root).is_some()
}

fn oracle_vector_path(root: &Path) -> Option<PathBuf> {
    let lance = root.join("oracle-data").join("vectors.lancedb");
    if lance.exists() {
        return Some(lance);
    }
    let json = root.join("oracle-data").join("vectors.json");
    if json.exists() {
        return Some(json);
    }
    None
}

/// The bundled `oracle/` package root, resolved once at startup from the Tauri
/// resource directory. This is the only trusted code location in a release
/// build — see `set_bundled_oracle_root`.
static BUNDLED_ORACLE_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Record the bundled package root from the app's resource directory. Resources
/// declared as `["../oracle"]` are staged by Tauri under `<resource_dir>/_up_`,
/// so the package root (the dir that *contains* `oracle/`) is `<resource_dir>/_up_`.
/// Call once from the Tauri setup hook.
pub fn set_bundled_oracle_root(resource_dir: &Path) {
    // Tauri may stage `../oracle` either under `<resource_dir>/_up_/oracle`
    // (path-preserving) or flattened to `<resource_dir>/oracle`; accept whichever
    // actually holds the package.
    for candidate in [resource_dir.join("_up_"), resource_dir.to_path_buf()] {
        if oracle_package_present(&candidate) {
            let _ = BUNDLED_ORACLE_ROOT.set(candidate);
            return;
        }
    }
}

fn bundled_oracle_root() -> Option<PathBuf> {
    BUNDLED_ORACLE_ROOT.get().cloned()
}

/// The WRITABLE root that owns the installed runtime (`oracle-data/venv`). It is
/// deliberately SEPARATE from the package/import root resolved by
/// [`find_oracle_package_root`]:
/// * the package root may be a read-only bundled resource (release) — the venv
///   must be created somewhere writable;
/// * in dev the staged bundled copy under `target/.../_up_` has the package
///   source but NO venv, while the source repo has the real, installed venv.
///
/// In a RELEASE build the data root MUST be an app-data dir (writable, not a
/// user-droppable code dir). It is recorded once at startup via
/// [`set_oracle_data_root`] from the Tauri setup hook, mirroring
/// [`set_bundled_oracle_root`].
static ORACLE_DATA_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Record the writable Oracle data root (where `oracle-data/venv` lives/will be
/// created). RELEASE: pass the app-data dir so the venv is never written into the
/// read-only bundled resource. Call once from the Tauri setup hook. Only invoked
/// in release builds (dev resolves the data root via the candidate search), hence
/// `dead_code` in a dev/test compile.
#[cfg_attr(debug_assertions, allow(dead_code))]
pub fn set_oracle_data_root(dir: &Path) {
    let _ = ORACLE_DATA_ROOT.set(dir.to_path_buf());
}

/// True when `root/oracle-data/venv` exists in ANY state (complete OR partial).
/// Used to PREFER a root that already owns a venv (even a half-installed one we
/// can repair) over one that has only the package source. Distinct from
/// `oracle_setup::venv_complete`, which additionally requires the completion
/// marker; here we want to find the venv's HOME, not judge its readiness.
fn root_has_venv(root: &Path) -> bool {
    let venv = super::oracle_setup::oracle_venv_dir(root);
    venv.exists() || super::oracle_setup::venv_python(&venv).exists()
}

/// A path that is a bundled/staged READ-ONLY code location and must never be
/// chosen as the WRITABLE data root: the recorded `BUNDLED_ORACLE_ROOT` (or one
/// of its ancestors), or a path matching the actual Cargo/Tauri staging SHAPE.
/// Installing a venv at such a path would either fail (release, read-only) or
/// pollute the build tree (dev).
///
/// FIX 4: match the staging shape, not any component literally named `target`.
/// A user dir like `C:\Users\alice\target\Aspis Management` is legitimate and
/// must NOT be excluded. We only treat a path as staged when it is the recorded
/// bundle root / an ancestor of it, OR it contains a `target` component
/// IMMEDIATELY followed by `debug` / `release` / `bundle` (the Cargo build-profile
/// layout), OR it contains the Tauri `_up_` staging component. Conservative by
/// design: better to mis-classify a real staging dir as staged than a user dir.
fn is_bundled_or_staged_root(root: &Path) -> bool {
    if let Some(bundled) = bundled_oracle_root() {
        // The recorded bundle root itself, or anything UNDER it (its staged
        // subtree). MUST be `root.starts_with(bundled)`, NOT the reverse: the
        // bundle (`…/target/debug/_up_`) is NESTED INSIDE the source repo, so
        // excluding *ancestors* of the bundle would wrongly exclude the repo root
        // itself — the very place that owns `oracle-data/venv` in dev — and force
        // a fallback to the deps-less system Python.
        if root.starts_with(&bundled) {
            return true;
        }
    }
    let components: Vec<&std::ffi::OsStr> = root
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    components.windows(2).any(|pair| {
        pair[0] == "target" && (pair[1] == "debug" || pair[1] == "release" || pair[1] == "bundle")
    }) || components.iter().any(|name| *name == "_up_")
}

/// Resolve the WRITABLE data/runtime root that owns `oracle-data/venv`.
///
/// RELEASE (FIX 1, fail closed): the ONLY accepted value is the app-data dir
/// recorded via [`set_oracle_data_root`] at startup. The environment is NEVER
/// scanned — no candidate set is even built — so a user-droppable code dir can
/// never become the trusted data/venv root (the RCE vector). If nothing was
/// recorded, returns `None` (Oracle stays unavailable). See [`resolve_data_root`].
///
/// DEV: a recorded root still wins if present; otherwise the candidate search runs
/// (must be CONSISTENT between install and resolve so the venv is created and read
/// at the same place):
/// 1. Among the SAME candidate set as the package finder, prefer the NON-staged
///    candidate that ALREADY contains `oracle-data/venv` (complete or partial). In
///    dev this is the source repo `Aspis Management`, never the staged `_up_` copy.
/// 2. Fresh install (no venv anywhere yet): pick the best candidate that holds the
///    `oracle/` package source AND is writable — i.e. NOT the bundled/staged
///    read-only path. In dev this is again the source repo.
pub fn oracle_data_root() -> Option<PathBuf> {
    let recorded = ORACLE_DATA_ROOT.get();
    let candidates = if cfg!(debug_assertions) {
        oracle_root_candidates(None)
    } else {
        // RELEASE: never scan the environment for a writable data root (see
        // [`resolve_data_root`]); the recorded app-data dir is the only trusted
        // source. Pass no candidates so the fail-closed rule cannot fall through.
        Vec::new()
    };
    resolve_data_root(
        recorded.map(PathBuf::as_path),
        !cfg!(debug_assertions),
        &candidates,
    )
}

/// Pure release-vs-dev decision for the writable data/runtime root, extracted so
/// the security-critical fail-closed rule is unit-testable without `#[cfg]`.
///
/// SECURITY (FIX 1 — release RCE): a RELEASE build MUST NOT derive the writable
/// data root from the live environment (cwd, exe ancestors, `ASPIS_MANAGEMENT_ROOT`,
/// `~/Desktop/...`, etc.). Those are user-droppable locations; selecting one as the
/// data root would let `install_oracle_runtime` create a trusted venv there and run
/// `pip install` against attacker-controlled code = RCE. So in release the recorded
/// app-data dir (set once at startup from the Tauri setup hook) is the ONLY accepted
/// value; if it was never recorded, we return `None` (fail closed) rather than fall
/// back to the candidate search.
///
/// * release + recorded  → the recorded root, verbatim.
/// * release + none      → `None` (NEVER the candidate search).
/// * dev (any)           → recorded if present, else the candidate search, so the
///   source repo keeps resolving as the data root in development.
fn resolve_data_root(
    recorded: Option<&Path>,
    is_release: bool,
    candidates: &[PathBuf],
) -> Option<PathBuf> {
    if let Some(recorded) = recorded {
        return Some(recorded.to_path_buf());
    }
    if is_release {
        return None;
    }
    select_data_root(candidates)
}

/// Pure candidate-selection step of [`oracle_data_root`] (extracted so it can be
/// unit-tested with a fixed candidate set, since the full resolver scans the live
/// environment). Encodes rules 2 and 3 from [`oracle_data_root`]:
/// 1. PREFER the first NON-staged candidate that already owns `oracle-data/venv`
///    (the real, installed runtime).
/// 2. ELSE pick the first NON-staged writable package-source candidate.
///
/// FIX 3: the `!is_bundled_or_staged_root` exclusion applies to BOTH rules. A
/// staged/bundled path (e.g. a stale `target/.../_up_/oracle-data/venv` left by an
/// old install) must NEVER be chosen as the writable data root — venv or not —
/// because installing/writing there fails (release, read-only) or pollutes the
/// build tree (dev), and it re-introduces the staged-copy bug class.
fn select_data_root(candidates: &[PathBuf]) -> Option<PathBuf> {
    if let Some(with_venv) = candidates
        .iter()
        .find(|c| root_has_venv(c) && !is_bundled_or_staged_root(c))
    {
        return Some(with_venv.clone());
    }
    candidates
        .iter()
        .find(|c| oracle_package_present(c) && !is_bundled_or_staged_root(c))
        .cloned()
}

/// The ordered, de-duplicated set of roots to probe for a Python Oracle, shared
/// by the data-ready finder and the lighter package-root finder used by setup.
/// The bundled, read-only code location is probed first.
fn oracle_root_candidates(graph_root: Option<PathBuf>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(bundled) = bundled_oracle_root() {
        candidates.push(bundled);
    }
    if let Ok(root) = std::env::var("ASPIS_MANAGEMENT_ROOT") {
        add_candidate_with_ancestors(&mut candidates, PathBuf::from(root));
    }
    if let Ok(root) = std::env::var("ORACLE_INDEX_ROOT") {
        add_candidate_with_ancestors(&mut candidates, PathBuf::from(root));
    }
    if let Some(root) = graph_root {
        add_candidate_with_ancestors(&mut candidates, root);
    }
    if let Ok(cwd) = std::env::current_dir() {
        add_candidate_with_ancestors(&mut candidates, cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        add_candidate_with_ancestors(&mut candidates, exe);
    }
    add_default_user_roots(&mut candidates);
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter_map(|candidate| candidate.canonicalize().ok())
        .filter(|candidate| seen.insert(candidate.clone()))
        .collect()
}

/// Find the root that contains the `oracle/` Python package, regardless of
/// whether the index data (`oracle-data/`) has been built yet. Used by the
/// runtime setup/bootstrap, which runs *before* the first index exists.
///
/// Security: setup runs `pip install` and imports the package, so in a RELEASE
/// build it is locked to the bundled, read-only code — never a user-writable
/// "drop" dir. (The runtime path keeps the wider search so a release exe run
/// against an existing local checkout still works.) Dev keeps the wide search.
pub fn find_oracle_package_root(graph_root: Option<PathBuf>) -> Option<PathBuf> {
    #[cfg(not(debug_assertions))]
    {
        let _ = graph_root;
        return bundled_oracle_root().filter(|root| oracle_package_present(root));
    }
    #[cfg(debug_assertions)]
    {
        oracle_root_candidates(graph_root)
            .into_iter()
            .find(|candidate| oracle_package_present(candidate))
    }
}

pub fn oracle_package_present(root: &Path) -> bool {
    root.join("oracle").join("cli.py").exists()
        && root.join("oracle").join("requirements.txt").exists()
}

fn add_candidate_with_ancestors(candidates: &mut Vec<PathBuf>, path: PathBuf) {
    let start = if path.extension().is_some() {
        path.parent().map(Path::to_path_buf).unwrap_or(path)
    } else {
        path
    };
    candidates.push(start.clone());
    candidates.extend(start.ancestors().skip(1).map(Path::to_path_buf));
}

fn add_default_user_roots(candidates: &mut Vec<PathBuf>) {
    // `USERPROFILE` is Windows-only; macOS/Linux use `HOME`. Without the fallback
    // the user-folder probe is silently empty on Mac (mirrors vault.rs).
    let Some(profile) = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
    else {
        return;
    };
    for base in ["Desktop", "Documents", "Downloads"] {
        add_candidate_with_ancestors(candidates, profile.join(base).join("Aspis Management"));
    }
}

pub fn parse_python_oracle_json<T: DeserializeOwned>(stdout: &str) -> Result<T, String> {
    serde_json::from_str(stdout).map_err(|e| format!("Python Oracle output was invalid: {e}"))
}

fn oracle_http_session() -> &'static OracleHttpSession {
    ORACLE_HTTP_SESSION.get_or_init(|| {
        let port = random_oracle_port();
        OracleHttpSession {
            base_url: format!("http://127.0.0.1:{port}"),
            port,
            auth_token: random_token(),
        }
    })
}

/// The process-wide AGENT auth token (bounded-only). Generated lazily with the
/// SAME RNG/length as the operator token. Published in the discovery file; never
/// returned to the operator/UI path.
pub(crate) fn oracle_agent_token() -> &'static str {
    ORACLE_AGENT_TOKEN.get_or_init(random_token)
}

/// The resident server's loopback base URL (`http://127.0.0.1:<port>`) and port,
/// used by `oracle_service` to build the discovery file. The base URL is always
/// loopback by construction (see [`oracle_http_session`]).
pub(crate) fn oracle_session_endpoint() -> (String, u16) {
    let session = oracle_http_session();
    (session.base_url.clone(), session.port)
}

/// Build (once) and return the shared blocking HTTP client. The client itself
/// carries NO timeout; each request sets its own via `.timeout(...)`. Building
/// fails only if reqwest cannot construct a client, which is fatal regardless,
/// so we expect() — the `static` guarantees it is never dropped in async.
fn oracle_http_client() -> &'static reqwest::blocking::Client {
    ORACLE_HTTP_CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .build()
            .expect("failed to build Oracle blocking HTTP client")
    })
}

/// A3 (pure, unit-testable): from a parsed chunk-index manifest, the canonical `index_root`,
/// and the mini's `project_root` (both already verbatim-stripped), return the file_ids
/// (project-relative POSIX paths) that fall UNDER `project_root`. `project_root == index_root`
/// => all indexed files. Reads `manifest["roots"][index_root_str]["files"]` keys, falling back to
/// the legacy top-level `manifest["files"]` when `manifest["root"] == index_root_str`. Sorted.
pub(crate) fn scope_file_ids_from_manifest(
    manifest: &serde_json::Value,
    index_root: &std::path::Path,
    project_root: &std::path::Path,
) -> Vec<String> {
    let index_key = index_root.to_string_lossy().to_string();

    // The files map: modern "roots" structure first, then the legacy top-level form.
    let files_map = match manifest.get("roots") {
        Some(roots) => roots
            .get(&index_key)
            .and_then(|r| r.get("files"))
            .and_then(|f| f.as_object()),
        None => {
            if manifest.get("root").and_then(|r| r.as_str()) == Some(index_key.as_str()) {
                manifest.get("files").and_then(|f| f.as_object())
            } else {
                None
            }
        }
    };
    let files_map = match files_map {
        Some(m) => m,
        None => return Vec::new(),
    };

    // The prefix the file_ids must start with to be UNDER project_root.
    let prefix = if project_root == index_root {
        String::new()
    } else {
        match project_root.strip_prefix(index_root) {
            Ok(rel) => {
                // Normalize Windows backslashes so a nested-project prefix matches the
                // POSIX-keyed manifest file_ids (else nested projects get an empty scope).
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                if rel_str.is_empty() {
                    String::new()
                } else {
                    format!("{rel_str}/")
                }
            }
            // project_root is NOT under index_root: never widen scope.
            Err(_) => return Vec::new(),
        }
    };

    let mut matched: Vec<String> = files_map
        .keys()
        .filter(|file_id| prefix.is_empty() || file_id.starts_with(&prefix))
        .cloned()
        .collect();
    matched.sort();
    matched.dedup();
    matched
}

/// A3: resolve the mini's Oracle scope — the indexed file_ids under `project_root`. Best-effort:
/// returns an empty Vec on any failure (no index root, missing/invalid manifest), which the
/// bounded endpoint treats as "no documents in scope" (safe default, never the full corpus).
pub(crate) fn oracle_agent_scope_file_ids(project_root: &std::path::Path) -> Vec<String> {
    let index_root = match crate::oracle::commands::current_oracle_index_root() {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let manifest_path = index_root
        .join("oracle-data")
        .join("chunk-index-manifest.json");
    let manifest_content = match std::fs::read_to_string(&manifest_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let manifest: serde_json::Value = match serde_json::from_str(&manifest_content) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let project_root_norm = strip_windows_verbatim_prefix(project_root.to_path_buf());
    scope_file_ids_from_manifest(&manifest, &index_root, &project_root_norm)
}

/// A3: POST a SCOPED agent ask to the resident Oracle's bounded endpoint with the agent token.
/// Returns the answer text. `allowed_file_ids` confines retrieval to the mini's project files.
pub(crate) fn oracle_agent_ask(query: &str, allowed_file_ids: &[String]) -> Result<String, String> {
    // Fail-CLOSED short-circuit: an empty scope means no indexed docs for this project — don't
    // even hit the server (the bounded endpoint would answer grounded-empty anyway).
    if allowed_file_ids.is_empty() {
        return Ok("(oracle: no indexed files in scope for this project)".to_string());
    }

    let (base_url, _port) = oracle_session_endpoint();
    let url = format!("{base_url}/ask-bounded");
    let body = serde_json::json!({
        "query": query,
        "allowed_file_ids": allowed_file_ids,
        "limit": 5,
    });
    let resp = oracle_http_client()
        .post(&url)
        .header("x-oracle-auth-token", oracle_agent_token())
        .timeout(std::time::Duration::from_secs(90))
        .json(&body)
        // `.without_url()` keeps the loopback URL/port out of the error text returned to the model.
        .send()
        .map_err(|e| format!("oracle request failed: {}", e.without_url()))?;
    if !resp.status().is_success() {
        return Err(format!("oracle returned HTTP {}", resp.status()));
    }
    let v: serde_json::Value = resp.json().map_err(|e| format!("oracle bad json: {e}"))?;
    match v.get("answer").and_then(|a| a.as_str()) {
        Some(s) => Ok(s.to_string()),
        None => Err("oracle returned an unexpected response format".to_string()),
    }
}

fn oracle_child() -> &'static Mutex<Option<Child>> {
    ORACLE_CHILD.get_or_init(|| Mutex::new(None))
}

fn oracle_child_spawn() -> &'static Mutex<Option<Instant>> {
    ORACLE_CHILD_SPAWN.get_or_init(|| Mutex::new(None))
}

/// The OS pid of the currently-tracked resident Python child, if one is alive in
/// the registry. Max-recall finding (2026-07-02): the discovery file used to
/// publish `std::process::id()` — the APP's own pid — which made the MCP
/// children's pid-liveness gate watch the wrong process (a hung/crashed Python
/// server under a live app was never detected). This accessor exposes the REAL
/// server pid so `publish_discovery` can record it.
pub(crate) fn oracle_child_pid() -> Option<u32> {
    oracle_child()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|child| child.id())
}

/// Test seam: swap the tracked resident child, returning the previous one so
/// the test can restore it. Lets `discovery_pid()` (oracle_service.rs) assert
/// the published pid is the CHILD's, guarding against a regression back to
/// `std::process::id()` (the bug the accessor above exists to fix).
#[cfg(test)]
pub(crate) fn swap_oracle_child_for_test(child: Option<Child>) -> Option<Child> {
    std::mem::replace(
        &mut *oracle_child().lock().unwrap_or_else(|e| e.into_inner()),
        child,
    )
}

/// The monotonic age of the currently-tracked resident child, if a spawn stamp is
/// recorded. `None` when no child is tracked (no stamp) — a missing stamp is treated
/// by the caller as "do not force-kill" (we cannot prove it is hung).
fn oracle_child_age() -> Option<Duration> {
    oracle_child_spawn()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .map(|spawned| spawned.elapsed())
}

/// Finding 1 (pure, unit-testable): given the tracked child's age (if known) and the
/// hung-child upper bound, decide whether a `StillStarting` child should be KEPT
/// (slow-but-progressing cold boot) or FORCE-REPLACED (permanently wedged).
///
/// * `Some(age)` within `hung_timeout` → `false` (keep as "starting"; never kill a
///   progressing boot).
/// * `Some(age)` strictly past `hung_timeout` → `true` (tear down + respawn).
/// * `None` (no spawn stamp recorded) → `false`: without an age we cannot prove the
///   child is hung, so we conservatively keep it rather than kill a child we may not
///   even own. In practice a tracked, alive child always has a stamp.
fn still_starting_child_is_hung(age: Option<Duration>, hung_timeout: Duration) -> bool {
    matches!(age, Some(age) if age > hung_timeout)
}

fn oracle_server_start_lock() -> &'static Mutex<()> {
    ORACLE_SERVER_START_LOCK.get_or_init(|| Mutex::new(()))
}

fn oracle_cli_fallback_lock() -> &'static Mutex<()> {
    ORACLE_CLI_FALLBACK_LOCK.get_or_init(|| Mutex::new(()))
}

fn random_oracle_port() -> u16 {
    let mut bytes = [0u8; 2];
    if getrandom::fill(&mut bytes).is_err() {
        let fallback = std::process::id() as u16;
        return 20_000 + (fallback % 30_000);
    }
    20_000 + (u16::from_le_bytes(bytes) % 30_000)
}

/// Bounded wait until the fixed session port is actually BINDABLE before we spawn
/// a new resident server on it (P1). Cross-platform: it probes by attempting a
/// loopback `TcpListener::bind` and immediately dropping the listener on success.
///
/// WHY: the session port is chosen once and reused for every respawn. A just-killed
/// child can leave the socket briefly held (the OS releases it asynchronously), so a
/// fresh spawn could hit [Errno 10048]/EADDRINUSE and (before the Python bind-fail
/// exit) linger as an unbound zombie. Waiting for the port to free here makes the
/// new server's bind reliably succeed, so exactly one server owns the port.
///
/// The caller has already killed/torn down any TRACKED child by the time this runs;
/// this wait covers the OS's asynchronous socket release. If the port is STILL held
/// past the bounded deadline (a foreign/leftover holder we do not track), we return
/// an error instead of spawning a doomed child — the Python side would `os._exit(1)`
/// on the collision anyway, but bailing here avoids the wasted spawn + a confusing
/// "exited before ready". Best-effort and never panics.
const ORACLE_PORT_FREE_TIMEOUT: Duration = Duration::from_secs(5);

fn oracle_port_is_bindable(port: u16) -> bool {
    // Bind to loopback only (matches the server's host). Success → free; drop the
    // listener immediately so the subsequent server spawn can bind it.
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn wait_for_oracle_port_free(stop: &AtomicBool) -> Result<(), String> {
    let port = oracle_http_session().port;
    let started = Instant::now();
    loop {
        // Single-instance abort: a stopping supervisor (its `stop` flag was set by
        // `start_supervisor` superseding it, or by `on_lock`) must abandon this wait
        // PROMPTLY rather than hold `oracle_server_start_lock` for up to
        // `ORACLE_PORT_FREE_TIMEOUT` while a newer supervisor waits to bring up its
        // own server. Checked FIRST each slice so the old thread releases the start
        // lock within ~one poll cadence. The aborted-error sentinel signals
        // "superseded, not a real failure" to the caller (same as the readiness wait).
        if stop.load(Ordering::SeqCst) {
            return Err(ORACLE_SERVER_ABORTED_ERROR.to_string());
        }
        if oracle_port_is_bindable(port) {
            return Ok(());
        }
        if started.elapsed() >= ORACLE_PORT_FREE_TIMEOUT {
            return Err(format!(
                "Oracle session port {port} is still in use; not spawning a second \
                 server on a held port."
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

pub(crate) fn random_token() -> String {
    let mut bytes = [0u8; 32];
    if getrandom::fill(&mut bytes).is_err() {
        let fallback = format!(
            "{}-{}-{:p}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
            &bytes
        );
        return hex::encode(sha2::Sha256::digest(fallback.as_bytes()));
    }
    hex::encode(bytes)
}

/// The typed error returned by [`run_python_oracle`] when the Oracle is GENUINELY
/// unavailable: the resident HTTP server never came up AND no `oracle/` package
/// could be resolved for the CLI fallback. Pure + side-effect-free so the Bug A
/// control-flow contract is unit-testable without spawning a server or a CLI.
///
/// Bug A: this branch is reached ONLY AFTER an HTTP attempt fails — it is NOT an
/// upfront `python_oracle_available(root)` gate (that wrongly blocked Ask because
/// the workspace/index `root` never holds the package). When an HTTP error is
/// available it is folded into the message so the user sees the real reason the
/// server could not serve, rather than just the package hint.
fn cli_fallback_unavailable_error(http_err: Option<&str>) -> String {
    match http_err {
        Some(http_err) => format!("{MISSING_PACKAGE_ROOT_ERROR} ({http_err})"),
        None => MISSING_PACKAGE_ROOT_ERROR.to_string(),
    }
}

/// Bounded error returned when the outer hard-timeout cap fires and flips the
/// shared `cancel` flag while [`run_python_oracle`] is mid-flight. F1: the worker
/// checks `cancel` BEFORE every remaining expensive step (the in-lock HTTP retry and
/// the CLI subprocess fallback) and bails with this string. W1: the cooperative
/// `cancel` cannot interrupt an HTTP request already in flight (`reqwest::blocking`
/// is uninterruptible), so each HTTP request additionally BUDGETS its timeout
/// against the shared deadline — bounding the residual overrun to clock slack rather
/// than a full `PYTHON_ORACLE_TIMEOUT`. Together a timed-out ask winds the worker
/// down promptly instead of running the old ~270s tail and leaking an orphaned
/// thread + Python subprocess + pipe readers.
pub(crate) const ORACLE_CALL_CANCELLED_ERROR: &str = "Oracle call cancelled (timed out).";

pub fn run_python_oracle<T: DeserializeOwned>(
    root: &Path,
    command: &str,
    extra_args: &[String],
    llm_config: Option<&OracleLlmRuntimeConfig>,
    cancel: &AtomicBool,
    // W1: absolute cap deadline shared with the outer waiter. The first/primary
    // `/ask` HTTP request is a non-interruptible `reqwest::blocking` call, so the
    // worker cannot bail mid-flight on `cancel`; instead we BUDGET each HTTP request
    // timeout against this deadline so a doomed worker overruns the cap only by clock
    // slack, not by a full `PYTHON_ORACLE_TIMEOUT`. `None` keeps the unbudgeted
    // behaviour (tests / callers without a hard cap).
    deadline: Option<Instant>,
) -> Result<T, String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("Python Oracle root is invalid: {e}"))?;

    // BUG A FIX: try the resident HTTP server FIRST and do NOT gate on
    // `python_oracle_available(&root)`. The workspace/index `root` holds only the
    // index DATA (`oracle-data/`), never the `oracle/` PACKAGE source — the
    // package lives at the bundled/dev package root resolved independently by
    // `ensure_oracle_server` → `build_oracle_server_command` →
    // `find_oracle_package_root`. Gating the whole call on a package presence
    // check at the index root therefore wrongly blocked Ask with "Python Oracle
    // data is not available." even when the server could serve it. The CLI
    // fallback below keeps its own `find_oracle_package_root` + data-path checks,
    // which return clear, typed errors when the package/data are genuinely absent.
    let http_attempt = run_python_oracle_http(&root, command, extra_args, llm_config, deadline);
    if let Ok(payload) = http_attempt {
        return Ok(payload);
    }
    let http_err = http_attempt.err();

    // P1/P2: the resident server is simply not up yet (the supervisor is the sole
    // spawner and is bringing it up). Return the fast typed "starting" error
    // IMMEDIATELY — do NOT fall through to the heavy `oracle.cli` subprocess
    // fallback, which would load a SECOND embedding model and compete (CPU/VRAM)
    // with the server the supervisor is starting, exactly when resources are
    // tightest. The supervisor makes the server ready within seconds and the next
    // ask succeeds over HTTP. This only short-circuits the precise "starting"
    // sentinel; a genuine HTTP failure (connection refused after a crash, a real
    // server error) still flows to the CLI fallback below as before.
    if http_err.as_deref() == Some(ORACLE_SERVER_STARTING_ERROR) {
        return Err(ORACLE_SERVER_STARTING_ERROR.to_string());
    }

    // F1: the first HTTP attempt has already consumed up to one request timeout.
    // If the outer cap fired while we were in it, bail NOW — before blocking on the
    // CLI-fallback lock (which a concurrent slow ask may hold) and before any
    // further expensive work.
    if cancel.load(Ordering::Relaxed) {
        return Err(ORACLE_CALL_CANCELLED_ERROR.to_string());
    }

    // W4: recovering a POISONED cli-fallback lock needs NO defensive child reset
    // here (unlike `oracle_server_start_lock`). This lock guards only the CLI
    // subprocess path, whose child handle is a LOCAL inside `run_with_timeout` —
    // it is never stored in the `ORACLE_CHILD` global, and `run_with_timeout` kills
    // + waits + drains its child on every exit (success, error, timeout, cancel).
    // A panic that poisoned this lock could at worst orphan that local child, which
    // is not "tracked spawnable state" a later ask could double-spawn; there is
    // nothing to reset, so the standard recover-into-inner is correct.
    let _cli_guard = oracle_cli_fallback_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    // F1: acquiring the lock can itself block behind another in-flight ask for up
    // to a full request timeout. Re-check cancel before the (cheap) readiness probe
    // and the heavy CLI subprocess so a timed-out worker does not start either.
    if cancel.load(Ordering::Relaxed) {
        return Err(ORACLE_CALL_CANCELLED_ERROR.to_string());
    }
    // F1: the old code did a SECOND full `run_python_oracle_http` here — another up
    // to `PYTHON_ORACLE_TIMEOUT` (90s), and a `reqwest::blocking` request is NOT
    // interruptible mid-flight, so once started the cap's cancel could not cut it
    // short. That pushed the worst case past the cap. Replace the UNCONDITIONAL
    // second HTTP call with a CHEAP (5s) readiness probe, but branch on its precise
    // outcome (W3):
    //   * Ready        — a concurrent ask brought the resident server up while we
    //                    waited on the lock: retry the request ONCE over HTTP (the
    //                    common, fast win).
    //   * NotReady     — the probe failed for a TRANSIENT reason (server busy / TCP
    //                    RST / unparseable), NOT a confirmed wrong-root server. The
    //                    old code always retried HTTP here; preserve that — call
    //                    `run_python_oracle_http`, whose `ensure_oracle_server`
    //                    waits/restarts the server as needed, BEFORE falling to the
    //                    heavy CLI subprocess. Skipping it on a transient blip caused
    //                    an unnecessary CLI fallback (perf regression).
    //   * RootMismatch — the server is healthy but serving a DIFFERENT workspace
    //                    root: an HTTP retry would hit the wrong-root server, so skip
    //                    it and fall through to the correct, scoped CLI subprocess.
    // The probe itself cannot blow the budget; the single HTTP retry is bounded by
    // one request timeout exactly as the old unconditional path was.
    match probe_oracle_server_ready(&root) {
        ReadyProbe::Ready | ReadyProbe::NotReady => {
            if let Ok(payload) =
                run_python_oracle_http(&root, command, extra_args, llm_config, deadline)
            {
                return Ok(payload);
            }
            // The HTTP retry failed; re-check cancel (the request may have consumed
            // time) before the heavier CLI step.
            if cancel.load(Ordering::Relaxed) {
                return Err(ORACLE_CALL_CANCELLED_ERROR.to_string());
            }
        }
        // Healthy server, wrong workspace root: do NOT retry HTTP against it — fall
        // straight through to the scoped CLI subprocess below.
        ReadyProbe::RootMismatch { .. } => {}
    }

    // F1: final gate before the heaviest step — spawning a Python subprocess. If
    // the cap fired above, do NOT spawn the child; bail so no orphaned subprocess +
    // pipe-reader threads are left behind. (The CLI run itself is also cancel-aware
    // and kills the child within one poll tick if the cap fires mid-run.)
    if cancel.load(Ordering::Relaxed) {
        return Err(ORACLE_CALL_CANCELLED_ERROR.to_string());
    }

    // Interpreter = the venv under the DATA root; PYTHONPATH = the PACKAGE root so
    // `-m oracle.cli` imports regardless of cwd. cwd / data paths = the INDEX root.
    // FIX 2: resolve the package root up front and fail with the shared, explicit
    // message if it is absent, instead of running a `-m oracle.cli` with no import
    // path that fails as an opaque `ModuleNotFoundError`.
    let package_root = match find_oracle_package_root(Some(root.to_path_buf())) {
        Some(package_root) => package_root,
        // Genuinely unavailable: the resident server never came up AND there is no
        // package to run the CLI against. Surface the underlying HTTP failure (the
        // real reason Ask could not be served) alongside the package-root hint, so
        // the user sees an actionable, typed error rather than a bare panic.
        None => return Err(cli_fallback_unavailable_error(http_err.as_deref())),
    };
    let python = super::oracle_setup::resolve_oracle_python();
    let sqlite = root.join("oracle-data").join("metadata.sqlite");
    let vectors = oracle_vector_path(&root)
        .ok_or_else(|| "Python Oracle vector store is not available.".to_string())?;
    let chunks = root.join("oracle-data").join("chunks.lancedb");
    let mut args = vec![
        "-m".into(),
        "oracle.cli".into(),
        command.into(),
        "--sqlite".into(),
        path_arg(&sqlite),
        "--vectors".into(),
        path_arg(&vectors),
        "--chunks".into(),
        path_arg(&chunks),
    ];
    args.extend(extra_args.iter().cloned());

    let mut command_builder = Command::new(python);
    command_builder
        .args(args)
        .current_dir(&root)
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONPATH", &package_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_no_window(&mut command_builder);
    apply_llm_env(&mut command_builder, llm_config);
    // F1: pass the cancel flag so that if the outer cap fires while the CLI
    // subprocess is running, the child is killed promptly (within one poll tick)
    // instead of running out the full `PYTHON_ORACLE_TIMEOUT`.
    let output = run_with_timeout(command_builder, PYTHON_ORACLE_TIMEOUT, Some(cancel))
        .map_err(|e| format!("Python Oracle failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Python Oracle command failed: {}",
            stderr.trim().chars().take(400).collect::<String>()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_python_oracle_json(&stdout)
}

/// Run the Oracle doctor (`python -m oracle.bootstrap.doctor --root <index_root>`)
/// under the resolved VENV interpreter and parse its JSON report.
///
/// Reuses the existing python-resolution + subprocess + timeout machinery. The
/// child env mirrors the rest of the Oracle subprocess calls and additionally:
/// * `PYTHONPATH = <oracle code root>` so the `oracle` package imports even if
///   cwd resolution differs;
/// * `HF_HUB_OFFLINE = 1` / `TRANSFORMERS_OFFLINE = 1` so the embedder load in
///   check 2 never reaches the network (it must validate the *cached* model);
/// * `ORACLE_REQUIRE_REAL_EMBEDDER = 1` so check 2 is strict — a missing/mock
///   model RAISES (caught into `ok:false`) instead of silently hash-mocking.
///
/// `index_root` is the user-selected workspace; it is passed as `--root`. The
/// doctor imports the `oracle` package from `code_root` (PYTHONPATH) but runs
/// under the venv interpreter resolved from the DATA root (where the installed
/// runtime lives — separate from the package root in release). A non-zero exit or
/// unparseable output maps to a sanitized `String` error (caller wraps into
/// `OracleError::from_python`).
pub fn run_python_oracle_doctor(
    code_root: &Path,
    index_root: &Path,
) -> Result<crate::oracle::oracle_error::OracleDoctorReport, String> {
    let code_root = code_root
        .canonicalize()
        .map_err(|e| format!("Oracle code root is invalid: {e}"))?;
    // Interpreter = the venv under the DATA root; PYTHONPATH (below) = the package
    // root so `-m oracle.bootstrap.doctor` imports regardless of which holds source.
    let python = super::oracle_setup::resolve_oracle_python();
    let args = vec![
        "-m".to_string(),
        "oracle.bootstrap.doctor".to_string(),
        "--root".to_string(),
        path_arg(&index_root.to_path_buf()),
    ];

    // FIX 5: cwd = the INDEX/workspace root (`--root`), not the read-only bundled
    // package root, so the doctor's relative paths resolve under the workspace.
    // PYTHONPATH stays the package root (imports) and the interpreter is the
    // data-root venv (resolved above), so imports + venv are unaffected by the cwd.
    let mut command_builder = Command::new(python);
    command_builder
        .args(args)
        .current_dir(index_root)
        .env("PYTHONPATH", &code_root)
        .env("HF_HUB_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        .env("ORACLE_REQUIRE_REAL_EMBEDDER", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_no_window(&mut command_builder);

    let output = run_with_timeout(command_builder, ORACLE_DOCTOR_TIMEOUT, None)
        .map_err(|e| format!("Oracle doctor failed: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Oracle doctor command failed: {}",
            stderr.trim().chars().take(400).collect::<String>()
        ));
    }
    parse_oracle_doctor_report(&stdout)
}

/// Parse the doctor's stdout into a report. The doctor prints exactly one JSON
/// line, but import-time chatter may precede it, so we scan from the last line
/// for the first one that deserializes into the typed report.
fn parse_oracle_doctor_report(
    stdout: &str,
) -> Result<crate::oracle::oracle_error::OracleDoctorReport, String> {
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| line.starts_with('{'))
        .find_map(|line| serde_json::from_str(line).ok())
        .ok_or_else(|| "Oracle doctor output was invalid.".to_string())
}

fn run_python_oracle_http<T: DeserializeOwned>(
    root: &Path,
    command: &str,
    extra_args: &[String],
    llm_config: Option<&OracleLlmRuntimeConfig>,
    // W1: the absolute cap deadline (worker-start + `ORACLE_CALL_HARD_TIMEOUT`),
    // shared with the outer waiter. A `reqwest::blocking` request cannot be
    // interrupted mid-flight, so we instead BUDGET its timeout: the per-request
    // timeout is `min(PYTHON_ORACLE_TIMEOUT, remaining_budget)`. This bounds how far
    // a doomed worker can overrun the cap to clock slack rather than a full 90s
    // request. `None` (non-ask callers/tests) keeps the unbudgeted 90s timeout.
    deadline: Option<Instant>,
) -> Result<T, String> {
    // P1: do NOT spawn here — only the supervisor owns the resident server. Probe
    // readiness cheaply (bounded) and bail fast with a typed "starting" error if
    // it is not up yet, so a command can never race a second server onto the held
    // session port (the old `ensure_oracle_server` call was that racing spawner).
    require_oracle_server_ready(root)?;
    let session = oracle_http_session();
    let client = oracle_http_client();
    let mut request = if command == "ask" {
        client
            .post(format!("{}/ask", session.base_url))
            .json(&oracle_ask_payload(extra_args))
    } else {
        client.get(oracle_command_url(&session.base_url, command, extra_args)?)
    };
    let request_timeout = match deadline {
        Some(deadline) => {
            let remaining = deadline.saturating_duration_since(Instant::now());
            // The cap has already elapsed (or is about to): firing a fresh request
            // would only overrun it, so bail with the bounded cancelled error
            // instead of starting a doomed blocking call.
            if remaining.is_zero() {
                return Err(ORACLE_CALL_CANCELLED_ERROR.to_string());
            }
            remaining.min(PYTHON_ORACLE_TIMEOUT)
        }
        None => PYTHON_ORACLE_TIMEOUT,
    };
    request = request.timeout(request_timeout);
    request = apply_oracle_auth(request);
    request = apply_llm_headers(request, llm_config);
    let response = request
        .send()
        .map_err(|e| format!("Oracle HTTP request failed: {e}"))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|e| format!("Oracle HTTP response read failed: {e}"))?;
    if !status.is_success() {
        return Err(format!(
            "Oracle HTTP command failed ({status}): {}",
            text.chars().take(400).collect::<String>()
        ));
    }
    parse_python_oracle_json(&text)
}

fn oracle_ask_payload(extra_args: &[String]) -> serde_json::Value {
    let query = arg_value(extra_args, "--query").unwrap_or_default();
    let limit = arg_value(extra_args, "--limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(8);
    serde_json::json!({ "query": query, "limit": limit })
}

pub fn run_python_oracle_http_post<T: DeserializeOwned>(
    root: &Path,
    path: &str,
) -> Result<T, String> {
    // P1: supervisor-only spawn. Probe (no spawn) and bail fast if not ready.
    require_oracle_server_ready(root)?;
    let session = oracle_http_session();
    let client = oracle_http_client();
    let url = format!("{}{}", session.base_url, path);
    let response = client
        .post(url)
        .timeout(PYTHON_ORACLE_TIMEOUT)
        .header("x-oracle-auth-token", &session.auth_token)
        // Strip the loopback URL from reqwest errors (defense in depth; these can
        // reach the UI), matching the GET wrapper.
        .send()
        .map_err(|e| format!("Oracle HTTP request failed: {}", e.without_url()))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|e| format!("Oracle HTTP response read failed: {}", e.without_url()))?;
    if !status.is_success() {
        return Err(format!(
            "Oracle HTTP command failed ({status}): {}",
            text.chars().take(400).collect::<String>()
        ));
    }
    parse_python_oracle_json(&text)
}

pub fn run_python_oracle_http_get<T: DeserializeOwned>(
    root: &Path,
    path: &str,
) -> Result<T, String> {
    // P1: supervisor-only spawn. Probe (no spawn) and bail fast if not ready.
    require_oracle_server_ready(root)?;
    let session = oracle_http_session();
    let client = oracle_http_client();
    let url = format!("{}{}", session.base_url, path);
    let response = client
        .get(url)
        .timeout(PYTHON_ORACLE_TIMEOUT)
        .header("x-oracle-auth-token", &session.auth_token)
        .send()
        // Strip the loopback URL from the reqwest error for consistency with the
        // other wrappers (defense in depth; this error can reach the UI).
        .map_err(|e| format!("Oracle HTTP request failed: {}", e.without_url()))?;
    let status = response.status();
    let text = response
        .text()
        // Strip the loopback URL from the reqwest error for consistency with the
        // `.send()` map_err above (defense in depth; this error can reach the UI).
        .map_err(|e| format!("Oracle HTTP response read failed: {}", e.without_url()))?;
    if !status.is_success() {
        return Err(format!(
            "Oracle HTTP command failed ({status}): {}",
            text.chars().take(400).collect::<String>()
        ));
    }
    parse_python_oracle_json(&text)
}

/// A single retrieval chunk from the Oracle `/context` endpoint, pared down to the
/// ONLY fields the suspect-file localization needs: the file the chunk came from
/// and its retrieval score. We deliberately do NOT deserialize `text` — the card
/// stores only FILE PATHS as suspects, never any code snippet, so the retrieved
/// source body never crosses back into the project store. `#[serde(default)]` on
/// `score` keeps a malformed/absent score from failing the whole parse (it just
/// sorts last). The `file_source` alias matches the Python payload key
/// (`chunk_context_payload` in `query_engine.py`).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ContextChunk {
    #[serde(alias = "file_source")]
    pub file_source: String,
    #[serde(default)]
    pub score: f64,
}

/// Envelope returned by the operator `/context` (both GET and the POST mirror):
/// `{ "query": ..., "chunks": [...] }`. Only `chunks` is consumed; `query` is
/// echoed back by the server and ignored here.
#[derive(Debug, serde::Deserialize)]
struct ContextResponse {
    #[serde(default)]
    chunks: Vec<ContextChunk>,
}

/// PURE retrieval against the Oracle `/context` endpoint (LanceDB + lexical, NO
/// LLM, no generation cost). Mirrors the read-only GET HTTP path
/// ([`run_python_oracle_http_get`]) — same resolved index `root`, same operator
/// auth header, same NON-spawning readiness gate, same per-request timeout — but
/// returns the parsed retrieval chunks instead of a typed status payload.
///
/// `query` and `limit` are sent in the JSON request BODY (`{"q":..,"limit":..}`)
/// to the operator `POST /context`, NOT as URL query params. The caller aggregates
/// the returned chunks into top-K distinct files (see `aggregate_suspect_files` in
/// `commands.rs`). Fail-closed: a not-ready server returns the fast typed
/// [`ORACLE_SERVER_STARTING_ERROR`] (classified as `ServerUnavailable`), an empty
/// index simply yields zero chunks; the caller treats any `Err` (and an empty list)
/// as "no suspects", never as a hard failure that breaks card creation.
///
/// PRIVACY: the card text crosses to the server in the POST body, so it can never
/// appear in the request URL (and thus never in uvicorn access logs, proxies, or
/// process monitors). We deliberately do NOT use `POST /context-bounded` — that
/// endpoint is the MCP scoped contract whose absent `allowed_file_ids` means "no
/// documents" (always zero chunks); the operator `POST /context` is the full-corpus
/// mirror of `GET /context`, added precisely for this privacy reason.
pub fn oracle_context_chunks(
    root: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<ContextChunk>, String> {
    // P1: supervisor-only spawn. Probe (no spawn) and bail fast if not ready —
    // identical contract to the other read-only HTTP wrappers.
    require_oracle_server_ready(root)?;
    let session = oracle_http_session();
    let client = oracle_http_client();
    let url = format!("{}/context", session.base_url);
    let response = client
        .post(url)
        .timeout(PYTHON_ORACLE_TIMEOUT)
        .header("x-oracle-auth-token", &session.auth_token)
        .json(&serde_json::json!({ "q": query, "limit": limit }))
        .send()
        // PRIVACY: this error string is persisted into a project note on the
        // fail-closed path. With the POST body carrying the query the URL is clean
        // by construction (no `q=` param), but `without_url()` still strips the
        // loopback URL from the reqwest error as defense in depth so nothing about
        // the request can leak into the stored note.
        .map_err(|e| format!("Oracle HTTP request failed: {}", e.without_url()))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|e| format!("Oracle HTTP response read failed: {}", e.without_url()))?;
    if !status.is_success() {
        return Err(context_http_error(status));
    }
    parse_context_chunks(&text)
}

/// PRIVACY: the fixed, body-free error for a non-200 `/context` response. Built
/// from ONLY the status code — never the response body, which a future server
/// version could make echo the query (or chunk text); this string is persisted
/// into a project note on the fail-closed path. Pinned by unit test.
fn context_http_error(status: reqwest::StatusCode) -> String {
    format!("Oracle HTTP command failed ({status}).")
}

/// PRIVACY: parse the `/context` envelope; a serde error would embed a fragment
/// of the response body, which for `/context` includes chunk `text` (source
/// code). Since this error is persisted into a project note on the fail-closed
/// path, map any parse failure to a FIXED, body-free message rather than the
/// serde error (which could leak code). Pinned by unit test.
fn parse_context_chunks(text: &str) -> Result<Vec<ContextChunk>, String> {
    let parsed: ContextResponse = serde_json::from_str(text)
        .map_err(|_| "Oracle returned an unparseable /context response.".to_string())?;
    Ok(parsed.chunks)
}

// ---------------------------------------------------------------------------
// Design grounding (Phase 2 STEP 4) — chunks WITH text.
// ---------------------------------------------------------------------------

/// A retrieval chunk for the DESIGN grounding path, which — UNLIKE the
/// privacy-minimal [`ContextChunk`] used for suspect-file localization — KEEPS the
/// chunk `text`. The design LLM is grounded in the REAL target codebase, so the
/// retrieved source body is injected (in-process only) into the prompt sent to the
/// loopback provider. This is acceptable: agents already read the target, and the
/// text never crosses to the management plane, is never logged, and never rides a
/// Tauri event — it goes ONLY into the prompt body delivered to the on-box LLM.
///
/// This is a SEPARATE struct from [`ContextChunk`] on purpose: the existing
/// privacy-minimal struct and its callers (suspect localization) must NOT start
/// carrying source text. The `file_source` alias matches the Python payload key
/// (`chunk_context_payload` in `query_engine.py`); `text` defaults to empty so a
/// malformed/absent text never fails the whole parse.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignContextChunk {
    #[serde(alias = "file_source")]
    pub file_source: String,
    #[serde(default)]
    pub score: f32,
    #[serde(default)]
    pub text: String,
}

/// Envelope for the design `/context` response that KEEPS chunk text. Distinct from
/// the privacy-minimal [`ContextResponse`] so the text-dropping one is never
/// accidentally reused on the design path (and vice versa).
#[derive(Debug, serde::Deserialize)]
struct DesignContextResponse {
    #[serde(default)]
    chunks: Vec<DesignContextChunk>,
}

/// Retrieve top-K grounding chunks WITH TEXT against the Oracle `/context` endpoint
/// over the TARGET project index. Mirrors [`oracle_context_chunks`] exactly (same
/// resident server, same operator auth header, same NON-spawning readiness gate,
/// same per-request timeout, query in the POST BODY so it never appears in a URL/log)
/// but returns chunks that retain `text` for prompt grounding.
///
/// GRACEFUL DEGRADE: a not-ready server returns the fast typed
/// [`ORACLE_SERVER_STARTING_ERROR`]; the design command maps EVERY `Err` (and an
/// empty list) to "no grounding" — it never hard-fails generation. PRIVACY: the
/// query crosses to the loopback server in the POST body (never a URL param, so
/// never in access logs); the returned source text stays in-process.
pub fn oracle_context_chunks_with_text(
    root: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<DesignContextChunk>, String> {
    require_oracle_server_ready(root)?;
    let session = oracle_http_session();
    let client = oracle_http_client();
    let url = format!("{}/context", session.base_url);
    let response = client
        .post(url)
        .timeout(PYTHON_ORACLE_TIMEOUT)
        .header("x-oracle-auth-token", &session.auth_token)
        .json(&serde_json::json!({ "q": query, "limit": limit }))
        // Strip the loopback URL from the reqwest error (defense in depth; this
        // error can reach the UI).
        .send()
        .map_err(|e| format!("Oracle HTTP request failed: {}", e.without_url()))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|e| format!("Oracle HTTP response read failed: {}", e.without_url()))?;
    if !status.is_success() {
        return Err(context_http_error(status));
    }
    parse_design_context_chunks(&text)
}

/// PRIVACY: parse the design `/context` envelope. A serde error embeds a fragment of
/// the response body, which for `/context` includes chunk `text` (source code). Map
/// any parse failure to a FIXED, body-free message rather than the serde error.
/// Pinned by unit test.
fn parse_design_context_chunks(text: &str) -> Result<Vec<DesignContextChunk>, String> {
    let parsed: DesignContextResponse = serde_json::from_str(text)
        .map_err(|_| "Oracle returned an unparseable /context response.".to_string())?;
    Ok(parsed.chunks)
}

/// P1: NON-spawning readiness gate for the COMMAND paths (operator `/ask`,
/// snapshot, the read-only status polls, the index/watch HTTP wrappers).
///
/// The resident server has exactly ONE owner — the supervisor
/// ([`crate::backend::oracle_service::reconcile_once`] → [`ensure_oracle_server`]).
/// Before this fix the command HTTP wrappers ALSO called the spawning
/// `ensure_oracle_server`, so on unlock the supervisor and the frontend's
/// post-unlock boot polls raced to spawn a server on the SAME fixed session port:
/// one bound it, the other failed to bind ([Errno 10048]) and lingered, the
/// supervisor kept respawning on the held port, and every command blocked up to
/// the 165s hard cap behind that loop ("always indexing" / "Oracle is busy").
///
/// This gate does NO spawning and NO teardown: it issues ONE cheap, bounded
/// (`ORACLE_SERVER_HEALTH_TIMEOUT`, 5s) health probe and returns immediately.
/// * `Ready`                — proceed with the HTTP request.
/// * `NotReady`/`RootMismatch` — return [`ORACLE_SERVER_STARTING_ERROR`] (a fast,
///   typed "starting" the caller surfaces as `ServerUnavailable`); the supervisor
///   brings the server up within seconds and the next poll/ask succeeds.
fn require_oracle_server_ready(root: &Path) -> Result<(), String> {
    match probe_oracle_server_ready(root) {
        ReadyProbe::Ready => Ok(()),
        ReadyProbe::NotReady | ReadyProbe::RootMismatch { .. } => {
            Err(ORACLE_SERVER_STARTING_ERROR.to_string())
        }
    }
}

pub(crate) fn ensure_oracle_server(root: &Path, stop: &AtomicBool) -> Result<(), String> {
    // Single-instance fast-out FIRST (before even the cheap readiness probe): a
    // stopping supervisor must not probe, take the start lock, or spawn — a
    // superseding supervisor or a lock teardown owns the lifecycle now. Checking the
    // flag before the probe also makes the abort instant (no bounded health round-trip).
    if stop.load(Ordering::SeqCst) {
        return Err(ORACLE_SERVER_ABORTED_ERROR.to_string());
    }
    if oracle_server_ready(root) {
        return Ok(());
    }

    // W4: detect a POISONED start lock distinctly from a clean acquire. A panic
    // mid-`spawn_oracle_server` (e.g. after `command.spawn()` but before the child
    // handle was stored, or during a teardown) poisons this lock and can leave an
    // orphaned child the recorded handle never points at. On poison recovery ONLY,
    // defensively kill any tracked child before re-entering the start section, so a
    // subsequent ask cannot leak the first child and then spawn a second server on
    // the same port. The normal (un-poisoned) path is unchanged.
    let _start_guard = match oracle_server_start_lock().lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            let guard = poisoned.into_inner();
            // Network-free: only reaps the tracked child handle. Safe to call while
            // holding the start lock (it locks the distinct child mutex). Best-effort.
            let _ = kill_oracle_child();
            guard
        }
    };
    if oracle_server_ready(root) {
        return Ok(());
    }
    // Re-check the stop flag after acquiring the lock: we may have blocked here
    // while a newer supervisor took over (or a lock arrived). Abort before any
    // wait/spawn so the stopping supervisor releases the lock promptly.
    if stop.load(Ordering::SeqCst) {
        return Err(ORACLE_SERVER_ABORTED_ERROR.to_string());
    }
    if oracle_child_is_running()? {
        // A child is recorded as alive. Wait for it to serve `root` — but FIX 5:
        // do NOT blindly burn the full start timeout. After a rapid lock→unlock the
        // recorded child can be a just-killed/dying process (or one started against
        // a different root) that will NEVER answer `oracle_server_ready(root)`. If
        // it exits while we wait, `wait_for_oracle_server_ready` returns
        // `ChildDied` and we fall through to respawn IMMEDIATELY instead of stalling
        // the first post-unlock query for the whole 60s timeout.
        match wait_for_oracle_server_ready(root, stop)? {
            ServerWaitOutcome::Ready => return Ok(()),
            // The supervisor was superseded/stopped mid-wait: release the lock and
            // return promptly WITHOUT spawning. The child is left for the new
            // supervisor / the lock teardown to own.
            ServerWaitOutcome::Aborted => {
                return Err(ORACLE_SERVER_ABORTED_ERROR.to_string());
            }
            ServerWaitOutcome::ChildDied | ServerWaitOutcome::WrongRoot => {
                // ChildDied: `oracle_child_is_running` cleared the dead handle.
                // WrongRoot: the wait already tore the wrong-root server down.
                // Either way fall through to a fresh spawn below (after ensuring the
                // port is actually free).
            }
            // P1: the child is alive and still booting (a cold model load overran the
            // start timeout). Do NOT kill+respawn it — that is the loop. Report
            // not-ready WITHOUT teardown so the supervisor re-checks next tick and the
            // command paths keep returning the fast "starting" error meanwhile.
            //
            // Finding 1: UNLESS its TOTAL age exceeds `ORACLE_HUNG_CHILD_TIMEOUT`. A
            // progressing cold boot is well under that bound and is kept; a child that
            // is alive but has NEVER answered `/health` past the hung timeout (a wedged
            // model load / CUDA-init deadlock) would otherwise be kept FOREVER, leaving
            // the app permanently "starting". Force-replace it: log (redacted) + tear
            // it down, then fall through to a fresh spawn below.
            ServerWaitOutcome::StillStarting => {
                if still_starting_child_is_hung(oracle_child_age(), ORACLE_HUNG_CHILD_TIMEOUT) {
                    log_oracle_supervisor_event(
                        root,
                        "hung child: alive but /health never answered past the hung \
                         timeout — force-killing and respawning the resident server",
                    );
                    let _ = stop_python_oracle_runtime_unlocked();
                    // Fall through to the port-free wait + fresh spawn below.
                } else {
                    return Err(ORACLE_SERVER_STARTING_ERROR.to_string());
                }
            }
        }
    }

    // Single-instance abort: a stop may have arrived during the alive-child wait
    // above. Do NOT spawn a fresh server while stopping — that is exactly the
    // double-spawn this fix eliminates. Release the lock and return promptly.
    if stop.load(Ordering::SeqCst) {
        return Err(ORACLE_SERVER_ABORTED_ERROR.to_string());
    }

    // P1: before binding a new server, make sure the fixed session port is actually
    // FREE. A just-killed child can leave the socket briefly held (close/TIME_WAIT),
    // and a leftover process from a crashed prior app run can still own it. Spawning
    // onto a held port is what produced the [Errno 10048] bind-failure zombie. Wait
    // (bounded) for the port to become bindable; if it is held by a LIVE foreign
    // process we cannot reap, surface a clear error rather than spawning a doomed
    // child (the Python side would now `os._exit(1)` on the collision anyway).
    wait_for_oracle_port_free(stop)?;

    spawn_oracle_server(root)?;

    match wait_for_oracle_server_ready(root, stop)? {
        ServerWaitOutcome::Ready => Ok(()),
        // Superseded/stopped mid-wait on our freshly-spawned child: do NOT kill it
        // (the new supervisor / lock teardown owns lifecycle now) — just release the
        // lock and return. Leaving the child running is safe: it is the single
        // tracked child on the session port, and the surviving supervisor adopts it.
        ServerWaitOutcome::Aborted => Err(ORACLE_SERVER_ABORTED_ERROR.to_string()),
        // We just spawned this child ourselves; if it dies before serving, that is a
        // genuine startup failure (not a stale-handle race), so surface it. The
        // child handle was already cleared by `oracle_child_is_running` inside the
        // wait; tear down any residue for good measure.
        ServerWaitOutcome::ChildDied => {
            let _ = stop_python_oracle_runtime_unlocked();
            Err("Oracle server exited before it became ready.".into())
        }
        // The freshly-spawned child is healthy but reports a different root: it will
        // never serve ours. The wait already tore it down; surface a clear error.
        ServerWaitOutcome::WrongRoot => {
            Err("Oracle server started against a different workspace root.".into())
        }
        // The freshly-spawned child is still booting past the start timeout (cold
        // model load). KEEP it — the supervisor's next tick re-checks it and
        // publishes once it answers. Reporting "starting" (no teardown) is what
        // prevents the kill+respawn loop on a slow-but-healthy boot.
        ServerWaitOutcome::StillStarting => Err(ORACLE_SERVER_STARTING_ERROR.to_string()),
    }
}

/// Spawn the resident Oracle server process against `root` and record its child
/// handle. The caller MUST hold `oracle_server_start_lock` so the spawn is
/// serialized against teardown and other starts (one server per port).
fn spawn_oracle_server(root: &Path) -> Result<(), String> {
    let mut command = build_oracle_server_command(root)?;
    // The resident server reads its provider credentials EXCLUSIVELY from its own
    // env (oracle/server/routes.py::server_side_llm_config returns None and ignores
    // all client-supplied creds). Without ORACLE_LLM_API_KEY here it answers every
    // /ask as extractive ("API key is not configured"). Resolve the LLM config from
    // the vault and inject it onto the spawn env. The resolver returns None when
    // remote answering is disabled / the vault is locked / no key — in which case
    // no ORACLE_LLM_* env is set and the server stays (correctly) extractive.
    //
    // SAFETY (no deadlock): this runs under `oracle_server_start_lock` (held by the
    // caller), but the resolver's vault/keyring reads take their OWN locks only and
    // never the Oracle start lock, so there is no lock-ordering cycle.
    let llm_config = super::commands::resolve_oracle_llm_runtime_config();
    apply_llm_env(&mut command, llm_config.as_ref());
    apply_oracle_server_logs(&mut command, root);
    apply_no_window(&mut command);

    #[cfg(test)]
    ORACLE_SERVER_SPAWN_COUNT.fetch_add(1, Ordering::SeqCst);

    let child = command
        .spawn()
        .map_err(|e| format!("Oracle server could not start: {e}"))?;
    *oracle_child().lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
    // Finding 1: record the monotonic spawn stamp in lock-step with the child handle
    // so the supervisor can bound the TOTAL age of a `StillStarting` child and
    // force-replace a permanently-wedged one (see `ORACLE_HUNG_CHILD_TIMEOUT`).
    *oracle_child_spawn()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    Ok(())
}

/// Build the resident-server `Command` (interpreter + args + env + cwd) WITHOUT
/// spawning it. Split out so the interpreter/PYTHONPATH/cwd seam is unit-testable
/// via `Command::get_program`/`get_envs` without launching a real process.
///
/// * interpreter = the venv under the DATA root (where the installed runtime
///   lives), via [`super::oracle_setup::resolve_oracle_python`];
/// * `PYTHONPATH` = the PACKAGE root (the `oracle/` source) so
///   `python -m oracle.server.main` imports regardless of cwd — critical because
///   cwd/`--root` is the INDEX/workspace root, which has no `oracle/` source.
///   Without it the server cannot import `oracle` and exits before becoming ready;
/// * cwd = the INDEX/workspace root (per-workspace `oracle-data` index identity).
///
/// Logging redirection and the no-window flag are applied by the caller (they are
/// process-launch concerns, not part of the resolvable seam).
fn build_oracle_server_command(root: &Path) -> Result<Command, String> {
    // FIX 2: resolve the package/import root FIRST and fail with a clear message
    // when it is absent, instead of spawning a server with no `PYTHONPATH` that is
    // doomed to `ModuleNotFoundError: oracle` and surfaces only as the opaque
    // "exited before it became ready". The index root is the dev search hint;
    // release ignores it and locks to the bundled root.
    let package_root = find_oracle_package_root(Some(root.to_path_buf()));
    build_oracle_server_command_with_package_root(root, package_root)
}

/// Inner builder taking the already-resolved package root so the "absent package
/// root → explicit Err (no spawn)" contract (FIX 2) is unit-testable without a
/// live package-root resolution. `None` maps to [`MISSING_PACKAGE_ROOT_ERROR`].
fn build_oracle_server_command_with_package_root(
    root: &Path,
    package_root: Option<PathBuf>,
) -> Result<Command, String> {
    let package_root = package_root.ok_or_else(|| MISSING_PACKAGE_ROOT_ERROR.to_string())?;
    let session = oracle_http_session();
    // FAIL-CLOSED interpreter (resident-server respawn-loop fix): when the venv
    // runtime is not installed, surface an explicit Err instead of spawning a
    // doomed server on a bare system interpreter (it would crash instantly and
    // drive the ~10s supervisor respawn loop).
    let python = super::oracle_setup::resolve_oracle_runtime_python().ok_or_else(|| {
        "Oracle Python runtime is not installed (oracle-data venv missing); \
         refusing to spawn the resident server on a bare system interpreter."
            .to_string()
    })?;
    // Spawn with a verbatim-stripped cwd: when `root` carries the Windows `\\?\`
    // extended-length prefix, the server's `str(Path.cwd().resolve())` would echo
    // a `\\?\C:\…` string and the readiness root-compare would transiently
    // disagree, making the supervisor respawn the server several times per session.
    // Stripping the prefix from the cwd yields a clean `Path.cwd()` on the server
    // side. No-op on non-Windows / non-verbatim paths (passes through unchanged).
    let spawn_cwd = strip_windows_verbatim_prefix(root.to_path_buf());
    let mut command = Command::new(python);
    command
        .args(["-m", "oracle.server.main"])
        .current_dir(&spawn_cwd)
        .env("PYTHONIOENCODING", "utf-8")
        .env("ORACLE_PORT", session.port.to_string())
        .env("ORACLE_AUTH_TOKEN", &session.auth_token)
        // Two-tier auth (Step 4b): the AGENT token authorizes ONLY the bounded
        // endpoints and is the token published to MCP thin-clients. Both tiers
        // are live for the unlocked session.
        .env("ORACLE_AGENT_AUTH_TOKEN", oracle_agent_token())
        // App-supervised resident model: the app owns this server's lifecycle and
        // tears it down on lock/idle-expiry, so the server must NOT self-exit on
        // its own idle timer (that would leave a stale discovery file pointing at
        // a dead port). The supervisor restarts it if it dies. The Python config
        // reads this exact key (oracle/config.py: ORACLE_DISABLE_IDLE_EXIT).
        .env(ORACLE_DISABLE_IDLE_EXIT_ENV, "1")
        // Parent-death watchdog (orphan-server fix): the server self-exits when
        // this app pid is gone. Covers every teardown `on_app_exit` cannot reach —
        // SIGKILL, crash, `tauri dev` rebuild — on macOS AND Windows. The Python
        // side (oracle/server/main.py: _start_parent_watchdog) is a no-op when
        // this env var is absent, so CLI/test runs are unaffected.
        .env("ORACLE_PARENT_PID", std::process::id().to_string())
        .env("PYTHONPATH", &package_root);
    // A4 (live wiring) — authoritative embed-device override: force the resident embedder to CPU when
    // the coordinator reports GPU pressure (a local decode active / low free memory) at spawn time, so
    // the featherweight query embed doesn't fight the coder. When NOT under pressure we leave the env
    // UNSET so the embedder's own A1 device logic picks cuda/mps/cpu — never force "mps" (wrong on a
    // CUDA host). The resident embedder loads once, so this is a spawn-time decision.
    if crate::backend::oracle_coordinator::current_embed_device() == "cpu" {
        command.env("ORACLE_EMBED_DEVICE", "cpu");
    }
    Ok(command)
}

/// Result of waiting for the resident server to answer health at a given root.
enum ServerWaitOutcome {
    /// The server answered `oracle_server_ready(root)` within the timeout.
    Ready,
    /// The recorded child process exited before becoming ready (handle cleared).
    /// FIX 5: lets the caller respawn immediately instead of waiting the full
    /// timeout on a dead-but-recorded child after a rapid lock→unlock.
    ChildDied,
    /// P1: the start timeout elapsed but the child is STILL ALIVE and was NOT a
    /// confirmed wrong-root server — it is simply still booting (e.g. a cold
    /// embedding-model load that overran the timeout). The caller must KEEP the
    /// child (do NOT kill+respawn): tearing it down here was exactly what let a
    /// new process try to spawn on the still-held port and drove the restart loop.
    /// The supervisor's next tick re-checks it and publishes once it answers.
    StillStarting,
    /// The start timeout elapsed and the child is HEALTHY but serving a DIFFERENT
    /// workspace root — it will never answer for `root`, so the caller tears it
    /// down (already done here) and respawns against the correct root.
    WrongRoot,
    /// The supervisor's stop flag was observed mid-wait: a NEWER supervisor (or a
    /// lock teardown) is taking over, so this stopping supervisor must abandon the
    /// wait PROMPTLY and release `oracle_server_start_lock` WITHOUT spawning or
    /// respawning. The child is left untouched — the new supervisor / the teardown
    /// owns its lifecycle. This is what makes the supervisor truly single-instance:
    /// the old thread exits within ~one poll slice instead of running its start to
    /// completion while a second supervisor races a second spawn (the double-spawn).
    Aborted,
}

fn wait_for_oracle_server_ready(
    root: &Path,
    stop: &AtomicBool,
) -> Result<ServerWaitOutcome, String> {
    let started = Instant::now();
    // F4: the loop runs up to ~240 times (every 250ms over the 60s timeout). We do
    // NOT log (or sha256-redact) the server_root mismatch on every poll. Instead we
    // remember only the LAST-SEEN mismatch pair (cheap normalized strings, no hash)
    // and emit ONE redacted summary line at the end of the wait episode if the
    // server stayed healthy-but-wrong-root for the whole timeout.
    let mut last_mismatch: Option<(String, String)> = None;
    while started.elapsed() < ORACLE_SERVER_START_TIMEOUT {
        // Single-instance abort: a stopping supervisor (its `stop` flag was set by
        // `start_supervisor` superseding it, or by `on_lock`) must abandon the wait
        // PROMPTLY rather than hold `oracle_server_start_lock` to completion while a
        // newer supervisor spins up a second server. Checked FIRST each slice so the
        // old thread exits within ~one poll cadence. We do NOT touch the child here:
        // the superseding supervisor / the lock teardown owns its lifecycle.
        if stop.load(Ordering::SeqCst) {
            return Ok(ServerWaitOutcome::Aborted);
        }
        match probe_oracle_server_ready(root) {
            ReadyProbe::Ready => return Ok(ServerWaitOutcome::Ready),
            ReadyProbe::NotReady => {}
            ReadyProbe::RootMismatch { expected, server } => {
                last_mismatch = Some((expected, server));
            }
        }
        // FIX 5: if the recorded child has exited, stop waiting now — continuing to
        // poll a dead process would just burn the rest of the timeout. `oracle_
        // child_is_running` clears the dead handle as a side effect, so the caller
        // can respawn cleanly.
        if !oracle_child_is_running()? {
            return Ok(ServerWaitOutcome::ChildDied);
        }
        thread::sleep(Duration::from_millis(250));
    }
    // The server never served `root` within the timeout. Two very different cases:
    //
    // (a) WRONG ROOT — the server was HEALTHY the whole time but serving a DIFFERENT
    //     workspace root. It will NEVER answer for `root`, so it must be torn down
    //     and respawned against the correct root. Surface it ONCE, REDACTED (last
    //     path component + short hash), to the persistent Oracle log so it is
    //     diagnosable in a release Windows GUI build (which has no stderr) without
    //     ever leaking the user's absolute paths.
    //
    // (b) STILL STARTING — the child is still ALIVE and was NOT a confirmed wrong-
    //     root server (no mismatch seen). It is simply slow to boot (a cold
    //     embedding-model load can exceed the 60s start timeout). P1: do NOT kill
    //     it. Tearing down a healthy-but-still-loading child here is precisely what
    //     freed the port for a competing respawn and produced the restart loop /
    //     bind-failure zombie. Keep the child; the supervisor's next tick re-checks
    //     it and publishes the moment it answers.
    if let Some((expected, server)) = last_mismatch {
        log_oracle_supervisor_event(
            root,
            &format!(
                "server_root mismatch: expected={} server={}",
                redact_root_for_log(&expected),
                redact_root_for_log(&server),
            ),
        );
        let _ = stop_python_oracle_runtime_unlocked();
        return Ok(ServerWaitOutcome::WrongRoot);
    }
    // No mismatch was ever observed and (since we did not return ChildDied above)
    // the child is still alive: it is mid-boot. Leave it running.
    Ok(ServerWaitOutcome::StillStarting)
}

/// Append a single diagnostic line to the persistent Oracle supervisor log under
/// `<root>/oracle-data/logs/oracle-supervisor.log`. F2: this is the diagnostic
/// channel that SURVIVES a release Windows GUI build — a windowed subprocess has
/// no attached stderr, so `eprintln!` is silently discarded there. The rest of the
/// Oracle runtime already logs to this `oracle-data/logs` directory (see
/// [`apply_oracle_server_logs`]), so this matches the established channel.
///
/// Best-effort and side-effect-bounded: a missing/unwritable log dir is ignored
/// (we never want diagnostics to break the supervisor). The CALLER is responsible
/// for redacting any path/secret out of `message` before passing it here — this
/// helper writes `message` verbatim.
fn log_oracle_supervisor_event(root: &Path, message: &str) {
    let log_dir = root.join("oracle-data").join("logs");
    if std::fs::create_dir_all(&log_dir).is_err() {
        return;
    }
    let log_path = log_dir.join("oracle-supervisor.log");
    // NITPICK 1: this log is append-only and otherwise unbounded. Before appending,
    // cheaply cap it: if it has grown past `ORACLE_SUPERVISOR_LOG_MAX_BYTES`, rotate
    // in place down to the last `ORACLE_SUPERVISOR_LOG_KEEP_LINES` lines. Best-effort
    // — any IO failure is swallowed so diagnostics never break the supervisor.
    rotate_oracle_supervisor_log_if_large(&log_path);
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        use std::io::Write;
        let ts = chrono::Utc::now().to_rfc3339();
        let _ = writeln!(file, "{ts} {message}");
    }
}

/// Size threshold above which the supervisor log is rotated (~1 MiB).
const ORACLE_SUPERVISOR_LOG_MAX_BYTES: u64 = 1024 * 1024;
/// Number of trailing lines retained when the supervisor log is rotated.
const ORACLE_SUPERVISOR_LOG_KEEP_LINES: usize = 200;

/// Best-effort in-place rotation of the supervisor log: if it exceeds
/// `ORACLE_SUPERVISOR_LOG_MAX_BYTES`, rewrite it holding only its last
/// `ORACLE_SUPERVISOR_LOG_KEEP_LINES` lines. Every failure path is swallowed (the
/// log is purely diagnostic — losing it must never break or panic the supervisor).
fn rotate_oracle_supervisor_log_if_large(log_path: &Path) {
    // Cheap pre-check: only read/rewrite when the file is actually oversized.
    let oversized = std::fs::metadata(log_path)
        .map(|m| m.len() > ORACLE_SUPERVISOR_LOG_MAX_BYTES)
        .unwrap_or(false);
    if !oversized {
        return;
    }
    let Ok(contents) = std::fs::read_to_string(log_path) else {
        return;
    };
    let lines: Vec<&str> = contents.lines().collect();
    let start = lines.len().saturating_sub(ORACLE_SUPERVISOR_LOG_KEEP_LINES);
    let mut kept = lines[start..].join("\n");
    if !kept.is_empty() {
        kept.push('\n');
    }
    // Overwrite (truncate) with the retained tail. A failure here just leaves the
    // oversized file in place until the next event retries — acceptable.
    let _ = std::fs::write(log_path, kept);
}

fn oracle_child_is_running() -> Result<bool, String> {
    let mut guard = oracle_child().lock().unwrap_or_else(|e| e.into_inner());
    let Some(child) = guard.as_mut() else {
        return Ok(false);
    };
    match child.try_wait() {
        Ok(Some(_)) => {
            *guard = None;
            // Finding 1: keep the spawn stamp in lock-step with the child slot — a
            // dead-and-cleared child has no meaningful age.
            *oracle_child_spawn()
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = None;
            Ok(false)
        }
        Ok(None) => Ok(true),
        Err(e) => Err(format!("Oracle process status failed: {e}")),
    }
}

fn apply_oracle_server_logs(command: &mut Command, root: &Path) {
    let log_dir = root.join("oracle-data").join("logs");
    if std::fs::create_dir_all(&log_dir).is_err() {
        command.stdout(Stdio::null()).stderr(Stdio::null());
        return;
    }
    let stdout_path = log_dir.join("oracle-server.stdout.log");
    let stderr_path = log_dir.join("oracle-server.stderr.log");
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stdout_path);
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stderr_path);
    // FINDING 5: the server's ENV now carries the LLM key, so a future stray
    // `os.environ`/traceback dump could land in these files. Tighten both to
    // owner-only (Windows icacls owner SID / Unix 0600), reusing the discovery
    // file's restricted-permission mechanism. BEST-EFFORT: a failure here must never
    // block server startup (we still attach the handles below), so we only note it.
    // Applied on every spawn so a pre-existing, too-open log is tightened on open.
    for log_path in [&stdout_path, &stderr_path] {
        if !crate::backend::oracle_service::restrict_existing_path_to_owner(log_path) {
            log_oracle_supervisor_event(
                root,
                "warning: could not restrict oracle-server log file to owner-only \
                 permissions (continuing)",
            );
        }
    }
    match (stdout, stderr) {
        (Ok(stdout), Ok(stderr)) => {
            command
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr));
        }
        _ => {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
}

pub(crate) fn apply_no_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

pub fn stop_python_oracle_runtime() -> Result<(), String> {
    let _start_guard = oracle_server_start_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    stop_python_oracle_runtime_unlocked()
}

fn stop_python_oracle_runtime_unlocked() -> Result<(), String> {
    // Courtesy "stop the watcher" HTTP call. A blocking `.send()` must never run
    // on a tokio async worker (it can block the executor, and the reqwest blocking
    // client must not be touched there). We fire it on a detached OS thread using
    // the shared static client, so the caller — which may be an async command — is
    // never blocked and no blocking client/response is constructed on the executor.
    spawn_oracle_watcher_stop();
    kill_oracle_child()
}

/// Best-effort, fire-and-forget HTTP request asking the Oracle server to stop its
/// filesystem watcher. Runs on a detached `std::thread` so it never blocks (or
/// drop-panics on) a tokio async worker. Errors are intentionally swallowed: the
/// child process is killed regardless, which tears the server down anyway.
fn spawn_oracle_watcher_stop() {
    let Some(session) = ORACLE_HTTP_SESSION.get() else {
        return;
    };
    let url = format!("{}/index/watch/stop", session.base_url);
    let auth_token = session.auth_token.clone();
    std::thread::spawn(move || {
        let _ = oracle_http_client()
            .post(url)
            .timeout(ORACLE_STOP_REQUEST_TIMEOUT)
            .header("x-oracle-auth-token", auth_token)
            .send();
    });
}

/// Kill the tracked Oracle child process (if any) and nothing else — performs no
/// network I/O, so it is safe to call from `Drop` (including while the tokio
/// runtime is shutting down) without risking a blocking-client drop panic.
pub fn kill_python_oracle_child() -> Result<(), String> {
    kill_oracle_child()
}

/// Deadline for reaping a killed Oracle child (see [`reap_child_bounded`]). The
/// single `oracle-supervisor` thread calls `kill_oracle_child` inline, so an
/// UNBOUNDED `child.wait()` on a Python server that hangs on exit would stall the
/// supervisor forever and the server would never respawn. We bound the reap and
/// detach if the child overruns, keeping the supervisor making progress.
const ORACLE_CHILD_REAP_DEADLINE: Duration = Duration::from_secs(5);

/// Reap a just-killed child WITHOUT blocking unbounded: poll `try_wait` with short
/// sleeps until the child is reaped OR `deadline` elapses, then return. The normal
/// fast-exit case returns on the FIRST `try_wait` (no sleep) because a killed
/// process is almost always already gone; only a child that hangs on exit waits
/// out the deadline. Returns `true` if the child was reaped within the deadline,
/// `false` if it overran (caller detaches — the handle is dropped regardless, so
/// the OS reaps the zombie when the process exits). Never blocks longer than
/// `deadline + one poll interval`.
fn reap_child_bounded(child: &mut Child, deadline: Duration) -> bool {
    const POLL: Duration = Duration::from_millis(50);
    let started = Instant::now();
    loop {
        match child.try_wait() {
            // Reaped (exited) — done. Also treat a status error as "stop waiting":
            // we cannot reap it, and blocking would defeat the bound.
            Ok(Some(_)) => return true,
            Err(_) => return false,
            Ok(None) => {
                if started.elapsed() >= deadline {
                    // The child is still alive past the deadline (hung on exit).
                    // Stop waiting and let the caller detach so the supervisor
                    // keeps making progress; the dropped handle is reaped by the
                    // OS when the process finally exits.
                    return false;
                }
                thread::sleep(POLL);
            }
        }
    }
}

/// Kill the tracked Oracle child process (if any). Performs no network I/O, so it
/// is safe to call from `Drop` and from an async context.
///
/// The reap after `kill()` is BOUNDED ([`reap_child_bounded`] / `ORACLE_CHILD_REAP_DEADLINE`):
/// this runs inline on the single `oracle-supervisor` thread, so an unbounded
/// `child.wait()` on a Python server that hangs on exit would stall the supervisor
/// and prevent any respawn. Either way the child handle is cleared (taken above),
/// so `oracle_child_is_running()` / `should_restart` see it gone and respawn.
fn kill_oracle_child() -> Result<(), String> {
    let Some(child_lock) = ORACLE_CHILD.get() else {
        return Ok(());
    };
    let mut guard = child_lock.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut child) = guard.take() else {
        return Ok(());
    };
    // Finding 1: clear the spawn stamp in lock-step with taking the child handle so a
    // later age check never reads a stale stamp belonging to a torn-down child.
    *oracle_child_spawn()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    match child.try_wait() {
        Ok(Some(_)) => Ok(()),
        Ok(None) => {
            let result = child
                .kill()
                .map_err(|e| format!("Oracle process kill failed: {e}"));
            // Bounded reap: returns on the first poll for the normal fast-exit
            // case; detaches (drops the handle) if the child hangs past the
            // deadline so the supervisor is never blocked unbounded.
            let _ = reap_child_bounded(&mut child, ORACLE_CHILD_REAP_DEADLINE);
            result
        }
        Err(e) => Err(format!("Oracle process status failed: {e}")),
    }
}

fn apply_llm_headers(
    mut request: reqwest::blocking::RequestBuilder,
    llm_config: Option<&OracleLlmRuntimeConfig>,
) -> reqwest::blocking::RequestBuilder {
    if let Some(config) = llm_config {
        request = request
            .header("x-oracle-llm-provider", &config.provider)
            .header("x-oracle-llm-model", &config.model);
        if let Some(base_url) = config.base_url.as_deref() {
            request = request.header("x-oracle-llm-base-url", base_url);
        }
        // SECURITY: the LLM API key is intentionally NOT sent as an HTTP header.
        // The resident server reads its provider credentials exclusively from its
        // own environment (see oracle/server/routes.py::server_side_llm_config,
        // which returns None and ignores all client-supplied x-oracle-llm-* creds).
        // The key reaches the server via ORACLE_LLM_API_KEY on the spawn env
        // (CLI/spawn path below). Sending it over HTTP was dead and risked leaking
        // the secret into a debug log for zero benefit.
    }
    request
}

fn apply_oracle_auth(
    request: reqwest::blocking::RequestBuilder,
) -> reqwest::blocking::RequestBuilder {
    let session = oracle_http_session();
    request.header("x-oracle-auth-token", &session.auth_token)
}

fn apply_llm_env(command: &mut Command, llm_config: Option<&OracleLlmRuntimeConfig>) {
    if let Some(config) = llm_config {
        command.env("ORACLE_LLM_PROVIDER", &config.provider);
        command.env("ORACLE_LLM_MODEL", &config.model);
        if let Some(base_url) = config.base_url.as_deref() {
            command.env("ORACLE_LLM_BASE_URL", base_url);
        }
        if let Some(api_key) = config.api_key.as_deref() {
            command.env("ORACLE_LLM_API_KEY", api_key);
        }
    }
}

/// Outcome of a single readiness probe. `RootMismatch` carries the two NORMALIZED
/// (cheap string-compare) roots WITHOUT hashing — F4: the expensive sha256
/// redaction is deferred to the at-most-once log in
/// [`wait_for_oracle_server_ready`], never run on every 250ms poll.
enum ReadyProbe {
    /// The server answered 200 and is serving the expected workspace root.
    Ready,
    /// The server is unreachable / unhealthy / unparseable (treated as not ready).
    NotReady,
    /// The server is HEALTHY but serving a DIFFERENT workspace root than asked.
    RootMismatch { expected: String, server: String },
}

/// Single readiness probe against the resident server. Cheap: one short-timeout
/// HTTP GET plus string compares; NO hashing (F4 keeps the sha256 off the poll
/// path). Logging of a persistent mismatch is the wait loop's job, done once.
fn probe_oracle_server_ready(root: &Path) -> ReadyProbe {
    let session = oracle_http_session();
    let client = oracle_http_client();
    let response = match client
        .get(format!("{}/health", session.base_url))
        .timeout(ORACLE_SERVER_HEALTH_TIMEOUT)
        .header("x-oracle-auth-token", &session.auth_token)
        .send()
    {
        Ok(response) => response,
        Err(_) => return ReadyProbe::NotReady,
    };
    if !response.status().is_success() {
        return ReadyProbe::NotReady;
    }
    let text = match response.text() {
        Ok(text) => text,
        Err(_) => return ReadyProbe::NotReady,
    };
    let payload: serde_json::Value = match serde_json::from_str(&text) {
        Ok(payload) => payload,
        Err(_) => return ReadyProbe::NotReady,
    };
    let Some(expected_root) = normalize_existing_path_for_compare(root) else {
        return ReadyProbe::NotReady;
    };
    let Some(server_root) = payload.get("server_root").and_then(|value| value.as_str()) else {
        return ReadyProbe::NotReady;
    };
    classify_server_root_match(&expected_root, server_root)
}

/// Decide Ready vs RootMismatch from the (already `canonicalize`+normalized)
/// expected root and the RAW `server_root` string the Python server reports.
/// Both sides are funnelled through [`normalize_path_text_for_compare`], which
/// strips the Windows `\\?\` verbatim prefix and applies case/separator
/// folding, so a `\\?\C:\X` vs `C:\X` pair for the SAME path compares EQUAL
/// (the asymmetry that previously caused spurious supervisor respawns). Pure
/// over its inputs so the root-match decision is unit-testable without HTTP.
fn classify_server_root_match(expected_root_norm: &str, server_root_raw: &str) -> ReadyProbe {
    let server_root_norm = normalize_path_text_for_compare(server_root_raw);
    if server_root_norm == *expected_root_norm {
        ReadyProbe::Ready
    } else {
        ReadyProbe::RootMismatch {
            expected: expected_root_norm.to_string(),
            server: server_root_norm,
        }
    }
}

pub(crate) fn oracle_server_ready(root: &Path) -> bool {
    matches!(probe_oracle_server_ready(root), ReadyProbe::Ready)
}

/// Decide the chunk-store readiness from a parsed `/runtime` JSON body. Prefers
/// the nested `chunk_store.ready` (camelCase or snake), falls back to the
/// top-level `ready` mirror the sidecar emits. Pure over the JSON so it is
/// unit-testable without a live server.
fn runtime_chunk_store_ready(payload: &serde_json::Value) -> bool {
    payload
        .get("chunkStore")
        .or_else(|| payload.get("chunk_store"))
        .and_then(|store| store.get("ready"))
        .and_then(serde_json::Value::as_bool)
        .or_else(|| payload.get("ready").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

/// Probe the LIVE resident server for the doctor's honest live-server check.
/// Does NOT spawn (only the supervisor owns the server): it reuses the cheap
/// readiness probe, then — only if the server is up and serving the expected
/// root — fetches the now-fast `/runtime` and reads the CHUNK store readiness.
///
/// Returns:
/// * `Unreachable` when the server is down / unhealthy / serving a different root
///   (the doctor can't prove Oracle answers, so this is red);
/// * `ChunkStoreNotReady` when the server is up but the chunk index is empty;
/// * `Ready` when the server is up AND the chunk store is ready.
///
/// Cheap: at most two short-timeout GETs (`/health` via the readiness probe,
/// then `/runtime`), no model load. Never logs the port/token/path.
pub(crate) fn probe_oracle_live_server(
    root: &Path,
) -> crate::oracle::oracle_error::LiveServerProbe {
    use crate::oracle::oracle_error::LiveServerProbe;

    // Step 1: is the resident server up and serving THIS workspace root? A
    // mismatched/absent server is "unreachable" for the doctor's purpose.
    if !matches!(probe_oracle_server_ready(root), ReadyProbe::Ready) {
        return LiveServerProbe::Unreachable;
    }

    // Step 2: fetch the (now-fast) /runtime and read the chunk-store readiness.
    let session = oracle_http_session();
    let client = oracle_http_client();
    let response = match client
        .get(format!("{}/runtime", session.base_url))
        .timeout(ORACLE_SERVER_HEALTH_TIMEOUT)
        .header("x-oracle-auth-token", &session.auth_token)
        .send()
    {
        Ok(response) if response.status().is_success() => response,
        // Up for /health but /runtime failed/unauthorized: treat as unreachable
        // for the doctor (we cannot confirm the index is ready).
        _ => return LiveServerProbe::Unreachable,
    };
    let payload: serde_json::Value = match response.json() {
        Ok(payload) => payload,
        Err(_) => return LiveServerProbe::Unreachable,
    };
    if runtime_chunk_store_ready(&payload) {
        LiveServerProbe::Ready
    } else {
        LiveServerProbe::ChunkStoreNotReady
    }
}

/// Redact a normalized root path for a diagnostic log line: keep only the final
/// path component (the workspace folder name, low sensitivity) plus a short hash
/// of the full normalized string so two different roots are distinguishable
/// without ever emitting the absolute path (which would leak the OS username and
/// machine layout). Used only on the root-mismatch warning path.
fn redact_root_for_log(normalized_root: &str) -> String {
    let last = normalized_root
        .rsplit(['\\', '/'])
        .find(|segment| !segment.is_empty())
        .unwrap_or("<root>");
    let digest = sha2::Sha256::digest(normalized_root.as_bytes());
    let short = hex::encode(&digest[..4]);
    format!("…/{last}#{short}")
}

/// Strip the Windows Extended-length (`\\?\`) / verbatim-UNC (`\\?\UNC\`) prefix
/// that `Path::canonicalize` prepends on Windows, returning a path whose *string
/// form* matches the pre-canonicalize layout. This matters because the resolved
/// root is sent to the Python server (`--root`, `root=` query) and recorded as
/// the index identity: a `\\?\C:\…` string would make the Python side treat an
/// already-indexed workspace as a new one and needlessly re-index. Case is
/// preserved (unlike [`normalize_path_text_for_compare`], which is lossy and only
/// for equality checks). No-op on non-Windows.
pub(crate) fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest.to_string());
        }
        path
    }
    #[cfg(not(windows))]
    {
        path
    }
}

fn normalize_existing_path_for_compare(path: &Path) -> Option<String> {
    path.canonicalize()
        .ok()
        .map(|path| normalize_path_text_for_compare(&path.to_string_lossy()))
}

fn normalize_path_text_for_compare(value: &str) -> String {
    #[cfg(windows)]
    {
        let mut normalized = value.replace('/', "\\");
        if let Some(rest) = normalized.strip_prefix(r"\\?\UNC\") {
            normalized = format!(r"\\{rest}");
        } else if let Some(rest) = normalized.strip_prefix(r"\\?\") {
            normalized = rest.to_string();
        }
        while normalized.ends_with('\\') && normalized.len() > 3 {
            normalized.pop();
        }
        normalized.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        let mut normalized = value.replace('\\', "/");
        while normalized.ends_with('/') && normalized.len() > 1 {
            normalized.pop();
        }
        normalized
    }
}

fn oracle_command_url(
    base_url: &str,
    command: &str,
    extra_args: &[String],
) -> Result<String, String> {
    let url = match command {
        "snapshot" => format!("{base_url}/snapshot"),
        "ask" => return Err("Oracle ask must use POST body.".into()),
        "node" => {
            let node_id = arg_value(extra_args, "--node-id")
                .ok_or_else(|| "Oracle node command missing --node-id.".to_string())?;
            format!("{base_url}/node/{}", encode_path(&node_id))
        }
        "similar" => {
            let node_id = arg_value(extra_args, "--node-id")
                .ok_or_else(|| "Oracle similar command missing --node-id.".to_string())?;
            let limit = arg_value(extra_args, "--limit").unwrap_or_else(|| "8".into());
            format!("{base_url}/similar/{}?limit={limit}", encode_path(&node_id))
        }
        "duplicates" => format!("{base_url}/duplicate-labels"),
        "cluster" => {
            let cluster_id = arg_value(extra_args, "--cluster-id")
                .ok_or_else(|| "Oracle cluster command missing --cluster-id.".to_string())?;
            format!("{base_url}/cluster/{}", encode_path(&cluster_id))
        }
        "coverage" => format!("{base_url}/coverage"),
        "runtime" => format!("{base_url}/runtime"),
        other => return Err(format!("Unsupported Oracle HTTP command: {other}")),
    };
    Ok(url)
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

fn encode_path(value: &str) -> String {
    urlencoding::encode(&value.replace('\\', "/"))
        .replace("%2F", "/")
        .replace("%2f", "/")
}

fn path_arg(path: &PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

/// Spawn `command`, capture its piped stdout/stderr and wait up to `timeout`,
/// killing the child if it overruns. F1: when `cancel` is supplied and flips to
/// `true` mid-run, the child is killed within one poll tick (25ms) and a bounded
/// cancellation error is returned — so a timed-out Oracle ask does not leave a
/// subprocess + pipe-reader threads running for the rest of the request timeout.
/// `cancel` is also checked once BEFORE spawning, so a child is never started for
/// an already-cancelled call.
pub(crate) fn run_with_timeout(
    mut command: Command,
    timeout: Duration,
    cancel: Option<&AtomicBool>,
) -> Result<ProcessOutput, String> {
    // F1: do not even spawn if the call was already cancelled before we got here.
    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
        return Err(ORACLE_CALL_CANCELLED_ERROR.to_string());
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("process could not start: {e}"))?;
    let stdout = child
        .stdout
        .take()
        .map(|mut pipe| thread::spawn(move || read_pipe_output(&mut pipe, "stdout")));
    let stderr = child
        .stderr
        .take()
        .map(|mut pipe| thread::spawn(move || read_pipe_output(&mut pipe, "stderr")));
    let started = Instant::now();

    loop {
        let wait_result = child.try_wait();
        let status = match wait_result {
            Ok(status) => status,
            Err(e) => {
                // BLOCKER 1: do not leak the child + the two pipe-reader threads
                // when try_wait itself errors. Mirror the cancel-path cleanup
                // before propagating the error.
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_pipe_output(stdout);
                let _ = join_pipe_output(stderr);
                return Err(format!("process wait failed: {e}"));
            }
        };
        if let Some(status) = status {
            // Join BOTH readers before propagating either error — `?` on stdout
            // alone would drop (detach, never join) the stderr handle and discard
            // its output, unlike every other exit path here.
            let stdout = join_pipe_output(stdout);
            let stderr = join_pipe_output(stderr);
            return Ok(ProcessOutput {
                status,
                stdout: stdout?,
                stderr: stderr?,
            });
        }

        // F1: cooperative cancellation — kill the child promptly so the worker can
        // wind down well inside the remaining budget after the outer cap fires.
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = join_pipe_output(stdout);
            let _ = join_pipe_output(stderr);
            return Err(ORACLE_CALL_CANCELLED_ERROR.to_string());
        }

        if started.elapsed() >= timeout {
            // BLOCKER 2: a failed kill here is the root cause of a later zombie +
            // permanently blocked pipe-reader threads, so surface it via the
            // established supervisor log channel (best-effort) instead of fully
            // swallowing it.
            if let Err(kill_err) = child.kill() {
                if let Some(root) = oracle_data_root() {
                    log_oracle_supervisor_event(
                        &root,
                        &format!("run_with_timeout: kill after timeout failed: {kill_err}"),
                    );
                }
            }
            let _ = child.wait();
            // BLOCKER 2: join the pipe readers so they unblock once the kill closes
            // the child's stdout/stderr — mirrors the cancel-path cleanup above.
            let _ = join_pipe_output(stdout);
            let _ = join_pipe_output(stderr);
            return Err(format!("process exceeded {}s timeout", timeout.as_secs()));
        }

        thread::sleep(Duration::from_millis(25));
    }
}

fn read_pipe_output<R: Read>(pipe: &mut R, stream_name: &str) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    pipe.read_to_end(&mut output)
        .map_err(|e| format!("process {stream_name} read failed: {e}"))?;
    Ok(output)
}

fn join_pipe_output(
    handle: Option<thread::JoinHandle<Result<Vec<u8>, String>>>,
) -> Result<Vec<u8>, String> {
    match handle {
        Some(handle) => handle
            .join()
            .map_err(|_| "process output reader panicked".to_string())?,
        None => Ok(Vec::new()),
    }
}

// ---------------------------------------------------------------------------
// P6.2 — Semantic similarity Oracle wrappers.
// ---------------------------------------------------------------------------

/// A single similar-file result from the Oracle `/similar` endpoint.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimilarFile {
    /// file_id of the similar file (project-relative path).
    pub id: String,
    #[serde(default)]
    pub score: f64,
}

/// Envelope returned by `GET /clusters`.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClustersResponse {
    #[serde(default)]
    epoch: String,
    #[serde(default)]
    clusters: Vec<serde_json::Value>,
}

/// PRIVACY: fixed body-free error for semantic Oracle calls. Never carries the
/// response body (which could include file paths).
fn semantic_http_error(status: reqwest::StatusCode) -> String {
    format!("Oracle HTTP command failed ({status}).")
}

/// Retrieve top-K similar files from the Oracle `/similar/{node_id}` endpoint.
///
/// `node_id` is the project-relative file path (the same `id` used in the Oracle
/// node_cards table, populated by `learn_files`). Returns a list of `SimilarFile`
/// sorted by cosine similarity desc. Fail-closed: a not-ready server returns
/// [`ORACLE_SERVER_STARTING_ERROR`]; an empty index yields an empty list.
///
/// Mirrors [`oracle_context_chunks`] exactly: same resident server, same operator
/// auth header, same NON-spawning readiness gate, same per-request timeout,
/// same PRIVACY posture (fixed error strings, `.without_url()`).
pub fn oracle_similar(
    root: &Path,
    node_id: &str,
    limit: usize,
) -> Result<Vec<SimilarFile>, String> {
    require_oracle_server_ready(root)?;
    let session = oracle_http_session();
    let client = oracle_http_client();
    let encoded = urlencoding_hack(node_id);
    let url = format!("{}/similar/{encoded}", session.base_url);
    let response = client
        .get(url)
        .query(&[("limit", limit.to_string())])
        .timeout(PYTHON_ORACLE_TIMEOUT)
        .header("x-oracle-auth-token", &session.auth_token)
        .send()
        .map_err(|e| format!("Oracle HTTP request failed: {}", e.without_url()))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|e| format!("Oracle HTTP response read failed: {}", e.without_url()))?;
    if !status.is_success() {
        return Err(semantic_http_error(status));
    }
    let parsed: Vec<SimilarFile> = serde_json::from_str(&text)
        .map_err(|_| "Oracle returned an unparseable /similar response.".to_string())?;
    Ok(parsed)
}

/// Minimal percent-encode for file paths used as URL path segments. The Oracle
/// `/similar/{node_id:path}` route uses FastAPI's `:path` converter which handles
/// URL-decoding; we only need to encode characters that would break the URL
/// (primarily `#` and `?`). This avoids pulling in a full URL-encoding crate.
fn urlencoding_hack(path: &str) -> String {
    path.replace('%', "%25")
        .replace('#', "%23")
        .replace('?', "%3F")
        .replace(' ', "%20")
}

/// Retrieve the current Oracle clusters epoch from `GET /clusters`.
///
/// Returns the epoch string (ISO 8601 timestamp) from the clusters endpoint.
/// Used for cache-staleness checks: if the Oracle epoch is newer than the cache
/// epoch, the cache is stale. Fail-open: if the Oracle is down or the response
/// is unparseable, returns an empty string (which makes the cache appear stale,
/// triggering a refresh — safe degradation).
///
/// Mirrors the same privacy posture as [`oracle_context_chunks`]: loopback
/// session, operator auth, NON-spawning readiness gate, fixed body-free errors.
pub fn oracle_clusters_epoch(root: &Path) -> String {
    // Best-effort: if the server is not ready, return empty (cache stale → refresh
    // will be attempted when the server comes up).
    if require_oracle_server_ready(root).is_err() {
        return String::new();
    }
    let session = oracle_http_session();
    let client = oracle_http_client();
    let url = format!("{}/clusters", session.base_url);
    let response = match client
        .get(url)
        .timeout(PYTHON_ORACLE_TIMEOUT)
        .header("x-oracle-auth-token", &session.auth_token)
        .send()
    {
        Ok(r) => r,
        Err(_) => return String::new(),
    };
    if !response.status().is_success() {
        return String::new();
    }
    let text = match response.text() {
        Ok(t) => t,
        Err(_) => return String::new(),
    };
    let parsed: ClustersResponse = match serde_json::from_str(&text) {
        Ok(p) => p,
        Err(_) => return String::new(),
    };
    parsed.epoch
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::oracle::model::{OracleAnswer, OracleSnapshot};
    use std::fs;

    fn test_oracle_lock() -> &'static Mutex<()> {
        static TEST_ORACLE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        TEST_ORACLE_LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn scope_file_ids_from_manifest_filters_to_project_root() {
        use std::path::Path;
        let manifest = serde_json::json!({
            "roots": {
                "/idx": { "files": { "a.rs": 1, "projA/b.rs": 1, "projB/c.rs": 1 } }
            }
        });
        let idx = Path::new("/idx");
        // project_root == index_root -> all indexed files (sorted).
        assert_eq!(
            scope_file_ids_from_manifest(&manifest, idx, idx),
            vec!["a.rs", "projA/b.rs", "projB/c.rs"]
        );
        // a subdir scopes to only that project's files.
        assert_eq!(
            scope_file_ids_from_manifest(&manifest, idx, Path::new("/idx/projA")),
            vec!["projA/b.rs"]
        );
        // legacy top-level form.
        let legacy = serde_json::json!({ "root": "/idx", "files": { "x.rs": 1 } });
        assert_eq!(
            scope_file_ids_from_manifest(&legacy, idx, idx),
            vec!["x.rs"]
        );
        // project_root NOT under index_root -> empty (never widen scope).
        assert_eq!(
            scope_file_ids_from_manifest(&manifest, idx, Path::new("/other")),
            Vec::<String>::new()
        );
        // a root absent from the manifest -> empty.
        assert_eq!(
            scope_file_ids_from_manifest(&manifest, Path::new("/missing"), Path::new("/missing")),
            Vec::<String>::new()
        );
    }

    /// PRIVACY (suspect localization): a malformed `/context` body — which can
    /// contain chunk `text` (source code) and the echoed query — must map to the
    /// FIXED parse-error message; none of the body may survive into the error,
    /// because that string is persisted into a project note on the fail-closed
    /// path (`append_oracle_localization_failure_note`).
    #[test]
    fn context_parse_error_is_fixed_and_body_free() {
        const BODY_MARKER: &str = "SECRET_SOURCE_SNIPPET_let_api_key";
        let body = format!("{{\"query\": \"{BODY_MARKER}\", \"chunks\": \"not-a-list\"}}");
        let err = parse_context_chunks(&body).unwrap_err();
        assert_eq!(err, "Oracle returned an unparseable /context response.");
        assert!(!err.contains(BODY_MARKER));
        // Non-JSON garbage takes the same fixed message.
        let err = parse_context_chunks(BODY_MARKER).unwrap_err();
        assert_eq!(err, "Oracle returned an unparseable /context response.");
    }

    #[test]
    fn context_parse_accepts_envelope_and_defaults() {
        // Happy path: chunks parsed, text field deliberately ignored, missing
        // score defaults (sorts last) and a missing chunks key is an empty list.
        let body = r#"{"query":"q","chunks":[
            {"file_source":"src/a.rs","score":0.9,"text":"fn main() {}"},
            {"file_source":"src/b.rs"}
        ]}"#;
        let chunks = parse_context_chunks(body).unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].file_source, "src/a.rs");
        assert!((chunks[0].score - 0.9).abs() < 1e-9);
        assert_eq!(chunks[1].score, 0.0);
        assert!(parse_context_chunks(r#"{"query":"q"}"#).unwrap().is_empty());
    }

    /// DESIGN grounding: the design parser KEEPS chunk `text` (unlike the
    /// privacy-minimal suspect parser). The text is grounding for the on-box LLM.
    #[test]
    fn design_context_parse_keeps_text() {
        let body = r#"{"query":"q","chunks":[
            {"file_source":"src/Button.tsx","score":0.91,"text":"export const Button = () => <button/>;"},
            {"file_source":"src/theme.ts"}
        ]}"#;
        let chunks = parse_design_context_chunks(body).unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].file_source, "src/Button.tsx");
        assert!((chunks[0].score - 0.91).abs() < 1e-6);
        assert_eq!(chunks[0].text, "export const Button = () => <button/>;");
        // A chunk with no text defaults to empty (never fails the parse).
        assert_eq!(chunks[1].text, "");
        assert_eq!(chunks[1].score, 0.0);
    }

    /// PRIVACY: a malformed design `/context` body must map to the FIXED, body-free
    /// message — the body can contain source `text`, which must never survive into an
    /// error string that could reach the UI.
    #[test]
    fn design_context_parse_error_is_fixed_and_body_free() {
        const MARKER: &str = "SECRET_DESIGN_TOKEN_let_api_key";
        let body = format!("{{\"chunks\": \"not-a-list\", \"leak\": \"{MARKER}\"}}");
        let err = parse_design_context_chunks(&body).unwrap_err();
        assert_eq!(err, "Oracle returned an unparseable /context response.");
        assert!(!err.contains(MARKER));
    }

    /// PRIVACY: the non-200 error is built from ONLY the status code — fixed and
    /// body-free by construction (the body never reaches the formatter).
    #[test]
    fn context_http_error_contains_only_status() {
        let err = context_http_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            err,
            "Oracle HTTP command failed (500 Internal Server Error)."
        );
    }

    #[test]
    fn chunk_store_ready_prefers_nested_snake_case() {
        // The Python /runtime body is snake_case: chunk_store.ready drives the
        // live-server verdict, NOT the empty vector_store.
        let payload = serde_json::json!({
            "vector_store": {"records": 0, "ready": false},
            "chunk_store": {"records": 4177, "ready": true},
            "ready": false
        });
        assert!(runtime_chunk_store_ready(&payload));
    }

    #[test]
    fn chunk_store_ready_false_when_chunk_store_not_ready() {
        let payload = serde_json::json!({
            "chunk_store": {"records": 0, "ready": false},
            "ready": false
        });
        assert!(!runtime_chunk_store_ready(&payload));
    }

    #[test]
    fn chunk_store_ready_falls_back_to_top_level_ready() {
        // An older payload without a chunk_store block: fall back to the
        // top-level `ready` mirror rather than reporting a false negative.
        let payload = serde_json::json!({ "ready": true });
        assert!(runtime_chunk_store_ready(&payload));
        let payload = serde_json::json!({ "ready": false });
        assert!(!runtime_chunk_store_ready(&payload));
        // No signal at all -> not ready (never panics on a missing field).
        assert!(!runtime_chunk_store_ready(&serde_json::json!({})));
    }

    /// Build a temp dir holding the `oracle/` package SOURCE (cli.py +
    /// requirements.txt) so `oracle_package_present` is satisfied.
    fn make_package_source(dir: &Path) {
        fs::create_dir_all(dir.join("oracle")).unwrap();
        fs::write(dir.join("oracle").join("cli.py"), "").unwrap();
        fs::write(dir.join("oracle").join("requirements.txt"), "").unwrap();
    }

    /// Add a (complete) `oracle-data/venv` under `dir`: the OS-specific venv python
    /// plus the completion marker, mirroring a fully-installed runtime.
    fn make_complete_venv(dir: &Path) {
        let venv = crate::oracle::oracle_setup::oracle_venv_dir(dir);
        let py = crate::oracle::oracle_setup::venv_python(&venv);
        fs::create_dir_all(py.parent().unwrap()).unwrap();
        fs::write(&py, "").unwrap();
        fs::write(venv.join(".oracle-runtime-complete"), b"ok").unwrap();
    }

    fn unique_temp(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "aspis-data-root-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn select_data_root_prefers_candidate_owning_the_venv() {
        let base = unique_temp("prefers-venv");
        // Candidate A: only the package source (no venv). Candidate B: source + a
        // complete venv. The venv-bearing one must win even though A is listed first.
        let only_source = base.join("a-source-only");
        let with_venv = base.join("b-source-and-venv");
        make_package_source(&only_source);
        make_package_source(&with_venv);
        make_complete_venv(&with_venv);

        let candidates = vec![only_source.clone(), with_venv.clone()];
        assert_eq!(select_data_root(&candidates), Some(with_venv));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn select_data_root_falls_back_to_writable_source_when_no_venv() {
        let base = unique_temp("fresh-install");
        // No venv anywhere yet (fresh install): pick the writable package-source
        // candidate, never a candidate that lacks the `oracle/` source.
        let no_package = base.join("not-a-package");
        let source = base.join("writable-source");
        fs::create_dir_all(&no_package).unwrap();
        make_package_source(&source);

        let candidates = vec![no_package, source.clone()];
        assert_eq!(select_data_root(&candidates), Some(source));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn select_data_root_excludes_staged_target_root_for_fresh_install() {
        let base = unique_temp("exclude-staged");
        // A staged/read-only copy lives under a `target` dir (the `_up_` Tauri copy
        // in dev): it has the package source but must NOT be chosen as the writable
        // data root. A sibling writable source must be picked instead.
        let staged = base.join("target").join("debug").join("_up_");
        let writable = base.join("source-repo");
        make_package_source(&staged);
        make_package_source(&writable);

        // Staged listed first; the writable one (`target/debug` staging shape
        // excluded) must win.
        let candidates = vec![staged.clone(), writable.clone()];
        assert_eq!(select_data_root(&candidates), Some(writable.clone()));

        // FIX 3: even if the staged copy already OWNS a venv (e.g. stale from an old
        // install), it must STILL be excluded as the writable data root — writing
        // there fails (release, read-only) / pollutes the build tree (dev). The
        // sibling writable source must be picked instead.
        make_complete_venv(&staged);
        let candidates = vec![staged.clone(), writable.clone()];
        assert_eq!(select_data_root(&candidates), Some(writable));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn select_data_root_none_when_no_package_anywhere() {
        let base = unique_temp("empty");
        let empty = base.join("empty-dir");
        fs::create_dir_all(&empty).unwrap();
        assert_eq!(select_data_root(&[empty]), None);
        let _ = fs::remove_dir_all(&base);
    }

    /// FIX 1 (release RCE): the release-vs-dev data-root decision must FAIL CLOSED.
    /// * release + recorded → the recorded root, verbatim (never the candidate set).
    /// * release + none     → `None` (NEVER the candidate search — a user-writable
    ///   drop dir must never become the trusted data/venv root in release).
    /// * dev + none         → the candidate search (source repo keeps working).
    #[test]
    fn resolve_data_root_fails_closed_in_release() {
        let base = unique_temp("resolve-data-root");
        let recorded = base.join("app-data");
        fs::create_dir_all(&recorded).unwrap();
        // A candidate that WOULD be selected by the search (writable package source).
        let candidate = base.join("source-repo");
        make_package_source(&candidate);
        let candidates = vec![candidate.clone()];

        // release + recorded → recorded, verbatim (candidates ignored entirely).
        assert_eq!(
            resolve_data_root(Some(recorded.as_path()), true, &candidates),
            Some(recorded.clone()),
            "release must return the recorded root verbatim"
        );
        // release + none → None (fail closed; never reach the candidate search).
        assert_eq!(
            resolve_data_root(None, true, &candidates),
            None,
            "release with no recorded root must fail closed (None), never search candidates"
        );
        // dev + none → candidate search (source repo resolves).
        assert_eq!(
            resolve_data_root(None, false, &candidates),
            Some(candidate.clone()),
            "dev with no recorded root must fall back to the candidate search"
        );
        // dev + recorded → recorded (recorded still wins in dev when present).
        assert_eq!(
            resolve_data_root(Some(recorded.as_path()), false, &candidates),
            Some(recorded),
            "a recorded root wins in dev too"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// FIX 4: `is_bundled_or_staged_root` must match the Cargo/Tauri staging SHAPE,
    /// not any path component literally named `target`. A legitimate user dir like
    /// `~/target/Aspis Management` must NOT be excluded; `.../target/debug/_up_`
    /// (and `target/release`, `target/bundle`, or any `_up_`) MUST be excluded.
    #[test]
    fn is_bundled_or_staged_root_matches_staging_shape_not_any_target() {
        // A user dir that merely happens to contain a `target` component — NOT
        // followed by a build profile and with no `_up_` — must be allowed.
        let user_dir = PathBuf::from("/home/alice/target/Aspis Management");
        assert!(
            !is_bundled_or_staged_root(&user_dir),
            "a user dir named `target` (no debug/release/bundle/_up_) must not be excluded"
        );

        // Real Cargo/Tauri staging shapes must be excluded.
        for staged in [
            "/repo/src-tauri/target/debug/_up_",
            "/repo/src-tauri/target/release",
            "/repo/src-tauri/target/debug",
            "/repo/src-tauri/target/release/bundle",
            "/repo/src-tauri/target/bundle",
            "/somewhere/_up_/oracle",
        ] {
            assert!(
                is_bundled_or_staged_root(&PathBuf::from(staged)),
                "staging shape must be excluded: {staged}"
            );
        }
    }

    /// FIX 2: when the package/import root cannot be resolved, the server command
    /// builder returns an explicit Err (no doomed spawn), surfacing the real cause.
    #[test]
    fn build_oracle_server_command_errors_when_package_root_absent() {
        let _guard = test_oracle_lock().lock().unwrap();
        let root = PathBuf::from("..");
        let err = build_oracle_server_command_with_package_root(&root, None)
            .expect_err("absent package root must produce an Err, not a Command");
        assert_eq!(err, MISSING_PACKAGE_ROOT_ERROR);
    }

    /// FAIL-CLOSED runtime interpreter (resident server respawn-loop fix): when the
    /// venv runtime is NOT installed, the resident-server command builder must
    /// PROPAGATE the runtime-missing Err (no doomed spawn on system python that
    /// crashes instantly and drives the ~10s supervisor respawn loop / conhost
    /// flashes). We force a present package root (so the FIX-2 gate passes) and rely
    /// on the test environment having no installed venv runtime, so the interpreter
    /// resolver is the path that fails. If a real venv happens to be installed in the
    /// dev environment the build succeeds — assert it then points at the venv python,
    /// never a bare literal.
    #[test]
    fn build_oracle_server_command_propagates_runtime_missing_error() {
        let _guard = test_oracle_lock().lock().unwrap();
        let root = PathBuf::from("..");
        // A non-None package root so this exercises the INTERPRETER gate, not the
        // package-root gate already covered above.
        let package_root = Some(PathBuf::from("."));
        match build_oracle_server_command_with_package_root(&root, package_root) {
            Err(err) => {
                // The fail-closed interpreter Err must surface (NOT the package-root
                // one), and must never be a bare interpreter literal.
                assert_ne!(err, MISSING_PACKAGE_ROOT_ERROR);
                assert_ne!(err, "python");
                assert_ne!(err, "python3");
            }
            Ok(command) => {
                // Only reachable when a real venv runtime IS installed locally: the
                // program must then be the venv interpreter, never a bare literal.
                let program = command.get_program().to_string_lossy().to_string();
                assert_ne!(program, "python");
                assert_ne!(program, "python3");
                assert!(
                    program.contains("venv"),
                    "installed runtime must resolve to the venv interpreter: {program}"
                );
            }
        }
    }

    /// Regression for the resident-server "API key is not configured" bug: the
    /// spawn path must inject `ORACLE_LLM_API_KEY` (the resident server reads its
    /// creds ONLY from its own env). We assert the `apply_llm_env` seam directly:
    /// a supplied config sets the key (+ provider/model/flags + fallback), and a
    /// `None` config sets NONE of the `ORACLE_LLM_*` env (server stays extractive).
    #[test]
    fn apply_llm_env_injects_key_when_config_present_and_nothing_when_absent() {
        fn env_value<'a>(command: &'a Command, key: &str) -> Option<&'a std::ffi::OsStr> {
            command
                .get_envs()
                .find(|(k, _)| *k == std::ffi::OsStr::new(key))
                .and_then(|(_, v)| v)
        }

        // (a) None config -> no ORACLE_LLM_* env at all.
        let mut bare = Command::new("python");
        apply_llm_env(&mut bare, None);
        assert!(
            env_value(&bare, "ORACLE_LLM_API_KEY").is_none(),
            "no config must not set ORACLE_LLM_API_KEY"
        );
        assert!(
            env_value(&bare, "ORACLE_LLM_PROVIDER").is_none(),
            "no config must not set ORACLE_LLM_PROVIDER"
        );

        // (b) Config with a key -> key + provider/model + base_url present.
        let config = OracleLlmRuntimeConfig {
            provider: "scaleway".into(),
            model: "voxtral".into(),
            base_url: Some("https://example.test/v1".into()),
            api_key: Some("secret-key".into()),
        };
        let mut command = Command::new("python");
        apply_llm_env(&mut command, Some(&config));

        assert_eq!(
            env_value(&command, "ORACLE_LLM_API_KEY"),
            Some(std::ffi::OsStr::new("secret-key")),
            "the resident server must inherit ORACLE_LLM_API_KEY"
        );
        assert_eq!(
            env_value(&command, "ORACLE_LLM_PROVIDER"),
            Some(std::ffi::OsStr::new("scaleway"))
        );
        assert_eq!(
            env_value(&command, "ORACLE_LLM_MODEL"),
            Some(std::ffi::OsStr::new("voxtral"))
        );
        assert_eq!(
            env_value(&command, "ORACLE_LLM_BASE_URL"),
            Some(std::ffi::OsStr::new("https://example.test/v1"))
        );
        // The removed fallback/privacy-gate env must never be set.
        assert!(env_value(&command, "ORACLE_LLM_ZDR_REQUIRED").is_none());
        assert!(env_value(&command, "ORACLE_LLM_GDPR_REQUIRED").is_none());
        assert!(env_value(&command, "ORACLE_LLM_FALLBACK_API_KEY").is_none());
    }

    /// FINDING 2: the manual `Debug` MUST NOT leak the plaintext API key (the derived
    /// one would dump it into the world-readable oracle-server.stderr.log on any
    /// `{:?}`/panic). It must render `[redacted]` for a present key and `None` when
    /// absent, while still showing the non-secret fields.
    #[test]
    fn debug_redacts_api_key_for_runtime_config() {
        let config = OracleLlmRuntimeConfig {
            provider: "scaleway".into(),
            model: "voxtral".into(),
            base_url: Some("https://example.test/v1".into()),
            api_key: Some("super-secret-primary-key".into()),
        };
        let rendered = format!("{config:?}");

        // No plaintext key may appear anywhere in the output.
        assert!(
            !rendered.contains("super-secret-primary-key"),
            "Debug leaked the api_key: {rendered}"
        );
        // The redaction placeholder must be present for the Some-keyed config.
        assert!(
            rendered.contains("[redacted]"),
            "expected [redacted] for the key: {rendered}"
        );
        // Non-secret fields are still observable for diagnostics.
        assert!(
            rendered.contains("scaleway"),
            "provider must be shown: {rendered}"
        );
        assert!(
            rendered.contains("voxtral"),
            "model must be shown: {rendered}"
        );

        // Absent key -> `None`, not `[redacted]`, and still no leak.
        let no_key = OracleLlmRuntimeConfig {
            api_key: None,
            ..config
        };
        let rendered_none = format!("{no_key:?}");
        assert!(
            rendered_none.contains("api_key: None"),
            "absent key must render as None: {rendered_none}"
        );
        assert!(
            !rendered_none.contains("[redacted]"),
            "absent key must not render [redacted]: {rendered_none}"
        );
    }

    /// The spawn seam: the resident-server command must (a) point at the same
    /// interpreter `resolve_oracle_python()` resolves (the venv under the DATA
    /// root), (b) set `PYTHONPATH` to the PACKAGE root resolved for the index root,
    /// and (c) run with cwd = the index/workspace root. We inspect the built
    /// `Command` WITHOUT spawning a process.
    #[test]
    fn build_oracle_server_command_sets_pythonpath_to_package_root_and_venv_interpreter() {
        let _guard = test_oracle_lock().lock().unwrap();
        // Use the real source repo as the index root so the dev package finder
        // resolves a package root; otherwise PYTHONPATH would be absent and there is
        // nothing meaningful to assert. Skip when not running from the source tree.
        let root = PathBuf::from("..");
        let Some(expected_package_root) = find_oracle_package_root(Some(root.clone())) else {
            return;
        };

        let Ok(command) = build_oracle_server_command(&root) else {
            return;
        };

        // (a) interpreter == the shared resolver's choice (the venv interpreter
        // under the DATA root when installed, else its documented fallback).
        let expected_interpreter = super::super::oracle_setup::resolve_oracle_python();
        assert_eq!(
            command.get_program(),
            std::ffi::OsStr::new(&expected_interpreter),
            "server interpreter must match resolve_oracle_python"
        );

        // (b) PYTHONPATH == the resolved package root (import path), so `-m oracle…`
        // imports even though cwd is the index root.
        let pythonpath = command
            .get_envs()
            .find(|(k, _)| *k == std::ffi::OsStr::new("PYTHONPATH"))
            .and_then(|(_, v)| v)
            .map(|v| v.to_owned());
        assert_eq!(
            pythonpath.as_deref(),
            Some(expected_package_root.as_os_str()),
            "PYTHONPATH must be the package/import root"
        );

        // (c) cwd == the index/workspace root.
        assert_eq!(
            command.get_current_dir(),
            Some(root.as_path()),
            "server cwd must be the index/workspace root"
        );

        // (d) ORACLE_PARENT_PID == this app's pid, so the server's parent-death
        // watchdog can self-exit when the app dies without a clean shutdown
        // (SIGKILL / crash / dev rebuild) — the orphan-server fix.
        let parent_pid = command
            .get_envs()
            .find(|(k, _)| *k == std::ffi::OsStr::new("ORACLE_PARENT_PID"))
            .and_then(|(_, v)| v)
            .map(|v| v.to_owned());
        assert_eq!(
            parent_pid.as_deref(),
            Some(std::ffi::OsString::from(std::process::id().to_string()).as_os_str()),
            "ORACLE_PARENT_PID must be the supervising app's pid"
        );
    }

    /// Bug A regression: the genuinely-unavailable error produced by
    /// `run_python_oracle` (when the HTTP server is down AND no package is found)
    /// must NOT be the old upfront-gate string "Python Oracle data is not
    /// available." — that gate keyed on `python_oracle_available(root)` and
    /// wrongly blocked Ask before the HTTP server was ever tried. The replacement
    /// error is the package-root hint, and any underlying HTTP failure is folded
    /// in so the user sees the real reason rather than the misleading data-gate.
    #[test]
    fn cli_fallback_unavailable_error_is_not_the_old_data_gate() {
        let without_http = cli_fallback_unavailable_error(None);
        assert_eq!(without_http, MISSING_PACKAGE_ROOT_ERROR);
        assert_ne!(
            without_http, "Python Oracle data is not available.",
            "the removed upfront availability gate must not resurface"
        );

        let with_http =
            cli_fallback_unavailable_error(Some("Oracle HTTP request failed: connection refused"));
        assert!(
            with_http.starts_with(MISSING_PACKAGE_ROOT_ERROR),
            "the package-root hint must lead the message"
        );
        assert!(
            with_http.contains("connection refused"),
            "the underlying HTTP failure must be folded into the unavailable error"
        );
        assert_ne!(with_http, "Python Oracle data is not available.");
    }

    #[test]
    fn python_oracle_availability_requires_cli_and_data_files() {
        let root =
            std::env::temp_dir().join(format!("aspis-python-oracle-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("oracle")).unwrap();
        fs::create_dir_all(root.join("oracle-data")).unwrap();
        fs::write(root.join("oracle").join("cli.py"), "").unwrap();
        fs::write(root.join("oracle-data").join("metadata.sqlite"), "").unwrap();

        assert!(!python_oracle_available(&root));

        fs::create_dir_all(root.join("oracle-data").join("vectors.lancedb")).unwrap();
        assert!(python_oracle_available(&root));

        let _ = fs::remove_dir_all(&root);
    }

    /// FIX 1 regression: the env key the Rust spawn sets for idle-exit suppression
    /// MUST be the exact key the Python config reads. We assert the Rust const both
    /// (a) equals the literal canonical name, and (b) actually appears as an
    /// `os.getenv("<key>")` in the committed `oracle/config.py`, so a rename on
    /// either side that silently re-enables idle-exit (the original bug) fails CI.
    #[test]
    fn idle_exit_env_key_matches_python_config() {
        assert_eq!(
            ORACLE_DISABLE_IDLE_EXIT_ENV, "ORACLE_DISABLE_IDLE_EXIT",
            "idle-exit env const drifted from the canonical name"
        );

        // CARGO_MANIFEST_DIR is the src-tauri crate dir; the Python package sits at
        // the repo root one level up.
        let config = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(|repo_root| repo_root.join("oracle").join("config.py"));
        // Be resilient if the source tree layout differs in some CI checkout: only
        // assert the cross-language contract when the file is actually present.
        if let Some(config) = config.filter(|path| path.exists()) {
            let source = fs::read_to_string(&config).expect("read oracle/config.py");
            let needle = format!("os.getenv(\"{ORACLE_DISABLE_IDLE_EXIT_ENV}\"");
            assert!(
                source.contains(&needle),
                "oracle/config.py must read the same idle-exit key the Rust spawn \
                 sets ({ORACLE_DISABLE_IDLE_EXIT_ENV}); looked for `{needle}`"
            );
        }
    }

    #[test]
    fn add_default_user_roots_includes_desktop_management_folder() {
        let Some(profile) = std::env::var_os("USERPROFILE").map(PathBuf::from) else {
            return;
        };
        let mut candidates = Vec::new();
        add_default_user_roots(&mut candidates);

        assert!(candidates
            .iter()
            .any(|candidate| candidate == &profile.join("Desktop").join("Aspis Management")));
    }

    #[test]
    fn python_oracle_answer_deserializes_rust_shape() {
        let json = r#"{
            "mode": "python-oracle",
            "query": "cloudflare worker secret rotation",
            "summary": "Grounded Oracle matches: commands.rs.",
            "results": [{
                "id": "src-tauri/src/backend/commands.rs",
                "label": "commands.rs",
                "node_type": "file",
                "cluster": 1,
                "score": 9.0,
                "file_source": "src-tauri/src/backend/commands.rs",
                "function_primary": "Cloudflare Worker secret rotation command backend.",
                "dependencies": ["src-tauri/src/backend/providers.rs"]
            }]
        }"#;

        let answer: OracleAnswer = parse_python_oracle_json(json).unwrap();

        assert_eq!(answer.mode, "python-oracle");
        assert_eq!(
            answer.results[0].file_source,
            "src-tauri/src/backend/commands.rs"
        );
    }

    #[test]
    fn python_oracle_snapshot_deserializes_rust_shape() {
        let json = r#"{
            "status": "ready",
            "source": "python-oracle",
            "phase": "phase1-python",
            "node_count": 65,
            "edge_count": 0,
            "cluster_count": 11,
            "duplicate_labels": []
        }"#;

        let snapshot: OracleSnapshot = parse_python_oracle_json(json).unwrap();

        assert_eq!(snapshot.source, "python-oracle");
        assert_eq!(snapshot.node_count, 65);
    }

    // Windows-only: `\\?\` verbatim prefixes only ever appear in roots produced
    // by the Windows path APIs (the strip is documented as a no-op elsewhere),
    // so the equivalence only holds where the host parser recognizes the prefix.
    #[cfg(windows)]
    #[test]
    fn oracle_ready_path_compare_accepts_windows_verbatim_prefix() {
        assert_eq!(
            normalize_path_text_for_compare(r"\\?\C:\Users\gualt\Desktop\Aspis Management\"),
            normalize_path_text_for_compare(r"C:\Users\gualt\Desktop\Aspis Management")
        );
    }

    /// The readiness root-match must return Ready (NOT RootMismatch) when the
    /// expected root and the server-reported root are the SAME path differing
    /// only by the Windows `\\?\` verbatim prefix — in BOTH directions
    /// (verbatim expected / plain server, and plain expected / verbatim server).
    /// This is the spurious-mismatch that previously triggered supervisor
    /// respawn churn. Skips on non-Windows where `\\?\` is not a path prefix.
    #[test]
    #[cfg(windows)]
    fn oracle_ready_root_match_is_verbatim_prefix_agnostic_both_directions() {
        let plain = r"C:\Users\gualt\Desktop\aspis bio";
        let verbatim = r"\\?\C:\Users\gualt\Desktop\aspis bio";

        // expected = plain (as the comparison stores it), server reports verbatim.
        let expected_norm = normalize_path_text_for_compare(plain);
        assert!(
            matches!(
                classify_server_root_match(&expected_norm, verbatim),
                ReadyProbe::Ready
            ),
            "plain expected vs verbatim server_root must be Ready"
        );

        // Reverse: expected normalized from a verbatim source, server reports plain.
        let expected_norm_v = normalize_path_text_for_compare(verbatim);
        assert!(
            matches!(
                classify_server_root_match(&expected_norm_v, plain),
                ReadyProbe::Ready
            ),
            "verbatim expected vs plain server_root must be Ready"
        );

        // Negative control: genuinely different roots still report a mismatch.
        assert!(
            matches!(
                classify_server_root_match(&expected_norm, r"C:\Users\gualt\Desktop\other"),
                ReadyProbe::RootMismatch { .. }
            ),
            "a different root must still be RootMismatch"
        );
    }

    /// REGRESSION: the `oracle_server_ready` root comparison must hold end-to-end
    /// between the EXPECTED side (`normalize_existing_path_for_compare`, which
    /// `canonicalize()`s and so on Windows briefly carries the `\\?\` verbatim
    /// prefix) and the SERVER side (the raw string the Python server reports via
    /// `str(Path.cwd().resolve())`, which carries NO prefix). If these ever
    /// diverged, `oracle_server_ready` would return false forever and the Ask path
    /// would respawn-loop / stall — the exact symptom under investigation. Using a
    /// real existing directory (the crate's parent, the workspace root) exercises
    /// the actual canonicalize, not a hand-built string.
    #[test]
    fn oracle_ready_expected_root_matches_server_reported_resolve() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crate manifest dir has a parent");
        let expected =
            normalize_existing_path_for_compare(dir).expect("existing dir canonicalizes");
        // Mirror what the Python server publishes: `str(Path.cwd().resolve())` for
        // the same directory. `std::fs::canonicalize` is Rust's resolve(); the
        // server side is then run through the SAME text normalizer the comparison
        // uses, so the two sides must be byte-identical after normalization.
        let server_reported = std::fs::canonicalize(dir).expect("resolve existing dir");
        let server_side = normalize_path_text_for_compare(&server_reported.to_string_lossy());
        assert_eq!(
            expected, server_side,
            "expected_root and the server-reported root must normalize equal"
        );
    }

    /// The mismatch-diagnostic log line must NEVER contain the absolute path: it
    /// emits only the final component plus a short hash, so the username/machine
    /// layout cannot leak into logs (privacy invariant).
    #[test]
    fn redact_root_for_log_keeps_last_component_and_hides_full_path() {
        let full = if cfg!(windows) {
            r"c:\users\gualt\desktop\aspis management"
        } else {
            "/home/gualt/desktop/aspis management"
        };
        let redacted = redact_root_for_log(full);
        assert!(
            redacted.contains("aspis management"),
            "keeps the workspace folder name: {redacted}"
        );
        assert!(
            !redacted.contains("gualt"),
            "must not leak the username segment: {redacted}"
        );
        assert!(
            !redacted.contains(full),
            "must not contain the full path: {redacted}"
        );
        // Distinct roots produce distinct redactions (the hash disambiguates).
        let other = redact_root_for_log("c:\\other\\aspis management");
        assert_ne!(redacted, other, "different roots must redact differently");
    }

    /// The async command layer enforces `ORACLE_CALL_HARD_TIMEOUT` as the absolute
    /// upper bound on a single Ask. It must be finite, must be at least as long as
    /// one request (`PYTHON_ORACLE_TIMEOUT`) so a legitimately slow-but-answering
    /// call is not cut off, and must stay within a UI-tolerable ceiling so the
    /// spinner can never outlive it. This locks the "Ask is always bounded"
    /// invariant against future edits to the component timeouts.
    #[test]
    fn oracle_call_hard_timeout_is_a_sane_finite_bound() {
        assert!(
            ORACLE_CALL_HARD_TIMEOUT >= PYTHON_ORACLE_TIMEOUT,
            "the overall cap must not cut off a single in-flight request"
        );
        assert!(
            ORACLE_CALL_HARD_TIMEOUT >= ORACLE_SERVER_START_TIMEOUT + PYTHON_ORACLE_TIMEOUT,
            "the cap must cover a cold start followed by the request"
        );
        assert!(
            ORACLE_CALL_HARD_TIMEOUT <= Duration::from_secs(300),
            "the cap must stay within a UI-tolerable ceiling"
        );
    }

    /// The bounding MECHANISM used by `try_python_oracle_with_llm`: a worker that
    /// never sends must NOT keep the caller waiting past the deadline —
    /// `recv_timeout` returns `Err` (→ typed timeout) rather than blocking forever.
    /// Deterministic: a 50ms deadline against a worker that sleeps far longer.
    #[test]
    fn recv_timeout_bounds_a_never_answering_worker() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(30));
            let _ = tx.send(());
        });
        let started = Instant::now();
        let outcome = rx.recv_timeout(Duration::from_millis(50));
        assert!(outcome.is_err(), "must time out, not receive");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "must return promptly at the deadline, not wait for the worker"
        );
    }

    #[test]
    fn oracle_ask_payload_keeps_query_out_of_request_url() {
        let args = vec![
            "--query".into(),
            "secret project question".into(),
            "--limit".into(),
            "5".into(),
        ];

        let payload = oracle_ask_payload(&args);

        assert_eq!(payload["query"], "secret project question");
        assert_eq!(payload["limit"], 5);
        assert!(oracle_command_url("http://127.0.0.1:1234", "ask", &args).is_err());
    }

    #[test]
    fn run_python_oracle_reads_project_snapshot_when_available() {
        let _guard = test_oracle_lock().lock().unwrap();
        let root = PathBuf::from("..");
        if !python_oracle_available(&root) {
            return;
        }
        let _ = stop_python_oracle_runtime();
        let root = root.canonicalize().unwrap();

        // P1: the command paths no longer spawn the resident server — the
        // supervisor is the SOLE spawner. Bring it up explicitly (the supervisor's
        // role) before querying, mirroring production where `reconcile_once` has
        // started it before any `run_python_oracle` call.
        ensure_oracle_server(&root, &AtomicBool::new(false))
            .expect("supervisor brings the server up");

        let snapshot: OracleSnapshot =
            run_python_oracle(&root, "snapshot", &[], None, &AtomicBool::new(false), None).unwrap();

        assert_eq!(snapshot.source, "python-oracle");
        assert!(snapshot.node_count > 0);

        let _ = stop_python_oracle_runtime();
    }

    #[test]
    fn concurrent_oracle_server_startup_spawns_once() {
        let _guard = test_oracle_lock().lock().unwrap();
        let root = PathBuf::from("..");
        if !python_oracle_available(&root) {
            return;
        }
        let root = root.canonicalize().unwrap();
        let _ = stop_python_oracle_runtime();
        ORACLE_SERVER_SPAWN_COUNT.store(0, Ordering::SeqCst);

        let handles = (0..8)
            .map(|_| {
                let root = root.clone();
                thread::spawn(move || ensure_oracle_server(&root, &AtomicBool::new(false)))
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle.join().unwrap().unwrap();
        }
        assert!(oracle_server_ready(&root));
        assert_eq!(ORACLE_SERVER_SPAWN_COUNT.load(Ordering::SeqCst), 1);

        let _ = stop_python_oracle_runtime();
    }

    /// Double-spawn fix #1 (wait side): a supervisor whose stop flag is ALREADY set
    /// must ABANDON the readiness wait immediately with `Aborted` — never burning the
    /// 60s start timeout and never touching the child. This is what lets a superseded
    /// supervisor exit within ~one poll slice so the replacement is the only one
    /// (re)spawning. No live server is needed: the pre-set flag short-circuits the
    /// very first loop iteration.
    #[test]
    fn wait_aborts_immediately_when_stop_is_already_set() {
        let _guard = test_oracle_lock().lock().unwrap();
        // A root whose server is definitely not serving.
        let root =
            std::env::temp_dir().join(format!("aspis-oracle-wait-abort-{}", std::process::id()));
        let _ = fs::create_dir_all(&root);

        let stop = AtomicBool::new(true); // supervisor already told to stop
        let started = Instant::now();
        let outcome = wait_for_oracle_server_ready(&root, &stop).expect("wait must not error");
        let elapsed = started.elapsed();

        assert!(
            matches!(outcome, ServerWaitOutcome::Aborted),
            "a pre-set stop flag must abort the wait (no respawn, no child teardown)"
        );
        // Must bail far below the full start timeout (well under one poll slice).
        assert!(
            elapsed < Duration::from_secs(1),
            "abort must be near-instant, took {elapsed:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// Double-spawn fix #1 (ensure side): a stopping supervisor's `ensure_oracle_server`
    /// must NOT spawn a server — it returns the typed Aborted error without incrementing
    /// the spawn count and without touching the child handle. This is the guard that, on
    /// a lock→unlock, keeps the superseded supervisor from running its (re)spawn to
    /// completion while the replacement also spawns (the double-spawn).
    #[test]
    fn ensure_does_not_spawn_when_stop_is_set() {
        let _guard = test_oracle_lock().lock().unwrap();
        let _ = stop_python_oracle_runtime();
        // No child recorded after teardown.
        if let Some(child_lock) = ORACLE_CHILD.get() {
            let mut slot = child_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(mut child) = slot.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        let root =
            std::env::temp_dir().join(format!("aspis-oracle-ensure-abort-{}", std::process::id()));
        let _ = fs::create_dir_all(&root);

        let stop = AtomicBool::new(true);
        let before = ORACLE_SERVER_SPAWN_COUNT.load(Ordering::SeqCst);
        let started = Instant::now();
        let err = ensure_oracle_server(&root, &stop).expect_err("stopping ⇒ Err (aborted)");
        let elapsed = started.elapsed();

        assert_eq!(err, ORACLE_SERVER_ABORTED_ERROR);
        assert!(
            elapsed < Duration::from_secs(1),
            "abort must be near-instant, took {elapsed:?}"
        );
        assert_eq!(
            ORACLE_SERVER_SPAWN_COUNT.load(Ordering::SeqCst),
            before,
            "a stopping supervisor must never spawn the resident server"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// FIX 5 regression: when the recorded child is gone/dead and the server is not
    /// answering at `root`, `wait_for_oracle_server_ready` must return `ChildDied`
    /// PROMPTLY (so `ensure_oracle_server` can respawn immediately) instead of
    /// burning the full `ORACLE_SERVER_START_TIMEOUT`. This is the dead-but-recorded
    /// child guard that prevents the 60s stall on the first query after a rapid
    /// lock→unlock.
    #[test]
    fn wait_returns_child_died_fast_when_no_child_and_not_ready() {
        let _guard = test_oracle_lock().lock().unwrap();
        // Ensure no child handle is recorded so `oracle_child_is_running()` is false.
        if let Some(child_lock) = ORACLE_CHILD.get() {
            let mut slot = child_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(mut child) = slot.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        // A root whose server is definitely not serving (a transient temp dir).
        let root =
            std::env::temp_dir().join(format!("aspis-oracle-wait-fix5-{}", std::process::id()));
        let _ = fs::create_dir_all(&root);

        let started = Instant::now();
        let outcome = wait_for_oracle_server_ready(&root, &AtomicBool::new(false))
            .expect("wait must not error");
        let elapsed = started.elapsed();

        assert!(
            matches!(outcome, ServerWaitOutcome::ChildDied),
            "no recorded child + not ready must report ChildDied for an immediate respawn"
        );
        // Must bail far below the full start timeout (the whole point of the guard).
        assert!(
            elapsed < ORACLE_SERVER_START_TIMEOUT / 4,
            "guard must bail promptly, not stall the full timeout (took {elapsed:?})"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// Finding 1: a `StillStarting` child whose TOTAL age is past the hung timeout is
    /// classified for teardown/respawn, while a fresh one within the window is kept as
    /// "starting". Deterministic — the age is injected, NOT slept out, so the test is
    /// instant and never sleeps the real 300s hung timeout.
    #[test]
    fn still_starting_hung_child_is_classified_for_respawn() {
        let hung_timeout = Duration::from_millis(100);

        // Fresh / progressing boot: age well under the hung timeout → KEEP (no kill).
        assert!(
            !still_starting_child_is_hung(Some(Duration::from_millis(10)), hung_timeout),
            "a child still within the hung window must be kept as starting (no kill)"
        );

        // Exactly at the bound is still "within" (strictly-greater triggers): KEEP.
        assert!(
            !still_starting_child_is_hung(Some(hung_timeout), hung_timeout),
            "a child exactly at the bound must be kept (only strictly-past is hung)"
        );

        // Past the hung timeout: a wedged child → FORCE-REPLACE.
        assert!(
            still_starting_child_is_hung(Some(Duration::from_millis(101)), hung_timeout),
            "a child past the hung timeout must be classified for teardown/respawn"
        );

        // No spawn stamp recorded: cannot prove it is hung → conservatively KEEP.
        assert!(
            !still_starting_child_is_hung(None, hung_timeout),
            "without an age we must not force-kill a child we may not own"
        );
    }

    /// Finding 1: the spawn stamp set in `spawn_oracle_server` and cleared in
    /// `kill_oracle_child` keeps `oracle_child_age` in lock-step with the tracked
    /// child slot, so a torn-down child never leaves a stale age behind. Exercised
    /// directly against the global slots (no real server process needed).
    #[test]
    fn child_age_tracks_spawn_stamp_lifecycle() {
        let _guard = test_oracle_lock().lock().unwrap();
        // Clear any residue from other tests.
        *oracle_child_spawn()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        assert!(
            oracle_child_age().is_none(),
            "no stamp ⇒ no age (a missing-stamp child is never treated as hung)"
        );

        // Simulate a spawn that happened ~10s ago (monotonic), as the supervisor
        // would record it.
        *oracle_child_spawn()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Instant::now() - Duration::from_secs(10));
        let age = oracle_child_age().expect("a recorded stamp yields an age");
        assert!(
            age >= Duration::from_secs(10) && age < Duration::from_secs(60),
            "age must reflect the recorded spawn stamp, got {age:?}"
        );

        // Tearing down (kill clears the stamp) must leave no stale age.
        *oracle_child_spawn()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        assert!(
            oracle_child_age().is_none(),
            "clearing the stamp must clear the age in lock-step"
        );
    }

    /// P1: the NON-spawning command gate must NOT spawn a server. When the resident
    /// server is not up, `require_oracle_server_ready` returns the fast typed
    /// "starting" error (no spawn, no 165s wait) and the recorded child handle stays
    /// untouched — exactly so a command can never race a second server onto the held
    /// session port. The error classifies as ServerUnavailable so the UI shows a
    /// quick "try again" rather than a hard failure.
    #[test]
    fn require_server_ready_does_not_spawn_and_returns_starting() {
        let _guard = test_oracle_lock().lock().unwrap();
        let _ = stop_python_oracle_runtime();
        // No child recorded after teardown.
        if let Some(child_lock) = ORACLE_CHILD.get() {
            let mut slot = child_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(mut child) = slot.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        let root = std::env::temp_dir().join(format!("aspis-oracle-gate-{}", std::process::id()));
        let _ = fs::create_dir_all(&root);

        let before = ORACLE_SERVER_SPAWN_COUNT.load(Ordering::SeqCst);
        let started = Instant::now();
        let err = require_oracle_server_ready(&root).expect_err("not ready ⇒ Err");
        let elapsed = started.elapsed();

        assert_eq!(err, ORACLE_SERVER_STARTING_ERROR);
        // Must return within the bounded probe budget, NEVER the 165s hard cap.
        assert!(
            elapsed < ORACLE_SERVER_HEALTH_TIMEOUT + Duration::from_secs(2),
            "gate must be a cheap bounded probe, took {elapsed:?}"
        );
        // It must NOT have spawned a server (only the supervisor spawns).
        assert_eq!(
            ORACLE_SERVER_SPAWN_COUNT.load(Ordering::SeqCst),
            before,
            "the command gate must never spawn the resident server"
        );
        // The "starting" message classifies as a fast, typed ServerUnavailable.
        let typed = crate::oracle::oracle_error::OracleError::from_python(&err);
        assert_eq!(
            typed.kind,
            crate::oracle::oracle_error::OracleErrorKind::ServerUnavailable
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// P1: `wait_for_oracle_port_free` must report the port free when nothing holds
    /// it, and must bail with an error (instead of letting a doomed second server
    /// spawn) when the session port is held by a live socket — bounded by
    /// `ORACLE_PORT_FREE_TIMEOUT`, never hanging.
    #[test]
    fn wait_for_port_free_detects_free_and_held_port() {
        let _guard = test_oracle_lock().lock().unwrap();
        let port = oracle_http_session().port;
        // A never-set stop flag → behavior identical to the pre-stop signature.
        let never_stop = AtomicBool::new(false);

        // Nothing holds it (best effort: the test runner does not run a server here):
        // a free port returns Ok promptly.
        if oracle_port_is_bindable(port) {
            assert!(wait_for_oracle_port_free(&never_stop).is_ok());
        }

        // Now hold the port with a real listener and assert the wait bails (bounded).
        let listener = std::net::TcpListener::bind(("127.0.0.1", port))
            .expect("bind session port for the held-port case");
        let started = Instant::now();
        let result = wait_for_oracle_port_free(&never_stop);
        let elapsed = started.elapsed();
        assert!(
            result.is_err(),
            "a held session port must not be reported free (would spawn a doomed server)"
        );
        assert!(
            elapsed < ORACLE_PORT_FREE_TIMEOUT + Duration::from_secs(2),
            "the port-free wait must stay bounded, took {elapsed:?}"
        );
        drop(listener);
    }

    /// Fix 2: a stopping supervisor must abandon `wait_for_oracle_port_free` PROMPTLY
    /// (within ~one poll slice) instead of holding `oracle_server_start_lock` for the
    /// full `ORACLE_PORT_FREE_TIMEOUT` while a newer supervisor waits. With the stop
    /// flag pre-set the wait returns the aborted sentinel near-instantly even though
    /// the port is genuinely held (would otherwise burn the whole timeout).
    #[test]
    fn wait_for_port_free_honors_stop_flag() {
        let _guard = test_oracle_lock().lock().unwrap();
        let port = oracle_http_session().port;

        // Hold the port so the wait would, absent a stop, spin to the full timeout.
        let listener = std::net::TcpListener::bind(("127.0.0.1", port))
            .expect("bind session port for the stop-flag case");

        let stop = AtomicBool::new(true); // supervisor already told to stop
        let started = Instant::now();
        let result = wait_for_oracle_port_free(&stop);
        let elapsed = started.elapsed();

        assert_eq!(
            result.err().as_deref(),
            Some(ORACLE_SERVER_ABORTED_ERROR),
            "a stop-signalled wait must return the aborted sentinel, not a port error"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "a stop-signalled port-free wait must return promptly, took {elapsed:?}"
        );
        drop(listener);
    }

    #[test]
    fn parse_oracle_doctor_report_ignores_leading_chatter() {
        let stdout = "Loading weights: 100%\nsome import noise\n{\"ok\":true,\"checks\":[{\"id\":\"runtime\",\"ok\":true,\"detail\":\"d\",\"remediation\":\"\"}]}\n";
        let report = parse_oracle_doctor_report(stdout).expect("parse report");
        assert!(report.ok);
        assert_eq!(report.checks.len(), 1);
        assert_eq!(report.checks[0].id, "runtime");
    }

    #[test]
    fn parse_oracle_doctor_report_errors_on_garbage() {
        let err = parse_oracle_doctor_report("not json at all\n").unwrap_err();
        assert!(err.contains("invalid"), "unexpected error: {err}");
    }

    /// A cross-platform `Command` that sleeps ~`secs` seconds without producing
    /// output. Used by the timeout/cancellation tests so they don't depend on a
    /// particular shell being present (Windows: `powershell`; Unix: `sh -c sleep`).
    fn sleep_command(secs: u32) -> Command {
        #[cfg(windows)]
        {
            let mut command = Command::new("powershell");
            command.args([
                "-NoProfile",
                "-Command",
                &format!("Start-Sleep -Seconds {secs}"),
            ]);
            command
        }
        #[cfg(not(windows))]
        {
            let mut command = Command::new("sh");
            command.args(["-c", &format!("sleep {secs}")]);
            command
        }
    }

    /// A cross-platform `Command` that prints a line to stdout AND stderr and then
    /// sleeps ~`secs` seconds. The early output fills the pipes so the reader threads
    /// are actively spawned/blocked; used by the BLOCKER 2 regression to prove the
    /// timeout path drains/joins those readers (it must not block forever, and the
    /// child's death must close the pipes so the readers unblock and join).
    fn noisy_sleep_command(secs: u32) -> Command {
        #[cfg(windows)]
        {
            let mut command = Command::new("powershell");
            command.args([
                "-NoProfile",
                "-Command",
                &format!(
                    "Write-Output 'out'; [Console]::Error.WriteLine('err'); Start-Sleep -Seconds {secs}"
                ),
            ]);
            command
        }
        #[cfg(not(windows))]
        {
            let mut command = Command::new("sh");
            command.args(["-c", &format!("echo out; echo err 1>&2; sleep {secs}")]);
            command
        }
    }

    #[test]
    fn run_with_timeout_kills_slow_process() {
        let result = run_with_timeout(sleep_command(5), Duration::from_millis(150), None);

        assert!(result.unwrap_err().contains("timeout"));
    }

    /// Fix 2: a child that has already exited is reaped on the FIRST `try_wait`,
    /// so `reap_child_bounded` returns `true` essentially immediately (the normal
    /// fast-exit case must not regress / must not sleep out the deadline).
    #[test]
    fn reap_child_bounded_returns_immediately_for_exited_child() {
        // A trivially-fast command; wait for it to exit so try_wait sees Ready.
        let mut child = sleep_command(0).spawn().expect("spawn fast child");
        let _ = child.wait();
        let started = Instant::now();
        let reaped = reap_child_bounded(&mut child, Duration::from_secs(5));
        assert!(reaped, "an already-exited child must report reaped");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "fast-exit reap must return promptly, took {:?}",
            started.elapsed()
        );
    }

    /// Fix 2: a child that does NOT exit must NOT block the (single supervisor)
    /// thread unbounded — `reap_child_bounded` returns `false` once the SHORT test
    /// deadline elapses (we use a long-sleeping child + a ~300ms deadline; no real
    /// long sleeps), proving the bound. We kill the child afterward to avoid leaks.
    #[test]
    fn reap_child_bounded_returns_false_for_non_exiting_child_at_deadline() {
        let mut child = sleep_command(30).spawn().expect("spawn long child");
        let deadline = Duration::from_millis(300);
        let started = Instant::now();
        let reaped = reap_child_bounded(&mut child, deadline);
        let elapsed = started.elapsed();
        // Clean up the still-running child before asserting.
        let _ = child.kill();
        let _ = child.wait();

        assert!(
            !reaped,
            "a child still alive past the deadline must report NOT reaped (detach)"
        );
        // Bounded by deadline + one poll interval (+ scheduling slack).
        assert!(
            elapsed < Duration::from_secs(2),
            "reap must be bounded near the deadline, took {elapsed:?}"
        );
    }

    /// BLOCKER 2 regression: the timeout path must kill the child AND join its two
    /// pipe-reader threads. The child emits output up front (so both readers are
    /// spawned and blocked on `read_to_end`) and then sleeps past the op timeout.
    /// `run_with_timeout` must return the timeout error PROMPTLY — proving it did not
    /// (a) leave the readers blocked forever (a join after a kill that never closed
    /// the pipes would hang the test), or (b) skip joining them. If the readers were
    /// not joined, the helper still returns, but the leak-shaped hang we guard
    /// against would manifest as this test exceeding its prompt-return bound.
    #[test]
    fn run_with_timeout_joins_readers_on_timeout_with_output() {
        let started = Instant::now();
        let result = run_with_timeout(noisy_sleep_command(30), Duration::from_millis(300), None);
        let elapsed = started.elapsed();

        assert!(
            result.unwrap_err().contains("timeout"),
            "must report the timeout error"
        );
        // Must return promptly after the op timeout: the kill closes the child's
        // stdout/stderr, the readers unblock, and the joins complete. A hang here
        // would mean a reader was left blocked / not joined.
        assert!(
            elapsed < Duration::from_secs(5),
            "timeout path did not wind down promptly (readers not drained/joined?): took {elapsed:?}"
        );
    }

    /// BLOCKER 1/2 + cancel regression: same noisy child, but cancelled mid-flight.
    /// The cancel path must also kill + drain/join both readers and return the
    /// bounded cancellation error promptly.
    #[test]
    fn run_with_timeout_joins_readers_on_cancel_with_output() {
        use std::sync::Arc;
        let cancel = Arc::new(AtomicBool::new(false));
        let flipper = Arc::clone(&cancel);
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            flipper.store(true, Ordering::Relaxed);
        });

        let started = Instant::now();
        let result = run_with_timeout(
            noisy_sleep_command(30),
            Duration::from_secs(30),
            Some(&cancel),
        );
        let elapsed = started.elapsed();
        handle.join().unwrap();

        assert_eq!(result.unwrap_err(), ORACLE_CALL_CANCELLED_ERROR);
        assert!(
            elapsed < Duration::from_secs(5),
            "cancel path did not wind down promptly (readers not drained/joined?): took {elapsed:?}"
        );
    }

    /// NITPICK 1 regression: an oversized supervisor log is rotated in place to its
    /// last `ORACLE_SUPERVISOR_LOG_KEEP_LINES` lines on the next append, and the most
    /// recent line is retained. A small log is left untouched.
    #[test]
    fn supervisor_log_rotates_when_oversized() {
        let root = unique_temp("supervisor-log-rotate");
        let log_dir = root.join("oracle-data").join("logs");
        fs::create_dir_all(&log_dir).unwrap();
        let log_path = log_dir.join("oracle-supervisor.log");

        // Write > 1 MiB of distinct lines so the size check trips.
        let mut big = String::new();
        let mut i = 0u32;
        while big.len() <= (ORACLE_SUPERVISOR_LOG_MAX_BYTES as usize) + 1024 {
            big.push_str(&format!("old-line-{i}\n"));
            i += 1;
        }
        fs::write(&log_path, &big).unwrap();
        assert!(fs::metadata(&log_path).unwrap().len() > ORACLE_SUPERVISOR_LOG_MAX_BYTES);

        // Append one event → triggers rotation, then writes the new line.
        log_oracle_supervisor_event(&root, "fresh-event-marker");

        let after = fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = after.lines().collect();
        assert!(
            lines.len() <= ORACLE_SUPERVISOR_LOG_KEEP_LINES + 1,
            "log not rotated: {} lines remain",
            lines.len()
        );
        assert!(
            after.contains("fresh-event-marker"),
            "the just-appended event must survive rotation"
        );
        assert!(
            (fs::metadata(&log_path).unwrap().len()) < ORACLE_SUPERVISOR_LOG_MAX_BYTES,
            "rotated log must be back under the cap"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// WARNING 3 regression: a probe against a root with NO resident server (the
    /// loopback port is unused → connection refused) must map to `NotReady`, NOT
    /// `RootMismatch`. This is the branch that — in the cli-fallback lock section —
    /// STILL retries `run_python_oracle_http` (whose `ensure_oracle_server`
    /// waits/restarts) before any CLI fallback. Mapping a transient/unreachable probe
    /// to `RootMismatch` would wrongly skip that HTTP retry and force the heavy CLI
    /// subprocess. No outbound network: only a loopback connect that is refused.
    #[test]
    fn probe_maps_unreachable_server_to_notready_not_rootmismatch() {
        let _guard = test_oracle_lock().lock().unwrap();
        // Make sure no server we (or another test) started is answering: tear down
        // any tracked child first. The session base URL is a fixed random loopback
        // port; with no listener the health GET is refused → NotReady.
        let _ = stop_python_oracle_runtime();

        let root = unique_temp("probe-notready");
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();

        match probe_oracle_server_ready(&root) {
            ReadyProbe::NotReady => {}
            ReadyProbe::Ready => {
                panic!("no server is running; probe must not report Ready")
            }
            ReadyProbe::RootMismatch { .. } => panic!(
                "an unreachable/transient probe must be NotReady (retries HTTP), \
                 never RootMismatch (which would skip the HTTP retry → CLI fallback)"
            ),
        }

        // And the thin wrapper agrees it is not ready.
        assert!(!oracle_server_ready(&root));

        let _ = fs::remove_dir_all(&root);
    }

    /// NITPICK 1: a small log must NOT be rotated/truncated — existing history is
    /// preserved and the new line is appended.
    #[test]
    fn supervisor_log_preserved_when_small() {
        let root = unique_temp("supervisor-log-small");
        let log_dir = root.join("oracle-data").join("logs");
        fs::create_dir_all(&log_dir).unwrap();
        let log_path = log_dir.join("oracle-supervisor.log");
        fs::write(&log_path, "2020-01-01T00:00:00Z existing-line\n").unwrap();

        log_oracle_supervisor_event(&root, "appended-line");

        let after = fs::read_to_string(&log_path).unwrap();
        assert!(after.contains("existing-line"), "history must be preserved");
        assert!(after.contains("appended-line"), "new line must be appended");

        let _ = fs::remove_dir_all(&root);
    }

    /// F1 regression: a worker whose `cancel` flag is ALREADY set when it reaches
    /// the CLI subprocess must NOT spawn (or, if racing, must die within one poll
    /// tick) — proving cooperative cancellation shortens the worker path far below
    /// the full `timeout`. Deterministic, no network: a long-sleeping child that
    /// would run 30s is cut to well under a second by a pre-set cancel flag.
    #[test]
    fn run_with_timeout_bails_immediately_when_cancel_preset() {
        let cancel = AtomicBool::new(true);
        let started = Instant::now();

        // Generous 30s op timeout: if cancel were ignored we would block ~30s.
        let result = run_with_timeout(sleep_command(30), Duration::from_secs(30), Some(&cancel));
        let elapsed = started.elapsed();

        let err = result.unwrap_err();
        assert_eq!(err, ORACLE_CALL_CANCELLED_ERROR);
        // The pre-spawn cancel check means we never even launch the child, so this
        // returns near-instantly — orders of magnitude below both the op timeout
        // and the outer hard cap.
        assert!(
            elapsed < Duration::from_secs(2),
            "cancel did not shorten the path: took {elapsed:?}"
        );
    }

    /// F1 regression: a child already running when `cancel` flips mid-flight is
    /// killed within ~one poll tick, not at the full op timeout. Sets cancel from
    /// another thread shortly after the child starts and asserts the wait returns
    /// promptly with the bounded cancellation error.
    #[test]
    fn run_with_timeout_kills_running_child_on_cancel() {
        use std::sync::Arc;
        let cancel = Arc::new(AtomicBool::new(false));
        let flipper = Arc::clone(&cancel);
        // Flip cancel ~200ms in, while the 30s child is mid-run.
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            flipper.store(true, Ordering::Relaxed);
        });

        let started = Instant::now();
        let result = run_with_timeout(sleep_command(30), Duration::from_secs(30), Some(&cancel));
        let elapsed = started.elapsed();
        handle.join().unwrap();

        assert_eq!(result.unwrap_err(), ORACLE_CALL_CANCELLED_ERROR);
        // Killed shortly after the flip, far below the 30s op timeout.
        assert!(
            elapsed < Duration::from_secs(3),
            "cancel did not kill the running child promptly: took {elapsed:?}"
        );
    }
}
