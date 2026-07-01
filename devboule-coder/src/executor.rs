//! The REAL tool executor (L2.3): dispatches a validated [`AgentAction`] to one
//! of THREE backends behind the [`crate::agent_loop::ToolExecutor`] seam.
//!
//! * **MCP backend** ([`McpBackend`]) — the private, grounded Oracle and the
//!   write-delegating `spawn_mini`. `oracle_ask` / `oracle_context` /
//!   `spawn_mini` map to MCP `call_tool` calls whose tool names + params match
//!   the Python server contract (see [`RealExecutor::execute`]). `plan` is
//!   recorded LOCALLY as a milestone (no server tool). The orchestrator NEVER
//!   writes files itself — every write goes through `spawn_mini` (the WRITE arm).
//! * **FS backend** ([`FsBackend`]) — `read` / `grep` / `glob` run IN-PROCESS,
//!   READ-ONLY, and ROOT-CONFINED. No server round-trip: local navigation is a
//!   local concern. Every path is canonicalized and re-checked to be inside the
//!   project root (defense in depth on top of [`crate::action`]'s parse-time
//!   `..`/abs rejection), symlinks are never followed out of root, and output is
//!   bounded.
//! * **Exa backend** ([`ExaBackend`]) — `fetch` / `websearch` are the EGRESS
//!   exception: they reach the public web via Exa. The executor only holds an
//!   [`ExaBackend`] when an API key is configured, so `web: Option<…>` and the
//!   burst's `allow_egress` gate is `web.is_some()`. The structural egress gate
//!   in [`run_burst`](crate::agent_loop::run_burst) is still authoritative — a
//!   disabled egress action never even reaches this executor.
//!
//! The real MCP transport (rmcp child process) lives in
//! [`crate::rmcp_backend`], isolated so this module is fully UNIT-TESTABLE
//! against [`MockMcpBackend`] with no live server, no GPU, and no network.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::json;

use crate::action::AgentAction;
use crate::agent_loop::{ToolExecutor, ToolResult, MAX_RESULT_LEN};
use crate::model::CoderModel;
use crate::planner::run_planner;

// --- FS backend caps ---------------------------------------------------------
// Bound the in-process walk so a pathological tree (a huge monorepo, a deeply
// nested glob) cannot stall the burst or blow the transcript. The loop re-caps
// the final output at MAX_RESULT_LEN regardless; these bound the WORK, not just
// the text.

/// Max number of files `grep` will scan in one call before stopping early.
const GREP_MAX_FILES: usize = 5_000;
/// Max number of `file:line` matches `grep` returns.
const GREP_MAX_MATCHES: usize = 500;
/// Max number of paths `glob` returns.
const GLOB_MAX_RESULTS: usize = 1_000;
/// Max bytes of any single file `read` / `grep` will pull into memory. A file
/// larger than this is read up to the cap and marked truncated, so a 2 GB blob
/// can never be slurped whole.
const FILE_READ_CAP: usize = MAX_RESULT_LEN;

/// Max bytes of an HTTP response body the executor will buffer before stopping.
/// A few result-lengths is plenty (the parsers re-cap to [`MAX_RESULT_LEN`]); the
/// point is to never hold a gigabyte in RAM when an endpoint returns one. Shared
/// by the Exa backend and the loopback model client (via [`read_body_capped`]).
pub(crate) const HTTP_BODY_CAP: usize = 4 * MAX_RESULT_LEN;

// =============================================================================
// MCP backend seam
// =============================================================================

/// The MCP transport seam: a single `call_tool(name, params)` the executor uses
/// for every server-backed action. Isolating it here makes [`RealExecutor`]
/// unit-testable against [`MockMcpBackend`] (which asserts the right tool name +
/// params per action) with NO live Python server. The real rmcp-over-stdio impl
/// is [`crate::rmcp_backend::RmcpBackend`].
///
/// `params` is the action-specific argument object; the impl injects the
/// session identity (`role` / `agent_id` / `session_token`) — see the contract
/// note on [`RealExecutor::execute`]. Returns the tool's text result, or an
/// error string suitable to feed back to the model as a failed [`ToolResult`].
#[async_trait]
pub trait McpBackend: Send + Sync {
    async fn call_tool(&self, name: &str, params: serde_json::Value) -> Result<String, String>;

    /// Route an [`AgentAction::McpTool`] to the named USER MCP server (Phase B). The
    /// Oracle-only backend ([`crate::rmcp_backend::RmcpBackend`]) uses the DEFAULT,
    /// which errors (there is no user-server surface on a plain Oracle connection);
    /// [`crate::multi_mcp::MultiMcpBackend`] overrides it to dispatch to the named
    /// backend, returning a recoverable `Err` for an unknown server (never a panic —
    /// the burst recovers). Kept as a SEPARATE method (not folded into `call_tool`)
    /// so Oracle dispatch — fixed tool names through `call_tool` — is unchanged and a
    /// user server can never be reached except through this explicit user path.
    async fn call_user_tool(
        &self,
        server: &str,
        _tool: &str,
        _params: serde_json::Value,
    ) -> Result<String, String> {
        Err(format!(
            "no user MCP servers are configured (cannot call server `{server}`)"
        ))
    }

    /// The CONFIGURED user-MCP server names this backend can route to. Empty for the
    /// Oracle-only backend; [`crate::multi_mcp::MultiMcpBackend`] returns its user
    /// server names. Surfaced up through [`RealExecutor`] so the burst can validate an
    /// `mcp_tool` server name at parse time.
    fn user_server_names(&self) -> &[String] {
        &[]
    }
}

// =============================================================================
// FS backend — in-process, root-confined, read-only
// =============================================================================

/// Local filesystem navigation, ROOT-CONFINED and READ-ONLY. Holds the canonical
/// project root; every operation resolves against it and rejects anything that
/// escapes it post-canonicalization (the symlink / `..` defense the parse-time
/// check cannot give, because it runs before the path touches the disk).
#[derive(Debug, Clone)]
pub struct FsBackend {
    /// The canonical project root. All reads are confined here.
    root: PathBuf,
}

impl FsBackend {
    /// Build from a project root, canonicalizing it once. An un-canonicalizable
    /// root (does not exist) is an error: we refuse to run a confined backend
    /// whose confinement boundary is undefined.
    pub fn new(project_root: impl AsRef<Path>) -> Result<Self, String> {
        let root = project_root
            .as_ref()
            .canonicalize()
            .map_err(|e| format!("project root is not accessible: {e}"))?;
        Ok(Self { root })
    }

    /// Resolve a model-supplied RELATIVE path against the root and confirm the
    /// CANONICAL result stays inside the root. Belt-and-suspenders over the
    /// parse-time validation: a path can pass the static `..`/abs check yet still
    /// resolve outside the root via a symlink component, so we canonicalize and
    /// re-check the prefix here. Returns the confined absolute path.
    fn resolve(&self, rel: &str) -> Result<PathBuf, String> {
        // The parse layer already rejected absolute / `..` / drive-prefixed
        // paths, but we never trust a single layer for a confinement boundary.
        let joined = self.root.join(rel);
        let canonical = joined
            .canonicalize()
            .map_err(|e| format!("path not found or inaccessible: {e}"))?;
        if !canonical.starts_with(&self.root) {
            return Err("path escapes the project root".to_string());
        }
        Ok(canonical)
    }

    /// Read a file by relative path, confined to the root, capped at
    /// [`FILE_READ_CAP`] bytes. A directory, a missing file, or an escape is an
    /// error string (fed back as a failed [`ToolResult`]).
    pub fn read(&self, rel: &str) -> Result<String, String> {
        // DEFERRED TOCTOU: `resolve` canonicalizes-and-checks, then we re-open by
        // path below — a racing symlink swap between the two could redirect the
        // open out of root. Residual + low risk in the single-user local L2
        // deployment; the proper fix (open `O_NOFOLLOW`/`openat` via `rustix`) is
        // out of scope for L2 and tracked separately.
        let path = self.resolve(rel)?;
        let meta =
            std::fs::symlink_metadata(&path).map_err(|e| format!("cannot stat path: {e}"))?;
        if meta.is_dir() {
            return Err(format!("`{rel}` is a directory, not a file"));
        }
        // Read at most FILE_READ_CAP+1 bytes so we can detect (and mark) an
        // over-cap file without slurping a giant blob into memory.
        let bytes =
            read_capped(&path, FILE_READ_CAP + 1).map_err(|e| format!("read failed: {e}"))?;
        let truncated = bytes.len() > FILE_READ_CAP;
        let slice = if truncated {
            &bytes[..FILE_READ_CAP]
        } else {
            &bytes[..]
        };
        let mut text = String::from_utf8_lossy(slice).into_owned();
        if truncated {
            text.push_str("\n[…file truncated at read cap]");
        }
        Ok(text)
    }

    /// Grep `pattern` (a `regex`-crate NFA, ReDoS-free) across the root, walking
    /// with the `ignore` crate so `.git` / `target` / `node_modules` / dotdirs /
    /// gitignored paths are skipped for free. `glob` optionally restricts which
    /// files are searched. Returns `file:line: text` matches, bounded by
    /// [`GREP_MAX_FILES`] / [`GREP_MAX_MATCHES`].
    pub fn grep(&self, pattern: &str, glob: Option<&str>) -> Result<String, String> {
        // Compile here too (the parse layer already validated it compiles); the
        // NFA engine guarantees linear-time matching regardless of input.
        let re = regex::Regex::new(pattern).map_err(|e| format!("invalid regex: {e}"))?;
        let matcher = glob.map(build_globset).transpose()?;

        let mut out = String::new();
        let mut files_scanned = 0usize;
        let mut matches = 0usize;

        'walk: for entry in self.walker() {
            if files_scanned >= GREP_MAX_FILES || matches >= GREP_MAX_MATCHES {
                break;
            }
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue, // unreadable entry: skip, never abort the whole grep
            };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let rel = match entry.path().strip_prefix(&self.root) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if let Some(set) = &matcher {
                if !set.is_match(rel) {
                    continue;
                }
            }
            files_scanned += 1;

            // Read each candidate capped; binary / huge files are matched on
            // their capped head only, never slurped whole.
            let bytes = match read_capped(entry.path(), FILE_READ_CAP) {
                Ok(b) => b,
                Err(_) => continue,
            };
            // Skip apparent binary content (a NUL in the head) rather than emit
            // garbage lines.
            if bytes.contains(&0) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            for (lineno, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    let shown = elide_line(line);
                    out.push_str(&format!("{}:{}: {}\n", rel.display(), lineno + 1, shown));
                    matches += 1;
                    if matches >= GREP_MAX_MATCHES {
                        out.push_str("[…match cap reached]\n");
                        break 'walk;
                    }
                }
            }
        }

        if out.is_empty() {
            Ok("[0 matches]".to_string())
        } else {
            Ok(out)
        }
    }

    /// Expand a glob within the root (root-confined), returning matching relative
    /// paths, bounded by [`GLOB_MAX_RESULTS`]. Uses the same `ignore` walker so
    /// `.git` / `target` / dotdirs are skipped, matching each entry's
    /// root-relative path against the compiled glob.
    pub fn glob(&self, pattern: &str) -> Result<String, String> {
        let set = build_globset(pattern)?;
        let mut out = String::new();
        let mut count = 0usize;

        for entry in self.walker() {
            if count >= GLOB_MAX_RESULTS {
                out.push_str("[…result cap reached]\n");
                break;
            }
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let rel = match entry.path().strip_prefix(&self.root) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if rel.as_os_str().is_empty() {
                continue; // the root itself
            }
            if set.is_match(rel) {
                out.push_str(&format!("{}\n", rel.display()));
                count += 1;
            }
        }

        if out.is_empty() {
            Ok("[0 paths]".to_string())
        } else {
            Ok(out)
        }
    }

    /// A root-confined `ignore` walker. `git_ignore` etc. skip gitignored files;
    /// `hidden(true)` skips dotfiles/dotdirs; we never follow symlinks
    /// (`follow_links(false)` is the default) so a link pointing OUT of the root
    /// is never traversed. `.git` / `target` / `node_modules` are skipped via the
    /// explicit filter below in addition to gitignore (so they are skipped even
    /// when not gitignored, e.g. a fresh checkout's `target`).
    fn walker(&self) -> ignore::Walk {
        ignore::WalkBuilder::new(&self.root)
            .hidden(true) // skip dotfiles/dotdirs
            .follow_links(false) // NEVER follow a symlink out of root
            .git_ignore(true)
            .git_global(false)
            .git_exclude(true)
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                !matches!(name.as_ref(), ".git" | "target" | "node_modules")
            })
            .build()
    }
}

/// Compile a glob pattern into a single-pattern [`globset::GlobSet`]. `**`
/// matches across separators; the pattern is matched against ROOT-RELATIVE
/// paths so it cannot reference anything above the root.
fn build_globset(pattern: &str) -> Result<globset::GlobSet, String> {
    let glob = globset::GlobBuilder::new(pattern)
        .literal_separator(false)
        .build()
        .map_err(|e| format!("invalid glob: {e}"))?;
    let mut builder = globset::GlobSetBuilder::new();
    builder.add(glob);
    builder.build().map_err(|e| format!("invalid glob: {e}"))
}

/// Read at most `cap` bytes from `path` without allocating for the whole file.
fn read_capped(path: &Path, cap: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    file.take(cap as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Trim a single grep match line to a sane width for the transcript.
fn elide_line(line: &str) -> String {
    const MAX: usize = 200;
    let t = line.trim_end();
    if t.chars().count() <= MAX {
        return t.to_string();
    }
    let cut: String = t.chars().take(MAX).collect();
    format!("{cut}…")
}

// =============================================================================
// Exa web backend — egress, gated
// =============================================================================

/// Number of web-search results to request. Small on purpose: the model wants a
/// few grounded hits, not a page of links.
const EXA_NUM_RESULTS: u32 = 5;

/// An Exa-built HTTP request, returned by the pure request-builders so the URL /
/// headers / body JSON can be unit-tested WITHOUT performing the live call.
///
/// `Debug` is implemented MANUALLY (not derived) so the API key carried in
/// `api_key_header` is NEVER printed — a derived `Debug` would leak it into any
/// log / panic message that formats an `ExaRequest`. The field stays public so
/// the in-module tests can assert on it directly; only the `Debug` rendering is
/// redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct ExaRequest {
    pub url: &'static str,
    pub api_key_header: String,
    pub body: serde_json::Value,
}

impl std::fmt::Debug for ExaRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExaRequest")
            .field("url", &self.url)
            .field("api_key_header", &"<redacted>")
            .field("body", &self.body)
            .finish()
    }
}

/// The egress web backend. Holds the Exa API key (env-supplied, never logged).
/// Present ONLY when a key is configured, so the executor's `web: Option<…>`
/// directly encodes "egress is possible".
#[derive(Clone)]
pub struct ExaBackend {
    api_key: String,
    client: reqwest::Client,
}

impl ExaBackend {
    /// Build from an API key. The key is validated as non-empty here; the live
    /// HTTP client is rustls-only (no native-tls / OpenSSL).
    pub fn new(api_key: impl Into<String>) -> Result<Self, String> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err("Exa API key must not be empty".to_string());
        }
        let client = reqwest::Client::builder()
            // Bound a stalled Exa call: without this the `.await` in `run` can hang
            // indefinitely and the burst's wall-clock check (top of the loop) never
            // runs again.
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;
        Ok(Self { api_key, client })
    }

    /// PURE request builder for `fetch{url}` -> Exa `/contents`. Body:
    /// `{ "urls": [url], "text": true, "livecrawl": "always" }`. Separated from
    /// the live call so the request shape is unit-testable.
    pub fn build_fetch_request(&self, url: &str) -> ExaRequest {
        ExaRequest {
            url: "https://api.exa.ai/contents",
            api_key_header: self.api_key.clone(),
            body: json!({
                "urls": [url],
                "text": true,
                // Exa Contents API: a per-page LLM summary + key highlights. These are
                // the "distilled findings" the planner panel renders (not just raw text).
                "summary": true,
                "highlights": true,
                "livecrawl": "always",
            }),
        }
    }

    /// PURE request builder for `websearch{query}` -> Exa `/search`. Body:
    /// `{ "query": query, "type": "neural", "numResults": <small>,
    ///    "contents": {"text": true} }`. Separated for unit-testing.
    pub fn build_search_request(&self, query: &str) -> ExaRequest {
        ExaRequest {
            url: "https://api.exa.ai/search",
            api_key_header: self.api_key.clone(),
            body: json!({
                "query": query,
                "type": "neural",
                "numResults": EXA_NUM_RESULTS,
                // Query-guided per-page summary + highlights (the "findings"), plus text.
                "contents": {
                    "text": true,
                    "summary": { "query": query },
                    "highlights": true,
                },
            }),
        }
    }

    /// Perform a built request and parse the JSON to a compact text result. The
    /// live HTTP is integration-deferred; the parsing is unit-tested against a
    /// fixture string via [`parse_exa_response`].
    async fn run(&self, req: ExaRequest) -> Result<ExaOutcome, String> {
        let resp = self
            .client
            .post(req.url)
            .header("x-api-key", &self.api_key)
            .json(&req.body)
            .send()
            .await
            .map_err(|e| format!("exa request failed: {e}"))?;
        let status = resp.status();
        // Bound the body read: a hostile/buggy endpoint returning a gigabyte must
        // not be buffered whole into RAM. We accept at most a few result-lengths
        // of body (the parser re-caps to MAX_RESULT_LEN anyway) and stop early.
        let text = read_body_capped(resp, HTTP_BODY_CAP).await?;
        if !status.is_success() {
            // Do NOT echo the key; the body may carry an error message, capped.
            return Err(format!(
                "exa returned HTTP {}: {}",
                status.as_u16(),
                elide_line(&text)
            ));
        }
        Ok(ExaOutcome {
            text: parse_exa_response(&text),
            pages: parse_exa_pages(&text),
        })
    }
}

/// Parse an Exa `/contents` or `/search` JSON response to a compact text blob.
/// Both endpoints return a `results` array of objects carrying (some of)
/// `title` / `url` / `text`. We render each result as a small header + its text;
/// a body without `results` falls back to the raw (capped) JSON so the model
/// still sees SOMETHING actionable. PURE + total so it is unit-testable against
/// a captured fixture.
pub fn parse_exa_response(body: &str) -> String {
    let value: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return elide_to(body, MAX_RESULT_LEN),
    };
    let results = value.get("results").and_then(|r| r.as_array());
    let Some(results) = results else {
        return elide_to(body, MAX_RESULT_LEN);
    };
    if results.is_empty() {
        return "[exa: 0 results]".to_string();
    }
    let mut out = String::new();
    for (i, r) in results.iter().enumerate() {
        let title = r.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let url = r.get("url").and_then(|v| v.as_str()).unwrap_or("");
        let text = r.get("text").and_then(|v| v.as_str()).unwrap_or("");
        out.push_str(&format!("[{}] {title}\n{url}\n", i + 1));
        if !text.is_empty() {
            out.push_str(text.trim());
            out.push('\n');
        }
        out.push('\n');
        if out.len() >= MAX_RESULT_LEN {
            break;
        }
    }
    elide_to(&out, MAX_RESULT_LEN)
}

/// A single web page surfaced to the planner panel's Websearch view: the source
/// URL + title + a distilled summary (Exa's per-page `summary`, falling back to
/// `highlights` then a `text` snippet). Serialized into the activity bridge so the
/// host shows REAL pages/findings, not demo content.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct ExaPage {
    pub url: String,
    pub title: String,
    pub summary: String,
}

/// The result of an Exa call: the compact `text` fed back to the burst model
/// (unchanged behavior) PLUS the structured `pages` for the live Websearch view.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct ExaOutcome {
    pub text: String,
    pub pages: Vec<ExaPage>,
}

/// Parse an Exa `/search` or `/contents` JSON body into structured pages for the
/// Websearch view. Summary precedence: `summary` -> joined `highlights` -> a `text`
/// snippet. PURE + total: invalid JSON / no `results` -> empty Vec (never panics).
/// Caps to 6 pages and each summary to 400 chars (char-boundary safe).
pub fn parse_exa_pages(body: &str) -> Vec<ExaPage> {
    let value: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let results = match value.get("results").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return Vec::new(),
    };

    let mut pages = Vec::with_capacity(results.len().min(6));
    for res in results.iter().take(6) {
        // Cap url + title (chars, boundary-safe): the whole event is one bridge line
        // bounded by MAX_LINE_BYTES on the host — a long url/title would push the line
        // over the cap and the reader would silently drop the entire event.
        let url: String = res
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(500)
            .collect();
        // A result with no source URL is not a real page for the Websearch view.
        if url.is_empty() {
            continue;
        }
        let title_raw = res.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let title: String = if title_raw.is_empty() {
            url.clone()
        } else {
            title_raw.chars().take(160).collect()
        };

        let summary_raw = res.get("summary").and_then(|v| v.as_str()).unwrap_or("");
        let summary = if !summary_raw.is_empty() {
            summary_raw.to_string()
        } else if let Some(highlights) = res.get("highlights").and_then(|v| v.as_array()) {
            let parts: Vec<String> = highlights
                .iter()
                .filter_map(|h| h.as_str())
                .take(5)
                .map(|s| s.chars().take(100).collect::<String>())
                .collect();
            if !parts.is_empty() {
                parts.join(" … ")
            } else {
                let text = res.get("text").and_then(|v| v.as_str()).unwrap_or("");
                text.trim().chars().take(240).collect()
            }
        } else {
            let text = res.get("text").and_then(|v| v.as_str()).unwrap_or("");
            text.trim().chars().take(240).collect()
        };

        let summary = summary.trim().chars().take(400).collect::<String>();

        pages.push(ExaPage {
            url,
            title,
            summary,
        });
    }
    pages
}

/// Read an HTTP response body, buffering AT MOST `cap` bytes then stopping. The
/// body is streamed chunk-by-chunk (`bytes_stream`) and accumulation halts once
/// the cap is reached, so a multi-gigabyte body is never held whole in RAM —
/// mirroring the `read_capped`/`take` pattern the FS backend uses for files. The
/// returned text is `from_utf8_lossy` of the (possibly truncated) head; the
/// callers re-cap to [`MAX_RESULT_LEN`] downstream. `Content-Length`, when
/// present and over `cap`, lets us stop before pulling the first oversized chunk.
pub(crate) async fn read_body_capped(
    resp: reqwest::Response,
    cap: usize,
) -> Result<String, String> {
    use futures::StreamExt;

    // Fast path the doc promises: when the server declares a `Content-Length`
    // larger than the cap, reject BEFORE pulling a single (possibly oversized)
    // chunk — no point streaming a body we already know we will reject. The
    // streaming truncation below is still the authoritative fallback for chunked /
    // length-less responses (and for a server that under-declares its length).
    if let Some(len) = resp.content_length() {
        if len > cap as u64 {
            return Err(format!(
                "response too large ({len} bytes declared, cap {cap})"
            ));
        }
    }

    let mut buf: Vec<u8> = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("response read failed: {e}"))?;
        // `accumulate_capped` reports whether the buffer is full; once it is we
        // stop pulling further chunks so a gigabyte body is never read whole.
        if accumulate_capped(&mut buf, &chunk, cap) {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Append as much of `chunk` to `buf` as fits under `cap` TOTAL bytes; returns
/// `true` once `buf` has reached the cap (caller should stop reading). PURE so
/// the cap logic is unit-testable without a live HTTP response.
fn accumulate_capped(buf: &mut Vec<u8>, chunk: &[u8], cap: usize) -> bool {
    if buf.len() >= cap {
        return true;
    }
    let room = cap - buf.len();
    if chunk.len() <= room {
        buf.extend_from_slice(chunk);
        buf.len() >= cap
    } else {
        buf.extend_from_slice(&chunk[..room]);
        true
    }
}

/// Truncate a string to at most `cap` CHARS, appending a marker when cut. Counts
/// `chars()` (not bytes) so the bound matches the loop's `cap_result` semantics
/// (`MAX_RESULT_LEN` chars): a previous byte cap could let a multibyte body exceed
/// the intended char bound, then the loop would cap it a SECOND time. Iterating
/// `chars` also never splits a UTF-8 codepoint. (The loop re-caps too; this keeps
/// the executor honest.)
fn elide_to(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.to_string();
    }
    let kept: String = s.chars().take(cap).collect();
    format!("{kept}\n[…truncated]")
}

// =============================================================================
// The real executor
// =============================================================================

/// The L2.3 executor: dispatches each [`AgentAction`] to the MCP, FS, or Exa
/// backend. Constructed by `main` from config/env; `web` is `None` when no Exa
/// key is configured, which is what makes the burst's `allow_egress` false.
pub struct RealExecutor {
    mcp: std::sync::Arc<dyn McpBackend>,
    fs: FsBackend,
    web: Option<ExaBackend>,
    /// The model that drives the LOCAL planner (Phase 11.2). `None` when no model
    /// is configured (the no-config dev path), which disables the `plan` routine
    /// with a clear error rather than running it against a missing model.
    model: Option<std::sync::Arc<dyn CoderModel>>,
    /// The Oracle-side project key. The planner passes it to `project_structure` /
    /// `plan_submit` / `project_create_plan_tasks`; the runner passes it to
    /// `project_get` / `project_claim_task` / `project_update_status` / `spawn_mini_coder`.
    /// Empty when not configured; the planner/runner then escalate with a precise message
    /// instead of acting against the wrong project. (Since Piece 1b the runner reads tasks
    /// off the Kanban by this id, so the executor no longer needs a local project_root —
    /// the FS backend holds its own canonical root for the read-only navigation tools.)
    project_id: String,
    /// The coder-tier milestone emitter the planner writes its phases through (the
    /// FILE BRIDGE to the host Console). Disabled (no-op) unless the host set
    /// `DEVBOULE_ACTIVITY_FILE` at launch — observability never gates the run.
    activity: crate::activity::Activity,
    /// Orchestrator composer auto-create toggle (from `DEVBOULE_AUTO_CREATE`). `true` (default) ⇒
    /// an approved plan auto-creates its tasks on the board; `false` ⇒ the plan is drafted +
    /// submitted but its tasks are NOT created (the operator creates them). See `run_planner`.
    auto_create: bool,
    /// Live steer INBOX (from `DEVBOULE_STEER_FILE`). The burst calls `drain_steer`
    /// between rounds to inject app-sent messages as human turns. Disabled (no-op)
    /// when the env is unset (every non-orchestrator / test executor).
    steer: crate::steer::Steer,
    /// FIX 1: `true` ONLY when this binary IS the orchestrator (launched with a seeded
    /// `DEVBOULE_GOAL`, the same `config::seeded_goal().is_some()` signal that marks
    /// `OmlxModel.orchestrator`). Surfaced through [`ToolExecutor::is_orchestrator`] so the
    /// burst CO-ENFORCES it with the activity bridge to gate the Kairion `question` emission:
    /// a plain coder / mini (no goal) can NEVER emit a question event even if the activity file
    /// is set. Defaults `false`; set from config via [`RealExecutor::with_orchestrator`].
    is_orchestrator: bool,
}

impl RealExecutor {
    pub fn new(
        mcp: std::sync::Arc<dyn McpBackend>,
        fs: FsBackend,
        web: Option<ExaBackend>,
    ) -> Self {
        Self {
            mcp,
            fs,
            web,
            model: None,
            project_id: String::new(),
            // Disabled by default; `with_planner` resolves the real emitter from env.
            activity: crate::activity::Activity::disabled(),
            // Default ON (the pre-feature behavior); `with_auto_create` overrides from env.
            auto_create: true,
            // Resolve the steer inbox from the launch env; disabled when unset.
            steer: crate::steer::Steer::from_env(),
            // Default NON-orchestrator; `with_orchestrator` sets it from config at launch.
            is_orchestrator: false,
        }
    }

    /// Set the orchestrator-composer auto-create toggle (from `DEVBOULE_AUTO_CREATE`). Builder-style
    /// so existing `new()` callers/tests keep the default-ON behavior unchanged.
    pub fn with_auto_create(mut self, auto_create: bool) -> Self {
        self.auto_create = auto_create;
        self
    }

    /// FIX 1: mark this executor as the orchestrator (set from `config::seeded_goal().is_some()`
    /// at launch). Builder-style so existing `new()` callers/tests stay NON-orchestrator (the
    /// default), keeping the Kairion question event off for every plain coder / mini / test.
    pub fn with_orchestrator(mut self, is_orchestrator: bool) -> Self {
        self.is_orchestrator = is_orchestrator;
        self
    }

    /// Attach the planner inputs (the model that drives the local planner + the
    /// Oracle-side project id). Builder-style so existing call sites / tests that
    /// do not need the planner keep using [`RealExecutor::new`] unchanged. Without
    /// this, the `plan` action escalates with a clear "planner not configured"
    /// message rather than running half-wired.
    pub fn with_planner(
        mut self,
        model: std::sync::Arc<dyn CoderModel>,
        project_id: impl Into<String>,
    ) -> Self {
        self.model = Some(model);
        self.project_id = project_id.into();
        // Resolve the coder-tier milestone emitter from the launch env
        // (`DEVBOULE_ACTIVITY_FILE`). Unset (the headless / no-host case) ⇒ a disabled
        // no-op emitter, so the planner runs identically with or without the Console.
        self.activity = crate::activity::Activity::from_env();
        self
    }

    /// `true` when an Exa key is configured. The binary passes this as the
    /// burst's `allow_egress` so the structural gate and the backend presence
    /// can never disagree.
    pub fn egress_enabled(&self) -> bool {
        self.web.is_some()
    }
}

#[async_trait]
impl ToolExecutor for RealExecutor {
    /// The configured user-MCP server names, surfaced from the MCP backend (the
    /// `MultiMcpBackend` when user servers are wired; empty for the plain Oracle
    /// backend). The burst validates an `mcp_tool` server name against this at parse
    /// time, so an unknown server is an immediate format error.
    fn known_mcp_servers(&self) -> &[String] {
        self.mcp.user_server_names()
    }

    /// Drain the live steer inbox (app → running orchestrator) AND the Censor
    /// steer file at `<project_root>/.aspis/steer_censor` (Phase B deep findings).
    /// Empty when no steer sources are active.
    fn drain_steer(&self) -> Vec<String> {
        let mut msgs = self.steer.drain();
        // Phase B: immediate steer cache from Censor passes
        drain_steer_cache(&self.fs.root, &mut msgs);
        // NOTE: the persistent queue (.aspis/censor_queue/) is NOT drained here.
        // It survives restarts and is consumed by cloud coders via MCP
        // `censor_findings(drain_queue=true)`. The local coder gets findings
        // via the steer_cache file (overwritten each Censor pass).
        msgs
    }
    fn emit_chat(&self, role: &str, text: &str) {
        self.activity.chat(role, text);
    }

    /// B14b: hand the burst an owned clone of the live activity bridge, so its streaming closure
    /// can emit `chat_delta`s (cumulative reply-so-far) without borrowing the executor across the
    /// async model call. The final `emit_chat` still lands the durable entry; deltas are ephemeral.
    fn activity_handle(&self) -> crate::activity::Activity {
        self.activity.clone()
    }

    /// FIX 1: surface the orchestrator flag so the burst co-enforces it with the activity
    /// bridge before emitting a Kairion `question` event. `false` for any executor not built
    /// via [`RealExecutor::with_orchestrator`] (every plain coder / mini / test path).
    fn is_orchestrator(&self) -> bool {
        self.is_orchestrator
    }

    async fn execute(&self, action: &AgentAction) -> ToolResult {
        match action {
            // --- MCP backend: private/grounded Oracle + write delegation ------
            // Contract MUST match the Python server. The session identity
            // (role/agent_id/session_token) is injected by the MCP backend impl
            // (it owns the token from `agent_register`), so the params here carry
            // ONLY the action-specific arguments.
            AgentAction::OracleAsk { query } => {
                self.mcp_call("oracle_ask", json!({ "query": query })).await
            }
            AgentAction::OracleContext { query, limit } => {
                let mut params = json!({ "query": query });
                if let Some(limit) = limit {
                    params["limit"] = json!(limit);
                }
                self.mcp_call("oracle_context", params).await
            }
            // spawn_mini is the WRITE arm: the orchestrator NEVER writes files
            // itself; it delegates to the mini sub-agent. Tool name is
            // `spawn_mini_coder` on the server.
            AgentAction::SpawnMini { task, files, write } => {
                // Coarse orchestrator milestone: a count label only (NO task text — it
                // is free-form model output we must not surface). The mini's OWN
                // per-round activity is emitted in-process under its own agent_id; this
                // is the orchestrator's "I am delegating" line on its own timeline.
                self.activity.milestone(
                    &format!(
                        "delegating {} file{} to mini-coder",
                        files.len(),
                        if files.len() == 1 { "" } else { "s" }
                    ),
                    crate::activity::Node::Dot,
                );
                let params = json!({
                    "task": task,
                    "files": files,
                    "write": write,
                });
                self.mcp_call("spawn_mini_coder", params).await
            }
            // `plan` triggers the LOCAL planner (Phase 11.2): STRUCTURE -> EXPLORE ->
            // PLAN -> plan_submit (the human gate) -> on approval, create the plan's
            // tasks on the project KANBAN (`project_create_plan_tasks`, the single shared
            // task store). The goal is the model's `steps` joined into one framing line.
            // The burst model gets back a COMPACT outcome line ("Plan: N tasks, submitted
            // -> approved (planId …, created on the board)") so the outer loop knows the
            // verdict without re-reading the plan.
            AgentAction::Plan { steps } => self.run_plan(steps).await,

            // `run_plan` EXECUTES the approved plan (Phase 11.3): the deterministic DAG
            // runner reads the active plan's tasks off the Kanban (`project_get`, filtered
            // by planId), delegates each ready task to `spawn_mini_coder` (Censor gate +
            // retry/escalate) in dependency order, lands finished tasks in `review`, and
            // stops on the first BLOCKED task. No LLM here — the plan is already produced
            // + human-approved; this just drives the board.
            AgentAction::RunPlan {} => self.run_tasks().await,

            // --- FS backend: in-process, root-confined, read-only -------------
            // The FS ops do BLOCKING syscalls (`std::fs`, the `ignore` walker), so
            // they run on a `spawn_blocking` thread — never on the async reactor,
            // which would stall the TUI. A `Clone` of the backend (cheap: one
            // `PathBuf`) and the owned args move into the closure; confinement +
            // caps are unchanged (the same `FsBackend` method runs, just off-thread).
            // A `JoinError` (the blocking task panicked / was cancelled) becomes a
            // failed `ToolResult`, never a panic that takes down the burst.
            AgentAction::Read { path } => {
                let fs = self.fs.clone();
                let arg = path.clone();
                match tokio::task::spawn_blocking(move || fs.read(&arg)).await {
                    Ok(Ok(text)) => ToolResult::ok(text),
                    Ok(Err(e)) => ToolResult::err(format!("read {path}: {e}")),
                    Err(e) => ToolResult::err(format!("read {path} failed to run: {e}")),
                }
            }
            AgentAction::Grep { pattern, glob } => {
                let fs = self.fs.clone();
                let pattern = pattern.clone();
                let glob = glob.clone();
                let res =
                    tokio::task::spawn_blocking(move || fs.grep(&pattern, glob.as_deref())).await;
                match res {
                    Ok(Ok(text)) => ToolResult::ok(text),
                    Ok(Err(e)) => ToolResult::err(format!("grep: {e}")),
                    Err(e) => ToolResult::err(format!("grep failed to run: {e}")),
                }
            }
            AgentAction::Glob { pattern } => {
                let fs = self.fs.clone();
                let pattern = pattern.clone();
                match tokio::task::spawn_blocking(move || fs.glob(&pattern)).await {
                    Ok(Ok(text)) => ToolResult::ok(text),
                    Ok(Err(e)) => ToolResult::err(format!("glob: {e}")),
                    Err(e) => ToolResult::err(format!("glob failed to run: {e}")),
                }
            }

            // Level-2 progressive disclosure: load an installed library skill's body (or a guarded
            // supporting file). Library skills live in `<root>/.claude/skills/`; the role-skill dirs
            // (mini/coder/design/orchestrator) are excluded — they're injected directly. Re-discovers
            // off-thread at call time (cheap, and picks up a just-installed skill).
            AgentAction::LoadSkill { name } => {
                let root = self.fs.root.clone();
                let req = name.clone();
                let label = name.clone();
                match tokio::task::spawn_blocking(move || {
                    let roots = vec![root.join(".claude").join("skills")];
                    let skills = crate::skills::discover_library_skills(
                        &roots,
                        &["mini", "coder", "design", "orchestrator"],
                    );
                    crate::skills::load_skill_content(&skills, &req)
                })
                .await
                {
                    Ok(Ok(text)) => ToolResult::ok(text),
                    Ok(Err(e)) => ToolResult::err(format!("load_skill {label}: {e}")),
                    Err(e) => ToolResult::err(format!("load_skill {label} failed to run: {e}")),
                }
            }

            // --- Exa backend: egress, gated -----------------------------------
            // The burst's egress gate already blocks these when egress is off, so
            // reaching here with `web == None` is a logic error — report it
            // rather than silently succeed.
            AgentAction::Fetch { url } => match &self.web {
                Some(web) => match web.run(web.build_fetch_request(url)).await {
                    Ok(outcome) => {
                        self.activity.websearch(url, &outcome.pages);
                        ToolResult::ok(outcome.text)
                    }
                    Err(e) => ToolResult::err(format!("fetch {url}: {e}")),
                },
                None => ToolResult::err("fetch reached the executor with egress disabled"),
            },
            AgentAction::Websearch { query } => match &self.web {
                Some(web) => match web.run(web.build_search_request(query)).await {
                    Ok(outcome) => {
                        self.activity.websearch(query, &outcome.pages);
                        ToolResult::ok(outcome.text)
                    }
                    Err(e) => ToolResult::err(format!("websearch: {e}")),
                },
                None => ToolResult::err("websearch reached the executor with egress disabled"),
            },

            // --- User MCP server: routed through the MCP backend --------------
            // `mcp_tool` is DECOUPLED from web egress (`is_egress() == false`, design
            // §5.2): a user server is its own opt-in capability, so the web-egress gate
            // never blocks it — it reaches here whether or not `allow_egress` is set. Its
            // gate is the KNOWN-SERVER set, checked at PARSE time (validate_with_servers);
            // here we route to the named user backend via the MultiMcpBackend. A plain
            // Oracle backend (no user servers) returns a recoverable error, never a panic.
            AgentAction::McpTool {
                server,
                tool,
                params,
            } => match self.mcp.call_user_tool(server, tool, params.clone()).await {
                Ok(text) => ToolResult::ok(text),
                Err(e) => ToolResult::err(format!("mcp_tool {server}.{tool}: {e}")),
            },

            // --- Terminal actions never reach an executor ---------------------
            // The loop returns before dispatching them; if one arrives here it is
            // a logic error.
            AgentAction::AskUser { .. }
            | AgentAction::Done { .. }
            | AgentAction::Escalate { .. } => {
                ToolResult::err("terminal action must not be dispatched")
            }
        }
    }
}

impl RealExecutor {
    /// Drive the LOCAL planner for a `plan` action (Phase 11.2). The goal is the
    /// joined plan `steps`. Requires the model + project id wired via
    /// [`RealExecutor::with_planner`]; without them the action is an error the model
    /// can recover from (never a panic). The planner submits the plan through the human
    /// gate and, ON APPROVAL, creates the plan's tasks on the project Kanban; on any
    /// planner error (structure failure, no valid plan in the retry budget, plan_submit
    /// failure, or the bulk Kanban create failing) we surface the precise reason as a
    /// failed result.
    async fn run_plan(&self, steps: &[String]) -> ToolResult {
        let Some(model) = self.model.clone() else {
            return ToolResult::err(
                "plan: the local planner is not configured (no model wired); cannot plan",
            );
        };
        // The goal is the model's intent: the plan steps as one framing line. The
        // action layer already validated each step non-empty + bounded.
        let goal = steps.join("\n");
        match run_planner(
            &goal,
            model.as_ref(),
            self.mcp.as_ref(),
            &self.fs,
            &self.project_id,
            &self.activity,
            self.auto_create,
        )
        .await
        {
            Ok(outcome) => ToolResult::ok(outcome.compact_summary()),
            Err(e) => ToolResult::err(format!("plan: {e}")),
        }
    }

    /// Drive the Phase 11.3 DAG runner for a `run_plan` action. The runner is
    /// deterministic (no model) and reuses the existing `mcp` (to read the Kanban via
    /// `project_get` + advance tasks via `project_claim_task`/`project_update_status` +
    /// delegate via `spawn_mini_coder`), the Oracle-side `project_id` (the board key), and
    /// `activity` (Console milestones). A run that completes every plan task is an OK
    /// result; a run that stops on a BLOCKED task is a FAILED result whose text names the
    /// task + tells the orchestrator to `ask_user` (a block is an expected outcome,
    /// surfaced as an error the model recovers from, never a panic). No active plan on the
    /// board, or a backend/transport error, is an error.
    async fn run_tasks(&self) -> ToolResult {
        match crate::runner::run_tasks(self.mcp.as_ref(), &self.project_id, &self.activity).await {
            Ok(report) if report.blocked.is_none() => ToolResult::ok(report.compact_summary()),
            Ok(report) => ToolResult::err(report.compact_summary()),
            Err(e) => ToolResult::err(format!("run_plan: {e}")),
        }
    }

    /// Dispatch one MCP `call_tool` and wrap its result. A backend error becomes
    /// a failed [`ToolResult`] the model can recover from, never a panic.
    async fn mcp_call(&self, name: &str, params: serde_json::Value) -> ToolResult {
        match self.mcp.call_tool(name, params).await {
            Ok(text) => ToolResult::ok(text),
            Err(e) => ToolResult::err(format!("{name}: {e}")),
        }
    }
}

/// Drain the immediate steer cache file (overwritten each Censor pass).
fn drain_steer_cache(root: &Path, msgs: &mut Vec<String>) {
    let path = root.join(".aspis").join("steer_censor");
    if !path.exists() {
        return;
    }
    if let Ok(content) = std::fs::read_to_string(&path) {
        let trimmed = content.trim().to_string();
        if !trimmed.is_empty() {
            msgs.push(trimmed);
        }
    }
    let _ = std::fs::remove_file(&path);
}

/// Drain the persistent Censor queue directory (accumulated batch files).
fn drain_censor_queue_dir(root: &Path, msgs: &mut Vec<String>) {
    let queue_dir = root.join(".aspis").join("censor_queue").join("pending");
    if !queue_dir.exists() {
        return;
    }
    let mut files: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&queue_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|e| e == "json") {
                files.push(p);
            }
        }
    }
    files.sort();
    for path in &files {
        if let Ok(content) = std::fs::read_to_string(path) {
            let trimmed = content.trim().to_string();
            if !trimmed.is_empty() {
                msgs.push(trimmed);
            }
        }
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::tempdir;

    // --- MockMcpBackend: asserts the right tool name + params per action ------

    /// Records every `call_tool(name, params)` so a test can assert the
    /// action->MCP mapping (tool name + the action-specific params). Returns a
    /// canned ok result.
    struct MockMcpBackend {
        calls: Mutex<Vec<(String, serde_json::Value)>>,
    }
    impl MockMcpBackend {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }
        fn last(&self) -> (String, serde_json::Value) {
            self.calls
                .lock()
                .unwrap()
                .last()
                .cloned()
                .expect("a call was made")
        }
    }
    #[async_trait]
    impl McpBackend for MockMcpBackend {
        async fn call_tool(&self, name: &str, params: serde_json::Value) -> Result<String, String> {
            self.calls
                .lock()
                .unwrap()
                .push((name.to_string(), params.clone()));
            Ok(format!("[mock {name}]"))
        }
    }

    /// Build a RealExecutor over a tempdir root + the mock MCP backend + no web.
    fn exec_with(root: &Path, mcp: std::sync::Arc<MockMcpBackend>) -> RealExecutor {
        let fs = FsBackend::new(root).expect("tempdir root canonicalizes");
        RealExecutor::new(mcp, fs, None)
    }

    // --- MCP action mapping ---------------------------------------------------

    #[tokio::test]
    async fn oracle_ask_maps_to_oracle_ask_tool() {
        let dir = tempdir().unwrap();
        let mcp = std::sync::Arc::new(MockMcpBackend::new());
        let exec = exec_with(dir.path(), mcp.clone());
        let r = exec
            .execute(&AgentAction::OracleAsk {
                query: "where is the launch path".into(),
            })
            .await;
        assert!(r.ok);
        let (name, params) = mcp.last();
        assert_eq!(name, "oracle_ask");
        assert_eq!(params["query"], json!("where is the launch path"));
    }

    #[tokio::test]
    async fn oracle_context_maps_and_forwards_limit() {
        let dir = tempdir().unwrap();
        let mcp = std::sync::Arc::new(MockMcpBackend::new());
        let exec = exec_with(dir.path(), mcp.clone());
        exec.execute(&AgentAction::OracleContext {
            query: "q".into(),
            limit: Some(3),
        })
        .await;
        let (name, params) = mcp.last();
        assert_eq!(name, "oracle_context");
        assert_eq!(params["query"], json!("q"));
        assert_eq!(params["limit"], json!(3));
    }

    #[tokio::test]
    async fn oracle_context_omits_limit_when_absent() {
        let dir = tempdir().unwrap();
        let mcp = std::sync::Arc::new(MockMcpBackend::new());
        let exec = exec_with(dir.path(), mcp.clone());
        exec.execute(&AgentAction::OracleContext {
            query: "q".into(),
            limit: None,
        })
        .await;
        let (_name, params) = mcp.last();
        assert!(
            params.get("limit").is_none(),
            "limit must be omitted when None: {params}"
        );
    }

    #[tokio::test]
    async fn spawn_mini_maps_to_spawn_mini_coder_with_write_arm() {
        let dir = tempdir().unwrap();
        let mcp = std::sync::Arc::new(MockMcpBackend::new());
        let exec = exec_with(dir.path(), mcp.clone());
        exec.execute(&AgentAction::SpawnMini {
            task: "fix it".into(),
            files: vec!["src/a.rs".into()],
            write: true,
        })
        .await;
        let (name, params) = mcp.last();
        assert_eq!(
            name, "spawn_mini_coder",
            "spawn_mini maps to spawn_mini_coder"
        );
        assert_eq!(params["task"], json!("fix it"));
        assert_eq!(params["files"], json!(["src/a.rs"]));
        assert_eq!(params["write"], json!(true), "the WRITE arm is forwarded");
    }

    #[tokio::test]
    async fn plan_without_planner_configured_is_a_clear_error() {
        // Phase 11.2: `plan` now drives the LOCAL planner. An executor built via
        // `new` (no `with_planner`) has no model wired, so the action returns a
        // clear not-configured error the model can recover from — and must NOT call
        // any server tool (the planner short-circuits before STRUCTURE).
        let dir = tempdir().unwrap();
        let mcp = std::sync::Arc::new(MockMcpBackend::new());
        let exec = exec_with(dir.path(), mcp.clone());
        let r = exec
            .execute(&AgentAction::Plan {
                steps: vec!["a".into(), "b".into()],
            })
            .await;
        assert!(
            !r.ok,
            "plan without a wired planner is an error: {}",
            r.output
        );
        assert!(
            r.output.contains("not configured"),
            "names the missing-planner cause: {}",
            r.output
        );
        assert!(
            mcp.calls.lock().unwrap().is_empty(),
            "plan must NOT call any server tool when the planner is not configured"
        );
    }

    #[tokio::test]
    async fn plan_with_planner_drives_the_local_planner_and_submits() {
        // With the planner wired (`with_planner`), the `plan` action runs the real
        // STRUCTURE -> EXPLORE -> PLAN -> plan_submit routine against the mock MCP.
        // The mock returns a one-file spine, a canned note from the scripted model,
        // a valid plan, and an approved plan_submit; the compact result names the
        // outcome. (The deep planner behavior is unit-tested in `crate::planner`.)
        use crate::model::ScriptedModel;
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), b"fn a() {}\n").unwrap();

        // A planner-aware mock MCP: a one-file spine, grounding, an approved plan_submit,
        // and the Kanban bulk-create. It RECORDS whether the create was called + with
        // which planId so the test can assert the plan landed on the board.
        // (Distinct from MockMcpBackend, which canned every call.)
        struct PlannerMcp {
            created_plan_id: Mutex<Option<String>>,
        }
        #[async_trait]
        impl McpBackend for PlannerMcp {
            async fn call_tool(
                &self,
                name: &str,
                params: serde_json::Value,
            ) -> Result<String, String> {
                match name {
                    "project_structure" => Ok(json!({
                        "spine": [{"path": "src/a.rs", "inDegree": 1, "topReferencedSymbols": ["a"]}],
                        "summary": {"scanned": 1},
                    })
                    .to_string()),
                    "oracle_context" => Ok("[grounding]".to_string()),
                    "plan_submit" => Ok(json!({"planId": "p1", "status": "approved"}).to_string()),
                    "project_create_plan_tasks" => {
                        let plan_id = params
                            .get("plan_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        *self.created_plan_id.lock().unwrap() = Some(plan_id.clone());
                        Ok(json!({
                            "project": {"id": "proj"},
                            "planId": plan_id,
                            "idMap": {},
                            "tasks": [],
                        })
                        .to_string())
                    }
                    other => Err(format!("unexpected {other}")),
                }
            }
        }

        let note = format!(
            "```note\n{}\n```",
            json!({"source": "src/a.rs", "role": "core", "key_symbols": ["a"], "watch_out": ""})
        );
        let plan = format!(
            "```plan\n{}\n```",
            json!({"projectGoal": "g", "tasks": [{
                "id": "T001", "title": "edit a", "scope": ["src/a.rs"], "contextFiles": [],
                "acceptance": "cargo test", "dependsOn": [], "status": "pending", "attempts": 0,
            }]})
        );
        let model: std::sync::Arc<dyn CoderModel> =
            std::sync::Arc::new(ScriptedModel::new(vec![note, plan]));

        let fs = FsBackend::new(dir.path()).unwrap();
        let mcp = std::sync::Arc::new(PlannerMcp {
            created_plan_id: Mutex::new(None),
        });
        let exec = RealExecutor::new(mcp.clone(), fs, None).with_planner(model, "proj");

        let r = exec
            .execute(&AgentAction::Plan {
                steps: vec!["do the thing".into()],
            })
            .await;
        assert!(r.ok, "the planner succeeded: {}", r.output);
        assert!(
            r.output.contains("approved"),
            "compact result names the verdict: {}",
            r.output
        );
        assert!(
            r.output.contains("1 task"),
            "compact result names the task count: {}",
            r.output
        );
        assert!(
            r.output.contains("planId p1"),
            "compact result names the planId: {}",
            r.output
        );
        // The plan's tasks were created ON THE BOARD (no local tasks.json anymore),
        // tagged with the approved planId from plan_submit.
        assert_eq!(
            mcp.created_plan_id.lock().unwrap().as_deref(),
            Some("p1"),
            "the plan was created on the Kanban under planId p1"
        );
        assert!(
            !dir.path().join(".devboule/tasks.json").exists(),
            "no local tasks.json is written anymore"
        );
    }

    // --- FS confinement -------------------------------------------------------

    #[tokio::test]
    async fn read_is_confined_and_returns_contents() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), b"fn main() {}\n").unwrap();
        let fs = FsBackend::new(dir.path()).unwrap();
        let got = fs.read("src/main.rs").expect("confined read succeeds");
        assert!(got.contains("fn main()"), "got: {got}");
    }

    #[tokio::test]
    async fn read_rejects_post_canonicalize_escape_via_symlink() {
        // A symlink INSIDE the root that points OUTSIDE it: the static path check
        // passes (no `..`, relative), but canonicalization resolves outside the
        // root and resolve() must reject it.
        let outside = tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"top secret").unwrap();
        let root = tempdir().unwrap();
        let link = root.path().join("escape.txt");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), &link).unwrap();
        #[cfg(not(unix))]
        return; // symlink creation differs on Windows; the unix path proves the guard
        let fs = FsBackend::new(root.path()).unwrap();
        let err = fs
            .read("escape.txt")
            .expect_err("a symlink-out read must be rejected");
        assert!(err.contains("escapes the project root"), "got: {err}");
    }

    #[tokio::test]
    async fn symlinked_dir_out_of_root_is_not_traversed_by_glob() {
        // A symlinked DIRECTORY pointing outside the root must not be walked into:
        // its outside contents must never appear in glob/grep results.
        let outside = tempdir().unwrap();
        std::fs::write(outside.path().join("leak.rs"), b"// leaked\n").unwrap();
        let root = tempdir().unwrap();
        std::fs::write(root.path().join("inside.rs"), b"// inside\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), root.path().join("linkdir")).unwrap();
        #[cfg(not(unix))]
        return;
        let fs = FsBackend::new(root.path()).unwrap();
        let listed = fs.glob("**/*.rs").unwrap();
        assert!(
            listed.contains("inside.rs"),
            "the in-root file is listed: {listed}"
        );
        assert!(
            !listed.contains("leak.rs"),
            "the symlinked-out file must NOT be traversed: {listed}"
        );
    }

    #[tokio::test]
    async fn grep_finds_a_planted_match() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/a.rs"),
            b"let x = 1;\n// TODO: fix\nok\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("src/b.rs"), b"nothing here\n").unwrap();
        let fs = FsBackend::new(dir.path()).unwrap();
        let got = fs.grep("TODO", None).unwrap();
        assert!(got.contains("src/a.rs:2:"), "names file:line: {got}");
        assert!(got.contains("TODO: fix"), "shows the matched line: {got}");
        assert!(!got.contains("src/b.rs"), "non-matching file absent: {got}");
    }

    #[tokio::test]
    async fn grep_glob_restricts_files() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), b"MATCH\n").unwrap();
        std::fs::write(dir.path().join("a.txt"), b"MATCH\n").unwrap();
        let fs = FsBackend::new(dir.path()).unwrap();
        let got = fs.grep("MATCH", Some("*.rs")).unwrap();
        assert!(got.contains("a.rs"), "rs file matched: {got}");
        assert!(!got.contains("a.txt"), "txt file excluded by glob: {got}");
    }

    #[tokio::test]
    async fn glob_lists_expected_files_and_skips_git_and_target() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), b"\n").unwrap();
        std::fs::write(dir.path().join("src/main.rs"), b"\n").unwrap();
        // These must be skipped by the walker.
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/config.rs"), b"\n").unwrap();
        std::fs::create_dir(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/build.rs"), b"\n").unwrap();
        let fs = FsBackend::new(dir.path()).unwrap();
        let got = fs.glob("**/*.rs").unwrap();
        assert!(got.contains("src/lib.rs"), "lib.rs listed: {got}");
        assert!(got.contains("src/main.rs"), "main.rs listed: {got}");
        assert!(!got.contains(".git"), ".git skipped: {got}");
        assert!(!got.contains("target"), "target skipped: {got}");
    }

    #[tokio::test]
    async fn read_rejects_a_directory() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        let fs = FsBackend::new(dir.path()).unwrap();
        let err = fs.read("src").expect_err("reading a dir is an error");
        assert!(err.contains("directory"), "got: {err}");
    }

    #[tokio::test]
    async fn fetch_with_no_web_is_an_error_not_a_panic() {
        // The structural gate normally blocks this; defense in depth here.
        let dir = tempdir().unwrap();
        let mcp = std::sync::Arc::new(MockMcpBackend::new());
        let exec = exec_with(dir.path(), mcp);
        let r = exec
            .execute(&AgentAction::Fetch {
                url: "https://x.test".into(),
            })
            .await;
        assert!(!r.ok);
        assert!(r.output.contains("egress disabled"), "got: {}", r.output);
    }

    // --- McpTool routing (Phase B) -------------------------------------------

    /// A backend that records `call_user_tool(server, tool, params)` and returns a
    /// labelled result so the executor's McpTool arm can be asserted end-to-end. It
    /// also knows a fixed set of user-server names (so `known_mcp_servers` is exercised).
    struct UserRoutingMock {
        user_calls: Mutex<Vec<(String, String, serde_json::Value)>>,
        names: Vec<String>,
        unknown: bool,
    }
    impl UserRoutingMock {
        fn new(names: &[&str], unknown: bool) -> Self {
            Self {
                user_calls: Mutex::new(Vec::new()),
                names: names.iter().map(|s| s.to_string()).collect(),
                unknown,
            }
        }
    }
    #[async_trait]
    impl McpBackend for UserRoutingMock {
        async fn call_tool(&self, name: &str, _params: serde_json::Value) -> Result<String, String> {
            // The Oracle path must NOT be hit by an mcp_tool dispatch.
            Err(format!("oracle call_tool unexpectedly invoked for {name}"))
        }
        async fn call_user_tool(
            &self,
            server: &str,
            tool: &str,
            params: serde_json::Value,
        ) -> Result<String, String> {
            self.user_calls
                .lock()
                .unwrap()
                .push((server.to_string(), tool.to_string(), params));
            if self.unknown {
                return Err(format!("unknown user MCP server `{server}`"));
            }
            Ok(format!("[user {server}.{tool}]"))
        }
        fn user_server_names(&self) -> &[String] {
            &self.names
        }
    }

    #[tokio::test]
    async fn mcp_tool_routes_through_call_user_tool_not_the_oracle() {
        let dir = tempdir().unwrap();
        let mcp = std::sync::Arc::new(UserRoutingMock::new(&["my-db"], false));
        let fs = FsBackend::new(dir.path()).unwrap();
        let exec = RealExecutor::new(mcp.clone(), fs, None);
        let r = exec
            .execute(&AgentAction::McpTool {
                server: "my-db".into(),
                tool: "query".into(),
                params: json!({"sql": "SELECT 1"}),
            })
            .await;
        assert!(r.ok, "mcp_tool dispatched ok: {}", r.output);
        assert!(r.output.contains("user my-db.query"), "routed to the user backend: {}", r.output);
        let calls = mcp.user_calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "exactly one user-tool call");
        assert_eq!(calls[0].0, "my-db");
        assert_eq!(calls[0].1, "query");
        assert_eq!(calls[0].2, json!({"sql": "SELECT 1"}));
    }

    #[tokio::test]
    async fn mcp_tool_unknown_server_is_an_error_not_a_panic() {
        let dir = tempdir().unwrap();
        let mcp = std::sync::Arc::new(UserRoutingMock::new(&["my-db"], true));
        let fs = FsBackend::new(dir.path()).unwrap();
        let exec = RealExecutor::new(mcp, fs, None);
        let r = exec
            .execute(&AgentAction::McpTool {
                server: "ghost".into(),
                tool: "query".into(),
                params: json!({}),
            })
            .await;
        assert!(!r.ok, "unknown server is a failed result");
        assert!(r.output.contains("mcp_tool ghost.query"), "names the call: {}", r.output);
        assert!(r.output.contains("unknown user MCP server"), "carries the cause: {}", r.output);
    }

    #[tokio::test]
    async fn known_mcp_servers_surfaces_the_backend_names() {
        // The executor surfaces the backend's user-server names so the burst can
        // validate an mcp_tool server at parse time.
        let dir = tempdir().unwrap();
        let mcp = std::sync::Arc::new(UserRoutingMock::new(&["my-db", "api"], false));
        let fs = FsBackend::new(dir.path()).unwrap();
        let exec = RealExecutor::new(mcp, fs, None);
        assert_eq!(
            exec.known_mcp_servers(),
            &["my-db".to_string(), "api".to_string()]
        );
    }

    #[tokio::test]
    async fn plain_oracle_backend_has_no_user_servers() {
        // The no-user-servers path: a plain MockMcpBackend (Oracle only) reports an
        // EMPTY user-server set, so every mcp_tool is rejected at parse time and the
        // existing executor wiring is unchanged.
        let dir = tempdir().unwrap();
        let mcp = std::sync::Arc::new(MockMcpBackend::new());
        let exec = exec_with(dir.path(), mcp);
        assert!(
            exec.known_mcp_servers().is_empty(),
            "a plain Oracle backend exposes no user servers"
        );
    }

    #[tokio::test]
    async fn egress_enabled_tracks_web_presence() {
        let dir = tempdir().unwrap();
        let mcp = std::sync::Arc::new(MockMcpBackend::new());
        let no_web = exec_with(dir.path(), mcp.clone());
        assert!(!no_web.egress_enabled(), "no key -> egress off");
        let fs = FsBackend::new(dir.path()).unwrap();
        let with_web = RealExecutor::new(mcp, fs, Some(ExaBackend::new("test-key").unwrap()));
        assert!(with_web.egress_enabled(), "key present -> egress on");
    }

    // --- Exa request building + response parsing ------------------------------

    #[test]
    fn exa_fetch_request_is_well_formed() {
        let exa = ExaBackend::new("secret-key").unwrap();
        let req = exa.build_fetch_request("https://example.com/page");
        assert_eq!(req.url, "https://api.exa.ai/contents");
        assert_eq!(req.api_key_header, "secret-key");
        assert_eq!(req.body["urls"], json!(["https://example.com/page"]));
        assert_eq!(req.body["text"], json!(true));
        assert_eq!(req.body["livecrawl"], json!("always"));
    }

    #[test]
    fn exa_search_request_is_well_formed() {
        let exa = ExaBackend::new("secret-key").unwrap();
        let req = exa.build_search_request("rust async traits");
        assert_eq!(req.url, "https://api.exa.ai/search");
        assert_eq!(req.api_key_header, "secret-key");
        assert_eq!(req.body["query"], json!("rust async traits"));
        assert_eq!(req.body["type"], json!("neural"));
        assert_eq!(req.body["numResults"], json!(EXA_NUM_RESULTS));
        assert_eq!(req.body["contents"]["text"], json!(true));
    }

    #[test]
    fn exa_empty_key_is_rejected() {
        assert!(ExaBackend::new("   ").is_err());
    }

    #[test]
    fn exa_request_debug_redacts_the_api_key() {
        // FIX 5: the manual Debug must NEVER print the API key.
        let exa = ExaBackend::new("super-secret-exa-key").unwrap();
        let req = exa.build_search_request("q");
        // The key is still accessible to assertions (tests rely on it)...
        assert_eq!(req.api_key_header, "super-secret-exa-key");
        // ...but Debug renders it redacted.
        let dbg = format!("{req:?}");
        assert!(
            !dbg.contains("super-secret-exa-key"),
            "Debug must not leak the key: {dbg}"
        );
        assert!(
            dbg.contains("<redacted>"),
            "Debug marks the key redacted: {dbg}"
        );
    }

    #[test]
    fn body_cap_stops_at_the_cap_across_chunks() {
        // FIX 2: the accumulation helper (the load-bearing cap logic behind
        // `read_body_capped`) must never buffer more than `cap` bytes, even when a
        // single chunk overshoots, and must signal "full" so the stream stops.
        let cap = 10usize;
        let mut buf = Vec::new();

        // A small first chunk fits, not yet full.
        assert!(!accumulate_capped(&mut buf, b"abc", cap));
        assert_eq!(buf, b"abc");

        // A chunk that overshoots is truncated to the remaining room and reports full.
        assert!(accumulate_capped(&mut buf, b"defghijklmnop", cap));
        assert_eq!(buf.len(), cap, "buffer never exceeds the cap");
        assert_eq!(&buf, b"abcdefghij");

        // Once full, further chunks are ignored and it stays full.
        assert!(accumulate_capped(&mut buf, b"more", cap));
        assert_eq!(buf.len(), cap, "buffer stays at the cap, never grows");
    }

    #[test]
    fn body_cap_exact_fit_reports_full() {
        // A chunk that EXACTLY reaches the cap reports full so we stop early.
        let cap = 4usize;
        let mut buf = Vec::new();
        assert!(accumulate_capped(&mut buf, b"abcd", cap));
        assert_eq!(buf, b"abcd");
    }

    #[test]
    fn parse_exa_search_fixture_to_compact_text() {
        // A captured-shape /search response.
        let fixture = r#"{
            "results": [
                {"title": "First", "url": "https://a.test", "text": "alpha body"},
                {"title": "Second", "url": "https://b.test", "text": "beta body"}
            ]
        }"#;
        let out = parse_exa_response(fixture);
        assert!(out.contains("[1] First"), "first header: {out}");
        assert!(out.contains("https://a.test"), "first url: {out}");
        assert!(out.contains("alpha body"), "first text: {out}");
        assert!(out.contains("[2] Second"), "second header: {out}");
        assert!(out.contains("beta body"), "second text: {out}");
    }

    #[test]
    fn parse_exa_empty_results_is_marked() {
        let out = parse_exa_response(r#"{"results": []}"#);
        assert_eq!(out, "[exa: 0 results]");
    }

    #[test]
    fn parse_exa_pages_summary_precedence_and_totality() {
        // summary wins; else highlights joined; else a text snippet; bad JSON -> empty.
        let fixture = r#"{
            "results": [
                {"title": "Sum", "url": "https://s.test", "summary": "the summary", "text": "ignored", "highlights": ["h"]},
                {"title": "Hi", "url": "https://h.test", "highlights": ["one", "two"], "text": "ignored"},
                {"title": "Txt", "url": "https://t.test", "text": "fallback body"},
                {"title": "", "url": "https://u.test"}
            ]
        }"#;
        let pages = parse_exa_pages(fixture);
        assert_eq!(pages.len(), 4);
        assert_eq!(pages[0].summary, "the summary");
        assert_eq!(pages[1].summary, "one … two");
        assert_eq!(pages[2].summary, "fallback body");
        // empty title falls back to the url
        assert_eq!(pages[3].title, "https://u.test");
        // total: invalid / no-results -> empty, never panics
        assert!(parse_exa_pages("not json").is_empty());
        assert!(parse_exa_pages(r#"{"foo": 1}"#).is_empty());
        // cap: at most 6 pages
        let many = format!(
            r#"{{"results": [{}]}}"#,
            (0..10)
                .map(|i| format!(r#"{{"url": "https://x{i}.test", "summary": "s{i}"}}"#))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert_eq!(parse_exa_pages(&many).len(), 6);
    }

    #[test]
    fn parse_exa_non_json_falls_back_to_capped_raw() {
        let out = parse_exa_response("not json at all");
        assert!(out.contains("not json"), "raw fallback: {out}");
    }

    #[test]
    fn elide_to_caps_on_chars_not_bytes() {
        // FIX 7: a multibyte string must be capped by CHAR count (matching the
        // loop's cap_result), never by bytes. 5 multibyte chars under a 10-char cap
        // pass through untouched even though they exceed 10 BYTES.
        let multibyte = "élève"; // 5 chars, > 5 bytes
        assert_eq!(
            elide_to(multibyte, 10),
            multibyte,
            "5 chars under a 10-char cap is untouched"
        );

        // Over the char cap: keep exactly `cap` chars (a clean char boundary), mark cut.
        let long = "αβγδεζηθικ"; // 10 Greek chars, 20 bytes
        let out = elide_to(long, 4);
        assert!(out.starts_with("αβγδ"), "kept the first 4 chars: {out}");
        assert!(out.contains("[…truncated]"), "marks truncation: {out}");
        assert_eq!(
            out.chars().filter(|c| ('α'..='κ').contains(c)).count(),
            4,
            "exactly 4 source chars retained: {out}"
        );
    }
}
