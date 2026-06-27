//! Slice 3a — Rust-side HTTP client for the Pigeon mailbox (Python FastAPI service on loopback).
//!
//! This module ONLY adds a client + tests; nothing calls it yet. The production entry point is
//! `crate::backend::pigeon_service::PigeonClient::from_running()`, which the mini-dispatch executor
//! will use later to drive register/poll/done/fail against the running Pigeon supervisor.
//!
//! Endpoints + shapes mirror `pigeon/dispatcher.py`:
//!   - POST /pigeon/agent  body {agent_id, agent_type, status}        -> {ok:true}
//!   - GET  /pigeon/poll?agent_id=<id>                                -> {ticket_no:int|null, payload:dict|null}
//!   - POST /pigeon/done   body {ticket_no, agent_id, result}         -> {ok:true, reply_ticket_no}
//!   - POST /pigeon/fail   body {ticket_no, agent_id, error}          -> {ok:true, outcome}
//! Auth: header `x-pigeon-auth-token: <token>` (401 on mismatch).

use std::time::Duration;

use serde_json::Value;

/// Header name the dispatcher checks on every authenticated route.
const AUTH_HEADER: &str = "x-pigeon-auth-token";
/// Default agent_type advertised on registration (the dispatcher defaults to "local" too).
const DEFAULT_AGENT_TYPE: &str = "local";

/// Blocking HTTP client for the Pigeon mailbox. Cheap to construct (builds its own
/// `reqwest::blocking::Client` on `new`); a real caller holds one instance for the agent's lifetime.
#[allow(dead_code)]
pub struct PigeonClient {
    base_url: String,
    token: String,
    http: reqwest::blocking::Client,
}

#[allow(dead_code)]
impl PigeonClient {
    /// Build a client against `base_url` (e.g. `http://127.0.0.1:24871`, no trailing slash) with the
    /// given auth `token`.
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Result<Self, String> {
        let http = reqwest::blocking::Client::builder()
            .build()
            .map_err(|e| format!("Pigeon client: failed to build HTTP client: {e}"))?;
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: token.into(),
            http,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// POST /pigeon/agent — announce this agent and its current load status to the mailbox.
    pub fn register_agent(&self, agent_id: &str, status: &str) -> Result<(), String> {
        let body = serde_json::json!({
            "agent_id": agent_id,
            "agent_type": DEFAULT_AGENT_TYPE,
            "status": status,
        });
        let resp = self
            .http
            .post(self.url("/pigeon/agent"))
            .timeout(Duration::from_secs(10))
            .header(AUTH_HEADER, &self.token)
            .json(&body)
            .send()
            .map_err(|e| format!("Pigeon register_agent: request failed: {e}"))?;
        // We don't need the body — just confirm a 2xx.
        let _ = parse_ok(resp, "register_agent")?;
        Ok(())
    }

    /// GET /pigeon/poll?agent_id=<id> — claim the next pending task for this agent.
    /// Returns `None` when the queue is empty (`ticket_no` is null), else `Some((ticket_no, payload))`.
    pub fn poll(&self, agent_id: &str) -> Result<Option<(i64, Value)>, String> {
        let resp = self
            .http
            // MAX-RECALL: /poll returns IMMEDIATELY (claim-or-null, not a long-poll), and it runs on
            // the single mini-executor thread — a long timeout here would stall every timeout/launch/
            // finalize if the dispatcher wedges. 5s is generous for a loopback immediate-return.
            .get(self.url("/pigeon/poll"))
            .timeout(Duration::from_secs(5))
            .header(AUTH_HEADER, &self.token)
            .query(&[("agent_id", agent_id)])
            .send()
            .map_err(|e| format!("Pigeon poll: request failed: {e}"))?;
        let body = parse_ok(resp, "poll")?;
        match body.get("ticket_no") {
            // null or absent => empty queue.
            None | Some(Value::Null) => Ok(None),
            Some(t) => {
                let ticket_no = t
                    .as_i64()
                    .ok_or_else(|| format!("Pigeon poll: ticket_no not an integer: {t}"))?;
                let payload = body.get("payload").cloned().unwrap_or(Value::Null);
                Ok(Some((ticket_no, payload)))
            }
        }
    }

    /// POST /pigeon/done — mark a claimed task done and store its result. Returns the auto-created
    /// reply ticket_no (the receipt the original sender will poll for).
    pub fn done(&self, ticket_no: i64, agent_id: &str, result: Value) -> Result<i64, String> {
        let body = serde_json::json!({
            "ticket_no": ticket_no,
            "agent_id": agent_id,
            "result": result,
        });
        let resp = self
            .http
            .post(self.url("/pigeon/done"))
            .timeout(Duration::from_secs(35))
            .header(AUTH_HEADER, &self.token)
            .json(&body)
            .send()
            .map_err(|e| format!("Pigeon done: request failed: {e}"))?;
        let body = parse_ok(resp, "done")?;
        body.get("reply_ticket_no")
            .and_then(|r| r.as_i64())
            .ok_or_else(|| format!("Pigeon done: missing/invalid reply_ticket_no in response: {body}"))
    }

    /// POST /pigeon/send — enqueue a task for `receiver_id`. Returns the assigned `ticket_no`.
    /// Mirrors the Python `_pigeon_send_*` producers (same body shape); the dispatcher auto-computes
    /// `delivery_mode`, so we omit it. Used by the executor to enqueue async Censor reviews.
    pub fn send(
        &self,
        sender_id: &str,
        receiver_id: &str,
        project_id: &str,
        priority: i64,
        payload: Value,
    ) -> Result<i64, String> {
        let body = serde_json::json!({
            "sender_id": sender_id,
            "receiver_id": receiver_id,
            "project_id": project_id,
            "priority": priority,
            "payload": payload,
        });
        let resp = self
            .http
            .post(self.url("/pigeon/send"))
            .timeout(Duration::from_secs(10))
            .header(AUTH_HEADER, &self.token)
            .json(&body)
            .send()
            .map_err(|e| format!("Pigeon send: request failed: {e}"))?;
        let body = parse_ok(resp, "send")?;
        body.get("ticket_no")
            .and_then(|t| t.as_i64())
            .ok_or_else(|| format!("Pigeon send: missing/invalid ticket_no in response: {body}"))
    }

    /// POST /pigeon/fail — report a claimed task as failed (requeue or dead-letter, server decides).
    /// Maps 403 (wrong receiver) / 409 (not claimed) / 404 (missing) into an `Err` carrying the code.
    pub fn fail(&self, ticket_no: i64, agent_id: &str, error: &str) -> Result<(), String> {
        let body = serde_json::json!({
            "ticket_no": ticket_no,
            "agent_id": agent_id,
            "error": error,
        });
        let resp = self
            .http
            .post(self.url("/pigeon/fail"))
            .timeout(Duration::from_secs(10))
            .header(AUTH_HEADER, &self.token)
            .json(&body)
            .send()
            .map_err(|e| format!("Pigeon fail: request failed: {e}"))?;
        let _ = parse_ok(resp, "fail")?;
        Ok(())
    }
}

/// Map a response into its parsed JSON body on 2xx, or an `Err(String)` that ALWAYS includes the
/// numeric status code so callers can distinguish 403/404/409/413/401 etc.
fn parse_ok(resp: reqwest::blocking::Response, op: &str) -> Result<Value, String> {
    let status = resp.status();
    if status.is_success() {
        resp.json::<Value>()
            .map_err(|e| format!("Pigeon {op}: invalid JSON in response: {e}"))
    } else {
        let code = status.as_u16();
        // MAX-RECALL: truncate the body — callers log these errors, and a response body could echo
        // task/result text. The status code (always present) is what callers branch on.
        let detail = resp.text().unwrap_or_default();
        let detail: String = detail.chars().take(256).collect();
        Err(format!("Pigeon {op}: HTTP {code}: {detail}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    /// What the mock server captured from the single request it served.
    struct Captured {
        request_line: String,
        headers: Vec<(String, String)>,
        body: String,
    }

    impl Captured {
        fn header(&self, name: &str) -> Option<&str> {
            let lname = name.to_ascii_lowercase();
            self.headers
                .iter()
                .find(|(k, _)| k.to_ascii_lowercase() == lname)
                .map(|(_, v)| v.as_str())
        }
    }

    /// Spin a one-shot HTTP/1.1 server on an ephemeral loopback port. It accepts exactly one
    /// connection, parses the request line / headers / body, returns the canned JSON `response_body`
    /// with `status_line`, and hands the captured request back over a channel.
    ///
    /// Returns `(base_url, join_handle_yielding_captured)`.
    fn spawn_mock(
        status_line: &'static str,
        response_body: &'static str,
    ) -> (String, thread::JoinHandle<Captured>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local_addr");
        let base_url = format!("http://127.0.0.1:{}", addr.port());

        let (ready_tx, ready_rx) = mpsc::channel::<()>();
        let handle = thread::spawn(move || {
            // Signal the test thread that the listener is live (it already is post-bind, but this
            // also pins listener ownership into the thread).
            let _ = ready_tx.send(());
            let (mut stream, _) = listener.accept().expect("accept one connection");

            // Read the head (until CRLFCRLF), then any Content-Length body.
            let mut buf = Vec::new();
            let mut tmp = [0u8; 1024];
            let header_end;
            loop {
                let n = stream.read(&mut tmp).expect("read head");
                if n == 0 {
                    panic!("connection closed before headers complete");
                }
                buf.extend_from_slice(&tmp[..n]);
                if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                    header_end = pos + 4;
                    break;
                }
            }

            let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
            let mut lines = head.split("\r\n");
            let request_line = lines.next().unwrap_or_default().to_string();
            let mut headers = Vec::new();
            let mut content_length = 0usize;
            for line in lines {
                if line.is_empty() {
                    continue;
                }
                if let Some((k, v)) = line.split_once(':') {
                    let k = k.trim().to_string();
                    let v = v.trim().to_string();
                    if k.eq_ignore_ascii_case("content-length") {
                        content_length = v.parse().unwrap_or(0);
                    }
                    headers.push((k, v));
                }
            }

            // Drain the remaining body bytes if any.
            let mut body_bytes = buf[header_end..].to_vec();
            while body_bytes.len() < content_length {
                let n = stream.read(&mut tmp).expect("read body");
                if n == 0 {
                    break;
                }
                body_bytes.extend_from_slice(&tmp[..n]);
            }
            let body = String::from_utf8_lossy(&body_bytes).to_string();

            let response = format!(
                "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            );
            stream.write_all(response.as_bytes()).expect("write response");
            stream.flush().ok();

            Captured {
                request_line,
                headers,
                body,
            }
        });

        // Wait until the thread is up before returning (best-effort).
        let _ = ready_rx.recv();
        (base_url, handle)
    }

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|w| w == needle)
    }

    const TOKEN: &str = "test-token-abc123";

    #[test]
    fn poll_parses_a_task() {
        let (base, handle) = spawn_mock(
            "HTTP/1.1 200 OK",
            r#"{"ticket_no": 5, "payload": {"kind": "edit", "path": "src/lib.rs"}}"#,
        );
        let client = PigeonClient::new(base, TOKEN).unwrap();
        let got = client.poll("mini-1").unwrap();

        let cap = handle.join().unwrap();
        // Request line carries the agent_id query param against /pigeon/poll.
        assert!(
            cap.request_line.starts_with("GET /pigeon/poll?agent_id=mini-1 "),
            "request line was: {}",
            cap.request_line
        );
        assert_eq!(cap.header(AUTH_HEADER), Some(TOKEN));

        let (ticket, payload) = got.expect("expected Some(task)");
        assert_eq!(ticket, 5);
        assert_eq!(payload["kind"], "edit");
        assert_eq!(payload["path"], "src/lib.rs");
    }

    #[test]
    fn poll_empty_queue_is_none() {
        let (base, handle) = spawn_mock("HTTP/1.1 200 OK", r#"{"ticket_no": null, "payload": null}"#);
        let client = PigeonClient::new(base, TOKEN).unwrap();
        let got = client.poll("mini-1").unwrap();
        let _ = handle.join().unwrap();
        assert!(got.is_none(), "null ticket_no must map to None");
    }

    #[test]
    fn done_sends_fields_and_parses_reply_ticket() {
        let (base, handle) = spawn_mock("HTTP/1.1 200 OK", r#"{"ok": true, "reply_ticket_no": 42}"#);
        let client = PigeonClient::new(base, TOKEN).unwrap();
        let reply = client
            .done(7, "mini-1", serde_json::json!({"status": "ok", "summary": "done"}))
            .unwrap();

        let cap = handle.join().unwrap();
        assert!(cap.request_line.starts_with("POST /pigeon/done "), "{}", cap.request_line);
        assert_eq!(cap.header(AUTH_HEADER), Some(TOKEN));

        let sent: Value = serde_json::from_str(&cap.body).expect("body is JSON");
        assert_eq!(sent["ticket_no"], 7);
        assert_eq!(sent["agent_id"], "mini-1");
        assert_eq!(sent["result"]["status"], "ok");
        assert_eq!(sent["result"]["summary"], "done");

        assert_eq!(reply, 42);
    }

    #[test]
    fn fail_maps_409_to_err_with_code() {
        let (base, handle) = spawn_mock(
            "HTTP/1.1 409 Conflict",
            r#"{"detail": "task is not in a claimed state"}"#,
        );
        let client = PigeonClient::new(base, TOKEN).unwrap();
        let res = client.fail(7, "mini-1", "boom");

        let cap = handle.join().unwrap();
        assert!(cap.request_line.starts_with("POST /pigeon/fail "), "{}", cap.request_line);
        let sent: Value = serde_json::from_str(&cap.body).expect("body is JSON");
        assert_eq!(sent["ticket_no"], 7);
        assert_eq!(sent["agent_id"], "mini-1");
        assert_eq!(sent["error"], "boom");

        let err = res.expect_err("409 must be an Err");
        assert!(err.contains("409"), "error must carry the status code, got: {err}");
    }

    #[test]
    fn register_agent_sends_body_and_auth_header() {
        let (base, handle) = spawn_mock("HTTP/1.1 200 OK", r#"{"ok": true}"#);
        let client = PigeonClient::new(base, TOKEN).unwrap();
        client.register_agent("mini-1", "loaded").unwrap();

        let cap = handle.join().unwrap();
        assert!(cap.request_line.starts_with("POST /pigeon/agent "), "{}", cap.request_line);
        assert_eq!(cap.header(AUTH_HEADER), Some(TOKEN));

        let sent: Value = serde_json::from_str(&cap.body).expect("body is JSON");
        assert_eq!(sent["agent_id"], "mini-1");
        assert_eq!(sent["status"], "loaded");
        assert_eq!(sent["agent_type"], DEFAULT_AGENT_TYPE);
    }

    #[test]
    fn send_posts_fields_and_parses_ticket() {
        let (base, handle) = spawn_mock(
            "HTTP/1.1 200 OK",
            r#"{"ticket_no": 99, "status": "pending", "delivery_mode": "queued"}"#,
        );
        let client = PigeonClient::new(base, TOKEN).unwrap();
        let ticket = client
            .send(
                "executor",
                "censor-pool",
                "proj-1",
                80,
                serde_json::json!({"file": "src/a.rs"}),
            )
            .unwrap();

        let cap = handle.join().unwrap();
        assert!(cap.request_line.starts_with("POST /pigeon/send "), "{}", cap.request_line);
        assert_eq!(cap.header(AUTH_HEADER), Some(TOKEN));

        let sent: Value = serde_json::from_str(&cap.body).expect("body is JSON");
        assert_eq!(sent["sender_id"], "executor");
        assert_eq!(sent["receiver_id"], "censor-pool");
        assert_eq!(sent["project_id"], "proj-1");
        assert_eq!(sent["priority"], 80);
        assert_eq!(sent["payload"]["file"], "src/a.rs");
        assert_eq!(ticket, 99);
    }
}
