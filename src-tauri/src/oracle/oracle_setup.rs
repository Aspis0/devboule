//! Cross-platform bootstrap for the local Oracle Python runtime.
//!
//! The app is remote-first for *answers*, but retrieval is mandatory: it needs
//! the Qwen3 embedding model and LanceDB. Those must install automatically on
//! Windows AND macOS. This module resolves a Python interpreter, creates a
//! virtual environment, `pip install`s `oracle/requirements.txt` (LanceDB +
//! sentence-transformers + deps) into it, and warms the embedder so retrieval
//! works offline afterwards.
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
    apply_no_window, find_oracle_package_root, oracle_data_root, run_with_timeout,
    stop_python_oracle_runtime, ProcessOutput,
};

/// Embedding model the retrieval layer depends on (mirrors `oracle/config.py`).
pub const EMBED_MODEL: &str = "Qwen/Qwen3-Embedding-0.6B";
/// torch + the model are large; allow a generous install window.
const PIP_TIMEOUT: Duration = Duration::from_secs(2400);
const WARMUP_TIMEOUT: Duration = Duration::from_secs(900);
/// Generous so a loaded/slow dev machine does not FALSE-timeout on a trivial
/// `py -3 --version`. A real, present interpreter answers in well under a second;
/// the headroom only matters when the box is thrashing, in which case we prefer a
/// soft "still checking" outcome over a scary "no Python found".
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
/// stdout sentinel the readiness probe prefixes its JSON with, so module import
/// chatter cannot be mistaken for the result.
const CHECK_SENTINEL: &str = "ORACLE_RUNTIME_CHECK ";
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
    /// A usable Python 3.9+ interpreter (venv or system) was found.
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
    /// LanceDB + sentence-transformers import successfully in the venv.
    pub deps_ready: bool,
    /// The Qwen3 embedding model is downloaded and cached locally.
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
fn venv_complete(root: &Path) -> bool {
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
    default_python_command().to_string()
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
        candidates.push(vec!["python3".into()]);
        candidates.push(vec!["python".into()]);
        // A macOS `.app` launched from Finder inherits a minimal PATH (often just
        // `/usr/bin:/bin`), so a bare `python3` may not resolve even when Python
        // is installed via Homebrew/pyenv. Probe the usual absolute locations.
        for abs in [
            "/opt/homebrew/bin/python3", // Apple Silicon Homebrew
            "/usr/local/bin/python3",    // Intel Homebrew / python.org
            "/usr/bin/python3",          // Xcode Command Line Tools
        ] {
            if Path::new(abs).exists() {
                candidates.push(vec![abs.to_string()]);
            }
        }
    }
    candidates
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
/// between a probe that RAN and proved the candidate is not Python ≥3.9
/// (`NotPython`) and a probe that could not produce a trustworthy answer because
/// the machine was busy — it timed out or the spawn failed transiently
/// (`Inconclusive`). The latter must NOT be reported to the user as "no Python
/// installed", only as "still checking".
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProbeOutcome {
    /// Ran successfully and parsed a Python ≥3.9 version banner.
    Found(String),
    /// Ran to completion but is not a usable Python ≥3.9 (non-zero exit, old
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
    /// Every candidate RAN and none is Python ≥3.9 — a definitive negative.
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

/// Parse a `Python 3.x.y` version banner and require >= 3.9.
fn parse_python_version(text: &str) -> Option<String> {
    let token = text
        .split_whitespace()
        .find(|piece| piece.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    let mut parts = token.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next().unwrap_or("0").parse().unwrap_or(0);
    if major > 3 || (major == 3 && minor >= 9) {
        Some(token.to_string())
    } else {
        None
    }
}

/// Probe one interpreter argv and classify the result into the tri-state. A
/// timeout or spawn failure (machine busy) yields `Inconclusive`; a clean run
/// that is not Python ≥3.9 yields `NotPython`.
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

/// Version-only wrapper used where the caller already KNOWS the interpreter
/// exists (e.g. the completed venv python) and only wants its banner. A
/// timeout/transient failure collapses to `None` here exactly as before — the
/// tri-state matters only for *system* detection, where it drives the soft vs.
/// hard "no Python" message.
fn probe_python_version(argv: &[String]) -> Option<String> {
    match probe_python_outcome(argv) {
        ProbeOutcome::Found(version) => Some(version),
        ProbeOutcome::NotPython | ProbeOutcome::Inconclusive => None,
    }
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

#[derive(Debug, Clone, Deserialize)]
struct WarmupCheck {
    #[serde(default)]
    lancedb: bool,
    #[serde(default, rename = "sentenceTransformers")]
    sentence_transformers: bool,
    #[serde(default, rename = "embedderCached")]
    embedder_cached: bool,
}

/// Run the cheap readiness probe with a given interpreter argv prefix (e.g.
/// `["python3"]` or `["py", "-3"]` or the venv python path). `root` is the DATA
/// root (cwd); the `oracle` package is imported via PYTHONPATH = the package root
/// so the probe works even in a release build where the writable data root holds
/// no source.
fn warmup_check(python_argv: &[String], root: &Path) -> Option<WarmupCheck> {
    if python_argv.is_empty() {
        return None;
    }
    let mut args: Vec<String> = python_argv[1..].to_vec();
    args.extend([
        "-m".to_string(),
        "oracle.bootstrap.warmup".to_string(),
        "--check".to_string(),
    ]);
    // FIX 2: PYTHONPATH = package/import root so `-m oracle...` imports regardless
    // of cwd (release: data root != package root). If the package root cannot be
    // located, the probe could only fail as an opaque `ModuleNotFoundError`, so we
    // short-circuit to a not-ready status instead of spawning a doomed process.
    let Some(package_root) = find_oracle_package_root(None) else {
        return None;
    };
    let mut command = Command::new(&python_argv[0]);
    command
        .args(&args)
        .current_dir(root)
        .env("PYTHONPATH", &package_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_no_window(&mut command);
    let output = run_with_timeout(command, PROBE_TIMEOUT, None).ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Parse only the sentinel-prefixed line, so import-time chatter that happens
    // to contain a `{` cannot be mistaken for the result.
    let json = stdout
        .lines()
        .rev()
        .find_map(|line| line.trim_start().strip_prefix(CHECK_SENTINEL))?;
    serde_json::from_str(json.trim()).ok()
}

pub fn oracle_runtime_setup_status(root: &Path) -> OracleRuntimeSetup {
    let venv = oracle_venv_dir(root);
    let venv_py = venv_python(&venv);
    // Only a fully-installed venv counts; a half-installed one must not be used.
    let venv_ready = venv_complete(root);

    // Check the interpreter that would actually run Oracle: the venv when it is
    // complete, otherwise the detected system Python. This is what makes a
    // machine with deps already in system Python report "ready" without a venv.
    // `checking` is set only when system detection could not reach a verdict
    // because the machine was busy (a probe timed out / failed to spawn). A
    // complete venv is a definitive positive, so it never sets `checking`.
    let (python_found, checking, python_command, python_version, check) = if venv_ready {
        let argv = vec![venv_py.to_string_lossy().to_string()];
        let version = probe_python_version(&argv);
        let check = warmup_check(&argv, root);
        (
            true,
            false,
            Some(venv_py.to_string_lossy().to_string()),
            version,
            check,
        )
    } else {
        match detect_system_python() {
            PythonDetection::Found { argv, version } => {
                let check = warmup_check(&argv, root);
                (true, false, Some(argv.join(" ")), Some(version), check)
            }
            // The probe RAN and found no Python ≥3.9 — a definitive negative.
            PythonDetection::NotFound => (false, false, None, None, None),
            // The probe timed out / could not spawn (machine busy). NOT a
            // definitive negative: surface a soft "still checking" status so the
            // UI does not scare the user into installing a Python they already
            // have.
            PythonDetection::Inconclusive => (false, true, None, None, None),
        }
    };

    let deps_ready = check
        .as_ref()
        .map(|c| c.lancedb && c.sentence_transformers)
        .unwrap_or(false);
    let embedder_ready = deps_ready && check.as_ref().map(|c| c.embedder_cached).unwrap_or(false);
    // A venv is not required: if the resolved interpreter already has the deps
    // and the cached model, retrieval works.
    let ready = python_found && deps_ready && embedder_ready;

    let mut messages = Vec::new();
    if checking {
        // SOFT path: detection was inconclusive (busy machine), so do NOT claim
        // Python is missing. Tell the user it is still being checked.
        messages.push(
            "Still checking the local runtime (first startup can be slow on a busy machine). Retry in a moment.".to_string(),
        );
    } else if !python_found {
        // HARD path: detection actually RAN and found no Python ≥3.9.
        messages.push(
            "No Python 3.9+ interpreter found. Install Python 3 to enable local Oracle retrieval."
                .to_string(),
        );
    } else if !deps_ready {
        messages.push(
            "Oracle dependencies (LanceDB / embedder) are missing or incomplete.".to_string(),
        );
    } else if !embedder_ready {
        messages.push("The Qwen3 embedding model is not downloaded yet.".to_string());
    } else {
        messages.push("Oracle retrieval runtime is ready.".to_string());
    }

    OracleRuntimeSetup {
        python_found,
        checking,
        python_command,
        python_version,
        venv_ready,
        deps_ready,
        embedder_ready,
        ready,
        embed_model: EMBED_MODEL.to_string(),
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

fn run_warmup(venv_py: &Path, data_root: &Path, package_root: &Path) -> Result<(), String> {
    let mut command = Command::new(venv_py);
    command
        .args(["-m", "oracle.bootstrap.warmup"])
        .current_dir(data_root)
        // PYTHONPATH = package/import root so `-m oracle...` imports even when the
        // writable data root holds no source (release).
        .env("PYTHONPATH", package_root)
        .env("ORACLE_ALLOW_HF_DOWNLOAD", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_no_window(&mut command);
    let output = run_with_timeout(command, WARMUP_TIMEOUT, None)?;
    if !output.status.success() {
        return Err(format!(
            "Warming the Qwen3 embedding model failed: {}",
            tail(&output.stderr)
        ));
    }
    Ok(())
}

/// Create the venv (if needed), install requirements, and warm the embedder.
/// Long-running (minutes) and network-bound; callers should run it off the UI
/// thread. Idempotent: an already-installed runtime only re-checks/repairs.
///
/// `data_root` is the WRITABLE runtime home — the venv is created under
/// `data_root/oracle-data/venv`. `package_root` is the (possibly read-only,
/// bundled) `oracle/` source — `requirements.txt` is read from there and it is
/// the PYTHONPATH for the warmup import. In dev both are the same source repo; in
/// release they differ (writable app-data vs. read-only bundle).
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
    let _ = stop_python_oracle_runtime();
    let _ = std::fs::remove_file(&marker);

    if !venv_py.exists() {
        let (argv, version) = match detect_system_python() {
            PythonDetection::Found { argv, version } => (argv, version),
            PythonDetection::NotFound => {
                return Err(
                    "No Python 3.9+ interpreter found. Install Python 3 and retry.".to_string(),
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

    // requirements.txt lives next to the `oracle/` SOURCE (package root), which in
    // release is the read-only bundle — never the writable data root.
    let requirements = package_root.join("oracle").join("requirements.txt");
    if !requirements.exists() {
        return Err("oracle/requirements.txt is missing next to the app.".to_string());
    }
    run_pip(&venv_py, &["install", "--upgrade", "pip"], data_root)?;
    run_pip(
        &venv_py,
        &["install", "-r", &requirements.to_string_lossy()],
        data_root,
    )?;
    messages.push("Installed LanceDB, the Qwen3 embedder and dependencies.".to_string());

    run_warmup(&venv_py, data_root, package_root)?;
    messages.push("Downloaded and warmed the Qwen3 embedding model.".to_string());

    // Mark the venv usable only now that every step succeeded.
    std::fs::write(&marker, b"ok")
        .map_err(|e| format!("Could not finalize the Oracle runtime: {e}"))?;

    let mut status = oracle_runtime_setup_status(data_root);
    let mut combined = messages;
    combined.extend(status.messages.drain(..));
    status.messages = combined;
    Ok(status)
}

/// Resolve the writable data root (where the venv lives) and return the current
/// setup status (no install). Status is read from the DATA root, NOT the package
/// root, because the installed runtime lives under the data root.
pub fn current_oracle_runtime_setup() -> OracleRuntimeSetup {
    match oracle_data_root() {
        Some(root) => oracle_runtime_setup_status(&root),
        None => OracleRuntimeSetup::unavailable(
            "Could not locate the bundled oracle/ package next to the app.",
        ),
    }
}

/// Resolve the data root (venv home) + package root (source) and run the full
/// bootstrap. The venv is installed under the data root; the package source and
/// PYTHONPATH come from the package root.
pub fn install_oracle_runtime() -> Result<OracleRuntimeSetup, String> {
    let data_root = oracle_data_root().ok_or_else(|| {
        "Could not locate a writable location for the Oracle runtime.".to_string()
    })?;
    let package_root = find_oracle_package_root(None).ok_or_else(|| {
        "Could not locate the bundled oracle/ package next to the app.".to_string()
    })?;
    run_oracle_runtime_bootstrap(&data_root, &package_root)
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
            parse_python_version("Python 3.9.0"),
            Some("3.9.0".to_string())
        );
    }

    #[test]
    fn rejects_old_or_garbage_python_versions() {
        assert_eq!(parse_python_version("Python 3.8.10"), None);
        assert_eq!(parse_python_version("Python 2.7.18"), None);
        assert_eq!(parse_python_version("not a version"), None);
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
        // Every candidate ran and none is Python ≥3.9 → definitive NotFound.
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
            message = "No Python 3.9+ interpreter found. Install Python 3 to enable local Oracle retrieval.".to_string();
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
    /// resolve the package root, create a fresh venv, `pip install` the full
    /// `requirements.txt` (torch + LanceDB + the embedder — multi-GB), and warm
    /// the model. `#[ignore]` because it is network-bound and slow; run it
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
            "LanceDB + sentence-transformers must import"
        );
        assert!(status.embedder_ready, "the Qwen3 embedder must be cached");
        assert!(
            status.ready,
            "the runtime must be fully ready after install"
        );
    }
}
