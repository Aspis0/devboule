//! Per-agent token / cost window (MC-P6): a BEST-EFFORT, Claude-Code-coupled
//! read of how many tokens an agent has consumed (+ an approximate USD cost for
//! API-priced Claude), surfaced as a small badge in the work-mode rail.
//!
//! WHY BEST-EFFORT: there is no first-class token accounting in this app. The only
//! source is Claude Code's PRIVATE session transcript at
//! `~/.claude/projects/<cwd-mangled>/*.jsonl`, where each assistant turn carries a
//! `message.usage` object. We read it leniently and degrade to `unavailable` on ANY
//! surprise (unknown cwd, missing dir, malformed file, unknown model). It never
//! crashes and never blocks the UI.
//!
//! AGENT → SESSION-FILE LIMITATION (documented): Claude Code does not expose which
//! `*.jsonl` belongs to which of our agents — a project dir can hold many session
//! files across many runs. We therefore pick the NEWEST `*.jsonl` in the agent's
//! project dir as a best-effort proxy for "the agent's current session". When two
//! Claude agents run in the same project at once this can attribute usage to the
//! wrong row; that is an accepted limitation of a best-effort badge, not a bug.
//!
//! PRIVACY (the single biggest risk — reviewer will check): we read ONLY the numeric
//! `usage` fields out of each line and DISCARD everything else. Message text, tool
//! input/output, file contents, and prompts are NEVER parsed into our structs, NEVER
//! returned over IPC, and NEVER logged. The parser (`sum_usage_from_jsonl`) extracts
//! four integers per line and the model string; nothing else leaves this module.
//!
//! PERFORMANCE: a long session JSONL can be many MB (6+ MB observed). We read only a
//! TAIL-BOUNDED window (`MAX_TRANSCRIPT_TAIL_BYTES`) so the read can never OOM or
//! stall the UI. The frontend fetches this only for the SELECTED agent on a slow
//! cadence — never per rail row, never on the 5s live-state tick.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::State;

use super::state::BackendState;

/// Max bytes read from the tail of a session JSONL. A session transcript grows
/// unbounded; we only need a recent window to show a meaningful running total, and
/// a hard cap is what keeps the read off the OOM/UI-block path. ~4 MiB comfortably
/// covers many recent assistant turns while staying cheap. NOTE: because we read a
/// TAIL slice (not the whole file), the summed total is a recent-window estimate,
/// not the lifetime total of a very long session — an accepted best-effort tradeoff
/// documented in the badge tooltip.
const MAX_TRANSCRIPT_TAIL_BYTES: u64 = 4 * 1024 * 1024;

/// Per-agent token usage summary returned to the UI. camelCase over IPC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTokenUsage {
    pub tokens: TokenCounts,
    /// Approximate USD cost for API-priced Claude, or `None` when the model/pricing
    /// is unknown or the source is a flat-rate subscription (codex). PRICES DRIFT —
    /// see `PRICING` below; this is intentionally approximate.
    pub cost_usd: Option<f64>,
    /// Where the numbers came from:
    ///   - "claude-transcript": summed from a Claude Code session JSONL.
    ///   - "subscription": codex / a subscription-priced mini — no per-token cost.
    ///   - "unavailable": cwd unknown, no transcript dir, unreadable, or non-claude
    ///     non-codex client. Tokens are zeroed; the badge hides.
    pub source: String,
}

/// Summed token counts. All `u64` so a long session can never overflow. NOTHING
/// here is message content — these are the only fields the parser keeps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
    /// Convenience sum of the four above, so the UI does not re-add.
    pub total: u64,
}

impl TokenCounts {
    fn finalize(mut self) -> Self {
        self.total = self
            .input
            .saturating_add(self.output)
            .saturating_add(self.cache_creation)
            .saturating_add(self.cache_read);
        self
    }
}

/// Source string constants (mirror the TS union in backend.ts).
pub const SOURCE_CLAUDE_TRANSCRIPT: &str = "claude-transcript";
pub const SOURCE_SUBSCRIPTION: &str = "subscription";
pub const SOURCE_UNAVAILABLE: &str = "unavailable";

/// The `unavailable` reply: zeroed tokens, no cost. Used for every degrade path.
fn unavailable() -> AgentTokenUsage {
    AgentTokenUsage {
        tokens: TokenCounts::default().finalize(),
        cost_usd: None,
        source: SOURCE_UNAVAILABLE.into(),
    }
}

/// The `subscription` reply: zeroed tokens, no per-token cost. codex (and codex
/// subscription minis) ride a flat subscription, so there is no API token bill to
/// surface; we still return a stable shape so the badge can show "subscription".
fn subscription() -> AgentTokenUsage {
    AgentTokenUsage {
        tokens: TokenCounts::default().finalize(),
        cost_usd: None,
        source: SOURCE_SUBSCRIPTION.into(),
    }
}

// ---------------------------------------------------------------------------
// MANUALLY-MAINTAINED, APPROXIMATE pricing table.
//
// Per-model USD price per MILLION tokens. Claude Code rides Anthropic API pricing;
// these numbers CHANGE over time and across tiers/regions — treat them as a rough
// cost hint, NOT a billing source of truth. To update: edit this constant only.
// When a session's model is not listed, `cost_usd` is `None` (we never guess a
// price for an unknown model). cache_read is the cheap cached-input rate;
// cache_creation (cache write) is the more expensive 5m-write rate.
//
// Last reviewed: 2026-06 (approximate public list prices).
// ---------------------------------------------------------------------------

/// One model's $/Mtok rates. All four match the four `usage` fields.
#[derive(Debug, Clone, Copy)]
struct ModelPricing {
    input_per_mtok: f64,
    output_per_mtok: f64,
    cache_write_per_mtok: f64,
    cache_read_per_mtok: f64,
}

/// (model-key-prefix, pricing). Matched by `model.starts_with(prefix)` so a dated
/// model id (e.g. "claude-opus-4-8-20260101") still resolves to its family rate.
/// Order matters: more specific prefixes FIRST.
const PRICING: &[(&str, ModelPricing)] = &[
    (
        // Opus family.
        "claude-opus",
        ModelPricing {
            input_per_mtok: 15.0,
            output_per_mtok: 75.0,
            cache_write_per_mtok: 18.75,
            cache_read_per_mtok: 1.5,
        },
    ),
    (
        // Sonnet family.
        "claude-sonnet",
        ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
            cache_write_per_mtok: 3.75,
            cache_read_per_mtok: 0.3,
        },
    ),
    (
        // Haiku family.
        "claude-haiku",
        ModelPricing {
            input_per_mtok: 0.8,
            output_per_mtok: 4.0,
            cache_write_per_mtok: 1.0,
            cache_read_per_mtok: 0.08,
        },
    ),
];

/// Resolve a model string to its pricing family, or `None` for an unknown model.
fn pricing_for_model(model: &str) -> Option<ModelPricing> {
    let normalized = model.trim().to_ascii_lowercase();
    PRICING
        .iter()
        .find(|(prefix, _)| normalized.starts_with(prefix))
        .map(|(_, pricing)| *pricing)
}

/// Approximate USD cost for a token total at a given model's pricing, or `None`
/// when the model is unknown (we never guess). Pure + unit-tested.
fn cost_for(tokens: &TokenCounts, model: Option<&str>) -> Option<f64> {
    let pricing = pricing_for_model(model?)?;
    let per_m = 1_000_000.0;
    let cost = (tokens.input as f64) / per_m * pricing.input_per_mtok
        + (tokens.output as f64) / per_m * pricing.output_per_mtok
        + (tokens.cache_creation as f64) / per_m * pricing.cache_write_per_mtok
        + (tokens.cache_read as f64) / per_m * pricing.cache_read_per_mtok;
    Some(cost)
}

// ---------------------------------------------------------------------------
// cwd -> Claude Code project dir mangling.
// ---------------------------------------------------------------------------

/// Mangle an absolute cwd to Claude Code's `~/.claude/projects/<dir>` directory
/// name. Derived from THIS repo's own observed dir:
///   `C:\Users\gualt\Desktop\Devboule` -> `C--Users-gualt-Desktop-Devboule`
/// The rule Claude Code uses: every character that is NOT `[A-Za-z0-9]` is replaced
/// by a single `-` (so `:`, `\`, `/`, space, and `.` all become `-`). Note the
/// leading drive `C:` becomes `C-` and the following `\` adds another `-`, yielding
/// the `C--` prefix. Pure + unit-tested against the known path above.
pub fn mangle_cwd_to_project_dir(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// `~/.claude/projects` for the current user, or `None` if no home dir is set.
/// Cross-platform: `USERPROFILE` on Windows, `HOME` on Unix/macOS (mirrors
/// `cli_agents::user_home`).
fn claude_projects_root() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())?;
    Some(home.join(".claude").join("projects"))
}

/// Newest `*.jsonl` (by mtime) in `dir`, or `None` if the dir is missing/empty or
/// holds no `.jsonl`. Best-effort: any IO error yields `None`.
fn newest_jsonl(dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let mtime = match entry.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
            best = Some((mtime, path));
        }
    }
    best.map(|(_, path)| path)
}

/// Read the TAIL (last `MAX_TRANSCRIPT_TAIL_BYTES`) of a file as a UTF-8-lossy
/// string. Reading the tail (not the head) keeps the most RECENT turns. We drop the
/// first (likely partial) line at the seam so a half-line never parses wrong.
/// Best-effort: any IO error yields `None`.
fn read_tail_bounded(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let (start, trim_first_line) = if len > MAX_TRANSCRIPT_TAIL_BYTES {
        (len - MAX_TRANSCRIPT_TAIL_BYTES, true)
    } else {
        (0, false)
    };
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity((len - start).min(MAX_TRANSCRIPT_TAIL_BYTES) as usize);
    file.take(MAX_TRANSCRIPT_TAIL_BYTES)
        .read_to_end(&mut buf)
        .ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    if trim_first_line {
        // Drop everything up to and including the first newline: the tail seam very
        // likely landed mid-line. If there is no newline at all, keep nothing
        // partial (return empty) rather than parse a fragment.
        match text.find('\n') {
            Some(idx) => Some(text[idx + 1..].to_string()),
            None => Some(String::new()),
        }
    } else {
        Some(text)
    }
}

/// Result of parsing a transcript tail: summed token counts + the last seen model
/// (used for pricing). PRIVACY: this is the ONLY data extracted — four integers and
/// the model name; no message text.
struct ParsedUsage {
    tokens: TokenCounts,
    model: Option<String>,
}

/// Parse a JSONL transcript body, summing ONLY the numeric `message.usage` fields
/// across all lines and capturing the most-recent assistant `message.model` for
/// pricing. Malformed lines are skipped. Lines without a `usage` object contribute
/// nothing. PRIVACY: we deserialize each line into a minimal shape that captures
/// ONLY `message.model` (a short id) + the four `usage` integers — message text,
/// tool I/O, and content blocks are NOT bound to any field, so they cannot be
/// returned or logged. Pure + unit-tested (incl. a privacy assertion).
fn sum_usage_from_jsonl(body: &str) -> ParsedUsage {
    // Minimal, content-free shapes. serde only fills the fields named here; every
    // other key in the line (content, tool_use, text, …) is ignored and dropped.
    #[derive(Deserialize)]
    struct Line {
        message: Option<Msg>,
    }
    #[derive(Deserialize)]
    struct Msg {
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        usage: Option<Usage>,
    }
    #[derive(Deserialize)]
    struct Usage {
        #[serde(default)]
        input_tokens: u64,
        #[serde(default)]
        output_tokens: u64,
        #[serde(default)]
        cache_creation_input_tokens: u64,
        #[serde(default)]
        cache_read_input_tokens: u64,
    }

    let mut tokens = TokenCounts::default();
    let mut model: Option<String> = None;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: Line = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(_) => continue, // skip malformed lines (best-effort)
        };
        let Some(msg) = parsed.message else { continue };
        if let Some(found_model) = msg.model {
            if !found_model.trim().is_empty() {
                model = Some(found_model);
            }
        }
        if let Some(usage) = msg.usage {
            tokens.input = tokens.input.saturating_add(usage.input_tokens);
            tokens.output = tokens.output.saturating_add(usage.output_tokens);
            tokens.cache_creation = tokens
                .cache_creation
                .saturating_add(usage.cache_creation_input_tokens);
            tokens.cache_read = tokens
                .cache_read
                .saturating_add(usage.cache_read_input_tokens);
        }
    }
    ParsedUsage {
        tokens: tokens.finalize(),
        model,
    }
}

/// Build the usage summary from an agent's resolved cwd. Pure over the filesystem
/// (no AppHandle / locks): mangle cwd -> project dir -> newest jsonl -> tail-read ->
/// sum -> price. ANY missing step degrades to `unavailable`. Separated from the
/// command so it can be tested by pointing `claude_projects_root` at a fixture (the
/// command wires the real cwd resolution).
fn usage_from_cwd(cwd: &str, launched_after: Option<std::time::SystemTime>) -> AgentTokenUsage {
    // Our mangling ("each non-alphanumeric -> '-'") was derived from ASCII paths and
    // may NOT match Claude Code's encoding of non-ASCII characters. A mismatch could
    // (rarely) resolve to a DIFFERENT project's transcript dir -> wrong tokens. Safe
    // degrade: any non-ASCII char in the cwd -> unavailable, never a guessed dir.
    if !cwd.is_ascii() {
        return unavailable();
    }
    let Some(projects_root) = claude_projects_root() else {
        return unavailable();
    };
    let dir = projects_root.join(mangle_cwd_to_project_dir(cwd));
    if !dir.is_dir() {
        return unavailable();
    }
    let Some(jsonl) = newest_jsonl(&dir) else {
        return unavailable();
    };
    // BUG #18: the dir is keyed by PROJECT cwd, so newest_jsonl can resolve to a
    // DIFFERENT/earlier agent's session. Attribute the transcript to this agent
    // ONLY when we can confirm it was last written AT/AFTER the agent launched.
    // FAIL-CLOSED: a transcript older than the launch — OR one whose mtime we
    // cannot read — is not borrowed; the badge degrades to unavailable.
    if let Some(after) = launched_after {
        match fs::metadata(&jsonl).and_then(|m| m.modified()) {
            Ok(mtime) if mtime >= after => {}
            _ => return unavailable(),
        }
    }
    let Some(body) = read_tail_bounded(&jsonl) else {
        return unavailable();
    };
    let parsed = sum_usage_from_jsonl(&body);
    // A transcript dir/file was found but carried no `usage` objects in the read
    // window (total == 0). Surfacing "claude-transcript" with zeros would render a
    // misleading "0 tok · $0.00" badge; degrade to unavailable so the badge hides.
    if parsed.tokens.total == 0 {
        return unavailable();
    }
    let cost = cost_for(&parsed.tokens, parsed.model.as_deref());
    AgentTokenUsage {
        tokens: parsed.tokens,
        cost_usd: cost,
        source: SOURCE_CLAUDE_TRANSCRIPT.into(),
    }
}

/// Tauri command: best-effort per-agent token / cost window. See module docs.
///
/// Flow:
///   1) ensure unlocked.
///   2) find the agent's session in the live state; if absent -> unavailable.
///   3) gate on the resolved CLIENT (ledger-stamped):
///        - "claude": read the transcript (below).
///        - "codex" + any codex-* mini backend kind: "subscription".
///        - anything else / unknown: "unavailable".
///   4) resolve the agent's launch cwd from `currentProjectId` ->
///      `resolve_project_root_by_id`; if unknown/unresolvable -> unavailable.
///   5) `usage_from_cwd` does the transcript read + sum + pricing.
///
/// NEVER crashes, NEVER blocks: every failure path returns a well-formed
/// `unavailable`. PRIVACY: only numeric usage leaves this command.
#[tauri::command]
pub fn get_agent_token_usage(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    agent_id: String,
) -> Result<AgentTokenUsage, String> {
    state.ensure_unlocked()?;

    // Read the live state (stamps the resolved client from the ledger). A read
    // failure or a missing session degrades to unavailable rather than erroring,
    // so a transient state-file hiccup never breaks the badge.
    let live = match super::agents::read_agent_live_state_snapshot(&app) {
        Ok(state) => state,
        Err(_) => return Ok(unavailable()),
    };
    // The snapshot is the raw persisted state (no ledger stamp). For the CLIENT
    // gate we must consult the ledger directly — that is the authoritative launch
    // CLI for a live agent (the persisted session.client may be stale/None).
    let session = live.sessions.iter().find(|s| s.agent_id == agent_id);
    let Some(session) = session else {
        return Ok(unavailable());
    };

    let client = super::agents::read_agent_ledger_entry(&app, &agent_id)
        .ok()
        .flatten()
        .map(|entry| entry.client)
        .or_else(|| session.client.clone())
        .unwrap_or_default()
        .to_ascii_lowercase();

    // codex rides a flat subscription -> no token bill. Match EXACTLY the normalized
    // built-in "codex": a `starts_with` would wrongly route a hypothetical custom
    // client named e.g. "codex-local" here instead of letting it fall through to
    // the non-claude `unavailable` branch.
    if client == "codex" {
        return Ok(subscription());
    }
    // Only Claude reads a transcript; every other / unknown client is unavailable.
    if client != "claude" {
        return Ok(unavailable());
    }

    // Resolve the agent's working dir from its project id. A mini inherits its
    // parent's project, so its session.currentProjectId is already set by the
    // executor; a normal claude agent has it from launch. Absent/unresolvable ->
    // unavailable (we never guess a cwd).
    let Some(project_id) = session.current_project_id.clone() else {
        return Ok(unavailable());
    };
    let cwd = match super::projects::resolve_project_root_by_id(&app, &project_id) {
        Ok(root) => root.to_string_lossy().into_owned(),
        Err(_) => return Ok(unavailable()),
    };

    // BUG #18: pass the agent's launch time so a transcript that predates this
    // agent (an earlier/other session sharing the project dir) is not borrowed.
    // Use ONLY launch_token_issued_at — a reliable launch anchor. first_seen_at is
    // NOT used as a fallback: for an agent without a launch token (e.g. a mini) it
    // can be re-stamped on reconnect to AFTER an in-progress transcript's last
    // write and wrongly filter a live badge; absent anchor => no filter (review).
    // `timestamp() >= 0` rejects a nonsensical pre-epoch launch time (a corrupt /
    // hand-edited state file) as "no valid anchor" — it drops to None, so the badge
    // degrades to its pre-#18 best-effort attribution rather than trusting garbage,
    // and we never feed a pre-1970 instant into the chrono->SystemTime conversion.
    let launched_after = session
        .launch_token_issued_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .filter(|dt| dt.timestamp() >= 0)
        .map(std::time::SystemTime::from);
    Ok(usage_from_cwd(&cwd, launched_after))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Mutex;

    #[test]
    fn mangles_known_repo_path_to_observed_dir() {
        // The exact rule, derived from this repo's real dir under ~/.claude/projects.
        assert_eq!(
            mangle_cwd_to_project_dir(r"C:\Users\gualt\Desktop\Devboule"),
            "C--Users-gualt-Desktop-Devboule"
        );
    }

    #[test]
    fn mangles_dots_and_slashes_to_dash() {
        assert_eq!(
            mangle_cwd_to_project_dir("/home/u/proj.v2/sub dir"),
            "-home-u-proj-v2-sub-dir"
        );
    }

    #[test]
    fn sums_usage_across_assistant_turns_with_cache_fields() {
        // Two assistant turns + a non-usage line + a malformed line: only the two
        // usage objects are summed; the malformed line is skipped.
        let body = concat!(
            r#"{"type":"assistant","message":{"model":"claude-opus-4-8","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":5}}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"hi"}}"#,
            "\n",
            r#"{not valid json"#,
            "\n",
            r#"{"type":"assistant","message":{"model":"claude-opus-4-8","usage":{"input_tokens":200,"output_tokens":60,"cache_creation_input_tokens":0,"cache_read_input_tokens":7}}}"#,
            "\n",
        );
        let parsed = sum_usage_from_jsonl(body);
        assert_eq!(parsed.tokens.input, 300);
        assert_eq!(parsed.tokens.output, 110);
        assert_eq!(parsed.tokens.cache_creation, 10);
        assert_eq!(parsed.tokens.cache_read, 12);
        assert_eq!(parsed.tokens.total, 300 + 110 + 10 + 12);
        assert_eq!(parsed.model.as_deref(), Some("claude-opus-4-8"));
    }

    #[test]
    fn missing_usage_fields_default_to_zero() {
        // A usage object with only input_tokens — the other three default to 0,
        // not a parse failure.
        let body = r#"{"message":{"model":"claude-haiku-3","usage":{"input_tokens":42}}}"#;
        let parsed = sum_usage_from_jsonl(body);
        assert_eq!(parsed.tokens.input, 42);
        assert_eq!(parsed.tokens.output, 0);
        assert_eq!(parsed.tokens.cache_creation, 0);
        assert_eq!(parsed.tokens.cache_read, 0);
        assert_eq!(parsed.tokens.total, 42);
    }

    #[test]
    fn pricing_for_known_opus_model_is_computed() {
        let tokens = TokenCounts {
            input: 1_000_000,
            output: 1_000_000,
            cache_creation: 0,
            cache_read: 0,
            total: 2_000_000,
        };
        // 1M input @ $15 + 1M output @ $75 = $90.00.
        let cost = cost_for(&tokens, Some("claude-opus-4-8")).expect("known model has a price");
        assert!((cost - 90.0).abs() < 1e-6, "got {cost}");
    }

    #[test]
    fn pricing_for_unknown_model_is_none() {
        let tokens = TokenCounts {
            input: 1_000_000,
            output: 0,
            cache_creation: 0,
            cache_read: 0,
            total: 1_000_000,
        };
        assert_eq!(cost_for(&tokens, Some("gpt-4o")), None);
        assert_eq!(cost_for(&tokens, None), None);
    }

    #[test]
    fn subscription_and_unavailable_have_no_cost_and_zero_tokens() {
        let sub = subscription();
        assert_eq!(sub.source, SOURCE_SUBSCRIPTION);
        assert_eq!(sub.cost_usd, None);
        assert_eq!(sub.tokens.total, 0);

        let un = unavailable();
        assert_eq!(un.source, SOURCE_UNAVAILABLE);
        assert_eq!(un.cost_usd, None);
        assert_eq!(un.tokens.total, 0);
    }

    #[test]
    fn missing_transcript_dir_is_unavailable() {
        // A cwd whose mangled dir cannot exist under ~/.claude/projects -> unavailable.
        let usage = usage_from_cwd(r"Z:\definitely\not\a\real\claude\project\dir-xyz-9999", None);
        assert_eq!(usage.source, SOURCE_UNAVAILABLE);
        assert_eq!(usage.tokens.total, 0);
    }

    #[test]
    fn non_ascii_cwd_is_unavailable() {
        // A cwd carrying a non-ASCII char must degrade BEFORE any mangling/fs access:
        // our "non-alphanumeric -> '-'" rule may not match Claude Code's encoding of
        // non-ASCII, and a mismatched dir could resolve to a DIFFERENT project's
        // transcript -> wrong tokens. Safe degrade is unavailable.
        let usage = usage_from_cwd(r"C:\Users\café\Desktop\Progetto", None);
        assert_eq!(usage.source, SOURCE_UNAVAILABLE);
        assert_eq!(usage.tokens.total, 0);
    }

    /// Serializes the tests that mutate the process-wide home env var so they do not
    /// clobber each other (or other tests) under the parallel test runner.
    static HOME_ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn found_but_empty_transcript_is_unavailable_not_zero_badge() {
        // A transcript dir + newest .jsonl EXIST but carry no `usage` objects
        // (total == 0). The badge must HIDE (unavailable), not show a misleading
        // "0 tok · $0.00" claude-transcript badge.
        let _guard = HOME_ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // ASCII-only cwd (so it survives the non-ASCII gate) with a known mangling.
        let cwd = r"C:\aspis\toktest\empty-xyz";
        let mangled = mangle_cwd_to_project_dir(cwd);

        let home = std::env::temp_dir().join(format!(
            "aspis-tokhome-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let project_dir = home.join(".claude").join("projects").join(&mangled);
        fs::create_dir_all(&project_dir).unwrap();
        {
            // A non-empty JSONL with lines that carry NO usage object: total stays 0.
            let mut f = File::create(project_dir.join("session.jsonl")).unwrap();
            f.write_all(br#"{"type":"user","message":{"role":"user","content":"hi"}}"#)
                .unwrap();
            f.write_all(b"\n").unwrap();
            f.write_all(
                br#"{"type":"assistant","message":{"model":"claude-opus-4-8","content":[{"type":"text","text":"hello"}]}}"#,
            )
            .unwrap();
            f.write_all(b"\n").unwrap();
        }

        // Point the home resolution at our fixture. Save + restore both vars.
        let prev_userprofile = std::env::var_os("USERPROFILE");
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("USERPROFILE", &home);
        std::env::set_var("HOME", &home);

        let usage = usage_from_cwd(cwd, None);

        match prev_userprofile {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        let _ = fs::remove_dir_all(&home);

        assert_eq!(usage.source, SOURCE_UNAVAILABLE);
        assert_eq!(usage.tokens.total, 0);
        assert_eq!(usage.cost_usd, None);
    }

    #[test]
    fn ignores_a_transcript_written_before_the_agent_launched() {
        // BUG #18: the transcript dir is keyed by PROJECT cwd, so newest_jsonl can
        // resolve to a DIFFERENT or earlier agent's session. A transcript last
        // written BEFORE this agent launched cannot be its work — the badge must not
        // borrow another session's numbers; it degrades to unavailable.
        let _guard = HOME_ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let cwd = r"C:\aspis\toktest\attribution-xyz";
        let mangled = mangle_cwd_to_project_dir(cwd);
        let home = std::env::temp_dir().join(format!(
            "aspis-tokhome-attr-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let project_dir = home.join(".claude").join("projects").join(&mangled);
        fs::create_dir_all(&project_dir).unwrap();
        {
            let mut f = File::create(project_dir.join("session.jsonl")).unwrap();
            f.write_all(
                br#"{"type":"assistant","message":{"model":"claude-opus-4-8","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#,
            )
            .unwrap();
            f.write_all(b"\n").unwrap();
        }

        let prev_userprofile = std::env::var_os("USERPROFILE");
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("USERPROFILE", &home);
        std::env::set_var("HOME", &home);

        // launched_after in the FUTURE: the just-written transcript predates it, so
        // it cannot be this agent's -> unavailable (no borrowed numbers).
        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
        let filtered = usage_from_cwd(cwd, Some(future));
        // launched_after in the PAST: the transcript was written AFTER launch, so it
        // IS this agent's -> attributed (exercises the filter-PASS branch).
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let recent = usage_from_cwd(cwd, Some(past));
        // No launch time -> no filter -> the transcript is attributed normally.
        let attributed = usage_from_cwd(cwd, None);

        match prev_userprofile {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        let _ = fs::remove_dir_all(&home);

        assert_eq!(filtered.source, SOURCE_UNAVAILABLE);
        assert_eq!(recent.source, SOURCE_CLAUDE_TRANSCRIPT);
        assert_eq!(attributed.source, SOURCE_CLAUDE_TRANSCRIPT);
        assert!(attributed.tokens.total > 0);
    }

    #[test]
    fn tail_bound_is_respected_and_drops_partial_seam_line() {
        // Build a file larger than the tail cap: a leading junk block, then a clean
        // newline-terminated usage line at the very end. Reading the tail must NOT
        // return the leading block, must drop the partial first line at the seam,
        // and must still parse the trailing usage line.
        let dir = std::env::temp_dir().join(format!(
            "aspis-toktail-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        {
            let mut f = File::create(&path).unwrap();
            // > tail cap of leading filler (no newlines so it is one huge partial line).
            let filler = "x".repeat((MAX_TRANSCRIPT_TAIL_BYTES as usize) + 1024);
            f.write_all(filler.as_bytes()).unwrap();
            f.write_all(b"\n").unwrap();
            f.write_all(
                br#"{"message":{"model":"claude-sonnet-4","usage":{"input_tokens":7,"output_tokens":3,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#,
            )
            .unwrap();
            f.write_all(b"\n").unwrap();
        }
        let body = read_tail_bounded(&path).expect("tail read");
        // The returned body must be at most the cap, and must NOT contain the filler.
        assert!(body.len() as u64 <= MAX_TRANSCRIPT_TAIL_BYTES);
        let parsed = sum_usage_from_jsonl(&body);
        assert_eq!(parsed.tokens.input, 7);
        assert_eq!(parsed.tokens.output, 3);
        let _ = fs::remove_dir_all(&dir);
    }

    /// PRIVACY GUARD: feed a transcript whose message content carries a unique
    /// secret marker, then assert the marker appears NOWHERE in the parsed struct
    /// or its serialized JSON. Proves the parser keeps ONLY numeric usage + the
    /// short model id, never message text / tool I/O.
    #[test]
    fn privacy_no_message_content_in_output() {
        const SECRET: &str = "TOPSECRET_TRANSCRIPT_CONTENT_DO_NOT_LEAK_42";
        let body = format!(
            concat!(
                r#"{{"type":"user","message":{{"role":"user","content":"{secret}"}}}}"#,
                "\n",
                r#"{{"type":"assistant","message":{{"model":"claude-opus-4-8","content":[{{"type":"text","text":"{secret}"}}],"usage":{{"input_tokens":11,"output_tokens":22,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}}}}"#,
                "\n",
                r#"{{"type":"tool_result","toolUseResult":{{"stdout":"{secret}"}}}}"#,
                "\n",
            ),
            secret = SECRET
        );
        let parsed = sum_usage_from_jsonl(&body);
        // Numbers were extracted correctly.
        assert_eq!(parsed.tokens.input, 11);
        assert_eq!(parsed.tokens.output, 22);
        // The model id (allowed) is present; the SECRET content is NOT.
        assert_eq!(parsed.model.as_deref(), Some("claude-opus-4-8"));
        assert!(
            !parsed.model.as_deref().unwrap_or_default().contains(SECRET),
            "model field leaked content"
        );
        // Build the full IPC reply and serialize it: the secret must be absent.
        let usage = AgentTokenUsage {
            tokens: parsed.tokens,
            cost_usd: cost_for(&parsed.tokens, parsed.model.as_deref()),
            source: SOURCE_CLAUDE_TRANSCRIPT.into(),
        };
        let json = serde_json::to_string(&usage).unwrap();
        assert!(
            !json.contains(SECRET),
            "serialized usage leaked transcript content: {json}"
        );
        // And the debug rendering (what a stray eprintln! would emit) is clean too.
        let debug = format!("{usage:?}");
        assert!(!debug.contains(SECRET), "debug render leaked content");
    }
}
