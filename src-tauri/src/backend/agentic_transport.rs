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
            // Per-TURN output budget. Phase 7: the agentic path intentionally does NOT use
            // the one-shot caps (MAX_PROMPT_FILE_BYTES=32K front-load / OMLX_MAX_TOKENS_DEFAULT
            // =6144) — it reads files on demand and the runaway guard is `max_rounds`, not a
            // truncating token cap. A6: the per-turn budget is now MACHINE-TIERED (replaces the
            // blind 8192) so big-RAM hosts get more headroom and small hosts stay safe.
            max_tokens: crate::backend::oracle_coordinator::detected_max_tokens(),
        }
    }

    /// Phase 7: use a registry model's per-model tuned sampling, falling back to `tuned()`
    /// for any unset field. Connects the Phase-3 registry to the Phase-6 agentic transport.
    pub fn from_registry(entry: &crate::backend::model_registry::ModelRegistryEntry) -> Self {
        let d = Self::tuned();
        Self {
            temperature: entry.temperature.unwrap_or(d.temperature),
            top_p: entry.top_p.unwrap_or(d.top_p),
            top_k: entry.top_k.unwrap_or(d.top_k),
            thinking_budget: entry.thinking_budget.unwrap_or(d.thinking_budget),
            max_tokens: d.max_tokens,
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
            // An assistant turn that made tool calls MUST carry the `tool_calls` array
            // (content null) — otherwise the following `tool` messages are protocol-invalid.
            let mut obj = if let Some(tcs) = &m.tool_calls {
                json!({
                    "role": m.role,
                    "content": Value::Null,
                    "tool_calls": tcs
                        .iter()
                        .map(|tc| json!({
                            "id": tc.id,
                            "type": "function",
                            "function": { "name": tc.name, "arguments": tc.arguments },
                        }))
                        .collect::<Vec<_>>(),
                })
            } else {
                json!({ "role": m.role, "content": m.content })
            };
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
                    arguments: tc["function"]["arguments"]
                        .as_str()
                        .unwrap_or("{}")
                        .to_string(),
                })
            })
            .collect();
        if !calls.is_empty() {
            return Ok(LlmTurn::ToolCalls(calls));
        }
    }

    // A null/absent content with no VALID tool calls = an empty final message (graceful),
    // NOT an abort — a malformed turn from a quantized local model shouldn't kill the loop.
    let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
    Ok(LlmTurn::Message(content.to_string()))
}

/// Blocking OpenAI-compatible `AgentLlm` (oMLX/Ollama loopback **or** remote Cloud
/// e.g. OpenRouter). `base_url` includes the `/v1` suffix. Optional `api_key` is
/// sent as `Authorization: Bearer …` (required for public HTTPS providers; unused
/// by local oMLX/Ollama which ignore the bearer). The blocking client is fine here:
/// the agent loop runs on a dedicated worker, like the one-shot mini PTY path.
pub struct HttpAgentLlm {
    pub base_url: String,
    pub model: String,
    pub tools: Value,
    pub params: SamplingParams,
    pub enable_thinking: bool,
    /// When set, sent as Bearer on every chat/completions request (OpenRouter/cloud).
    pub api_key: Option<String>,
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
        Self::with_api_key(base_url, model, tools, params, enable_thinking, None)
    }

    pub fn with_api_key(
        base_url: String,
        model: String,
        tools: Value,
        params: SamplingParams,
        enable_thinking: bool,
        api_key: Option<String>,
    ) -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(600))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            base_url,
            model,
            tools,
            params,
            enable_thinking,
            api_key: api_key.filter(|k| !k.trim().is_empty()),
            client,
        })
    }
}

impl AgentLlm for HttpAgentLlm {
    fn next_turn(&mut self, messages: &[ChatMsg]) -> Result<LlmTurn, String> {
        let body = build_chat_request(
            &self.model,
            messages,
            &self.tools,
            &self.params,
            self.enable_thinking,
        );
        let url = format!("{}/chat/completions", self.base_url);
        let mut req = self.client.post(url).json(&body);
        if let Some(key) = self.api_key.as_deref() {
            req = req.bearer_auth(key);
        }
        let resp = req
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
        let raw =
            json!({ "choices": [{ "message": { "role": "assistant", "content": "all done" } }] });
        assert_eq!(
            parse_llm_turn(&raw).unwrap(),
            LlmTurn::Message("all done".to_string())
        );
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
        assert_eq!(
            body["chat_template_kwargs"]["enable_thinking"],
            json!(false)
        );
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

    #[test]
    fn from_registry_uses_per_model_params_with_fallback() {
        use crate::backend::model_registry::ModelRegistryEntry;
        let entry = ModelRegistryEntry {
            id: "m".into(),
            backend: "omlx".into(),
            size_bytes: 0,
            tier: "agentic".into(),
            roles: vec![],
            enabled: true,
            temperature: Some(0.3),
            top_p: None,
            top_k: Some(40),
            thinking_budget: None,
            context_window: 8192,
        };
        let p = SamplingParams::from_registry(&entry);
        assert_eq!(p.temperature, 0.3); // from entry
        assert_eq!(p.top_p, 0.95); // fallback to tuned()
        assert_eq!(p.top_k, 40); // from entry
        assert_eq!(p.thinking_budget, 2000); // fallback to tuned()
    }

    #[test]
    fn build_request_serializes_assistant_tool_calls() {
        use crate::backend::agentic_loop::{ChatMsg, ToolCall};
        let msgs = vec![
            ChatMsg {
                role: "assistant".into(),
                content: String::new(),
                tool_call_id: None,
                tool_calls: Some(vec![ToolCall {
                    id: "c1".into(),
                    name: "read_file".into(),
                    arguments: "{}".into(),
                }]),
            },
            ChatMsg {
                role: "tool".into(),
                content: "data".into(),
                tool_call_id: Some("c1".into()),
                tool_calls: None,
            },
        ];
        let body = build_chat_request("m", &msgs, &Value::Null, &SamplingParams::tuned(), false);
        let arr = body["messages"].as_array().unwrap();
        // Assistant turn carries the tool_calls array with content null (protocol-valid).
        assert_eq!(arr[0]["tool_calls"][0]["id"], "c1");
        assert_eq!(arr[0]["tool_calls"][0]["type"], "function");
        assert_eq!(arr[0]["tool_calls"][0]["function"]["name"], "read_file");
        assert!(arr[0]["content"].is_null());
        // Tool result references the call id.
        assert_eq!(arr[1]["role"], "tool");
        assert_eq!(arr[1]["tool_call_id"], "c1");
    }
}
