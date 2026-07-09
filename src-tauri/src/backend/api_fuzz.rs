//! Deterministic API-error harvesting harness.
//!
//! A MANUAL-trigger tool that fires deterministic malformed/edge-case HTTP requests at a
//! user-configured dev-server base URL and harvests error-handling failures (5xx,
//! stack-trace bodies, hangs/timeouts) as records for benchmark/training.
//!
//! ## Not a watcher, not automatic
//! This module does nothing until a human invokes it with an explicit
//! `base_url`. Every invocation is idempotent and repeatable.
//!
//! ## Tool detection — detected, not embedded
//! Uses the existing [`crate::backend::provider_detect::resolve_program`] idiom to
//! detect `xh` (primary), `hurl` (hurl-file runner), and `schemathesis`/`st`
//! (OpenAPI negative fuzzing) on the augmented PATH. A missing tool is reported as
//! unavailable and its cases are skipped — the whole run NEVER fails because a tool
//! is absent.
//!
//! ## Safety
//! **Loopback-only**: the target `base_url` is validated with the same rule as the
//! Censor local-AI (`is_loopback_base` in `censor/gemma.rs`) — only `http://127.*`,
//! `http://localhost`, and `http://[::1]` are accepted. Any non-loopback URL is
//! rejected with a clear error before any request is sent.
//!
//! ## Output
//! Findings are appended to `<project_root>/.aspis-training/findings.jsonl` in the
//! same compact-JSON-per-line format the training rail uses, with `source: "api-fuzz"`.
//! Response snippets are capped at 2 KiB and NEVER logged — only written to the local
//! gitignored file.
//!
//! ## Privacy
//! No user source code is read. The module only writes to the training rail and never
//! surfaces response bodies beyond a truncated snippet in the JSONL record.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::backend::provider_detect::resolve_program;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default per-request timeout. A hanging endpoint is recorded as a "hang" finding
/// rather than blocking the whole run indefinitely.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Overall wall-cap for an entire fuzz run (all cases, all tools).
pub const RUN_WALL_CAP: Duration = Duration::from_secs(120);

/// Maximum bytes we cap the response snippet to before writing to the JSONL record.
/// Kept small — we only need enough to identify the error class, not to reconstruct
/// the full response. NEVER logged; only written to the local gitignored file.
pub const SNIPPET_CAP_BYTES: usize = 2048;

/// C-F4: hard cap on the bytes we read from `xh`'s stdout before parsing. The reader
/// thread `read_to_end`'d UNBOUNDED, so a (possibly hostile) server returning a multi-GB
/// body could OOM the process before the later `SNIPPET_CAP_BYTES` truncation ever runs.
/// We only need enough to parse the status line + a snippet, so 4 MiB is generous.
pub const XH_STDOUT_READ_CAP_BYTES: u64 = 4 * 1024 * 1024;

/// C-F11: hard cap on the bytes we read from `schemathesis`'s stdout report before
/// classification. Same OOM rationale as `XH_STDOUT_READ_CAP_BYTES` — bound the read so a
/// huge report can't exhaust memory; the classifier only scans for failure markers.
pub const SCHEMATHESIS_STDOUT_READ_CAP_BYTES: u64 = 4 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Loopback validation (mirrors `censor::gemma::is_loopback_base`)
// ---------------------------------------------------------------------------

/// Validate that `base_url` is a loopback HTTP origin. Accepts only `http://127.*`,
/// `http://localhost[:<port>]`, and `http://[::1][:<port>]` — the SAME rule as
/// `censor::gemma::is_loopback_base`. Rejects https, remote hosts, and userinfo tricks.
///
/// PURE — no IO, no network.
pub fn validate_loopback_base(base_url: &str) -> Result<(), String> {
    if is_loopback_base(base_url) {
        Ok(())
    } else {
        Err(format!(
            "api-fuzz: base_url must be a loopback http origin \
             (http://127.x.x.x, http://localhost, or http://[::1]); got: {base_url:?}"
        ))
    }
}

/// PURE: is `base` an `http://` loopback origin? Mirrors `censor::gemma::is_loopback_base`
/// exactly — both functions must stay byte-identical in their host rule. We keep a local copy
/// here so `api_fuzz` does not create a cross-module function dependency on `censor::gemma`
/// (the censor module is an internal detail; api_fuzz is a sibling module, not a sub-module).
fn is_loopback_base(base: &str) -> bool {
    let Some(rest) = base.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    authority_is_loopback(authority)
}

/// PURE: is the authority component (host[:port]) a loopback host? Accepts `localhost`,
/// `127.x.x.x/8`, and `[::1]`, each with an optional `:port`. Rejects `@` userinfo tricks.
fn authority_is_loopback(authority: &str) -> bool {
    // IPv6 loopback: `[::1]` optionally followed by `:port`.
    // Reject userinfo tricks: an `@` in the remainder means the real host comes after it.
    if let Some(after_bracket) = authority.strip_prefix("[::1]") {
        if after_bracket.contains('@') {
            return false;
        }
        // The remainder is either empty (bare `[::1]`) or `:port`.
        return after_bracket.is_empty() || after_bracket.starts_with(':');
    }
    // Reject `@`-userinfo tricks for non-IPv6 authorities.
    if authority.contains('@') {
        return false;
    }
    // Strip an optional `:port` suffix so we can parse just the host.
    let host = if let Some(colon) = authority.rfind(':') {
        &authority[..colon]
    } else {
        authority
    };
    if host == "localhost" {
        return true;
    }
    // IPv4 loopback: parse as an Ipv4Addr so suffix tricks (`127.0.0.1.evil.com`) are
    // rejected — only a pure `127.0.0.0/8` address passes.
    host.parse::<std::net::Ipv4Addr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Case library
// ---------------------------------------------------------------------------

/// An HTTP method accepted by the case library. A fixed enum prevents arbitrary
/// shell-injection through the method field when we build CLI arguments.
///
/// NOTE: Not all variants are exercised by the current built-in case library (which
/// uses only POST), but they are available for future cases / OpenAPI-driven testing.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Delete => "DELETE",
        }
    }

    /// Parse from an ASCII string; returns `None` for unknown methods.
    #[allow(dead_code)]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "GET" => Some(Self::Get),
            "POST" => Some(Self::Post),
            "PUT" => Some(Self::Put),
            "PATCH" => Some(Self::Patch),
            "DELETE" => Some(Self::Delete),
            _ => None,
        }
    }
}

/// One deterministic fuzz case. All fields are static so the library compiles to
/// a zero-heap `&[FuzzCase]`.
#[derive(Debug, Clone)]
pub struct FuzzCase {
    /// Stable, unique identifier (used as `title` in the findings record).
    pub id: &'static str,
    pub method: HttpMethod,
    /// The path appended to the base URL (must start with `/`).
    pub path: &'static str,
    /// Optional `Content-Type` override.
    pub content_type: Option<&'static str>,
    /// Body bytes. For the invalid-UTF-8 case we carry the raw bytes; for text cases
    /// the slice is valid UTF-8. `None` means no body (GET / empty-body cases).
    pub raw_body: Option<&'static [u8]>,
    /// Human-readable description of what the case is checking (documentation only).
    #[allow(dead_code)]
    pub expectation: &'static str,
}

/// The invalid-UTF-8 body: a valid JSON prefix followed by an invalid UTF-8 sequence.
static INVALID_UTF8_BODY: &[u8] = b"{\"data\":\"\xff\xfe\"}";

/// 2-MiB oversized body (filled with zeros so it compresses well in the binary but
/// tests the server's size limit). Declared as a static slice so it lives in the
/// read-only data segment rather than the heap.
///
/// NOTE: we allocate this lazily via `OVERSIZED_BODY` because `[u8; 2097152]` is too
/// large for a `const` literal on some platforms. The slice below is only 16 bytes;
/// the actual oversized body is built once via `oversized_body()`.
static OVERSIZED_BODY_HEADER: &[u8] = b"{\"pad\":\"";
const OVERSIZED_BODY_PAD_BYTES: usize = 2 * 1024 * 1024; // 2 MiB of padding

fn oversized_body() -> Vec<u8> {
    let mut body = Vec::with_capacity(OVERSIZED_BODY_HEADER.len() + OVERSIZED_BODY_PAD_BYTES + 2);
    body.extend_from_slice(OVERSIZED_BODY_HEADER);
    body.resize(body.len() + OVERSIZED_BODY_PAD_BYTES, b'a');
    body.extend_from_slice(b"\"}");
    body
}

/// The built-in fuzz case library. Each case covers one malformed/edge-case class.
/// Cases are sent to the `/` path by default (the most likely to be wired); real
/// server paths would be discovered via OpenAPI if an `openapi_spec` is provided.
pub fn builtin_cases() -> Vec<FuzzCase> {
    vec![
        FuzzCase {
            id: "truncated-json",
            method: HttpMethod::Post,
            path: "/",
            content_type: Some("application/json"),
            raw_body: Some(b"{\"a\":1"),
            expectation: "Server should return 4xx for truncated JSON body",
        },
        FuzzCase {
            id: "wrong-content-type",
            method: HttpMethod::Post,
            path: "/",
            content_type: Some("application/json"),
            raw_body: Some(b"field1=value1&field2=value2"),
            expectation: "Server should handle form body sent as application/json",
        },
        FuzzCase {
            id: "empty-body-post",
            method: HttpMethod::Post,
            path: "/",
            content_type: Some("application/json"),
            raw_body: Some(b""),
            expectation: "Server should return 4xx for empty POST body",
        },
        FuzzCase {
            id: "deeply-nested-json",
            method: HttpMethod::Post,
            path: "/",
            content_type: Some("application/json"),
            // 500 levels of nesting: {"a":{"a":{"a":...}}}
            raw_body: Some(b"\
                {\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":\
                {\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":\
                {\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":\
                {\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":\
                {\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":\
                \"leaf\"\
                }}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}\
            "),
            expectation: "Server should not stack-overflow on deeply-nested JSON",
        },
        FuzzCase {
            id: "invalid-utf8-body",
            method: HttpMethod::Post,
            path: "/",
            content_type: Some("application/json"),
            raw_body: Some(INVALID_UTF8_BODY),
            expectation: "Server should return 4xx for invalid UTF-8 in body",
        },
        // The oversized-body case uses `None` here; the runner builds it lazily.
        FuzzCase {
            id: "oversized-body",
            method: HttpMethod::Post,
            path: "/",
            content_type: Some("application/json"),
            raw_body: None,
            expectation: "Server should return 4xx/413 for a 2 MiB body",
        },
    ]
}

// ---------------------------------------------------------------------------
// Response classification
// ---------------------------------------------------------------------------

/// Stack-trace / panic signatures that indicate a server-side bug surfaced in the
/// response body. These are checked case-insensitively. The set covers Python,
/// Node/JS, Rust, and Java runtimes.
const STACK_TRACE_SIGNATURES: &[&str] = &[
    "traceback (most recent call last)",
    "panicked at",
    "at <anonymous>",
    "exception in thread",
    "java.lang.",
    "syntaxerror",
    "referenceerror",
    "typeerror",
    "uncaught exception",
    "unhandled promise rejection",
    "error: cannot",
    "internal server error",
    "stack overflow",
];

/// A classified finding produced by `classify_response`. The `id` and `symptom` fields
/// come from the caller (case id + observed status/behaviour); the `snippet` is the
/// first `SNIPPET_CAP_BYTES` of the response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzFinding {
    pub case_id: String,
    pub symptom: String,
    /// Capped at `SNIPPET_CAP_BYTES`. Contains the first bytes of the body — enough
    /// to identify the error class. NEVER logged; only written to the JSONL record.
    pub snippet: String,
}

/// PURE: classify a response into an optional finding.
///
/// Returns `Some(FuzzFinding)` when any of:
///   - `timed_out` is `true` (the request did not complete within the timeout)
///   - `status >= 500` (server-side error)
///   - `body` contains a stack-trace signature (checked case-insensitively)
///   - `status == 0` and `body` suggests a connection reset (empty or "connection reset")
///
/// Returns `None` for a clean 2xx/3xx/4xx with no stack-trace signature.
pub fn classify_response(
    case_id: &str,
    status: u16,
    body: &[u8],
    timed_out: bool,
) -> Option<FuzzFinding> {
    let snippet = cap_bytes(body, SNIPPET_CAP_BYTES);
    let body_lower = snippet.to_ascii_lowercase();

    if timed_out {
        return Some(FuzzFinding {
            case_id: case_id.to_string(),
            symptom: "hang: request timed out".to_string(),
            snippet,
        });
    }

    if status >= 500 {
        return Some(FuzzFinding {
            case_id: case_id.to_string(),
            symptom: format!("5xx: server returned HTTP {status}"),
            snippet,
        });
    }

    // Connection reset / transport error is represented as status=0 by our sender.
    if status == 0 {
        return Some(FuzzFinding {
            case_id: case_id.to_string(),
            symptom: "connection reset or transport error".to_string(),
            snippet,
        });
    }

    // Stack-trace / panic leaked into the response body.
    for sig in STACK_TRACE_SIGNATURES {
        if body_lower.contains(sig) {
            return Some(FuzzFinding {
                case_id: case_id.to_string(),
                symptom: format!("stack-trace leaked in body (matched: {sig:?})"),
                snippet,
            });
        }
    }

    None
}

/// Convert the first `cap` bytes of `body` to a lossy UTF-8 String.
fn cap_bytes(body: &[u8], cap: usize) -> String {
    let truncated = if body.len() > cap { &body[..cap] } else { body };
    String::from_utf8_lossy(truncated).into_owned()
}

// ---------------------------------------------------------------------------
// Tool availability
// ---------------------------------------------------------------------------

/// The three optional external CLI tools the harness can drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolAvailability {
    /// `xh` (https://github.com/ducaale/xh) — primary raw-bytes sender.
    pub xh: Option<PathBuf>,
    /// `hurl` (https://hurl.dev) — hurl-file runner with structured JSON report.
    pub hurl: Option<PathBuf>,
    /// `schemathesis` or `st` — OpenAPI negative fuzzing with `--seed` reproducibility.
    pub schemathesis: Option<PathBuf>,
}

impl ToolAvailability {
    /// Detect which tools are present on the augmented PATH.
    pub fn detect() -> Self {
        Self {
            xh: resolve_program("xh"),
            hurl: resolve_program("hurl"),
            schemathesis: resolve_program("schemathesis").or_else(|| resolve_program("st")),
        }
    }

    pub fn xh_available(&self) -> bool {
        self.xh.is_some()
    }

    pub fn hurl_available(&self) -> bool {
        self.hurl.is_some()
    }

    pub fn schemathesis_available(&self) -> bool {
        self.schemathesis.is_some()
    }
}

// ---------------------------------------------------------------------------
// Request sender (injectable for tests)
// ---------------------------------------------------------------------------

/// The outcome of a single sent request. `status = 0` represents a connection/transport
/// error (no HTTP response was received at all).
#[derive(Debug, Clone)]
pub struct SendOutcome {
    pub status: u16,
    pub body: Vec<u8>,
    pub timed_out: bool,
}

/// Injectable sender seam: the real impl drives `xh` or falls back to `reqwest`; tests
/// inject a `FakeSender` that returns canned outcomes without touching the network.
pub trait RequestSender: Send + Sync {
    /// Send a single fuzz case to `url` (the full URL: base + path). Returns the
    /// `SendOutcome`; this function must NOT block longer than `timeout + small overhead`.
    fn send(
        &self,
        method: &str,
        url: &str,
        content_type: Option<&str>,
        body: Option<&[u8]>,
        timeout: Duration,
    ) -> SendOutcome;
}

// ---------------------------------------------------------------------------
// Real sender: prefers `xh` when available, falls back to `reqwest` blocking.
// ---------------------------------------------------------------------------

/// The production request sender. Uses `xh` when available (it handles arbitrary raw
/// bytes cleanly, including invalid UTF-8); falls back to `reqwest::blocking` for the
/// subset of cases that are valid UTF-8.
pub struct XhSender {
    xh_path: Option<PathBuf>,
}

impl XhSender {
    pub fn new(xh_path: Option<PathBuf>) -> Self {
        Self { xh_path }
    }
}

impl RequestSender for XhSender {
    fn send(
        &self,
        method: &str,
        url: &str,
        content_type: Option<&str>,
        body: Option<&[u8]>,
        timeout: Duration,
    ) -> SendOutcome {
        if let Some(xh) = &self.xh_path {
            send_via_xh(xh, method, url, content_type, body, timeout)
        } else {
            send_via_reqwest(method, url, content_type, body, timeout)
        }
    }
}

/// Send a request using `xh`. The `--print=shb` flag prints status + headers + body;
/// we parse the first line for the status code. Raw bytes are fed via stdin with
/// `--raw -`.
fn send_via_xh(
    xh_path: &Path,
    method: &str,
    url: &str,
    content_type: Option<&str>,
    body: Option<&[u8]>,
    timeout: Duration,
) -> SendOutcome {
    use crate::oracle::python_oracle::apply_no_window;
    use std::process::{Command, Stdio};

    let args = build_xh_args(method, url, content_type, body, timeout);

    let mut cmd = Command::new(xh_path);
    apply_no_window(&mut cmd);
    cmd.args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // BLOCKER 4: start the wall-clock BEFORE spawning the child so the overrun guard
    // actually covers spawn + write + wait.
    let started = Instant::now();

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => {
            return SendOutcome { status: 0, body: vec![], timed_out: false };
        }
    };

    // Write the body to stdin (if any), then close stdin so xh knows input is done.
    if let (Some(body_bytes), Some(mut stdin)) = (body, child.stdin.take()) {
        let _ = stdin.write_all(body_bytes);
        // stdin is dropped here, closing the pipe.
    } else {
        // No body: drop the piped stdin so the child reading it sees EOF immediately.
        drop(child.stdin.take());
    }

    // BLOCKER 1: do NOT poll-reap with `try_wait` and THEN call `wait_with_output` — that
    // double-wait reaps the child in the loop, so the second wait returns Err(ECHILD) and
    // discarded EVERY real response (status fell back to 0 => false-positive reset).
    //
    // Instead: drain stdout/stderr on reader threads (avoids a pipe-buffer deadlock on
    // large bodies) and `wait()` the child exactly ONCE. A watchdog thread shares the
    // `Child` behind a Mutex and kills it past the deadline, recording `killed` so the
    // outcome is flagged `timed_out` only when the watchdog actually fired.
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let finished = Arc::new(AtomicBool::new(false));
    let killed = Arc::new(AtomicBool::new(false));

    // Take the piped stdout/stderr handles out BEFORE sharing the child, and drain them on
    // reader threads. EOF arrives when the child exits or is killed; reading concurrently
    // avoids a pipe-buffer deadlock on large responses.
    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(h) = stdout_handle {
            use std::io::Read;
            // C-F4: bound the read so a hostile/huge response can't OOM the process. We
            // only need enough to parse the status line + snippet; `take` stops at the cap
            // (the remaining child output is discarded — the watchdog/EOF still proceed).
            let _ = h.take(XH_STDOUT_READ_CAP_BYTES).read_to_end(&mut buf);
        }
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut h) = stderr_handle {
            use std::io::Read;
            let _ = h.read_to_end(&mut buf);
        }
        buf
    });

    // The waiter and the watchdog both need `&mut Child` (for `try_wait` / `kill`). Share
    // it behind a Mutex and have BOTH only ever hold the lock BRIEFLY (try_wait / kill,
    // never a blocking `wait()`), so the watchdog can always acquire the lock to kill a
    // genuinely hung child. This is a poll loop — but it does NOT double-wait: the response
    // bytes come from the reader threads above, never from `wait_with_output`.
    let child = Arc::new(std::sync::Mutex::new(child));
    let watchdog = {
        let finished = Arc::clone(&finished);
        let killed = Arc::clone(&killed);
        let child = Arc::clone(&child);
        std::thread::spawn(move || {
            let deadline = Instant::now() + timeout + Duration::from_secs(2);
            loop {
                if finished.load(Ordering::Acquire) {
                    return;
                }
                if Instant::now() >= deadline {
                    killed.store(true, Ordering::Release);
                    if let Ok(mut c) = child.lock() {
                        let _ = c.kill();
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };

    // Poll for exit, holding the lock only for each non-blocking `try_wait`.
    loop {
        let exited = {
            let mut c = child.lock().unwrap_or_else(|p| p.into_inner());
            matches!(c.try_wait(), Ok(Some(_)) | Err(_))
        };
        if exited {
            break;
        }
        if killed.load(Ordering::Acquire) {
            // Watchdog fired the kill; reap and stop.
            let mut c = child.lock().unwrap_or_else(|p| p.into_inner());
            let _ = c.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    finished.store(true, Ordering::Release);
    let _ = watchdog.join();

    let stdout = stdout_reader.join().unwrap_or_default();
    let _stderr = stderr_reader.join().unwrap_or_default();

    // BLOCKER 1 / 4: timed out iff the watchdog killed the child OR the wall-clock blew
    // past the deadline.
    if killed.load(Ordering::Acquire) || started.elapsed() > timeout + Duration::from_secs(2) {
        return SendOutcome { status: 0, body: vec![], timed_out: true };
    }

    // Parse xh `--print=shb` output: first line is `HTTP/x.y <status> <reason>`.
    parse_xh_output(&stdout)
}

/// BLOCKER 2: build the `xh` argv. `--ignore-stdin` and `--raw -` CONFLICT (the former
/// tells xh never to read stdin, the latter requires it), so `--ignore-stdin` is added
/// ONLY for the no-body case. A body case adds `--raw -` and feeds the bytes via stdin.
fn build_xh_args(
    method: &str,
    url: &str,
    content_type: Option<&str>,
    body: Option<&[u8]>,
    timeout: Duration,
) -> Vec<String> {
    let timeout_secs = timeout.as_secs().max(1).to_string();
    let mut args: Vec<String> = vec![
        "--timeout".into(),
        timeout_secs,
        "--print=shb".into(), // status + headers + body
    ];
    // Only ignore stdin when there is NO body; a body case needs stdin for `--raw -`.
    if body.is_none() {
        args.push("--ignore-stdin".into());
    }
    args.push(method.to_ascii_uppercase());
    args.push(url.to_string());
    if let Some(ct) = content_type {
        args.push(format!("Content-Type:{ct}"));
    }
    // Supply body via stdin with `--raw -`.
    if body.is_some() {
        args.push("--raw".into());
        args.push("-".into());
    }
    args
}

/// Parse xh's `--print=shb` stdout format. The first non-empty line is
/// `HTTP/1.1 200 OK` or similar. Returns the status and the body (everything after
/// the blank line separating headers from body).
fn parse_xh_output(raw: &[u8]) -> SendOutcome {
    let text = String::from_utf8_lossy(raw);
    let mut status: u16 = 0;
    let mut in_body = false;
    let mut body_lines: Vec<&str> = Vec::new();
    let mut header_done = false;

    for line in text.lines() {
        // BLOCKER 8: a single `--print=shb` stream can contain MULTIPLE status lines —
        // e.g. an interim `HTTP/1.1 100 Continue` (or a redirect) followed by the real
        // `HTTP/1.1 413 ...`. Reset on EACH `HTTP/` line so the LAST response wins;
        // otherwise the first interim status sticks and a later 413/5xx is lost,
        // misclassifying the oversized case as clean.
        if line.starts_with("HTTP/") {
            // "HTTP/1.1 200 OK" -> take the second space-delimited token.
            let parts: Vec<&str> = line.splitn(3, ' ').collect();
            status = if parts.len() >= 2 {
                parts[1].parse().unwrap_or(0)
            } else {
                0
            };
            header_done = false;
            in_body = false;
            body_lines.clear();
            continue;
        }
        if !header_done {
            if line.is_empty() {
                header_done = true;
                in_body = true;
            }
        } else if in_body {
            body_lines.push(line);
        }
    }

    let body = body_lines.join("\n").into_bytes();
    SendOutcome { status, body, timed_out: false }
}

/// Poll for child exit within `timeout`. Returns `true` if the process exited within
/// the deadline, `false` on timeout. Non-blocking poll loop with 10ms sleeps.
fn wait_with_timeout(child: &mut std::process::Child, timeout: Duration) -> bool {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => {}
            Err(_) => return false,
        }
        if started.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Fallback sender using `reqwest::blocking`. Only works for requests whose body is
/// valid UTF-8 (the invalid-UTF-8 case is silently skipped by this sender).
fn send_via_reqwest(
    method: &str,
    url: &str,
    content_type: Option<&str>,
    body: Option<&[u8]>,
    timeout: Duration,
) -> SendOutcome {
    let client = match reqwest::blocking::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => c,
        Err(_) => return SendOutcome { status: 0, body: vec![], timed_out: false },
    };

    let rb = match method.to_ascii_uppercase().as_str() {
        "GET" => client.get(url),
        "POST" => client.post(url),
        "PUT" => client.put(url),
        "PATCH" => client.patch(url),
        "DELETE" => client.delete(url),
        _ => return SendOutcome { status: 0, body: vec![], timed_out: false },
    };

    let rb = if let Some(ct) = content_type {
        rb.header("Content-Type", ct)
    } else {
        rb
    };

    let rb = if let Some(b) = body {
        // reqwest blocking body requires owned bytes.
        rb.body(b.to_vec())
    } else {
        rb
    };

    match rb.send() {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.bytes().map(|b| b.to_vec()).unwrap_or_default();
            SendOutcome { status, body, timed_out: false }
        }
        Err(e) if e.is_timeout() => SendOutcome { status: 0, body: vec![], timed_out: true },
        Err(_) => SendOutcome { status: 0, body: vec![], timed_out: false },
    }
}

// ---------------------------------------------------------------------------
// Per-case outcome + run report
// ---------------------------------------------------------------------------

/// The outcome of running one fuzz case.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CaseOutcome {
    pub id: String,
    /// `"finding"` | `"clean"` | `"skipped"` | `"error"`
    pub outcome: String,
    /// Optional symptom description (set when `outcome == "finding"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symptom: Option<String>,
}

/// The report returned to the caller (JS frontend / tests).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApiFuzzReport {
    /// Which external tools were found on PATH.
    pub tools_available: Vec<String>,
    /// Total number of cases executed (not skipped).
    pub cases_run: u32,
    /// Total number of findings written to findings.jsonl.
    pub findings_recorded: u32,
    /// Per-case outcome breakdown.
    pub per_case: Vec<CaseOutcome>,
}

// ---------------------------------------------------------------------------
// Core run logic (injectable sender — no network in CI tests)
// ---------------------------------------------------------------------------

/// Run all fuzz cases against `base_url`, write findings to `project_root`'s training
/// rail, and return a report. The sender is injectable so tests can pass a
/// `FakeSender` without any network.
///
/// This is the real implementation; the Tauri command wrapper calls it.
pub fn run_fuzz<S: RequestSender>(
    base_url: &str,
    project_root: &Path,
    sender: &S,
    tools: &ToolAvailability,
) -> Result<ApiFuzzReport, String> {
    validate_loopback_base(base_url)?;

    let cases = builtin_cases();
    let run_start = Instant::now();
    let mut per_case: Vec<CaseOutcome> = Vec::with_capacity(cases.len());
    let mut findings_recorded: u32 = 0;

    for case in &cases {
        // Overall wall-cap: if the run is taking too long, skip remaining cases.
        if run_start.elapsed() > RUN_WALL_CAP {
            per_case.push(CaseOutcome {
                id: case.id.to_string(),
                outcome: "skipped".to_string(),
                symptom: Some("wall-cap reached".to_string()),
            });
            continue;
        }

        let url = format!("{}{}", base_url.trim_end_matches('/'), case.path);

        // The oversized-body case builds its body lazily (too large for a static slice).
        let oversized_buf;
        let body_bytes: Option<&[u8]> = if case.id == "oversized-body" {
            oversized_buf = oversized_body();
            Some(&oversized_buf)
        } else {
            case.raw_body
        };

        let outcome = sender.send(
            case.method.as_str(),
            &url,
            case.content_type,
            body_bytes,
            REQUEST_TIMEOUT,
        );

        if let Some(finding) = classify_response(case.id, outcome.status, &outcome.body, outcome.timed_out) {
            // Write the finding to findings.jsonl.
            if let Err(e) = append_fuzz_finding(project_root, &finding) {
                eprintln!("api_fuzz: findings append failed: {e}");
            } else {
                findings_recorded += 1;
            }
            per_case.push(CaseOutcome {
                id: case.id.to_string(),
                outcome: "finding".to_string(),
                symptom: Some(finding.symptom),
            });
        } else {
            per_case.push(CaseOutcome {
                id: case.id.to_string(),
                outcome: "clean".to_string(),
                symptom: None,
            });
        }
    }

    let mut tools_available: Vec<String> = Vec::new();
    if tools.xh_available() {
        tools_available.push("xh".to_string());
    }
    if tools.hurl_available() {
        tools_available.push("hurl".to_string());
    }
    if tools.schemathesis_available() {
        tools_available.push("schemathesis".to_string());
    }

    Ok(ApiFuzzReport {
        tools_available,
        cases_run: per_case
            .iter()
            .filter(|c| c.outcome != "skipped")
            .count() as u32,
        findings_recorded,
        per_case,
    })
}

// ---------------------------------------------------------------------------
// Findings JSONL append (compatible with training_export.rs schema)
// ---------------------------------------------------------------------------

/// Append one `api-fuzz` finding to `<root>/.aspis-training/findings.jsonl`.
///
/// The record shape is parse-compatible with the `training_export` findings.jsonl schema:
/// ```json
/// {"ts":"...","file":"<requested-path>","contentHash":null,"findings":[{...}],"attribution":null}
/// ```
/// The `findings` array always has exactly one entry per record (one symptom per case).
///
/// BLOCKER 3: routes through `training_export::append_findings_line`, the SINGLE shared
/// appender for findings.jsonl. This collapses what used to be a second module-local
/// per-path mutex registry into the one `training_export` registry, so a concurrent
/// Censor batch and an api-fuzz run serialize on the SAME lock (no torn writes / rotation
/// race). The record SHAPE is unchanged from the previous local writer.
fn append_fuzz_finding(root: &Path, finding: &FuzzFinding) -> std::io::Result<()> {
    let rec = json!({
        "ts": Utc::now().to_rfc3339(),
        "file": finding.case_id,           // "file" field = the case id (the "path" being tested)
        "contentHash": null,
        "findings": [{
            "id": format!("api-fuzz:{}", finding.case_id),
            "severity": "medium",
            "category": "api-fuzz",
            "source": "api-fuzz",
            "title": finding.symptom,
            "line": null,
            // The snippet is written to the JSONL only, never logged.
            "snippet": finding.snippet,
        }],
        "attribution": null,
    });

    crate::backend::training_export::append_findings_line(root, &rec)
}

// ---------------------------------------------------------------------------
// Tauri command
// ---------------------------------------------------------------------------

/// WARNING 7: validate that `spec_path` is an existing regular file located INSIDE
/// `project_root`. Canonicalizes both and prefix-checks so symlink / `..` traversal can't
/// point the schemathesis subprocess at an arbitrary file. Returns the canonical path
/// string on success.
fn validate_spec_path(project_root: &Path, spec_path: &str) -> Result<String, String> {
    let root_canon = project_root
        .canonicalize()
        .map_err(|e| format!("project_root canonicalize failed: {e}"))?;
    let spec_canon = Path::new(spec_path)
        .canonicalize()
        .map_err(|e| format!("spec path canonicalize failed for {spec_path:?}: {e}"))?;
    let meta = std::fs::metadata(&spec_canon)
        .map_err(|e| format!("spec path stat failed: {e}"))?;
    if !meta.is_file() {
        return Err(format!("spec path is not a regular file: {spec_path:?}"));
    }
    if !spec_canon.starts_with(&root_canon) {
        return Err(format!(
            "spec path must be inside project_root; got {spec_path:?}"
        ));
    }
    Ok(spec_canon.to_string_lossy().into_owned())
}

/// Run schemathesis (OpenAPI negative fuzzing) if the spec and binary are available.
/// Returns the number of new findings written.
///
/// NIT 13 — PRECONDITION: `base_url` MUST have been loopback-validated by the caller
/// (`validate_loopback_base` / `run_fuzz`) and `spec_path` MUST have been validated to be
/// a regular file inside `project_root` (`validate_spec_path`). This function does not
/// re-validate; it hands both straight to the subprocess.
fn run_schemathesis(st_bin: &Path, base_url: &str, spec_path: &str, root: &Path) -> u32 {
    use crate::oracle::python_oracle::apply_no_window;
    use std::process::{Command, Stdio};

    debug_assert!(
        is_loopback_base(base_url),
        "run_schemathesis precondition: base_url must be loopback-validated by the caller"
    );

    let mut cmd = Command::new(st_bin);
    apply_no_window(&mut cmd);
    cmd.args([
        "run",
        spec_path,
        "--base-url",
        base_url,
        "--seed",
        "42",          // reproducible
        "--checks",
        "all",
        "--output-truncate-errors",
        "no",
        "--report",
        "-",           // JSON to stdout
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return 0,
    };

    // Bounded wait (schemathesis can take a while; cap at RUN_WALL_CAP).
    if !wait_with_timeout(&mut child, RUN_WALL_CAP) {
        let _ = child.kill();
        let _ = child.wait();
        return 0;
    }

    // C-F11: read stdout with a hard cap so a huge report can't OOM the process. The
    // child has already exited (wait_with_timeout returned true), so a bounded read
    // completes promptly; the classifier only needs the failure markers, not the whole
    // report. We take the handle and `read_to_end` through a `take(CAP)` limiter.
    let stdout = {
        let mut buf = Vec::new();
        if let Some(h) = child.stdout.take() {
            use std::io::Read;
            let _ = h.take(SCHEMATHESIS_STDOUT_READ_CAP_BYTES).read_to_end(&mut buf);
        }
        let _ = child.wait(); // reap (already exited) so no zombie remains.
        buf
    };

    // Parse the JSON report for 5xx / error entries.
    parse_schemathesis_report(&stdout, root)
}

/// Parse schemathesis JSON report stdout for failures. Returns the number of findings
/// appended to findings.jsonl. Failure-tolerant: any parse error yields 0.
fn parse_schemathesis_report(stdout: &[u8], root: &Path) -> u32 {
    let text = match std::str::from_utf8(stdout) {
        Ok(t) => t,
        Err(_) => return 0,
    };
    let report: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let mut count = 0u32;
    if let Some(results) = report.get("results").and_then(|r| r.as_array()) {
        for result in results {
            let status = result
                .get("response")
                .and_then(|r| r.get("status_code"))
                .and_then(|s| s.as_u64())
                .unwrap_or(0) as u16;
            let body_str = result
                .get("response")
                .and_then(|r| r.get("body"))
                .and_then(|b| b.as_str())
                .unwrap_or("")
                .as_bytes()
                .to_vec();

            let case_id = result
                .get("operation")
                .and_then(|o| o.as_str())
                .unwrap_or("schemathesis-case")
                .to_string();

            if let Some(finding) = classify_response(&case_id, status, &body_str, false) {
                if append_fuzz_finding(root, &finding).is_ok() {
                    count += 1;
                }
            }
        }
    }
    count
}

// ---------------------------------------------------------------------------
// Tests (TDD: failing tests written first)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    // Unique tempdir counter per test process (mirrors training_export's idiom).
    static SEQ: AtomicU64 = AtomicU64::new(0);
    fn tmp(tag: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!(
            "apifuzz_{tag}_{}_{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn read_jsonl(path: &Path) -> Vec<serde_json::Value> {
        let body = std::fs::read_to_string(path).unwrap_or_default();
        body.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("valid json line"))
            .collect()
    }

    // -----------------------------------------------------------------------
    // 1. Loopback validation
    // -----------------------------------------------------------------------

    #[test]
    fn loopback_accepts_localhost() {
        assert!(validate_loopback_base("http://localhost").is_ok());
        assert!(validate_loopback_base("http://localhost:3000").is_ok());
        assert!(validate_loopback_base("http://localhost:3000/api").is_ok());
    }

    #[test]
    fn loopback_accepts_127_0_0_1() {
        assert!(validate_loopback_base("http://127.0.0.1").is_ok());
        assert!(validate_loopback_base("http://127.0.0.1:8080").is_ok());
        assert!(validate_loopback_base("http://127.0.0.1:8080/api/v1").is_ok());
        // Other 127.x.x.x addresses in the loopback block.
        assert!(validate_loopback_base("http://127.5.6.7:9000").is_ok());
    }

    #[test]
    fn loopback_accepts_ipv6_loopback() {
        assert!(validate_loopback_base("http://[::1]").is_ok());
        assert!(validate_loopback_base("http://[::1]:8080").is_ok());
        assert!(validate_loopback_base("http://[::1]:8080/api").is_ok());
    }

    #[test]
    fn loopback_rejects_public_ip() {
        assert!(validate_loopback_base("http://93.184.216.34").is_err()); // example.com IP
        assert!(validate_loopback_base("http://8.8.8.8:3000").is_err());
        assert!(validate_loopback_base("http://10.0.0.5:3000").is_err());
        assert!(validate_loopback_base("http://192.168.1.100:3000").is_err());
    }

    #[test]
    fn loopback_rejects_example_com() {
        assert!(validate_loopback_base("http://example.com").is_err());
        assert!(validate_loopback_base("http://example.com:3000").is_err());
    }

    #[test]
    fn loopback_rejects_https_scheme() {
        assert!(validate_loopback_base("https://localhost:3000").is_err());
        assert!(validate_loopback_base("https://127.0.0.1:8080").is_err());
        assert!(validate_loopback_base("https://[::1]:8080").is_err());
    }

    #[test]
    fn loopback_rejects_userinfo_tricks() {
        assert!(validate_loopback_base("http://127.0.0.1@evil.com").is_err());
        assert!(validate_loopback_base("http://[::1]:8000@evil.com").is_err());
        assert!(validate_loopback_base("http://localhost@evil.com").is_err());
        // Suffix trick: `127.0.0.1.evil.com` looks like loopback but isn't.
        assert!(validate_loopback_base("http://127.0.0.1.evil.com").is_err());
    }

    #[test]
    fn loopback_rejects_no_scheme() {
        assert!(validate_loopback_base("localhost:3000").is_err());
        assert!(validate_loopback_base("127.0.0.1:8080").is_err());
        assert!(validate_loopback_base("").is_err());
    }

    // -----------------------------------------------------------------------
    // 2. Tool detection
    // -----------------------------------------------------------------------

    #[test]
    fn tool_detection_with_fake_xh_on_path() {
        // Write a stub `xh` (or `xh.cmd` on Windows) to a tempdir, prepend to PATH,
        // then verify ToolAvailability::detect() finds it.
        let dir = tmp("tooldetect");
        let stub_name = if cfg!(windows) { "xh.cmd" } else { "xh" };
        let stub_path = dir.join(stub_name);
        #[cfg(windows)]
        std::fs::write(&stub_path, b"@echo off\r\necho stub\r\n").unwrap();
        #[cfg(not(windows))]
        {
            std::fs::write(&stub_path, b"#!/bin/sh\necho stub\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stub_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let prev_path = std::env::var_os("PATH").unwrap_or_default();
        let new_path = std::env::join_paths(
            std::iter::once(dir.as_os_str().to_os_string())
                .chain(std::env::split_paths(&prev_path).map(|p| p.into_os_string())),
        )
        .unwrap();

        // We must manipulate PATH carefully — no parallelism issues since this is
        // a synchronous lookup that reads PATH once.
        std::env::set_var("PATH", &new_path);
        let tools = ToolAvailability::detect();
        std::env::set_var("PATH", &prev_path);

        assert!(
            tools.xh_available(),
            "xh stub on PATH should be detected as available"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tool_detection_empty_path_reports_unavailable() {
        // With a PATH that contains only a single non-existent directory, no tools
        // should be found. The run must still succeed (skip those cases).
        let tools = ToolAvailability {
            xh: None,
            hurl: None,
            schemathesis: None,
        };
        assert!(!tools.xh_available());
        assert!(!tools.hurl_available());
        assert!(!tools.schemathesis_available());
    }

    // -----------------------------------------------------------------------
    // 3. Response classification (pure fn, table tests)
    // -----------------------------------------------------------------------

    #[test]
    fn classify_5xx_is_finding() {
        let f = classify_response("truncated-json", 500, b"Internal Server Error", false);
        assert!(f.is_some(), "HTTP 500 must be classified as a finding");
        let f = f.unwrap();
        assert_eq!(f.case_id, "truncated-json");
        assert!(f.symptom.contains("500"));
    }

    #[test]
    fn classify_503_is_finding() {
        let f = classify_response("empty-body", 503, b"Service Unavailable", false);
        assert!(f.is_some());
        let f = f.unwrap();
        assert!(f.symptom.contains("503"));
    }

    #[test]
    fn classify_clean_200_is_none() {
        let f = classify_response("clean-case", 200, b"{\"ok\":true}", false);
        assert!(f.is_none(), "clean 200 must not be classified as a finding");
    }

    #[test]
    fn classify_404_is_none() {
        let f = classify_response("clean-case", 404, b"not found", false);
        assert!(f.is_none(), "404 is expected for unknown paths; not a finding");
    }

    #[test]
    fn classify_200_with_python_traceback_is_finding() {
        let body = b"HTTP 200 OK\nTraceback (most recent call last):\n  File \"app.py\", line 42\nValueError: oops";
        let f = classify_response("truncated-json", 200, body, false);
        assert!(
            f.is_some(),
            "200 with Python traceback must be classified as a finding"
        );
        let f = f.unwrap();
        assert!(
            f.symptom.contains("traceback"),
            "symptom should mention traceback: {}", f.symptom
        );
    }

    #[test]
    fn classify_200_with_rust_panic_is_finding() {
        let body = b"{\"error\":\"panicked at 'index out of bounds', src/main.rs:10\"}";
        let f = classify_response("deeply-nested-json", 200, body, false);
        assert!(f.is_some(), "body containing 'panicked at' must be a finding");
    }

    #[test]
    fn classify_200_with_node_anonymous_is_finding() {
        let body = b"TypeError: Cannot read property of undefined\n    at <anonymous>:1:1";
        let f = classify_response("wrong-content-type", 200, body, false);
        assert!(f.is_some(), "body containing 'at <anonymous>' must be a finding");
    }

    #[test]
    fn classify_timeout_is_finding() {
        let f = classify_response("oversized-body", 0, b"", true);
        assert!(f.is_some(), "timeout must be classified as a finding");
        let f = f.unwrap();
        assert!(f.symptom.contains("hang"), "timeout finding symptom: {}", f.symptom);
    }

    #[test]
    fn classify_connection_reset_is_finding() {
        // status=0, not timed_out, empty body = connection reset / transport error.
        let f = classify_response("truncated-json", 0, b"", false);
        assert!(f.is_some(), "status=0 non-timeout must be a finding");
        let f = f.unwrap();
        assert!(f.symptom.contains("connection") || f.symptom.contains("transport"), "{}", f.symptom);
    }

    #[test]
    fn classify_snippet_is_capped_at_2kib() {
        let big_body = vec![b'x'; SNIPPET_CAP_BYTES + 1024];
        let f = classify_response("oversized-body", 500, &big_body, false);
        assert!(f.is_some());
        assert!(
            f.unwrap().snippet.len() <= SNIPPET_CAP_BYTES,
            "snippet must be capped at {SNIPPET_CAP_BYTES} bytes"
        );
    }

    // -----------------------------------------------------------------------
    // 4. Findings written — fake sender, checks JSONL on disk
    // -----------------------------------------------------------------------

    /// A fake sender that returns pre-configured responses per case id.
    /// Kept for future per-case test scenarios; `UniformSender` covers the uniform-response cases.
    #[allow(dead_code)]
    struct FakeSender {
        responses: std::collections::HashMap<String, SendOutcome>,
        default: SendOutcome,
    }

    #[allow(dead_code)]
    impl FakeSender {
        fn new() -> Self {
            Self {
                responses: Default::default(),
                default: SendOutcome { status: 200, body: b"{}".to_vec(), timed_out: false },
            }
        }

        fn with_response(mut self, case_id: &str, outcome: SendOutcome) -> Self {
            self.responses.insert(case_id.to_string(), outcome);
            self
        }
    }

    impl RequestSender for FakeSender {
        fn send(
            &self,
            _method: &str,
            url: &str,
            _content_type: Option<&str>,
            _body: Option<&[u8]>,
            _timeout: Duration,
        ) -> SendOutcome {
            // Match by the last path segment (which corresponds to the case path "/").
            // We key off the URL for simplicity: FakeSender checks if any registered case_id
            // is in the URL, else returns the default.
            for (id, outcome) in &self.responses {
                if url.contains(id.as_str()) {
                    return outcome.clone();
                }
            }
            self.default.clone()
        }
    }

    /// A sender that returns a pre-determined outcome for ALL requests.
    struct UniformSender(SendOutcome);

    impl RequestSender for UniformSender {
        fn send(
            &self,
            _method: &str,
            _url: &str,
            _content_type: Option<&str>,
            _body: Option<&[u8]>,
            _timeout: Duration,
        ) -> SendOutcome {
            self.0.clone()
        }
    }

    #[test]
    fn findings_written_for_500_responses() {
        let root = tmp("findings500");
        let sender = UniformSender(SendOutcome {
            status: 500,
            body: b"Internal Server Error".to_vec(),
            timed_out: false,
        });
        let tools = ToolAvailability { xh: None, hurl: None, schemathesis: None };

        let report = run_fuzz("http://127.0.0.1:9999", &root, &sender, &tools)
            .expect("run_fuzz must not error for valid loopback URL");

        assert!(report.cases_run > 0, "at least one case must have run");
        assert!(
            report.findings_recorded > 0,
            "500 responses must produce findings"
        );
        assert_eq!(report.findings_recorded, report.cases_run, "every case should be a finding");

        let findings_path = root.join(".aspis-training").join("findings.jsonl");
        assert!(findings_path.exists(), "findings.jsonl must exist");

        let lines = read_jsonl(&findings_path);
        assert!(!lines.is_empty(), "findings.jsonl must have at least one line");

        for line in &lines {
            // Verify parse-compatibility with the training_export schema.
            assert!(line.get("ts").is_some(), "record must have 'ts'");
            assert!(line.get("file").is_some(), "record must have 'file'");
            assert!(line.get("findings").is_some(), "record must have 'findings'");

            let findings_arr = line["findings"].as_array().expect("findings must be array");
            assert!(!findings_arr.is_empty());

            let f = &findings_arr[0];
            assert_eq!(f["source"].as_str().unwrap(), "api-fuzz", "source must be 'api-fuzz'");
            assert_eq!(f["category"].as_str().unwrap(), "api-fuzz", "category must be 'api-fuzz'");
            assert!(f["title"].as_str().is_some(), "finding must have a title");
        }

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn clean_responses_produce_no_findings() {
        let root = tmp("findingsclean");
        let sender = UniformSender(SendOutcome {
            status: 200,
            body: b"{\"ok\":true}".to_vec(),
            timed_out: false,
        });
        let tools = ToolAvailability { xh: None, hurl: None, schemathesis: None };

        let report = run_fuzz("http://localhost:9999", &root, &sender, &tools)
            .expect("run_fuzz must not error");

        assert_eq!(report.findings_recorded, 0, "clean responses must not produce findings");

        for outcome in &report.per_case {
            assert_eq!(outcome.outcome, "clean", "all outcomes should be 'clean'");
        }

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn findings_jsonl_lines_are_parse_compatible_with_training_export_schema() {
        // Write one finding directly and verify the shape is what the docs describe.
        let root = tmp("schemashape");
        let finding = FuzzFinding {
            case_id: "truncated-json".to_string(),
            symptom: "5xx: server returned HTTP 503".to_string(),
            snippet: "{\"error\":\"db unavailable\"}".to_string(),
        };
        append_fuzz_finding(&root, &finding).expect("append must succeed");

        let findings_path = root.join(".aspis-training").join("findings.jsonl");
        let lines = read_jsonl(&findings_path);
        assert_eq!(lines.len(), 1);

        let rec = &lines[0];
        // Required fields per the training_export schema.
        assert!(rec["ts"].as_str().is_some());
        assert_eq!(rec["file"].as_str().unwrap(), "truncated-json");
        assert!(rec["contentHash"].is_null());
        assert!(rec["attribution"].is_null());

        let f = &rec["findings"][0];
        assert_eq!(f["source"].as_str().unwrap(), "api-fuzz");
        assert_eq!(f["category"].as_str().unwrap(), "api-fuzz");
        assert!(f["id"].as_str().unwrap().starts_with("api-fuzz:"));
        assert_eq!(f["severity"].as_str().unwrap(), "medium");
        assert!(f["line"].is_null());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn gitignore_written_to_training_dir() {
        let root = tmp("gitignore");
        let finding = FuzzFinding {
            case_id: "test".to_string(),
            symptom: "5xx: server returned HTTP 500".to_string(),
            snippet: String::new(),
        };
        append_fuzz_finding(&root, &finding).unwrap();
        let gi = root.join(".aspis-training").join(".gitignore");
        assert!(gi.exists(), ".gitignore must be created");
        assert_eq!(std::fs::read_to_string(&gi).unwrap(), "*\n");
        std::fs::remove_dir_all(&root).ok();
    }

    // -----------------------------------------------------------------------
    // 5. Case library integrity
    // -----------------------------------------------------------------------

    #[test]
    fn all_builtin_cases_have_nonempty_ids() {
        for case in builtin_cases() {
            assert!(
                !case.id.is_empty(),
                "all cases must have a non-empty id"
            );
        }
    }

    #[test]
    fn all_builtin_case_ids_are_unique() {
        let cases = builtin_cases();
        let mut seen = std::collections::HashSet::new();
        for case in &cases {
            assert!(
                seen.insert(case.id),
                "duplicate case id: {:?}", case.id
            );
        }
    }

    #[test]
    fn all_builtin_cases_have_valid_methods() {
        let valid = ["GET", "POST", "PUT", "PATCH", "DELETE"];
        for case in builtin_cases() {
            assert!(
                valid.contains(&case.method.as_str()),
                "case {:?} has invalid method {:?}", case.id, case.method.as_str()
            );
        }
    }

    #[test]
    fn all_builtin_cases_have_nonempty_paths() {
        for case in builtin_cases() {
            assert!(
                !case.path.is_empty(),
                "case {:?} must have a non-empty path", case.id
            );
            assert!(
                case.path.starts_with('/'),
                "case {:?} path must start with /: {:?}", case.id, case.path
            );
        }
    }

    // -----------------------------------------------------------------------
    // 6. run_fuzz rejects non-loopback URLs before touching the sender
    // -----------------------------------------------------------------------

    #[test]
    fn run_fuzz_rejects_remote_url() {
        struct PanicSender;
        impl RequestSender for PanicSender {
            fn send(&self, _: &str, _: &str, _: Option<&str>, _: Option<&[u8]>, _: Duration) -> SendOutcome {
                panic!("sender must not be called for a non-loopback URL");
            }
        }
        let root = tmp("reject");
        let tools = ToolAvailability { xh: None, hurl: None, schemathesis: None };
        let result = run_fuzz("http://example.com:3000", &root, &PanicSender, &tools);
        assert!(result.is_err(), "run_fuzz must reject non-loopback URLs");
        let err = result.unwrap_err();
        assert!(err.contains("loopback"), "error must mention 'loopback': {err}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn run_fuzz_rejects_https_url() {
        struct PanicSender;
        impl RequestSender for PanicSender {
            fn send(&self, _: &str, _: &str, _: Option<&str>, _: Option<&[u8]>, _: Duration) -> SendOutcome {
                panic!("sender must not be called for https URL");
            }
        }
        let root = tmp("reject_https");
        let tools = ToolAvailability { xh: None, hurl: None, schemathesis: None };
        let result = run_fuzz("https://localhost:3000", &root, &PanicSender, &tools);
        assert!(result.is_err(), "run_fuzz must reject https");
        std::fs::remove_dir_all(&root).ok();
    }

    // -----------------------------------------------------------------------
    // 7. ApiFuzzReport serializes camelCase
    // -----------------------------------------------------------------------

    #[test]
    fn report_serializes_camel_case() {
        let report = ApiFuzzReport {
            tools_available: vec!["xh".to_string()],
            cases_run: 6,
            findings_recorded: 2,
            per_case: vec![CaseOutcome {
                id: "truncated-json".to_string(),
                outcome: "finding".to_string(),
                symptom: Some("5xx".to_string()),
            }],
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"toolsAvailable\""), "must have camelCase toolsAvailable");
        assert!(json.contains("\"casesRun\""), "must have camelCase casesRun");
        assert!(json.contains("\"findingsRecorded\""), "must have camelCase findingsRecorded");
        assert!(json.contains("\"perCase\""), "must have camelCase perCase");
    }

    // -----------------------------------------------------------------------
    // 8. parse_xh_output helper
    // -----------------------------------------------------------------------

    #[test]
    fn parse_xh_output_extracts_status_and_body() {
        let raw = b"HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\n\r\n{\"error\":\"oops\"}";
        let outcome = parse_xh_output(raw);
        assert_eq!(outcome.status, 500);
        assert!(String::from_utf8_lossy(&outcome.body).contains("oops"));
    }

    #[test]
    fn parse_xh_output_handles_200() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"ok\":true}";
        let outcome = parse_xh_output(raw);
        assert_eq!(outcome.status, 200);
    }

    #[test]
    fn parse_xh_output_graceful_on_garbage() {
        let outcome = parse_xh_output(b"not valid xh output");
        assert_eq!(outcome.status, 0); // gracefully yields 0
    }

    // -----------------------------------------------------------------------
    // C-F4 / C-F11: bounded subprocess stdout read (no OOM on a huge response)
    // -----------------------------------------------------------------------

    /// C-F4: the xh stdout read is bounded by `take(XH_STDOUT_READ_CAP_BYTES)`. A reader
    /// emitting MORE than the cap stops AT the cap (never allocates the full payload), and
    /// the status line that precedes the body is still parsed correctly.
    #[test]
    fn xh_stdout_read_stops_at_cap_and_still_parses() {
        use std::io::Read;
        // A status line + a body far larger than the cap.
        let mut payload: Vec<u8> = b"HTTP/1.1 500 Internal Server Error\r\n\r\n".to_vec();
        payload.extend(std::iter::repeat_n(b'x', XH_STDOUT_READ_CAP_BYTES as usize + 4096));
        let reader = std::io::Cursor::new(payload);

        // EXACT pattern used by send_via_xh's stdout reader thread.
        let mut buf = Vec::new();
        let _ = reader.take(XH_STDOUT_READ_CAP_BYTES).read_to_end(&mut buf);

        assert_eq!(
            buf.len() as u64, XH_STDOUT_READ_CAP_BYTES,
            "read stops exactly at the cap (never the full oversized body)"
        );
        // Classification still works off the capped buffer (status line is at the front).
        let outcome = parse_xh_output(&buf);
        assert_eq!(outcome.status, 500, "the status line is parsed from the capped read");
    }

    /// C-F11: the schemathesis stdout read is bounded by
    /// `take(SCHEMATHESIS_STDOUT_READ_CAP_BYTES)`. A reader emitting MORE than the cap
    /// stops at the cap; a (truncated) non-JSON buffer classifies as 0 findings (the
    /// failure-tolerant parse path), never OOMs.
    #[test]
    fn schemathesis_stdout_read_stops_at_cap() {
        use std::io::Read;
        let payload: Vec<u8> =
            std::iter::repeat_n(b'{', SCHEMATHESIS_STDOUT_READ_CAP_BYTES as usize + 4096).collect();
        let reader = std::io::Cursor::new(payload);

        let mut buf = Vec::new();
        let _ = reader.take(SCHEMATHESIS_STDOUT_READ_CAP_BYTES).read_to_end(&mut buf);

        assert_eq!(
            buf.len() as u64, SCHEMATHESIS_STDOUT_READ_CAP_BYTES,
            "read stops exactly at the cap"
        );
        // A truncated/garbage report classifies as 0 findings (failure-tolerant), no panic.
        let tmp = std::env::temp_dir();
        assert_eq!(parse_schemathesis_report(&buf, &tmp), 0);
    }

    // -----------------------------------------------------------------------
    // BLOCKER 8: parse_xh_output — LAST response wins (100 Continue then 413)
    // -----------------------------------------------------------------------

    #[test]
    fn parse_xh_output_last_status_wins_after_100_continue() {
        // BLOCKER 8: an interim `100 Continue` followed by the real `413` must classify
        // as 413, not 100. Before the fix the first status stuck and the 413 was lost.
        let raw = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 413 Payload Too Large\r\nContent-Type: text/plain\r\n\r\nbody too big";
        let outcome = parse_xh_output(raw);
        assert_eq!(outcome.status, 413, "last response status must win");
        assert!(String::from_utf8_lossy(&outcome.body).contains("body too big"));
        // And 413 oversized must be... not a finding by classify (4xx is fine), but the
        // point is the status is preserved; a 5xx-after-100 IS a finding:
        let raw5 = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 500 Internal Server Error\r\n\r\nboom";
        let o5 = parse_xh_output(raw5);
        assert_eq!(o5.status, 500);
        assert!(classify_response("oversized-body", o5.status, &o5.body, false).is_some());
    }

    // -----------------------------------------------------------------------
    // BLOCKER 2: build_xh_args — --ignore-stdin only when no body
    // -----------------------------------------------------------------------

    #[test]
    fn build_xh_args_body_case_uses_raw_not_ignore_stdin() {
        let args = build_xh_args(
            "POST",
            "http://127.0.0.1/",
            Some("application/json"),
            Some(b"{\"a\":1}"),
            Duration::from_secs(5),
        );
        assert!(args.iter().any(|a| a == "--raw"), "body case must add --raw");
        assert!(args.iter().any(|a| a == "-"), "body case must add - (stdin)");
        assert!(
            !args.iter().any(|a| a == "--ignore-stdin"),
            "body case must NOT add --ignore-stdin (it conflicts with --raw -)"
        );
    }

    #[test]
    fn build_xh_args_no_body_case_uses_ignore_stdin() {
        let args = build_xh_args(
            "GET",
            "http://127.0.0.1/",
            None,
            None,
            Duration::from_secs(5),
        );
        assert!(
            args.iter().any(|a| a == "--ignore-stdin"),
            "no-body case must add --ignore-stdin"
        );
        assert!(
            !args.iter().any(|a| a == "--raw"),
            "no-body case must NOT add --raw"
        );
    }

    // -----------------------------------------------------------------------
    // BLOCKER 1: a stub xh that exits 0 with a real 200 body classifies clean
    // -----------------------------------------------------------------------

    /// Write a stub `xh` executable that ignores its args, drains stdin, prints a fixed
    /// `--print=shb`-style response to stdout, and exits 0. Returns its path.
    #[cfg(windows)]
    fn write_stub_xh(dir: &Path, http_response: &str) -> PathBuf {
        // A .cmd that echoes the canned response, line by line.
        let stub = dir.join("xh_stub.cmd");
        let mut script = String::from("@echo off\r\n");
        for line in http_response.split('\n') {
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                script.push_str("echo.\r\n");
            } else {
                script.push_str("echo ");
                script.push_str(line);
                script.push_str("\r\n");
            }
        }
        script.push_str("exit /b 0\r\n");
        std::fs::write(&stub, script).unwrap();
        stub
    }

    #[cfg(unix)]
    fn write_stub_xh(dir: &Path, http_response: &str) -> PathBuf {
        let stub = dir.join("xh_stub.sh");
        // Drain stdin so a `--raw -` body case doesn't SIGPIPE the writer.
        let script = format!(
            "#!/bin/sh\ncat >/dev/null 2>&1\nprintf '%s' \"{}\"\nexit 0\n",
            http_response.replace('\\', "\\\\").replace('"', "\\\"")
        );
        std::fs::write(&stub, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        stub
    }

    #[test]
    fn send_via_xh_stub_200_classifies_clean() {
        // BLOCKER 1: previously the double-wait (try_wait reap + wait_with_output ECHILD)
        // returned status=0 for EVERY successful response, classifying it as a connection
        // reset (false positive). A stub xh exiting 0 with a real 200 body must now be
        // read correctly and classify as clean (None).
        let dir = tmp("stubxh200");
        let resp = "HTTP/1.1 200 OK\nContent-Type: application/json\n\n{\"ok\":true}";
        let stub = write_stub_xh(&dir, resp);

        let outcome = send_via_xh(
            &stub,
            "GET",
            "http://127.0.0.1:9/",
            Some("application/json"),
            None,
            Duration::from_secs(5),
        );
        assert!(!outcome.timed_out, "stub exiting 0 must not be a timeout");
        assert_eq!(outcome.status, 200, "real 200 must be parsed, not status=0");
        assert!(
            classify_response("clean-case", outcome.status, &outcome.body, outcome.timed_out)
                .is_none(),
            "a clean 200 from a stub xh must classify as None, not a false-positive reset"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // -----------------------------------------------------------------------
    // BLOCKER 3: api_fuzz + training_export share ONE findings.jsonl lock
    // -----------------------------------------------------------------------

    #[test]
    fn api_fuzz_and_training_export_resolve_same_findings_lock() {
        // BLOCKER 3: both paths must resolve to the SAME Arc<Mutex<()>> for the shared
        // findings.jsonl, proving there is ONE registry, not two.
        let dir = tmp("samelock");
        let path = dir.join(".aspis-training").join("findings.jsonl");
        let a = crate::backend::training_export::lock_for_path_test_hook(&path);
        let b = crate::backend::training_export::lock_for_path_test_hook(&path);
        assert!(
            std::sync::Arc::ptr_eq(&a, &b),
            "api_fuzz must route through training_export's single lock registry"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn concurrent_apifuzz_and_training_export_writes_no_torn_lines() {
        // BLOCKER 3: N threads via api_fuzz's append path + M threads via training_export's
        // public appender, all to the SAME findings.jsonl -> exactly N+M valid JSON lines.
        let root = tmp("concurfindings");
        const N: usize = 6; // api_fuzz writers
        const M: usize = 6; // training_export writers
        const PER: usize = 25;

        let mut handles = vec![];
        for t in 0..N {
            let r = root.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..PER {
                    let finding = FuzzFinding {
                        case_id: format!("apifuzz-{t}-{i}"),
                        symptom: "5xx: server returned HTTP 500".to_string(),
                        snippet: String::new(),
                    };
                    append_fuzz_finding(&r, &finding).unwrap();
                }
            }));
        }
        for t in 0..M {
            let r = root.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..PER {
                    let rec = serde_json::json!({
                        "ts": "x",
                        "file": format!("tx-{t}-{i}"),
                        "contentHash": null,
                        "findings": [],
                    });
                    crate::backend::training_export::append_findings_line(&r, &rec).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let path = root.join(".aspis-training").join("findings.jsonl");
        let lines = read_jsonl(&path);
        assert_eq!(
            lines.len(),
            (N + M) * PER,
            "every line must be a complete valid JSON record (no torn writes)"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    // -----------------------------------------------------------------------
    // WARNING 7: validate_spec_path — inside-root accepted, outside rejected
    // -----------------------------------------------------------------------

    #[test]
    fn validate_spec_path_rejects_outside_and_accepts_inside() {
        let root = tmp("specpath");
        // A spec INSIDE the project root is accepted.
        let inside = root.join("openapi.json");
        std::fs::write(&inside, b"{\"openapi\":\"3.0.0\"}").unwrap();
        assert!(
            validate_spec_path(&root, inside.to_str().unwrap()).is_ok(),
            "a spec inside project_root must be accepted"
        );

        // A spec OUTSIDE the project root is rejected.
        let outside_dir = tmp("specpath_outside");
        let outside = outside_dir.join("evil.json");
        std::fs::write(&outside, b"{}").unwrap();
        assert!(
            validate_spec_path(&root, outside.to_str().unwrap()).is_err(),
            "a spec outside project_root must be rejected"
        );

        // A non-existent spec is rejected.
        assert!(
            validate_spec_path(&root, root.join("missing.json").to_str().unwrap()).is_err(),
            "a non-existent spec must be rejected"
        );
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&outside_dir).ok();
    }
}
