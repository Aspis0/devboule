//! pi-sidecar bridge — Phase 0 spike.
//!
//! Spawns a Node.js sidecar process (`pi-sidecar/sidecar.mjs`) that embeds the pi SDK,
//! reads its JSONL event stream from stdout, maps pi events to the existing
//! `MiniActivityEvent` / `ConsoleActivity` schema, and emits them on the
//! `mini-activity://pi-spike` channel so the existing `WorkConsole.tsx` renders them
//! WITHOUT any React changes.
//!
//! Design doc: `docs/devboule-on-pi-architecture.md` §7 (bridge), §11 (decisions).
//! Mirror pattern: `oracle/python_oracle.rs` (Command spawn + env injection).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};

use super::mini_activity::{ConsoleActivity, ConsoleEntry, MiniActivityEvent, NodeStyle};

// ---- constants -------------------------------------------------------------

/// The fixed agent id for the pi spike. Channel: `mini-activity://pi-spike`.
const PI_SPIKE_AGENT_ID: &str = "pi-spike";

// ---- state -----------------------------------------------------------------

/// Tauri-managed state for the pi sidecar. Holds the child process handle,
/// stdin writer, and a generation counter to prevent stale-event races (finding #5).
pub struct PiSidecarState {
    inner: Mutex<Option<PiSidecarInner>>,
    /// Monotonically incremented on each spawn. Reader threads tag events with
    /// their generation; events from a stale generation are discarded.
    generation: AtomicU64,
}

struct PiSidecarInner {
    child: Child,
    stdin: ChildStdin,
}

impl Default for PiSidecarState {
    fn default() -> Self {
        Self {
            inner: Mutex::new(None),
            generation: AtomicU64::new(0),
        }
    }
}

// ---- sidecar spawn (findings #1, #3, #5, #8) ------------------------------

/// Resolve the path to the `pi-sidecar/sidecar.mjs` script relative to the app.
/// Searches: (1) CWD (dev), (2) resource dir (release).
fn resolve_sidecar_script() -> Result<std::path::PathBuf, String> {
    // Dev: the script is at `<repo>/pi-sidecar/sidecar.mjs` relative to CWD.
    let dev_path = std::env::current_dir()
        .map_err(|e| format!("Cannot resolve CWD: {e}"))?
        .join("pi-sidecar")
        .join("sidecar.mjs");
    if dev_path.exists() {
        return Ok(dev_path);
    }
    Err(
        "pi-sidecar/sidecar.mjs not found. Run `npm install` in pi-sidecar/ first."
            .to_string(),
    )
}

/// Spawn the pi sidecar process and start the stdout event reader thread.
/// Idempotent: if already running, returns Ok immediately.
pub fn spawn_pi_sidecar(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<PiSidecarState>();
    let mut guard = state.inner.lock().unwrap_or_else(|e| e.into_inner());

    // Already running?
    if let Some(ref mut inner) = *guard {
        // Check if child is still alive.
        match inner.child.try_wait() {
            Ok(Some(_)) => {
                // Dead — fall through to respawn.
            }
            Ok(None) => return Ok(()), // Still running.
            Err(_) => return Ok(()),   // Assume running.
        }
    }

    let script = resolve_sidecar_script()?;

    // Resolve the pi-sidecar directory for npm package resolution.
    let sidecar_dir = script
        .parent()
        .ok_or_else(|| "Cannot resolve pi-sidecar directory".to_string())?
        .to_path_buf();

    // Finding #8: use `node` directly — the OS resolves via PATH.
    // If spawn fails, the error message is clear enough.
    let mut cmd = Command::new("node");
    cmd.arg(&script)
        .current_dir(&sidecar_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Finding #3: inherit stderr so it flows to the Tauri process stderr
        // (visible in the `tauri dev` terminal). Piped-but-never-read stderr
        // deadlocks after ~64KB.
        .stderr(Stdio::inherit());

    // Finding #1: Decision #10 — Claude blocked for external MCP (2026-07).
    // Default to OpenAI. Env vars still overridable.
    // TODO: read from vault via save_oracle_llm_settings path (vault.rs:927).
    cmd.env(
        "DEVBOULE_PI_PROVIDER",
        std::env::var("DEVBOULE_PI_PROVIDER").unwrap_or_else(|_| "openai".to_string()),
    );
    cmd.env(
        "DEVBOULE_PI_MODEL",
        std::env::var("DEVBOULE_PI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string()),
    );

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn pi sidecar (is Node.js installed?): {e}"))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "pi sidecar stdin not captured".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "pi sidecar stdout not captured".to_string())?;

    // Finding #5: bump generation so the old reader thread's events are discarded.
    let gen = state.generation.fetch_add(1, Ordering::SeqCst) + 1;

    // Start the stdout JSONL reader thread. It maps pi events to
    // MiniActivityEvent snapshots and emits them on the per-agent channel.
    let app_clone = app.clone();
    std::thread::spawn(move || {
        read_sidecar_events(app_clone, stdout, gen);
    });

    *guard = Some(PiSidecarInner { child, stdin });

    Ok(())
}

// ---- prompt command --------------------------------------------------------

/// Tauri command: send a prompt text to the running pi sidecar. Spawns the
/// sidecar if not already running.
#[tauri::command]
pub async fn spike_pi_prompt(app: AppHandle, text: String) -> Result<(), String> {
    // Ensure sidecar is running.
    spawn_pi_sidecar(&app)?;

    let state = app.state::<PiSidecarState>();
    let mut guard = state.inner.lock().unwrap_or_else(|e| e.into_inner());
    let inner = guard
        .as_mut()
        .ok_or_else(|| "pi sidecar not running after spawn".to_string())?;

    // Write the prompt command as JSONL to stdin.
    let cmd = serde_json::json!({
        "type": "prompt",
        "message": text,
    });
    let line = serde_json::to_string(&cmd).map_err(|e| format!("JSON serialize error: {e}"))?;
    inner
        .stdin
        .write_all(format!("{line}\n").as_bytes())
        .map_err(|e| format!("Failed to write to pi sidecar stdin: {e}"))?;
    inner
        .stdin
        .flush()
        .map_err(|e| format!("Failed to flush pi sidecar stdin: {e}"))?;

    Ok(())
}

// ---- event mapping ---------------------------------------------------------

/// The pi SDK event shapes we parse from the sidecar's JSONL stdout.
/// Only the fields we need for mapping are extracted; the rest is ignored.

#[derive(Debug, Deserialize)]
struct PiEvent {
    #[serde(rename = "type")]
    event_type: String,
    // message_update fields
    #[serde(default)]
    assistant_message_event: Option<AssistantMessageEvent>,
    // tool_execution_* fields
    #[serde(rename = "toolCallId", default)]
    tool_call_id: Option<String>,
    #[serde(rename = "toolName", default)]
    tool_name: Option<String>,
    #[serde(default)]
    args: Option<serde_json::Value>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(rename = "isError", default)]
    is_error: Option<bool>,
    // agent_end fields (not used in the spike but parsed for forward-compat)
    #[allow(dead_code)]
    #[serde(default)]
    messages: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct AssistantMessageEvent {
    #[serde(rename = "type")]
    delta_type: String,
    #[serde(default)]
    delta: Option<String>,
    // Finding #4: track contentIndex to handle multiple text blocks per message.
    #[serde(rename = "contentIndex", default)]
    content_index: Option<u32>,
}

/// Stateful mapper: converts pi SDK events into `ConsoleActivity` snapshots.
struct EventMapper {
    running: bool,
    entries: Vec<ConsoleEntry>,
    /// The text accumulated for the CURRENT text block (reset on each text_start).
    accumulated_text: String,
    tool_names: HashMap<String, String>,
    /// Finding #7: monotonic turn counter for StreamingChat.seq.
    turn_seq: u64,
    /// Finding #4: the contentIndex of the text block currently being accumulated.
    /// None when no text block is active.
    active_content_index: Option<u32>,
}

impl EventMapper {
    fn new() -> Self {
        Self {
            running: false,
            entries: Vec::new(),
            accumulated_text: String::new(),
            tool_names: HashMap::new(),
            turn_seq: 0,
            active_content_index: None,
        }
    }

    fn now_str() -> String {
        // Short HH:MM:SS local timestamp (same pattern as mini_activity.rs).
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = now.as_secs() % 86400;
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{h:02}:{m:02}:{s:02}")
    }

    fn build_snapshot(&self) -> ConsoleActivity {
        ConsoleActivity {
            running: Some(self.running),
            run_count: if self.running { Some(1) } else { Some(0) },
            empty: if self.entries.is_empty() {
                Some(true)
            } else {
                None
            },
            entries: if self.entries.is_empty() {
                None
            } else {
                Some(self.entries.clone())
            },
            task_cost_estimate_usd: None,
            // Finding #7: use monotonic turn_seq instead of hardcoded 1.
            streaming_chat: if self.running && !self.accumulated_text.is_empty() {
                Some(super::mini_activity::StreamingChat {
                    seq: self.turn_seq,
                    text: self.accumulated_text.clone(),
                })
            } else {
                None
            },
        }
    }

    fn emit_snapshot(&self, app: &AppHandle) {
        let snapshot = self.build_snapshot();
        let event = MiniActivityEvent::Snapshot {
            activity: snapshot,
        };
        let channel = super::mini_activity::mini_activity_channel(PI_SPIKE_AGENT_ID);
        let _ = app.emit(&channel, event);
    }

    /// Finding #4: flush the current accumulated_text as a ChatEntry, if non-empty.
    fn flush_text_block(&mut self) {
        if !self.accumulated_text.is_empty() {
            self.entries.push(ConsoleEntry::Chat {
                role: "assistant".to_string(),
                text: std::mem::take(&mut self.accumulated_text),
                time: Self::now_str(),
                msg_id: None,
            });
        }
        self.active_content_index = None;
    }

    fn handle_event(&mut self, app: &AppHandle, event: &PiEvent) {
        match event.event_type.as_str() {
            "agent_start" => {
                self.running = true;
                self.turn_seq += 1; // Finding #7: new turn.
                self.accumulated_text.clear();
                self.active_content_index = None;
                self.emit_snapshot(app);
            }

            "agent_end" => {
                // Finalize any accumulated text as a ChatEntry.
                self.flush_text_block();
                self.running = false;
                self.emit_snapshot(app);
            }

            "message_update" => {
                if let Some(ref delta_event) = event.assistant_message_event {
                    match delta_event.delta_type.as_str() {
                        // Finding #4: reset accumulated_text on each text_start
                        // (flush previous block first), track contentIndex.
                        "text_start" => {
                            self.flush_text_block();
                            self.active_content_index = delta_event.content_index;
                        }
                        "text_delta" => {
                            if let Some(ref delta) = delta_event.delta {
                                self.accumulated_text.push_str(delta);
                            }
                        }
                        "text_end" => {
                            // Text block done — leave accumulated_text for
                            // agent_end or next text_start to flush.
                        }
                        _ => {}
                    }
                }
                self.emit_snapshot(app);
            }

            "tool_execution_start" => {
                let tool_name = event
                    .tool_name
                    .clone()
                    .unwrap_or_else(|| "tool".to_string());
                if let Some(ref id) = event.tool_call_id {
                    self.tool_names.insert(id.clone(), tool_name.clone());
                }
                // Flush any accumulated text before the tool call.
                self.flush_text_block();
                let args_str = event
                    .args
                    .as_ref()
                    .map(|a| serde_json::to_string(a).unwrap_or_default())
                    .unwrap_or_default();
                self.entries.push(ConsoleEntry::Coder {
                    node: Some(NodeStyle::Dot),
                    text: format!("🔧 Calling `{tool_name}`"),
                    time: Self::now_str(),
                });
                self.entries.push(ConsoleEntry::Coder {
                    node: None,
                    text: format!("  args: {args_str}"),
                    time: String::new(),
                });
                self.emit_snapshot(app);
            }

            "tool_execution_update" => {
                self.emit_snapshot(app);
            }

            "tool_execution_end" => {
                let tool_name = event
                    .tool_call_id
                    .as_ref()
                    .and_then(|id| self.tool_names.get(id))
                    .cloned()
                    .unwrap_or_else(|| "tool".to_string());
                let result_summary = event
                    .result
                    .as_ref()
                    .and_then(|r| {
                        r.get("content")
                            .and_then(|c| c.as_array())
                            .and_then(|arr| arr.first())
                            .and_then(|item| item.get("text"))
                            .and_then(|t| t.as_str())
                    })
                    .map(|text| {
                        if text.len() > 200 {
                            format!("{}…", &text[..200])
                        } else {
                            text.to_string()
                        }
                    })
                    .unwrap_or_else(|| "(no result)".to_string());
                let is_error = event.is_error.unwrap_or(false);
                let status_icon = if is_error { "❌" } else { "✅" };
                self.entries.push(ConsoleEntry::Coder {
                    node: Some(NodeStyle::Sage),
                    text: format!("{status_icon} `{tool_name}` → {result_summary}"),
                    time: Self::now_str(),
                });
                self.emit_snapshot(app);
            }

            "turn_start" | "turn_end" | "message_start" | "message_end" => {
                self.emit_snapshot(app);
            }

            "response" => {
                self.emit_snapshot(app);
            }

            "ready" => {
                self.emit_snapshot(app);
            }

            _ => {
                // Unknown event type — ignore silently.
            }
        }
    }
}

// ---- stdout reader (finding #5: generation guard, #9: truncated log) -------

/// Blocking reader thread: reads JSONL from the sidecar's stdout, parses each
/// line as a `PiEvent`, and maps it to `MiniActivityEvent` snapshots emitted on
/// the `mini-activity://pi-spike` channel.
fn read_sidecar_events(app: AppHandle, stdout: std::process::ChildStdout, gen: u64) {
    let reader = BufReader::new(stdout);
    let mut mapper = EventMapper::new();

    for line_result in reader.lines() {
        let line = match line_result {
            Ok(l) => l,
            Err(_) => break, // stdout closed (sidecar exited).
        };
        if line.trim().is_empty() {
            continue;
        }
        // Finding #5: discard events from a superseded generation.
        let state = app.state::<PiSidecarState>();
        if state.generation.load(Ordering::SeqCst) != gen {
            break; // A newer sidecar was spawned — stop reading.
        }
        match serde_json::from_str::<PiEvent>(&line) {
            Ok(event) => {
                mapper.handle_event(&app, &event);
            }
            Err(_) => {
                // Finding #9: truncate logged line to avoid leaking large payloads.
                let preview: String = line.chars().take(200).collect();
                eprintln!("[pi-sidecar] unparseable JSONL line: {preview}...");
            }
        }
    }

    // Sidecar exited — mark as stopped (only if still our generation).
    let state = app.state::<PiSidecarState>();
    if state.generation.load(Ordering::SeqCst) == gen {
        mapper.running = false;
        mapper.emit_snapshot(&app);
    }
}
