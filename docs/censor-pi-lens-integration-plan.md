# Censor → Pi-Lens Integration Plan (Option C)

**Data:** 2026-06-30  
**Stato:** Bozza  
**Obiettivo:** Rimuovere il verdict gate Rust dal mini-coder e usare un sistema tipo pi-lens per iniettare i findings del Censor nel contesto dell'AI agent.

---

## 1. Problema

Il Censor (Rust/Tauri backend) lancia 35+ runner deterministici + opzionale Gemma (LLM locale) su ogni file modificato. I findings vengono scritti in shard JSON su disco (`.aspis-censor/`). **Ma l'AI agent non li vede mai.** I findings esistono come "post-it appesi al muro" — il Censor fa il suo lavoro, ma il feedback non ritorna al coder.

Il verdict gate attuale (`spawn_verdict_thread` + condvar in `mini_coder_executor.rs`) è complesso, blocca lo scheduling, e duplica lavoro che pi-lens fa già bene.

---

## 2. Architettura Esistente (As-Is)

### Censor (Rust/Tauri)

```
File modificato
  → notify watcher (watch.rs)
  → debounce (400ms FINE / 4s COARSE)
  → lancia runner deterministici (runners/mod.rs)
  → opzionale: Gemma LLM (gemma.rs)
  → scrive shard JSON (ledger.rs) in .aspis-censor/
  → emette `censor://findings-updated` → UI frontend
```

### Mini-Coder Verdict Gate (attuale)

```
Agentic worker completa task
  → executor: finalize_finished_mini()
  → se Running → spawn_verdict_thread() (background)
  → status → Censoring
  → verdict thread: run_censor() → requeue se passed
  → condvar wait: worker aspetta verdict
  → Zero deadlock: verdict thread non blocca executor
```

**Problemi:**

- Thread Rust dedicato per ogni task → overhead complesso
- `EscalationFinding` / `summarize_findings_for_feedback` → logica duplicata
- `build_retry_directive()` → re-queue custom con feedback struct
- Il verdict gate è sincrono per il worker, anche se asincrono per l'executor
- Budget decision (retry/escalate) codificato in Rust invece che nel prompt

---

## 3. Ricerca Online: Pattern Esistenti

### 3.1 Pi-Lens (pi-coding-agent extension)

**Architettura:**

- **Client (TypeScript):** hook registration, file resolution, config loading, TUI rendering
- **Daemon (`@harms-haus/code-lens`):** esecuzione runner (prettier, eslint, LSP, tsc), caching, LSP lifecycle
- **Comunicazione:** Unix domain socket, JSON-RPC 2.0

**Integrazione con Agent:**

- Hook `tool_result`: intercetta write/edit/bash → risolve file → lancia checks → **appende risultati al tool result content**
- Hook `tool_execution_update`/`end`: monitora subagent → checks su file modificati
- **`pi.sendMessage()`** con `customType: "pi-lens-diagnostics"` → TUI renderer
- **Append al tool result:** il testo dei findings viene inserito direttamente nel risultato dello strumento → l'LLM lo vede nel contesto

**Pattern chiave:** `tool_result` hook → **non blocca** l'agent → appende findings al risultato → LLM li vede nel prossimo turn.

### 3.2 Zeph — LSP Context Injection

**Pattern:**

- Hook automatico dopo `write_file`: fetch diagnostics → inject come `[lsp ...]` message
- Hook dopo `read_file`: fetch hover info
- Hook prima di `rename_symbol`: fetch references
- I dati iniettati appaiono come messaggio `user` nella conversation history
- Per-turn `token_budget` cap per evitare context bloat

### 3.3 Claude Code — Stop Hooks

**Pattern:**

- Quando il modello dice "ho finito" (nessun tool use) → stop hooks valutano se è davvero finito
- I stop hooks possono restituire **blocking errors** → iniettati nella message history
- Il loop continua con `stopHookActive: true` (prevents re-running hooks)
- **Circuit breaker:** dopo 3 tentativi consecutivi → si ferma

### 3.4 Cursor — Stop Hook Pattern

**Pattern:**

- `pnpm lint` e `pnpm build` dopo ogni agente turn
- Hook `stop` che cattura errori → forza l'agente a correggere

### 3.5 Cargo-Context (Rust)

**Pattern:**

- MCP server (`cargo-context-mcp`) esporta tools: `build_context_pack`, `get_last_error`, `get_diff`, `expand_macros`
- CLI produce "context pack" in markdown/XML/JSON → consumabile da qualsiasi LLM
- **Token budgeting** con priority/proportional/truncate strategies
- **Secret scrubbing** built-in
- **MCP resources:** `cargo-context://diff`, `cargo-context://errors`, `cargo-context://map`

---

## 4. Decisione: Option C

### Perché Option C (non A o B)?

| Opzione | Vantaggi | Svantaggi |
|---------|----------|-----------|
| **A:** Censor → cache pi-lens | Zero duplicazione runner | Ponte Rust→JSON file, formato accettabile ma non ottimale |
| **B:** Pi-lens legge shard Censor | Tutto in TS, nessun ponte Rust | Parsing shard→Diagnostic mapping, doppio formato |
| **C:** Rimuovere verdict gate, usare pi-lens | **Architettura più semplice, zero threading custom, niente EscalationFinding, budget decision nel prompt** | Gemma async → findings un turno in ritardo (accettabile) |

**Scelgo Option C** perché:

1. **Rimuove complessità non necessaria:** il verdict gate con condvar è un pattern anti-pattern in un sistema agent-driven
2. **Sfrutta pi-lens come layer unificato:** lo stesso sistema che inietta LSP diagnostics inietta anche Censor findings
3. **Budget decision nel prompt:** più flessibile di un enum Rust — l'LLM può decidere caso-per-caso
4. **Gemma rimane async:** i findings del modello locale arrivano "un turno dopo" — questo è accettabile perché l'LLM locale è già asincrono
5. **Il mini-coder diventa più semplice:** no più Censoring status, no più condvar, no più requeue custom

---

## 5. Architettura Target (To-Be)

```
┌─────────────────────────────────────────────────────────────────────┐
│                        Devboule                              │
│                                                                      │
│  ┌──────────────┐    ┌──────────────────┐    ┌───────────────────┐  │
│  │  Mini-Coder   │    │    Censor        │    │   Pi-Lens         │  │
│  │  (Agent Loop) │    │  (Rust/Tauri)    │    │  (TS Extension)   │  │
│  │               │    │                  │    │                   │  │
│  │  Task N:      │    │  File modificato │    │  Hook:            │  │
│  │  - write file │    │  → runner        │    │  tool_result      │  │
│  │  → tool_result│    │  → shard JSON    │    │  → leggi shard    │  │
│  │               │    │  → event         │    │  → inject context │  │
│  │  Turno N+1:   │    │                  │    │                   │  │
│  │  - vede       │    │                  │    │                   │  │
│  │    findings   │    │                  │    │                   │  │
│  └──────────────┘    └──────────────────┘    └───────────────────┘  │
│         ▲                      ▲                      ▲              │
│         │                      │                      │              │
│         └──────────────────────┴──────────────────────┘              │
│                          Agent Context                               │
└─────────────────────────────────────────────────────────────────────┘
```

### Flusso

1. **Agente scrive file** (task Kanban N) → tool call `write`/`edit`
2. **Censor watcher** vede la modifica → lancia runner → scrive shard
3. **Pi-lens hook `tool_result`** intercetta la write → legge gli shard di Censor → formatta findings
4. **Pi-lens inietta findings** nel contesto dell'agente (append al tool result o messaggio user)
5. **Agente turno N+1** vede i findings → decide: fix, ignore, o continue
6. **Budget decision** nel prompt dell'agente: "Se vedi 🔴 blocker, correggili. Se dopo N tentativi persiste, escalate."

---

## 6. Design Tecnico Dettagliato

### 6.1 Censor: Cosa NON Cambia

- **Watcher:** continua a monitorare filesystem, lanciare runner, scrivere shard
- **Gemma/LLM:** continua async/offloaded, scrive findings nello shard
- **Main Coder:** continua a usare MCP `censor_findings` per pull interattivo
- **UI Frontend:** continua a ricevere `censor://findings-updated`
- **Schema degli shard:** rimane lo stesso (Finding struct, CensorShard, ecc.)

### 6.2 Censor: Cosa Cambia

#### Rimosso da `mini_coder.rs`

```rust
// RIMOSSO:
enum MiniCoderStatus {
    Pending,
    Running,
    Censoring,     // ← RIMUOVI
    Done,
    Failed,
}

struct EscalationFinding { ... }  // ← RIMUOVI
fn summarize_findings_for_feedback(...) { ... }  // ← RIMUOVI
```

#### Rimosso da `mini_coder_executor.rs`

```rust
// RIMOSSO:
fn spawn_verdict_thread(...) { ... }  // ← RIMUOVI tutto il thread
fn build_retry_directive(...) { ... }  // ← RIMUOVI, sostituito da prompt template

// SEMPLIFICATO:
fn finalize_finished_mini(directive, outcome) {
    match directive.status {
        MiniCoderStatus::Running => {
            // NO MORE: spawn_verdict_thread()
            // NO MORE: directive.status = Censoring
            // Directamente a Done/Failed
            apply_terminal_outcome(directive);
        }
        MiniCoderStatus::Done | MiniCoderStatus::Failed => {
            apply_terminal_outcome(directive);
        }
    }
}
```

#### Rimossi da `mini_coder_executor.rs`

- `wait_for_verdict()` con condvar
- `set_verdict_ready_flag()`
- `Arc<AtomicBool>` per verdict_ready
- Executor skip logic per `Censoring` state

#### Rimossi da `sandbox/mod.rs`

- Nessuna modifica necessaria (sandbox non è coinvolto nel verdict gate)

### 6.3 Pi-Lens: Cosa AGGIUNGERE

#### Nuovo Hook: `censor-shard-reader`

```typescript
// In pi-lens hook-runner.ts

/**
 * Legge gli shard Censor per un dato file e li formatta come findings.
 * Legge da .aspis-censor/<sha256(file)>.json
 */
async function readCensorShard(file: string): Promise<CensorFinding[]> {
    const shardPath = getCensorShardPath(file);
    const shard = await readJsonShard(shardPath);
    return shard?.findings?.filter(f => f.disposition === 'Open') ?? [];
}

/**
 * Formatta i findings Censor nel formato pi-lens Diagnostic[].
 * Mappa: Severity → DiagnosticSeverity, Category → tag
 */
function formatAsDiagnostics(findings: CensorFinding[]): Diagnostic[] {
    return findings.map(f => ({
        file: f.file,
        range: {
            start: { line: f.line, character: 0 },
            end: { line: f.line, character: 999 }
        },
        severity: severityMap[f.severity],  // Low→3, Medium→2, High→1, Critical→0
        source: `censor:${f.source}`,
        message: `[${f.category}] ${f.title}\n${f.body}`,
        code: f.category,
        tags: [f.verdict === 'Confirmed' ? 1 : 2]  // 1=QuickFix, 2=Unnecessary
    }));
}
```

#### Integrazione nel Hook `tool_result`

```typescript
// In pi-lens index.ts — hook tool_result

pi.on("tool_result", async (event, ctx) => {
    // ... existing logic: resolve files from write/edit/bash ...
    
    // NEW: Read Censor findings for affected files
    const censorFindings = [];
    for (const file of affectedFiles) {
        const findings = await readCensorShard(file);
        censorFindings.push(...findings);
    }
    
    // Format as diagnostics
    const diagnostics = formatAsDiagnostics(censorFindings);
    
    if (diagnostics.length > 0) {
        // Inject into tool result (so LLM sees them)
        const formatted = formatDiagnosticsForLLM(diagnostics);
        event.result.content.push({
            type: "text",
            text: `\n\n--- [Censor Findings] ---\n${formatted}\n--- [End Censor] ---`
        });
        
        // Also send to TUI
        if (rendererEnabled) {
            sendDiagnosticMessage(pi, ctx, { diagnostics }, affectedFiles.length);
        }
    }
});
```

#### Token Budgeting (da cargo-context)

```typescript
// Per evitare context bloat, limitare i findings iniettati
const MAX_FINDINGS_PER_FILE = 10;
const MAX_TOTAL_FINDINGS = 50;
const MAX_BYTES = 4096;  // budget per-turn

function prioritizeFindings(diagnostics: Diagnostic[]): Diagnostic[] {
    // 1. Critical/High severity first
    // 2. One finding per file max (se > MAX_FINDINGS_PER_FILE)
    // 3. Truncate body se supera MAX_BYTES
    return diagnostics
        .sort((a, b) => a.severity - b.severity)
        .slice(0, MAX_TOTAL_FINDINGS)
        .map(d => ({ ...d, message: truncate(d.message, MAX_BYTES) }));
}
```

#### Prompt Template per Budget Decision

```markdown
# Template iniettato come system hint o user message

[Censor Feedback Loop]

I seguenti findings sono stati rilevati dai runner deterministici:

{findings}

**Regole:**
1. 🔴 Critical/High: correggi SEMPRE prima di procedere
2. 🟡 Medium: correggi se hai tempo, altrimenti nota e continua
3. 🟢 Low/Style: ignora se non influisce sulla funzionalità
4. Se dopo 3 tentativi lo stesso finding persiste → escalate nel task description
5. I findings con verdict=Confirmed sono più affidabili di Suspected

[End Censor Feedback Loop]
```

### 6.4 Bridge Shard → Diagnostic: Formato

Gli shard Censor hanno già una struttura ricca:

```json
{
    "file_rel_path": "src/main.rs",
    "content_hash": "sha256...",
    "findings": [
        {
            "id": "abc123",
            "file": "src/main.rs",
            "line": 42,
            "severity": "High",
            "category": "Correctness",
            "source": "clippy",
            "title": "unused_mut",
            "body": "variable does not need to be mutable",
            "content_hash": "...",
            "created_at": "2026-06-30T10:04:00Z",
            "disposition": "Open",
            "verdict": "Confirmed",
            "provenance": "clippy"
        }
    ],
    "created_at": "2026-06-30T10:04:00Z"
}
```

Il mapping a Diagnostic di pi-lens è diretto:

| Censor Field | → | Diagnostic Field |
|-------------|-----|-----------------|
| `file` | → | `file` |
| `line` | → | `range.start.line` |
| `severity` (Low/Med/High/Critical) | → | `severity` (3/2/1/0) |
| `category` (Correctness/Security/etc) | → | `code` + `source` |
| `source` (runner name) | → | `source` |
| `title` | → | `message` (prefix) |
| `body` | → | `message` (suffix) |
| `disposition: Open` | → | `active: true` |
| `verdict: Confirmed` | → | `tags: [QuickFix]` |
| `verdict: Suspected` | → | `tags: [Unnecessary]` |

---

## 7. Implementazione: Phases

### Phase 1: Lettura Shard (Rust → TS Bridge)

**Obiettivo:** Pi-lens legge gli shard Censor scritti da Rust

- [ ] Implementare `readCensorShard(file)` in pi-lens
  - Calcola SHA-256 del path relativo → cerca `<sha256>.json` in `.aspis-censor/`
  - Parse JSON → estrai `findings[]` con `disposition === 'Open'`
  - Handle file non trovato gracefully (ritorna [])
- [ ] Test: verificare che gli shard scritti dal Censor siano leggibili
- [ ] Validare path security: `validate_rel_path()` per prevenire path traversal

### Phase 2: Formattazione Findings

**Obiettivo:** Trasformare shard findings in formato leggibile dall'LLM

- [ ] Implementare `formatAsDiagnostics()` — mapping Censor → Diagnostic
- [ ] Implementare `formatDiagnosticsForLLM()` — formattazione markdown per LLM
- [ ] Implementare token budgeting (priorità severity, cap per-file, cap totale)
- [ ] Test: verificare che i findings formattati siano leggibili e utili

### Phase 3: Integrazione Hook `tool_result`

**Obiettivo:** Iniettare findings nel contesto dell'agente

- [ ] Modificare hook `tool_result` in pi-lens per chiamare `readCensorShard()`
- [ ] Append findings al tool result content (come testo)
- [ ] Send diagnostic message al TUI (se enabled)
- [ ] Test end-to-end: agente scrive file → findings appaiono nel prossimo turno

### Phase 4: Rimozione Verdict Gate Rust

**Obiettivo:** Semplicificare il mini-coder

- [ ] Rimuovere `Censoring` status da `MiniCoderStatus` enum
- [ ] Rimuovere `spawn_verdict_thread()` da `mini_coder_executor.rs`
- [ ] Rimuovere `build_retry_directive()` e `EscalationFinding`
- [ ] Rimuovere condvar + AtomicBool per verdict
- [ ] Semplificare `finalize_finished_mini()` — no più Censoring state
- [ ] Test: verificare che il mini-coder funzioni senza verdict gate

### Phase 5: Prompt Template + Budget Decision

**Obiettivo:** Spostare la logica di retry/escalate nel prompt

- [ ] Creare prompt template per feedback loop Censor
- [ ] Integrare template nel sistema di prompt dell'agente
- [ ] Test: verificare che l'agente applichi correttamente le regole di budget

### Phase 6: Gemma Async Handling

**Obiettivo:** Gestire il caso in cui Gemma non ha ancora finito

- [ ] Documentare il comportamento: findings Gemma arrivano "un turno dopo"
- [ ] Se Gemma non ha finito → nessun finding Gemma in quel turno → OK
- [ ] Turno dopo → c'è → l'agente lo vede
- [ ] Test: verificare il timing Gemma vs runner deterministici

### Phase 7: Testing End-to-End

**Obiettivo:** Verificare l'intero flusso

- [ ] Scenario 1: Agente scrive file con bug → Censor rileva → Agente corregge
- [ ] Scenario 2: Agente scrive file pulito → nessun finding → Agente continua
- [ ] Scenario 3: Agente ignora finding → retry → escalate dopo 3 tentativi
- [ ] Scenario 4: Multi-file write → findings per ogni file
- [ ] Scenario 5: Censor non trusted → nessun finding → Agente continua (comportamento normale)

---

## 8. Rischi e Mitigazioni

### Rischio 1: Context Bloat

**Problema:** Troppe findings iniettate nel contesto → LLM confuso o budget token esaurito.

**Mitigazione:**

- Token budgeting da cargo-context: priority strategy (Critical → High → Medium → Low)
- Cap: MAX_FINDINGS_PER_FILE = 10, MAX_TOTAL_FINDINGS = 50, MAX_BYTES = 4096
- Solo findings con `disposition: Open` — Fixed/Fp/Wontfix non iniettati
- Truncation del body se supera il budget

### Rischio 2: Race Condition Shard Read

**Problema:** Pi-lens legge lo shard mentre Censor lo sta ancora scrivendo.

**Mitigazione:**

- Censor usa atomic write + `.lock` sidecar (già implementato in `ledger.rs`)
- Pi-lens: se il file è lockato o incompleto → skip → prossimo turno
- Retry automatico: il prossimo tool_result hook rilegge

### Rischio 3: Gemma Async Timing

**Problema:** Gemma (LLM locale) è più lento dei runner deterministici → findings incompleti.

**Mitigazione:**

- Accettabile: i findings Gemma arrivano "un turno dopo"
- Runner deterministici (clippy, eslint, ecc.) sono più veloci → findings completi nel turno corrente
- L'agente corregge i deterministici subito, i Gemma nel turno successivo
- Documentare come feature, non come bug

### Rischio 4: Performance Hook

**Problema:** Leggere shard + formattare findings su ogni tool_result → latenza aggiunta.

**Mitigazione:**

- Cache locale: se lo shard non è cambiato dall'ultima volta → skip
- Async: il hook non blocca il tool_result (come fa già pi-lens)
- Timeout: se la lettura supera 5s → skip → prossimo turno
- Cooldown: max 1 check Censor per 2 secondi (simile al cooldown 5s di pi-lens)

### Rischio 5: Breaking Changes Rust

**Problema:** Rimuovere `Censoring` status e `spawn_verdict_thread` → breaking changes.

**Mitigazione:**

- Migrare il mini-coder PR-by-PR
- Test automatici per ogni fase
- Backup del codice prima della rimozione
- Il Censor watcher e gli shard RESTANO — solo il verdict gate è rimosso

---

## 9. Confronto con Alternative

### Perché non MCP (come cargo-context)?

- **Pro:** cargo-context ha un ottimo MCP server con tools e resources
- **Contro:** richiederebbe di esporre Censor come MCP server → aggiunta di complessità
- **Scelta:** Pi-lens è già nel nostro stack, usa hook nativi di pi, zero nuova infrastruttura

### Perché non scrivere nel cache di pi-lens (Opzione A)?

- **Pro:** Zero duplicazione runner
- **Contro:** Ponte Rust→JSON file, formato non standard, accoppiamento stretto
- **Scelta:** Pi-lens legge direttamente gli shard Censor (Opzione B/C) → più semplice, meno bridge

### Perché non Keep Verdict Gate?

- **Pro:** Funziona (già implementato)
- **Contro:** Complesso (condvar, Arc<Mutex>, AtomicBool), duplica lavoro di pi-lens, budget decision in Rust invece che nel prompt
- **Scelta:** Rimuovere → più semplice, più flessibile, più agent-native

---

## 10. Riepilogo: Cosa Cambia

### Non Cambia

- ✅ Censor watcher (filesystem monitoring)
- ✅ Runner deterministici (35+ tools)
- ✅ Gemma LLM (async, locale)
- ✅ Schema degli shard (Finding, CensorShard)
- ✅ `censor://findings-updated` event → UI frontend
- ✅ MCP `censor_findings` → main coder pull interattivo

### Cambia

- ❌ `MiniCoderStatus::Censoring` → RIMOSSO
- ❌ `spawn_verdict_thread()` → RIMOSSO
- ❌ `build_retry_directive()` → RIMOSSO
- ❌ `EscalationFinding` → RIMOSSO
- ❌ Condvar + AtomicBool → RIMOSSO
- ✅ Pi-lens hook `tool_result` → LEGGE shard Censor
- ✅ Pi-lens → INIETT findings nel contesto agente
- ✅ Prompt template → BUDGET DECISION (retry/escalate)
- ✅ Agent loop → vede findings nel prossimo turno

### Nuovo

- 🆕 `readCensorShard()` in pi-lens
- 🆕 `formatAsDiagnostics()` mapping Censor → Diagnostic
- 🆕 Token budgeting (priority strategy)
- 🆕 Prompt template per feedback loop

---

## 11. Checklist Must-Have Truths

Verificare dopo ogni fase:

- [ ] **I runner deterministici continuano a funzionare** (nessuna modifica al Censor core)
- [ ] **Gli shard Censor sono leggibili da pi-lens** (formato JSON valido, path sicuro)
- [ ] **I findings sono iniettati nel contesto agente** (tool_result hook appende testo)
- [ ] **Il mini-coder funziona senza verdict gate** (no più Censoring status)
- [ ] **Il budget decision è nel prompt** (retry/escalate logic nel template)
- [ ] **Gemma async è gestito** (findings un turno dopo = accettabile)
- [ ] **Nessun breaking change per UI frontend** (`censor://findings-updated` ancora emesso)
- [ ] **Nessun breaking change per main coder** (MCP `censor_findings` ancora funzionante)

---

## 12. Riferimenti

- [Pi-Lens Architecture](https://github.com/harms-haus/pi-lens/blob/main/docs/architecture.md) — hook system, daemon, JSON-RPC
- [Zeph LSP Context Injection](https://bug-ops.github.io/zeph/concepts/lsp-context-injection.html) — automatic diagnostics injection
- [Cargo-Context](https://github.com/asmuelle/cargo-context) — MCP server, token budgeting, context pack
- [Claude Code Agent Loop](https://claude-code-from-source.com/ch05-agent-loop/) — stop hooks, error recovery, state management
- [Cursor Stop Hook](https://lirantal.com/blog/cursor-stop-hook-lint-build-verification) — lint/build after each turn
