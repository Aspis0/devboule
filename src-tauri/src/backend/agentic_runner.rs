//! Agentic runner (Phase 6d): assembles the loop engine + oMLX/Ollama transport + the
//! sandboxed tools into one entry point the executor calls for an agentic-tier local model.
//! `default_tool_definitions` is the `tools` JSON given to the model; `run_agentic_coder`
//! wires `HttpAgentLlm` + `ScopedAgentTools` + `run_agent_loop`.
//!
//! LIVE-WIRING (the remaining, delicate step): `mini_coder_executor::claim_and_launch`
//! should, for an agentic-tier model, spawn a worker THREAD running `run_agentic_coder`
//! (HttpAgentLlm is blocking, like the PTY path) instead of `spawn_one_shot_mini`; on a
//! `Done` outcome the writes have already been applied by the tools, so it reports a
//! `MiniCoderResult{status:"done", filesTouched, edits:[]}` to the directive's result_path
//! and the EXISTING finalize → Censor → retry path runs unchanged.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::backend::agentic_loop::{run_agent_loop, LoopOutcome};
use crate::backend::agentic_tools::ScopedAgentTools;
use crate::backend::agentic_transport::{HttpAgentLlm, SamplingParams};

/// House rules for the local agentic coder (anti-loop, scope discipline). Embedded as a const
/// because the working doc (docs/local-coder-AGENTS.md) is not bundled with the app.
pub const AGENTIC_SYSTEM_PROMPT: &str = "\
You are a focused coding agent working DIRECTLY in a project via tools. Rules:\n\
- Use read_file/list_dir/grep to UNDERSTAND before you change anything. Never guess a file's\n\
  contents or invent APIs — read them.\n\
- Stay strictly inside the task's scope. Make MINIMAL, targeted edits with edit_file (exact\n\
  unique oldString) or write_file. Do not reformat or touch unrelated code.\n\
- edit_file's oldString must match EXACTLY and be unique; read the file first to get it right.\n\
- Work in small steps: read → edit → (read back if unsure). Do not repeat the same tool call.\n\
- When the task is complete, STOP and reply with a one-line plain-text summary (NO tool call).\n\
  Do not keep calling tools after the work is done.\n\
- If you cannot proceed (missing info, ambiguous task), say so plainly in a final message.";

/// DEFAULT round budget for the agentic loop (the runaway guard — replaces the one-shot token
/// cap). Generous on purpose: a real multi-file task needs many read→edit→verify cycles, and
/// the loop normally ends on its own (a final message). Overridable per the user's config
/// (`miniAgenticMaxRounds`). The loop is additionally bounded by the per-turn HTTP timeout.
pub const AGENTIC_MAX_ROUNDS: u32 = 40;

/// OpenAI-style `tools` array offered to the agentic coder (read + edit + search).
pub fn default_tool_definitions() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a file's contents (within the project scope).",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string", "description": "The file path to read." } },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_dir",
                "description": "List a directory (defaults to the scope root).",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string", "description": "The directory path to list." } },
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "grep",
                "description": "Search files for a literal substring.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "The substring to search for." },
                        "path": { "type": "string", "description": "The directory to search within." }
                    },
                    "required": ["pattern"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Replace the unique occurrence of oldString with newString in a file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "The file path to edit." },
                        "oldString": { "type": "string", "description": "The exact string to replace (must be unique in the file)." },
                        "newString": { "type": "string", "description": "The replacement string." }
                    },
                    "required": ["path", "oldString", "newString"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Create or overwrite a file (within scope).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "The file path to write." },
                        "content": { "type": "string", "description": "The full file content." }
                    },
                    "required": ["path", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "run",
                "description": "Run an allowlisted build/test/lint command in the project (any language: e.g. cargo test, go test ./..., pytest, npm test, make, gradle test, npx tsc --noEmit, dotnet test). No shell, no chaining/redirection, no escaping the project dir.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "e.g. 'cargo test', 'pytest', 'go test ./...', 'npm test'." }
                    },
                    "required": ["command"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "oracle_ask",
                "description": "Ask the project's Oracle a natural-language question about THIS codebase (how a module/database works, where a symbol is defined, how data flows). Returns a grounded answer over the project's indexed files. Read-only — prefer it over guessing.",
                "parameters": {
                    "type": "object",
                    "properties": { "query": { "type": "string", "description": "The question about this codebase." } },
                    "required": ["query"]
                }
            }
        }
    ])
}

/// Assemble the components and run the agentic coding loop. `root` is the scope (the
/// directive's project/scratch root); `system` is the local-coder house rules
/// (docs/local-coder-AGENTS.md). Blocking — call on a worker thread.
#[allow(clippy::too_many_arguments)]
pub fn run_agentic_coder(
    base_url: String,
    model: String,
    params: SamplingParams,
    enable_thinking: bool,
    system: &str,
    task: &str,
    root: PathBuf,
    write_allowlist: Vec<String>,
    net: crate::backend::sandbox::NetPolicy,
    // Broker Slice 2: effective working set (persisted + transient, already resolved by
    // `claim_and_launch`).  Passed to `ScopedAgentTools::with_working_set` so both the
    // app-level write check and the OS sandbox policy (`agentic_run_policy_with_working_set`)
    // treat these folders as writable.
    working_set: Vec<PathBuf>,
    max_rounds: u32,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(LoopOutcome, Vec<String>, bool, Option<String>), String> {
    // A4 (§4c live wiring): hold a compute permit for the WHOLE local agentic decode so the
    // coordinator's `active_local_decodes` reflects real GPU activity (what `admit_local_spawn`
    // gates on). RAII over this synchronous call → decremented on EVERY return/panic path. A `None`
    // (cap reached) does NOT hard-block here — the placement gate upstream owns admission; this keeps
    // the count honest without regressing an in-flight run.
    let _decode_permit = crate::backend::oracle_coordinator::coordinator().try_acquire_decode();
    let tools = default_tool_definitions();
    // Q1: append the per-MODEL-FAMILY thinking directive to the house-rules system prompt.
    // Gemma/North have no thinking_budget param, so their reasoning is bounded in the prompt;
    // Qwen (param-controlled) gets an empty directive → system unchanged. Computed from `&model`
    // BEFORE it is moved into HttpAgentLlm.
    let dir = crate::backend::mini_coder_executor::mini_thinking_directive(Some(&model));
    let effective_system = if dir.is_empty() {
        system.to_string()
    } else {
        format!("{system}\n{dir}")
    };
    let mut llm = HttpAgentLlm::new(base_url, model, tools, params, enable_thinking)?;
    // Reads are project-wide (for context); WRITES are confined to the directive's file
    // allowlist (empty = no extra restriction beyond the root).
    // A3: resolve the read-only Oracle scope (the project's indexed file_ids) BEFORE `root` is
    // moved into ScopedAgentTools. Best-effort: empty on any failure → the oracle_ask tool is
    // present but the bounded endpoint answers grounded-empty.
    let oracle_scope = crate::oracle::python_oracle::oracle_agent_scope_file_ids(&root);
    let mut fs_tools = ScopedAgentTools::new(root)
        .with_write_allowlist(write_allowlist)
        .with_net(net)
        .with_working_set(working_set)
        .with_oracle(oracle_scope);
    let outcome = run_agent_loop(
        &mut llm,
        &mut fs_tools,
        &effective_system,
        task,
        max_rounds,
        cancel,
    );
    let touched = fs_tools.touched().to_vec();
    let net_blocked = fs_tools.net_blocked();
    let out_of_scope_write = fs_tools.out_of_scope_write().map(str::to_string);
    Ok((outcome, touched, net_blocked, out_of_scope_write))
}

/// Serialize an agentic run into the MiniCoderResult wire JSON the executor's finalize path
/// reads. A finished loop → status "done" + the files the tools wrote (NO `edits` key — the
/// tools already applied them on disk). An aborted loop (runaway / LLM error) →
/// "needs_clarification" so it ESCALATES rather than falsely claiming success.
/// `net_blocked` is set to true when `ScopedAgentTools::net_blocked()` fired during the run
/// (net=None + network-blocked heuristic matched); omitted (NO-CHURN) when false.
/// `out_of_scope_write` is set to the canonicalized parent folder when a write attempt
/// targeted a path outside (root + working_set); omitted (NO-CHURN) when None.
pub fn agentic_result_json(
    outcome: &LoopOutcome,
    touched: &[String],
    net_blocked: bool,
    out_of_scope_write: Option<&str>,
) -> String {
    let mut value = match outcome {
        LoopOutcome::Done { output, .. } => json!({
            "status": "done",
            "output": if output.is_empty() { "agentic loop complete" } else { output.as_str() },
            "filesTouched": touched,
        }),
        LoopOutcome::Aborted { reason, .. } => json!({
            "status": "needs_clarification",
            "question": format!("agentic coder did not finish: {reason}"),
            "filesTouched": touched,
        }),
    };
    // NO-CHURN: only inject the field when true so existing result files that pre-date
    // this feature continue to deserialize cleanly (serde default = false).
    if net_blocked {
        value["netBlocked"] = json!(true);
    }
    // NO-CHURN: only inject when Some so existing result files pre-dating Slice 2
    // continue to deserialize cleanly (serde default = None).
    if let Some(folder) = out_of_scope_write {
        value["folderWriteBlocked"] = json!(folder);
    }
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn default_tool_definitions_are_well_formed() {
        let tools = default_tool_definitions();
        let arr = tools.as_array().expect("tools must be a JSON array");
        assert_eq!(arr.len(), 7);

        let mut names = HashSet::new();
        for tool in arr {
            assert_eq!(tool["type"], "function");
            let func = &tool["function"];
            assert_eq!(func["parameters"]["type"], "object");
            names.insert(func["name"].as_str().unwrap().to_string());
        }
        let expected: HashSet<String> =
            ["read_file", "list_dir", "grep", "edit_file", "write_file", "run", "oracle_ask"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        assert_eq!(names, expected);

        let edit = arr
            .iter()
            .find(|t| t["function"]["name"] == "edit_file")
            .expect("edit_file present");
        let required: HashSet<String> = edit["function"]["parameters"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            required,
            ["path", "oldString", "newString"].iter().map(|s| s.to_string()).collect()
        );
    }

    #[test]
    fn agentic_result_json_done_and_aborted() {
        let done = agentic_result_json(
            &LoopOutcome::Done { output: "ok".into(), rounds: 2 },
            &["src/a.rs".to_string()],
            false,
            None,
        );
        let v: Value = serde_json::from_str(&done).unwrap();
        assert_eq!(v["status"], "done");
        assert_eq!(v["output"], "ok");
        assert_eq!(v["filesTouched"][0], "src/a.rs");
        assert!(v.get("edits").is_none()); // agentic path applied edits itself
        assert!(v.get("netBlocked").is_none()); // not set when false (NO-CHURN)
        assert!(v.get("folderWriteBlocked").is_none()); // not set when None (NO-CHURN)

        let aborted = agentic_result_json(
            &LoopOutcome::Aborted { reason: "max rounds (8) exceeded".into(), rounds: 8 },
            &[],
            false,
            None,
        );
        let v2: Value = serde_json::from_str(&aborted).unwrap();
        assert_eq!(v2["status"], "needs_clarification");
        assert!(v2["question"].as_str().unwrap().contains("max rounds"));
    }

    #[test]
    fn agentic_result_json_net_blocked_is_present_when_true() {
        let json = agentic_result_json(
            &LoopOutcome::Done { output: "done".into(), rounds: 1 },
            &[],
            true,
            None,
        );
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["netBlocked"], true);
    }

    #[test]
    fn agentic_result_json_net_blocked_absent_when_false() {
        let json = agentic_result_json(
            &LoopOutcome::Done { output: "done".into(), rounds: 1 },
            &[],
            false,
            None,
        );
        let v: Value = serde_json::from_str(&json).unwrap();
        // NO-CHURN: field must be absent, not `false`.
        assert!(v.get("netBlocked").is_none());
    }

    #[test]
    fn agentic_result_json_folder_write_blocked_present_when_some() {
        let json = agentic_result_json(
            &LoopOutcome::Done { output: "done".into(), rounds: 1 },
            &[],
            false,
            Some("/tmp/outside"),
        );
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["folderWriteBlocked"], "/tmp/outside");
    }

    #[test]
    fn agentic_result_json_folder_write_blocked_absent_when_none() {
        let json = agentic_result_json(
            &LoopOutcome::Done { output: "done".into(), rounds: 1 },
            &[],
            false,
            None,
        );
        let v: Value = serde_json::from_str(&json).unwrap();
        assert!(v.get("folderWriteBlocked").is_none());
    }
}

