//! Port of `oracle/ingestion/retrieval_text.py` — byte-parity with the live
//! Python pipeline including frozen production bugs.
//!
//! Every public function mirrors its Python counterpart. The `ChunkMeta`
//! struct holds the dict keys consumed by `chunk_embedding_text()`.
//! `symbols_used` is kept as a raw `String` to replicate the char-iteration
//! bug (see `golden/README.md` § FROZEN PRODUCTION BUGS).

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;

use crate::config;

// ═══════════════════════════════════════════════════════════════════════════
// ChunkMeta — mirrors the dict keys read by chunk_embedding_text()
// ═══════════════════════════════════════════════════════════════════════════

/// Metadata for a single chunk, matching the dict keys consumed by the
/// Python `chunk_embedding_text()` function.
#[derive(Debug, Clone)]
pub struct ChunkMeta {
    pub file_id: String,
    pub file_sorgente: String,
    pub text: String,
    pub kind: String,
    pub symbol_name: String,
    pub language: String,
    pub line_start: i64,
    pub line_end: i64,
    /// Raw `symbols_used` value — kept as a `String` to replicate the
    /// frozen char-iteration bug (Python iterates the JSON string's chars
    /// instead of parsing it as a list).
    pub symbols_used: String,
    pub chunk_index: usize,
    pub id: String,
}

// ═══════════════════════════════════════════════════════════════════════════
// Path helpers — replicate pathlib.Path behaviour on POSIX-style strings
// ═══════════════════════════════════════════════════════════════════════════

/// `Path(source).name` — the final component of a POSIX path.
pub fn path_name(source: &str) -> &str {
    source.rsplit('/').next().unwrap_or(source)
}

/// `Path(source).suffix` — the last dot-segment of the name.
/// Returns `""` when the name starts with a dot and has no other dot,
/// matching Python's `pathlib.Path.suffix`.
pub fn path_suffix(source: &str) -> &str {
    let name = path_name(source);
    suffix_of_name(name)
}

/// `Path(source).stem` — name without the last suffix.
pub fn path_stem(source: &str) -> &str {
    let name = path_name(source);
    let sfx = suffix_of_name(name);
    if sfx.is_empty() {
        name
    } else {
        &name[..name.len() - sfx.len()]
    }
}

/// Internal: suffix from a bare name (no directory components).
fn suffix_of_name(name: &str) -> &str {
    if let Some(dot_pos) = name.rfind('.') {
        if dot_pos == 0 {
            // Name starts with dot — suffix is empty unless there's another dot.
            // e.g. ".env" -> "", ".env.local" -> ".local"
            if name[1..].contains('.') {
                return &name[dot_pos..];
            }
            return "";
        }
        &name[dot_pos..]
    } else {
        ""
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Regex patterns (compiled once via OnceLock)
// ═══════════════════════════════════════════════════════════════════════════

fn symbol_patterns() -> &'static [Regex; 7] {
    static PATTERNS: OnceLock<[Regex; 7]> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            // JS/TS function
            Regex::new(r"\b(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)").unwrap(),
            // JS/TS const/let/var
            Regex::new(r"\b(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=").unwrap(),
            // JS/TS class
            Regex::new(r"\b(?:export\s+)?class\s+([A-Za-z_$][\w$]*)").unwrap(),
            // Rust fn
            Regex::new(r"\b(?:pub\s+)?fn\s+([A-Za-z_][\w]*)").unwrap(),
            // Rust struct/enum/trait
            Regex::new(r"\b(?:pub\s+)?(?:struct|enum|trait)\s+([A-Za-z_][\w]*)").unwrap(),
            // Python def
            Regex::new(r"\bdef\s+([A-Za-z_][\w]*)\s*\(").unwrap(),
            // Python/other class
            Regex::new(r"\bclass\s+([A-Za-z_][\w]*)\s*[:\(]").unwrap(),
        ]
    })
}

fn route_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"['"](/(?:api/|workers/|artifacts/|outputs/|jobs/|projects/)[^'"\s)]+)"#)
            .unwrap()
    })
}

fn mcp_tool_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(?:oracle|project|cloudflare|scaleway)_[a-z0-9_]+\b").unwrap()
    })
}

fn tag_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)@([a-z0-9_-]+)\(([^)]+)\)").unwrap())
}

// ═══════════════════════════════════════════════════════════════════════════
// Profile helpers (delegate to crate::config)
// ═══════════════════════════════════════════════════════════════════════════

/// Whether the semantic-prefix-v2 profile is active for the given override.
fn semantic_prefix_enabled(profile: Option<&str>) -> bool {
    config::active_chunk_profile_version(profile) == config::SEMANTIC_PREFIX_PROFILE_VERSION
}

// ═══════════════════════════════════════════════════════════════════════════
// classify_source_kind
// ═══════════════════════════════════════════════════════════════════════════

pub fn classify_source_kind(source: &str) -> String {
    let lower = source.to_lowercase();
    if lower.contains("/tests/")
        || lower.ends_with(".test.js")
        || lower.ends_with(".test.ts")
        || lower.ends_with(".spec.js")
        || lower.ends_with(".spec.ts")
        || lower.ends_with("_test.py")
    {
        return "test_regression_secondary".to_string();
    }
    if lower.ends_with(".md")
        || lower.ends_with(".txt")
        || lower.ends_with(".rmd")
        || lower.contains("/docs/")
        || lower.contains("roadmap")
        || lower.contains("handoff")
    {
        return "documentation_or_plan_secondary".to_string();
    }
    if lower.contains("/dist/")
        || lower.contains("/build/")
        || lower.contains("/coverage/")
        || lower.contains(".min.js")
        || lower.contains(".bundle.js")
    {
        return "generated_low_priority".to_string();
    }
    if lower.ends_with(".js")
        || lower.ends_with(".jsx")
        || lower.ends_with(".ts")
        || lower.ends_with(".tsx")
        || lower.ends_with(".mjs")
        || lower.ends_with(".py")
        || lower.ends_with(".rs")
        || lower.ends_with(".kt")
        || lower.ends_with(".java")
        || lower.ends_with(".r")
        || lower.ends_with(".sh")
        || lower.ends_with(".ps1")
    {
        return "implementation_primary".to_string();
    }
    "structured_config".to_string()
}

// ═══════════════════════════════════════════════════════════════════════════
// priority_hint
// ═══════════════════════════════════════════════════════════════════════════

pub fn priority_hint(source_kind: &str) -> &'static str {
    match source_kind {
        "implementation_primary" => "prefer_for_how_where_which_implementation_questions",
        "test_regression_secondary" => "use_when_query_asks_tests_or_regressions",
        "documentation_or_plan_secondary" => "use_when_query_asks_plans_docs_status_or_rationale",
        "generated_low_priority" => "avoid_unless_query_explicitly_asks_generated_build_output",
        _ => "use_for_config_and_schema_questions",
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// classify_domains — 13 domain classifiers in exact insertion order
// ═══════════════════════════════════════════════════════════════════════════

pub fn classify_domains(source: &str, text: &str) -> Vec<String> {
    let haystack = format!("{}\n{}", source, text).to_lowercase();
    let mut domains: Vec<String> = Vec::new();

    let mut add = |name: &str, needles: &[&str]| {
        if needles.iter().any(|n| haystack.contains(n)) && !domains.iter().any(|d| d == name) {
            domains.push(name.to_string());
        }
    };

    add(
        "rnaseq_output_release",
        &[
            "output_renders",
            "artifact_url",
            "manifest_url",
            "downloadrenderedartifact",
            "outputs/render",
        ],
    );
    add(
        "rnaseq_browser_upload",
        &[
            "browseruploadsession",
            "createbrowseruploadsession",
            "completebrowseruploadfile",
            "rna_upload_sessions",
        ],
    );
    add(
        "rnaseq_scaleway_lifecycle",
        &[
            "cleanupscalewayinstanceafterterminal",
            "terminatescalewayinstance",
            "releasescalewayinstanceslot",
            "scaleway.mjs",
        ],
    );
    add(
        "cloudflare_worker_secret_rotation",
        &[
            "rotate_cloudflare_worker_secret",
            "put_cloudflare_worker_secret",
            "worker secret",
            "workers scripts write",
        ],
    );
    add(
        "cloudflare_provider_console",
        &["cloudflare", "workers", "routes", "zone", "account_id"],
    );
    add(
        "scaleway_provider_console",
        &[
            "scaleway",
            "instance",
            "serverless",
            "commercial_type",
            "project_id",
        ],
    );
    add(
        "oracle_indexing",
        &[
            "index_file_chunks",
            "chunk_index_status",
            "lancedb",
            "qwen3-embedding",
            "chunk-profile",
        ],
    );
    add(
        "oracle_answering",
        &[
            "answer_from_context",
            "queryengine",
            "oracle_ask",
            "oracle_context",
        ],
    );
    add(
        "oracle_mcp_agents",
        &[
            "mcp",
            "project_claim_task",
            "project_update_status",
            "create_mcp_server",
        ],
    );
    add(
        "projects_mini_notion",
        &[
            "projectsview",
            "kanban",
            "project.md",
            "project_claim_task",
            "agent claims",
        ],
    );
    add(
        "windows_hello_auth",
        &["windows hello", "biometric", "webcam", "pin", "unlock"],
    );
    add(
        "provider_privacy",
        &[
            "zdr",
            "gdpr",
            "infomaniak",
            "mistral",
            "openrouter",
            "scaleway",
        ],
    );
    add(
        "gpu_cpu_lifecycle",
        &[
            "gpu",
            "cpu",
            "vm",
            "egpu",
            "terminate",
            "delete",
            "scale-to-zero",
        ],
    );

    domains
}

// ═══════════════════════════════════════════════════════════════════════════
// Symbol helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Replace runs of whitespace with a single space, after trimming.
/// Equivalent to Python `re.sub(r"\s+", " ", value.strip())`.
fn normalize_whitespace(s: &str) -> String {
    let trimmed = s.trim();
    let mut result = String::new();
    let mut prev_space = false;
    for c in trimmed.chars() {
        if c.is_whitespace() {
            if !prev_space {
                result.push(' ');
                prev_space = true;
            }
        } else {
            result.push(c);
            prev_space = false;
        }
    }
    result
}

/// Add a symbol to the list, deduplicating by lowercase key and
/// truncating to 160 characters.  Mirrors Python `add_symbol()`.
fn add_symbol(symbols: &mut Vec<String>, seen: &mut HashSet<String>, value: &str) {
    let clean = normalize_whitespace(value);
    if clean.is_empty() {
        return;
    }
    let key = clean.to_lowercase();
    if seen.contains(&key) {
        return;
    }
    seen.insert(key);
    let truncated: String = clean.chars().take(160).collect();
    symbols.push(truncated);
}

// ═══════════════════════════════════════════════════════════════════════════
// extract_routes
// ═══════════════════════════════════════════════════════════════════════════

pub fn extract_routes(text: &str) -> Vec<String> {
    let mut routes: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let pat = route_pattern();
    for cap in pat.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            add_symbol(&mut routes, &mut seen, m.as_str());
        }
    }
    routes
}

// ═══════════════════════════════════════════════════════════════════════════
// extract_symbols
// ═══════════════════════════════════════════════════════════════════════════

pub fn extract_symbols(source: &str, text: &str) -> Vec<String> {
    let mut symbols: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // 1. SYMBOL_PATTERNS (7 patterns, exact order)
    for pat in symbol_patterns() {
        for cap in pat.captures_iter(text) {
            if let Some(m) = cap.get(1) {
                add_symbol(&mut symbols, &mut seen, m.as_str());
            }
        }
    }
    // 2. Routes
    for route in extract_routes(text) {
        add_symbol(&mut symbols, &mut seen, &route);
    }
    // 3. MCP tools (case-insensitive)
    let mcp = mcp_tool_pattern();
    for m in mcp.find_iter(text) {
        add_symbol(&mut symbols, &mut seen, m.as_str());
    }
    // 4. Tags
    let tag = tag_pattern();
    for cap in tag.captures_iter(text) {
        let key = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let value = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let formatted = format!("@{}({})", key, value);
        add_symbol(&mut symbols, &mut seen, &formatted);
    }
    // 5. File basename (stem)
    let basename = path_stem(source);
    if !basename.is_empty() {
        add_symbol(&mut symbols, &mut seen, basename);
    }
    symbols
}

// ═══════════════════════════════════════════════════════════════════════════
// question_templates
// ═══════════════════════════════════════════════════════════════════════════

pub fn question_templates(domains: &[String], source: &str, symbols: &[String]) -> Vec<String> {
    let mut questions: Vec<String> = Vec::new();

    for domain in domains {
        match domain.as_str() {
            "rnaseq_output_release" => {
                questions.push(
                    "How does RNA-seq release outputs after a successful run in the browser?"
                        .to_string(),
                );
                questions.push(
                    "Where are RNA-seq artifact URLs, manifest URLs, and rendered outputs created?"
                        .to_string(),
                );
            }
            "rnaseq_browser_upload" => {
                questions.push(
                    "Which files implement RNA-seq browser upload sessions and completion?"
                        .to_string(),
                );
                questions.push(
                    "Where is safe upload completion handled for RNA-seq inputs?".to_string(),
                );
            }
            "rnaseq_scaleway_lifecycle" => {
                questions.push(
                    "Which files control RNA-seq Scaleway instance lifecycle and terminal cleanup?"
                        .to_string(),
                );
                questions.push(
                    "Where are Scaleway instances terminated or released after RNA-seq jobs?"
                        .to_string(),
                );
            }
            "cloudflare_worker_secret_rotation" => {
                questions
                    .push("Where is Cloudflare Worker secret rotation implemented?".to_string());
                questions.push(
                    "Which code writes Cloudflare Worker secrets and validates rotation requests?"
                        .to_string(),
                );
            }
            "scaleway_provider_console" => {
                questions.push(
                    "Which files implement the Scaleway dashboard, instances, containers, and project scope?"
                        .to_string(),
                );
                questions.push(
                    "Where are Scaleway CPU GPU VM terminate and delete actions controlled?"
                        .to_string(),
                );
            }
            "cloudflare_provider_console" => {
                questions.push(
                    "Which files implement Cloudflare Workers inventory, routes, and secret management?"
                        .to_string(),
                );
            }
            "oracle_indexing" => {
                questions.push(
                    "How does Oracle chunk, embed, index, and refresh LanceDB records?".to_string(),
                );
                questions.push("Where is incremental Oracle indexing implemented?".to_string());
            }
            "oracle_answering" => {
                questions.push(
                    "How does Oracle retrieve context and answer questions from chunks?"
                        .to_string(),
                );
            }
            "oracle_mcp_agents" => {
                questions.push(
                    "How can CLI agents call Oracle and update project status through MCP?"
                        .to_string(),
                );
                questions.push(
                    "Which MCP tools let agents read projects, claim tasks, and update status?"
                        .to_string(),
                );
            }
            "projects_mini_notion" => {
                questions.push(
                    "Which files implement the mini Notion Projects Kanban and agent claims?"
                        .to_string(),
                );
            }
            "windows_hello_auth" => {
                questions.push(
                    "Which files control Windows Hello PIN webcam fingerprint unlock behavior?"
                        .to_string(),
                );
            }
            "provider_privacy" => {
                questions.push(
                    "Which Oracle LLM providers are allowed for GDPR ZDR and where are they configured?"
                        .to_string(),
                );
            }
            _ => {} // gpu_cpu_lifecycle and any unknown domains produce no questions
        }
    }

    if let Some(first) = symbols.first() {
        questions.push(format!("Where is {} implemented or referenced?", first));
    }
    if !source.is_empty() {
        questions.push(format!("What does {} do?", source));
    }

    // Deduplicate by lowercase key, preserving first-occurrence order.
    let mut deduped: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for q in &questions {
        let key = q.to_lowercase();
        if seen.insert(key) {
            deduped.push(q.clone());
        }
    }
    deduped
}

// ═══════════════════════════════════════════════════════════════════════════
// chunk_embedding_text
// ═══════════════════════════════════════════════════════════════════════════

pub fn chunk_embedding_text(chunk: &ChunkMeta, profile: Option<&str>) -> String {
    if !semantic_prefix_enabled(profile) {
        return format!("{}\n{}", chunk.file_id, chunk.text);
    }

    // source: file_id fallback to file_sorgente (Python: chunk.get("file_id") or chunk.get("file_sorgente") or "")
    let source = if !chunk.file_id.is_empty() {
        chunk.file_id.clone()
    } else {
        chunk.file_sorgente.clone()
    };
    let text = &chunk.text;
    let domains = classify_domains(&source, text);
    let symbols = extract_symbols(&source, text);
    let routes = extract_routes(text);
    let source_kind = classify_source_kind(&source);
    let questions = question_templates(&domains, &source, &symbols);

    let chunk_kind = &chunk.kind;
    let symbol_name = &chunk.symbol_name;
    let chunk_lang = &chunk.language;
    let line_range = if chunk.line_start != 0 {
        format!("L{}-L{}", chunk.line_start, chunk.line_end)
    } else {
        String::new()
    };
    let symbols_used = &chunk.symbols_used;

    let mut header: Vec<String> = Vec::new();

    // Required fields (always present)
    header.push("TASK: retrieve Aspis Bio and Aspis Management code/docs chunks that answer architecture, implementation, cloud, oracle, and project-management questions.".to_string());
    header.push(format!("SOURCE_PATH: {}", source));
    header.push(format!("FILE_NAME: {}", path_name(&source)));
    {
        let ext = path_suffix(&source).to_lowercase();
        if ext.is_empty() {
            header.push("EXTENSION: none".to_string());
        } else {
            header.push(format!("EXTENSION: {}", ext));
        }
    }
    header.push(format!("SOURCE_KIND: {}", source_kind));
    header.push(format!("PRIORITY_HINT: {}", priority_hint(&source_kind)));
    if domains.is_empty() {
        header.push("DOMAIN_TAGS: general".to_string());
    } else {
        header.push(format!("DOMAIN_TAGS: {}", domains.join(", ")));
    }

    // Conditional fields (mirrors Python if-blocks exactly)
    if !chunk_kind.is_empty() {
        header.push(format!("CHUNK_KIND: {}", chunk_kind));
    }
    if !symbol_name.is_empty() {
        header.push(format!("SYMBOL_NAME: {}", symbol_name));
    }
    if !chunk_lang.is_empty() {
        header.push(format!("LANGUAGE: {}", chunk_lang));
    }
    if !line_range.is_empty() && line_range != "L0-L0" {
        header.push(format!("LINE_RANGE: {}", line_range));
    }
    if !symbols.is_empty() {
        let limited: Vec<&str> = symbols.iter().take(40).map(|s| s.as_str()).collect();
        header.push(format!("SYMBOLS: {}", limited.join(", ")));
    }

    // ── FROZEN BUG: symbols_used is a JSON string, iterate chars ──
    // Python: `for s in symbols_used:` where symbols_used is e.g. `'["Optional", "Path", "os"]'`
    // This iterates individual characters, producing garbled REFERENCES lines.
    if !symbols_used.is_empty() {
        let stem = path_stem(&source);
        let used: Vec<String> = symbols_used
            .chars()
            .map(|c| c.to_string())
            .filter(|s| s != symbol_name && s.as_str() != stem)
            .collect();
        if !used.is_empty() {
            let limited: Vec<&str> = used.iter().take(20).map(|s| s.as_str()).collect();
            header.push(format!("REFERENCES: {}", limited.join(", ")));
        }
    }

    if !routes.is_empty() {
        let limited: Vec<&str> = routes.iter().take(30).map(|s| s.as_str()).collect();
        header.push(format!("ROUTES_APIS: {}", limited.join(", ")));
    }

    if !questions.is_empty() {
        header.push("QUESTIONS_THIS_CHUNK_CAN_ANSWER:".to_string());
        for q in questions.iter().take(10) {
            header.push(format!("- {}", q));
        }
    }

    header.push("RAW_CHUNK:".to_string());
    header.push(text.clone());

    header.join("\n")
}

// ═══════════════════════════════════════════════════════════════════════════
// query_embedding_text
// ═══════════════════════════════════════════════════════════════════════════

pub fn query_embedding_text(query: &str, profile: Option<&str>) -> String {
    if !semantic_prefix_enabled(profile) {
        return query.to_string();
    }
    let domains = classify_domains("", query);
    let mut lines: Vec<String> = Vec::new();
    lines.push("TASK: retrieve Aspis Bio and Aspis Management code/docs chunks that answer architecture, implementation, cloud, oracle, and project-management questions.".to_string());
    lines.push(format!("QUERY: {}", query));
    if !domains.is_empty() {
        lines.push(format!("QUERY_DOMAIN_TAGS: {}", domains.join(", ")));
    }
    lines.join("\n")
}

// ═══════════════════════════════════════════════════════════════════════════
// Unit tests for path helpers
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_name() {
        assert_eq!(path_name("src/app.py"), "app.py");
        assert_eq!(path_name("build/keep.md"), "keep.md");
        assert_eq!(path_name(".env"), ".env");
        assert_eq!(path_name("app.py"), "app.py");
    }

    #[test]
    fn test_path_suffix() {
        assert_eq!(path_suffix("src/app.py"), ".py");
        assert_eq!(path_suffix("data/config.json"), ".json");
        assert_eq!(path_suffix("src/components/App.tsx"), ".tsx");
        assert_eq!(path_suffix(".env"), "");
        assert_eq!(path_suffix("Makefile"), "");
        assert_eq!(path_suffix("archive.tar.gz"), ".gz");
    }

    #[test]
    fn test_path_stem() {
        assert_eq!(path_stem("src/app.py"), "app");
        assert_eq!(path_stem("data/config.json"), "config");
        assert_eq!(path_stem(".env"), ".env");
        assert_eq!(path_stem("archive.tar.gz"), "archive.tar");
    }
}
