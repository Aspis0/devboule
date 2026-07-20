//! UNIVERSAL provider auto-detection + executable resolution (cross-platform).
//!
//! A GUI-launched app does NOT inherit the interactive shell's `PATH`, so a bare
//! `Command::new("codex")` / `Command::new("claude")` ENOENTs even when the tool is
//! installed (the classic "works in my terminal, not from the app icon" failure). This
//! module is the SINGLE source of truth for two things, used by BOTH the detector and the
//! spawner so detection and execution can never disagree:
//!
//!   1. [`augmented_path`] — the process `PATH` PLUS the per-OS directories where these CLIs
//!      are commonly installed (npm global, Homebrew, cargo, scoop, chocolatey, …). Inject
//!      this into a child's `PATH` env so a GUI launch finds the tool AND any tool it
//!      transitively spawns.
//!   2. [`resolve_program`] — a `which`-style scan over [`augmented_path`], trying the
//!      platform executable extensions on Windows (`claude` is often `claude.cmd`). Returns
//!      the FULL resolved path; the spawner runs THAT (never the bare name).
//!
//! [`detect_providers`] (the `#[tauri::command]`) probes what is actually present:
//! claude/codex by resolution, ollama/oMLX by resolution + a bounded loopback HTTP probe
//! (filling the live model list), and `api` as always-available (the user supplies a
//! command). Every probe is bounded (short timeout, loopback only) and failure-isolated:
//! one provider's probe error NEVER fails the whole detection.
//!
//! REUSE: this supersedes the narrow `projects::command_exists` (a boolean PATH check that
//! does NOT augment the GUI PATH nor return a path). `command_exists` is kept for its
//! existing callers; new code wanting a resolved path uses `resolve_program` here.
//!
//! PRIVACY: the HTTP probes hit ONLY loopback (`127.0.0.1`) endpoints with a redirect-free
//! client, so no request can be redirected off-box. No prompt or user data is sent — these
//! are metadata GETs (`/api/tags`, `/v1/models`).

use std::ffi::OsString;
#[cfg(target_os = "macos")]
use std::io::Read;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Serialize;

/// Default loopback base the ollama daemon listens on. The tags endpoint is the native
/// (non-OpenAI) ollama API and is the cheapest "is it running + what's pulled" probe.
const OLLAMA_TAGS_URL: &str = "http://127.0.0.1:11434/api/tags";

/// Default loopback base an oMLX (MLX) OpenAI-compatible server listens on. We probe the
/// standard OpenAI `/v1/models` listing. This is the documented default; a user who runs
/// oMLX on another loopback port still configures the design backend's `baseUrl` explicitly
/// (detection here is best-effort discovery, not the source of truth for generation).
const OMLX_MODELS_URL: &str = "http://127.0.0.1:8000/v1/models";

/// Per-probe HTTP timeout. Detection must feel instant in the UI and must never hang on a
/// port that accepts the TCP connection but never answers; 1.5s is generous for a loopback
/// metadata GET while keeping the whole `detect_providers` call snappy.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);
#[cfg(target_os = "macos")]
const CLI_HELP_PROBE_MAX_BYTES: usize = 64 * 1024;

/// Resolve the user's home directory cross-platform. Mirrors
/// [`super::cli_agents`]'s `user_home` (`USERPROFILE` on Windows, `HOME` on Unix/macOS); a
/// private copy so this module has no cross-module visibility dependency. `None` ⇒ neither
/// is set (we then simply omit the home-relative dirs from the augmented PATH).
fn user_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// Compare two paths for PATH-dedup. WARNING 2: Windows is case-insensitive, so an
/// ASCII-lowercased comparison treats differently-cased duplicates as equal; Unix is
/// case-sensitive, so we compare exactly. (We only lowercase ASCII — the drive/dir names
/// these CLIs install under are ASCII, and full Unicode case folding is not worth the cost.)
fn path_eq(a: &std::path::Path, b: &std::path::Path) -> bool {
    // Normalize a trailing path separator before comparing: a host PATH entry may
    // carry a trailing `\` (e.g. `C:\Program Files\nodejs\`) while our augmentation
    // literal does not (`pf.join("nodejs")` -> `...\nodejs`). Without trimming, the
    // de-dup misses and the dir is appended a second time. Trim both separators so a
    // forward-slash variant normalizes too.
    fn trim_trailing_sep(s: &str) -> &str {
        s.trim_end_matches(['\\', '/'])
    }
    #[cfg(windows)]
    {
        trim_trailing_sep(&a.as_os_str().to_string_lossy()).to_ascii_lowercase()
            == trim_trailing_sep(&b.as_os_str().to_string_lossy()).to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        trim_trailing_sep(&a.as_os_str().to_string_lossy())
            == trim_trailing_sep(&b.as_os_str().to_string_lossy())
    }
}

/// The current process `PATH` PLUS the per-OS directories where the supported CLIs are
/// commonly installed. The existing `PATH` entries come FIRST so a user's explicit ordering
/// always wins; the augmentation dirs are appended (and de-duplicated against what's already
/// there) so we never shadow a deliberately-chosen binary. Returned as an `OsString` ready
/// to set as a child's `PATH` env (`std::env::join_paths` round-trips it).
///
/// cfg-gated per OS — see the inline lists. A home-relative dir is included only when the
/// home directory resolves; a dir is included even if it does not yet exist (cheap, and the
/// `resolve_program` scan simply skips non-existent dirs).
pub fn augmented_path() -> OsString {
    // Start from the existing PATH so the user's ordering is preserved + wins.
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();

    let home = user_home();
    let mut push = |p: PathBuf| {
        // De-dup: never append a dir already present. WARNING 2 — Windows paths are
        // case-INSENSITIVE, so `C:\Users\me\AppData\Roaming\npm` and the same path with
        // different casing are the SAME dir and must not be appended twice; compare
        // case-insensitively there. Unix paths are case-sensitive, so compare exactly.
        if !dirs.iter().any(|existing| path_eq(existing, &p)) {
            dirs.push(p);
        }
    };

    #[cfg(windows)]
    {
        // npm global installs put `claude.cmd`/`codex.cmd` under %APPDATA%\npm; scoop/choco
        // are common third-party package managers; cargo for rust-built tools; WindowsApps
        // for winget/store shims.
        if let Some(appdata) = std::env::var_os("APPDATA") {
            push(PathBuf::from(appdata).join("npm"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let local = PathBuf::from(local);
            push(local.join("Microsoft").join("WindowsApps"));
            // Some npm/volta-style installs land here too.
            push(local.join("Programs"));
        }
        if let Some(programdata) = std::env::var_os("ProgramData") {
            push(PathBuf::from(programdata).join("chocolatey").join("bin"));
        }
        if let Some(home) = home.as_ref() {
            push(home.join(".cargo").join("bin"));
            push(home.join("scoop").join("shims"));
            push(home.join(".bun").join("bin"));
        }
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            let pf = PathBuf::from(pf);
            // Common per-tool install bins (nodejs ships node/npm; git ships its bin).
            push(pf.join("nodejs"));
        }
    }

    #[cfg(not(windows))]
    {
        // The canonical Unix/macOS bins a login shell would have but a GUI launch lacks.
        // Homebrew differs by arch: /usr/local (Intel) vs /opt/homebrew (Apple Silicon).
        push(PathBuf::from("/usr/local/bin"));
        push(PathBuf::from("/opt/homebrew/bin"));
        push(PathBuf::from("/usr/bin"));
        push(PathBuf::from("/bin"));
        if let Some(home) = home.as_ref() {
            push(home.join(".local").join("bin"));
            push(home.join(".npm-global").join("bin"));
            push(home.join(".bun").join("bin"));
            push(home.join(".cargo").join("bin"));
        }
    }

    // join_paths only fails if a dir contains the platform path separator; our dirs are
    // OS-derived and never do, so the fallback (the original PATH) is purely defensive.
    std::env::join_paths(dirs).unwrap_or_else(|_| {
        std::env::var_os("PATH").unwrap_or_default()
    })
}

/// The executable file-name candidates to try for a bare program `name`, in priority order.
/// On Windows a global npm CLI is typically a `.cmd` shim (`claude.cmd`), and tools may be
/// `.exe`/`.bat`/`.ps1`; we also try the bare name last (in case it is already extensioned
/// or PATHEXT-resolved). On Unix the only candidate is the bare name (no extensions).
fn program_candidates(name: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        // If the caller already gave an extension, trust it verbatim only.
        let lower = name.to_ascii_lowercase();
        if [".exe", ".cmd", ".bat", ".ps1", ".com"]
            .iter()
            .any(|ext| lower.ends_with(ext))
        {
            return vec![name.to_string()];
        }
        vec![
            format!("{name}.cmd"),
            format!("{name}.exe"),
            format!("{name}.bat"),
            format!("{name}.ps1"),
            name.to_string(),
        ]
    }
    #[cfg(not(windows))]
    {
        vec![name.to_string()]
    }
}

/// `which`-style resolution: scan [`augmented_path`] for the first directory containing an
/// executable file matching `name` (trying the platform extensions on Windows). Returns the
/// FULL path to spawn. `None` ⇒ not found anywhere on the augmented PATH.
///
/// A `name` that already contains a path separator is treated as an explicit path and
/// checked directly (mirrors how a shell resolves `./tool` or `/usr/bin/tool` without
/// consulting PATH) — this also lets the caller short-circuit an already-resolved path.
pub fn resolve_program(name: &str) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    // Path-bearing name: resolve directly, still trying extensions on Windows.
    if name.contains('/') || name.contains('\\') {
        for candidate in program_candidates(name) {
            let p = PathBuf::from(&candidate);
            if is_executable_file(&p) {
                return Some(p);
            }
        }
        return None;
    }

    let path = augmented_path();
    let candidates = program_candidates(name);
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for candidate in &candidates {
            let full = dir.join(candidate);
            if is_executable_file(&full) {
                return Some(full);
            }
        }
    }
    None
}

/// A path is runnable if it is an existing regular file. On Unix we additionally require an
/// execute bit (a non-executable file on PATH is not a command). On Windows the file
/// extension carries executability (handled by [`program_candidates`]), so an existing
/// regular file is sufficient.
///
/// Shared with `mcp_backend` for `DEVBOULE_MCP_BIN` / candidate binary resolution.
#[cfg(unix)]
pub(crate) fn is_executable_file(candidate: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(candidate) {
        Ok(meta) => meta.is_file() && (meta.permissions().mode() & 0o111 != 0),
        Err(_) => false,
    }
}

#[cfg(not(unix))]
pub(crate) fn is_executable_file(candidate: &std::path::Path) -> bool {
    candidate.is_file()
}

// ---------------------------------------------------------------------------
// Detection result + pure model-list parsers.
// ---------------------------------------------------------------------------

/// One detected provider. camelCase over the IPC boundary so the TS side reads it directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedProvider {
    /// One of `"claude" | "codex" | "ollama" | "omlx" | "api"`.
    pub kind: String,
    /// Whether this provider can be used right now on this machine.
    pub available: bool,
    // W2: the resolved absolute CLI path is DELIBERATELY NOT exposed over IPC — it
    // leaks the user's filesystem layout to the renderer with no UI need. Resolution
    // still happens internally (via `resolve_program`) at spawn time; the renderer
    // only needs `available`. Do not re-add a `path` field here.
    /// A short, human, secret-free status hint (e.g. "running", "cli only",
    /// "configure a command"). Never a URL or filesystem path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Live model tags/ids discovered from a reachable HTTP provider; empty otherwise.
    pub models: Vec<String>,
}

impl DetectedProvider {
    fn unavailable(kind: &str) -> Self {
        Self {
            kind: kind.to_string(),
            available: false,
            detail: None,
            models: Vec::new(),
        }
    }
}

/// BLOCKER 2: bound the model list a probe parser will return. A hostile loopback server
/// (within the body-size cap) could still list thousands of entries or one with a
/// megabyte-long name to bloat the IPC payload / UI. Cap the COUNT and each NAME length;
/// over-long names are dropped (a truncated tag is useless and could mislead generation).
const MAX_MODELS: usize = 200;
const MAX_MODEL_NAME_LEN: usize = 200;

/// Keep only well-formed, sane-length model names, capped at [`MAX_MODELS`]. Shared by both
/// parsers so the bound is enforced identically.
fn bound_models<I: IntoIterator<Item = String>>(names: I) -> Vec<String> {
    names
        .into_iter()
        .filter(|n| !n.is_empty() && n.len() <= MAX_MODEL_NAME_LEN)
        .take(MAX_MODELS)
        .collect()
}

/// PURE: extract the model tags from an ollama `/api/tags` JSON body. Shape:
/// `{"models":[{"name":"qwen2.5:7b", ...}, ...]}`. Tolerant — a missing/!array `models`,
/// or an entry without a string `name`, yields an empty/partial list rather than an error.
/// Bounded count + per-name length (BLOCKER 2).
fn parse_ollama_tags(body: &str) -> Vec<String> {
    let value: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    value
        .get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            bound_models(
                arr.iter()
                    .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(str::to_string)),
            )
        })
        .unwrap_or_default()
}

/// PURE: extract the model ids from an OpenAI-compatible `/v1/models` JSON body. Shape:
/// `{"data":[{"id":"model-name", ...}, ...]}`. Same tolerance as the ollama parser.
/// Bounded count + per-name length (BLOCKER 2).
pub(crate) fn parse_omlx_models(body: &str) -> Vec<String> {
    let value: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    value
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            bound_models(
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(str::to_string)),
            )
        })
        .unwrap_or_default()
}

/// A dedicated, redirect-free probe client: loopback-only targets, short timeouts, NO
/// redirect-following (a compromised local server cannot 3xx us off-box). Built fresh per
/// `detect_providers` call (detection is infrequent, so caching is not worth a `OnceLock`).
pub(crate) fn probe_client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("Devboule/0.1")
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(PROBE_TIMEOUT)
        .timeout(PROBE_TIMEOUT)
        .build()
        .ok()
}

/// Max bytes we will buffer from a probe response body. BLOCKER 2: `resp.text()` has NO
/// size cap, so a hostile/buggy loopback server could stream ~180 MB within the 1.5s
/// wall-time and OOM us. A model listing is a few KiB; 256 KiB is generous headroom while
/// hard-bounding memory. A body that EXCEEDS this (per Content-Length or actual bytes) is
/// rejected outright rather than truncated — a truncated JSON would not parse anyway.
const MAX_PROBE_BODY_BYTES: usize = 256 * 1024;

/// GET a bounded loopback URL and return its body text on a 2xx, else `None`. Failure-
/// isolated: any error (connection refused, timeout, non-2xx, oversized) yields `None` so a
/// single dead provider never poisons the whole detection.
///
/// BLOCKER 2: the body is read with a hard byte cap. We first reject on a declared
/// `Content-Length` over the cap (cheap, before buffering), then buffer the full bytes
/// (`PROBE_TIMEOUT` already bounds wall-time) and reject if the actual length exceeds the
/// cap, before any UTF-8 decode / JSON parse.
pub(crate) async fn probe_get(client: &reqwest::Client, url: &str) -> Option<String> {
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    // Reject early if the server DECLARES an oversized body (avoids buffering it at all).
    if let Some(len) = resp.content_length() {
        if len > MAX_PROBE_BODY_BYTES as u64 {
            return None;
        }
    }
    // Buffer the bytes (wall-time bounded by the client timeout) and enforce the cap on the
    // ACTUAL length too — a server can omit/lie about Content-Length.
    let bytes = resp.bytes().await.ok()?;
    if bytes.len() > MAX_PROBE_BODY_BYTES {
        return None;
    }
    String::from_utf8(bytes.to_vec()).ok()
}

fn looks_like_apple_fm_help(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    lower.contains("foundation model")
        && (lower.contains("fm respond")
            || lower.contains("fm chat")
            || lower.contains("fm schema")
            || lower.contains("apple"))
}

#[cfg(target_os = "macos")]
fn read_probe_pipe<R: std::io::Read + Send + 'static>(
    mut reader: R,
) -> std::thread::JoinHandle<Option<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut limited = reader.by_ref().take((CLI_HELP_PROBE_MAX_BYTES + 1) as u64);
        limited.read_to_end(&mut out).ok()?;
        if out.len() > CLI_HELP_PROBE_MAX_BYTES {
            return None;
        }
        Some(out)
    })
}

#[cfg(target_os = "macos")]
fn apple_fm_help_probe_matches(program: &std::path::Path) -> bool {
    let mut child = match std::process::Command::new(program)
        .arg("--help")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("PATH", augmented_path())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => return false,
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => return false,
    };
    let stdout_thread = read_probe_pipe(stdout);
    let stderr_thread = read_probe_pipe(stderr);

    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() >= PROBE_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                return false;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(25)),
            Err(_) => return false,
        }
    }

    let mut bytes = Vec::new();
    if let Ok(Some(stdout)) = stdout_thread.join() {
        bytes.extend(stdout);
    }
    if let Ok(Some(stderr)) = stderr_thread.join() {
        bytes.extend(stderr);
    }
    let output = String::from_utf8_lossy(&bytes);
    looks_like_apple_fm_help(&output)
}

#[cfg(target_os = "macos")]
fn detect_apple_fm() -> Option<DetectedProvider> {
    Some(match resolve_program("fm") {
        Some(program) if apple_fm_help_probe_matches(&program) => DetectedProvider {
            kind: "appleFm".into(),
            available: true,
            detail: Some("cli".into()),
            models: Vec::new(),
        },
        Some(_) | None => DetectedProvider::unavailable("appleFm"),
    })
}

#[cfg(not(target_os = "macos"))]
fn detect_apple_fm() -> Option<DetectedProvider> {
    None
}

/// Detect every supported provider on this machine. Always returns entries in a stable order
/// (`claude, codex, ollama, omlx, appleFm on macOS only, api`). Probes are bounded +
/// failure-isolated.
pub async fn detect_all_providers() -> Vec<DetectedProvider> {
    let client = probe_client();

    // --- claude / codex: CLI resolution over the augmented PATH ---
    // W2: we resolve only to learn AVAILABILITY; the resolved path is intentionally
    // NOT stored/returned (re-resolved at spawn time via `resolve_program`).
    let claude = match resolve_program("claude") {
        Some(_) => DetectedProvider {
            kind: "claude".into(),
            available: true,
            detail: Some("cli".into()),
            models: Vec::new(),
        },
        None => DetectedProvider::unavailable("claude"),
    };

    let codex = match resolve_program("codex") {
        Some(_) => DetectedProvider {
            kind: "codex".into(),
            available: true,
            detail: Some("cli".into()),
            models: Vec::new(),
        },
        None => DetectedProvider::unavailable("codex"),
    };

    // --- ollama: CLI OR a reachable loopback daemon (the daemon also fills models) ---
    let ollama = {
        let cli = resolve_program("ollama");
        let http_body = match &client {
            Some(c) => probe_get(c, OLLAMA_TAGS_URL).await,
            None => None,
        };
        match (http_body, cli) {
            (Some(body), _cli_path) => DetectedProvider {
                kind: "ollama".into(),
                available: true,
                detail: Some("running".into()),
                models: parse_ollama_tags(&body),
            },
            (None, Some(_)) => DetectedProvider {
                kind: "ollama".into(),
                available: true,
                detail: Some("cli only".into()),
                models: Vec::new(),
            },
            (None, None) => DetectedProvider::unavailable("ollama"),
        }
    };

    // --- oMLX: a reachable loopback OpenAI-compatible server (no CLI concept) ---
    let omlx = {
        let http_body = match &client {
            Some(c) => probe_get(c, OMLX_MODELS_URL).await,
            None => None,
        };
        match http_body {
            Some(body) => DetectedProvider {
                kind: "omlx".into(),
                available: true,
                detail: Some("running".into()),
                models: parse_omlx_models(&body),
            },
            None => DetectedProvider::unavailable("omlx"),
        }
    };

    // --- api: always available; the user supplies a command line ---
    let api = DetectedProvider {
        kind: "api".into(),
        available: true,
        detail: Some("configure a command".into()),
        models: Vec::new(),
    };

    // `detect_apple_fm` spawns `fm --help` and polls `try_wait` for up to ~1.5s on macOS —
    // synchronous, blocking work. Run it off the async reactor via spawn_blocking so it can
    // never stall a Tokio worker. On non-macOS it is a no-op returning None; the join still
    // resolves cheaply. A join failure (task panicked) degrades to "not detected".
    let apple_fm = tauri::async_runtime::spawn_blocking(detect_apple_fm)
        .await
        .unwrap_or(None);

    let mut providers = vec![claude, codex, ollama, omlx];
    if let Some(apple_fm) = apple_fm {
        providers.push(apple_fm);
    }
    providers.push(api);
    providers
}

/// The Tauri command wrapper. No auth gate: this returns ONLY non-secret machine
/// capability metadata (which CLIs exist + which loopback model servers respond), so the
/// Settings UI can populate the provider picker before/while the vault is locked. It never
/// reads vault secrets and never sends user data anywhere.
#[tauri::command]
pub async fn detect_providers() -> Result<Vec<DetectedProvider>, String> {
    Ok(detect_all_providers().await)
}

// ---------------------------------------------------------------------------
// External-tool dependency detection (TASK #13 — Settings "Dependencies" tab).
// ---------------------------------------------------------------------------

/// Max wait for a `<tool> --version` probe. Detection must feel instant; 2s is
/// generous for a local `--version` (no network — just a child process) and keeps
/// the whole `detect_dependencies` snappy even if a tool hangs.
const DEP_VERSION_PROBE_TIMEOUT: Duration = Duration::from_millis(2000);

/// Max chars of a version string we surface. Tools print a sentence like
/// "git version 2.30.1"; we show its first line as-is, but cap it so a chatty
/// tool can't bloat the IPC payload / UI.
const MAX_DEP_VERSION_LEN: usize = 120;

/// One external command-line tool Devboule can use, with its resolved location and
/// version (best-effort). camelCase over the IPC boundary so the TS side reads it
/// directly. Unlike `DetectedProvider`, dependencies intentionally EXPOSE `path`:
/// the Dependencies page is user-requested diagnostics that should show WHERE each
/// tool was found (this is a capability map the user asked to see, not a leak — the
/// same set of names is public in this file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedDependency {
    /// The tool's display name (the binary, monospace in the UI).
    pub name: String,
    /// What Devboule uses the tool for.
    pub purpose: String,
    /// Grouping bucket shown as a section header in the UI.
    pub category: String,
    /// Whether the binary was found on the augmented PATH.
    pub found: bool,
    /// Resolved absolute path when `found`; `None` when not found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Best-effort `tool --version` output (first line), or `None` on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// A hard-coded spec for one tool this machine MAY have. `program` is the binary
/// `resolve_program` looks for; `fallback` (optional) is tried only if the primary
/// is absent (e.g. `python3` → `python`).
struct DependencySpec {
    program: &'static str,
    fallback: Option<&'static str>,
    name: &'static str,
    purpose: &'static str,
    category: &'static str,
}

/// Run `<resolved> --version` and return its first trimmed line (stdout OR stderr —
/// `python --version` famously prints to stderr). Best-effort: any spawn failure, a
/// non-zero exit, or a timeout yields `None`. Bounded (see [`DEP_VERSION_PROBE_TIMEOUT`]
/// + [`MAX_DEP_VERSION_LEN`]) so a misbehaving tool can never hang or bloat detection.
fn probe_version(resolved: &PathBuf) -> Option<String> {
    let mut child = std::process::Command::new(resolved)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Inherit the augmented PATH so a tool that itself spawns another tool (or
        // resolves its own helpers) finds them the same way the spawner would.
        .env("PATH", augmented_path())
        .spawn()
        .ok()?;

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() >= DEP_VERSION_PROBE_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }

    let mut out = Vec::new();
    let mut err = Vec::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = std::io::copy(&mut s, &mut out);
    }
    if let Some(mut s) = child.stderr.take() {
        let _ = std::io::copy(&mut s, &mut err);
    }
    let out_s = String::from_utf8_lossy(&out);
    let err_s = String::from_utf8_lossy(&err);
    let text = out_s
        .lines()
        .next()
        .or_else(|| err_s.lines().next())
        .unwrap_or("")
        .trim()
        .to_string();
    if text.is_empty() {
        None
    } else {
        Some(text.chars().take(MAX_DEP_VERSION_LEN).collect())
    }
}

/// The curated, hard-coded tool list. Order here is the order they appear within
/// their category section (categories are grouped in the frontend). Each purpose +
/// category string matches the TASK #13 spec exactly. No heavy detection — only
/// resolution via [`resolve_program`] + an optional `--version`.
fn dependency_specs() -> Vec<DependencySpec> {
    vec![
        DependencySpec {
            program: "node",
            fallback: None,
            name: "node",
            purpose: "Runs the pi sidecar that powers the orchestrator and coders.",
            category: "Runtime",
        },
        DependencySpec {
            program: "python3",
            fallback: Some("python"),
            name: "python3",
            purpose: "Oracle indexing and embeddings.",
            category: "Runtime",
        },
        DependencySpec {
            program: "git",
            fallback: None,
            name: "git",
            purpose: "In-app version control (status, commit, push).",
            category: "Runtime",
        },
        DependencySpec {
            program: "ruff",
            fallback: None,
            name: "ruff",
            purpose: "Python linter used by the Censor code-review gate.",
            category: "Code review (Censor)",
        },
        DependencySpec {
            program: "oxlint",
            fallback: None,
            name: "oxlint",
            purpose: "Fast JS/TS linter used by the Censor gate.",
            category: "Code review (Censor)",
        },
        DependencySpec {
            program: "eslint",
            fallback: None,
            name: "eslint",
            purpose: "JS/TS linter (Censor gate, if configured).",
            category: "Code review (Censor)",
        },
        DependencySpec {
            program: "pyright",
            fallback: None,
            name: "pyright",
            purpose: "Python type checker (Censor gate).",
            category: "Code review (Censor)",
        },
        DependencySpec {
            program: "cargo",
            fallback: None,
            name: "cargo",
            purpose: "Rust build/test + clippy for Rust projects.",
            category: "Code review (Censor)",
        },
        DependencySpec {
            program: "shellcheck",
            fallback: None,
            name: "shellcheck",
            purpose: "Shell-script linter (Censor gate).",
            category: "Code review (Censor)",
        },
        DependencySpec {
            program: "claude",
            fallback: None,
            name: "claude",
            purpose: "Claude CLI — a cloud orchestrator/coder backend.",
            category: "AI providers",
        },
        DependencySpec {
            program: "codex",
            fallback: None,
            name: "codex",
            purpose: "Codex CLI — a cloud orchestrator/coder backend.",
            category: "AI providers",
        },
        DependencySpec {
            program: "ollama",
            fallback: None,
            name: "ollama",
            purpose: "Local model server backend.",
            category: "AI providers",
        },
    ]
}

/// Detect every external CLI Devboule can use. For each curated tool: resolve it on
/// the augmented PATH (falling back once if a fallback program is listed), run an
/// optional `--version` probe, and return the row. Failure-isolated: one tool's
/// probe error NEVER fails the others (or the whole detection).
pub fn detect_all_dependencies() -> Vec<DetectedDependency> {
    dependency_specs()
        .into_iter()
        .map(|spec| {
            let resolved = resolve_program(spec.program)
                .or_else(|| spec.fallback.and_then(resolve_program));
            match resolved {
                Some(path) => DetectedDependency {
                    name: spec.name.to_string(),
                    purpose: spec.purpose.to_string(),
                    category: spec.category.to_string(),
                    found: true,
                    path: Some(path.to_string_lossy().to_string()),
                    version: probe_version(&path),
                },
                None => DetectedDependency {
                    name: spec.name.to_string(),
                    purpose: spec.purpose.to_string(),
                    category: spec.category.to_string(),
                    found: false,
                    path: None,
                    version: None,
                },
            }
        })
        .collect()
}

/// The Tauri command wrapper. No auth gate: this returns ONLY non-secret machine
/// capability metadata (which CLIs exist + their resolved path/version), so the
/// Settings UI can populate the Dependencies tab before/while the vault is locked.
/// It never reads vault secrets and never sends user data anywhere. The resolution +
/// `--version` probes are blocking, so they run off the async reactor via
/// `spawn_blocking` (a join failure — task panicked — degrades to an error string).
#[tauri::command]
pub async fn detect_dependencies() -> Result<Vec<DetectedDependency>, String> {
    tauri::async_runtime::spawn_blocking(detect_all_dependencies)
        .await
        .map_err(|e| format!("dependency detection failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- augmented_path --------------------------------------------------------

    #[test]
    fn augmented_path_includes_current_path_entries() {
        // Whatever the test runner's PATH is, every entry must survive into the augmented
        // PATH (we only ever APPEND; never drop a user dir).
        let original = std::env::var_os("PATH").unwrap_or_default();
        let augmented = augmented_path();
        let orig_dirs: Vec<_> = std::env::split_paths(&original).collect();
        let aug_dirs: Vec<_> = std::env::split_paths(&augmented).collect();
        for d in orig_dirs {
            if d.as_os_str().is_empty() {
                continue;
            }
            assert!(
                aug_dirs.iter().any(|a| a == &d),
                "augmented PATH dropped an existing dir: {d:?}"
            );
        }
    }

    #[test]
    fn augmented_path_includes_expected_per_os_dirs() {
        let augmented = augmented_path();
        let dirs: Vec<PathBuf> = std::env::split_paths(&augmented).collect();
        let contains_ending = |suffix: &[&str]| {
            dirs.iter().any(|d| {
                let comps: Vec<String> = d
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().to_string())
                    .collect();
                comps.len() >= suffix.len()
                    && comps[comps.len() - suffix.len()..]
                        .iter()
                        .zip(suffix)
                        .all(|(a, b)| a.eq_ignore_ascii_case(b))
            })
        };

        #[cfg(not(windows))]
        {
            assert!(
                dirs.iter().any(|d| d == &PathBuf::from("/usr/local/bin")),
                "missing /usr/local/bin"
            );
            assert!(
                dirs.iter().any(|d| d == &PathBuf::from("/opt/homebrew/bin")),
                "missing /opt/homebrew/bin"
            );
            // Home-relative dirs only when HOME resolves (it does in the test env).
            if user_home().is_some() {
                assert!(contains_ending(&[".local", "bin"]), "missing ~/.local/bin");
                assert!(contains_ending(&[".cargo", "bin"]), "missing ~/.cargo/bin");
            }
        }
        #[cfg(windows)]
        {
            // npm global dir is the load-bearing one for claude.cmd/codex.cmd.
            if std::env::var_os("APPDATA").is_some() {
                assert!(contains_ending(&["npm"]), "missing %APPDATA%\\npm");
            }
            if std::env::var_os("LOCALAPPDATA").is_some() {
                assert!(
                    contains_ending(&["Microsoft", "WindowsApps"]),
                    "missing WindowsApps"
                );
            }
        }
    }

    #[test]
    fn augmented_path_does_not_re_add_an_augmentation_dir_already_on_path() {
        // The de-dup invariant: an augmentation dir that is ALREADY on PATH must not be
        // appended a second time. (We cannot assert the whole PATH is dup-free — the host's
        // own PATH may legitimately contain duplicates we must preserve verbatim.)
        //
        // Pick the FIRST augmentation dir augmented_path() would add, then assert it is not
        // appended again at the END (its count must not exceed the original count + at most
        // one). We compare the augmentation tail against the original head.
        let original: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        let augmented: Vec<PathBuf> = std::env::split_paths(&augmented_path()).collect();

        // Every dir the augmentation appended (beyond the preserved original prefix) must be
        // distinct AND not already present in the original — that is exactly what `push`'s
        // de-dup guarantees.
        assert!(
            augmented.len() >= original.len(),
            "augmented PATH dropped entries"
        );
        let appended = &augmented[original.len()..];
        let mut seen = std::collections::HashSet::new();
        for d in appended {
            if d.as_os_str().is_empty() {
                continue;
            }
            assert!(
                !original.iter().any(|o| o == d),
                "augmentation re-added a dir already on PATH: {d:?}"
            );
            assert!(
                seen.insert(d.clone()),
                "augmentation appended a duplicate dir: {d:?}"
            );
        }
    }

    // -- WARNING 2: case-(in)sensitive PATH dedup ------------------------------

    #[test]
    fn path_eq_is_case_insensitive_on_windows_only() {
        let a = PathBuf::from(r"C:\Users\Me\AppData\Roaming\npm");
        let b = PathBuf::from(r"c:\users\me\appdata\roaming\NPM");
        #[cfg(windows)]
        assert!(path_eq(&a, &b), "Windows PATH dedup must be case-insensitive");
        #[cfg(not(windows))]
        assert!(!path_eq(&a, &b), "Unix paths are case-sensitive");

        // Identical paths always compare equal on every OS.
        assert!(path_eq(&a, &a));
    }

    #[test]
    #[cfg(windows)]
    fn augmented_path_dedups_differently_cased_npm_dir() {
        // The fix's contract: an augmentation dir already on PATH in a DIFFERENT case must
        // NOT be appended again. We control PATH for the duration so the host's own npm entry
        // can't muddy the count: PATH = [lowercased-npm] ONLY. augmented_path() must then NOT
        // append the real-cased npm dir (case-insensitive dedup) -> npm appears exactly once.
        let appdata = match std::env::var_os("APPDATA") {
            Some(a) => PathBuf::from(a),
            None => return, // no APPDATA in this env -> nothing to assert
        };
        let npm = appdata.join("npm");
        let lowered = npm.to_string_lossy().to_ascii_lowercase();

        // SAFETY: single-threaded test mutation of PATH; restored before any assert.
        let prev = std::env::var_os("PATH");
        std::env::set_var("PATH", &lowered); // PATH contains ONLY the lowercased npm dir

        let aug: Vec<PathBuf> = std::env::split_paths(&augmented_path()).collect();
        let npm_lower = npm.to_string_lossy().to_ascii_lowercase();
        let count = aug
            .iter()
            .filter(|d| d.to_string_lossy().to_ascii_lowercase() == npm_lower)
            .count();

        // Restore PATH before asserting so a failure can't poison other tests.
        match prev {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        assert_eq!(count, 1, "npm dir must appear once despite case difference");
    }

    // -- resolve_program -------------------------------------------------------

    #[test]
    fn resolve_program_finds_a_known_present_binary() {
        // A binary guaranteed present on each platform's PATH.
        #[cfg(windows)]
        let known = "cmd"; // resolves cmd.exe via the extension candidates
        #[cfg(not(windows))]
        let known = "sh";

        let resolved = resolve_program(known);
        assert!(
            resolved.is_some(),
            "expected to resolve a known binary {known:?}"
        );
        let p = resolved.unwrap();
        assert!(p.is_file(), "resolved path is not a file: {p:?}");
    }

    #[test]
    fn resolve_program_returns_none_for_absent_binary() {
        assert!(
            resolve_program("definitely_not_a_real_program_xyz_12345").is_none(),
            "a guaranteed-absent name must not resolve"
        );
    }

    #[test]
    fn resolve_program_rejects_empty() {
        assert!(resolve_program("").is_none());
    }

    #[test]
    fn program_candidates_windows_tries_cmd_and_exe() {
        let cands = program_candidates("claude");
        #[cfg(windows)]
        {
            assert!(cands.iter().any(|c| c == "claude.cmd"), "{cands:?}");
            assert!(cands.iter().any(|c| c == "claude.exe"), "{cands:?}");
            assert!(cands.iter().any(|c| c == "claude"), "{cands:?}");
            // The .cmd shim must be tried FIRST (npm CLIs are .cmd on Windows).
            assert_eq!(cands.first().map(String::as_str), Some("claude.cmd"));
        }
        #[cfg(not(windows))]
        {
            assert_eq!(cands, vec!["claude".to_string()]);
        }
    }

    #[test]
    fn program_candidates_windows_trusts_explicit_extension() {
        let cands = program_candidates("foo.exe");
        #[cfg(windows)]
        assert_eq!(cands, vec!["foo.exe".to_string()]);
        #[cfg(not(windows))]
        assert_eq!(cands, vec!["foo.exe".to_string()]);
    }

    // -- model-list parsers (PURE) ---------------------------------------------

    #[test]
    fn parse_ollama_tags_extracts_names() {
        let body = r#"{"models":[{"name":"qwen2.5:7b"},{"name":"llama3.2:latest"}]}"#;
        assert_eq!(parse_ollama_tags(body), vec!["qwen2.5:7b", "llama3.2:latest"]);
    }

    #[test]
    fn parse_ollama_tags_tolerates_missing_and_malformed() {
        assert!(parse_ollama_tags("{}").is_empty());
        assert!(parse_ollama_tags("not json").is_empty());
        assert!(parse_ollama_tags(r#"{"models":"nope"}"#).is_empty());
        // An entry without a name is skipped, a good one alongside it is kept.
        assert_eq!(
            parse_ollama_tags(r#"{"models":[{"size":1},{"name":"keep"}]}"#),
            vec!["keep"]
        );
    }

    #[test]
    fn parse_omlx_models_extracts_ids() {
        let body = r#"{"data":[{"id":"mlx-qwen"},{"id":"mlx-llama"}]}"#;
        assert_eq!(parse_omlx_models(body), vec!["mlx-qwen", "mlx-llama"]);
    }

    #[test]
    fn parse_omlx_models_tolerates_missing_and_malformed() {
        assert!(parse_omlx_models("{}").is_empty());
        assert!(parse_omlx_models("garbage").is_empty());
        assert_eq!(
            parse_omlx_models(r#"{"data":[{"object":"model"},{"id":"keep"}]}"#),
            vec!["keep"]
        );
    }

    // -- BLOCKER 2: model count + name-length caps ------------------------------

    #[test]
    fn parse_ollama_tags_caps_model_count() {
        // A hostile server lists far more than MAX_MODELS; the parser truncates.
        let entries: Vec<String> = (0..(MAX_MODELS + 50))
            .map(|i| format!(r#"{{"name":"m{i}"}}"#))
            .collect();
        let body = format!(r#"{{"models":[{}]}}"#, entries.join(","));
        let out = parse_ollama_tags(&body);
        assert_eq!(out.len(), MAX_MODELS, "model list must be capped");
    }

    #[test]
    fn parse_omlx_models_caps_model_count() {
        let entries: Vec<String> = (0..(MAX_MODELS + 50))
            .map(|i| format!(r#"{{"id":"m{i}"}}"#))
            .collect();
        let body = format!(r#"{{"data":[{}]}}"#, entries.join(","));
        assert_eq!(parse_omlx_models(&body).len(), MAX_MODELS);
    }

    #[test]
    fn parse_ollama_tags_drops_overlong_name() {
        let long = "x".repeat(MAX_MODEL_NAME_LEN + 1);
        let body = format!(r#"{{"models":[{{"name":"{long}"}},{{"name":"keep"}}]}}"#);
        // The over-long name is dropped; the sane one is kept.
        assert_eq!(parse_ollama_tags(&body), vec!["keep"]);
    }

    #[test]
    fn parse_omlx_models_drops_overlong_name_and_empty() {
        let long = "y".repeat(MAX_MODEL_NAME_LEN + 1);
        let body = format!(r#"{{"data":[{{"id":"{long}"}},{{"id":""}},{{"id":"keep"}}]}}"#);
        assert_eq!(parse_omlx_models(&body), vec!["keep"]);
    }

    #[test]
    fn parse_ollama_tags_keeps_name_at_exact_cap() {
        let at_cap = "z".repeat(MAX_MODEL_NAME_LEN);
        let body = format!(r#"{{"models":[{{"name":"{at_cap}"}}]}}"#);
        assert_eq!(parse_ollama_tags(&body), vec![at_cap]);
    }

    // -- DetectedProvider serde (camelCase) ------------------------------------

    #[test]
    fn detected_provider_serializes_camel_case() {
        let p = DetectedProvider {
            kind: "ollama".into(),
            available: true,
            detail: Some("running".into()),
            models: vec!["qwen".into()],
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"kind\":\"ollama\""), "{json}");
        assert!(json.contains("\"available\":true"), "{json}");
        assert!(json.contains("\"detail\":\"running\""), "{json}");
        assert!(json.contains("\"models\":[\"qwen\"]"), "{json}");
    }

    #[test]
    fn w2_detected_provider_never_serializes_a_path_field() {
        // W2: the IPC payload must NOT carry a filesystem path under ANY field name.
        let p = DetectedProvider {
            kind: "claude".into(),
            available: true,
            detail: Some("cli".into()),
            models: Vec::new(),
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("\"path\""), "path must not leak over IPC: {json}");
        assert!(!json.contains("/"), "no filesystem path may appear: {json}");
    }

    #[test]
    fn detected_provider_unavailable_omits_optional_fields() {
        let p = DetectedProvider::unavailable("codex");
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, r#"{"kind":"codex","available":false,"models":[]}"#);
    }

    #[test]
    fn apple_fm_help_marker_requires_apple_foundation_model_cli_text() {
        assert!(looks_like_apple_fm_help(
            "Usage: fm respond\nApple Foundation Models on-device CLI\n"
        ));
        assert!(looks_like_apple_fm_help(
            "fm schema\nFoundation Model framework tools\n"
        ));
        assert!(!looks_like_apple_fm_help("fast file manager\nusage: fm <path>\n"));
        assert!(!looks_like_apple_fm_help("respond to terminal prompts\n"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn apple_fm_detection_is_not_offered_on_non_macos() {
        assert!(detect_apple_fm().is_none());
    }
}
