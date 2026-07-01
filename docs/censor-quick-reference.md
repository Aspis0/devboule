# Censor — Quick Reference (2 righe per file)

## `src-tauri/src/backend/censor/mod.rs`

Entry point del modulo: definisce `CENSOR_DIR` (`.aspis-censor/`), `now_stamp()` per timestamp RFC3339, esporta tutti i submodule. Il Censor è un subsystem di code-review continuo e local-first per progetto.

## `src-tauri/src/backend/censor/commands.rs`

Tutti i Tauri commands: `censor_start_watch`, `censor_stop_watch`, `censor_review_now`, `censor_get_findings`, `censor_count_open`, `censor_status`, `set_censor_trusted`, `censor_dispose_finding`, `censor_open_in_editor`. `CensorState` gestisce watcher attivo, gemma probe cache (identity-keyed), flag oneshot. Trust gating: nessun runner/Gemma senza trust utente. `CensorState` è single-active (un progetto alla volta). `kill_all_on_exit` ripulisce thread e subprocessi all'exit. `censor_review_now` è fire-and-forget (one-shot fallback se nessun watcher attivo).

## `src-tauri/src/backend/censor/orchestrator.rs`

Cuore del motore: `plan_fine()` seleziona i runner per ogni file, `coarse_runners()` il set project-wide. `run_fine_batch()` lancia runner per-file + opzionale Gemma, `run_coarse_pass()` lancia tutto-project. `fine_batch_collect()` legge file, lancia runner, append Gemma findings, scrive shard con source-scoped merge. `GEMMA_SOURCE = "gemma"`. FINE debounce 400ms, COARSE 4000ms. `run_fine_batch_no_rail()` per il mini-coder verdict gate (senza training export).

## `src-tauri/src/backend/censor/watch.rs`

Filesystem watcher: `notify` su project root, debounce thread + serialized worker thread. `bucket_event()` route TS/Py/Go/CSS/etc → FINE (400ms), Rust/Other → COARSE (4s). `CensorWatchHandle` con stop non-blocking signal-then-detached-reaper. `start_watch()` crea watcher + loop + worker, `stop()` ripulisce senza blocca. Ignore set include `.aspis-censor` (self-trigger guard).

## `src-tauri/src/backend/censor/gemma.rs`

Tier AI locale opzionale: `GemmaClient` trait con `probe()`, `generate()`, `generate_with_images()`. 4 provider: Ollama (default), oMLX (OpenAI-compat), AppleFm (fm CLI), Cloud (HTTPS remoto, opt-in). `GEMMA_MODEL = "NVIDIA-Nemotron-3-Nano-4B:Q4_K_M"`. Privacy: loopback solo, mai content su disco, redact secrets. Timeout 60s generate, 5s probe. MAX 24k chars per file, 20 findings max per risposta.

## `src-tauri/src/backend/censor/runners/mod.rs`

35+ runner deterministici: clippy, cargo-check, cargo-audit, cargo-deny, cargo-fmt, eslint, tsc, knip, prettier, ruff, ruff-format, bandit, vulture, pyright, gitleaks, jscpd, semgrep, gofmt, go-vet, cppcheck, tidy, ktlint, shellcheck, yamllint, sqlfluff, hadolint, actionlint, stylelint, lizard, zizmor, npm-audit, pip-audit, oxlint. Ogni runner: `parse_<tool>()` pure testabile + `run(root, target)` thin IO. `applicable_runners()` seleziona in base a project kinds + FileLang. Granularity: Fine (per-file) vs Coarse (project-wide).

## `src-tauri/src/backend/censor/schema.rs`

`Finding` struct: id, file, line, severity, category, source, title, body, content_hash, created_at, disposition (Open/Fixed/Fp/Wontfix), verdict (Suspected/Confirmed), provenance. `CensorShard`: file_rel_path, content_hash, findings[], created_at. `Disposition` e `Category` enums. `ProvenanceEntry` per audit trail.

## `src-tauri/src/backend/censor/catalog.rs`

Model catalog: Nemotron-3-Nano-4B (Recommended), Gemma 3 12B, Granite-4.0 H-Tiny, DeepSeek-R1 8B. `model_tool_capable()` probe Ollama `/api/show` capabilities (case-insensitive "tools"). Catalog esclude MiMo/Phi/GLM (inutilizzabili).

## `src-tauri/src/backend/censor/detect.rs`

`ProjectKind` enum: Rust/Node/Python/Go/Cpp/Html/Kotlin/Shell/Yaml/Sql/Dockerfile/GithubActions/Css/Other. `FileLang` enum per linguaggio file. `detect_project_kinds()` scansiona manifest (Cargo.toml, package.json, requirements.txt, go.mod, etc.) per determinare kinds presenti. `applicable_runners()` mappa kinds+lang → runner list.

## `src-tauri/src/backend/censor/ledger.rs`

Storage shards: `<sha256(fileRelPath)>.json` in `.aspis-censor/`. `read_supersede_write_shard()` source-scoped merge: refresh sorgenti solo per i runner appena eseguiti, le altre sorgenti sopravvivono. Lock-free con atomic write + `.lock` sidecar. `validate_rel_path()` security guard contro path traversal.

## `src-tauri/src/backend/censor/severity.rs`

`Severity` enum: Low/Medium/High/Critical. `Category` enum: Correctness/Security/Style/Performance/Maintainability. Normalizzatori tool→severity per ogni runner.

## `src-tauri/src/backend/censor/extract.rs`

Estrae e normalizza i finding dal testo grezzo del modello Gemma. `parse_gemma()` trasforma l'output del modello in `Vec<RawFinding>`. Secret redaction + length cap su title/body.

## `src-tauri/src/backend/censor/watch.rs` (dettagli)

Due debounce windows: FINE 400ms per TS/Py/Go/Cpp/HTML/Kotlin/Shell/YAML/SQL/Dockerfile/GithubActions/CSS. COARSE 4s per Rust/Other. Worker thread serializzato: un solo pass alla volta, nessun concurrent shard race. `Work` enum: Fine(files)/Coarse/ReviewNow(file?).

## `src-tauri/src/backend/mini_coder.rs`

Directive lifecycle: `MiniCoderDirective` con id, task, status, input, output. `MiniCoderStatus` enum: Pending/Running/Completed/Failed/Censoring (nuovo). `apply_emitted_edits()` valida e applica PTY edits. `apply_write_directive_edits()` per agentic MCP tools. `claim_pending_directive()` / `requeue_directive()` per scheduling. `set_directive_status()` thread-safe con Arc<Mutex>.

## `src-tauri/src/backend/mini_coder_executor.rs`

Executor loop: `run_executor()` claim → execute → finalize. `finalize_finished_mini()` gestisce outcome: se Running→Censoring (verdict thread), se Done/Failed→terminal. `spawn_verdict_thread()` background: Censor → requeue se passed → write verdict → notify worker. `run_pass()` lancia agentic worker o PTY subprocess. `spawn_agentic_worker()` loop multi-turno con condvar wait per verdict.

## `src-tauri/src/backend/sandbox/mod.rs`

Sandbox policy: `SandboxPolicy` con `readonly_root` (project root readable/writable per tools) + `writable_paths` whitelist + network policy + ulimit rlimits. Seatbelt su macOS per subprocessi PTY. Il sandbox NON blocca filesystem writes al project root — il vero gate è il project trust status.

---

## 🔄 Workflow AI + Censor (cosa succede realmente)

**Flusso attuale (pre-verdict): ASINCRONO, l'AI NON vede i findings.**

1. AI scrive file → filesystem write event → notify watcher parte
2. Debounce 400ms/4s → worker thread separato lancia runner deterministici
3. Shard scritti in `.aspis-censor/` → evento `censor://findings-updated` → UI frontend
4. AI: CONTINUA A LAVORARE, non blocca, non aspetta, non legge

**Il verdict gate** (`run_fine_batch_no_rail`) esiste ma è un passaggio che legge findings SENZA scrivere il training rail. Il `record_rail=false` evita duplicati in `pairs.jsonl`.

**Problema:** i findings esistono su disco ma non sono nel contesto dell'AI. Come un code reviewer che scrive post-it che nessuno legge.

**Soluzione proposta:** passare i findings all'AI nel contesto del task Kanban (opzione 1, zero modifiche al Censor).

---

## 🏗️ Sandbox + Censor: come interagiscono

**I coder NON scrivono direttamente i file.** Due percorsi:

1. **One-shot (EmitEdits):** mini coder (PTY subprocess Python) genera edits come testo (old_string → new_string) → Rust backend (`apply_emitted_edits`) VALIDA e APPLICA i edits al filesystem. Il Censor WATCHER vede queste writes.
2. **Agentic (AgenticIterative):** mini coder gira un tool-loop multi-turno via HTTP → scrive file attraverso MCP tools sandboxed → anche questi scrivono sul project root → Censor vede le writes.

**Il sandbox NON blocca le writes al filesystem.** Il `SandboxPolicy` ha `readonly_root` (project root, readable ma writeable per i tools agentic) + `writable_paths` (whitelist). Il sandbox controlla SOLO: (a) network policy, (b) rlimits CPU/memoria, (c) per i subprocessi PTY: Seatbelt su macOS. Le writes al filesystem del project root sono permesse.

**Il Censor vede TUTTE le writes al project root** (anche quelle dei coder), MA solo se il progetto è TRUSTED. Se non trusted, il watcher non parte affatto — né deterministici né Gemma.

---

## ⚡ Quando partono i programmi non-deterministici?

**Il Censor ha DUE livelli:**

1. **Deterministico (sempre attivo se il progetto è trusted):** 35+ runner (clippy, eslint, tsc, semgrep, gitleaks...) — partono su ogni file salvato, dopo debounce (400ms fine / 4s coarse). LANCIA SUBPROCESSI ESTERNI MA SONO DETERMINISTICI.

2. **Non-deterministico (Gemma, SOLO se configurato dall'utente):** Un modello locale (Nemotron 4B di default, o Gemma 3 12B, ecc.) via Ollama/oMLX/appleFm. Parte DOPO i runner deterministici, per file, per trovare semantic bugs che i linters non catturano. È **additivo** (non sovrascrive mai i finding deterministici).

**Il flusso per ogni file salvato:**

1. File salvato → notify event
2. Debounce (400ms fine o 4s coarse)
3. FINE: per ogni file, lancia tutti i runner deterministici applicabili → scrive shard
4. Se Gemma è attivo: chiama il modello locale → append findings allo shard
5. Emette `censor://findings-updated` → UI aggiorna

**Il "vero" non-deterministico** (il mini-coder / LLM che scrive codice) è fuori dal Censor: parte quando un task Kanban lo richiede, non quando salvi un file.

---

## 🏛️ Mini-Coder Verdict Workflow (design implementato)

**Problema:** il Censor corre in modo sincrono sul thread executor, bloccando tutto il scheduling per 5-30s. L'agentic worker non può ricevere nuovi task mentre il Censor è in esecuzione.

**Soluzione:** aggiungere status `Censoring` al directive state machine, con verdict thread in background e condvar synchronization per il worker.

### State Machine del Directive

```
Pending → Running → Censoring → Done (re-queue)
                              → Failed (terminal)
```

### Flusso di esecuzione

1. **Agentic worker** completa il lavoro → scrive result → `drop_guard()`
2. **Executor** vede directive "done" → `finalize_finished_mini()`
3. Se status `Running` → `spawn_verdict_thread()` (NON blocca)
4. Status → `Censoring` (directive resta "live", executor non finalizza)
5. **Verdict thread** (background):
   - `run_censor()` → ottiene verdict
   - Se passed → `requeue_directive()` (nuovo lavoro)
   - Scrive verdict in `.aspis-censor/verdict.json`
   - Status → `Done` o `Failed`
   - Notifica worker: `set_verdict_ready_flag()`
6. **Agentic worker** (sempre in loop):
   - `wait_for_verdict()` → condvar wait fino a notifica
   - Legge verdict
   - Se passed → re-queue → pick up nuovo lavoro
   - Se failed → exit loop

### Sincronizzazione

- **Condvar** (`std::sync::Condvar`): worker aspetta notifica dal verdict thread
- **Arc<AtomicBool>**: flag `verdict_ready` per wake-up
- **Arc<Mutex<Directive>>**: stato condiviso tra executor, verdict thread, worker
- **Zero deadlock**: verdict thread non aspetta executor, executor non aspetta verdict thread

### Executor skip logic

```rust
match directive.status {
    MiniCoderStatus::Running => {
        spawn_verdict_thread(directive, outcome);  // non-bloccante
        directive.status = MiniCoderStatus::Censoring;
    }
    MiniCoderStatus::Censoring => {
        // Verdict thread ha già finalizzato → skip
    }
    MiniCoderStatus::Done | MiniCoderStatus::Failed => {
        apply_terminal_outcome(directive);
    }
}
```

---

## 🔌 Pattern Event System di Pi (per blocking leggero)

**Pi ha un event bus nativo con `tool_call` e `tool_result` events.** Questo è il pattern che ci serve per integrare Censor senza custom threading.

### `tool_call` — può BLOCCARE

```typescript
pi.on("tool_call", async (event, ctx) => {
  // event.input.command, event.input.args
  if (event.input.command.includes("rm -rf")) {
    return { block: true, reason: "Dangerous command" };
  }
});
```

- **Blocca l'esecuzione dello strumento** prima che parta
- Ritorna `{ block: true, reason?: string }` per fermare
- Il modello riceve il `reason` e può riavviare

### `tool_result` — può MODIFICARE

```typescript
pi.on("tool_result", async (event, ctx) => {
  // event.input.command, event.result.content
  // può modificare event.result prima che arrivi al modello
});
```

- **Intercetta il risultato** dopo l'esecuzione
- Può modificare `event.result` prima che arrivi al modello
- Utile per aggiungere contesto (es. findings del Censor)

### Flusso eventi Pi

```
tool_execution_start → tool_call (can block) → tool_execution_update → tool_result (can modify) → tool_execution_end
```

### Applicazione al Verdict Workflow

Invece di custom threading con condvar, si potrebbe usare:

1. **`tool_result`** per intercettare il risultato del mini-coder
2. **`tool_call`** per bloccare il prossimo tool call finché il Censor non ha finito
3. **Zero custom state management** — si usa il sistema di eventi esistente di Pi

**Vantaggio:** zero modifiche al Rust backend, tutto nell'extension layer di Pi.

---

## 🔌 Pi-Lens Integration: Runner → Cache → Iniezione nel contesto

**Problema:** il verdict gate attuale (thread Rust dedicato + condvar) è complesso e du-plica lavoro già fatto da pi-lens. Pi-lens già lancia runner (oxlint, ruff, eslint, opengrep...) al `turn_end` dell'agente, colleziona `Diagnostic[]`, li categorizza in `blockerParts` / `advisoryParts` e li inietta nel prossimo turno come messaggio `user`:

```
Agente scrive file (turn N)
  │
  ├─► pi-lens turn_end: dispatch runner → Diagnostic[] → blockerParts / advisoryParts
  │       └─► writeCache("turn-end-findings", { content })
  │
  └─► Agente turno N+1:
        consumeTurnEndFindings() → inietta come messaggio "user":
        "[pi-lens automated check — not a user request]
         Address 🔴 blockers before continuing; ℹ️ advisories are informational only.
         ...
```

**Oggi Censor e pi-lens non parlano.** Censor = Rust/Tauri backend (watcher + runner + shard), pi-lens = TS extension Node.js (dispatcher + cache + injection). Due stack separati.

### Tre opzioni di integrazione

#### Opzione A: Censor scrive nel cache di pi-lens

Censor continua a lanciare runner come oggi. Il verdict thread non crea `EscalationFinding` ma **scrive i findings nel cache `turn-end-findings`** che pi-lens legge e inietta. Zero duplicazione runner, formato di iniezione già bello.

```
Censor (Rust) → .pi-lens/cache/turn-end-findings.json
                      │
                      └─► pi-lens consumeTurnEndFindings()
                              └─► inietta nel prossimo turno dell'agente
```

**Vantaggio:** zero duplicazione, format blocker/advisory già pronto.
**Sfida:** ponte Rust → JSON file che pi-lens legge.

#### Opzione B: Pi-lens legge gli shard di Censor

Si registra un runner "censor-ledger" in pi-lens che legge `.aspis-censor/shards/` e li trasforma in `Diagnostic[]`. Il dispatcher li passa attraverso il pipeline normale.

**Vantaggio:** nessun ponte Rust→TS, tutto in TS.
**Sfida:** parsing shard JSON → Diagnostic mapping.

#### Opzione C (preferita): Togliere il verdict gate, usare pi-lens come unico layer

Il mini-coder NON ha più il verdict thread in Rust. Il flusso diventa:

```
1. Agente salva file (task N)
2. Censor watcher vede modifiche → runner deterministici → shard su disco
3. censor://findings-updated → UI frontend (come oggi)
4. Pi-lens turn_end → legge shard (o lancia runner) → write cache → inietta
5. Agente turno N+1 vede i findings nel contesto → decide: fix o continua
6. Budget decision (retry/escalate) nel prompt dell'agente stesso:
   "Se vedi 🔴 blocker, correggili. Se dopo N tentativi persiste, escalate."
```

**Vantaggio:** architettura più semplice, zero threading custom, niente `EscalationFinding`/`summarize_findings_for_feedback`, budget decision nel prompt invece che in Rust.

**Sfida:** Gemma (Censor LLM) rimane async → findings un turno in ritardo (accettabile).

### Cosa NON cambia

- **Censor watcher:** continua a monitorare file system, lanciare runner, scrivere shard
- **Gemma/LLM:** continua async/offloaded, scrive findings nello shard
- **Main Coder:** continua a usare MCP `censor_findings` per pull interattivo
- **UI Frontend:** continua a ricevere `censor://findings-updated`

### Cosa cambia

- **Mini-coder verdict gate:** RIMOSSO dal Rust backend
- **Feedback loop:** passato da "thread Rust dedicato" a "pi-lens turn_end injection"
- **Budget decision:** passato da Rust (`GateDecision` enum) a prompt dell'agente
- **`EscalationFinding` / `summarize_findings_for_feedback`:** RIMOSSI
- **`build_retry_directive()`:** semplificato (no feedback struct, solo prompt template)

### Gemma/Censor LLM: rimane async

Gemma parte in background, scrive findings nello shard. Pi-lens legge gli shard al turn_end. Se Gemma non ha ancora finito → nessun finding Gemma in quel turno. Turno dopo → c'è. Questo è corretto e accettabile.

---

## 📋 Checklist implementazione

- [x] `Censoring` status in `MiniCoderStatus` enum (`mini_coder.rs`)
- [ ] `spawn_verdict_thread()` in `mini_coder_executor.rs`
- [ ] Executor skip logic per `Censoring` state
- [ ] Agentic worker `wait_for_verdict()` con condvar + AtomicBool
- [ ] One-shot worker: re-queue dal verdict thread (nessun condvar necessario)
- [ ] Test async verdict flow (executor non stall, worker resume)
- [ ] Validare pattern alternativo con Pi event system (`tool_call`/`tool_result`)

### Integrazione pi-lens (nuova)

- [ ] Valutare opzione A (Censor → cache pi-lens) vs B (pi-lens legge shard) vs C (togli verdict gate)
- [ ] Implementare bridge Rust → cache pi-lens (Opzione A)
- [ ] Oppure registrare runner "censor-ledger" in pi-lens (Opzione B)
- [ ] Rimuovere `EscalationFinding` / `summarize_findings_for_feedback` dal Rust
- [ ] Semplificare `build_retry_directive()` a prompt template
- [ ] Budget decision nel prompt dell'agente (retry/escalate logic)
- [ ] Test end-to-end: agente scrive → pi-lens inject → agente fix → verifiche
