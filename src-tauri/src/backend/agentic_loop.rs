//! Agentic tool-loop engine (Phase 6) — backend-agnostic, unit-testable.
//!
//! The core multi-turn loop for a capable (>20B) local coder: ask the model, run any
//! tool calls it emits (read/edit/run, sandboxed), feed the results back, repeat until it
//! reports done or a runaway guard trips. The LLM transport (oMLX/Ollama tool-calling) and
//! the sandboxed tool set are injected via the `AgentLlm` / `AgentTools` traits, so this
//! file has NO HTTP, NO filesystem, NO tauri — it is pure loop control + tested with mocks.

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON-string arguments as emitted by the model (parsed by the tool executor).
    pub arguments: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LlmTurn {
    ToolCalls(Vec<ToolCall>),
    Message(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatMsg {
    pub role: String,
    pub content: String,
    pub tool_call_id: Option<String>,
    /// Set on an ASSISTANT turn that made tool calls — the transport serializes these as the
    /// OpenAI `tool_calls` array so the following `tool` messages (by `tool_call_id`) are
    /// valid. Without this the server rejects the conversation (tool msg with no prior call).
    pub tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LoopOutcome {
    Done { output: String, rounds: u32 },
    Aborted { reason: String, rounds: u32 },
}

/// One model turn. The transport impl translates to/from the OpenAI tool-calling wire shape.
pub trait AgentLlm {
    fn next_turn(&mut self, messages: &[ChatMsg]) -> Result<LlmTurn, String>;
}

/// Execute a tool by name with raw JSON-string args. An `Err` is fed back to the model as
/// the tool result so it can recover (the loop does not abort on a tool error).
pub trait AgentTools {
    fn call(&mut self, name: &str, arguments: &str) -> Result<String, String>;
}

/// Drive the agentic loop until the model finishes, errors, or hits `max_rounds`
/// (the runaway guard — replaces the one-shot token cap). Never panics.
pub fn run_agent_loop(
    llm: &mut dyn AgentLlm,
    tools: &mut dyn AgentTools,
    system: &str,
    task: &str,
    max_rounds: u32,
    cancel: &std::sync::atomic::AtomicBool,
) -> LoopOutcome {
    let mut messages = vec![
        ChatMsg {
            role: "system".to_string(),
            content: system.to_string(),
            tool_call_id: None,
            tool_calls: None,
        },
        ChatMsg {
            role: "user".to_string(),
            content: task.to_string(),
            tool_call_id: None,
            tool_calls: None,
        },
    ];

    let mut rounds = 0u32;
    // Whether the model has actually done anything (executed ≥1 tool call). Guards against a
    // degenerate model returning a blank message on turn 1 being treated as a successful Done.
    let mut made_progress = false;
    loop {
        rounds += 1;
        if rounds > max_rounds {
            return LoopOutcome::Aborted {
                reason: format!("max rounds ({max_rounds}) exceeded"),
                rounds: rounds - 1,
            };
        }
        // User Stop / kill: bail before starting another (expensive) model turn.
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return LoopOutcome::Aborted { reason: "cancelled".to_string(), rounds: rounds - 1 };
        }

        match llm.next_turn(&messages) {
            Err(e) => return LoopOutcome::Aborted { reason: format!("llm error: {e}"), rounds },
            Ok(LlmTurn::Message(content)) => {
                // A blank final message before ANY tool call = the model produced nothing
                // useful → abort (escalate), don't report a false success.
                if content.trim().is_empty() && !made_progress {
                    return LoopOutcome::Aborted {
                        reason: "model returned no content and made no tool calls".to_string(),
                        rounds,
                    };
                }
                return LoopOutcome::Done { output: content, rounds };
            }
            Ok(LlmTurn::ToolCalls(calls)) => {
                if calls.is_empty() {
                    // A turn with neither a message nor any call: finished only if real work
                    // already happened; otherwise it's a degenerate empty response → abort.
                    if made_progress {
                        return LoopOutcome::Done { output: String::new(), rounds };
                    }
                    return LoopOutcome::Aborted {
                        reason: "model returned an empty turn before doing any work".to_string(),
                        rounds,
                    };
                }
                // Record the assistant's tool-call turn, then each tool result, so the next
                // turn sees the full transcript.
                // The assistant turn must carry the tool_calls array (OpenAI protocol) so the
                // following tool messages are valid — NOT a text summary.
                messages.push(ChatMsg {
                    role: "assistant".to_string(),
                    content: String::new(),
                    tool_call_id: None,
                    tool_calls: Some(calls.clone()),
                });
                for call in calls {
                    // Stop between tool calls too, so a kill can't fire one more write.
                    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                        return LoopOutcome::Aborted { reason: "cancelled".to_string(), rounds };
                    }
                    let result = tools
                        .call(&call.name, &call.arguments)
                        .unwrap_or_else(|e| format!("ERROR: {e}"));
                    messages.push(ChatMsg {
                        role: "tool".to_string(),
                        content: result,
                        tool_call_id: Some(call.id.clone()),
                        tool_calls: None,
                    });
                }
                made_progress = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct MockLlm {
        turns: VecDeque<Result<LlmTurn, String>>,
    }
    impl AgentLlm for MockLlm {
        fn next_turn(&mut self, _messages: &[ChatMsg]) -> Result<LlmTurn, String> {
            self.turns
                .pop_front()
                .unwrap_or_else(|| Err("no more scripted turns".to_string()))
        }
    }

    struct MockTools {
        calls: Vec<(String, String)>,
        response: String,
    }
    impl AgentTools for MockTools {
        fn call(&mut self, name: &str, arguments: &str) -> Result<String, String> {
            self.calls.push((name.to_string(), arguments.to_string()));
            Ok(self.response.clone())
        }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall { id: "1".to_string(), name: name.to_string(), arguments: "{}".to_string() }
    }

    #[test]
    fn happy_path_runs_tool_then_finishes() {
        let mut llm = MockLlm {
            turns: VecDeque::from(vec![
                Ok(LlmTurn::ToolCalls(vec![call("read_file")])),
                Ok(LlmTurn::Message("done".to_string())),
            ]),
        };
        let mut tools = MockTools { calls: vec![], response: "file content".to_string() };
        let outcome = run_agent_loop(&mut llm, &mut tools, "sys", "task", 5, &std::sync::atomic::AtomicBool::new(false));
        assert_eq!(outcome, LoopOutcome::Done { output: "done".to_string(), rounds: 2 });
        assert_eq!(tools.calls.len(), 1);
        assert_eq!(tools.calls[0].0, "read_file");
    }

    #[test]
    fn runaway_aborts_at_max_rounds() {
        // Always returns a NON-empty tool call → the loop never finishes → runaway guard.
        let mut llm = MockLlm {
            turns: VecDeque::from(vec![
                Ok(LlmTurn::ToolCalls(vec![call("grep")])),
                Ok(LlmTurn::ToolCalls(vec![call("grep")])),
                Ok(LlmTurn::ToolCalls(vec![call("grep")])),
                Ok(LlmTurn::ToolCalls(vec![call("grep")])),
            ]),
        };
        let mut tools = MockTools { calls: vec![], response: "ok".to_string() };
        let outcome = run_agent_loop(&mut llm, &mut tools, "sys", "task", 3, &std::sync::atomic::AtomicBool::new(false));
        assert_eq!(
            outcome,
            LoopOutcome::Aborted { reason: "max rounds (3) exceeded".to_string(), rounds: 3 }
        );
        assert_eq!(tools.calls.len(), 3); // 3 rounds executed before the abort
    }

    #[test]
    fn llm_error_aborts() {
        let mut llm = MockLlm { turns: VecDeque::from(vec![Err("api down".to_string())]) };
        let mut tools = MockTools { calls: vec![], response: String::new() };
        let outcome = run_agent_loop(&mut llm, &mut tools, "sys", "task", 5, &std::sync::atomic::AtomicBool::new(false));
        assert_eq!(
            outcome,
            LoopOutcome::Aborted { reason: "llm error: api down".to_string(), rounds: 1 }
        );
    }

    #[test]
    fn tool_error_is_fed_back_not_fatal() {
        struct FailingTools;
        impl AgentTools for FailingTools {
            fn call(&mut self, _n: &str, _a: &str) -> Result<String, String> {
                Err("boom".to_string())
            }
        }
        // Tool errors must NOT abort: the model gets the error and then finishes.
        let mut llm = MockLlm {
            turns: VecDeque::from(vec![
                Ok(LlmTurn::ToolCalls(vec![call("edit_file")])),
                Ok(LlmTurn::Message("recovered".to_string())),
            ]),
        };
        let mut tools = FailingTools;
        let outcome = run_agent_loop(&mut llm, &mut tools, "sys", "task", 5, &std::sync::atomic::AtomicBool::new(false));
        assert_eq!(outcome, LoopOutcome::Done { output: "recovered".to_string(), rounds: 2 });
    }

    #[test]
    fn cancel_flag_aborts_before_running_a_tool() {
        let mut llm = MockLlm {
            turns: VecDeque::from(vec![Ok(LlmTurn::ToolCalls(vec![call("read_file")]))]),
        };
        let mut tools = MockTools { calls: vec![], response: "x".to_string() };
        let cancel = std::sync::atomic::AtomicBool::new(true); // already cancelled
        let outcome = run_agent_loop(&mut llm, &mut tools, "sys", "task", 5, &cancel);
        assert_eq!(outcome, LoopOutcome::Aborted { reason: "cancelled".to_string(), rounds: 0 });
        assert_eq!(tools.calls.len(), 0); // never executed a tool
    }

    #[test]
    fn blank_first_message_aborts_not_false_done() {
        let mut llm =
            MockLlm { turns: VecDeque::from(vec![Ok(LlmTurn::Message("   ".to_string()))]) };
        let mut tools = MockTools { calls: vec![], response: String::new() };
        let outcome = run_agent_loop(
            &mut llm,
            &mut tools,
            "sys",
            "task",
            5,
            &std::sync::atomic::AtomicBool::new(false),
        );
        match outcome {
            LoopOutcome::Aborted { reason, .. } => assert!(reason.contains("no content")),
            other => panic!("expected Aborted, got {other:?}"),
        }
    }
}
