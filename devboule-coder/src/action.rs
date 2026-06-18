//! The emit-structured-action protocol (L2.2).
//!
//! The model talks to the burst loop ([`crate::agent_loop`]) by emitting EXACTLY
//! ONE fenced action block per turn:
//!
//! ```text
//! ```action
//! {"tool": "oracle_ask", "query": "where is the launch path"}
//! ```
//! ```
//!
//! [`AgentAction`] is the full action vocabulary (serde, internally tagged on the
//! `"tool"` field). Dispatch for most variants is STUBBED in L2.2 (see
//! [`crate::agent_loop::StubExecutor`]); the real MCP-backed executor lands in
//! L2.3 behind the same [`crate::agent_loop::ToolExecutor`] seam.
//!
//! [`parse_action`] enforces mini-swe-agent's format discipline: extract the
//! fenced blocks, require EXACTLY ONE, parse it, then STRICTLY validate the
//! arguments. Any deviation becomes a [`FormatError`] carrying a precise,
//! model-facing message the loop feeds back so the model can self-correct — we
//! never silently dispatch a malformed or unsafe action.

use std::path::{Component, Path};
use std::sync::OnceLock;

use regex::Regex;
use serde::Deserialize;

// --- Validation caps ---------------------------------------------------------
// Sane bounds so a runaway model cannot blow up the transcript or smuggle a huge
// argument into a (future) tool. These are content-length caps in CHARS (not
// bytes): we count `chars()` so a multibyte string is judged by its visible
// length, and the comparison can never split a UTF-8 boundary.

/// Max length (chars) of a free-text query / task / reply / question argument.
pub const MAX_TEXT_LEN: usize = 4096;
/// Max number of files a single `spawn_mini` may name (the READ arm).
pub const MAX_FILES: usize = 32;
/// Max number of files a single WRITE `spawn_mini` may name. Tighter than
/// [`MAX_FILES`]: the Oracle server REJECTS a `write=true` `spawn_mini_coder`
/// naming more than this, so we cap at the same bound at PARSE time to give the
/// model immediate, self-correctable feedback instead of a deep server error.
pub const MAX_WRITE_FILES: usize = 10;
/// Max length (chars) of a single path string. Keeps a pathological path from
/// dominating the transcript even before component validation runs.
pub const MAX_PATH_LEN: usize = 1024;
/// Max length (chars) of a `grep` regex pattern. Tighter than [`MAX_TEXT_LEN`]: a
/// regex is compiled (so it must be small and bounded) and a 4 KB pattern is
/// already pathological. The pattern is also compiled with the `regex` crate's
/// NFA engine in [`AgentAction::validate`], which rejects un-compilable input and
/// guarantees the L2.3 search path is ReDoS-free.
pub const MAX_GREP_PATTERN_LEN: usize = 512;

/// The structured action the model emits each turn.
///
/// Internally tagged on `"tool"`, so the wire form is a flat object:
/// `{"tool": "read", "path": "src/main.rs"}`. `deny_unknown_fields` makes a typo'd
/// or extra key a hard parse error (surfaced as [`FormatError::Invalid`]) rather
/// than being silently ignored — the model gets feedback instead of a wrong
/// dispatch.
// NOTE: `Eq` is intentionally NOT derived. The `McpTool { params: serde_json::Value }`
// field carries an arbitrary JSON value, and `serde_json::Value` is `PartialEq` but NOT
// `Eq` (it can hold an `f64`). `PartialEq` is all the codebase needs (assert_eq! in
// tests, the no-progress guard compares `(tool_name, target)` strings, not the action),
// so dropping `Eq` is sound and keeps the variant's params unconstrained.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentAction {
    /// PRIMARY: ask the private, grounded oracle a question.
    OracleAsk { query: String },
    /// PRIMARY: pull grounded context for a query (optionally limited).
    OracleContext {
        query: String,
        #[serde(default)]
        limit: Option<u32>,
    },
    /// Lay out a plan as ordered steps.
    Plan { steps: Vec<String> },
    /// EXECUTE the approved plan (Phase 11.3): run the active plan's tasks straight off
    /// the project KANBAN (the single shared task store) — each ready task delegated to a
    /// mini under the Censor gate, in dependency order — until all are done/in-review or
    /// one is BLOCKED. No arguments: the runner auto-detects the active plan on the board
    /// (the tasks the prior `plan` action created + the human approved).
    ///
    /// Modeled as an EMPTY STRUCT variant (`{}`), NOT a unit variant: serde's
    /// `deny_unknown_fields` is silently NOT enforced for a UNIT variant of an internally
    /// tagged enum, so `{"tool":"run_plan","junk":1}` would parse and swallow `junk` with
    /// no FORMAT ERROR feedback. A struct variant restores the strict-reject contract.
    RunPlan {},
    /// Spawn a mini sub-agent on a scoped task over `files`. `write` selects the
    /// edit arm (default read-only).
    SpawnMini {
        task: String,
        files: Vec<String>,
        #[serde(default)]
        write: bool,
    },
    /// Read a file by relative path.
    Read { path: String },
    /// Grep a pattern, optionally restricted to a glob.
    Grep {
        pattern: String,
        #[serde(default)]
        glob: Option<String>,
    },
    /// Expand a glob to matching paths.
    Glob { pattern: String },
    /// EGRESS: fetch a URL.
    Fetch { url: String },
    /// EGRESS: run a web search.
    Websearch { query: String },
    /// Call a tool on a USER-configured MCP server (Phase B). `server` is the routing
    /// key (a configured user-server name); `tool` is the tool to call on it; `params`
    /// is the tool's argument object. User MCP servers are external child processes the
    /// user OPTED INTO at config time, so a `mcp_tool` call is its OWN capability —
    /// DECOUPLED from the WEB-egress gate (`fetch`/`websearch`): a user may call a
    /// KNOWN configured server even with web egress OFF. Its gate is the
    /// KNOWN-SERVER set (the server must be configured/connected), enforced at PARSE
    /// time (see [`AgentAction::validate_with_servers`]) so an unknown server is
    /// rejected with immediate feedback rather than a late call-time error. Routed by
    /// the `MultiMcpBackend` to the named user backend.
    McpTool {
        server: String,
        // The wire key is `name` (NOT `tool`): the enum is internally tagged on
        // `"tool"` (the action TYPE), so the tool-to-call must use a different key or
        // serde rejects the `tool` field as colliding with the tag. The Rust field
        // stays `tool` for readability.
        #[serde(rename = "name")]
        tool: String,
        params: serde_json::Value,
    },
    /// TERMINAL: hand back to the human with a question.
    AskUser { question: String },
    /// TERMINAL: the final answer to the human.
    Done { reply: String },
    /// TERMINAL: give up this burst with a reason.
    Escalate { reason: String },
}

impl AgentAction {
    /// `true` for actions that leave the machine via the PUBLIC WEB through the
    /// external provider (`fetch`/`websearch`). The loop annotates these distinctly in
    /// the progress stream and STRUCTURALLY gates them on `allow_egress` (the Exa key).
    /// `oracle_*` is PRIVATE and grounded, so it is deliberately NOT egress.
    ///
    /// `mcp_tool` is deliberately NOT web-egress (design §5.2, decoupled): a user MCP
    /// server is a capability the user OPTED INTO at config time, separate from the
    /// web-search opt-in. It is gated instead by the KNOWN-SERVER set in
    /// [`AgentAction::validate_with_servers`] (the server must be configured/connected),
    /// so a configured server is callable even when web egress is OFF. Keeping it out of
    /// this set is what lets the `run_burst` gate (`is_egress() && !allow_egress`) block
    /// the web tools WITHOUT blocking user-MCP.
    pub fn is_egress(&self) -> bool {
        matches!(
            self,
            AgentAction::Fetch { .. } | AgentAction::Websearch { .. }
        )
    }

    /// A short stable name for the tool (for progress lines and the no-progress
    /// guard). Matches the wire `"tool"` tag.
    pub fn tool_name(&self) -> &'static str {
        match self {
            AgentAction::OracleAsk { .. } => "oracle_ask",
            AgentAction::OracleContext { .. } => "oracle_context",
            AgentAction::Plan { .. } => "plan",
            AgentAction::RunPlan {} => "run_plan",
            AgentAction::SpawnMini { .. } => "spawn_mini",
            AgentAction::Read { .. } => "read",
            AgentAction::Grep { .. } => "grep",
            AgentAction::Glob { .. } => "glob",
            AgentAction::Fetch { .. } => "fetch",
            AgentAction::Websearch { .. } => "websearch",
            AgentAction::McpTool { .. } => "mcp_tool",
            AgentAction::AskUser { .. } => "ask_user",
            AgentAction::Done { .. } => "done",
            AgentAction::Escalate { .. } => "escalate",
        }
    }

    /// The action's primary target, used by the loop's no-progress guard to
    /// detect a repeated `(tool, target)`. Empty when the action has no natural
    /// single target (e.g. `plan`).
    pub fn target(&self) -> String {
        match self {
            AgentAction::OracleAsk { query } => query.clone(),
            AgentAction::OracleContext { query, .. } => query.clone(),
            AgentAction::Plan { steps } => steps.join(" | "),
            // No natural single target — it operates on the persisted plan. The empty
            // target means two `run_plan` in ONE burst trip the no-progress guard, which
            // is correct: the flow is run_plan → done|ask_user (which ends the burst), so
            // a re-run happens in a FRESH burst (fresh window); an in-burst repeat is a
            // genuine spin worth escalating.
            AgentAction::RunPlan {} => String::new(),
            AgentAction::SpawnMini { task, .. } => task.clone(),
            AgentAction::Read { path } => path.clone(),
            AgentAction::Grep { pattern, .. } => pattern.clone(),
            AgentAction::Glob { pattern } => pattern.clone(),
            AgentAction::Fetch { url } => url.clone(),
            AgentAction::Websearch { query } => query.clone(),
            // The target is `server.tool` PLUS a compact form of `params`, so the
            // no-progress guard catches a truly identical repeated call but does NOT
            // falsely flag the same tool invoked with DIFFERENT arguments (e.g. a
            // paginated query). `params` is canonical-ish (serde_json renders object
            // keys in insertion order; identical JSON ⇒ identical string here, which is
            // all the guard needs — it compares for equality within one burst).
            AgentAction::McpTool {
                server,
                tool,
                params,
            } => format!("{server}.{tool} {params}"),
            AgentAction::AskUser { question } => question.clone(),
            AgentAction::Done { reply } => reply.clone(),
            AgentAction::Escalate { reason } => reason.clone(),
        }
    }

    /// STRICT post-parse validation. Returns a precise, model-facing message on
    /// the first violation so the loop can feed it back. Enforces text-length
    /// caps, the files-count cap, and that every path argument is a SAFE relative
    /// path (no absolute, no `..`, no `-`-leading component).
    ///
    /// `known_servers` is the set of CONFIGURED user-MCP server names (loaded at
    /// startup). It is consulted ONLY by the [`AgentAction::McpTool`] arm: a call
    /// naming a server NOT in this set is rejected here, at parse time, so the model
    /// gets an immediate `FormatError::Invalid` instead of a late call-time error.
    /// For every other action the set is irrelevant. When no user servers are
    /// configured the set is empty, so any `mcp_tool` is rejected.
    fn validate_with_servers(&self, known_servers: &[String]) -> Result<(), String> {
        match self {
            AgentAction::OracleAsk { query } => check_text("query", query),
            AgentAction::OracleContext { query, .. } => check_text("query", query),
            AgentAction::Plan { steps } => {
                if steps.is_empty() {
                    return Err("plan requires at least one step".to_string());
                }
                for step in steps {
                    check_text("step", step)?;
                }
                Ok(())
            }
            // No arguments to validate — it runs the already-persisted, already-approved plan.
            AgentAction::RunPlan {} => Ok(()),
            AgentAction::SpawnMini { task, files, write } => {
                check_text("task", task)?;
                if files.is_empty() {
                    return Err("files must not be empty".to_string());
                }
                // The WRITE arm is capped TIGHTER than the read arm: the Oracle
                // server rejects a `write=true` spawn naming more than
                // MAX_WRITE_FILES, so mirror that bound here for parse-time feedback
                // instead of a deep server error. The read arm keeps the 32 cap.
                if *write && files.len() > MAX_WRITE_FILES {
                    return Err(format!(
                        "write spawn_mini: at most {MAX_WRITE_FILES} files (split the task)"
                    ));
                }
                if files.len() > MAX_FILES {
                    return Err(format!("too many files: {} (max {MAX_FILES})", files.len()));
                }
                for f in files {
                    check_rel_path("files entry", f)?;
                }
                Ok(())
            }
            AgentAction::Read { path } => check_rel_path("path", path),
            AgentAction::Grep { pattern, glob } => {
                check_regex("pattern", pattern)?;
                if let Some(g) = glob {
                    check_glob_pattern("glob", g)?;
                }
                Ok(())
            }
            AgentAction::Glob { pattern } => check_glob_pattern("pattern", pattern),
            AgentAction::Fetch { url } => {
                check_text("url", url)?;
                check_url(url)
            }
            AgentAction::Websearch { query } => check_text("query", query),
            AgentAction::McpTool {
                server,
                tool,
                params,
            } => check_mcp_tool(server, tool, params, known_servers),
            AgentAction::AskUser { question } => check_text("question", question),
            AgentAction::Done { reply } => check_text("reply", reply),
            AgentAction::Escalate { reason } => check_text("reason", reason),
        }
    }
}

/// Validate an [`AgentAction::McpTool`] (design §2.4). All three checks give the
/// model a precise, self-correctable message at PARSE time:
/// * `server` non-empty AND in the configured `known_servers` set — an unknown
///   server (a typo, or one the user never configured) is rejected here instead of
///   reaching the dispatcher as a late call-time error;
/// * `tool` non-empty and within the standard [`MAX_TEXT_LEN`] text bound;
/// * `params` is a JSON OBJECT — a scalar/array top-level is rejected (the MCP
///   `call_tool` contract takes a named-argument object; a non-object would also be
///   rejected by the backend, so we reject it early).
fn check_mcp_tool(
    server: &str,
    tool: &str,
    params: &serde_json::Value,
    known_servers: &[String],
) -> Result<(), String> {
    if server.trim().is_empty() {
        return Err("`server` must not be empty".to_string());
    }
    // Known-server gate: the set is the CONFIGURED user-server names. An exact match
    // is required (the names are validated at config time to a safe charset, so a
    // plain `==` is the right comparison). Empty set ⇒ no user servers ⇒ always
    // rejected, which is correct (a no-user-servers launch must never run mcp_tool).
    if !known_servers.iter().any(|s| s == server) {
        return Err(format!(
            "unknown MCP server `{server}` — it is not a configured user MCP server"
        ));
    }
    check_text("tool", tool)?;
    // `params` MUST be a JSON object: the MCP call_tool contract takes a named-arg
    // map. Reject a scalar / array / null top-level with a precise message.
    if !params.is_object() {
        return Err("`params` must be a JSON object".to_string());
    }
    Ok(())
}

/// A free-text field must be non-empty and within the char cap.
fn check_text(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("`{field}` must not be empty"));
    }
    let len = value.chars().count();
    if len > MAX_TEXT_LEN {
        return Err(format!(
            "`{field}` too long: {len} chars (max {MAX_TEXT_LEN})"
        ));
    }
    Ok(())
}

/// Validate a relative path argument: non-empty, within the length cap, and a
/// SAFE relative path. Mirrors the censor/mini_coder discipline:
/// slash-normalize first so a backslash-separated `..` is caught on every OS,
/// reject a `DOS` drive prefix (`C:\...`) that `Path::components` would not flag
/// on a non-Windows host, then walk components rejecting `..`, root/prefix, and
/// any `-`-leading piece (argv-injection guard: a future tool may hand the path
/// to a CLI where a `-`-leading name reads as a flag).
pub(crate) fn check_rel_path(field: &str, raw: &str) -> Result<(), String> {
    if raw.trim().is_empty() {
        return Err(format!("`{field}` must not be empty"));
    }
    if raw.chars().count() > MAX_PATH_LEN {
        return Err(format!(
            "`{field}` path too long (max {MAX_PATH_LEN} chars)"
        ));
    }

    let normalized = raw.replace('\\', "/");

    // Catch a `C:`-style drive prefix explicitly: on a unix host `Path` parses
    // `C:\x` as one Normal component, so the Prefix arm below never fires for it.
    let mut head = normalized.bytes();
    if let (Some(first), Some(b':')) = (head.next(), head.next()) {
        if first.is_ascii_alphabetic() {
            return Err(format!("`{field}` must be relative, got absolute: {raw}"));
        }
    }

    let path = Path::new(&normalized);
    for component in path.components() {
        match component {
            Component::ParentDir => {
                return Err(format!("`{field}` must not contain '..': {raw}"));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("`{field}` must be relative, got absolute: {raw}"));
            }
            Component::Normal(name) => {
                // `Path::components` has already split on the separators, so this
                // component is a single name; check it directly for a leading '-'
                // (argv-injection guard).
                if name.to_string_lossy().starts_with('-') {
                    return Err(format!(
                        "`{field}` component must not start with '-': {raw}"
                    ));
                }
            }
            Component::CurDir => {}
        }
    }
    Ok(())
}

/// SSRF / scheme defense-in-depth for a `fetch` URL, applied at PARSE time so the
/// model gets feedback and a bad URL never propagates — even though egress is
/// also gated structurally by the loop (W7) and the real fetch goes via a remote
/// provider (L2.3). Deliberately a simple string check (no URL-parsing crate):
/// * scheme = text before the first `:`, lowercased; only `http`/`https` allowed,
/// * host = text between `://` and the next `/`, `:`, `?`, or `#`; reject the
///   obvious internal targets (loopback, unspecified, link-local cloud-metadata).
fn check_url(url: &str) -> Result<(), String> {
    let url = url.trim();

    // Scheme: everything before the first ':'. Reject anything but http/https.
    let scheme = match url.split_once(':') {
        Some((s, _)) => s.to_ascii_lowercase(),
        None => return Err("url has no scheme; use https:// or http://".to_string()),
    };
    if scheme != "https" && scheme != "http" {
        return Err(format!(
            "url scheme `{scheme}` not allowed; use https:// or http://"
        ));
    }

    // Host: between `://` and the next delimiter. A bracketed IPv6 literal
    // (`[::1]:8080`) carries its own colons, so when the authority starts with
    // `[` we take everything up to the closing `]`; otherwise we split on the
    // first `/`, `:`, `?`, or `#`. The brackets are then stripped so `[::1]` and
    // `::1` normalise to the same blocked host.
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or("");

    // SSRF via userinfo: `http://evil.com@127.0.0.1/` puts the REAL host after the
    // `@`, but the naive host-span extraction below would take `evil.com@127.0.0.1`
    // (or `evil.com`), miss the blocklist, and reqwest would then connect to the
    // post-`@` target. Reject ANY `@` in the AUTHORITY (the span before the first
    // `/ ? #`) outright — there is no legitimate need for userinfo in a fetch URL.
    // Mirrors `model_client::validate_omlx_base_url`, which rejects `@` the same way.
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err("url must not contain userinfo (@ in authority)".to_string());
    }

    let host_raw = if let Some(rest) = after_scheme.strip_prefix('[') {
        // Up to and including the closing bracket (e.g. `[::1]`).
        match rest.split_once(']') {
            Some((inner, _)) => &after_scheme[..inner.len() + 2],
            None => after_scheme, // malformed; keep whole, the check below still runs
        }
    } else {
        after_scheme
            .split(['/', ':', '?', '#'])
            .next()
            .unwrap_or("")
    };
    let host = host_raw
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();

    // IP-aware block: PARSE the host as an IP and reject the whole loopback /
    // unspecified ranges, not just specific literals. A plain string blocklist
    // only catches `127.0.0.1`, so `127.0.0.2` / `127.1.2.3` (all of 127/8) and
    // `::ffff:127.x` (IPv4-mapped loopback) slip through — exactly the SSRF holes
    // we must close here, at parse time, before a fetch is ever dispatched.
    if let Ok(v4) = host.parse::<std::net::Ipv4Addr>() {
        // `is_loopback()` covers all of 127.0.0.0/8; also block the unspecified
        // 0.0.0.0 and the cloud-metadata link-local address.
        if v4.is_loopback()
            || v4.is_unspecified()
            || v4 == std::net::Ipv4Addr::new(169, 254, 169, 254)
        {
            return Err(format!(
                "url host `{host_raw}` is not allowed (internal/loopback)"
            ));
        }
    } else if let Ok(v6) = host.parse::<std::net::Ipv6Addr>() {
        // `::1` (loopback) and `::` (unspecified) directly; plus the IPv4-mapped
        // form `::ffff:a.b.c.d`, which `is_loopback()` does NOT flag, so unwrap the
        // embedded v4 and re-apply the v4 rules.
        let mapped_blocked = v6.to_ipv4_mapped().is_some_and(|v4| {
            v4.is_loopback()
                || v4.is_unspecified()
                || v4 == std::net::Ipv4Addr::new(169, 254, 169, 254)
        });
        if v6.is_loopback() || v6.is_unspecified() || mapped_blocked {
            return Err(format!(
                "url host `{host_raw}` is not allowed (internal/loopback)"
            ));
        }
    }

    // Non-IP hosts (and any IP not caught above): keep the literal string checks.
    // `localhost` is a name, not an IP, so the IP parse above never sees it; the
    // bracketed `[::1]` was normalised to `::1` and is already caught as an IPv6
    // loopback, but it stays listed for clarity/defence in depth.
    const BLOCKED_HOSTS: [&str; 5] = [
        "localhost",
        "127.0.0.1",
        "0.0.0.0",
        "::1",
        "169.254.169.254",
    ];
    if BLOCKED_HOSTS.contains(&host.as_str()) {
        return Err(format!(
            "url host `{host_raw}` is not allowed (internal/loopback)"
        ));
    }

    Ok(())
}

/// Validate a `grep` regex: non-empty, within the (tighter) grep cap, and one
/// that actually COMPILES with the `regex` crate. Compiling here gives the model
/// precise feedback on a bad pattern AND guarantees the L2.3 search path uses the
/// `regex` NFA engine (linear-time, no catastrophic backtracking / ReDoS).
fn check_regex(field: &str, pattern: &str) -> Result<(), String> {
    if pattern.trim().is_empty() {
        return Err(format!("`{field}` must not be empty"));
    }
    let len = pattern.chars().count();
    if len > MAX_GREP_PATTERN_LEN {
        return Err(format!(
            "`{field}` too long: {len} chars (max {MAX_GREP_PATTERN_LEN})"
        ));
    }
    Regex::new(pattern)
        .map(|_| ())
        .map_err(|e| format!("`{field}` is not a valid regex: {e}"))
}

/// Validate a glob pattern argument (`glob.pattern`, `grep.glob`). It is NOT a
/// plain path (metacharacters `* ? [ ] { }` are allowed inside a segment), but it
/// must not ESCAPE the workspace: reject an absolute/rooted/drive-prefixed
/// pattern and any segment that is exactly `..`. Slash-normalise first so a
/// backslash-separated `..` is caught on every OS.
fn check_glob_pattern(field: &str, pattern: &str) -> Result<(), String> {
    if pattern.trim().is_empty() {
        return Err(format!("`{field}` must not be empty"));
    }
    if pattern.chars().count() > MAX_PATH_LEN {
        return Err(format!("`{field}` too long (max {MAX_PATH_LEN} chars)"));
    }

    let normalized = pattern.replace('\\', "/");

    // Absolute (leading `/`) or a `C:`-style drive prefix escapes the workspace.
    if normalized.starts_with('/') {
        return Err(format!(
            "`{field}` must be relative, got absolute: {pattern}"
        ));
    }
    let mut head = normalized.bytes();
    if let (Some(first), Some(b':')) = (head.next(), head.next()) {
        if first.is_ascii_alphabetic() {
            return Err(format!(
                "`{field}` must be relative, got absolute: {pattern}"
            ));
        }
    }

    // A `..` path component (not a `..` buried inside a longer segment like `a..b`)
    // is traversal. We split on `/` ourselves rather than via `Path::components`,
    // which would normalise away glob metacharacters.
    for segment in normalized.split('/') {
        if segment == ".." {
            return Err(format!("`{field}` must not contain '..': {pattern}"));
        }
    }
    Ok(())
}

/// Why a model turn failed the format contract. Each variant carries the exact
/// guidance the loop feeds back to the model so it can self-correct on the next
/// round. The messages are deliberately imperative and specific.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// No ```action``` block found.
    Missing,
    /// More than one ```action``` block found.
    TooMany(usize),
    /// Exactly one block, but its JSON is malformed, names an unknown tool, or
    /// fails strict argument validation. Carries the precise reason.
    Invalid(String),
}

impl FormatError {
    /// The precise, model-facing feedback string the loop appends to the
    /// transcript so the model can correct itself next round.
    pub fn feedback(&self) -> String {
        match self {
            FormatError::Missing => "FORMAT ERROR: no action found. Emit EXACTLY ONE \
                fenced ```action``` block containing a single JSON object, e.g.\n\
                ```action\n{\"tool\": \"oracle_ask\", \"query\": \"...\"}\n```"
                .to_string(),
            FormatError::TooMany(n) => format!(
                "FORMAT ERROR: found {n} action blocks. Emit EXACTLY ONE \
                ```action``` block per turn."
            ),
            FormatError::Invalid(msg) => format!(
                "FORMAT ERROR: the action block is invalid: {msg}. Fix the JSON and \
                emit EXACTLY ONE valid ```action``` block."
            ),
        }
    }
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.feedback())
    }
}

impl std::error::Error for FormatError {}

/// Matches a fenced ```action``` block and captures its inner body. Anchored to
/// line starts (`(?m)^`) so an indented or inline ```` ```action ```` inside
/// prose is not mistaken for a directive — the model must emit the fence at the
/// start of a line, exactly as instructed. The body capture is non-greedy and
/// `(?s)` lets it span newlines, so multi-line JSON is captured whole up to the
/// next closing fence.
fn action_re() -> &'static Regex {
    static ACTION_RE: OnceLock<Regex> = OnceLock::new();
    ACTION_RE.get_or_init(|| {
        Regex::new(r"(?ms)^```action[ \t]*\r?\n(.*?)\r?\n?^```[ \t]*$")
            .expect("static regex is valid")
    })
}

/// Anti-evasion fence-depth scan, independent of the capture regex above.
///
/// A model can WRAP a well-formed block inside an OUTER fence:
/// ```text
/// ```json
/// ```action
/// {"tool":"fetch","url":"..."}
/// ```
/// ```
/// The inner ```` ```action ```` opens and the inner ```` ``` ```` closes the
/// captured block early, so [`action_re`]'s `captures_iter` finds exactly ONE
/// block and the wrapped action would be dispatched — slipping past the count
/// guard. The capture regex alone cannot tell a TOP-LEVEL block from a nested
/// one, so we walk the fences ourselves, tracking open/close depth, and count
/// ```` ```action ```` openers that occur at the TOP LEVEL versus anywhere.
///
/// Returns `(top_level, total)`:
/// * `top_level` — ```` ```action ```` openers NOT inside another open fence,
/// * `total` — every ```` ```action ```` opener, including nested ones.
///
/// A legitimate single block is `(1, 1)`; the wrapper above is `(0, 1)`; two
/// real blocks are `(2, 2)`. [`parse_action`] requires `(1, 1)` to dispatch.
///
/// CommonMark closing rule (load-bearing for the anti-evasion): a fenced code
/// block is opened by a ```` ``` ```` line WITH an info string and CLOSES only on a
/// later ```` ``` ```` line with NO info string. An info-string fence line that
/// appears WHILE a block is already open is literal CONTENT, not a new fence. A
/// naive open/close TOGGLE on every fence line is defeated by even parity: two
/// UNCLOSED prose openers (```` ```json ````, ```` ```yaml ````) would toggle the
/// state back to "outside", so the following ```` ```action ```` would be miscounted
/// as TOP-LEVEL and dispatched. Tracking the real open/closed state (only a bare
/// fence closes) keeps that ```` ```action ```` recognised as nested -> `top_level == 0`.
///
/// We track whether we are currently inside ANY open fence. An ```` ```action ````
/// line ALWAYS counts toward `total`; it counts toward `top_level` ONLY when not
/// currently inside another open fence.
fn count_action_fences(input: &str) -> (usize, usize) {
    /// The info string of a fence line, or `None` if the line is not a fence.
    /// Empty (`Some("")`) means a BARE fence (no info string) -> a valid closer.
    fn fence_info(line: &str) -> Option<&str> {
        let t = line.trim_end_matches(['\r', ' ', '\t']);
        t.strip_prefix("```")
            .map(|rest| rest.split([' ', '\t']).next().unwrap_or(""))
    }

    let mut inside_fence = false;
    let mut top_level = 0usize;
    let mut total = 0usize;
    for line in input.lines() {
        let Some(info) = fence_info(line) else {
            continue;
        };
        if inside_fence {
            // Inside an open block: a BARE fence (no info string) CLOSES it; an
            // info-string fence line is literal CONTENT (NOT a new fence and NOT a
            // close). A nested ```` ```action ```` here is the wrapper case: count it
            // toward `total` but NOT `top_level`, and stay inside (it does not open
            // a real new block in CommonMark).
            if info.is_empty() {
                inside_fence = false;
            } else if info == "action" {
                total += 1;
            }
        } else {
            // Outside any block: an info-string fence OPENS a block (a bare fence
            // here opens an info-less block, equally). An ```` ```action ```` opener at
            // this level is genuinely TOP-LEVEL.
            if info == "action" {
                top_level += 1;
                total += 1;
            }
            inside_fence = true;
        }
    }
    (top_level, total)
}

/// Parse the model's raw output into exactly one [`AgentAction`].
///
/// Discipline (mini-swe-agent style):
/// - ZERO ```action``` blocks -> [`FormatError::Missing`].
/// - MORE THAN ONE block -> [`FormatError::TooMany`].
/// - Exactly one, but malformed JSON / unknown tool / unknown field ->
///   [`FormatError::Invalid`] with serde's message.
/// - Exactly one, parses, but fails strict argument validation ->
///   [`FormatError::Invalid`] with the validation message.
///
/// On success the returned action is already validated, so the caller may
/// dispatch it directly.
///
/// This is the zero-user-servers form: it validates against an EMPTY known-server
/// set, so any [`AgentAction::McpTool`] is rejected. The burst loop calls
/// [`parse_action_with_servers`] with the configured user-server names; every other
/// (test / non-burst) call site keeps using this unchanged.
// The burst loop now calls `parse_action_with_servers`; this convenience wrapper is
// exercised only by the unit tests across the crate, so it reads as dead code in a
// non-test build of this binary crate. Kept as the documented zero-servers entry point.
#[cfg_attr(not(test), allow(dead_code))]
pub fn parse_action(model_output: &str) -> Result<AgentAction, FormatError> {
    parse_action_with_servers(model_output, &[])
}

/// Parse the model's raw output into exactly one [`AgentAction`], validating any
/// [`AgentAction::McpTool`] against the configured user-MCP server names in
/// `known_servers`. Identical to [`parse_action`] except for the `mcp_tool`
/// known-server gate (see [`AgentAction::validate_with_servers`]). The burst loop
/// uses this so an unknown server name is an immediate `FormatError::Invalid`.
pub fn parse_action_with_servers(
    model_output: &str,
    known_servers: &[String],
) -> Result<AgentAction, FormatError> {
    // Anti-evasion fence scan, independent of the capture regex. A model can WRAP
    // a well-formed block inside an OUTER fence so the inner closing ``` ends the
    // captured block early and `captures_iter` sees exactly ONE — bypassing the
    // count guard and dispatching a wrapped action. Reconcile against a real
    // fence-depth walk: dispatch ONLY when there is exactly one TOP-LEVEL action
    // opener and NO nested ones (so top_level == total == 1).
    let (top_level, total) = count_action_fences(model_output);
    if total == 0 {
        return Err(FormatError::Missing);
    }
    if top_level != 1 || total != 1 {
        // Nested/wrapped (top_level == 0, total >= 1) or genuinely multiple blocks
        // (top_level >= 2). Tell the model precisely what is wrong: one block,
        // never nested inside another fence.
        if top_level == 0 {
            return Err(FormatError::Invalid(
                "the ```action``` block must not be nested inside another fenced \
                 block; emit EXACTLY ONE top-level ```action``` block"
                    .to_string(),
            ));
        }
        return Err(FormatError::TooMany(top_level.max(total)));
    }

    let mut blocks = action_re().captures_iter(model_output);
    let first = blocks.next().ok_or(FormatError::Missing)?;

    let body = first.get(1).map(|m| m.as_str().trim()).unwrap_or_default();

    let action: AgentAction =
        serde_json::from_str(body).map_err(|e| FormatError::Invalid(e.to_string()))?;
    action
        .validate_with_servers(known_servers)
        .map_err(FormatError::Invalid)?;
    Ok(action)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wrap a JSON object in a single fenced action block.
    fn block(json: &str) -> String {
        format!("```action\n{json}\n```")
    }

    #[test]
    fn every_variant_round_trips_from_its_block() {
        let cases: Vec<(&str, AgentAction)> = vec![
            (
                r#"{"tool":"oracle_ask","query":"where is the launch path"}"#,
                AgentAction::OracleAsk {
                    query: "where is the launch path".into(),
                },
            ),
            (
                r#"{"tool":"oracle_context","query":"q","limit":5}"#,
                AgentAction::OracleContext {
                    query: "q".into(),
                    limit: Some(5),
                },
            ),
            (
                r#"{"tool":"oracle_context","query":"q"}"#,
                AgentAction::OracleContext {
                    query: "q".into(),
                    limit: None,
                },
            ),
            (
                r#"{"tool":"plan","steps":["a","b"]}"#,
                AgentAction::Plan {
                    steps: vec!["a".into(), "b".into()],
                },
            ),
            (r#"{"tool":"run_plan"}"#, AgentAction::RunPlan {}),
            (
                r#"{"tool":"spawn_mini","task":"fix it","files":["src/a.rs"],"write":true}"#,
                AgentAction::SpawnMini {
                    task: "fix it".into(),
                    files: vec!["src/a.rs".into()],
                    write: true,
                },
            ),
            (
                r#"{"tool":"spawn_mini","task":"look","files":["src/a.rs"]}"#,
                AgentAction::SpawnMini {
                    task: "look".into(),
                    files: vec!["src/a.rs".into()],
                    write: false,
                },
            ),
            (
                r#"{"tool":"read","path":"src/main.rs"}"#,
                AgentAction::Read {
                    path: "src/main.rs".into(),
                },
            ),
            (
                r#"{"tool":"grep","pattern":"TODO","glob":"*.rs"}"#,
                AgentAction::Grep {
                    pattern: "TODO".into(),
                    glob: Some("*.rs".into()),
                },
            ),
            (
                r#"{"tool":"glob","pattern":"**/*.rs"}"#,
                AgentAction::Glob {
                    pattern: "**/*.rs".into(),
                },
            ),
            (
                r#"{"tool":"fetch","url":"https://example.com"}"#,
                AgentAction::Fetch {
                    url: "https://example.com".into(),
                },
            ),
            (
                r#"{"tool":"websearch","query":"rust regex"}"#,
                AgentAction::Websearch {
                    query: "rust regex".into(),
                },
            ),
            (
                r#"{"tool":"ask_user","question":"which env?"}"#,
                AgentAction::AskUser {
                    question: "which env?".into(),
                },
            ),
            (
                r#"{"tool":"done","reply":"all set"}"#,
                AgentAction::Done {
                    reply: "all set".into(),
                },
            ),
            (
                r#"{"tool":"escalate","reason":"stuck"}"#,
                AgentAction::Escalate {
                    reason: "stuck".into(),
                },
            ),
        ];

        for (json, expected) in cases {
            let parsed = parse_action(&block(json))
                .unwrap_or_else(|e| panic!("{json} should parse, got {e:?}"));
            assert_eq!(parsed, expected, "round-trip mismatch for {json}");
        }
    }

    #[test]
    fn exactly_one_block_is_extracted_even_amid_prose() {
        let out = format!(
            "Let me think about this.\n\nI'll ask the oracle:\n\n{}\n\nThat should help.",
            block(r#"{"tool":"oracle_ask","query":"the path"}"#)
        );
        let parsed = parse_action(&out).expect("one block amid prose parses");
        assert_eq!(
            parsed,
            AgentAction::OracleAsk {
                query: "the path".into()
            }
        );
    }

    #[test]
    fn zero_blocks_is_missing() {
        let out = "I think the answer is 42. No action needed.";
        assert_eq!(parse_action(out), Err(FormatError::Missing));
    }

    #[test]
    fn two_blocks_is_too_many() {
        let out = format!(
            "{}\n\n{}",
            block(r#"{"tool":"read","path":"a.rs"}"#),
            block(r#"{"tool":"read","path":"b.rs"}"#)
        );
        assert_eq!(parse_action(&out), Err(FormatError::TooMany(2)));
    }

    #[test]
    fn three_blocks_reports_the_real_count() {
        let out = format!(
            "{}\n{}\n{}",
            block(r#"{"tool":"read","path":"a.rs"}"#),
            block(r#"{"tool":"read","path":"b.rs"}"#),
            block(r#"{"tool":"read","path":"c.rs"}"#)
        );
        assert_eq!(parse_action(&out), Err(FormatError::TooMany(3)));
    }

    #[test]
    fn nested_wrapped_action_block_is_rejected() {
        // BLOCKER 1: a model wraps a well-formed action block inside an OUTER
        // ```json fence. The inner ``` closes the captured block early so the
        // naive capture finds exactly ONE block and would dispatch the wrapped
        // (evil) action. The fence-depth walk sees the action opener is NESTED
        // (top_level == 0) and rejects it — the action must NOT be dispatched.
        let out = "```json\n```action\n{\"tool\":\"fetch\",\"url\":\"http://169.254.169.254/\"}\n```\n```";
        match parse_action(out) {
            Err(FormatError::Invalid(msg)) => {
                assert!(
                    msg.contains("nested"),
                    "message must mention nesting: {msg}"
                )
            }
            other => panic!("nested/wrapped block must be rejected, got {other:?}"),
        }
    }

    #[test]
    fn legit_single_block_still_parses_after_anti_nesting() {
        // The anti-nesting guard must not reject a normal top-level single block.
        let out = block(r#"{"tool":"read","path":"src/main.rs"}"#);
        assert_eq!(
            parse_action(&out).unwrap(),
            AgentAction::Read {
                path: "src/main.rs".into()
            }
        );
    }

    #[test]
    fn action_block_after_a_legit_code_fence_still_parses() {
        // A model may show an unrelated code fence (e.g. ```rust …```) and THEN
        // emit a top-level action block. The fence walk closes the rust fence
        // before the action opener, so the action is at top level -> parses.
        let out = "Here is the snippet:\n```rust\nfn main() {}\n```\n\n```action\n{\"tool\":\"read\",\"path\":\"a.rs\"}\n```";
        assert_eq!(
            parse_action(out).unwrap(),
            AgentAction::Read {
                path: "a.rs".into()
            }
        );
    }

    #[test]
    fn two_unclosed_prose_fences_then_action_is_rejected() {
        // FIX 2 (fence-depth evasion): a naive open/close TOGGLE is defeated by
        // EVEN PARITY — two UNCLOSED prose openers (```json, ```yaml) would toggle
        // the state back to "outside", so the following ```action would be
        // miscounted as TOP-LEVEL and dispatched. Under the CommonMark closing rule
        // (only a BARE fence closes), both prose openers and the action line are
        // CONTENT inside the first open block -> top_level == 0 -> rejected.
        let out = "```json\n```yaml\n```action\n{\"tool\":\"fetch\",\"url\":\"http://169.254.169.254/\"}\n```";
        match parse_action(out) {
            Err(FormatError::Invalid(msg)) => {
                assert!(
                    msg.contains("nested"),
                    "message must mention nesting: {msg}"
                )
            }
            other => panic!("even-parity prose-fence evasion must be rejected, got {other:?}"),
        }
    }

    #[test]
    fn malformed_json_is_invalid() {
        let out = block(r#"{"tool":"read","path":"a.rs""#); // missing close brace
        match parse_action(&out) {
            Err(FormatError::Invalid(_)) => {}
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn unknown_tool_is_invalid() {
        let out = block(r#"{"tool":"delete_everything","path":"a.rs"}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(_)) => {}
            other => panic!("expected Invalid for unknown tool, got {other:?}"),
        }
    }

    #[test]
    fn unknown_field_is_invalid() {
        // deny_unknown_fields: a stray key must be rejected, not ignored.
        let out = block(r#"{"tool":"read","path":"a.rs","mode":"rw"}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(_)) => {}
            other => panic!("expected Invalid for unknown field, got {other:?}"),
        }
    }

    #[test]
    fn run_plan_rejects_unknown_fields() {
        // RunPlan is an EMPTY STRUCT variant (not a unit variant) precisely so
        // deny_unknown_fields fires: a stray field — e.g. a model confusing `plan` with
        // `run_plan` and attaching `steps` — is a hard parse error with feedback, NOT
        // silently swallowed (a unit variant in an internally-tagged enum would accept it).
        let out = block(r#"{"tool":"run_plan","steps":["oops"]}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(_)) => {}
            other => panic!("expected Invalid for run_plan with extra fields, got {other:?}"),
        }
    }

    #[test]
    fn absolute_path_is_invalid() {
        let out = block(r#"{"tool":"read","path":"/etc/passwd"}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("absolute"), "{msg}"),
            other => panic!("expected Invalid for absolute path, got {other:?}"),
        }
    }

    #[test]
    fn windows_drive_path_is_invalid() {
        // On a unix host this would otherwise parse as one Normal component.
        let out = block(r#"{"tool":"read","path":"C:\\Windows\\system32"}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("absolute"), "{msg}"),
            other => panic!("expected Invalid for drive path, got {other:?}"),
        }
    }

    #[test]
    fn parent_traversal_path_is_invalid() {
        let out = block(r#"{"tool":"read","path":"../secrets.txt"}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains(".."), "{msg}"),
            other => panic!("expected Invalid for .. path, got {other:?}"),
        }
    }

    #[test]
    fn backslash_traversal_path_is_invalid() {
        let out = block(r#"{"tool":"read","path":"a\\..\\b"}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains(".."), "{msg}"),
            other => panic!("expected Invalid for backslash .. path, got {other:?}"),
        }
    }

    #[test]
    fn dash_leading_component_is_invalid() {
        let out = block(r#"{"tool":"read","path":"src/-rf"}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("'-'"), "{msg}"),
            other => panic!("expected Invalid for -leading component, got {other:?}"),
        }
    }

    #[test]
    fn spawn_mini_empty_files_is_invalid() {
        // WARNING 5: empty files must be rejected (before the over-cap check).
        let out = block(r#"{"tool":"spawn_mini","task":"do it","files":[]}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => {
                assert!(msg.contains("files must not be empty"), "{msg}")
            }
            other => panic!("expected Invalid for empty files, got {other:?}"),
        }
    }

    #[test]
    fn glob_pattern_traversal_is_invalid() {
        // WARNING 1: a `..` component in a glob pattern is rejected.
        let out = block(r#"{"tool":"glob","pattern":"../../**/*.env"}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains(".."), "{msg}"),
            other => panic!("expected Invalid for traversal glob, got {other:?}"),
        }
    }

    #[test]
    fn glob_pattern_absolute_is_invalid() {
        let out = block(r#"{"tool":"glob","pattern":"/etc/**"}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("absolute"), "{msg}"),
            other => panic!("expected Invalid for absolute glob, got {other:?}"),
        }
    }

    #[test]
    fn glob_pattern_relative_with_metachars_is_ok() {
        // Glob metacharacters in segments are allowed; only traversal/absolute is not.
        let out = block(r#"{"tool":"glob","pattern":"src/**/*.rs"}"#);
        assert_eq!(
            parse_action(&out).unwrap(),
            AgentAction::Glob {
                pattern: "src/**/*.rs".into()
            }
        );
    }

    #[test]
    fn grep_glob_traversal_is_invalid() {
        // WARNING 1: the grep `glob` filter is path-checked the same way.
        let out = block(r#"{"tool":"grep","pattern":"TODO","glob":"../secrets/*"}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains(".."), "{msg}"),
            other => panic!("expected Invalid for traversal grep glob, got {other:?}"),
        }
    }

    #[test]
    fn grep_invalid_regex_is_invalid() {
        // WARNING 2: an un-compilable regex is rejected with a precise message.
        let out = block(r#"{"tool":"grep","pattern":"("}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("valid regex"), "{msg}"),
            other => panic!("expected Invalid for bad regex, got {other:?}"),
        }
    }

    #[test]
    fn grep_valid_regex_is_ok() {
        let out = block(r#"{"tool":"grep","pattern":"fn\\s+\\w+","glob":"src/**/*.rs"}"#);
        assert_eq!(
            parse_action(&out).unwrap(),
            AgentAction::Grep {
                pattern: r"fn\s+\w+".into(),
                glob: Some("src/**/*.rs".into()),
            }
        );
    }

    #[test]
    fn grep_over_cap_pattern_is_invalid() {
        // WARNING 2: the grep pattern uses the tighter MAX_GREP_PATTERN_LEN cap.
        let big = "a".repeat(MAX_GREP_PATTERN_LEN + 1);
        let json = serde_json::json!({"tool":"grep","pattern":big}).to_string();
        match parse_action(&block(&json)) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("too long"), "{msg}"),
            other => panic!("expected Invalid for over-cap grep pattern, got {other:?}"),
        }
    }

    #[test]
    fn over_cap_files_list_is_invalid() {
        let files: Vec<String> = (0..(MAX_FILES + 1)).map(|i| format!("f{i}.rs")).collect();
        let json = serde_json::json!({
            "tool": "spawn_mini",
            "task": "touch many",
            "files": files,
        })
        .to_string();
        match parse_action(&block(&json)) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("too many files"), "{msg}"),
            other => panic!("expected Invalid for over-cap files, got {other:?}"),
        }
    }

    #[test]
    fn write_spawn_mini_over_ten_files_is_invalid() {
        // FIX 4: the WRITE arm is capped at MAX_WRITE_FILES (10) — the server
        // rejects more, so we reject at parse time for self-correctable feedback.
        let files: Vec<String> = (0..(MAX_WRITE_FILES + 1))
            .map(|i| format!("f{i}.rs"))
            .collect();
        let json = serde_json::json!({
            "tool": "spawn_mini",
            "task": "edit many",
            "files": files,
            "write": true,
        })
        .to_string();
        match parse_action(&block(&json)) {
            Err(FormatError::Invalid(msg)) => {
                assert!(msg.contains("at most 10 files"), "{msg}")
            }
            other => panic!("expected Invalid for over-10 write spawn_mini, got {other:?}"),
        }
    }

    #[test]
    fn read_spawn_mini_with_eleven_files_is_ok() {
        // FIX 4: the READ arm keeps the 32 cap, so 11 files (write=false) is fine.
        let files: Vec<String> = (0..(MAX_WRITE_FILES + 1))
            .map(|i| format!("f{i}.rs"))
            .collect();
        let json = serde_json::json!({
            "tool": "spawn_mini",
            "task": "read many",
            "files": files,
            "write": false,
        })
        .to_string();
        match parse_action(&block(&json)) {
            Ok(AgentAction::SpawnMini { write, files, .. }) => {
                assert!(!write, "read arm");
                assert_eq!(files.len(), MAX_WRITE_FILES + 1, "all 11 files kept");
            }
            other => panic!("expected Ok for 11-file read spawn_mini, got {other:?}"),
        }
    }

    #[test]
    fn one_bad_file_in_list_is_invalid() {
        let out = block(r#"{"tool":"spawn_mini","task":"t","files":["ok.rs","/abs.rs"]}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("absolute"), "{msg}"),
            other => panic!("expected Invalid for a bad file entry, got {other:?}"),
        }
    }

    #[test]
    fn over_cap_text_is_invalid() {
        let big = "x".repeat(MAX_TEXT_LEN + 1);
        let json = serde_json::json!({"tool":"oracle_ask","query":big}).to_string();
        match parse_action(&block(&json)) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("too long"), "{msg}"),
            other => panic!("expected Invalid for over-cap text, got {other:?}"),
        }
    }

    #[test]
    fn empty_query_is_invalid() {
        let out = block(r#"{"tool":"oracle_ask","query":"   "}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("empty"), "{msg}"),
            other => panic!("expected Invalid for empty query, got {other:?}"),
        }
    }

    #[test]
    fn empty_plan_is_invalid() {
        let out = block(r#"{"tool":"plan","steps":[]}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("at least one step"), "{msg}"),
            other => panic!("expected Invalid for empty plan, got {other:?}"),
        }
    }

    #[test]
    fn fetch_file_scheme_is_invalid() {
        let out = block(r#"{"tool":"fetch","url":"file:///etc/passwd"}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("scheme"), "{msg}"),
            other => panic!("expected Invalid for file:// scheme, got {other:?}"),
        }
    }

    #[test]
    fn fetch_localhost_is_invalid() {
        let out = block(r#"{"tool":"fetch","url":"http://localhost/admin"}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("localhost"), "{msg}"),
            other => panic!("expected Invalid for localhost, got {other:?}"),
        }
    }

    #[test]
    fn fetch_cloud_metadata_ip_is_invalid() {
        let out = block(r#"{"tool":"fetch","url":"http://169.254.169.254/latest/meta-data/"}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("169.254.169.254"), "{msg}"),
            other => panic!("expected Invalid for metadata IP, got {other:?}"),
        }
    }

    #[test]
    fn fetch_bracketed_ipv6_loopback_is_invalid() {
        let out = block(r#"{"tool":"fetch","url":"http://[::1]:8080/"}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("not allowed"), "{msg}"),
            other => panic!("expected Invalid for [::1], got {other:?}"),
        }
    }

    #[test]
    fn fetch_uppercase_scheme_is_normalised_and_blocked() {
        // FILE:// must be rejected despite the uppercase scheme.
        let out = block(r#"{"tool":"fetch","url":"FILE:///etc/passwd"}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("scheme"), "{msg}"),
            other => panic!("expected Invalid for FILE:// scheme, got {other:?}"),
        }
    }

    #[test]
    fn fetch_valid_https_is_ok() {
        let out = block(r#"{"tool":"fetch","url":"https://example.com/x"}"#);
        assert_eq!(
            parse_action(&out).unwrap(),
            AgentAction::Fetch {
                url: "https://example.com/x".into()
            }
        );
    }

    #[test]
    fn fetch_loopback_ip_outside_127_0_0_1_is_invalid() {
        // FIX 8: the old string blocklist only caught 127.0.0.1; the whole 127/8
        // range is loopback and must be rejected (SSRF), so 127.0.0.2 and
        // 127.1.2.3 must both fail now.
        for url in [
            "http://127.0.0.2/x",
            "http://127.1.2.3/x",
            "http://127.255.255.254/admin",
        ] {
            let out = block(&format!(r#"{{"tool":"fetch","url":"{url}"}}"#));
            match parse_action(&out) {
                Err(FormatError::Invalid(msg)) => {
                    assert!(msg.contains("not allowed"), "{url}: {msg}")
                }
                other => panic!("expected Invalid for loopback {url}, got {other:?}"),
            }
        }
    }

    #[test]
    fn fetch_ipv4_mapped_ipv6_loopback_is_invalid() {
        // FIX 8: `::ffff:127.0.0.1` is IPv4-mapped loopback; Ipv6Addr::is_loopback
        // does NOT flag it, so the mapped-v4 unwrap must catch it.
        let out = block(r#"{"tool":"fetch","url":"http://[::ffff:127.0.0.1]/x"}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("not allowed"), "{msg}"),
            other => panic!("expected Invalid for ::ffff:127.0.0.1, got {other:?}"),
        }
    }

    #[test]
    fn fetch_normal_public_host_is_ok() {
        // FIX 8: a public host (and a public IP) must still pass.
        for url in ["http://93.184.216.34/x", "https://example.org/path"] {
            let out = block(&format!(r#"{{"tool":"fetch","url":"{url}"}}"#));
            assert!(
                matches!(parse_action(&out), Ok(AgentAction::Fetch { .. })),
                "public host {url} must be allowed"
            );
        }
    }

    #[test]
    fn fetch_userinfo_at_in_authority_is_invalid() {
        // FIX 1 (SSRF): `http://evil.com@127.0.0.1/` puts the REAL host after the
        // `@`. The naive host-span extraction would take `evil.com@127.0.0.1` (not
        // in the blocklist, not a valid IP) and pass — then reqwest connects to
        // 127.0.0.1. Any `@` in the authority must be rejected at parse time.
        for url in [
            "http://evil.com@127.0.0.1/",
            "http://x@localhost/",
            "http://a@169.254.169.254/",
        ] {
            let out = block(&format!(r#"{{"tool":"fetch","url":"{url}"}}"#));
            match parse_action(&out) {
                Err(FormatError::Invalid(msg)) => {
                    assert!(msg.contains("userinfo"), "{url}: {msg}")
                }
                other => panic!("expected Invalid for userinfo {url}, got {other:?}"),
            }
        }
    }

    #[test]
    fn fetch_normal_url_without_userinfo_is_ok() {
        // FIX 1: a normal URL (no `@` in the authority) must still pass.
        let out = block(r#"{"tool":"fetch","url":"https://example.com/p"}"#);
        assert!(
            matches!(parse_action(&out), Ok(AgentAction::Fetch { .. })),
            "a normal URL without userinfo must be allowed"
        );
    }

    #[test]
    fn egress_marker_is_correct() {
        assert!(AgentAction::Fetch {
            url: "https://x".into()
        }
        .is_egress());
        assert!(AgentAction::Websearch { query: "q".into() }.is_egress());
        assert!(!AgentAction::OracleAsk { query: "q".into() }.is_egress());
        assert!(!AgentAction::Read {
            path: "a.rs".into()
        }
        .is_egress());
    }

    #[test]
    fn crlf_block_parses() {
        // The fence regex tolerates CRLF line endings.
        let out = "```action\r\n{\"tool\":\"read\",\"path\":\"a.rs\"}\r\n```";
        assert_eq!(
            parse_action(out).unwrap(),
            AgentAction::Read {
                path: "a.rs".into()
            }
        );
    }

    // --- B.1: AgentAction::McpTool -------------------------------------------

    #[test]
    fn mcp_tool_with_known_server_round_trips() {
        // A valid mcp_tool block whose server is in the configured set parses and
        // round-trips, params preserved as an arbitrary JSON object.
        let out = block(r#"{"tool":"mcp_tool","server":"my-db","name":"query","params":{"sql":"SELECT 1","limit":10}}"#);
        let servers = vec!["my-db".to_string()];
        let parsed = parse_action_with_servers(&out, &servers).expect("known server parses");
        assert_eq!(
            parsed,
            AgentAction::McpTool {
                server: "my-db".into(),
                tool: "query".into(),
                params: serde_json::json!({"sql": "SELECT 1", "limit": 10}),
            }
        );
    }

    #[test]
    fn mcp_tool_unknown_server_is_invalid() {
        // A server NOT in the configured set is rejected at parse time with a precise
        // message — the model gets immediate feedback, not a late call-time error.
        let out = block(r#"{"tool":"mcp_tool","server":"typo-db","name":"query","params":{}}"#);
        let servers = vec!["my-db".to_string()];
        match parse_action_with_servers(&out, &servers) {
            Err(FormatError::Invalid(msg)) => {
                assert!(msg.contains("unknown MCP server"), "{msg}");
                assert!(msg.contains("typo-db"), "names the bad server: {msg}");
            }
            other => panic!("expected Invalid for unknown server, got {other:?}"),
        }
    }

    #[test]
    fn mcp_tool_with_no_servers_configured_is_invalid() {
        // The default `parse_action` validates against an EMPTY set, so ANY mcp_tool
        // is rejected when no user servers are configured (the no-user-servers path).
        let out = block(r#"{"tool":"mcp_tool","server":"my-db","name":"query","params":{}}"#);
        match parse_action(&out) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("unknown MCP server"), "{msg}"),
            other => panic!("expected Invalid with no servers configured, got {other:?}"),
        }
    }

    #[test]
    fn mcp_tool_non_object_params_is_invalid() {
        // A top-level scalar or array `params` is rejected (the call_tool contract is a
        // named-argument object). The server is known so this isolates the params check.
        let servers = vec!["my-db".to_string()];
        for bad in [
            r#"{"tool":"mcp_tool","server":"my-db","name":"query","params":[1,2,3]}"#,
            r#"{"tool":"mcp_tool","server":"my-db","name":"query","params":"a string"}"#,
            r#"{"tool":"mcp_tool","server":"my-db","name":"query","params":42}"#,
            r#"{"tool":"mcp_tool","server":"my-db","name":"query","params":null}"#,
        ] {
            match parse_action_with_servers(&block(bad), &servers) {
                Err(FormatError::Invalid(msg)) => {
                    assert!(msg.contains("`params` must be a JSON object"), "{bad}: {msg}")
                }
                other => panic!("expected Invalid for non-object params {bad}, got {other:?}"),
            }
        }
        // An empty object IS valid params (a no-argument tool call).
        let ok = block(r#"{"tool":"mcp_tool","server":"my-db","name":"query","params":{}}"#);
        assert!(
            matches!(
                parse_action_with_servers(&ok, &servers),
                Ok(AgentAction::McpTool { .. })
            ),
            "an empty params object is valid"
        );
    }

    #[test]
    fn mcp_tool_empty_server_or_tool_is_invalid() {
        let servers = vec!["my-db".to_string(), "".to_string()];
        // Empty server: rejected before the known-set check (and "" is never a real
        // configured name regardless).
        let empty_server =
            block(r#"{"tool":"mcp_tool","server":"","name":"query","params":{}}"#);
        match parse_action_with_servers(&empty_server, &servers) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("`server` must not be empty"), "{msg}"),
            other => panic!("expected Invalid for empty server, got {other:?}"),
        }
        // Empty tool on a known server: rejected by the text check.
        let empty_tool =
            block(r#"{"tool":"mcp_tool","server":"my-db","name":"   ","params":{}}"#);
        match parse_action_with_servers(&empty_tool, &servers) {
            Err(FormatError::Invalid(msg)) => assert!(msg.contains("`tool` must not be empty"), "{msg}"),
            other => panic!("expected Invalid for empty tool, got {other:?}"),
        }
    }

    #[test]
    fn mcp_tool_is_not_web_egress() {
        // DECOUPLING (design §5.2): a user MCP server is its OWN opt-in capability, NOT
        // the web-search opt-in. So `mcp_tool` is deliberately NOT web-egress — the
        // `run_burst` gate (`is_egress() && !allow_egress`) must NOT block it. Its gate
        // is the known-server set (validate_with_servers), not `allow_egress`.
        assert!(!AgentAction::McpTool {
            server: "my-db".into(),
            tool: "query".into(),
            params: serde_json::json!({}),
        }
        .is_egress());
        // The WEB tools stay egress (gated on allow_egress) — unchanged.
        assert!(AgentAction::Fetch { url: "https://x".into() }.is_egress());
        assert!(AgentAction::Websearch { query: "q".into() }.is_egress());
    }

    #[test]
    fn mcp_tool_name_and_target() {
        let action = AgentAction::McpTool {
            server: "my-db".into(),
            tool: "query".into(),
            params: serde_json::json!({}),
        };
        assert_eq!(action.tool_name(), "mcp_tool");
        // target is `server.tool <params>`: identical repeats (same server+tool+params)
        // trip the no-progress guard, but the same tool with DIFFERENT params does not.
        assert_eq!(action.target(), "my-db.query {}");
        // Different params ⇒ a DIFFERENT target (no false no-progress trip).
        let other = AgentAction::McpTool {
            server: "my-db".into(),
            tool: "query".into(),
            params: serde_json::json!({"page": 2}),
        };
        assert_ne!(action.target(), other.target(), "differing params ⇒ distinct targets");
    }
}
