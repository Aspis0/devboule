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
    max_rounds: u32,
) -> Result<LoopOutcome, String> {
    let tools = default_tool_definitions();
    let mut llm = HttpAgentLlm::new(base_url, model, tools, params, enable_thinking)?;
    let mut fs_tools = ScopedAgentTools::new(root);
    Ok(run_agent_loop(&mut llm, &mut fs_tools, system, task, max_rounds))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn default_tool_definitions_are_well_formed() {
        let tools = default_tool_definitions();
        let arr = tools.as_array().expect("tools must be a JSON array");
        assert_eq!(arr.len(), 5);

        let mut names = HashSet::new();
        for tool in arr {
            assert_eq!(tool["type"], "function");
            let func = &tool["function"];
            assert_eq!(func["parameters"]["type"], "object");
            names.insert(func["name"].as_str().unwrap().to_string());
        }
        let expected: HashSet<String> =
            ["read_file", "list_dir", "grep", "edit_file", "write_file"]
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
}
