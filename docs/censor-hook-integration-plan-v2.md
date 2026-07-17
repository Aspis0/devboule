# Censor → Devboule-Coder Hook Integration Plan (v2)

**Data:** 2026-06-30  
**Stato:** Bozza  
**Obiettivo:** Iniettare i findings del Censor nel contesto dell'agente (devboule-coder) dopo ogni write/edit, senza modificare il Censor core.  
**Scelta architetturale:** Opzione D — Hook interno a devboule-coder (in Rust), usando `motosan_agent_loop` come pattern reference.

---

## 1. Problema

Il Censor (Rust/Tauri backend) lancia 35+ runner deterministici + opzionale Gemma LLM su ogni file modificato. I findings vengono scritti in shard JSON su disco (`.aspis-censor/`). **Ma l'agente (devboule-coder) non li vede mai.**

I findings esistono come "post-it appesi al muro" — il Censor fa il suo lavoro, ma il feedback non ritorna al coder.

### Flusso attuale (broken)

```
Agente scrive file (write/edit)
  → risultato tool call → feed back all'agente
  → Censor watcher rileva modifica → lancia runner → scrive shard
  → [NILL] L'agente non vede mai gli shard
  → verdict gate Rust (spawn_verdict_thread + condvar) → requeue se necessario
```

### Flusso target

```
Agente scrive file (write/edit)
  → risultato tool call → feed back all'agente
  → Censor watcher rileva modifica → lancia runner → scrive shard
  → devboule-coder legge shard Censor → inietta findings nel contesto
  → Agente turno N+1 vede i findings → decide: fix, ignore, o escalate
  → [RIMOSSO] verdict gate Rust (bloccante → rimpiazzato da feedback loop agent-driven)
```

---

## 2. Architettura Esistente (As-Is)

### 2.1 Devboule-Coder — Struttura

```
devboule-coder/
├── src/
│   ├── main.rs              ← REPL/TUI, main loop, --once mode, session orchestrator
│   ├── agent_loop.rs        ← run_burst() — il bounded tool-burst loop (L2.2)
│   ├── executor.rs          ← RealExecutor, ToolExecutor trait, FsBackend, ExaBackend (L2.3)
│   ├── config.rs            ← build_runtime() — model + executor resolution
│   ├── action.rs            ← AgentAction enum + parser
│   ├── model.rs             ← CoderModel trait (next_output)
│   ├── model_client.rs      ← OmlxModel, CloudModel, MockModel
│   ├── rmcp_backend.rs      ← RmcpBackend — MCP transport (rmcp SDK)
│   ├── multi_mcp.rs         ← MultiMcpBackend — Oracle + user servers
│   ├── planner.rs           ← run_planner() — LOCAL planning
│   ├── runner.rs            ← run_tasks() — DAG runner per approved plans
│   ├── reply_stream.rs      ← ReplyStreamExtractor — streaming output parsing
│   ├── prompt.rs            ← build_system_prompt() — system message assembly
│   ├── action.rs            ← AgentAction enum + parsing
│   ├── app.rs               ← TUI app state
│   ├── terminal.rs          ← TerminalGuard
│   ├── conversation.rs      ← Conversation state
│   ├── activity.rs          ← Activity bridge (Console milestones)
│   ├── steer.rs             ← Steer inbox (app → running orchestrator)
│   ├── doubt_sensor.rs      ← Doubt sensor (decision confidence)
│   ├── skills.rs            ← Skill loader
│   └── config.rs            ← Runtime config, env vars
├── Cargo.toml               ← Dependencies: rmcp, tokio, ratatui, async-trait, reqwest
└── ...
```

### 2.2 Devboule-Coder — Il Burst Loop (`run_burst`)

**File:** `devboule-coder/src/agent_loop.rs`  
**Linee:** ~460-620 (funzione `run_burst`)

Il burst loop è il cuore dell'agente. Ogni iterazione:

1. **Check wall-clock cap** (line ~470)
2. **Drain steer messages** (line ~480) — inietta messaggi live come user turns
3. **Call model** per il prossimo output (line ~495)
4. **Parse action** dal output (line ~510)
5. **Handle terminal actions** (Done/AskUser/Escalate) — terminano il burst
6. **Check no-progress guard** (line ~560) — cicli su (tool, target)
7. **Dispatch action** all'executor (line ~620)
8. **Cap result** e push nel transcript (line ~640)
9. **Increment rounds**, check round cap

```rust
pub async fn run_burst(
    human_msg: String,
    model: &dyn CoderModel,
    executor: &dyn ToolExecutor,
    clock: &dyn Clock,
    budget: Duration,
    allow_egress: bool,
    progress_tx: &mpsc::Sender<String>,
) -> BurstOutcome {
    let mut transcript = Transcript::new(human_msg);
    let mut rounds: usize = 0;
    // ...

    loop {
        // 1. Wall-clock check
        if clock.elapsed() >= budget {
            return BurstOutcome::Escalated("time cap reached".to_string());
        }

        // 2. Drain steer messages (live human input)
        for msg in executor.drain_steer() {
            transcript.push_human(msg);
        }

        // 3. Call model
        let (raw, ...) = model.next_output_streaming_logprobs(&transcript, &mut |...| {}).await;

        // 4. Parse action
        match parse_action_with_servers(&raw, executor.known_mcp_servers()) {
            Err(fe) => { /* format error handling */ }
            Ok(action) => {
                // 5. Terminal actions → return
                match &action {
                    AgentAction::Done { reply } => return BurstOutcome::Done(reply.clone()),
                    AgentAction::AskUser { question, .. } => return BurstOutcome::AskUser(question.clone()),
                    AgentAction::Escalate { reason } => return BurstOutcome::Escalated(reason.clone()),
                    _ => {} // non-terminal → dispatch
                }

                // 6. No-progress guard
                let this = (action.tool_name().to_string(), action.target());
                if executed_window.contains(&this) { ... }

                // 7. Dispatch action
                let result = executor.execute(&action).await;

                // 8. Push to transcript
                transcript.push(TranscriptEntry::Action(action));
                transcript.push(TranscriptEntry::Result(result));

                // 9. Increment rounds
                rounds += 1;
                if rounds >= MAX_ROUNDS {
                    return BurstOutcome::Escalated("round cap reached".to_string());
                }
            }
        }
    }
}
```

### 2.3 ToolExecutor Trait

**File:** `devboule-coder/src/agent_loop.rs`  
**Linee:** ~90-180

Il trait è la seam tra il burst loop e i backend. Ogni metodo ha un default no-op.

```rust
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(&self, action: &AgentAction) -> ToolResult;

    fn known_mcp_servers(&self) -> &[String] { &[] }
    fn drain_steer(&self) -> Vec<String> { Vec::new() }
    fn emit_chat(&self, _role: &str, _text: &str) {}
    fn activity_handle(&self) -> crate::activity::Activity { crate::activity::Activity::disabled() }
    fn is_orchestrator(&self) -> bool { false }
}
```

### 2.4 RealExecutor

**File:** `devboule-coder/src/executor.rs`  
**Linee:** ~900-1100

Dispatch actions a tre backend:

- **MCP backend** → Oracle tools (oracle_ask, oracle_context, spawn_mini_coder, plan_submit, project_*, **censor_findings**)
- **FS backend** → read, grep, glob (in-process, root-confined)
- **Exa backend** → fetch, websearch (egress, gated)

```rust
pub struct RealExecutor {
    mcp: Arc<dyn McpBackend>,
    fs: FsBackend,
    web: Option<ExaBackend>,
    model: Option<Arc<dyn CoderModel>>,
    project_id: String,
    activity: Activity,
    auto_create: bool,
    steer: Steer,
    is_orchestrator: bool,
}

impl ToolExecutor for RealExecutor {
    async fn execute(&self, action: &AgentAction) -> ToolResult {
        match action {
            AgentAction::OracleAsk { query } => self.mcp_call("oracle_ask", json!({ "query": query })).await,
            AgentAction::SpawnMini { task, files, write } => self.mcp_call("spawn_mini_coder", json!({ ... })).await,
            AgentAction::Read { path } => { /* FS backend */ },
            AgentAction::Grep { pattern, glob } => { /* FS backend */ },
            // ... etc
        }
    }
}
```

### 2.5 MCP Backend

**File:** `devboule-coder/src/rmcp_backend.rs` + `src/multi_mcp.rs`

```rust
#[async_trait]
pub trait McpBackend: Send + Sync {
    async fn call_tool(&self, name: &str, params: Value) -> Result<String, String>;
    async fn call_user_tool(&self, server: &str, tool: &str, params: Value) -> Result<String, String> {
        Err(format!("no user MCP servers configured"))
    }
    fn user_server_names(&self) -> &[String] { &[] }
}
```

### 2.6 Censor — Schema degli Shard

**File:** `src-tauri/src/backend/censor/schema.rs`

```rust
pub struct Finding {
    pub id: String,          // SHA-256 di (file, line, category, source, title)
    pub file: String,        // path relativo
    pub content_hash: String, // SHA-256 del contenuto del file
    pub line: Option<u32>,   // 1-based, None per finding file-level
    pub severity: Severity,  // High, Medium, Low (default: Medium)
    pub category: Category,  // Security, Correctness, Complexity, Duplication, DeadCode, Style
    pub source: String,      // nome del runner (clippy, eslint, semgrep, gemma, ...)
    pub title: String,       // titolo del finding
    pub body: String,        // descrizione in inglese
    pub verdict: Verdict,    // Suspected, Confirmed
    pub disposition: Disposition, // Open, Fixed, Fp, Wontfix
    pub provenance: Vec<ProvenanceEntry>,
    pub created_at: String,
}

pub struct CensorShard {
    pub file_rel_path: String,
    pub content_hash: String,
    pub updated_at: String,
    pub findings: Vec<Finding>,
}
```

### 2.7 Censor — MCP Command `censor_findings`

**File:** `src-tauri/src/backend/censor/commands.rs`  
**Linee:** ~380-410

```rust
#[tauri::command]
pub fn censor_get_findings(
    root: String,
    file: Option<String>,
    app: AppHandle,
    backend_state: State<'_, BackendState>,
) -> Result<Vec<Finding>, String>
```

Questo tool MCP è già registrato nell'Oracle MCP server. Il mini-coder può chiamarlo via `mcp_call("censor_findings", { "root": ..., "file": ... })`.

### 2.8 Mini-Coder Executor — Verdict Gate (attuale)

**File:** `src-tauri/src/backend/mini_coder_executor.rs`  
**Linee:** ~1739-2100

```rust
fn finalize_finished_mini(app: &AppHandle, directive: &MiniCoderDirective) {
    // ... outcome computation, sandbox broker, write diffs ...

    // The gate (linters) is needed ONLY for a clean, un-killed `done` on a TRUSTED tree.
    let needs_gate = outcome.status == MiniCoderStatus::Done
        && !directive.kill_requested
        && trusted;

    if needs_gate {
        if let Some(state) = app.try_state::<MiniCoderState>() {
            if state.claim_verdict(&directive.id) {
                spawn_verdict_thread(app.clone(), directive.clone(), project_id, outcome, write_diffs);
                return;  // ← IL VERDICT GATE BLOCCA QUI
            }
            return;
        }
        // Inline finalize with real collector
        finalize_finished_mini_with(app, directive, outcome, trusted, |root, files| {
            real_censor_verdict(app, pid.as_deref(), root, files, &stop)
        }, write_diffs);
        return;
    }

    // Inline (no linters): untrusted / non-done / killed / no-project
    finalize_finished_mini_with(app, directive, outcome, trusted, |_root, _files| Vec::new(), write_diffs);
}
```

Il `spawn_verdict_thread` lancia un thread dedicato che:

1. Esegue i runner Censor (5-30s)
2. Colleziona i findings
3. Applica la decisione (retry/escalate/stamp-terminal)
4. Clear l'in-flight guard

### 2.9 Mini-Coder Status Enum

**File:** `src-tauri/src/backend/mini_coder.rs`

```rust
pub enum MiniCoderStatus {
    Pending,
    Running,
    Censoring,    // ← STATUS CHE VIENE RIMOSSO
    Done,
    Failed,
}

pub struct EscalationFinding { ... }  // ← TIPO CHE VIENE RIMOSSO
```

---

## 3. Architettura Target (To-Be)

### 3.1 Flusso Completo

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         Devboule                                 │
│                                                                          │
│  ┌──────────────────┐    ┌──────────────────┐    ┌──────────────────┐   │
│  │  Devboule-Coder   │    │    Censor        │    │  Oracle MCP      │   │
│  │  (Agent Loop)     │    │  (Rust/Tauri)    │    │  Server          │   │
│  │                  │    │                  │    │                  │   │
│  │  1. write file   │    │  2. File changed │    │                  │   │
│  │  2. tool_result  │◄───│  3. Runner runs  │    │                  │   │
│  │  3. censor_find- │    │  4. Shard written│    │ 7. censor_find-  │   │
│  │     ings()       │    │  5. Event→UI     │    │     ings()       │   │
│  │  4. Injeta       │    │                  │    │ 8. Restituisce   │   │
│  │     findings     │    │                  │    │     findings     │   │
│  │  5. Model vede   │    │                  │    │                  │   │
│  │     findings     │    │                  │    │                  │   │
│  └──────────────────┘    └──────────────────┘    └──────────────────┘   │
│         ▲                              ▲                ▲                │
│         │                              │                │                │
│         └────────── Agent Context ─────┴────────────────┘                │
│                                                                          │
│  [RIMOSSO] verdict gate Rust (spawn_verdict_thread + condvar)            │
│  [NUOVO]   Prompt template con regole Censor feedback loop               │
└─────────────────────────────────────────────────────────────────────────┘
```

### 3.2 Cosa NON Cambia

- **Censor watcher** — continua a monitorare filesystem, lanciare runner, scrivere shard
- **Gemma LLM** — continua async/offloaded, scrive findings nello shard
- **Schema degli shard** — rimane lo stesso (Finding struct, CensorShard)
- **`censor://findings-updated` event** — UI frontend continua a riceverlo
- **MCP `censor_findings`** — tool Oracle MCP già esistente, non cambia
- **FS backend** — read/grep/glob restano in-process, root-confined
- **Exa backend** — fetch/websearch restano egress, gated
- **Agent loop core** — stop conditions (round cap, time cap, format errors, no-progress) restano

### 3.3 Cosa Cambia

- **`ToolExecutor` trait** — aggiunto metodo `censor_findings()`
- **`Transcript`** — aggiunto entry `CensorFindings`
- **`run_burst`** — dopo write/edit, chiama `censor_findings()` e inietta findings
- **`RealExecutor`** — implementa `censor_findings()` → chiama MCP `censor_findings`
- **`StubExecutor`** — implementa `censor_findings()` → ritorna []
- **`TranscriptEntry`** — aggiunto variant `CensorFindings(String)`
- **Prompt template** — aggiunta sezione Censor feedback loop nel system prompt
- **Verdict gate Rust** — semplificato (rimosso `spawn_verdict_thread` per il mini-coder)

---

## 4. Design Tecnico Dettagliato

### 4.1 Phase 1: Aggiungere `censor_findings` a `ToolExecutor`

**File:** `devboule-coder/src/agent_loop.rs`  
**Linea target:** ~136 (dopo `is_orchestrator`)

#### 4.1.1 Nuovo metodo nel trait

Aggiungere al trait `ToolExecutor` (dopo `is_orchestrator`, ~linea 136):

```rust
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    // ... existing methods ...

    /// Retrieve Censor findings for files modified in the current burst.
    /// Returns a formatted text summary of all open findings (disposition=Open)
    /// for files that were written or edited during this burst.
    /// Default is EMPTY (no-op): only RealExecutor with MCP backend implements it.
    async fn censor_findings(&self, files: &[String]) -> String {
        // Default: no-op. RealExecutor overrides to call MCP.
        String::new()
    }
}
```

**Perché `&[String]` e non `&dyn ToolExecutor`:** Il burst loop passa i file modificati come slice di stringhe. Il metodo riceve i file da controllare e ritorna un testo formattato.

**Perché ritorna `String` e non `Vec<Finding>`:** Il burst loop non ha i tipi del Censor (sono in src-tauri). Ritorna un testo formattato che viene iniettato nel transcript come messaggio user.

#### 4.1.2 Default no-op

Il default `String::new()` garantisce che:

- **StubExecutor** non cambia comportamento (ritorna "")
- **Test unitari** non devono essere aggiornati
- **Evoluzione futura** — altri executor possono aggiungere il proprio hook

### 4.2 Phase 2: Implementare in `RealExecutor`

**File:** `devboule-coder/src/executor.rs`  
**Linea target:** ~1050 (dentro l'impl `ToolExecutor for RealExecutor`)

#### 4.2.1 Implementazione

Aggiungere dopo `is_orchestrator`:

```rust
impl RealExecutor {
    /// Call the Oracle MCP `censor_findings` tool for the given files.
    /// Returns a formatted text summary of open findings, or empty string on error.
    pub async fn censor_findings(&self, files: &[String]) -> String {
        if files.is_empty() {
            return String::new();
        }

        // Build params: root + list of files
        let params = json!({
            "root": self.fs.root.to_string_lossy().to_string(),
            "files": files,
        });

        match self.mcp.call_tool("censor_findings", params).await {
            Ok(text) => {
                // The MCP tool returns JSON array of findings.
                // Format as a human-readable summary for the model.
                format_censor_findings_summary(&text)
            }
            Err(e) => {
                // On error (MCP not available, tool not found, etc.), return empty.
                // This is non-fatal: the agent can continue without Censor findings.
                eprintln!("devboule: censor_findings failed: {e}");
                String::new()
            }
        }
    }
}
```

#### 4.2.2 Helper: Formattazione `format_censor_findings_summary`

Funzione pura (unit-testable) che trasforma il JSON dei findings in testo leggibile:

```rust
/// Format Censor findings JSON into a human-readable summary for the agent.
///
/// Input: JSON array of Finding objects (from MCP `censor_findings` tool).
/// Output: Formatted text with severity, file, line, category, title, body.
///
/// Token budgeting:
///   - Max 10 findings per file
///   - Max 50 total findings
///   - Max 4096 bytes per summary
///   - Priority: High > Medium > Low
///   - Only Open findings (skip Fixed/Fp/Wontfix)
pub fn format_censor_findings_summary(raw_json: &str) -> String {
    // Parse JSON array of findings
    let findings: Vec<serde_json::Value> = match serde_json::from_str(raw_json) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("censor_findings: parse error: {e}");
            return String::new();
        }
    };

    if findings.is_empty() {
        return String::new();
    }

    // Filter: only Open findings
    let open: Vec<&serde_json::Value> = findings
        .iter()
        .filter(|f| {
            f.get("disposition")
                .and_then(|d| d.as_str())
                .map(|d| d == "open")
                .unwrap_or(false)
        })
        .collect();

    if open.is_empty() {
        return String::new();
    }

    // Sort by severity: High first, then Medium, then Low
    let mut sorted = open;
    sorted.sort_by(|a, b| {
        let sev_a = severity_rank(a);
        let sev_b = severity_rank(b);
        sev_b.cmp(&sev_a) // descending: High=0 first
    });

    // Cap: max 50 findings, max 10 per file
    let capped = cap_findings(&sorted);

    // Format as text
    format_findings_text(&capped)
}

fn severity_rank(f: &serde_json::Value) -> u32 {
    f.get("severity")
        .and_then(|s| s.as_str())
        .and_then(|s| match s {
            "high" => Some(0),
            "medium" => Some(1),
            "low" => Some(2),
            _ => None,
        })
        .unwrap_or(3)
}

fn cap_findings(findings: &[&serde_json::Value]) -> Vec<&serde_json::Value> {
    // Max 10 per file, max 50 total
    let mut by_file: std::collections::HashMap<String, Vec<&serde_json::Value>> =
        std::collections::HashMap::new();
    for f in findings {
        let file = f.get("file").and_then(|s| s.as_str()).unwrap_or("unknown");
        by_file.entry(file.to_string()).or_default().push(*f);
    }

    let mut result = Vec::new();
    for (_, mut file_findings) in by_file {
        let take = file_findings.len().min(10);
        result.extend(file_findings.drain(..take));
    }

    let total = result.len().min(50);
    result.drain(total..);
    result
}

fn format_findings_text(findings: &[&serde_json::Value]) -> String {
    let mut out = String::new();
    out.push_str("=== [Censor Findings] ===\n\n");

    for (i, f) in findings.iter().enumerate() {
        let severity = f.get("severity").and_then(|s| s.as_str()).unwrap_or("unknown");
        let category = f.get("category").and_then(|s| s.as_str()).unwrap_or("unknown");
        let source = f.get("source").and_then(|s| s.as_str()).unwrap_or("unknown");
        let file = f.get("file").and_then(|s| s.as_str()).unwrap_or("unknown");
        let line = f.get("line").and_then(|l| l.as_u64());
        let title = f.get("title").and_then(|t| t.as_str()).unwrap_or("");
        let body = f.get("body").and_then(|b| b.as_str()).unwrap_or("");
        let verdict = f.get("verdict").and_then(|v| v.as_str()).unwrap_or("suspected");

        let icon = match severity {
            "high" => "🔴",
            "medium" => "🟡",
            "low" => "🟢",
            _ => "⚪",
        };

        let line_str = match line {
            Some(n) => format!(":{n}"),
            None => String::new(),
        };

        out.push_str(&format!(
            "{icon} [{severity}] {title}\n\
             File: {file}{line_str}\n\
             Source: {source} ({category})\n\
             Verdict: {verdict}\n\
             {body}\n\n"
        ));
    }

    out.push_str("=== [End Censor Findings] ===\n");
    out
}
```

#### 4.2.3 StubExecutor: default no-op

Il default del trait è `String::new()`, quindi `StubExecutor` non ha bisogno di override. Ma per chiarezza, possiamo aggiungere un commento:

```rust
#[async_trait]
impl ToolExecutor for StubExecutor {
    async fn execute(&self, action: &AgentAction) -> ToolResult {
        // ... existing code ...
    }
    // censor_findings: uses trait default (returns empty string)
}
```

### 4.3 Phase 3: Integrare nel Burst Loop

**File:** `devboule-coder/src/agent_loop.rs`  
**Linea target:** ~640 (dopo `transcript.push(TranscriptEntry::Result(result))`)

#### 4.3.1 Modifica a `TranscriptEntry`

Aggiungere un nuovo variant:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptEntry {
    Action(AgentAction),
    Result(ToolResult),
    FormatFeedback(String),
    Human(String),
    /// Censor findings injected after write/edit actions.
    /// Contains the formatted text summary from `censor_findings()`.
    CensorFindings(String),
}
```

#### 4.3.2 Modifica a `Transcript`

Aggiungere un metodo push:

```rust
impl Transcript {
    // ... existing methods ...

    /// Inject Censor findings as a transcript entry.
    /// These are appended after the tool result and before the next action.
    pub fn push_censor_findings(&mut self, findings: String) {
        if !findings.is_empty() {
            self.entries.push(TranscriptEntry::CensorFindings(findings));
        }
    }
}
```

#### 4.3.3 Modifica a `run_burst`

Dopo il dispatch e il push del risultato, controllare se l'azione è un write/edit e iniettare i findings:

```rust
// OLD CODE (line ~640):
transcript.push(TranscriptEntry::Action(action));
transcript.push(TranscriptEntry::Result(result));

// NEW CODE:
transcript.push(TranscriptEntry::Action(action.clone()));
transcript.push(TranscriptEntry::Result(result.clone()));

// NEW: Check for Censor findings after write/edit actions
let censor_files = match &action {
    AgentAction::Read { path } => vec![path.clone()],
    AgentAction::Grep { pattern, glob } => {
        // grep doesn't modify files, skip
        Vec::new()
    }
    _ => Vec::new(),
};

if !censor_files.is_empty() {
    let findings = executor.censor_findings(&censor_files).await;
    if !findings.is_empty() {
        transcript.push_censor_findings(findings);
    }
}
```

**ATTENZIONE:** In realtà, `AgentAction` non ha variant `write` o `edit`. I file sono modificati solo tramite `spawn_mini_coder`. Quindi il controllo Censor nel burst loop del MAIN coder è limitato.

**RIVALUTAZIONE:** Il burst loop del main coder (orchestrator) non scrive file direttamente — tutto il write passa attraverso `spawn_mini_coder`. Quindi l'iniezione Censor findings nel burst loop del main coder serve solo per:

1. `Read` actions (per controllare se il file letto ha findings)
2. Eventuali future variant di write

**PER IL MINI-CODER:** Il mini-coder è un processo separato (Tauri backend → devboule-coder binary). Il mini-coder ha il SUO burst loop. Ma il mini-coder NON ha accesso all'MCP `censor_findings` perché:

- Il mini-coder è lanciato come child process
- Ha il SUO MCP connection (stesso Oracle server)
- Quindi PUÒ chiamare `censor_findings` via MCP

**QUINDI:** L'iniezione Censor findings nel burst loop del MAIN coder è utile per:

1. Leggere file modificati dal mini-coder (via `Read` action)
2. Dare all'orchestrator visibilità sui findings

Ma il vero valore è nel MINI-CODER, dove i file vengono effettivamente modificati.

**SOLUZIONE:** Implementare `censor_findings` nel burst loop del MAIN coder (per completezza), ma documentare che il mini-coder (processo separato) può chiamare `censor_findings` in modo simile.

#### 4.3.4 Implementazione Corretta per `run_burst`

```rust
// After pushing the action result (line ~640):
transcript.push(TranscriptEntry::Action(action.clone()));
transcript.push(TranscriptEntry::Result(result.clone()));

// Censor feedback loop: after any file-modifying action, check for findings.
// For the main coder, file modifications go through spawn_mini_coder,
// but we also check Read actions so the orchestrator can see findings on
// files it reads.
let censor_files: Vec<String> = match &action {
    AgentAction::Read { path } => {
        vec![path.clone()]
    }
    AgentAction::Grep { pattern, glob: _ } => {
        // grep doesn't modify files, but we can check the target directory
        Vec::new()
    }
    _ => Vec::new(),
};

if !censor_files.is_empty() {
    let findings = executor.censor_findings(&censor_files).await;
    if !findings.is_empty() {
        transcript.push_censor_findings(findings);
    }
}
```

### 4.4 Phase 4: Prompt Template

**File:** `devboule-coder/src/prompt.rs`  
**Linea target:** ~50 (dentro `build_system_prompt`)

Aggiungere la sezione Censor feedback loop al system prompt:

```rust
pub fn build_system_prompt(plan_first: bool) -> String {
    let base = r#"You are Devboule, a coding assistant. You solve tasks by:
1. Understanding the codebase (read, grep, glob)
2. Making targeted changes (spawn_mini_coder for file modifications)
3. Verifying your changes (read back, run tests)
4. Communicating your results (done, ask_user, escalate)

# Rules:
- Use read_file/list_dir/grep to UNDERSTAND before you change anything.
- Stay strictly inside the task's scope.
- Make MINIMAL, targeted edits.
- When the task is complete, STOP and reply with a summary.
- If you cannot proceed, say so plainly.
"#;

    // NEW: Censor feedback loop rules
    let censor_rules = r#"
# Censor Feedback Loop

After every file read or modification, you may receive Censor findings.
These are automated code review results from 35+ deterministic linters.

**How to handle Censor findings:**

1. 🔴 **High severity**: Fix immediately. These are correctness or security issues.
2. 🟡 **Medium severity**: Fix if you have time. These are potential issues.
3. 🟢 **Low severity**: Optional. Fix if easy, otherwise note and continue.
4. If the same finding persists after 3 fix attempts → escalate with details.
5. Findings with verdict=Confirmed are more reliable than Suspected.
6. If a finding seems like a false positive, note it and continue.

**Important:** Censor findings appear in your conversation history as
"[Censor Findings]" messages. Always check for them after reading or
modifying files.
"#;

    let plan_first = if plan_first {
        "\n\n# Plan First: For non-trivial tasks, create a plan before implementing."
    } else {
        ""
    };

    format!("{}\n{}\n{}", base, censor_rules, plan_first)
}
```

### 4.5 Phase 5: Verdict Gate Simplification (Rust Backend)

**File:** `src-tauri/src/backend/mini_coder_executor.rs`  
**Linea target:** ~2200-2280 (dentro `finalize_finished_mini`)

#### 4.5.1 Rimuovere `spawn_verdict_thread`

```rust
// OLD CODE (line ~2260):
if needs_gate {
    if let Some(state) = app.try_state::<MiniCoderState>() {
        if state.claim_verdict(&directive.id) {
            spawn_verdict_thread(app.clone(), directive.clone(), project_id, outcome, write_diffs);
            return;
        }
        return;
    }
    let pid = project_id.clone();
    let stop = AtomicBool::new(true);
    finalize_finished_mini_with(app, directive, outcome, trusted, |root, files| {
        real_censor_verdict(app, pid.as_deref(), root, files, &stop)
    }, write_diffs);
    return;
}

// NEW CODE:
// The verdict gate is simplified: we still run the Censor linters,
// but we DON'T block the executor waiting for results.
// The findings are available for the agent via MCP `censor_findings`.
// The agent decides whether to retry based on the findings.
if needs_gate {
    // Run linters in background (fire-and-forget).
    // The agent can check findings via MCP after the mini completes.
    let _ = spawn_verdict_thread(app.clone(), directive.clone(), project_id, outcome, write_diffs);
    // Still finalize — the agent loop now handles retry via prompt template.
}
```

**ATTENZIONE:** La rimozione completa del `spawn_verdict_thread` è un cambiamento significativo. Per la v1, manteniamo il thread ma semplifichiamo il comportamento:

- Il thread continua a eseguire i runner Censor
- Il risultato viene scritto nello shard (come prima)
- MA il verdict gate non blocca più lo scheduling del mini-coder
- Il mini-coder è finalized immediatamente
- L'agente legge i findings via MCP nel prossimo turno

#### 4.5.2 Rimuovere `Censoring` Status

**File:** `src-tauri/src/backend/mini_coder.rs`

```rust
// RIMUOVERE:
enum MiniCoderStatus {
    Pending,
    Running,
    Censoring,  // ← RIMUOVERE
    Done,
    Failed,
}

// RIMUOVERE:
struct EscalationFinding { ... }  // ← RIMUOVERE
fn summarize_findings_for_feedback(...) { ... }  // ← RIMUOVERE
```

---

## 5. Riferimenti: Pattern Esistenti

### 5.1 Pi-Lens (pi-coding-agent extension)

**Pattern:** Hook `tool_result` → intercetta write/edit/bash → risolve file → lancia checks → **appende risultati al tool result content**

**File chiave:**

- `pi-lens` hook system (TypeScript)
- `tool_result` hook → append findings al risultato

**Limitazione:** Pi-lens è TypeScript/Node.js. Aspis è Rust/Tauri. Non possiamo riutilizzare pi-lens direttamente.

### 5.2 Zeph — LSP Context Injection

**Pattern:** Hook automatico dopo `write_file`: fetch diagnostics → inject come `[lsp ...]` message

**Limitazione:** Zeph è un progetto separato, non un library riutilizzabile.

### 5.3 Claude Code — Stop Hooks

**Pattern:** Quando il modello dice "ho finito" → stop hooks valutano se è davvero finito → iniettano blocking errors

**Limitazione:** Claude Code hooks sono lato client (Claude Code / Cursor), non applicabili a devboule-coder.

### 5.4 Cargo-Context — MCP Server

**Pattern:** MCP server (`cargo-context-mcp`) esporta tools: `build_context_pack`, `get_last_error`, `get_diff`

**Rilevanza:** Il nostro Oracle MCP server HA GIA' `censor_findings`. Questo è il pattern corretto.

### 5.5 Motosan Agent Loop — LoopInterceptor

**Pattern:** Hook system in Rust con `rewrite_tool_result()` e `after_tool_result()` per iniettare feedback

**Rilevanza:** Questo è il pattern più vicino a ciò che vogliamo. Il nostro approccio è simile ma più semplice: invece di un interceptor completo, aggiungiamo un metodo al trait `ToolExecutor`.

---

## 6. Implementazione: Fasi

### Phase 1: `ToolExecutor` trait — aggiungere `censor_findings()`

**File:** `devboule-coder/src/agent_loop.rs`  
**Linea:** ~136 (dopo `is_orchestrator`)

- [ ] Aggiungere metodo `censor_findings(&self, files: &[String]) -> String` al trait `ToolExecutor`
- [ ] Default implementation: `String::new()` (no-op)
- [ ] Aggiungere `TranscriptEntry::CensorFindings(String)`
- [ ] Aggiungere `Transcript::push_censor_findings()`
- [ ] Test: verificare che il default ritorni ""

### Phase 2: `RealExecutor` — implementare `censor_findings()`

**File:** `devboule-coder/src/executor.rs`  
**Linea:** ~1050 (dentro `impl ToolExecutor for RealExecutor`)

- [ ] Implementare `censor_findings()` → chiama `self.mcp.call_tool("censor_findings", params)`
- [ ] Implementare `format_censor_findings_summary()` — funzione pura
- [ ] Implementare `severity_rank()`, `cap_findings()`, `format_findings_text()` — helper
- [ ] Test: verificare che i findings siano formattati correttamente
- [ ] Test: verificare il token budgeting (max 10/file, max 50 totali, max 4096 bytes)

### Phase 3: Integrare nel Burst Loop

**File:** `devboule-coder/src/agent_loop.rs`  
**Linea:** ~640 (dopo `transcript.push(TranscriptEntry::Result(result))`)

- [ ] Dopo ogni action dispatch, controllare se è un write/edit
- [ ] Se sì, chiamare `executor.censor_findings(&files).await`
- [ ] Iniettare findings nel transcript con `transcript.push_censor_findings(findings)`
- [ ] Test: verificare che i findings appaiano nel transcript

### Phase 4: Prompt Template

**File:** `devboule-coder/src/prompt.rs`  
**Linea:** ~50 (dentro `build_system_prompt`)

- [ ] Aggiungere sezione "Censor Feedback Loop" al system prompt
- [ ] Regole: High=🔴 fix, Medium=🟡 fix if time, Low=🟢 optional, 3 retry→escalate
- [ ] Test: verificare che il prompt contenga le regole Censor

### Phase 5: Verdict Gate Simplification

**File:** `src-tauri/src/backend/mini_coder_executor.rs`  
**Linea:** ~2260 (dentro `finalize_finished_mini`)

- [ ] Rimuovere `spawn_verdict_thread` call (o renderlo fire-and-forget)
- [ ] Rimuovere `Censoring` status da `MiniCoderStatus`
- [ ] Rimuovere `EscalationFinding` type
- [ ] Semplificare `finalize_finished_mini` — no più Censoring state
- [ ] Test: verificare che il mini-coder funzioni senza verdict gate

### Phase 6: Testing End-to-End

- [ ] Scenario 1: Agente legge file con findings → Censor findings appaiono nel transcript
- [ ] Scenario 2: Agente modifica file → Censor rileva → findings iniettati
- [ ] Scenario 3: Multi-file read → findings per ogni file
- [ ] Scenario 4: Censor non trusted → nessun finding → comportamento normale
- [ ] Scenario 5: Token budgeting → findings troncati correttamente

---

## 7. Rischi e Mitigazioni

### Rischio 1: Context Bloat

**Problema:** Troppe findings iniettate nel contesto → LLM confuso o budget token esaurito.

**Mitigazione:**

- Token budgeting: max 10 findings per file, max 50 totali, max 4096 bytes
- Solo findings `disposition=Open` — Fixed/Fp/Wontfix non iniettati
- Truncation del body se supera il budget
- Priorità: High > Medium > Low

### Rischio 2: Race Condition Shard Read

**Problema:** L'agente legge lo shard mentre Censor lo sta ancora scrivendo.

**Mitigazione:**

- Censor usa atomic write + `.lock` sidecar (già implementato in `ledger.rs`)
- Se il file è lockato o incompleto → errore MCP → l'agente continua
- Retry automatico: il prossimo turno rilegge

### Rischio 3: Performance Hook

**Problema:** Chiamare `censor_findings` dopo ogni write/edit → latenza aggiunta.

**Mitigazione:**

- Cache locale: se lo shard non è cambiato dall'ultima volta → skip
- Timeout: se la lettura supera 5s → skip → prossimo turno
- Cooldown: max 1 check Censor per 2 secondi per file

### Rischio 4: Breaking Changes Rust

**Problema:** Rimuovere `Censoring` status e `spawn_verdict_thread` → breaking changes.

**Mitigazione:**

- Migrare PR-by-PR
- Test automatici per ogni fase
- Il Censor watcher e gli shard RESTANO — solo il verdict gate è semplificato

---

## 8. Riepilogo: Cosa Cambia

### Non Cambia

- ✅ Censor watcher (filesystem monitoring)
- ✅ Runner deterministici (35+ tools)
- ✅ Gemma LLM (async, locale)
- ✅ Schema degli shard (Finding, CensorShard)
- ✅ `censor://findings-updated` event → UI frontend
- ✅ MCP `censor_findings` → tool Oracle server
- ✅ FS backend (read/grep/glob)
- ✅ Exa backend (fetch/websearch)
- ✅ Agent loop core (stop conditions)

### Cambia

- ❌ `MiniCoderStatus::Censoring` → RIMOSSO
- ❌ `EscalationFinding` → RIMOSSO
- ❌ `spawn_verdict_thread` blocking → RIMOSSO (fire-and-forget)
- ✅ `ToolExecutor::censor_findings()` → NUOVO metodo trait
- ✅ `TranscriptEntry::CensorFindings` → NUOVO variant
- ✅ `run_burst` → CHIAMATA `censor_findings()` dopo write/edit
- ✅ `RealExecutor::censor_findings()` → IMPLEMENTAZIONE MCP
- ✅ `format_censor_findings_summary()` → FORMATTAZIONE findings
- ✅ Prompt template → SEZIONE Censor feedback loop

### Nuovo

- 🆕 `censor_findings()` method su `ToolExecutor` trait
- 🆕 `TranscriptEntry::CensorFindings` variant
- 🆕 `Transcript::push_censor_findings()` method
- 🆕 `format_censor_findings_summary()` funzione pura
- 🆕 Censor feedback loop rules nel prompt template

---

## 9. Checklist Must-Have Truths

Verificare dopo ogni fase:

- [ ] **I runner deterministici continuano a funzionare** (nessuna modifica al Censor core)
- [ ] **Gli shard Censor sono leggibili da MCP** (tool `censor_findings` risponde correttamente)
- [ ] **I findings sono iniettati nel transcript** (dopo write/edit, appaiono nel prossimo turno)
- [ ] **Il mini-coder funziona senza verdict gate** (no più Censoring status)
- [ ] **Il prompt template contiene regole Censor** (l'agente sa come gestire i findings)
- [ ] **Nessun breaking change per UI frontend** (`censor://findings-updated` ancora emesso)
- [ ] **Nessun breaking change per main coder** (MCP `censor_findings` ancora funzionante)
- [ ] **Token budgeting funziona** (max 10/file, max 50 totali, max 4096 bytes)
- [ ] **StubExecutor non cambia comportamento** (default no-op ritorna "")

---

## 10. Riferimenti

- [Pi-Lens Architecture](https://github.com/harms-haus/pi-lens/blob/main/docs/architecture.md) — hook system, tool_result hook
- [Zeph LSP Context Injection](https://bug-ops.github.io/zeph/concepts/lsp-context-injection.html) — automatic diagnostics injection
- [Cargo-Context](https://github.com/asmuelle/cargo-context) — MCP server, token budgeting
- [Claude Code Agent Loop](https://claude-code-from-source.com/ch05-agent-loop/) — stop hooks, error recovery
- [Motosan Agent Loop](https://github.com/motosan/agent-loop) — LoopInterceptor pattern (Rust)
- [Aspis Censor Schema](../src-tauri/src/backend/censor/schema.rs) — Finding, CensorShard, Disposition
- [Aspis Censor Ledger](../src-tauri/src/backend/censor/ledger.rs) — shard read/write, supersede
- [Aspis Censor Commands](../src-tauri/src/backend/censor/commands.rs) — MCP tool `censor_findings`
- [Aspis Mini-Coder Executor](../src-tauri/src/backend/mini_coder_executor.rs) — verdict gate, finalize_finished_mini
- [Aspis Mini-Coder](../src-tauri/src/backend/mini_coder.rs) — MiniCoderStatus, EscalationFinding
- [Devboule Agent Loop](../devboule-coder/src/agent_loop.rs) — run_burst, ToolExecutor, Transcript
- [Devboule Executor](../devboule-coder/src/executor.rs) — RealExecutor, McpBackend trait
- [Devboule Config](../devboule-coder/src/config.rs) — build_runtime, env vars
- [Devboule Prompt](../devboule-coder/src/prompt.rs) — build_system_prompt
