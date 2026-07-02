//! AGENTIC mini worker — the detached multi-turn tool-loop path of the mini-coder
//! executor (peer of the one-shot emit-edits PTY). Extracted VERBATIM from
//! `mini_coder_executor.rs` (role-untangle Phase 2, pure move): `should_run_agentic`
//! decides the path per the S2 capability policy; `spawn_agentic_worker` runs the
//! sandboxed loop on a worker thread and writes the result JSON the shared
//! finalize→Censor→retry path picks up. The executor's state/scheduler stays in
//! `mini_coder_executor.rs`; this module only owns the agentic dispatch decision +
//! worker thread. Phase 3 promotes this engine to the first-class Main coder.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager};

use super::mini_coder::{self, MiniCoderBackend, MiniCoderDirective};
use super::mini_coder_executor::{
    compose_agentic_system_prompt, mini_agent_id, mini_language_block, mini_model_sampling,
    mini_model_tier, MiniCoderState,
};

/// RAII guard releasing an AGENTIC worker's in-flight id on EVERY exit path (done/abort/
/// panic). Mirrors the executor's `VerdictInflightGuard`: a leaked id would keep the
/// directive forever "live" (never finalized) and forever excluded from the timeout sweep.
struct AgenticInflightGuard {
    set: Arc<Mutex<std::collections::HashSet<String>>>,
    inflight_key: String, // directive.id — what run_pass + plan_tick key on
    cancel_map: Arc<Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>>,
    cancel_key: String, // agent_id — what mini_coder_kill keys on
}

impl Drop for AgenticInflightGuard {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.inflight_key);
        self.cancel_map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.cancel_key);
    }
}

pub(crate) fn should_run_agentic(
    app: &AppHandle,
    backend: &MiniCoderBackend,
    directive: &MiniCoderDirective,
) -> bool {
    if !directive.write
        || directive.write_mode != mini_coder::WriteMode::AgenticIterative
        || backend.base_url.as_deref().is_none_or(|u| u.is_empty())
    {
        return false;
    }

    // NOTE: the local-backend non-loopback base_url gate lives in the CALLER, BEFORE the
    // agentic/one-shot branch, so it blocks BOTH paths (declining only agentic here would just
    // fall through to the one-shot, which makes the same remote request). (review F3/F4)

    // S2 — the toggle STAYS (always give the user the choice); capability drives only the
    // AUTO default, it never removes a choice:
    //   Safe           => never agentic (force emit-edits).
    //   Auto (default) => capability-driven, OPT-IN: agentic ONLY for a model the registry
    //                     confirms is agentic-tier. An unregistered/unknown model (tier None)
    //                     stays one-shot — the safe default (never assume an unknown model can
    //                     drive the multi-turn loop).
    //   AgenticAllowed => the user's explicit override wins — agentic even for a small model.
    match super::projects::read_mini_write_behavior(app) {
        mini_coder::MiniWriteBehavior::Safe => false,
        mini_coder::MiniWriteBehavior::Auto => {
            mini_model_tier(app, backend).as_deref() == Some("agentic")
        }
        mini_coder::MiniWriteBehavior::AgenticAllowed => true,
    }
}

/// Launch the AGENTIC tool-loop coder on a detached worker thread — the peer of
/// `spawn_one_shot_mini`, but a multi-turn HTTP loop (read/edit/grep via sandboxed tools)
/// instead of a one-shot PTY. The worker runs the loop, then writes the MiniCoderResult JSON
/// to `scratch_root/result_rel` so the SAME finalize→Censor→retry path picks it up. The
/// in-flight guard keeps the directive "live" until then (it has no PTY for run_pass to see).
/// Returns immediately; holds no lock.
pub(crate) fn spawn_agentic_worker(
    app: &AppHandle,
    project_root: &Path,
    scratch_root: &Path,
    result_rel: &str,
    backend: &MiniCoderBackend,
    directive: &MiniCoderDirective,
    net: crate::backend::sandbox::NetPolicy,
    // Broker Slice 2: effective working set (persisted + transient, already resolved by
    // `claim_and_launch`).  Passed through to `run_agentic_coder` → `ScopedAgentTools`.
    working_set: Vec<std::path::PathBuf>,
) -> Result<(), String> {
    let state = app
        .try_state::<MiniCoderState>()
        .ok_or_else(|| "executor state unavailable".to_string())?;
    // Key on directive.id — the SAME key run_pass's completion check + plan_tick's timeout
    // exclusion use (the agent_id is a different string and would never match).
    let id = directive.id.clone();
    // agent_id (= mini_agent_id) is the key mini_coder_kill cancels on (it has the live agent
    // id, not the directive id); directive.id is the key run_pass + plan_tick use.
    let agent_id = mini_agent_id(directive);
    // Claim BEFORE spawning so the next run_pass sees the directive as live (it has no PTY).
    if !state.claim_agentic(&id) {
        return Err("agentic worker already in flight".to_string());
    }
    let cancel = state.register_agentic_cancel(&agent_id);
    let guard_set = state.agentic_inflight_handle();
    let guard_cancel = state.agentic_cancel_handle();

    // should_run_agentic guarantees base_url Some + non-empty; defaults are defensive (an
    // empty model just makes oMLX error → the loop aborts → escalates, never a false done).
    let base_url = backend.base_url.clone().unwrap_or_default();
    let model = backend.model.clone().unwrap_or_default();
    let task = directive.task.clone();
    let root = project_root.to_path_buf();
    // WRITES confined to the directive's file allowlist (reads stay project-wide for context).
    let allowlist = directive.files.clone();
    let max_rounds = super::projects::read_agentic_max_rounds(app);
    let result_path = scratch_root.join(result_rel);
    // Phase 7: honor the registry's per-model sampling for this mini (was hardcoded tuned()).
    let sampling = mini_model_sampling(app, backend);
    // LANGUAGE LAYER (agentic path): the (mini × language) persona, appended to the agentic
    // system prompt. Computed HERE (borrowing root + allowlist) before they move into the
    // worker thread; the rendered block is then moved into the closure.
    let agentic_lang_block = mini_language_block(&root, &allowlist);

    let spawned = std::thread::Builder::new()
        .name("agentic-coder-worker".into())
        .spawn(move || {
            // RAII: release the in-flight id + cancel flag on EVERY exit (done/abort/panic).
            // Created FIRST so it drops LAST — AFTER the result file is written below.
            let _guard = AgenticInflightGuard {
                set: guard_set,
                inflight_key: id,
                cancel_map: guard_cancel,
                cancel_key: agent_id,
            };
            let system_prompt = compose_agentic_system_prompt(agentic_lang_block.as_deref());
            let json = match crate::backend::agentic_runner::run_agentic_coder(
                base_url,
                model,
                sampling,
                true, // thinking on — the MoE coders write best with reasoning
                &system_prompt,
                &task,
                root,
                allowlist,
                net,
                working_set,
                max_rounds,
                &cancel,
            ) {
                Ok((outcome, touched, net_blocked, out_of_scope_write)) => {
                    crate::backend::agentic_runner::agentic_result_json(
                        &outcome,
                        &touched,
                        net_blocked,
                        out_of_scope_write.as_deref(),
                    )
                }
                // Transport/init failure → escalate (NOT a false "done"); net_blocked=false,
                // out_of_scope_write=None (the LLM never got to run a tool).
                Err(e) => crate::backend::agentic_runner::agentic_result_json(
                    &crate::backend::agentic_loop::LoopOutcome::Aborted {
                        reason: e,
                        rounds: 0,
                    },
                    &[],
                    false,
                    None,
                ),
            };
            // Write the result the finalize path reads, BEFORE the guard releases the in-flight
            // id (so run_pass never finalizes against a missing file). A write failure leaves no
            // result → read_result_outcome synthesizes a failed outcome on the next pass.
            if let Err(e) = std::fs::write(&result_path, &json) {
                eprintln!("agentic worker: failed to write result {result_path:?}: {e}");
            }
            // _guard drops here → id + cancel flag released → next run_pass finalizes.
        });
    if let Err(e) = spawned {
        // spawn failed → don't leak the claim or the cancel flag.
        state.release_agentic(&directive.id, &mini_agent_id(directive));
        return Err(format!("could not spawn agentic worker: {e}"));
    }
    Ok(())
}
