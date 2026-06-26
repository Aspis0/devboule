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
    /// The project working dir; the worker reads the file from disk at review time.
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

/// Run the Censor LLM review for one request. STUB — Phase 3 replaces this body with the real
/// local/cloud Censor LLM call gated by the resource budget. For now it returns no findings.
pub fn process_censor_review(request: &CensorReviewRequest) -> CensorReviewResult {
    CensorReviewResult {
        project_id: request.project_id.clone(),
        file: request.file.clone(),
        findings: Vec::new(),
    }
}

/// Drain the `censor-pool` queue (only when Pigeon is enabled): for each review request, run the
/// (slow, in Phase 3) review on a DETACHED thread so the executor pass is never blocked, then post
/// the result back via `/done`. Mirrors `ingest_pigeon_directives`; every error is non-fatal and the
/// durable mailbox keeps an un-drained/failed task for the next tick.
///
/// Takes `app` BY VALUE (owned) so Phase 3 can `app.clone()` it into the review thread for LLM
/// config + the resource budget gate; today it is intentionally unused.
pub fn ingest_pigeon_censor_reviews(app: tauri::AppHandle) {
    let _ = &app; // Phase 3 clones `app` into the review thread (config + budget); inert for now.
    let Some(client) = crate::backend::pigeon_service::pigeon_client_from_running() else {
        return;
    };
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
                    std::thread::spawn(move || {
                        // Decrements the in-flight slot on ANY exit (success, error, or panic).
                        let _guard = InflightGuard;
                        let reviewed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            process_censor_review(&req)
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

    #[test]
    fn process_censor_review_stub_returns_empty() {
        let request = CensorReviewRequest {
            project_id: "test-project".into(),
            root: "/tmp/test".into(),
            file: "src/main.rs".into(),
            known_findings: Vec::new(),
        };
        let result = process_censor_review(&request);
        assert!(result.findings.is_empty());
        assert_eq!(result.file, request.file);
    }
}
