//! Pigeon message contract for the async Censor LLM review routed through Pigeon's `censor-pool`
//! receiver. Mirrors how mini-dispatch uses `mini-pool`; the worker that drains/processes it lands
//! separately. The mailbox payload is opaque JSON, so this is purely the (de)serialized shape.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Pigeon receiver-id (queue) for async Censor LLM reviews. Mirrors `PIGEON_MINI_POOL_RECEIVER`.
/// Must match the Python `PIGEON_CENSOR_POOL_RECEIVER` in `aspis_mcp.py`.
pub const PIGEON_CENSOR_POOL_RECEIVER: &str = "censor-pool";

/// What a finished mini sends to be reviewed asynchronously by the Censor LLM.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CensorReviewRequest {
    pub project_id: String,
    /// Optional hint of the project working dir. The worker RESOLVES the canonical root from
    /// `project_id` (the single source of truth via `resolve_project_root_by_id`), so a producer
    /// that only knows the project id may omit this.
    #[serde(default)]
    pub root: String,
    /// Project-relative path to review.
    pub file: String,
    /// Deterministic findings already open on this file, so the LLM does not repeat them.
    pub known_findings: Vec<CensorKnownFinding>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CensorKnownFinding {
    pub line: Option<u32>,
    pub title: String,
}

/// What the worker sends back after the LLM review.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CensorReviewResult {
    pub project_id: String,
    pub file: String,
    pub findings: Vec<CensorReviewFinding>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CensorReviewFinding {
    pub line: Option<u32>,
    pub title: String,
    pub body: String,
    /// "high" | "medium" | "low"
    pub severity: String,
}

/// Max tickets drained from the queue in a single executor pass.
const CENSOR_REVIEW_MAX_PER_PASS: usize = 8;
/// Hard cap on review threads running AT ONCE across all passes. Phase 3 makes each review a slow
/// LLM call; without this back-pressure, 8 threads/pass every 1.5s would pile up unbounded (each
/// holds a stack) and exhaust memory. We stop draining when the cap is reached, leaving tasks
/// PENDING in the durable mailbox (not claimed) for a later pass.
const CENSOR_REVIEW_MAX_INFLIGHT: usize = 4;
/// Count of review threads currently in flight (decremented by `InflightGuard` on drop, incl. panic).
static CENSOR_REVIEW_INFLIGHT: AtomicUsize = AtomicUsize::new(0);

/// RAII: decrements the in-flight counter when the review thread ends — including on panic unwind,
/// so a panicking review can never permanently leak a slot.
struct InflightGuard;
impl Drop for InflightGuard {
    fn drop(&mut self) {
        CENSOR_REVIEW_INFLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Build the Censor LLM client for `cfg`, reading the cloud API key from the vault ONLY when
/// the provider is `Cloud` (the opt-in remote-egress path). For every LOCAL provider the key
/// is `None`, so the loopback privacy clamp inside `OmlxClient` is untouched.
///
/// SECURITY: a `Cloud` provider with NO key (never saved / vault read error / deleted
/// mid-flight) returns `Err` instead of a client — NOT a key-less client. Without this, the
/// key-less Cloud client would clamp its remote https base to the loopback default and could
/// POST file content to a local `:8000` server (wrong endpoint). Returning `Err` makes a
/// keyless Cloud a clean no-op at BOTH call sites (ingest skips the drain; process logs +
/// returns), preserving the once-per-pass probe design (no per-file re-probe needed).
fn build_censor_client(
    cfg: &crate::backend::censor::gemma::CensorLocalAi,
) -> Result<Box<dyn crate::backend::censor::gemma::GemmaClient>, String> {
    let api_key = if cfg.provider == crate::backend::censor::gemma::CensorAiProvider::Cloud {
        let key = crate::backend::vault::read_censor_cloud_key().ok().flatten();
        if key.is_none() {
            return Err("Cloud Censor provider has no API key configured.".into());
        }
        key
    } else {
        None
    };
    crate::backend::censor::gemma::build_gemma_client_with_key(cfg, api_key.as_deref())
}

/// Run the Censor LLM review for one request: build the configured Censor model client
/// (Ollama / oMLX / AppleFM / cloud), and if it is available reuse the FINE pipeline's
/// `run_fine_batch_no_rail` with a `GemmaCtx` — which runs the LLM review on the file, writes the
/// shard, and emits `censor://findings-updated` so the strip/panel reflect the AI findings like any
/// other. AI review is opt-in: when no model is configured/loaded, `probe_available` is false and we
/// no-op. The returned `CensorReviewResult` is just the Pigeon receipt (the findings manifest via the
/// shard/event, not the mailbox). `request.known_findings` is intentionally unused here — the
/// pipeline recomputes the deterministic findings and feeds them to the LLM as "already known".
pub fn process_censor_review(
    app: &tauri::AppHandle,
    request: &CensorReviewRequest,
) -> CensorReviewResult {
    let receipt = CensorReviewResult {
        project_id: request.project_id.clone(),
        file: request.file.clone(),
        findings: Vec::new(),
    };
    // BLOCKER B (anti-RCE): NEVER run the repo's own tool-configs (eslintrc, build.rs, …) for an
    // UNTRUSTED project, even via this async path. The sync verdict gate enforces this for the mini;
    // mirror it here because the Pigeon producer is best-effort and does not pre-check trust.
    if !crate::backend::projects::project_censor_trusted(app, &request.project_id).unwrap_or(false) {
        return receipt;
    }
    // Resolve the canonical working root from the project id (single source of truth — the
    // producer need not know it). An unknown/unrooted project is a clean no-op.
    let root = match crate::backend::projects::resolve_project_root_by_id(app, &request.project_id) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "censor-review: cannot resolve root for project {} ({e})",
                request.project_id
            );
            return receipt;
        }
    };
    let cfg = crate::backend::projects::read_censor_local_ai(app);
    let client = match build_censor_client(&cfg) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("censor-review: no Censor LLM client ({e})");
            return receipt;
        }
    };
    // The ingest pass already gated on probe_available (probed ONCE per pass, not per file —
    // re-probing here would hit the backend O(N) and could falsely skip a file while the model is
    // busy generating for another). run_gemma itself returns empty on any generate error, so a
    // backend that went down between the pass-probe and now degrades gracefully to no findings.
    let available = true;
    // One-shot review on a detached daemon thread: the flag is permanently live — these threads die
    // with the process and there is no (nor any need for) a per-review cancellation path.
    let running = std::sync::atomic::AtomicBool::new(true);
    let ctx = crate::backend::censor::orchestrator::GemmaCtx {
        client: client.as_ref(),
        available,
    };
    // NOTE: run_fine_batch_no_rail also re-runs the deterministic FINE runners (not only the LLM).
    // Redundant with the watcher but idempotent — the per-shard lock + source-scoped merge serialize
    // writers and never cross sources. Accepted for the high-level reuse; an LLM-only leg could be
    // factored out later if the double-run proves costly.
    crate::backend::censor::orchestrator::run_fine_batch_no_rail(
        app,
        &request.project_id,
        &root,
        std::slice::from_ref(&request.file),
        Some(ctx),
        &running,
    );
    receipt
}

/// Drain the `censor-pool` queue (only when Pigeon is enabled): for each review request, run the
/// (slow, in Phase 3) review on a DETACHED thread so the executor pass is never blocked, then post
/// the result back via `/done`. Mirrors `ingest_pigeon_directives`; every error is non-fatal and the
/// durable mailbox keeps an un-drained/failed task for the next tick.
///
/// Takes `app` BY VALUE (owned) so Phase 3 can `app.clone()` it into the review thread for LLM
/// config + the resource budget gate; today it is intentionally unused.
pub fn ingest_pigeon_censor_reviews(app: tauri::AppHandle) {
    let Some(client) = crate::backend::pigeon_service::pigeon_client_from_running() else {
        return;
    };
    // Probe the Censor model ONCE per pass (not per file). If no model is configured/loaded, AI
    // review is a clean no-op: we DON'T drain, leaving the tasks PENDING in the durable mailbox for
    // a later pass — never claiming work we cannot run, and never O(N)-probing the backend.
    let cfg = crate::backend::projects::read_censor_local_ai(&app);
    let llm_available = match build_censor_client(&cfg) {
        Ok(c) => crate::backend::censor::gemma::probe_available(c.as_ref()),
        Err(_) => false,
    };
    if !llm_available {
        return;
    }
    for _ in 0..CENSOR_REVIEW_MAX_PER_PASS {
        // Back-pressure: never claim more than we can run concurrently. Tasks stay PENDING in the
        // durable mailbox until a later pass has a free slot.
        if CENSOR_REVIEW_INFLIGHT.load(Ordering::SeqCst) >= CENSOR_REVIEW_MAX_INFLIGHT {
            break;
        }
        match client.poll(PIGEON_CENSOR_POOL_RECEIVER) {
            Ok(Some((ticket, payload))) => match serde_json::from_value::<CensorReviewRequest>(payload)
            {
                Ok(req) => {
                    CENSOR_REVIEW_INFLIGHT.fetch_add(1, Ordering::SeqCst);
                    let app_for_thread = app.clone();
                    // Builder::spawn returns Err (instead of panicking) if the OS refuses a thread,
                    // so a spawn failure can't leak the reserved in-flight slot. ticket is Copy, so it
                    // stays usable below for the failure path.
                    let spawned = std::thread::Builder::new().spawn(move || {
                        // Decrements the in-flight slot on ANY exit (success, error, or panic).
                        let _guard = InflightGuard;
                        let reviewed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            process_censor_review(&app_for_thread, &req)
                        }));
                        let Some(client) =
                            crate::backend::pigeon_service::pigeon_client_from_running()
                        else {
                            // Dispatcher gone: leave the ticket claimed; the reclaim sweep requeues it.
                            return;
                        };
                        match reviewed {
                            Ok(result) => match serde_json::to_value(&result) {
                                Ok(value) => {
                                    if let Err(e) =
                                        client.done(ticket, PIGEON_CENSOR_POOL_RECEIVER, value)
                                    {
                                        eprintln!(
                                            "censor-review egress: done failed (ticket {ticket}): {e}"
                                        );
                                    }
                                }
                                // Serialize failure: do NOT close the ticket — leave it claimed so
                                // the sweep requeues it (mirrors the mini-pool egress contract).
                                Err(e) => eprintln!(
                                    "censor-review egress: serialize failed (ticket {ticket}): {e}"
                                ),
                            },
                            Err(_) => {
                                eprintln!("censor-review: review panicked (ticket {ticket})");
                                let _ = client.fail(
                                    ticket,
                                    PIGEON_CENSOR_POOL_RECEIVER,
                                    "censor review panicked",
                                );
                            }
                        }
                    });
                    if spawned.is_err() {
                        // OS refused the thread: undo the reserved slot and let the sweep requeue.
                        CENSOR_REVIEW_INFLIGHT.fetch_sub(1, Ordering::SeqCst);
                        let _ = client.fail(
                            ticket,
                            PIGEON_CENSOR_POOL_RECEIVER,
                            "censor review thread spawn failed",
                        );
                    }
                }
                Err(e) => {
                    eprintln!("censor-review ingest: undecodable request (ticket {ticket}): {e}");
                    let _ = client.fail(
                        ticket,
                        PIGEON_CENSOR_POOL_RECEIVER,
                        "undecodable censor-review request",
                    );
                }
            },
            Ok(None) => break,
            Err(e) => {
                eprintln!("censor-review ingest: poll error: {e}");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_camel_case() {
        let req = CensorReviewRequest {
            project_id: "test-project".into(),
            root: "/tmp/test".into(),
            file: "src/main.rs".into(),
            known_findings: vec![CensorKnownFinding {
                line: Some(42),
                title: "Old Finding".into(),
            }],
        };

        let json = serde_json::to_string(&req).expect("serialization failed");
        assert!(json.contains("\"projectId\""));
        assert!(json.contains("\"knownFindings\""));

        let deserialized: CensorReviewRequest =
            serde_json::from_str(&json).expect("deserialization failed");
        assert_eq!(req, deserialized);
    }

    #[test]
    fn result_round_trips_camel_case() {
        let res = CensorReviewResult {
            project_id: "test-project".into(),
            file: "src/main.rs".into(),
            findings: vec![CensorReviewFinding {
                line: Some(10),
                title: "New Smell".into(),
                body: "This is a smell".into(),
                severity: "high".into(),
            }],
        };

        let json = serde_json::to_string(&res).expect("serialization failed");
        assert!(json.contains("\"findings\""));
        assert!(json.contains("\"severity\""));

        let deserialized: CensorReviewResult =
            serde_json::from_str(&json).expect("deserialization failed");
        assert_eq!(res, deserialized);
    }
}
