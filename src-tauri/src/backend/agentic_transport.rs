//! Agentic-loop TRANSPORT (Phase 6b): the oMLX/Ollama (OpenAI-compatible) tool-calling
//! `AgentLlm` impl. The wire serialization (`build_chat_request`) + response parsing
//! (`parse_llm_turn`) are PURE + unit-tested; the HTTP call is thin blocking glue. Kept
//! separate from `agentic_loop` so the loop engine stays transport-free.

use std::time::Duration;

use serde_json::{json, Value};

use crate::backend::agentic_loop::{AgentLlm, ChatMsg, LlmTurn, ToolCall};

/// Per-model sampling, defaulting to the bake-off-tuned values (gemma/Qwen MoE on oMLX).
#[derive(Clone, Debug)]
pub struct SamplingParams {
    pub temperature: f64,
    pub top_p: f64,
    pub top_k: u32,
    pub thinking_budget: u32,
    pub max_tokens: u32,
}

impl SamplingParams {
    pub fn tuned() -> Self {
        Self {
            temperature: 0.6,
            top_p: 0.95,
            top_k: 20,
            thinking_budget: 2000,
            max_tokens: 8192,
        }
    }
}

/// PURE: build the OpenAI-compatible chat-completions request body. Omits `tools`/
/// `tool_choice` when no tools are given; adds `thinking_budget` only when thinking is on.
pub fn build_chat_request(
    model: &str,
    messages: &[ChatMsg],
    tools: &Value,
    params: &SamplingParams,
    enable_thinking: bool,
) -> Value {
    let msgs: Vec<Value> = messages
        .iter()
        .map(|m| {
            let mut obj = json!({ "role": m.role, "content": m.content });
            if let Some(id) = &m.tool_call_id {
                obj["tool_call_id"] = json!(id);
            }
            obj
        })
        .collect();

    let mut body = json!({
        "model": model,
        "messages": msgs,
        "temperature": params.temperature,
        "top_p": params.top_p,
        "top_k": params.top_k,
        "max_tokens": params.max_tokens,
        "stream": false,
        "chat_template_kwargs": { "enable_thinking": enable_thinking },
    });
    if enable_thinking {
        body["thinking_budget"] = json!(params.thinking_budget);
    }
    if tools.as_array().is_some_and(|a| !a.is_empty()) {
        body["tools"] = tools.clone();
        body["tool_choice"] = json!("auto");
    }
    body
}

/// PURE: map a chat-completions response to an `LlmTurn`. Tolerant — never panics on a
/// missing/wrong-type field; tool calls with an empty function name are skipped.
pub fn parse_llm_turn(resp: &Value) -> Result<LlmTurn, String> {
    let msg = resp["choices"][0]["message"]
        .as_object()
        .ok_or_else(|| "no choices/message in response".to_string())?;

    if let Some(arr) = msg.get("tool_calls").and_then(|v| v.as_array()) {
        let calls: Vec<ToolCall> = arr
            .iter()
            .filter_map(|tc| {
                let name = tc["function"]["name"].as_str().unwrap_or("");
                if name.is_empty() {
                    return None;
                }
                Some(ToolCall {
                    id: tc["id"].as_str().unwrap_or("").to_string(),
                    name: name.to_string(),
                    arguments: tc["function"]["arguments"].as_str().unwrap_or("{}").to_string(),
                })
            })
            .collect();
        if !calls.is_empty() {
            return Ok(LlmTurn::ToolCalls(calls));
        }
    }

    if let Some(content) = msg.get("content").and_then(|v| v.as_str()) {
        return Ok(LlmTurn::Message(content.to_string()));
    }

    Err("message had neither tool_calls nor content".to_string())
}

/// Blocking OpenAI-compatible `AgentLlm` (oMLX/Ollama loopback). `base_url` includes the
/// `/v1` suffix (e.g. `http://127.0.0.1:8000/v1`). The blocking client is fine here: the
/// agent loop runs on a dedicated worker, like the one-shot mini PTY path.
pub struct HttpAgentLlm {
    pub base_url: String,
    pub model: String,
    pub tools: Value,
    pub params: SamplingParams,
    pub enable_thinking: bool,
    client: reqwest::blocking::Client,
}

impl HttpAgentLlm {
    pub fn new(
        base_url: String,
        model: String,
        tools: Value,
        params: SamplingParams,
        enable_thinking: bool,
    ) -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(600))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { base_url, model, tools, params, enable_thinking, client })
    }
}

impl AgentLlm for HttpAgentLlm {
    fn next_turn(&mut self, messages: &[ChatMsg]) -> Result<LlmTurn, String> {
        let body =
            build_chat_request(&self.model, messages, &self.tools, &self.params, self.enable_thinking);
        let url = format!("{}/chat/completions", self.base_url);
        let resp = self
            .client
            .post(url)
            .json(&body)
            .send()
            .map_err(|e| format!("agentic LLM request failed: {e}"))?
            .json::<Value>()
            .map_err(|e| format!("agentic LLM response was not JSON: {e}"))?;
        parse_llm_turn(&resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tool_calls_response() {
        let raw = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "c1", "type": "function",
                        "function": { "name": "read_file", "arguments": "{\"path\":\"a.rs\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        match parse_llm_turn(&raw).unwrap() {
            LlmTurn::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "c1");
                assert_eq!(calls[0].name, "read_file");
                assert_eq!(calls[0].arguments, "{\"path\":\"a.rs\"}");
            }
            other => panic!("expected ToolCalls, got {other:?}"),
        }
    }

    #[test]
    fn parse_message_response() {
        let raw = json!({ "choices": [{ "message": { "role": "assistant", "content": "all done" } }] });
        assert_eq!(parse_llm_turn(&raw).unwrap(), LlmTurn::Message("all done".to_string()));
    }

    #[test]
    fn parse_garbage_is_err() {
        assert!(parse_llm_turn(&json!({})).is_err());
    }

    #[test]
    fn build_request_omits_tools_when_none() {
        let body = build_chat_request("m", &[], &Value::Null, &SamplingParams::tuned(), false);
        assert!(body["tools"].is_null());
        assert!(body["tool_choice"].is_null());
        assert!(body.get("thinking_budget").is_none());
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], json!(false));
        assert_eq!(body["model"], "m");
    }

    #[test]
    fn build_request_includes_tools_and_thinking_budget() {
        let tools = json!([{ "type": "function", "function": { "name": "read_file" } }]);
        let body = build_chat_request("m", &[], &tools, &SamplingParams::tuned(), true);
        assert!(body["tools"].is_array());
        assert_eq!(body["tool_choice"], json!("auto"));
        assert_eq!(body["thinking_budget"], json!(2000));
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], json!(true));
    }
}
