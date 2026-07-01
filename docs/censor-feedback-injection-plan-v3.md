# Censor Feedback Injection Plan v3

**Date:** 2026-06-30
**Status:** Draft (verified against actual source)
**Goal:** Automatically inject Censor runner findings into the agent's context after file modifications, without requiring the agent to explicitly ask.

---

## 1. The Problem (Real)

The Censor has 46 deterministic runners + optional LLM judge. After any file change, it runs linters and writes findings to `.aspis-censor/` shards. **But the agents (main coder and mini coders) never see these findings unless they explicitly call `censor_findings` — and they mostly forget.**

### Why the v2 plan was wrong

The v2 plan proposed modifying `devboule-coder`'s `run_burst` loop to check Censor after "write" actions. **devboule-coder has no write actions.** The `AgentAction` enum has no `Write` or `Edit` variant. File writes happen in the **Tauri Rust backend** via `apply_edits` in `mini_coder_executor.rs`. The devboule-coder binary is read-only on the filesystem — the plan was hooking the wrong layer.

### Verified Architecture (Agent Hierarchy + Two Write Paths)

```
                        ┌─────────────────────┐
                        │    Orchestrator      │
                        │  (plans only!)       │
                        └──────────┬──────────┘
                                   │ hands plan to
                        ┌──────────▼──────────┐
                        │    Main Coder        │  ← decision-maker, spawns minis
                        │  spawns minis,       │
                        │  decides fix/retry   │
                        └──┬──────────────┬───┘
                           │              │
              ┌────────────▼──┐   ┌──────▼──────────────┐
              │ Agentic Mini  │   │   Small Mini         │
              │ (MCP tools,   │   │   (emits diffs,      │
              │  sandbox,     │   │    fully dependent   │
              │  independent) │   │    on main coder)    │
              └──────┬───────┘   └──────┬──────────────┘
                     │                  │
        WriteMode::AgenticIterative    WriteMode::EmitEdits
                     │                  │
                     └────────┬─────────┘
                              │
                     ┌────────▼────────┐
                     │  Tauri Backend   │  ← FILES ACTUALLY WRITTEN HERE
                     │  (Rust)          │
                     └────────┬────────┘
                              │
                     ┌────────▼────────┐
                     │  Censor Watcher  │
                     │  → 46 runners    │
                     │  → optional LLM  │
                     │  → shard JSON    │
                     └────────┬────────┘
                              │
                     ┌────────▼────────┐
                     │  [GAP]           │
                     │  Main coder      │
                     │  never sees      │
                     │  findings!!      │
                     └─────────────────┘
```

### The correct injection point

Findings must be injected **by the Tauri backend** into the **main coder's context**:

1. When `SpawnMini` returns → append Censor findings to the tool result
2. When deep runners/LLM finish later → inject as steer message

The **main coder** is the decision-maker — it sees findings and decides: fix, re-spawn mini, or escalate.

| Who | Injection mechanism | Timing |
|---|---|---|
| **Main coder** (after spawn\_mini returns) | Append to `SpawnMini` tool result | Phase A (fast runners, ≤3s) |
| **Main coder** (deep findings later) | Steer message into burst loop | Phase B (slow runners/LLM) |

---

## 2. Design: Two-Phase Injection

### Phase A: Fast Runners (blocking, ≤3s)

After `apply_edits` writes files, wait for the Censor's **FINE pass** to complete (400ms debounce + runner execution). Append findings directly to the tool result — the agent sees them immediately.

### Phase B: Slow Runners + Optional LLM (async, post-hoc)

COARSE pass runners and the optional LLM judge take longer (4s debounce + 10-30s execution). When they complete, inject findings as a **steer message** into the agent's next burst turn.

```
Timeline:
t=0    apply_edits writes files
t=0.4  FINE runners start
t=1.5  FINE runners complete → Phase A injection (fast findings in tool result)
t=4    COARSE runners start
t=20   optional LLM completes
t=25   COARSE runners complete → Phase B injection (steer message in next turn)
```

---

## 3. Implementation

### 3.1 Phase A: Fast Runner Injection

**Where:** `mini_coder_executor.rs`, after files are written to disk — works for both paths:

- **AgenticIterative**: after MCP tool calls write to sandbox (backend applies them)
- **EmitEdits**: after `apply_edits()` writes the diffs to disk

**What changes:**

- After `apply_edits()` (currently around line 1862), collect the list of modified files
- Call `censor_get_findings` for those files with a **3-second timeout**
- Append formatted findings to the mini coder's output, which flows back as the `SpawnMini` tool result

```rust
// In finalize_finished_mini, after apply_edits succeeds:

let modified_files: Vec<String> = write_diffs.iter()
    .map(|(path, _)| path.clone())
    .collect();

// Phase A: wait for fast runners (3s timeout)
let findings = wait_for_censor_findings(
    &root, &modified_files, Duration::from_secs(3)
);

if !findings.is_empty() {
    // Append to the outcome — this flows back to the main coder
    outcome.censor_findings = Some(findings);
}
```

**New field on MiniCoderOutcome:**

```rust
pub struct MiniCoderOutcome {
    // ... existing fields ...
    /// Phase A Censor findings (fast runners only). None if no issues or timeout.
    pub censor_findings: Option<Vec<Finding>>,
}
```

**How the agent sees it:** When `SpawnMini` returns, the tool result includes:

```
✓ Mini completed: fixed bug in src/foo.rs

=== [Censor Fast Check] ===
🔴 [high] Unused variable
  File: src/foo.rs:42
  Source: clippy (correctness)
  The variable `x` is assigned but never read.

🟡 [medium] Long function
  File: src/foo.rs:15
  Source: lizard (complexity)
  Function `process_data` is 85 lines (threshold: 50).
=== [End Censor] ===
```

### 3.2 Phase B: Async Steer Injection

**Where:** The Censor orchestrator (`orchestrator.rs`), after COARSE pass + LLM complete.

**What changes:**

- When COARSE runners or the LLM produce findings for files touched by a currently-active mini coder or main coder, send a **steer message**
- Use the existing steer mechanism (Steer inbox → agent loop reads it mid-burst)
- This requires the backend to know which agent (project_id) is currently active

```rust
// In the Censor orchestrator, after runners complete:

fn emit_censor_steer(
    app: &AppHandle,
    project_id: &str,
    modified_files: &[String],
    findings: &[Finding],
) {
    let steer_msg = format_censor_steer(findings);
    // Write to the steer file watched by the main coder
    // (same mechanism as human steer messages)
    let steer_path = format!(".aspis/steer_censor_{project_id}");
    fs::write(&steer_path, steer_msg).ok();
}
```

**How the agent sees it:** In the next burst turn, a steer message appears:

```
💬 steer: === [Censor Deep Check] ===
🔴 [high] Potential SQL injection
  File: src/db.rs:128
  Source: semgrep (security)
  ...
=== [End Censor] ===
```

### 3.3 What the agent does with findings

The system prompt is updated with:

```
# Censor Feedback

After file modifications, you will see automated Censor findings in two forms:
1. Inline: appended to the tool result (fast checks, immediate)
2. Steer: injected as a mid-burst message (deep checks, next turn)

Priority: 🔴 High → fix now. 🟡 Medium → fix if time. 🟢 Low → note and continue.
If a finding persists after 2 fix attempts, escalate with details.
```

### 3.4 Token Budget

To prevent context bloat:

- Max 10 findings per injection
- Max 4096 bytes per injection message
- Priority: High > Medium > Low
- Only findings with `disposition = Open`
- Findings with `verdict = Confirmed` sorted before `Suspected`

---

## 4. What DOES NOT Change

| Component | Status |
|-----------|--------|
| Censor watcher | ✅ Unchanged — keeps monitoring filesystem |
| Censor runners (46 tools) | ✅ Unchanged — keeps running per-language linters |
| Censor shard format (Finding, CensorShard) | ✅ Unchanged — on-disk schema intact |
| Optional LLM judge | ✅ Unchanged — keeps async, writes to shards |
| `censor_get_findings` MCP tool | ✅ Unchanged — kept for explicit agent queries |
| `censor://findings-updated` event | ✅ Unchanged — UI frontend keeps receiving it |
| `spawn_verdict_thread` | ✅ KEPT as-is — already fire-and-forget, Phase A is additive |
| devboule-coder burst loop | ✅ Unchanged — no modifications needed |
| `ToolExecutor` trait | ✅ Unchanged — no new methods needed |
| `TranscriptEntry` | ✅ Unchanged — steer messages already supported via `Human` variant |
| `MiniCoderStatus` | ✅ Unchanged — no variants removed |

---

## 5. What Changes

| File | Change |
|------|--------|
| `mini_coder_executor.rs` | After `apply_edits`, call `wait_for_censor_findings()` with 3s timeout, append to outcome |
| `mini_coder.rs` | Add `censor_findings: Option<Vec<Finding>>` to `MiniCoderOutcome` |
| `censor/orchestrator.rs` | After COARSE runners/LLM complete, emit steer message for active project |
| `censor/commands.rs` | Add synchronous `censor_get_findings_for_files(root, files, timeout)` helper |
| `prompt.rs` | Add Censor feedback section to system prompt |
| `mini_coder_executor.rs` | `spawn_verdict_thread` already fire-and-forget — no changes needed |

### Deletion Plan: Remove everything replaced

This plan makes the following code **dead** — it MUST be deleted, not kept:

| Delete | File | Lines | Reason |
|--------|------|-------|--------|
| `spawn_verdict_thread` | `mini_coder_executor.rs` | ~2091-2165 | Phase A injects findings; no need to rerun linters in a thread |
| `run_verdict_thread_body` | `mini_coder_executor.rs` | ~2167+ | Called only by spawn_verdict_thread |
| `EscalationFinding` struct | `mini_coder.rs` | ~314-328 | Replaced by regular `Finding` from Censor schema |
| `summarize_findings_for_feedback` | `mini_coder.rs` | ~1080 | Replaced by `format_findings_text` (Step 3) |
| `build_retry_directive` | `mini_coder.rs` | ~1010 | Agent re-spawns minis via prompt, not Rust |
| `verdict_gate_decision` | `mini_coder.rs` | ~1100+ | Gate logic replaced by agent-driven decisions |
| `AwaitingRetry` variant | `mini_coder.rs` | `MiniCoderStatus` | No Rust-level retry state needed |
| `Escalated` variant | `mini_coder.rs` | `MiniCoderStatus` | Agent escalates via `Escalate` action |
| `needs_gate` block | `mini_coder_executor.rs` | ~1998-2035 | Phase A replaces the gate check entirely |
| `verdict_fn` parameter | `mini_coder_executor.rs` | `finalize_finished_mini_with` | No verdict function needed — findings already in outcome |
| `real_censor_verdict` call sites | `mini_coder_executor.rs` | verdict thread body | Only called from verdict gate paths |
| `claim_verdict` / `release_verdict` | `mini_coder_executor.rs` | inflight guard | No verdict gate means no inflight tracking |
| **~44 existing tests** | `mini_coder.rs` + `mini_coder_executor.rs` | See Step 8-C | Tests reference deleted symbols — must be removed |

### Guarantee: Nothing kept that's replaced

Every line of code that the new injection mechanism replaces is **removed**, not commented out, not kept "for reference." Clean codebase.

---

## 6. Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| **FINE runners too slow** (3s timeout hit) | Phase A returns empty; Phase B delivers findings via steer |
| **Steer file race** | Atomic write (Censor already uses this pattern in `ledger.rs`) |
| **Context bloat** | Token budget: 10 findings max, 4096 bytes, priority-sorted |
| **Agent ignores findings** | Prompt-level instructions + severity icons |
| **Duplicate findings** (same finding appears in Phase A + Phase B) | Deduplicate by `finding.id` (SHA-256, already deterministic) |
| **Findings for wrong project** | Steer is keyed by `project_id` |

---

## 7. Task Breakdown (TDD: red → green per step)

### Step 1: Add `wait_for_censor_findings` helper

**File:** `src-tauri/src/backend/censor/commands.rs`
**Verified:** `ledger::read_shard(root: &Path, file_rel_path: &str) -> io::Result<Option<CensorShard>>` exists. Shards at `.aspis-censor/<sha256(file)>.json`. Finding has `disposition: Disposition` (Open/Fixed/Fp/Wontfix).

#### 🔴 RED — Write a failing test first

Add to the existing `#[cfg(test)] mod tests` block:

```rust
#[test]
fn wait_for_censor_findings_returns_open_findings() {
    use std::fs;
    use std::time::Duration;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // No shards exist yet → must return empty immediately
    let got = wait_for_censor_findings(root, &["src/a.rs".into()], Duration::from_secs(1));
    assert!(got.is_empty(), "no shards → empty");
}

#[test]
fn wait_for_censor_findings_finds_preexisting_shard() {
    use std::fs;
    use std::time::Duration;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let censor_dir = root.join(".aspis-censor");
    fs::create_dir_all(&censor_dir).unwrap();
    // Write a shard with one open finding
    let shard = CensorShard {
        file_rel_path: "src/a.rs".into(),
        content_hash: "h".into(),
        updated_at: "t".into(),
        findings: vec![Finding {
            id: "f1".into(),
            file: "src/a.rs".into(),
            severity: Severity::High,
            category: Category::Correctness,
            source: "clippy".into(),
            title: "unused".into(),
            body: "x is unused".into(),
            disposition: Disposition::Open,
            ..Default::default()
        }],
    };
    let shard_path = censor_dir.join("abc.json");
    fs::write(&shard_path, serde_json::to_string(&shard).unwrap()).unwrap();
    // Wait should find it instantly (shard already on disk)
    let got = wait_for_censor_findings(root, &["src/a.rs".into()], Duration::from_secs(2));
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].title, "unused");
}

#[test]
fn wait_for_censor_findings_skips_non_open_disposition() {
    // Fixed/Fp/Wontfix findings must be skipped
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let censor_dir = root.join(".aspis-censor");
    fs::create_dir_all(&censor_dir).unwrap();
    let shard = CensorShard {
        file_rel_path: "src/a.rs".into(),
        content_hash: "h".into(),
        updated_at: "t".into(),
        findings: vec![
            Finding { id: "f1".into(), disposition: Disposition::Fixed, ..Default::default() },
            Finding { id: "f2".into(), disposition: Disposition::Fp, ..Default::default() },
            Finding { id: "f3".into(), disposition: Disposition::Wontfix, ..Default::default() },
        ],
    };
    fs::write(censor_dir.join("x.json"), serde_json::to_string(&shard).unwrap()).unwrap();
    let got = wait_for_censor_findings(root, &["src/a.rs".into()], Duration::from_secs(2));
    assert!(got.is_empty(), "Fixed/Fp/Wontfix must be skipped");
}
```

Run: `cargo test wait_for_censor_findings` → ❌ FAILS (function doesn't exist yet)

#### 🟢 GREEN — Implement

```rust
/// Wait up to `timeout` for Censor findings on the given files.
/// Polls the shard directory every 200ms. Returns immediately if no findings.
pub fn wait_for_censor_findings(
    root: &Path,
    files: &[String],
    timeout: Duration,
) -> Vec<Finding> {
    let deadline = Instant::now() + timeout;
    loop {
        let mut all: Vec<Finding> = Vec::new();
        for file in files {
            if let Ok(Some(shard)) = ledger::read_shard(root, file) {
                for f in shard.findings {
                    if f.disposition == Disposition::Open {
                        all.push(f);
                    }
                }
            }
        }
        if !all.is_empty() || Instant::now() >= deadline {
            return all;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}
```

Run: `cargo test wait_for_censor_findings` → ✅ PASSES

---

### Step 2: Add `censor_findings` field to `MiniCoderOutcome`

**File:** `src-tauri/src/backend/mini_coder.rs`
**Verified:** Struct at line 340 (`#[derive(…Default…)]`), body at line 342. Has 10 fields: `status`, `output`, `files_touched`, `edits`, `question`, `partial`, `error`, `escalation`, `net_blocked`, `folder_write_blocked`. Default is derived, no manual impl.

#### 🔴 RED — Write a failing test first

```rust
#[test]
fn mini_coder_outcome_has_censor_findings_field() {
    let outcome = MiniCoderOutcome::default();
    assert!(outcome.censor_findings.is_none(), "default is None");
}

#[test]
fn mini_coder_outcome_censor_findings_serializes() {
    let outcome = MiniCoderOutcome {
        censor_findings: Some(vec![]),
        ..Default::default()
    };
    let json = serde_json::to_string(&outcome).unwrap();
    // Must NOT include the field when None (skip_serializing_if)
    let outcome_none = MiniCoderOutcome::default();
    let json_none = serde_json::to_string(&outcome_none).unwrap();
    assert!(!json_none.contains("censorFindings"), "None must be skipped");
}
```

Run: `cargo test mini_coder_outcome_censor` → ❌ FAILS

#### 🟢 GREEN — Implement

Find `pub struct MiniCoderOutcome` (~line 348). Add:

```rust
    /// Phase A Censor findings (fast runners). None = no issues or timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub censor_findings: Option<Vec<crate::backend::censor::schema::Finding>>,
```

Run: `cargo test mini_coder_outcome_censor` → ✅ PASSES

---

### Step 3: Format findings as human-readable text

**File:** `src-tauri/src/backend/censor/commands.rs`

#### 🔴 RED — Write tests first

```rust
#[test]
fn format_findings_text_empty_returns_empty() {
    assert_eq!(format_findings_text(&[]), "");
}

#[test]
fn format_findings_text_sorts_by_severity() {
    let findings = vec![
        Finding { severity: Severity::Low, title: "low1".into(), ..Default::default() },
        Finding { severity: Severity::High, title: "high1".into(), ..Default::default() },
        Finding { severity: Severity::Medium, title: "mid1".into(), ..Default::default() },
    ];
    let text = format_findings_text(&findings);
    let hi = text.find("high1").unwrap();
    let mi = text.find("mid1").unwrap();
    let lo = text.find("low1").unwrap();
    assert!(hi < mi, "High must come before Medium");
    assert!(mi < lo, "Medium must come before Low");
}

#[test]
fn format_findings_text_caps_at_10() {
    let findings: Vec<Finding> = (0..15)
        .map(|i| Finding { title: format!("f{i}"), ..Default::default() })
        .collect();
    let text = format_findings_text(&findings);
    let count = text.matches("🟡").count(); // default severity = Medium
    assert!(count <= 10, "max 10 findings: got {count}");
}

#[test]
fn format_findings_text_max_4096_bytes() {
    let finding = Finding {
        body: "x".repeat(5000),
        ..Default::default()
    };
    let text = format_findings_text(&[finding]);
    assert!(text.len() <= 4096, "max 4096 bytes: got {}", text.len());
}
```

Run: `cargo test format_findings` → ❌ FAILS

#### 🟢 GREEN — Implement

```rust
/// Format Censor findings as human-readable text for agent context.
/// Token budget: max 10 findings, max 4096 bytes, sorted by severity.
pub fn format_findings_text(findings: &[Finding]) -> String {
    if findings.is_empty() {
        return String::new();
    }
    let mut sorted: Vec<&Finding> = findings.iter().collect();
    sorted.sort_by_key(|f| severity_rank(f.severity));
    let capped: Vec<&&Finding> = sorted.iter().take(10).collect();
    let mut out = String::new();
    for f in &capped {
        let icon = match f.severity {
            Severity::High => "🔴",
            Severity::Medium => "🟡",
            Severity::Low => "🟢",
        };
        let line = f.line.map(|n| format!(":{n}")).unwrap_or_default();
        out.push_str(&format!(
            "{icon} [{:?}] {}\n  File: {}{}\n  Source: {} ({:?})\n  {}\n\n",
            f.severity, f.title, f.file, line, f.source, f.category, f.body
        ));
        if out.len() > 4096 {
            out.truncate(4096);
            break;
        }
    }
    out
}

fn severity_rank(s: Severity) -> u8 {
    match s { Severity::High => 0, Severity::Medium => 1, Severity::Low => 2 }
}
```

Run: `cargo test format_findings` → ✅ PASSES

---

### Step 4: Call `wait_for_censor_findings` in `finalize_finished_mini`

**File:** `src-tauri/src/backend/mini_coder_executor.rs`
**Verified:** Function at line 1841, signature `fn finalize_finished_mini(app: &AppHandle, directive: &MiniCoderDirective)`. Edits applied via `apply_write_directive_edits` (NOT "apply_edits") at line 1866, returns `(outcome, write_diffs)` where `write_diffs: Vec<(String, Vec<DiffLine>)>`. Project root available as `apply_root` (line 1852). Insertion point: between line 1866 and line 1868.

#### 🔴 RED — Write test (integration-style, uses test harness)

```rust
#[test]
fn finalize_finished_mini_populates_censor_findings_after_write() {
    // Setup: create a temp project with a Rust file that has a clippy warning
    // 1. Write a minimal src/main.rs with `let x = 1;` (unused variable)
    // 2. Create a MiniCoderDirective with write=true, files=["src/main.rs"]
    // 3. Run finalize_finished_mini
    // 4. Assert outcome.censor_findings is Some and non-empty
    // NOTE: requires clippy installed on test machine — skip if not found
    if !command_exists("cargo") || !command_exists("clippy") {
        return; // skip test
    }
    // ... test body
}
```

Run: `cargo test finalize_finished_mini_censor` → ❌ FAILS (field not populated yet)

#### 🟢 GREEN — Implement

In `finalize_finished_mini` (~line 1841), after `apply_edits` succeeds:

```rust
// Collect modified files from write_diffs
let modified_files: Vec<String> = write_diffs.iter()
    .map(|(path, _)| path.clone())
    .collect();

// Phase A: wait for Censor fast runners (non-blocking: 3s timeout)
if !modified_files.is_empty() && trusted {
    let findings = censor::commands::wait_for_censor_findings(
        &root, &modified_files, Duration::from_secs(3)
    );
    if !findings.is_empty() {
        outcome.censor_findings = Some(findings);
    }
}
```

Run: `cargo test finalize_finished_mini_censor` → ✅ PASSES

### Step 4b: Emit Activity Console event for human-visible feedback

**File:** `src-tauri/src/backend/mini_coder_executor.rs`

**Why:** pi-lens shows `lsp active: typos, ast-grep, opengrep` in real-time. We need the same for Censor: when Phase A finds issues, the human sees it on the Activity Console alongside the spawn_mini progress.

**Verified:** `console_finalize()` at line 2911 already renders findings. We hook into the same mechanism — after Phase A populates `outcome.censor_findings`, emit a console event.

#### 🔴 RED — Write test first

```rust
#[test]
fn censor_findings_summary_counts_by_severity() {
    let findings = vec![
        Finding { severity: Severity::High, ..Default::default() },
        Finding { severity: Severity::High, ..Default::default() },
        Finding { severity: Severity::Medium, ..Default::default() },
    ];
    let summary = censor_findings_summary(&findings);
    assert_eq!(summary.high, 2);
    assert_eq!(summary.medium, 1);
    assert_eq!(summary.low, 0);
    assert_eq!(summary.total, 3);
}

#[test]
fn censor_findings_summary_empty_is_all_zeros() {
    let summary = censor_findings_summary(&[]);
    assert_eq!(summary.high, 0);
    assert_eq!(summary.total, 0);
}
```

Run → ❌ FAILS

#### 🟢 GREEN — Implement

Add a summary struct + function in `mini_coder_executor.rs`:

```rust
struct CensorFindingsSummary { high: usize, medium: usize, low: usize, total: usize }

fn censor_findings_summary(findings: &[Finding]) -> CensorFindingsSummary {
    let high = findings.iter().filter(|f| f.severity == Severity::High).count();
    let medium = findings.iter().filter(|f| f.severity == Severity::Medium).count();
    let low = findings.iter().filter(|f| f.severity == Severity::Low).count();
    CensorFindingsSummary { high, medium, low, total: findings.len() }
}
```

In Step 4's hook, right after `outcome.censor_findings = Some(findings)`, emit to console:

```rust
use crate::backend::mini_activity as console;
let summary = censor_findings_summary(&findings);
if let Some(store) = console::console_store(app) {
    store.update(app, &directive.id, |a| {
        console::set_censor_verdict(a, summary.high, summary.medium, summary.low);
    });
}
```

Look up `console_store` (line 4315) and `console::set_censor_verdict` — if it doesn't exist, use a Tauri event instead:

```rust
let _ = app.emit("censor://mini-findings", serde_json::json!({
    "agentId": directive.id,
    "total": summary.total,
    "high": summary.high,
    "medium": summary.medium,
    "low": summary.low,
}));
```

Run: `cargo test censor_findings_summary` → ✅ 2 passed
Run: `cargo check` → ✅ zero errors

---

### Step 5: Inject findings into the mini's output

**File:** `src-tauri/src/backend/mini_coder_executor.rs`
**Important:** There is NO text-formatted "terminal stamp" function. The outcome is JSON-serialized → Python MCP → LLM sees raw JSON. Findings must be prepended to `outcome.output` (an `Option<String>` field).

#### 🔴 RED — Write test

```rust
#[test]
fn terminal_output_includes_censor_findings_when_present() {
    let outcome = MiniCoderOutcome {
        censor_findings: Some(vec![Finding {
            severity: Severity::High,
            title: "test bug".into(),
            source: "clippy".into(),
            ..Default::default()
        }]),
        ..Default::default()
    };
    let stamp = format_terminal_stamp(&outcome, /* ... */);
    assert!(stamp.contains("Censor Fast Check"), "stamp must include Censor section");
    assert!(stamp.contains("test bug"), "stamp must include finding title");
}

#[test]
fn terminal_output_omits_censor_when_no_findings() {
    let outcome = MiniCoderOutcome::default();
    let stamp = format_terminal_stamp(&outcome, /* ... */);
    assert!(!stamp.contains("Censor Fast Check"), "no Censor section when None");
}
```

Run → ❌ FAILS

#### 🟢 GREEN — Implement

In the terminal stamp formatting (where the mini's output is assembled):

```rust
if let Some(ref findings) = outcome.censor_findings {
    if !findings.is_empty() {
        stamp.push_str("\n=== [Censor Fast Check] ===\n");
        stamp.push_str(&censor::commands::format_findings_text(findings));
        stamp.push_str("=== [End Censor] ===\n");
    }
}
```

Run → ✅ PASSES

---

### Step 6: Emit steer message for slow/deep findings

**File:** `src-tauri/src/backend/censor/orchestrator.rs`
**Verified:** Hook points: inside `run_fine_batch_inner` after `fine_batch_collect` returns, and inside `run_coarse_pass` after `coarse_pass_collect` returns. Both have `project_id`. Steer uses `DEVBOULE_STEER_FILE` env var. Solution: write to `<project_root>/.aspis/steer_censor` — the main coder is launched with that path as `DEVBOULE_STEER_FILE`.

#### 🔴 RED — Write test

```rust
#[test]
fn emit_censor_steer_writes_to_steer_file() {
    let tmp = tempfile::tempdir().unwrap();
    let steer_path = tmp.path().join("steer_censor_test_project");
    let findings = vec![Finding {
        severity: Severity::High,
        title: "SQL injection".into(),
        source: "semgrep".into(),
        ..Default::default()
    }];
    emit_censor_steer_to_path(&steer_path, &findings);
    let content = std::fs::read_to_string(&steer_path).unwrap();
    assert!(content.contains("Censor Deep Check"), "steer must be labeled");
    assert!(content.contains("SQL injection"), "steer must contain finding");
}

#[test]
fn emit_censor_steer_empty_findings_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let steer_path = tmp.path().join("steer_censor_test_project");
    emit_censor_steer_to_path(&steer_path, &[]);
    assert!(!steer_path.exists(), "no file for empty findings");
}
```

Run → ❌ FAILS

#### 🟢 GREEN — Implement

```rust
/// Write Censor findings as a steer message for the main coder.
/// The main coder's burst loop picks this up via `drain_steer()`.
pub fn emit_censor_steer_to_path(path: &Path, findings: &[Finding]) {
    if findings.is_empty() {
        return;
    }
    let text = format!(
        "=== [Censor Deep Check] ===\n{}\n=== [End Censor] ===",
        crate::backend::censor::commands::format_findings_text(findings)
    );
    std::fs::write(path, text).ok();
}
```

Run → ✅ PASSES

---

### Step 7: Update system prompt

**File:** `devboule-coder/src/prompt.rs`

#### 🔴 RED — Write test first

Add to the existing `#[cfg(test)] mod tests` in `prompt.rs`:

```rust
#[test]
fn system_prompt_includes_censor_feedback_rules() {
    let prompt = build_system_prompt(false);
    assert!(prompt.contains("Censor Feedback"), "prompt must have Censor section");
    assert!(prompt.contains("High → fix now"), "must explain High severity");
    assert!(prompt.contains("Medium → fix if time"), "must explain Medium severity");
    assert!(prompt.contains("Low → note and continue"), "must explain Low severity");
}
```

Run: `cargo test system_prompt_includes_censor` → ❌ FAILS

#### 🟢 GREEN — Implement

In `build_system_prompt()`, append:

```rust
    let censor_rules = "\n\n# Censor Feedback\n\n\
After spawn_mini completes, you may see Censor findings in the tool result.\n\
Deep findings may arrive later as [steer] messages.\n\n\
🔴 High → fix now (security/correctness)\n\
🟡 Medium → fix if time permits\n\
🟢 Low → note and continue\n\n\
If the same finding persists after 2 fix attempts, escalate.";

    format!("{base}{censor_rules}{plan_first}")
```

Run: `cargo test system_prompt_includes_censor` → ✅ PASSES

---

### Step 8: Delete old verdict gate (all replaced code)

**Files:** `mini_coder_executor.rs` + `mini_coder.rs`

**Verified:** All items listed below exist. Phase A (Steps 4+5) makes them redundant — agent-driven decisions replace Rust-level gate logic.

#### 🔴 RED — Prove gate code exists now (passes before deletion)

```rust
#[test]
fn old_verdict_gate_still_exists() {
    let exec = std::fs::read_to_string("src/backend/mini_coder_executor.rs").unwrap();
    assert!(exec.contains("fn spawn_verdict_thread"));
    assert!(exec.contains("if needs_gate"));

    let mc = std::fs::read_to_string("src/backend/mini_coder.rs").unwrap();
    assert!(mc.contains("struct EscalationFinding"));
    assert!(mc.contains("fn build_retry_directive"));
    assert!(mc.contains("AwaitingRetry"));
}
```

Run: `cargo test old_verdict_gate` → ✅ PASSES

#### 🟢 GREEN — Delete in order, verify each compiles

**A. `mini_coder.rs` deletions:**

| # | Delete | ~Line |
|---|--------|-------|
| 1 | `EscalationFinding` struct | 314-328 |
| 2 | `summarize_findings_for_feedback` fn | 1080 |
| 3 | `build_retry_directive` fn | 1010 |
| 4 | `verdict_gate_decision` fn + `GateDecision` enum | 1100+ |
| 5 | `AwaitingRetry` variant from `MiniCoderStatus` | ~202 |
| 6 | `Escalated` variant from `MiniCoderStatus` | ~208 |
| 7 | `EscalationInfo` struct (if only used by escalate path) | |
| 8 | Update `is_terminal()` — remove AwaitingRetry/Escalated arms | 210+ |

**B. `mini_coder_executor.rs` deletions:**

| # | Delete | ~Line |
|---|--------|-------|
| 9 | `spawn_verdict_thread` fn | 2091-2165 |
| 10 | `run_verdict_thread_body` fn | 2167+ |
| 11 | `real_censor_verdict` fn | 3159+ |
| 12 | `claim_verdict` / `release_verdict` inflight guard | |
| 13 | `if needs_gate { ... }` block in `finalize_finished_mini` | 1998-2035 |
| 14 | `verdict_fn` parameter from `finalize_finished_mini_with` | 2400 |
| 15 | All `use super::mini_coder::EscalationFinding` imports | |

Run after each: `cargo check` → zero errors

#### 🔴 RED — Verify deletion complete

```rust
#[test]
fn verdict_gate_code_is_gone() {
    let exec = std::fs::read_to_string("src/backend/mini_coder_executor.rs").unwrap();
    assert!(!exec.contains("fn spawn_verdict_thread"));
    assert!(!exec.contains("if needs_gate"));
    assert!(!exec.contains("real_censor_verdict"));
    assert!(!exec.contains("EscalationFinding"));

    let mc = std::fs::read_to_string("src/backend/mini_coder.rs").unwrap();
    assert!(!mc.contains("struct EscalationFinding"));
    assert!(!mc.contains("fn summarize_findings_for_feedback"));
    assert!(!mc.contains("fn build_retry_directive"));
    assert!(!mc.contains("AwaitingRetry"));
    assert!(!mc.contains("Escalated,"));
}
```

Run: `cargo test verdict_gate_code_is_gone` → ✅ PASSES

Run: `cargo test` (full suite) → ✅ all tests pass, no compile errors

**C. Delete tests that reference removed code (~44 tests):**

In `mini_coder.rs` (26 tests to delete): `retry_directive_preserves_write_mode`, `build_retry_directive_*` (2), `plan_tick_*awaiting_retry*` (2), `awaiting_retry_*` (7), `gate_*` (6), `escalated_*` (3), `verdict_gate_decision_*`, `escalated_outcome_*` (2), `chain_root_id_*`.

In `mini_coder_executor.rs` (18 tests to delete): `verdict_inflight_*` (3), `verdict_thread_body_*` (2), `verdict_stop_flag_*`, `*awaiting_retry*` (7), `*escalated*` (2), `finalize_gate_decision_*`, `b2_gate_and_a3_lister_*`, `mark_steer_requested_*`.

Any test whose function name or body references deleted symbols (`EscalationFinding`, `build_retry_directive`, `spawn_verdict_thread`, `AwaitingRetry`, `Escalated`) must be removed.

Run: `cargo test` → ✅ all remaining tests pass

---

## 8. Why This Approach

| Aspect | v2 Plan (hallucinated) | v3 Plan (this one) |
|--------|----------------------|---------------------|
| Injection point | devboule-coder burst loop | Tauri backend after apply_edits |
| Requires new AgentAction? | Yes (but no Write exists) | No |
| Requires new ToolExecutor method? | Yes (`censor_findings()`) | No |
| Requires new TranscriptEntry? | Yes (`CensorFindings`) | No (uses existing `Human` steer) |
| Handles fast vs slow runners? | No | Yes (Phase A + Phase B) |
| Removes things that don't exist? | Yes (`Censoring` status) | No |
| Modifies devboule-coder? | Yes (4 files) | No (only Tauri backend + prompt) |

---

## 9. References

- Censor schema: `src-tauri/src/backend/censor/schema.rs` (Finding, CensorShard, Severity, Category, Verdict, Disposition)
- Censor commands: `src-tauri/src/backend/censor/commands.rs` (`censor_get_findings` at line 561)
- Censor watcher: `src-tauri/src/backend/censor/watch.rs`
- Censor orchestrator: `src-tauri/src/backend/censor/orchestrator.rs`
- Mini coder executor: `src-tauri/src/backend/mini_coder_executor.rs` (`apply_edits`, `finalize_finished_mini`, `spawn_verdict_thread`)
- Mini coder types: `src-tauri/src/backend/mini_coder.rs` (`MiniCoderOutcome`, `MiniCoderStatus`, `EscalationFinding`)
- Agent loop: `devboule-coder/src/agent_loop.rs` (`run_burst`, `ToolExecutor`, `Transcript`, `TranscriptEntry`)
- Executor: `devboule-coder/src/executor.rs` (`RealExecutor`, `McpBackend`)
- Steer mechanism: `devboule-coder/src/steer.rs`, `devboule-coder/src/executor.rs` (`drain_steer`)
- Prompt: `devboule-coder/src/prompt.rs` (`build_system_prompt`)
