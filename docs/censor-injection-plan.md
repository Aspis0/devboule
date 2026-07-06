# Piano v5 (post second review)

## Le 5 correzioni

### 🔴 Issue 1: Race async ↔ retry (handshake via ready flag + atomic rename)

v4 perdeva la garanzia "scrivi-poi-leggi" di v3 sincrono. Con Phase A async, il retry
poteva leggere `steer_censor` prima che fosse scritto (vuoto) o peggio, trovare dati
stantii del retry precedente.

**Fix: ready-flag + atomic rename + delete-on-read.**

```rust
// Phase A (detached thread):
let findings = commands::collect_open_findings(root, &modified_files);
let text = format_censor_text(&findings);
let agent_dir = root.join(".aspis-mini").join(&agent_id);
std::fs::create_dir_all(&agent_dir).ok();

// Scrittura atomica: temp → rename (evita read parziali)
let tmp = agent_dir.join(".steer_censor.tmp");
let target = agent_dir.join("steer_censor");
std::fs::write(&tmp, text).ok();
std::fs::rename(&tmp, &target).ok();
// Ready flag DOPO la scrittura atomica — chi legge sa che il file è completo
std::fs::write(agent_dir.join("steer_ready"), "").ok();
```

```rust
// Retry directive creation (executor, prossimo pass):
let agent_dir = root.join(".aspis-mini").join(&agent_id);
let ready_path = agent_dir.join("steer_ready");
let steer_path = agent_dir.join("steer_censor");

let feedback = if ready_path.exists() {
    // Cancella SUBITO il ready flag — previene che un Phase A successivo
    // venga confuso con questo (soprattutto se Phase A è ancora running
    // per un altro giro e scrive un altro steer_censor)
    std::fs::remove_file(&ready_path).ok();
    let text = std::fs::read_to_string(&steer_path).unwrap_or_default();
    std::fs::remove_file(&steer_path).ok();  // consumato
    text
} else {
    String::new()  // Phase A non ha ancora finito → niente findings questo giro
};
// Se feedback è vuoto → il mini riparte senza, li vedrà al prossimo retry.
// Non è perfetto ma è SAFE: meglio "nessun finding" che "finding sbagliati".
```

**Garantito:** mai iniezione stantia (delete-on-read). Mai read parziale (atomic rename).
Se Phase A è lenta → un giro senza findings (recupera al prossimo).

### 🟡 Issue 2: PUSH al Main Coder è morto (essere onesti)

In v3 sincrono, `censor_feedback` poteva andare su `MiniCoderOutcome` → Main Coder lo
vedeva nella risposta MCP. In v4/v5 async, l'outcome è già stampato quando Phase A parte.

**Fix: il doc ammette che il Main Coder usa PULL.**

| Canale | Chi | Push/Pull | Meccanismo |
|---|---|---|---|
| Retry task | Mini-coder | **PUSH** | `steer_censor` via ready-flag |
| Main coder | Main Coder | **PULL** | `censor_findings(project_id, file=...)` MCP tool |
| Activity Console | Umano | **PUSH** | `push_coder_note` + `push_chat` |

Questo è onesto: il PUSH end-to-end funziona per il mini (chi scrive il codice — il
valore di pi). Il Main Coder mantiene il PULL esistente, che già funziona. Lo step 4
"system prompt PULL→PUSH" si applica solo al mini, non al Main Coder.

### 🟡 Issue 3: COARSE policy a 3 modi (disaccoppiato da Pigeon)

```rust
enum CoarsePolicy {
    Off,     // nessun coarse automatico (pi-like minimalism)
    Manual,  // solo trigger umano (UI button / MCP censor_review_now)
    Auto,    // cooldown nell'executor, gated su dirty-flag
}
```

**Default calcolato:** `Auto` quando Pigeon è attivo, `Manual` altrimenti. Ma è un
default sovrascrivibile da config — non incatenato a `pigeon_enabled`.

**Dirty flag:** l'executor setta `coarse_dirty = true` ogni volta che un mini finisce.
Il cooldown timer controlla `dirty && elapsed > COARSE_COOLDOWN`. Dopo il pass,
resetta `dirty = false`. Così non gira a vuoto su progetti idle.

**Pre-push security scan sempre:** prima di `request_git_push`, l'MCP tool lancia
gitleaks + cargo-audit + npm-audit sui file staged. Indipendentemente dalla policy
coarse. Questo è l'anti-regressione vero — i security scanner girano dove contano.

**Badge UI:** "ultimo coarse: 3m fa / 2g fa / mai" nel pannello Censor. Così
"manual" non scivola in "mai" senza che nessuno se ne accorga.

### 🟡 Issue 4: Coalescing FINE (evita runner×5 su retry rapidi)

Un mini che sbaglia e ritenta 5 volte in 20s → tsc/eslint lanciati 5 volte sullo stesso
file. Il watcher aveva il debounce per questo.

**Fix: per-project FINE cooldown.** L'executor tiene `last_fine_run: HashMap<FilePath, Instant>`.
Se il file è stato censorato negli ultimi N secondi (es. 5s), skippa `run_fine_batch`
per quel file — i finding non sono cambiati.

### 🟡 Issue 5: Pulizia `.aspis-mini/<agent_id>/`

**Fix:** già gestito da `finalize_finished_mini` che fa cleanup del result file.
Aggiungere `steer_censor` + `steer_ready` alla stessa cleanup. In più: `kill_all_on_exit`
dell'executor pulisce la dir `.aspis-mini` per tutti gli agent defunti.

## Flusso finale

```
mini finisce (Done)
  │
  ▼
finalize_finished_mini()
  │
  ├── outcome calcolato, stampato (invariato)
  │
  ├── Phase A: std::thread::spawn (ASYNC)
  │     ├── FINE cooldown check: se file già censorato <5s fa → skip
  │     ├── run_fine_batch_no_rail(root, modified_files)   ← FINE deterministici
  │     ├── collect_open_findings(root, modified_files)
  │     ├── atomic write → .aspis-mini/<agent_id>/steer_censor + steer_ready
  │     ├── push_coder_note + push_chat → Activity Console
  │     └── setta coarse_dirty = true
  │
  └── executor loop (ogni 1.5s)
        ├── retry directive creato?
        │     ├── controlla steer_ready → se c'è, consuma & appende al task (PUSH al mini)
        │     └── se non c'è: riparte senza (li vedrà al prossimo giro)
        │
        └── COARSE: se policy==Auto && dirty && cooldown scaduto
              └── spawn run_coarse_pass in background
```

### ⚠️ Limite architetturale: PUSH solo su retry, non su one-shot

Il canale 1 (steer→retry) consegue solo se il mini viene ri-messo in coda per un fix round.
Un mini **one-shot** (primo giro pulito, o ultimo giro senza più retry) non riceve mai i
findings dei FINE deterministici sul proprio output — quei findings restano solo su:

- **Console** (umano li vede)
- **PULL** (Main Coder al prossimo dispatch, via `censor_findings`)

Questo è un limite dell'architettura async scelta (Phase A parte dopo il calcolo dell'outcome).
Il "modello legge e reagisce" di pi vale per i mini **multi-step** (dove c'è retry).
Per gli one-shot il valore è sulla Console + sul Main Coder, non sul mini stesso.
Accettabile — documentato, non nascosto.

## Cosa buttiamo / teniamo

| File | Azione |
|---|---|
| `watch.rs` | DELETE |
| `commands.rs` watcher state | DELETE parziale |
| `orchestrator.rs` FINE event emitter | DELETE parziale |
| `orchestrator.rs` COARSE | TIENI (trigger da executor cooldown) |
| `runners/` (35 file) | TIENI |
| `ledger.rs` (shard) | TIENI (training rail) |
| `gemma.rs` | TIENI (Pigeon censor-pool) |
| `censor_review.rs` (Pigeon) | TIENI |

## Riepilogo righe

| Cosa | Righe |
|---|---|
| DELETE: watch.rs + watcher state + event emitter | ~-1128 |
| Phase A async: run_fine_batch + per-agent + ready-flag + console | +45 |
| COARSE: policy enum + dirty flag + cooldown timer | +35 |
| FINE coalescing: per-file cooldown | +20 |
| Retry: read ready-flag + consume steer | +20 |
| Pre-push security scan (BLOCKING con override) | +40 |
| FINE cooldown sweep (evict > cooldown×4) | +10 |
| **Netto** | **~-968** |

### Decisione: pre-push security scan BLOCCA

Prima di `request_git_push`, l'MCP tool lancia gitleaks + cargo-audit + npm-audit
sui file staged. Se trovano un finding **High**:

- **Blocca il push** — restituisce `{ blocked: true, findings: [...] }`
- Il Main Coder vede il blocco e deve fixare O usare `request_git_push(force: true)`
  per override esplicito (loggato nell'audit trail)
- Finding Medium/Low: warning nel risultato ma non bloccano

Questo è un security gate, non un advisor. L'override esplicito (`force: true`)
garantisce che un blocco non sia mai un deadlock — ma forza la decisione consapevole.

### FINE cooldown sweep

`last_fine_run: HashMap<FilePath, Instant>` cresce senza bound su sessioni lunghe.
Sweep eseguito nell'executor loop: ogni 10 minuti, evict entry più vecchie di
`cooldown × 4` (20s col default 5s). Così la mappa non eccede mai ~qualche decina di entry.

### UX: Tauri clipboard + feedback, no <select> nudo

- Invece di `navigator.clipboard`, usare `invokeBackendCommand("clipboard_write", { text })`
  che funziona nel webview Tauri. Dopo la copia: badge "copied!" per 1.5s.
- Invece di `<select>` HTML nudo, usare un dropdown custom (stile del design system
  cream-*, coerente col resto della UI).
