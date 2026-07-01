# Censor Persistent Queue Plan v4 — Phase B Agnostic Delivery

**Date:** 2026-06-30
**Status:** Draft — fully traced against source
**Lines verified:** 2026-06-30, against `sandbox-epic` HEAD

---

## ⚠️ SCOPE: Phase B ONLY

| | Phase A (unchanged) | Phase B (this plan) |
|---|---|---|
| Runners | FINE, 400ms debounce | COARSE, 4s debounce + optional LLM |
| Latency | ≤3s | 4s–30s |
| Delivery | Synchronous, appended to SpawnMini result | Asynchronous, persistent queue |
| Consumers | Main coder OR agentic mini | Main coder only |
| Survives restart | N/A | ✅ Queue on disk |

Phase A is done (v3 plan). This plan fixes the delivery gap for slow findings that
arrive AFTER the write→spawn cycle completes.

---

## 1. Files touched

| # | File | Action | Lines |
|---|------|--------|-------|
| F1 | `src-tauri/src/backend/censor/schema.rs` | **ADD** `FindingBatch` struct + `short_hash` | +25 |
| F2 | `src-tauri/src/backend/censor/orchestrator.rs` | **REPLACE** `emit_censor_steer` → `enqueue_censor_findings` | -30, +40 |
| F3 | `src-tauri/src/backend/censor/commands.rs` | **ADD** `drain_censor_queue` pub fn | +40 |
| F4 | `devboule-coder/src/executor.rs` | **MODIFY** `drain_steer()` → add queue drain | +25 |
| F5 | `devboule-coder/src/prompt.rs` | **MODIFY** Censor Feedback block → add queue instruction | +8 |
| F6 | `oracle/server/aspis_mcp.py` | **MODIFY** `censor_findings` tool + handler | +40 |

**Total:** ~170 lines added, ~30 removed. Zero new dependencies. Zero config changes.

---

## 2. Cross-file data flow

```
┌────────────────────────────────────────────────────────────────────┐
│ PRODUCER: Censor worker thread (orchestrator.rs)                   │
│                                                                     │
│ run_coarse_pass() line 736                                          │
│   → coarse_pass_collect(root) → changed: Vec<String>                │
│   → collect_open_findings(root, &changed) → findings: Vec<Finding>  │
│   → [REPLACE] enqueue_censor_findings(root, &findings, &changed,    │
│                                      "coarse")                      │
│       │                                                             │
│       ├── writes .aspis/censor_queue/pending/<ts>_<hash>.json       │
│       └── writes .aspis/steer_censor (immediate, best-effort)       │
│                                                                     │
│ run_fine_batch_inner() line 551 (same pattern, pass_type="fine")    │
│   → [REPLACE] enqueue_censor_findings(root, &findings, &changed,    │
│                                      "fine")                        │
└────────────────────────────────┬───────────────────────────────────┘
                                 │
                    ┌────────────┴────────────┐
                    │                         │
        ┌───────────▼──────────┐   ┌─────────▼──────────┐
        │ CONSUMER: Local       │   │ CONSUMER: Cloud     │
        │ devboule-coder        │   │ Claude/Codex        │
        │                       │   │                     │
        │ drain_steer() line 791│   │ censor_findings()   │
        │  [MODIFY] +queue drain│   │  [MODIFY] +drain_q  │
        │                       │   │  param in schema    │
        │ reads:                 │   │  + handler          │
        │  .aspis/censor_queue/  │   │                     │
        │    pending/*.json      │   │ calls via MCP:      │
        │  .aspis/steer_censor   │   │  censor_findings(   │
        │                       │   │    drain_queue=true) │
        │ deletes after read     │   │                     │
        └───────────────────────┘   └─────────────────────┘
```

---

## 3. Detailed per-file changes

### F1 — `src-tauri/src/backend/censor/schema.rs`

**Where:** After `CensorShard` struct (line ~204), before `#[cfg(test)]` (line 210).

**Add:**

```rust
/// A batch of Censor findings from a single review pass. Written to the
/// persistent queue directory as a timestamped JSON file. Drained by the
/// main coder (local: burst loop; cloud: MCP `censor_findings(drain_queue=true)`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindingBatch {
    /// Unique id: `{ISO-timestamp}_{short-hash}` — also the filename.
    pub batch_id: String,
    /// ISO-8601 with timezone.
    pub timestamp: String,
    /// Which pass produced this: "fine" | "coarse" | "llm".
    pub pass_type: String,
    /// Which files triggered this pass.
    pub files: Vec<String>,
    /// Findings from this pass (open disposition only).
    pub findings: Vec<Finding>,
}
```

**Tests to add** (in `#[cfg(test)] mod tests` block):

```rust
#[test]
fn finding_batch_round_trips() {
    let batch = FindingBatch {
        batch_id: "2026-06-30T12:00:00_abcd".into(),
        timestamp: "2026-06-30T12:00:00Z".into(),
        pass_type: "coarse".into(),
        files: vec!["src/a.rs".into()],
        findings: vec![sample_finding()],
    };
    let json = serde_json::to_string(&batch).unwrap();
    let back: FindingBatch = serde_json::from_str(&json).unwrap();
    assert_eq!(back.batch_id, batch.batch_id);
    assert_eq!(back.findings.len(), 1);
}
```

**Check:** `cargo test finding_batch` → ✅

---

### F2 — `src-tauri/src/backend/censor/orchestrator.rs`

#### F2a — REPLACE the producer function

**Current** (line 971–986):

```rust
fn emit_censor_steer(root: &Path, findings: &[Finding]) {
    if findings.is_empty() {
        return;
    }
    let text = format!(
        "=== [Censor Deep Check] ===\n{}\n=== [End Censor] ===\n",
        crate::backend::censor::commands::format_findings_text(findings)
    );
    let steer_dir = root.join(".aspis");
    let _ = std::fs::create_dir_all(&steer_dir);
    let steer_path = steer_dir.join("steer_censor");
    // Overwrite (not append) — steer file is drained every burst turn
    std::fs::write(&steer_path, text).ok();
}
```

**Replace with:**

```rust
/// Enqueue Censor findings to the persistent queue AND write the immediate
/// steer file as best-effort cache. The queue survives restarts; the steer
/// file is a shortcut for the local devboule-coder's burst loop.
fn enqueue_censor_findings(root: &Path, findings: &[Finding], files: &[String], pass_type: &str) {
    if findings.is_empty() {
        return;
    }
    // ── persistent queue (survives restart, accumulates) ──
    let queue_dir = root.join(".aspis").join("censor_queue").join("pending");
    let _ = std::fs::create_dir_all(&queue_dir);

    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for f in findings { f.id.hash(&mut h); }
    let hash_suffix = format!("{:04x}", h.finish() & 0xFFFF);

    let now = chrono::Utc::now();
    let batch = crate::backend::censor::schema::FindingBatch {
        batch_id: format!("{}_{}", now.format("%Y-%m-%dT%H:%M:%S"), hash_suffix),
        timestamp: now.to_rfc3339(),
        pass_type: pass_type.to_string(),
        files: files.to_vec(),
        findings: findings.to_vec(),
    };
    let path = queue_dir.join(format!("{}.json", batch.batch_id));
    std::fs::write(&path, serde_json::to_string(&batch).unwrap_or_default()).ok();

    // ── immediate steer cache (best-effort, for burst loop) ──
    let steer_dir = root.join(".aspis");
    let _ = std::fs::create_dir_all(&steer_dir);
    let text = format!(
        "=== [Censor {} Check] ===\n{}\n=== [End Censor] ===\n",
        pass_type,
        crate::backend::censor::commands::format_findings_text(findings)
    );
    std::fs::write(steer_dir.join("steer_censor"), text).ok();
}
```

#### F2b — UPDATE both call sites

**Site 1** — `run_fine_batch_inner`, line ~566–568:

**Current:**

```rust
    let steer_findings = collect_open_findings(root, &changed);
    emit_censor_steer(root, &steer_findings);
```

**Replace with:**

```rust
    enqueue_censor_findings(root, &steer_findings, &changed, "fine");
```

**Site 2** — `run_coarse_pass`, line ~741–744:

**Current:**

```rust
    let steer_findings = collect_open_findings(root, &changed);
    emit_censor_steer(root, &steer_findings);
```

**Replace with:**

```rust
    enqueue_censor_findings(root, &steer_findings, &changed, "coarse");
```

#### F2c — DELETE old tests, ADD new tests

**Delete:** `fn emit_censor_steer_writes_file` (line ~2156) and `fn emit_censor_steer_empty_skips` (line ~2173) — these test the old function.

**Add:**

```rust
#[test]
fn enqueue_censor_findings_creates_batch_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let f = vec![Finding {
        id: "abc".into(), severity: Severity::High, title: "test".into(),
        file: "src/a.rs".into(), source: "clippy".into(),
        category: Category::Correctness, ..Default::default()
    }];
    enqueue_censor_findings(root, &f, &["src/a.rs".into()], "coarse");
    // Batch file exists
    let queue_dir = root.join(".aspis").join("censor_queue").join("pending");
    let entries: Vec<_> = std::fs::read_dir(&queue_dir).unwrap()
        .filter_map(|e| e.ok()).collect();
    assert_eq!(entries.len(), 1, "one batch file");
    assert!(entries[0].path().to_str().unwrap().ends_with(".json"));
    // Steer cache also written
    assert!(root.join(".aspis").join("steer_censor").exists());
}

#[test]
fn enqueue_censor_findings_empty_skips() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    enqueue_censor_findings(root, &[], &[], "coarse");
    assert!(!root.join(".aspis").join("censor_queue").exists(),
            "no queue dir for empty findings");
    assert!(!root.join(".aspis").join("steer_censor").exists(),
            "no steer file for empty findings");
}

#[test]
fn enqueue_censor_findings_accumulates_multiple_passes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let f = vec![Finding {
        id: "abc".into(), severity: Severity::High, title: "test".into(),
        file: "src/a.rs".into(), source: "semgrep".into(),
        category: Category::Security, ..Default::default()
    }];
    let f2 = vec![Finding {
        id: "def".into(), severity: Severity::Medium, title: "test2".into(),
        file: "src/b.rs".into(), source: "zizmor".into(),
        category: Category::Correctness, ..Default::default()
    }];
    enqueue_censor_findings(root, &f, &["src/a.rs".into()], "coarse");
    std::thread::sleep(std::time::Duration::from_millis(10)); // unique timestamps
    enqueue_censor_findings(root, &f2, &["src/b.rs".into()], "coarse");
    let queue_dir = root.join(".aspis").join("censor_queue").join("pending");
    let count = std::fs::read_dir(&queue_dir).unwrap()
        .filter(|e| e.as_ref().map_or(false, |x| x.path().extension().map_or(false, |ext| ext == "json")))
        .count();
    assert_eq!(count, 2, "two distinct batch files, not overwritten");
}
```

**Check:** `cargo test enqueue_censor` → ✅ 3 pass

---

### F3 — `src-tauri/src/backend/censor/commands.rs`

**Where:** After `wait_for_censor_findings` (line ~957), before the `#[cfg(test)]` block.

**Add:**

```rust
/// Drain all pending Censor queue batches for a project root.
/// Reads every `<root>/.aspis/censor_queue/pending/*.json`, deletes
/// each file after reading (exactly-once delivery), and returns
/// deduplicated open findings sorted by severity (High first).
/// Returns empty vec if no queue directory or no batches.
pub fn drain_censor_queue(root: &Path) -> Vec<crate::backend::censor::schema::Finding> {
    let queue_dir = root.join(".aspis").join("censor_queue").join("pending");
    if !queue_dir.exists() {
        return vec![];
    }
    let mut batches: Vec<crate::backend::censor::schema::FindingBatch> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&queue_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "json") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(batch) = serde_json::from_str::<
                        crate::backend::censor::schema::FindingBatch
                    >(&content) {
                        batches.push(batch);
                    }
                }
                let _ = std::fs::remove_file(&path); // exactly-once delivery
            }
        }
    }
    batches.sort_by_key(|b| b.timestamp.clone());
    let mut seen = std::collections::HashSet::new();
    let mut findings = Vec::new();
    for batch in batches {
        for f in batch.findings {
            if seen.insert(f.id.clone()) {
                findings.push(f);
            }
        }
    }
    findings.sort_by_key(|f| severity_rank(f.severity));
    findings
}
```

**Tests to add** (in the existing `#[cfg(test)] mod tests` block):

```rust
#[test]
fn drain_censor_queue_returns_empty_when_no_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let got = drain_censor_queue(tmp.path());
    assert!(got.is_empty(), "no dir → empty");
}

#[test]
fn drain_censor_queue_drains_and_deletes_batches() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let queue_dir = root.join(".aspis").join("censor_queue").join("pending");
    std::fs::create_dir_all(&queue_dir).unwrap();

    let f1 = Finding {
        id: "id1".into(), severity: Severity::High, title: "bug1".into(),
        file: "src/a.rs".into(), source: "clippy".into(),
        category: Category::Correctness, ..Default::default()
    };
    let f2 = Finding {
        id: "id2".into(), severity: Severity::Medium, title: "bug2".into(),
        file: "src/b.rs".into(), source: "semgrep".into(),
        category: Category::Security, ..Default::default()
    };
    let batch = crate::backend::censor::schema::FindingBatch {
        batch_id: "2026-06-30T12:00:00_abcd".into(),
        timestamp: "2026-06-30T12:00:00Z".into(),
        pass_type: "coarse".into(),
        files: vec!["src/a.rs".into(), "src/b.rs".into()],
        findings: vec![f1, f2],
    };
    std::fs::write(
        queue_dir.join("2026-06-30T12:00:00_abcd.json"),
        serde_json::to_string(&batch).unwrap(),
    ).unwrap();

    let got = drain_censor_queue(root);
    assert_eq!(got.len(), 2, "both findings returned");
    assert_eq!(got[0].id, "id1", "High before Medium");
    // batch file deleted
    assert!(!queue_dir.join("2026-06-30T12:00:00_abcd.json").exists(),
            "file deleted after drain");
}

#[test]
fn drain_censor_queue_deduplicates_by_id() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let queue_dir = root.join(".aspis").join("censor_queue").join("pending");
    std::fs::create_dir_all(&queue_dir).unwrap();

    let f = Finding {
        id: "same_id".into(), severity: Severity::High, title: "dup".into(),
        file: "src/a.rs".into(), source: "clippy".into(),
        category: Category::Correctness, ..Default::default()
    };
    // Two batches with the same finding
    for ts in ["2026-06-30T12:00:00_aaaa", "2026-06-30T12:01:00_bbbb"] {
        let batch = crate::backend::censor::schema::FindingBatch {
            batch_id: ts.into(),
            timestamp: format!("{ts}Z"),
            pass_type: "coarse".into(),
            files: vec!["src/a.rs".into()],
            findings: vec![f.clone()],
        };
        std::fs::write(
            queue_dir.join(format!("{ts}.json")),
            serde_json::to_string(&batch).unwrap(),
        ).unwrap();
    }

    let got = drain_censor_queue(root);
    assert_eq!(got.len(), 1, "duplicate deduped to 1");
}
```

**Check:** `cargo test drain_censor_queue` → ✅ 3 pass

---

### F4 — `devboule-coder/src/executor.rs`

**Where:** `drain_steer()` at line 791.

**Current** (lines 791–815):

```rust
    fn drain_steer(&self) -> Vec<String> {
        let mut msgs = self.steer.drain();
        // Also drain Censor Phase B steer file
        let censor_steer = self.fs.root.join(".aspis").join("steer_censor");
        if censor_steer.exists() {
            if let Ok(content) = std::fs::read_to_string(&censor_steer) {
                if !content.trim().is_empty() {
                    for line in content.lines() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            msgs.push(trimmed.to_string());
                        }
                    }
                }
                // Delete after reading so findings aren't repeated
                let _ = std::fs::remove_file(&censor_steer);
            }
        }
        msgs
    }
```

**Replace with:**

```rust
    fn drain_steer(&self) -> Vec<String> {
        let mut msgs = self.steer.drain();
        // Drain Censor Phase B: immediate steer cache + persistent queue
        drain_censor_steer_file(&self.fs.root, &mut msgs);
        drain_censor_queue_files(&self.fs.root, &mut msgs);
        msgs
    }
```

**Add** two helper functions (same file, same `impl` block, after `drain_steer`):

```rust
/// Drain the immediate steer cache (overwritten each COARSE pass).
fn drain_censor_steer_file(root: &Path, msgs: &mut Vec<String>) {
    let path = root.join(".aspis").join("steer_censor");
    if !path.exists() { return; }
    if let Ok(content) = std::fs::read_to_string(&path) {
        let trimmed = content.trim().to_string();
        if !trimmed.is_empty() {
            msgs.push(trimmed);
        }
    }
    let _ = std::fs::remove_file(&path);
}

/// Drain the persistent queue (accumulated batches, never lost).
fn drain_censor_queue_files(root: &Path, msgs: &mut Vec<String>) {
    let queue_dir = root.join(".aspis").join("censor_queue").join("pending");
    if !queue_dir.exists() { return; }
    let mut files: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&queue_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().map_or(false, |e| e == "json") {
                files.push(p);
            }
        }
    }
    files.sort(); // oldest first (timestamped filenames are lexicographically ordered)
    for path in &files {
        if let Ok(content) = std::fs::read_to_string(path) {
            let trimmed = content.trim().to_string();
            if !trimmed.is_empty() {
                msgs.push(trimmed);
            }
        }
        let _ = std::fs::remove_file(path);
    }
}
```

**Check:** `cargo check -p devboule-coder` → ✅; `cargo test -p devboule-coder` → ✅

---

### F5 — `devboule-coder/src/prompt.rs`

**Where:** The Censor Feedback block, line ~279–288.

**Current:**

```
# Censor Feedback
After spawn_mini completes, you may receive Censor findings in the tool result — automated code review from 46 deterministic linters. Deep findings may arrive later as [steer] messages in your conversation.
```

**Replace with:**

```
# Censor Feedback

After **spawn_mini** completes you may see Censor findings in the tool result (fast checks).
Deep findings from slow linters and the optional LLM judge arrive asynchronously.
**Call `censor_findings(project_id, drain_queue=true)` at each step boundary to drain
pending deep findings.** The queue persists across sessions — findings accumulate and
wait for you to read them.

🔴 High → fix immediately (security/correctness)
🟡 Medium → fix on next pass
🟢 Low → note, continue if easy
Persistence: if the same finding survives 2 fix attempts, escalate.
```

**Check:** `cargo test -p devboule-coder prompt` → ✅

---

### F6 — `oracle/server/aspis_mcp.py`

#### F6a — ADD `drain_queue` parameter to tool schema

**Where:** Line ~949, the `censor_findings` tool definition.

**Current:**

```python
        "name": "censor_findings",
        "description": "Legge i finding APERTI di Censor ...",
        "parameters": {
            "project_id": {"type": "string"},
            "file": {"type": "string", "default": ""},
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "session_token": {"type": "string"},
        },
    },
```

**Replace with:**

```python
        "name": "censor_findings",
        "description": "Legge i finding APERTI di Censor ... Aggiungi drain_queue=true per svuotare anche la coda persistente dei finding asincroni (Phase B).",
        "parameters": {
            "project_id": {"type": "string"},
            "file": {"type": "string", "default": ""},
            "drain_queue": {"type": "boolean", "default": False},
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "session_token": {"type": "string"},
        },
    },
```

#### F6b — ADD `drain_censor_queue` Python function

**Where:** After `read_censor_open_findings` (line ~5138), before `_last_provenance_role`.

**Add:**

```python
def drain_censor_queue(root: Path) -> list[dict[str, Any]]:
    """Drain the persistent Censor queue directory. Reads every
    `<root>/.aspis/censor_queue/pending/*.json`, returns findings, deletes
    the files. Queue survives main-coder restarts. Cloud coders call this via
    `censor_findings(drain_queue=true)`."""
    queue_dir = root / ".aspis" / "censor_queue" / "pending"
    if not queue_dir.is_dir():
        return []
    findings: list[dict[str, Any]] = []
    try:
        entries = sorted(queue_dir.iterdir())
    except (FileNotFoundError, OSError):
        return []
    seen: set[str] = set()
    for entry in entries:
        if entry.suffix != ".json" or not entry.is_file():
            continue
        try:
            data = json.loads(entry.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, OSError):
            _safe_unlink(entry)
            continue
        batch_findings = data.get("findings")
        if isinstance(batch_findings, list):
            for f in batch_findings:
                if isinstance(f, dict) and str(f.get("disposition", "open")).lower() == "open":
                    fid = str(f.get("id", ""))
                    if fid and fid not in seen:
                        seen.add(fid)
                        findings.append(_safe_censor_finding(f))
        _safe_unlink(entry)
    return findings
```

#### F6c — MODIFY the `censor_findings` handler

**Where:** Line ~7826, inside the `if name == "censor_findings":` block.

**Current:**

```python
    if name == "censor_findings":
        agent_id, role = require_agent_tool(projects_path, args, name)
        project_id = normalize_project_id(str(args.get("project_id") or "").strip())
        if not project_id:
            raise McpError("project_id is required.")
        file_arg = str(args.get("file") or "").strip() or None
        if file_arg is not None:
            validate_censor_rel_path(file_arg)
        work_root = resolve_project_work_root(projects_path, project_id)
        findings = read_censor_open_findings(work_root, file_arg)
        audit_agent_read(
            projects_path, state_lock, agent_id, role, "censor_findings",
            f"Read {len(findings)} open Censor finding(s)"
            + (f" for {file_arg}" if file_arg else ""),
            project_id,
        )
        return {"projectId": project_id, "file": file_arg, "findings": findings}
```

**Replace with:**

```python
    if name == "censor_findings":
        agent_id, role = require_agent_tool(projects_path, args, name)
        project_id = normalize_project_id(str(args.get("project_id") or "").strip())
        if not project_id:
            raise McpError("project_id is required.")
        file_arg = str(args.get("file") or "").strip() or None
        if file_arg is not None:
            validate_censor_rel_path(file_arg)
        drain_queue = str(args.get("drain_queue") or "").strip().lower() in ("true", "1")
        work_root = resolve_project_work_root(projects_path, project_id)
        findings = read_censor_open_findings(work_root, file_arg)

        if drain_queue:
            queue_findings = drain_censor_queue(work_root)
            existing_ids = {f.get("id") for f in findings}
            for qf in queue_findings:
                if qf.get("id") not in existing_ids:
                    findings.append(qf)
                    existing_ids.add(qf.get("id"))

        audit_agent_read(
            projects_path, state_lock, agent_id, role, "censor_findings",
            f"Read {len(findings)} open Censor finding(s)"
            + (f" for {file_arg}" if file_arg else "")
            + (" (queue drained)" if drain_queue else ""),
            project_id,
        )
        return {"projectId": project_id, "file": file_arg,
                "findings": findings, "drainedQueue": drain_queue}
```

Check if `_safe_unlink` helper exists; if not, add:

```python
def _safe_unlink(path: Path) -> None:
    """Best-effort file deletion — never propagates errors."""
    try:
        path.unlink(missing_ok=True)
    except OSError:
        pass
```

#### F6d — UPDATE ROLE_RULES for cloud coders

**Where:** Line ~297, the coder ROLE_RULES that mentions `censor_findings`.

**Current:**

```python
"A ogni confine di step chiama censor_findings(project_id, file=<file toccati>) per i file che hai modificato.",
```

**Replace with:**

```python
"A ogni confine di step chiama censor_findings(project_id, file=<file toccati>, drain_queue=True) per i file che hai modificato. drain_queue svuota anche i finding asincroni (Phase B) accumulati nella coda persistente.",
```

**Check:** `python -m pytest oracle/tests/test_aspis_mcp.py -x -q` → ✅

---

## 4. What gets deleted

| Symbol | File | Line | Reason |
|--------|------|------|--------|
| `fn emit_censor_steer` | `orchestrator.rs` | 971–986 | Replaced by `enqueue_censor_findings` |
| `fn emit_censor_steer_writes_file` test | `orchestrator.rs` | 2156–2172 | Tests deleted function |
| `fn emit_censor_steer_empty_skips` test | `orchestrator.rs` | 2173–2179 | Tests deleted function |

**Nothing else is deleted.** The `steer_censor` cache file is still written (by `enqueue_censor_findings`), just as a secondary, not the source of truth.

---

## 5. Cross-file invariants

| Invariant | How enforced |
|-----------|-------------|
| Batch filenames are unique per pass | `ISO-timestamp + short-hash` — collision probability < 1/65536 per pass, separated by ≥4s debounce |
| Exactly-once delivery | File deleted immediately after drain; no `processed/` dir needed |
| No duplicate findings | Dedup by `finding.id` (SHA-256 of file+line+category+source+title) in both Rust and Python drain |
| Queue doesn't grow unbounded | Files are <4KB each. Even 1000 batches = 4MB. No rotation needed yet; documented as future concern. |
| Phase A unchanged | Zero lines touched in `mini_coder_executor.rs` or `finalize_finished_mini` |
| No Pigeon dependency | Pure filesystem queue. Pigeon stays default-off. |
| Local AND cloud consume identically | Same `.aspis/censor_queue/pending/` directory, different drain entry points |

---

## 6. Build & test checklist

```bash
# After F1
cargo test finding_batch                     # → 1 pass

# After F2
cargo test enqueue_censor                    # → 3 pass

# After F3
cargo test drain_censor_queue                # → 3 pass

# After F4
cargo check -p devboule-coder                # → 0 errors
cargo test -p devboule-coder                 # → all pass

# After F5
cargo test -p devboule-coder prompt          # → prompt tests pass

# After F6
python -m pytest oracle/tests/test_aspis_mcp.py -x -q  # → all pass

# Full suite
cargo test --lib                              # → 2680+ pass, 0 fail
```

---

## 7. References (all verified on disk)

| File | Key symbols |
|------|------------|
| `src-tauri/src/backend/censor/schema.rs` | `Finding` (line 118), `CensorShard` (line 192), `FindingBatch` (TO ADD) |
| `src-tauri/src/backend/censor/orchestrator.rs` | `emit_censor_steer` (line 971), `run_coarse_pass` (line 736), `run_fine_batch_inner` (line 551), `collect_open_findings` (line 965) |
| `src-tauri/src/backend/censor/commands.rs` | `format_findings_text` (line 883), `wait_for_censor_findings` (line 928), `drain_censor_queue` (TO ADD) |
| `devboule-coder/src/executor.rs` | `drain_steer` (line 791), `FsBackend.root` (line 117) |
| `devboule-coder/src/prompt.rs` | `build_system_prompt` (line 48), Censor block (line 279) |
| `oracle/server/aspis_mcp.py` | `censor_findings` schema (line 949), handler (line 7826), `read_censor_open_findings` (line 5088), ROLE_RULES coder (line 297) |
