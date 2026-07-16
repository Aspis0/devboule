//! Cross-platform bootstrap for the local Oracle runtime.
//!
//! The app is remote-first for *answers*, but retrieval is mandatory: it needs
//! the Qwen3 embedding model (ONNX int8, rust-only) and LanceDB. The Rust engine
//! (`oracle-core`) runs in-process and downloads the ONNX model automatically.
//! The slim MCP venv (under `oracle-data/venv`) is created here and populated
//! with `oracle/requirements-mcp.txt` (httpx + mcp[cli]) so the project-management
//! MCP server (`oracle/server/aspis_mcp.py`) can be launched by `cli_agents`.
//!
//! Cross-platform note: the venv layout and the `python3`/`py -3` resolution are
//! handled per-OS. The macOS path is written without a Mac to test and is
//! therefore UNVERIFIED — it follows the standard POSIX venv layout.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::python_oracle::{
    apply_no_window, find_oracle_package_root, oracle_data_root, run_with_timeout, ProcessOutput,
};

/// Embedding model the retrieval layer depends on (mirrors `oracle/config.py`).
pub const EMBED_MODEL: &str = "Qwen/Qwen3-Embedding-0.6B";
/// torch + the model are large; allow a generous install window.
const PIP_TIMEOUT: Duration = Duration::from_secs(2400);
/// Generous so a loaded/slow dev machine does not FALSE-timeout on a trivial
/// `py -3 --version`. A real, present interpreter answers in well under a second;
/// the headroom only matters when the box is thrashing, in which case we prefer a
/// soft "still checking" outcome over a scary "no Python found".
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
/// Written into the venv only after a full, successful install. A bare
/// interpreter file is NOT treated as a usable runtime without this.
const VENV_COMPLETE_MARKER: &str = ".oracle-runtime-complete";

/// Serializes installs so two clicks (or an install racing a live Oracle call)
/// cannot pip-clobber the same venv concurrently.
fn install_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// True while a runtime install/repair is holding the install lock. The Oracle
/// supervisor (`oracle_service`) checks this before each restart tick so it does
/// not fight an in-flight install (which STOPS the server first and pip-clobbers
/// the venv). Best-effort: a `try_lock` failure means the lock is held — either
/// by an install or, transiently, by another check, which is the safe answer
/// (skip the tick). Poisoning is also treated as "in progress" (an install
/// panicked mid-flight; do not race it).
pub(crate) fn install_in_progress() -> bool {
    match install_lock().try_lock() {
        Ok(_guard) => false,
        Err(std::sync::TryLockError::WouldBlock) => true,
        Err(std::sync::TryLockError::Poisoned(_)) => true,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OracleRuntimeSetup {
    /// A usable Python 3.10+ interpreter (venv or system) was found.
    pub python_found: bool,
    /// True while the runtime probe could not reach a definitive verdict because
    /// the machine was busy (a probe timed out or failed to spawn transiently).
    /// The UI uses this to show a soft "still checking, retry in a moment"
    /// message instead of the hard "install Python 3" one. Additive + defaulted
    /// so older clients that ignore it keep working.
    #[serde(default)]
    pub checking: bool,
    pub python_command: Option<String>,
    pub python_version: Option<String>,
    /// The Oracle virtual environment exists.
    pub venv_ready: bool,
    /// The slim MCP deps (httpx + mcp[cli]) import successfully in the venv.
    pub deps_ready: bool,
    /// The Qwen3 embedding model (ONNX int8) is downloaded and cached locally.
    pub embedder_ready: bool,
    /// Everything the retrieval layer needs is present.
    pub ready: bool,
    pub embed_model: String,
    pub messages: Vec<String>,
}

impl OracleRuntimeSetup {
    fn unavailable(message: impl Into<String>) -> Self {
        OracleRuntimeSetup {
            python_found: false,
            checking: false,
            python_command: None,
            python_version: None,
            venv_ready: false,
            deps_ready: false,
            embedder_ready: false,
            ready: false,
            embed_model: EMBED_MODEL.to_string(),
            messages: vec![message.into()],
        }
    }
}

// --- path helpers -----------------------------------------------------------

pub fn oracle_venv_dir(root: &Path) -> PathBuf {
    root.join("oracle-data").join("venv")
}

/// The interpreter inside a venv, per-OS (`Scripts\python.exe` on Windows,
/// `bin/python3` on Unix/macOS).
///
/// CPython's `venv` normally creates `bin/python3`, `bin/python` and
/// `bin/pythonX.Y` as symlinks, but some macOS interpreters (framework builds,
/// certain pyenv/Homebrew layouts) emit only a subset. Prefer whichever name
/// actually exists so a venv with only `bin/python` is still recognized as
/// usable; fall back to the canonical `bin/python3` for the not-yet-created case.
pub fn venv_python(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        return venv.join("Scripts").join("python.exe");
    }
    let bin = venv.join("bin");
    for name in ["python3", "python"] {
        let candidate = bin.join(name);
        if candidate.exists() {
            return candidate;
        }
    }
    bin.join("python3")
}

/// A venv that finished a full install (interpreter present AND completion
/// marker written). A half-installed venv (pip failed/timed out) must NOT be
/// treated as usable, or it would shadow a working system Python forever.
///
/// SECURITY (trust boundary, FIX 6): in a RELEASE build the `oracle/` package
/// import path and the `pip install` SOURCE are locked to the read-only bundled
/// package (see `find_oracle_package_root`), and the writable data root is locked
/// to the recorded app-data dir (see `oracle_data_root` / `resolve_data_root`).
/// The venv INTERPRETER itself, however, lives under that user-writable data dir
/// and is trusted as-is: we run whatever `venv_python` resolves there without
/// re-verifying it. Replacing that interpreter binary requires local write access
/// to the user's app-data dir, i.e. a local-privilege threat that is OUTSIDE this
/// boundary (an attacker with that access already controls the account). This is
/// the deliberate edge of the guarantee, not an oversight.
/// Whether the slim MCP venv at `<root>/oracle-data/venv` is fully installed
/// (the completion marker file is present). Used by the Rust-native doctor and
/// the runtime-status probe.
pub(crate) fn venv_complete(root: &Path) -> bool {
    let venv = oracle_venv_dir(root);
    venv_python(&venv).exists() && venv.join(VENV_COMPLETE_MARKER).exists()
}

/// The Python command the running Oracle should use: the venv interpreter (under
/// the DATA root, where the installed runtime lives) only when it is fully
/// installed, then the `PYTHON` override, then the OS default. Cheap (only fs
/// checks + the data-root resolve) so it is safe to call on every Oracle request.
///
/// The venv is resolved from [`oracle_data_root`] — NOT from the package/import
/// root — because the writable runtime and the (possibly read-only, bundled)
/// `oracle/` source can live in different places (see `oracle_data_root`).
pub fn resolve_oracle_python() -> String {
    if let Some(data_root) = oracle_data_root() {
        if venv_complete(&data_root) {
            return venv_python(&oracle_venv_dir(&data_root))
                .to_string_lossy()
                .to_string();
        }
    }
    if let Ok(value) = std::env::var("PYTHON") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    // Version-gated system detection (cached); only if NOTHING >= 3.10 exists do
    // we degrade to the legacy bare default (whose failure mode is at least a
    // clear "no such interpreter" rather than a 3.9 syntax-error traceback).
    cached_system_python().unwrap_or_else(|| default_python_command().to_string())
}

/// FAIL-CLOSED variant of [`resolve_oracle_python`] for SPAWNING the resident
/// server: `Some` only when the managed venv runtime is fully installed (or an
/// explicit `PYTHON` override is set), `None` when the runtime is missing. The
/// resident-server spawn must NOT fall back to a bare system interpreter — that
/// spawn is doomed (no installed deps), crashes instantly, and drives the ~10s
/// supervisor respawn loop the fail-closed contract exists to prevent (see
/// `build_oracle_server_command_propagates_runtime_missing_error`).
pub fn resolve_oracle_runtime_python() -> Option<String> {
    if let Some(data_root) = oracle_data_root() {
        if venv_complete(&data_root) {
            return Some(
                venv_python(&oracle_venv_dir(&data_root))
                    .to_string_lossy()
                    .to_string(),
            );
        }
    }
    if let Ok(value) = std::env::var("PYTHON") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

fn default_python_command() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

/// Pure seam for the fallback: a detection is usable as a `Command::new`
/// program only when it is `Found` with a single-token argv.
fn system_python_program(detection: PythonDetection) -> Option<String> {
    match detection {
        PythonDetection::Found { argv, .. } if argv.len() == 1 => Some(argv[0].clone()),
        _ => None,
    }
}

/// Best usable SYSTEM interpreter (version-gated detection, >= 3.10), cached
/// for the process lifetime so `resolve_oracle_python` stays cheap per request.
/// Without this, the bare-`python3` fallback can land on the macOS Xcode CLT
/// Python 3.9, which cannot parse the oracle package at all.
fn cached_system_python() -> Option<String> {
    static CACHE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| system_python_program(detect_system_python()))
        .clone()
}

// --- system python detection (only needed to CREATE the venv) ---------------

/// Ordered interpreter candidates to try when no venv exists yet. Each entry is
/// an argv prefix (`py -3` needs two tokens).
fn python_candidates() -> Vec<Vec<String>> {
    let mut candidates: Vec<Vec<String>> = Vec::new();
    if let Ok(value) = std::env::var("PYTHON") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            candidates.push(vec![trimmed.to_string()]);
        }
    }
    if cfg!(windows) {
        candidates.push(vec!["py".into(), "-3".into()]);
        candidates.push(vec!["python".into()]);
        candidates.push(vec!["python3".into()]);
    } else {
        for name in PYTHON_INTERPRETER_NAMES {
            candidates.push(vec![(**name).into()]);
        }
        candidates.push(vec!["python".into()]);
        // A macOS `.app` launched from Finder inherits a minimal PATH (often just
        // `/usr/bin:/bin`), so a bare `python3` may not resolve even when Python
        // is installed via Homebrew/pyenv. Probe the usual absolute locations.
        candidates.extend(unix_absolute_python_candidates(&|p: &Path| p.exists()));
    }
    candidates
}

/// Interpreter names worth probing, newest first. The unversioned `python3`
/// is NOT enough: Homebrew installs `python3.12` and only adds the
/// unversioned symlink when the formula is linked, so on many machines the
/// bare name resolves to the Xcode CLT 3.9 (too old for the runtime deps).
const PYTHON_INTERPRETER_NAMES: &[&str] = &[
    "python3",
    "python3.13",
    "python3.12",
    "python3.11",
    "python3.10",
];

/// Expand the well-known unix install dirs x interpreter names into absolute
/// candidate argvs. Pure over the `exists` probe so tests can fake the
/// filesystem.
fn unix_absolute_python_candidates(exists: &dyn Fn(&Path) -> bool) -> Vec<Vec<String>> {
    let mut found: Vec<Vec<String>> = Vec::new();
    for dir in [
        "/opt/homebrew/bin", // Apple Silicon Homebrew
        "/usr/local/bin",    // Intel Homebrew / python.org
        "/usr/bin",          // Xcode Command Line Tools
    ] {
        for name in PYTHON_INTERPRETER_NAMES {
            let abs = format!("{dir}/{name}");
            if exists(Path::new(&abs)) {
                found.push(vec![abs]);
            }
        }
    }
    found
}

fn run_capture(
    program: &str,
    args: &[String],
    timeout: Duration,
    cwd: Option<&Path>,
) -> Result<ProcessOutput, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    apply_no_window(&mut command);
    // Install/setup subprocesses are not part of a user Ask and have no outer
    // hard-timeout cap to cooperate with; they rely solely on their own `timeout`.
    run_with_timeout(command, timeout, None)
}

/// Outcome of probing ONE interpreter candidate. The crucial distinction is
/// between a probe that RAN and proved the candidate is not Python ≥3.10
/// (`NotPython`) and a probe that could not produce a trustworthy answer because
/// the machine was busy — it timed out or the spawn failed transiently
/// (`Inconclusive`). The latter must NOT be reported to the user as "no Python
/// installed", only as "still checking".
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProbeOutcome {
    /// Ran successfully and parsed a Python ≥3.10 version banner.
    Found(String),
    /// Ran to completion but is not a usable Python ≥3.10 (non-zero exit, old
    /// version, or unparseable banner). A definitive negative.
    NotPython,
    /// No trustworthy answer: the probe timed out or the process could not be
    /// spawned (machine busy / transient). NOT a definitive negative.
    Inconclusive,
}

/// Aggregate detection result across all candidates. `Found` wins over anything;
/// otherwise `Inconclusive` (some candidate timed out / failed to spawn) wins
/// over `NotFound` so a busy machine degrades to a soft "still checking" message
/// instead of the hard "install Python" one.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PythonDetection {
    Found {
        argv: Vec<String>,
        version: String,
    },
    /// Every candidate RAN and none is Python ≥3.10 — a definitive negative.
    NotFound,
    /// No candidate was `Found` and at least one was `Inconclusive` (timed
    /// out / busy). The genuine answer is unknown; retry later.
    Inconclusive,
}

/// Classify a `run_with_timeout`/`run_capture` result for ONE candidate into the
/// tri-state. Pure over the (Result, parsed-version) seam so the timeout→
/// Inconclusive mapping is unit-testable without spawning a process.
///
/// - `run_result` is `Ok((status_success, parsed_version))` when the probe ran,
///   or `Err(message)` when `run_with_timeout` failed (timeout OR spawn failure).
/// - A timeout or a spawn failure is `Inconclusive` (transient, machine busy).
/// - A clean run with a non-success exit or no parseable ≥3.9 version is the
///   definitive `NotPython`.
fn classify_probe_outcome(run_result: Result<(bool, Option<String>), String>) -> ProbeOutcome {
    match run_result {
        // The probe never produced a trustworthy answer (timed out or could not
        // spawn). `run_with_timeout` is the only error source here and every one
        // of its error variants is a transient/busy condition, so treat them all
        // as Inconclusive rather than a definitive "no Python".
        Err(_) => ProbeOutcome::Inconclusive,
        Ok((false, _)) => ProbeOutcome::NotPython,
        Ok((true, Some(version))) => ProbeOutcome::Found(version),
        Ok((true, None)) => ProbeOutcome::NotPython,
    }
}

/// Fold per-candidate outcomes into the aggregate detection result. First
/// `Found` wins; else `Inconclusive` if any candidate was inconclusive; else
/// `NotFound`. `candidates` pairs each argv with its probe outcome.
fn classify_python_detection(candidates: Vec<(Vec<String>, ProbeOutcome)>) -> PythonDetection {
    let mut saw_inconclusive = false;
    for (argv, outcome) in candidates {
        match outcome {
            ProbeOutcome::Found(version) => return PythonDetection::Found { argv, version },
            ProbeOutcome::Inconclusive => saw_inconclusive = true,
            ProbeOutcome::NotPython => {}
        }
    }
    if saw_inconclusive {
        PythonDetection::Inconclusive
    } else {
        PythonDetection::NotFound
    }
}

/// Parse a `Python 3.x.y` version banner and require >= 3.10 (the runtime deps
/// — torch / sentence-transformers / lancedb — do not install on 3.9, which is
/// what the macOS Xcode CLT `python3` ships).
fn parse_python_version(text: &str) -> Option<String> {
    let token = text
        .split_whitespace()
        .find(|piece| piece.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    let mut parts = token.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next().unwrap_or("0").parse().unwrap_or(0);
    if major > 3 || (major == 3 && minor >= 10) {
        Some(token.to_string())
    } else {
        None
    }
}

/// Probe one interpreter argv and classify the result into the tri-state. A
/// timeout or spawn failure (machine busy) yields `Inconclusive`; a clean run
/// that is not Python ≥3.10 yields `NotPython`.
fn probe_python_outcome(argv: &[String]) -> ProbeOutcome {
    if argv.is_empty() {
        return ProbeOutcome::NotPython;
    }
    let mut args: Vec<String> = argv[1..].to_vec();
    args.push("--version".to_string());
    let run_result = run_capture(&argv[0], &args, PROBE_TIMEOUT, None).map(|output| {
        let banner = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        (output.status.success(), parse_python_version(&banner))
    });
    classify_probe_outcome(run_result)
}

/// Detect a usable system Python across the ordered candidates, preserving the
/// busy/timeout signal: if no candidate is `Found` but one timed out / failed to
/// spawn, the result is `Inconclusive` rather than a false `NotFound`.
///
/// Short-circuits on the first `Found` (no need to probe the rest), so the common
/// happy path costs a single fast `--version` call.
fn detect_system_python() -> PythonDetection {
    let mut outcomes: Vec<(Vec<String>, ProbeOutcome)> = Vec::new();
    for argv in python_candidates() {
        let outcome = probe_python_outcome(&argv);
        if let ProbeOutcome::Found(version) = &outcome {
            return PythonDetection::Found {
                argv,
                version: version.clone(),
            };
        }
        outcomes.push((argv, outcome));
    }
    classify_python_detection(outcomes)
}

// --- status -----------------------------------------------------------------

/// Report the current status of the Oracle runtime. Since M3 the engine is
/// always Rust (ONNX in-process), so there is no Python subprocess to probe.
/// The status reflects the ONNX model presence + the slim MCP venv readiness.
pub fn current_oracle_runtime_setup() -> OracleRuntimeSetup {
    match oracle_data_root() {
        Some(root) => rust_runtime_setup_status(&root),
        None => OracleRuntimeSetup::unavailable(
            "Could not locate the bundled oracle/ package next to the app.",
        ),
    }
}

fn rust_runtime_setup_status(root: &Path) -> OracleRuntimeSetup {
    let data_dir = rust_model_data_dir(root);
    let model_present = oracle_core::model_download::model_present(&data_dir, true); // int8
    let venv_ready = venv_complete(root);
    let mut messages = Vec::new();
    if model_present {
        messages.push("Rust engine: ONNX embedding model is installed.".to_string());
    } else {
        messages.push("Rust engine: ONNX embedding model not downloaded yet.".to_string());
    }
    if venv_ready {
        messages.push("Slim MCP venv: installed and ready.".to_string());
    } else {
        messages.push("Slim MCP venv: not yet installed.".to_string());
    }
    let ready = model_present && venv_ready;
    OracleRuntimeSetup {
        // The Rust engine needs no Python — report the Python-specific gates as
        // satisfied so the existing UI does not show a spurious "install Python 3".
        python_found: true,
        checking: false,
        python_command: None,
        python_version: None,
        venv_ready,
        deps_ready: venv_ready,
        embedder_ready: model_present,
        ready,
        embed_model: "Qwen/Qwen3-Embedding-0.6B (ONNX int8)".to_string(),
        messages,
    }
}

// --- install ----------------------------------------------------------------

fn create_venv(argv: &[String], venv: &Path) -> Result<(), String> {
    if let Some(parent) = venv.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Could not create Oracle data directory: {e}"))?;
    }
    let mut args: Vec<String> = argv[1..].to_vec();
    args.push("-m".into());
    args.push("venv".into());
    args.push(venv.to_string_lossy().to_string());
    let output = run_capture(&argv[0], &args, PROBE_TIMEOUT, None)?;
    if !output.status.success() {
        return Err(format!(
            "Creating the Python virtual environment failed: {}",
            tail(&output.stderr)
        ));
    }
    Ok(())
}

fn run_pip(venv_py: &Path, pip_args: &[&str], root: &Path) -> Result<(), String> {
    // `-q` keeps pip's notoriously chatty output (progress bars, hash lines)
    // from being buffered unbounded in memory; `--disable-pip-version-check`
    // avoids a spurious network call.
    let mut args = vec![
        "-m".to_string(),
        "pip".to_string(),
        "--quiet".to_string(),
        "--disable-pip-version-check".to_string(),
    ];
    args.extend(pip_args.iter().map(|s| s.to_string()));
    let output = run_capture(&venv_py.to_string_lossy(), &args, PIP_TIMEOUT, Some(root))?;
    if !output.status.success() {
        return Err(format!(
            "pip {} failed: {}",
            pip_args.join(" "),
            tail(&output.stderr)
        ));
    }
    Ok(())
}

/// Detect whether `root` holds a legacy "fat" runtime venv (the old torch-based
/// venv that used to live under `oracle-data/venv`). Returns `true` when any
/// `site-packages/torch` directory is present.
///
/// This is used to detect a venv that needs to be REPLACED during the M3
/// fat→slim migration: the old venv consumed 2-3 GB (torch + transformers),
/// the new slim venv only needs httpx + mcp[cli] (~50 MB). When detected, the
/// old venv is deleted and recreated slim.
pub fn is_fat_runtime_venv(root: &Path) -> bool {
    let venv = oracle_venv_dir(root);
    if !venv.exists() {
        return false;
    }
    // Check both Unix and Windows venv layouts.
    let unix_site_packages = venv.join("lib");
    let windows_site_packages = venv.join("Lib");
    // Walk the site-packages tree looking for a `torch` directory.
    fn has_torch_in_lib(lib_dir: &Path) -> bool {
        if !lib_dir.exists() {
            return false;
        }
        for entry in std::fs::read_dir(lib_dir).ok().into_iter().flatten() {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            if path.is_dir() {
                let path_str = path.to_string_lossy();
                // Match `<version>/site-packages/torch` on Unix.
                if path_str.contains("site-packages") && path.join("torch").is_dir() {
                    return true;
                }
                // Recurse into sub-directories that might be site-packages.
                if has_torch_in_lib(&path) {
                    return true;
                }
            }
        }
        false
    }
    has_torch_in_lib(&unix_site_packages) || windows_site_packages.join("torch").is_dir()
}

/// Delete the venv directory under `root`. Used during the M3 migration when a
/// legacy fat venv is detected and must be replaced with the slim MCP venv.
fn delete_venv(root: &Path) -> Result<(), String> {
    let venv = oracle_venv_dir(root);
    if !venv.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(&venv)
        .map_err(|e| format!("Could not delete legacy venv at {}: {e}", venv.display()))
}

/// Create the venv (if needed), install requirements, and mark it complete.
/// Long-running (minutes) and network-bound; callers should run it off the UI
/// thread. Idempotent: an already-installed runtime only re-checks/repairs.
///
/// `data_root` is the WRITABLE runtime home — the venv is created under
/// `data_root/oracle-data/venv`. `package_root` is the (possibly read-only,
/// bundled) `oracle/` source — `requirements-mcp.txt` is read from there.
///
/// MIGRATION (fat→slim): if the existing venv contains a `torch` install (the
/// old fat runtime from M2.x), it is deleted and recreated slim to reclaim the
/// ~2-3 GB that torch consumed.
pub fn run_oracle_runtime_bootstrap(
    data_root: &Path,
    package_root: &Path,
) -> Result<OracleRuntimeSetup, String> {
    // Refuse concurrent installs: two of them pip-clobbering the same venv (or
    // one racing a live Oracle call) corrupts site-packages on Windows.
    let _guard = install_lock()
        .try_lock()
        .map_err(|_| "Oracle runtime setup is already running.".to_string())?;

    let mut messages: Vec<String> = Vec::new();
    let venv = oracle_venv_dir(data_root);
    let venv_py = venv_python(&venv);
    let marker = venv.join(VENV_COMPLETE_MARKER);

    // Stop any running Oracle server so pip is not overwriting an interpreter
    // that another process has mapped, and clear the completion marker so a
    // failure below leaves the venv correctly flagged as incomplete.
    let _ = std::fs::remove_file(&marker);

    // MIGRATION: detect a legacy fat venv (torch present) and replace it.
    if is_fat_runtime_venv(data_root) {
        eprintln!(
            "[oracle-setup] M3 migration: detected legacy fat venv (torch present) at {}; \
             deleting and recreating slim MCP venv",
            venv.display()
        );
        delete_venv(data_root)?;
        messages.push(
            "M3 migration: replaced legacy fat venv (torch + embedder) with slim MCP venv.".to_string(),
        );
    }

    if !venv_py.exists() {
        let (argv, version) = match detect_system_python() {
            PythonDetection::Found { argv, version } => (argv, version),
            PythonDetection::NotFound => {
                return Err(
                    "No Python 3.10+ interpreter found. Install Python 3.10+ and retry.".to_string(),
                )
            }
            // Detection couldn't reach a verdict (machine busy / probe timed
            // out). Don't claim Python is absent — ask the user to retry once
            // the machine is less loaded.
            PythonDetection::Inconclusive => {
                return Err(
                    "Could not verify the local Python interpreter right now (the machine looks busy). Retry in a moment.".to_string(),
                )
            }
        };
        messages.push(format!(
            "Using system Python {version} ({}).",
            argv.join(" ")
        ));
        create_venv(&argv, &venv)?;
        messages.push("Created the Oracle virtual environment.".to_string());
    } else {
        messages.push("Reusing the existing Oracle virtual environment.".to_string());
    }

    // requirements-mcp.txt lives next to the `oracle/` SOURCE (package root), which in
    // release is the read-only bundle — never the writable data root.
    let requirements = package_root.join("oracle").join("requirements-mcp.txt");
    if !requirements.exists() {
        return Err("oracle/requirements-mcp.txt is missing next to the app.".to_string());
    }
    run_pip(&venv_py, &["install", "--upgrade", "pip"], data_root)?;
    run_pip(
        &venv_py,
        &["install", "-r", &requirements.to_string_lossy()],
        data_root,
    )?;
    messages.push("Installed slim MCP deps (httpx + mcp[cli]).".to_string());

    // Mark the venv usable only now that every step succeeded.
    std::fs::write(&marker, b"ok")
        .map_err(|e| format!("Could not finalize the Oracle runtime: {e}"))?;

    let mut status = rust_runtime_setup_status(data_root);
    let mut combined = messages;
    combined.append(&mut status.messages);
    status.messages = combined;
    Ok(status)
}

// --- Rust engine (ONNX) runtime helpers -----------------------------------

/// The oracle-data directory the Rust engine downloads/reads the ONNX model
/// under, derived identically to the in-process server
/// (`OracleDataPaths::from_root(root).root`). `root` is the workspace/data root
/// returned by `oracle_data_root()`.
/// The data directory the bundled ONNX embedder model lives under (under
/// `<root>/oracle-data/models/...`). Used by the Rust-native doctor to probe
/// model presence without loading it.
pub(crate) fn rust_model_data_dir(root: &Path) -> PathBuf {
    oracle_core::config::OracleDataPaths::from_root(root).root
}

// --- public entry points ---------------------------------------------------

/// Resolve the data root (venv home) + package root (source) and run the full
/// bootstrap. The venv is installed under the data root; the package source and
/// PYTHONPATH come from the package root.
pub fn install_oracle_runtime() -> Result<OracleRuntimeSetup, String> {
    let data_root = oracle_data_root().ok_or_else(|| {
        "Could not locate a writable location for the Oracle runtime.".to_string()
    })?;
    // The package root is needed for the requirements file + migration check.
    let package_root = find_oracle_package_root(None).ok_or_else(|| {
        "Could not locate the bundled oracle/ package next to the app.".to_string()
    })?;
    // Run the bootstrap (creates/reuses venv, installs requirements-mcp.txt,
    // handles fat→slim migration). The ONNX model install is separate and
    // triggered by the supervisor when the engine is Rust.
    let status = run_oracle_runtime_bootstrap(&data_root, &package_root)?;
    // If the model is not yet present, trigger the ONNX download now so the
    // overall status reflects readiness.
    let data_dir = rust_model_data_dir(&data_root);
    if !oracle_core::model_download::model_present(&data_dir, true) {
        eprintln!("[oracle-setup] ONNX model not present; downloading...");
        oracle_core::model_download::ensure_qwen3_onnx(&data_dir, true, |p| {
            let pct = match p.bytes_total {
                Some(t) if t > 0 => format!("{}%", (p.bytes_done * 100) / t),
                _ => format!("{} bytes", p.bytes_done),
            };
            eprintln!(
                "[rust-oracle model] {} ({}/{}) {}",
                p.file, p.index, p.total_files, pct
            );
        })
        .map_err(|e| format!("ONNX model download failed: {e:#}"))?;
    }
    Ok(rust_runtime_setup_status(&data_root))
}

fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    // Keep the last ~400 chars, sliced on a real char boundary (pip/torch output
    // is full of multi-byte glyphs, so a raw byte offset could panic).
    let start = trimmed
        .char_indices()
        .rev()
        .take(400)
        .last()
        .map_or(0, |(idx, _)| idx);
    trimmed[start..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modern_python_versions() {
        assert_eq!(
            parse_python_version("Python 3.11.4"),
            Some("3.11.4".to_string())
        );
        assert_eq!(
            parse_python_version("Python 3.10.0"),
            Some("3.10.0".to_string())
        );
        // 3.9 is the macOS Xcode CLT python: too old for the runtime deps
        // (torch/sentence-transformers/lancedb need >= 3.10) — must be rejected.
        assert_eq!(parse_python_version("Python 3.9.0"), None);
    }

    #[test]
    fn rejects_old_or_garbage_python_versions() {
        assert_eq!(parse_python_version("Python 3.8.10"), None);
        assert_eq!(parse_python_version("Python 2.7.18"), None);
        assert_eq!(parse_python_version("not a version"), None);
    }

    /// Homebrew can ship ONLY `python3.12` with NO unversioned `python3`
    /// symlink — the absolute-path probe must find versioned names too, or
    /// detection collapses onto the CLT 3.9. Pure over a fake filesystem.
    #[test]
    fn absolute_candidates_include_versioned_interpreters() {
        let only_versioned = |p: &Path| p == Path::new("/opt/homebrew/bin/python3.12");
        assert_eq!(
            unix_absolute_python_candidates(&only_versioned),
            vec![vec!["/opt/homebrew/bin/python3.12".to_string()]]
        );
        let nothing = |_: &Path| false;
        assert!(unix_absolute_python_candidates(&nothing).is_empty());
    }

    /// The system-python fallback must only accept a detection whose argv is a
    /// SINGLE token (a bare program for `Command::new`); the Windows `py -3`
    /// two-token launcher is install-only. Pure over the detection enum.
    #[test]
    fn system_python_program_filters_detection() {
        assert_eq!(
            system_python_program(PythonDetection::Found {
                argv: vec!["/opt/homebrew/bin/python3".to_string()],
                version: "3.12.0".to_string(),
            }),
            Some("/opt/homebrew/bin/python3".to_string())
        );
        assert_eq!(
            system_python_program(PythonDetection::Found {
                argv: vec!["py".to_string(), "-3".to_string()],
                version: "3.12.0".to_string(),
            }),
            None
        );
        assert_eq!(system_python_program(PythonDetection::NotFound), None);
        assert_eq!(system_python_program(PythonDetection::Inconclusive), None);
    }

    #[test]
    fn timeout_maps_to_inconclusive_not_notpython() {
        // A simulated probe TIMEOUT / spawn failure (the `Err` arm of
        // run_with_timeout) must classify as Inconclusive — the seam that keeps a
        // busy machine from reporting a false "no Python".
        assert_eq!(
            classify_probe_outcome(Err("process exceeded 20s timeout".to_string())),
            ProbeOutcome::Inconclusive
        );
        assert_eq!(
            classify_probe_outcome(Err("process could not start: busy".to_string())),
            ProbeOutcome::Inconclusive
        );
    }

    #[test]
    fn clean_run_maps_to_found_or_notpython() {
        // A probe that RAN: success + parsed version → Found; non-success or no
        // parseable ≥3.9 version → the definitive NotPython.
        assert_eq!(
            classify_probe_outcome(Ok((true, Some("3.11.4".to_string())))),
            ProbeOutcome::Found("3.11.4".to_string())
        );
        assert_eq!(
            classify_probe_outcome(Ok((false, None))),
            ProbeOutcome::NotPython
        );
        assert_eq!(
            classify_probe_outcome(Ok((true, None))),
            ProbeOutcome::NotPython
        );
    }

    #[test]
    fn detection_inconclusive_when_no_found_but_some_busy() {
        // No candidate Found, but one timed out → aggregate Inconclusive (soft
        // "still checking"), NOT NotFound (hard "install Python").
        let detection = classify_python_detection(vec![
            (vec!["py".into(), "-3".into()], ProbeOutcome::Inconclusive),
            (vec!["python".into()], ProbeOutcome::NotPython),
        ]);
        assert_eq!(detection, PythonDetection::Inconclusive);
    }

    #[test]
    fn detection_notfound_only_when_all_ran_and_none_python() {
        // Every candidate ran and none is Python ≥3.10 → definitive NotFound.
        let detection = classify_python_detection(vec![
            (vec!["python".into()], ProbeOutcome::NotPython),
            (vec!["python3".into()], ProbeOutcome::NotPython),
        ]);
        assert_eq!(detection, PythonDetection::NotFound);
    }

    #[test]
    fn detection_found_wins_over_later_inconclusive() {
        // A Found candidate wins regardless of any inconclusive sibling.
        let detection = classify_python_detection(vec![
            (
                vec!["python".into()],
                ProbeOutcome::Found("3.12.1".to_string()),
            ),
            (vec!["python3".into()], ProbeOutcome::Inconclusive),
        ]);
        assert_eq!(
            detection,
            PythonDetection::Found {
                argv: vec!["python".into()],
                version: "3.12.1".to_string()
            }
        );
    }

    #[test]
    fn inconclusive_status_sets_checking_and_soft_message() {
        // End-to-end mapping: when detection is Inconclusive the status must set
        // `checking=true`, NOT report a missing Python, and carry the SOFT
        // "still checking" message — never the hard "Install Python 3" one.
        // We drive this through the message seam by constructing the same
        // branches oracle_runtime_setup_status uses.
        let detection = PythonDetection::Inconclusive;
        let (python_found, checking) = match detection {
            PythonDetection::Found { .. } => (true, false),
            PythonDetection::NotFound => (false, false),
            PythonDetection::Inconclusive => (false, true),
        };
        assert!(!python_found);
        assert!(checking);

        // The hard message must NOT fire when `checking` is set.
        let mut message = String::new();
        if checking {
            message = "Still checking the local runtime (first startup can be slow on a busy machine). Retry in a moment.".to_string();
        } else if !python_found {
            message = "No Python 3.10+ interpreter found. Install Python 3.10+ to enable local Oracle retrieval.".to_string();
        }
        assert!(message.contains("Still checking"));
        assert!(!message.contains("Install Python 3"));
    }

    #[test]
    fn venv_python_path_is_os_specific() {
        let venv = PathBuf::from("/tmp/venv");
        let py = venv_python(&venv);
        if cfg!(windows) {
            assert!(py.ends_with("Scripts/python.exe") || py.ends_with("Scripts\\python.exe"));
        } else {
            assert!(py.ends_with("bin/python3"));
        }
    }

    #[test]
    fn venv_dir_lives_under_oracle_data() {
        let root = PathBuf::from("/app");
        assert_eq!(
            oracle_venv_dir(&root),
            PathBuf::from("/app/oracle-data/venv")
        );
    }

    #[test]
    fn resolve_python_returns_nonempty_command() {
        // Whether or not a data-root venv resolves, PYTHON is set, or we fall back
        // to the OS default, the result must always be a non-empty command string.
        assert!(!resolve_oracle_python().is_empty());
    }

    /// End-to-end exercise of the REAL install the "Install runtime" button runs:
    /// resolve the package root, create a fresh venv, `pip install` the slim
    /// `requirements-mcp.txt` (httpx + mcp[cli]), and mark complete.
    /// `#[ignore]` because it is network-bound and slow; run it
    /// explicitly to validate the venv path that the cheap status probe never
    /// touches:
    ///   cargo test --lib -- --ignored --nocapture install_runtime_end_to_end
    #[test]
    #[ignore]
    fn install_runtime_end_to_end() {
        let package_resolved = find_oracle_package_root(None);
        eprintln!("[install-e2e] resolved package root: {package_resolved:?}");
        package_resolved.expect("could not locate the oracle/ package root");
        let data_root = oracle_data_root().expect("could not locate a writable data root");
        eprintln!(
            "[install-e2e] venv target: {}",
            oracle_venv_dir(&data_root).display()
        );

        let status = install_oracle_runtime().expect("install_oracle_runtime failed");

        eprintln!("[install-e2e] messages:");
        for line in &status.messages {
            eprintln!("  - {line}");
        }
        eprintln!(
            "[install-e2e] python_found={} venv_ready={} deps_ready={} embedder_ready={} ready={}",
            status.python_found,
            status.venv_ready,
            status.deps_ready,
            status.embedder_ready,
            status.ready,
        );

        // The venv path is the whole point of this test: after a real install the
        // completion marker must exist under the DATA root and the resolved
        // interpreter must point at that freshly built venv.
        assert!(
            venv_complete(&data_root),
            "venv completion marker not written"
        );
        assert!(
            resolve_oracle_python().contains("venv"),
            "resolve_oracle_python should pick the freshly built venv"
        );
        assert!(status.venv_ready, "status should report the venv as ready");
        assert!(
            status.deps_ready,
            "httpx + mcp must import"
        );
        assert!(
            status.embedder_ready,
            "the ONNX embedder must be cached"
        );
        assert!(
            status.ready,
            "the runtime must be fully ready after install"
        );
    }

    #[test]
    fn rust_runtime_status_reports_absent_model() {
        let dir = std::env::temp_dir().join(format!(
            "aspis-m24-absent-{}",
            std::process::id()
        ));
        let s = rust_runtime_setup_status(&dir);
        assert!(!s.ready, "absent model dir must not be ready");
        assert!(!s.embedder_ready);
        assert!(
            s.python_found,
            "python gates reported satisfied under rust engine"
        );
        // M3: venv_ready/deps_ready now report the SLIM MCP venv truthfully —
        // an absent data dir has no venv, so both must be false.
        assert!(!s.venv_ready && !s.deps_ready);
        assert!(s.embed_model.contains("ONNX"));
    }

    /// Test the fat→slim migration detection helper with a temp dir.
    /// Creates both Unix and Windows venv layouts and asserts the detection
    /// correctly identifies the presence/absence of `torch`.
    #[test]
    fn fat_venv_detection_unix_layout() {
        let dir = std::env::temp_dir().join(format!(
            "aspis-fat-venv-unix-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let venv = oracle_venv_dir(&dir);
        // Unix layout: lib/python3.x/site-packages/torch
        let site_packages = venv.join("lib").join("python3.12").join("site-packages");
        std::fs::create_dir_all(site_packages.join("torch")).unwrap();
        // Write the completion marker so venv_complete would return true.
        std::fs::create_dir_all(venv.join("bin")).unwrap();
        std::fs::write(venv.join("bin").join("python3"), "").unwrap();
        std::fs::write(venv.join(VENV_COMPLETE_MARKER), b"ok").unwrap();

        assert!(
            is_fat_runtime_venv(&dir),
            "Unix layout with torch must be detected as fat venv"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fat_venv_detection_windows_layout() {
        let dir = std::env::temp_dir().join(format!(
            "aspis-fat-venv-win-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let venv = oracle_venv_dir(&dir);
        // Windows layout: Lib/site-packages/torch
        let site_packages = venv.join("Lib").join("site-packages");
        std::fs::create_dir_all(site_packages.join("torch")).unwrap();
        std::fs::create_dir_all(venv.join("Scripts")).unwrap();
        std::fs::write(venv.join("Scripts").join("python.exe"), "").unwrap();
        std::fs::write(venv.join(VENV_COMPLETE_MARKER), b"ok").unwrap();

        assert!(
            is_fat_runtime_venv(&dir),
            "Windows layout with torch must be detected as fat venv"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fat_venv_detection_slim_venv() {
        let dir = std::env::temp_dir().join(format!(
            "aspis-slim-venv-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let venv = oracle_venv_dir(&dir);
        // Slim venv: no torch, just httpx + mcp
        let site_packages = venv.join("lib").join("python3.12").join("site-packages");
        std::fs::create_dir_all(site_packages.join("httpx")).unwrap();
        std::fs::create_dir_all(venv.join("bin")).unwrap();
        std::fs::write(venv.join("bin").join("python3"), "").unwrap();
        std::fs::write(venv.join(VENV_COMPLETE_MARKER), b"ok").unwrap();

        assert!(
            !is_fat_runtime_venv(&dir),
            "Slim venv without torch must NOT be detected as fat venv"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fat_venv_detection_no_venv() {
        let dir = std::env::temp_dir().join(format!(
            "aspis-no-venv-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        assert!(
            !is_fat_runtime_venv(&dir),
            "Non-existent venv must NOT be detected as fat venv"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
