//! LIVE e2e harness for the voted Censor path. Runs the PRODUCTION pipeline —
//! [`super::gemma::run_gemma`] → `cluster_and_vote` → `split_by_threshold` — against the
//! local oMLX server on REAL files from this repo, with the exact client, timeouts,
//! prompts and parsing the app uses. Ignored by default: it needs oMLX serving the
//! fine-tuned Censor model. Run manually:
//!
//! ```sh
//! cargo test --lib censor_live_e2e -- --ignored --nocapture
//! ```
//!
//! Override the model with `ASPIS_LIVE_CENSOR_MODEL`.

use std::path::Path;
use std::time::Instant;

use super::gemma::{
    probe_available, run_gemma, CensorAiProvider, CensorLocalAi, OmlxClient, OMLX_DEFAULT_BASE,
};
use super::runners::RawFinding;
use super::schema::{Category, Severity};

const DEFAULT_MODEL: &str = "Censor-Qwen25-14B-6040e2-4bit";

/// Project-convention suppressions, injected through the ALREADY-KNOWN section of the
/// prompt (the in-distribution channel the model was TRAINED to not re-report). Each
/// entry is phrased like an already-reported finding title. Targets the top noise
/// categories measured in the 2026-07-03 live run on clean files. Disable with
/// `ASPIS_LIVE_CENSOR_SUPPRESS=0` for A/B comparison.
const SUPPRESSIONS: &[&str] = &[
    "Ignored Result in `let _ = fs::remove_file/fs::copy/...` cleanup calls — intentional best-effort cleanup in this codebase",
    "Division-by-zero risk in divisions by numeric constants (e.g. `/ 4`, `* 70 / 100`)",
    "Unnecessary cloning of paths, maps or collections",
    "`unwrap()`/`expect()` usage inside `#[cfg(test)]` test code — accepted style here",
    "Magic numbers that should be named constants",
];

/// Real, logic-heavy files across subsystems; all under `MAX_FILE_CHARS` so nothing is
/// truncated. Paths are repo-relative (what the watcher hands `run_gemma` in production).
const FILES: &[&str] = &[
    "src-tauri/src/backend/fs_replace.rs",
    "src-tauri/src/backend/task_size.rs",
    "src-tauri/src/backend/agent_role.rs",
    "src-tauri/src/polis/footprint.rs",
    "src-tauri/src/backend/cloud_claude_config.rs",
    "src-tauri/src/backend/saved_workflows.rs",
    "src-tauri/src/backend/main_coder.rs",
    "src-tauri/src/backend/fs_watch.rs",
    "src-tauri/src/backend/agentic_loop.rs",
    "src-tauri/src/backend/censor/runners/gitleaks.rs",
];

#[test]
#[ignore = "needs a running oMLX server with the Censor model loaded"]
fn censor_live_e2e() {
    let model =
        std::env::var("ASPIS_LIVE_CENSOR_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());

    // The production config shape for the voted tier (what a user sets in Projects →
    // censorLocalAi), resolved through the SAME `review_params()` the app calls.
    let cfg = CensorLocalAi {
        provider: CensorAiProvider::Omlx,
        base_url: None,
        model: Some(model.clone()),
        ollama_model: None,
        n_samples: Some(5),
        min_votes_block: Some(2),
        min_votes_verify: Some(1),
        temperature: Some(0.7),
        prompt_style: Some("censor_v2".to_string()),
    };
    let params = cfg.review_params();
    let client = OmlxClient::new(OMLX_DEFAULT_BASE, &model);
    assert!(
        probe_available(&client),
        "oMLX not reachable at {OMLX_DEFAULT_BASE} or model {model} not served"
    );

    // Optional overrides: point the harness at a different tree (e.g. copies with planted
    // bugs) and/or a custom comma-separated file list, without touching the default set.
    let root = std::env::var("ASPIS_LIVE_CENSOR_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
        });
    let files: Vec<String> = std::env::var("ASPIS_LIVE_CENSOR_FILES")
        .map(|s| s.split(',').map(|f| f.trim().to_string()).collect())
        .unwrap_or_else(|_| FILES.iter().map(|s| s.to_string()).collect());
    let suppress = std::env::var("ASPIS_LIVE_CENSOR_SUPPRESS").as_deref() != Ok("0");
    println!("== censor_live_e2e: model={model} suppressions={suppress} params={params:?}");

    for rel in &files {
        let content = match std::fs::read_to_string(root.join(rel)) {
            Ok(c) => c,
            Err(e) => {
                println!("## {rel}: SKIP (read failed: {e})");
                continue;
            }
        };
        // Convention suppressions ride the deterministic-findings slot: build_user_body
        // renders them under ALREADY-KNOWN exactly like linter findings, which is the
        // contract the model was trained to not re-report.
        let deterministic: Vec<RawFinding> = if suppress {
            SUPPRESSIONS
                .iter()
                .map(|title| RawFinding {
                    file: rel.to_string(),
                    line: None,
                    severity: Severity::Low,
                    category: Category::Style,
                    source: "convention".to_string(),
                    title: (*title).to_string(),
                    body: String::new(),
                })
                .collect()
        } else {
            Vec::new()
        };
        let t0 = Instant::now();
        let findings = run_gemma(&client, true, &root, rel, &content, &deterministic, &params);
        let secs = t0.elapsed().as_secs_f32();
        println!(
            "## {rel} ({} chars) -> {} findings in {secs:.1}s",
            content.chars().count(),
            findings.len()
        );
        for f in &findings {
            println!(
                "  - line {:?} [{:?}/{:?}] {} :: {}",
                f.line, f.severity, f.category, f.title, f.body
            );
        }
    }
}
