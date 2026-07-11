//! Verbatim port of the Oracle lexical scoring stack from Python.
//!
//! Ported from `oracle/server/query_engine.py` with golden-verified parity.
//! All scoring uses f64 throughout, matching Python's float semantics.
//! Accumulation order matches Python to preserve float equality.

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const STOPWORDS: &[&str] = &[
    "about", "after", "and", "are", "can", "does", "for", "from", "how", "into", "the", "this",
    "that", "what", "when", "where", "which", "with",
];

// ---------------------------------------------------------------------------
// Tokenization regex (compiled once via OnceLock)
// ---------------------------------------------------------------------------

fn token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[a-z0-9_/-]+").unwrap())
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn ends_with_any(s: &str, suffixes: &[&str]) -> bool {
    suffixes.iter().any(|suffix| s.ends_with(suffix))
}

fn starts_with_any(s: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| s.starts_with(prefix))
}

// ---------------------------------------------------------------------------
// ScoredChunk — input type for the scorer (mirrors the Python chunk dict)
// ---------------------------------------------------------------------------

/// A chunk ready for lexical scoring.  Fields correspond exactly to the
/// Python `chunk.get(...)` calls in `query_engine.py`.
#[derive(Debug, Clone, Deserialize)]
pub struct ScoredChunk {
    pub id: String,
    #[serde(default)]
    pub file_id: String,
    #[serde(default)]
    pub file_sorgente: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub chunk_index: usize,
    #[serde(default)]
    pub start_char: usize,
    #[serde(default)]
    pub end_char: usize,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub symbol_name: String,
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub line_start: usize,
    #[serde(default)]
    pub line_end: usize,
    #[serde(default)]
    pub symbols_used: String,
    #[serde(default)]
    pub area: String,
    #[serde(default)]
    pub cluster_semantic: String,
    #[serde(default)]
    pub label: String,
}

// ---------------------------------------------------------------------------
// ChunkContextPayload — output of lexical_chunk_context
// ---------------------------------------------------------------------------

/// Result payload produced by [`lexical_chunk_context`].
/// Mirrors the Python `chunk_context_payload()` return dict.
#[derive(Debug, Clone)]
pub struct ChunkContextPayload {
    pub chunk_id: String,
    pub file_source: String,
    pub chunk_index: usize,
    pub start_char: usize,
    pub end_char: usize,
    pub score: f64,
    pub retrieval: String,
    pub text: String,
    pub last_modified: String,
    pub kind: String,
    pub symbol_name: String,
    pub signature: String,
    pub language: String,
    pub line_start: usize,
    pub line_end: usize,
    pub symbols_used: String,
}

// ---------------------------------------------------------------------------
// query_terms — tokenization + stopword filtering
// ---------------------------------------------------------------------------

/// Extract query terms: lowercase, regex tokenise `[a-z0-9_/-]+`,
/// drop tokens shorter than 3 chars and stopwords.
pub fn query_terms(query: &str) -> HashSet<String> {
    let lower = query.to_lowercase();
    token_re()
        .find_iter(&lower)
        .map(|m| m.as_str().to_string())
        .filter(|term| term.len() >= 3 && !STOPWORDS.contains(&term.as_str()))
        .collect()
}

// ---------------------------------------------------------------------------
// semantic_expansions — hand-built synonym map (every entry byte-exact)
// ---------------------------------------------------------------------------

/// Expand query terms into semantic synonyms.  Condition order matches
/// the Python source exactly.
pub fn semantic_expansions(terms: &HashSet<String>) -> HashSet<String> {
    let mut expanded = HashSet::new();

    if ["limit", "limits", "limiting", "limited"]
        .iter()
        .any(|t| terms.contains(*t))
    {
        expanded.extend(
            [
                "cap",
                "caps",
                "control",
                "controls",
                "max_scale",
                "min_scale",
                "scale-to-zero",
            ]
            .map(str::to_string),
        );
    }
    if ["spawn", "spawning"].iter().any(|t| terms.contains(*t)) {
        expanded.extend(
            [
                "provision",
                "provisioning",
                "create",
                "creation",
                "cold start",
                "scale-to-zero",
            ]
            .map(str::to_string),
        );
    }
    if terms.contains("gpu") {
        expanded.extend(["l4", "cuda", "vram"].map(str::to_string));
    }
    if ["rna-seq", "rnaseq", "rna"]
        .iter()
        .any(|t| terms.contains(*t))
    {
        expanded.extend(["rnaseq", "rna-seq", "aspis-rna-seq"].map(str::to_string));
    }
    if [
        "output", "outputs", "result", "results", "release", "download",
    ]
    .iter()
    .any(|t| terms.contains(*t))
    {
        expanded.extend(
            [
                "output_renders",
                "artifact_url",
                "manifest_url",
                "outputs/render",
                "rendered_outputs",
                "artifact",
            ]
            .map(str::to_string),
        );
    }
    if ["successful", "success", "completed", "complete"]
        .iter()
        .any(|t| terms.contains(*t))
    {
        expanded.extend(["done", "ready", "results ready", "terminal"].map(str::to_string));
    }
    if terms.contains("browser") {
        expanded.extend(
            [
                "download",
                "/artifacts/",
                "artifact_url",
                "job_views",
                "public",
            ]
            .map(str::to_string),
        );
    }
    if ["upload", "uploads", "session", "sessions", "completion"]
        .iter()
        .any(|t| terms.contains(*t))
    {
        expanded.extend(
            [
                "createbrowseruploadsession",
                "completebrowseruploadfile",
                "completebrowseruploadsession",
                "browser upload",
            ]
            .map(str::to_string),
        );
    }
    if ["terminal", "cleanup", "lifecycle", "instance", "instances"]
        .iter()
        .any(|t| terms.contains(*t))
    {
        expanded.extend(
            [
                "cleanupscalewayinstanceafterterminal",
                "terminatescalewayinstance",
                "releasescalewayinstanceslot",
            ]
            .map(str::to_string),
        );
    }
    if ["privacy", "private", "safe", "zdr", "gdpr"]
        .iter()
        .any(|t| terms.contains(*t))
    {
        expanded.extend(
            [
                "zdr",
                "gdpr",
                "zero data retention",
                "allowed provider",
                "scaleway",
                "infomaniak",
                "mistral",
            ]
            .map(str::to_string),
        );
    }
    if [
        "agent", "agents", "terminal", "task", "tasks", "finished", "done",
    ]
    .iter()
    .any(|t| terms.contains(*t))
    {
        expanded.extend(
            [
                "project_claim_task",
                "project_update_status",
                "oracle_ask",
                "oracle_context",
                "read_project",
            ]
            .map(str::to_string),
        );
    }
    if ["paid", "stop", "stops", "cleanup", "resources", "resource"]
        .iter()
        .any(|t| terms.contains(*t))
    {
        expanded.extend(
            [
                "cleanupscalewayinstanceafterterminal",
                "terminatescalewayinstance",
                "delete",
                "with_volumes=all",
                "release",
            ]
            .map(str::to_string),
        );
    }

    expanded
}

// ===================================================================
// Domain bonus functions — ported VERBATIM, same order, same weights
// ===================================================================

/// Domain mechanism bonus (GPU/Scaleway spawn + limits).
fn domain_mechanism_bonus(query: &str, terms: &HashSet<String>, source: &str, text: &str) -> f64 {
    if !((terms.contains("scaleway") && terms.contains("gpu"))
        && ["spawn", "spawning", "limit", "limits"]
            .iter()
            .any(|t| terms.contains(*t)))
    {
        return 0.0;
    }
    let mut bonus = 0.0_f64;
    let answer_signals = [
        "scale-to-zero",
        "min_scale=0",
        "no gpu required",
        "serverless containers",
        "max_scale",
        "cpu specialists",
        "billing stops",
        "delete after",
    ];
    for signal in &answer_signals {
        if text.contains(signal) {
            bonus += 1.25;
        }
    }
    if source.contains("scaleway") {
        bonus += 0.75;
    }
    if source.contains("biovision") {
        bonus += 0.5;
    }
    if query.to_lowercase().contains("how") && text.contains("open questions") {
        bonus -= 3.0;
    }
    bonus
}

/// Source quality bonus — gating rule: only added when score+domain_bonus > 0.
fn source_quality_bonus(query: &str, terms: &HashSet<String>, source: &str) -> f64 {
    let q = query.to_lowercase();
    let asks_for_tests = ["test", "tests", "spec", "coverage", "regression"]
        .iter()
        .any(|t| terms.contains(*t));
    let asks_for_plan = [
        "plan",
        "plans",
        "roadmap",
        "proposal",
        "handoff",
        "docs",
        "documentation",
    ]
    .iter()
    .any(|t| terms.contains(*t));
    let asks_for_implementation = q.contains("how")
        || [
            "where",
            "which",
            "control",
            "controls",
            "release",
            "download",
            "outputs",
            "result",
            "results",
            "lifecycle",
            "provider",
            "worker",
            "scaleway",
            "cloudflare",
            "oracle",
        ]
        .iter()
        .any(|t| terms.contains(*t));

    let mut bonus = 0.0_f64;

    let real_source_prefixes = [
        "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/",
        "aspis-lab/compute/tier1/",
        "aspis-lab/cloudflare/orasis/src/",
        "aspis-biovision/src/",
        "src-tauri/src/",
        "src/",
        "cloudflare/workers/",
        "oracle/",
    ];
    if starts_with_any(source, &real_source_prefixes) {
        bonus += 3.0;
        if asks_for_implementation {
            bonus += 4.0;
        }
    }

    if source.starts_with("oracle/evals/")
        && !["eval", "evals", "benchmark", "bakeoff", "smoke"]
            .iter()
            .any(|t| terms.contains(*t))
    {
        bonus -= 12.0;
    }
    if source.ends_with("oracle/ingestion/retrieval_text.py")
        && ![
            "embedding",
            "embeddings",
            "prefix",
            "prefixes",
            "profile",
            "profiles",
            "retrieval",
            "semantic",
            "taxonomy",
            "chunk",
            "chunks",
        ]
        .iter()
        .any(|t| terms.contains(*t))
    {
        bonus -= 18.0;
    }
    if source.ends_with("oracle/server/query_engine.py")
        && ![
            "ranking",
            "retrieval",
            "queryengine",
            "query_engine",
            "score",
            "scores",
            "smoke",
            "eval",
            "context",
        ]
        .iter()
        .any(|t| terms.contains(*t))
    {
        bonus -= 18.0;
    }
    if source.starts_with("oracle/bootstrap/")
        && !["bootstrap", "ingest", "graph"]
            .iter()
            .any(|t| terms.contains(*t))
    {
        bonus -= 12.0;
    }

    if source.contains("/tests/")
        || ends_with_any(source, &[".test.js", ".test.ts", ".spec.js", ".spec.ts"])
    {
        if asks_for_tests {
            bonus += 1.0;
        } else {
            bonus -= 10.0;
        }
    }

    let planning_markers = [
        "/docs/", " plan/", "-plan.", "roadmap", "handoff", "session", "bug log", "bugs.md",
        "proposal",
    ];
    if ends_with_any(source, &[".md", ".txt"])
        || planning_markers.iter().any(|m| source.contains(m))
    {
        if asks_for_plan {
            bonus += 1.0;
        } else {
            bonus -= 8.0;
        }
    }

    let static_public_js = source.contains("/cloudflare/aspis-bio-website/public/")
        && ends_with_any(source, &[".js", ".css", ".html"]);
    if static_public_js
        && asks_for_implementation
        && !["browser", "frontend", "ui", "website"]
            .iter()
            .any(|t| terms.contains(*t))
    {
        bonus -= 4.0;
    }

    let generated_markers = ["/dist/", "/build/", "/coverage/", ".min.js", ".bundle.js"];
    if generated_markers.iter().any(|m| source.contains(m)) {
        bonus -= 8.0;
    }

    bonus
}

/// RNA-seq output release bonus.
fn rnaseq_output_release_bonus(terms: &HashSet<String>, source: &str, text: &str) -> f64 {
    let rnaseq_terms = ["rna-seq", "rnaseq", "rna"];
    let output_terms = [
        "output",
        "outputs",
        "result",
        "results",
        "release",
        "download",
        "browser",
        "successful",
        "success",
    ];
    if !(rnaseq_terms.iter().any(|t| terms.contains(*t))
        && output_terms.iter().any(|t| terms.contains(*t)))
    {
        return 0.0;
    }

    let mut bonus = 0.0_f64;
    let source_weights = [
        (
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/subworker.mjs",
            28.0,
        ),
        (
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/outputs.mjs",
            26.0,
        ),
        (
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/job_views.mjs",
            24.0,
        ),
        (
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/runner_status.mjs",
            22.0,
        ),
        (
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/provider/scaleway.mjs",
            10.0,
        ),
    ];
    for &(suffix, weight) in &source_weights {
        if source.ends_with(suffix) {
            bonus += weight;
            break;
        }
    }

    if source.contains("/aspis-bio-website/public/")
        && (terms.contains("browser") || terms.contains("download"))
    {
        bonus += 5.0;
    }
    if source.contains("/tests/") || source.ends_with(".test.js") {
        bonus -= 12.0;
    }
    if ends_with_any(source, &[".md", ".txt"]) && !source.ends_with("output_catalog.json") {
        bonus -= 10.0;
    }

    let signals = [
        "runner-status",
        "output_renders",
        "artifact_url",
        "manifest_url",
        "results ready",
        "downloadrenderedartifact",
        "renderartifactisregistered",
        "registeredartifactisdownloadable",
        "requestoutputrenderrecordwithpayload",
        "handleoutputrenderstatuscallback",
        "normalizerunnerstatuspayload",
        "sanitizeoutputrenders",
        "sanitizerenderrecord",
        "/artifacts/",
        "outputs/render",
        "rendered_outputs",
        "content-disposition",
    ];
    for signal in &signals {
        if text.contains(signal) {
            bonus += 1.4;
        }
    }
    if text.contains("status === \"done\"") || text.contains("status: \"ready\"") {
        bonus += 2.0;
    }
    bonus
}

/// RNA-seq browser upload session bonus.
fn rnaseq_browser_upload_bonus(terms: &HashSet<String>, source: &str, text: &str) -> f64 {
    let rnaseq_terms = ["rna-seq", "rnaseq", "rna"];
    let upload_terms = [
        "upload",
        "uploads",
        "session",
        "sessions",
        "completion",
        "complete",
        "browser",
    ];
    let upload_specific = [
        "upload",
        "uploads",
        "session",
        "sessions",
        "completion",
        "complete",
    ];
    if !(rnaseq_terms.iter().any(|t| terms.contains(*t))
        && upload_terms.iter().any(|t| terms.contains(*t))
        && upload_specific.iter().any(|t| terms.contains(*t)))
    {
        return 0.0;
    }

    let mut bonus = 0.0_f64;
    let source_weights = [
        (
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/rna_upload_sessions.mjs",
            70.0,
        ),
        (
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/subworker.mjs",
            30.0,
        ),
        (
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/job_views.mjs",
            16.0,
        ),
    ];
    for &(suffix, weight) in &source_weights {
        if source.ends_with(suffix) {
            bonus += weight;
            break;
        }
    }
    let signals = [
        "createbrowseruploadsession",
        "completebrowseruploadfile",
        "completebrowseruploadsession",
        "getbrowseruploadsessionstatus",
        "browseruploadsession",
        "upload_session",
        "browser_upload",
    ];
    for signal in &signals {
        if text.contains(signal) {
            bonus += 2.2;
        }
    }
    if text.contains("output_renders") || text.contains("downloadrenderedartifact") {
        bonus -= 10.0;
    }
    if ends_with_any(source, &[".md", ".txt"]) {
        bonus -= 8.0;
    }
    bonus
}

/// RNA-seq Scaleway lifecycle bonus.
fn rnaseq_scaleway_lifecycle_bonus(terms: &HashSet<String>, source: &str, text: &str) -> f64 {
    let rnaseq_terms = ["rna-seq", "rnaseq", "rna"];
    let lifecycle_terms = [
        "lifecycle",
        "terminal",
        "cleanup",
        "instance",
        "instances",
        "vm",
        "scaleway",
    ];
    if !(rnaseq_terms.iter().any(|t| terms.contains(*t))
        && terms.contains("scaleway")
        && lifecycle_terms.iter().any(|t| terms.contains(*t)))
    {
        return 0.0;
    }

    let mut bonus = 0.0_f64;
    let source_weights = [
        (
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/provider/scaleway.mjs",
            34.0,
        ),
        (
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/subworker.mjs",
            26.0,
        ),
        (
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/instance_tier.mjs",
            20.0,
        ),
    ];
    for &(suffix, weight) in &source_weights {
        if source.ends_with(suffix) {
            bonus += weight;
            break;
        }
    }
    let signals = [
        "cleanupscalewayinstanceafterterminal",
        "terminatescalewayinstance",
        "releasescalewayinstanceslot",
        "deletescaleawayinstance",
        "baredelete",
        "with_volumes=all",
        "instance_tier",
        "commercial_type",
    ];
    for signal in &signals {
        if text.contains(signal) {
            bonus += 2.0;
        }
    }
    if source.starts_with("oracle/evals/") {
        bonus -= 20.0;
    }
    if ends_with_any(source, &[".md", ".txt"]) {
        bonus -= 8.0;
    }
    bonus
}

/// Scaleway paid cleanup bonus.
fn scaleway_paid_cleanup_bonus(terms: &HashSet<String>, source: &str, text: &str) -> f64 {
    let cleanup_terms = [
        "cleanup",
        "stop",
        "stops",
        "terminate",
        "delete",
        "paid",
        "resource",
        "resources",
        "terminal",
        "session",
        "done",
        "job",
        "compute",
    ];
    if !(terms.contains("scaleway") && cleanup_terms.iter().any(|t| terms.contains(*t))) {
        return 0.0;
    }

    let mut bonus = 0.0_f64;
    let source_weights = [
        (
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/provider/scaleway.mjs",
            36.0,
        ),
        (
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/subworker.mjs",
            22.0,
        ),
        (
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/queue/handlers.mjs",
            16.0,
        ),
        ("src-tauri/src/backend/providers.rs", 14.0),
        ("src-tauri/src/backend/commands.rs", 10.0),
    ];
    for &(suffix, weight) in &source_weights {
        if source.ends_with(suffix) {
            bonus += weight;
            break;
        }
    }
    let signals = [
        "cleanupscalewayinstanceafterterminal",
        "terminatescalewayinstance",
        "releasescalewayinstanceslot",
        "delete",
        "with_volumes=all",
        "release",
        "terminate",
        "terminal",
    ];
    for signal in &signals {
        if text.contains(signal) {
            bonus += 2.0;
        }
    }
    if source.starts_with("oracle/") {
        bonus -= 20.0;
    }
    if ends_with_any(source, &[".md", ".txt"]) {
        bonus -= 8.0;
    }
    bonus
}

/// Cloudflare secret rotation bonus.
fn cloudflare_secret_rotation_bonus(terms: &HashSet<String>, source: &str, text: &str) -> f64 {
    if !(terms.contains("cloudflare")
        && ["worker", "workers"].iter().any(|t| terms.contains(*t))
        && ["secret", "secrets", "rotation", "rotate"]
            .iter()
            .any(|t| terms.contains(*t)))
    {
        return 0.0;
    }

    let mut bonus = 0.0_f64;
    let source_weights = [
        ("src-tauri/src/backend/commands.rs", 28.0),
        ("src-tauri/src/backend/providers.rs", 18.0),
        ("src/components/dashboard/WorkersTable.tsx", 16.0),
        ("src/components/views/SecretsView.tsx", 12.0),
    ];
    for &(suffix, weight) in &source_weights {
        if source.ends_with(suffix) {
            bonus += weight;
            break;
        }
    }
    let signals = [
        "rotate_cloudflare_worker_secret",
        "put_cloudflare_worker_secret",
        "validate_cloudflare_secret_rotation_request",
        "secret_rotation_result",
        "workers scripts write",
        "rotateworkersecret",
        "rotate worker secret",
    ];
    for signal in &signals {
        if text.contains(signal) {
            bonus += 1.8;
        }
    }
    if source.starts_with("oracle/") {
        bonus -= 12.0;
    }
    bonus
}

/// Oracle privacy provider bonus.
fn oracle_privacy_provider_bonus(terms: &HashSet<String>, source: &str, text: &str) -> f64 {
    let privacy_terms = ["privacy", "private", "safe", "zdr", "gdpr"];
    let provider_terms = ["provider", "providers", "ai", "llm", "answers", "answer"];
    if !(terms.contains("oracle")
        && privacy_terms.iter().any(|t| terms.contains(*t))
        && provider_terms.iter().any(|t| terms.contains(*t)))
    {
        return 0.0;
    }

    let mut bonus = 0.0_f64;
    let source_weights = [
        ("src/components/views/OracleView.tsx", 28.0),
        ("src-tauri/src/backend/vault.rs", 26.0),
        ("oracle/server/answerer.py", 24.0),
        ("src-tauri/src/graph/commands.rs", 18.0),
        ("src-tauri/src/backend/model.rs", 12.0),
        ("src/types/backend.ts", 8.0),
    ];
    for &(suffix, weight) in &source_weights {
        if source.ends_with(suffix) {
            bonus += weight;
            break;
        }
    }
    let signals = [
        "zdr",
        "gdpr",
        "scaleway",
        "infomaniak",
        "mistral",
        "allowed",
        "privacy",
        "oracle_llm",
        "llm_provider",
    ];
    for signal in &signals {
        if text.contains(signal) {
            bonus += 1.4;
        }
    }
    if source.ends_with("src-tauri/src/backend/providers.rs") {
        bonus -= 8.0;
    }
    if source.ends_with("oracle/server/aspis_mcp.py") {
        bonus -= 10.0;
    }
    bonus
}

/// Agent project workflow bonus.
fn agent_project_workflow_bonus(terms: &HashSet<String>, source: &str, text: &str) -> f64 {
    let agent_terms = [
        "agent",
        "agents",
        "terminal",
        "cli",
        "orchestrator",
        "coder",
        "verifier",
    ];
    let task_terms = [
        "project", "task", "tasks", "status", "finished", "done", "mark", "current",
    ];
    if !(agent_terms.iter().any(|t| terms.contains(*t))
        && task_terms.iter().any(|t| terms.contains(*t)))
    {
        return 0.0;
    }

    let mut bonus = 0.0_f64;
    let source_weights = [
        ("oracle/server/aspis_mcp.py", 34.0),
        ("src/components/views/ProjectsView.tsx", 24.0),
        ("src-tauri/src/backend/agents.rs", 18.0),
        ("src/components/views/AgentsView.tsx", 12.0),
        ("docs/aspis-mcp.md", 8.0),
    ];
    for &(suffix, weight) in &source_weights {
        if source.ends_with(suffix) {
            bonus += weight;
            break;
        }
    }
    let signals = [
        "project_claim_task",
        "project_update_status",
        "project_read",
        "oracle_ask",
        "oracle_context",
        "agent",
        "claim",
        "status",
    ];
    for signal in &signals {
        if text.contains(signal) {
            bonus += 1.8;
        }
    }
    if source.starts_with("aspis-lab/cloudflare/") && !text.contains("project_") {
        bonus -= 8.0;
    }
    bonus
}

/// Windows Hello unlock bonus.
fn windows_hello_unlock_bonus(terms: &HashSet<String>, source: &str, text: &str) -> f64 {
    if !(terms.contains("windows")
        && ["hello", "webcam", "camera", "unlock", "pin", "biometric"]
            .iter()
            .any(|t| terms.contains(*t)))
    {
        return 0.0;
    }

    let mut bonus = 0.0_f64;
    let source_weights = [
        ("src-tauri/src/backend/auth.rs", 30.0),
        ("src/components/auth/LockedScreen.tsx", 24.0),
        ("src-tauri/src/backend/state.rs", 12.0),
        ("src/context/AppContext.tsx", 8.0),
    ];
    for &(suffix, weight) in &source_weights {
        if source.ends_with(suffix) {
            bonus += weight;
            break;
        }
    }
    let signals = [
        "windows hello",
        "unlock",
        "biometric",
        "webcam",
        "camera",
        "pin",
        "auth",
        "credential",
    ];
    for signal in &signals {
        if text.contains(signal) {
            bonus += 1.4;
        }
    }
    if source.starts_with("oracle/") || source.starts_with("src-tauri/src/graph/") {
        bonus -= 10.0;
    }
    bonus
}

/// Implementation file bonus.
fn implementation_file_bonus(terms: &HashSet<String>, source: &str, text: &str) -> f64 {
    if !["file", "files", "where", "which", "control", "controls"]
        .iter()
        .any(|t| terms.contains(*t))
    {
        return 0.0;
    }

    let mut bonus = 0.0_f64;

    if (terms.contains("scaleway") && terms.contains("gpu"))
        && ["cpu", "vm", "lifecycle", "actions", "instance", "instances"]
            .iter()
            .any(|t| terms.contains(*t))
    {
        if source.ends_with("aspis-lab/cloudflare/orasis/src/gpu_lifecycle.ts") {
            bonus += 7.0;
        }
        if source.ends_with("aspis-lab/cloudflare/orasis/src/runner.ts") {
            bonus += 6.0;
        }
        if source.ends_with("aspis-lab/cloudflare/orasis/src/routes/jobs.ts") {
            bonus += 5.0;
        }
        if source.ends_with("aspis-lab/cloudflare/orasis/src/routes/segment.ts") {
            bonus += 4.0;
        }
        if text.contains("durable object") && text.contains("scaleway") {
            bonus += 2.0;
        }
        if text.contains("cpurunner") || text.contains("gpurunner") {
            bonus += 2.0;
        }
    }

    if (terms.contains("rnaseq") && terms.contains("scaleway"))
        && ["vm", "lifecycle", "instance", "instances"]
            .iter()
            .any(|t| terms.contains(*t))
    {
        if source.ends_with("aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/provider/scaleway.mjs") {
            bonus += 7.0;
        }
        if source.ends_with("aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/instance_tier.mjs") {
            bonus += 5.0;
        }
    }

    bonus
}

// ===================================================================
// Core scoring function
// ===================================================================

/// Compute the lexical score for a single chunk against a query.
///
/// Accumulation order matches Python exactly: base term scoring,
/// then semantic expansions, then domain bonuses in fixed order,
/// then conditional source_quality_bonus, then domain_bonus, then clamp.
pub fn lexical_chunk_score(query: &str, terms: &HashSet<String>, chunk: &ScoredChunk) -> f64 {
    let source = chunk.file_sorgente.to_lowercase();
    let text = chunk.text.to_lowercase();

    // Base term scoring: +1.0 per term in text, +0.35 per term in source
    let mut score = 0.0_f64;
    for term in terms {
        if text.contains(term.as_str()) {
            score += 1.0;
        }
        if source.contains(term.as_str()) {
            score += 0.35;
        }
    }

    // Semantic expansion scoring: +0.55 per matched expansion in text
    for synonym in semantic_expansions(terms) {
        if text.contains(synonym.as_str()) {
            score += 0.55;
        }
    }

    // Domain bonuses — accumulated in the EXACT order from Python
    let domain_bonus = domain_mechanism_bonus(query, terms, &source, &text)
        + rnaseq_output_release_bonus(terms, &source, &text)
        + rnaseq_browser_upload_bonus(terms, &source, &text)
        + rnaseq_scaleway_lifecycle_bonus(terms, &source, &text)
        + scaleway_paid_cleanup_bonus(terms, &source, &text)
        + cloudflare_secret_rotation_bonus(terms, &source, &text)
        + oracle_privacy_provider_bonus(terms, &source, &text)
        + agent_project_workflow_bonus(terms, &source, &text)
        + windows_hello_unlock_bonus(terms, &source, &text)
        + implementation_file_bonus(terms, &source, &text);

    // source_quality_bonus gated: only added when score + domain_bonus > 0.0
    if score + domain_bonus > 0.0 {
        score += source_quality_bonus(query, terms, &source);
    }
    score += domain_bonus;

    score.max(0.0)
}

// ===================================================================
// chunk_context_payload — mirrors Python's chunk_context_payload()
// ===================================================================

/// Build a context payload from a chunk, score, and retrieval tag.
/// Mirrors the Python `chunk_context_payload()` function.
pub fn chunk_context_payload(
    chunk: &ScoredChunk,
    score: f64,
    retrieval: &str,
) -> ChunkContextPayload {
    ChunkContextPayload {
        chunk_id: chunk.id.clone(),
        file_source: chunk.file_sorgente.clone(),
        chunk_index: chunk.chunk_index,
        start_char: chunk.start_char,
        end_char: chunk.end_char,
        score,
        retrieval: retrieval.to_string(),
        text: chunk.text.clone(),
        last_modified: String::new(), // not available from chunk dict in golden fixture
        kind: chunk.kind.clone(),
        symbol_name: chunk.symbol_name.clone(),
        signature: chunk.signature.clone(),
        language: chunk.language.clone(),
        line_start: chunk.line_start,
        line_end: chunk.line_end,
        symbols_used: chunk.symbols_used.clone(),
    }
}

// ===================================================================
// lexical_chunk_context — standalone ranking over a plain chunk list
// ===================================================================

/// Rank chunks by lexical relevance to the query.
///
/// Doc note: ported from the Python `lexical_chunk_context`.  The Python
/// version receives chunks from a SQLite store; this Rust version takes a
/// plain slice of [`ScoredChunk`], mirroring the pure ranking core.
pub fn lexical_chunk_context(
    query: &str,
    chunks: &[ScoredChunk],
    limit: usize,
) -> Vec<ChunkContextPayload> {
    let terms = query_terms(query);
    if terms.is_empty() {
        return vec![];
    }
    let mut rows: Vec<ChunkContextPayload> = Vec::new();
    for chunk in chunks {
        let score = lexical_chunk_score(query, &terms, chunk);
        if score > 0.0 {
            rows.push(chunk_context_payload(chunk, score, "lexical"));
        }
    }
    rows.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.file_source.cmp(&b.file_source))
            .then_with(|| a.chunk_index.cmp(&b.chunk_index))
    });
    let n = limit.max(1);
    rows.truncate(n);
    rows
}

// ===================================================================
// Tests
// ===================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_terms_basic() {
        let terms = query_terms("How do agents claim tasks and update project status?");
        let expected: HashSet<String> = ["agents", "claim", "project", "status", "tasks", "update"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(terms, expected);
    }

    #[test]
    fn test_query_terms_stopwords() {
        let terms = query_terms("What is the architecture of this project?");
        assert!(terms.contains("architecture"));
        assert!(terms.contains("project"));
        assert!(!terms.contains("what"));
        assert!(!terms.contains("the"));
        assert!(!terms.contains("is"));
    }

    #[test]
    fn test_semantic_expansions_agent() {
        let terms: HashSet<String> = ["agents", "claim", "project", "status", "tasks", "update"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let exp = semantic_expansions(&terms);
        assert!(exp.contains("project_claim_task"));
        assert!(exp.contains("project_update_status"));
        assert!(exp.contains("oracle_ask"));
        assert!(exp.contains("oracle_context"));
        assert!(exp.contains("read_project"));
    }
}
