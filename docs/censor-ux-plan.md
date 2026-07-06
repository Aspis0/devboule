# Piano UX — Censor status bar + COARSE policy

## Problema attuale

La barra dei tool mancanti in `CensorPanelView` è un flex-wrap di bottoni rossi, uno per
ogni tool non installato. Con 35+ runner, se ne mancano 15 diventa una riga gigante di
pulsanti `[❌ clippy] [❌ tsc] [❌ gitleaks] ...` che occupa spazio e fa schifo.

Inoltre: nessun controllo per la policy COARSE (off/manual/auto) e nessuna indicazione
di quando è stata l'ultima run coarse.

## Design nuovo

```
┌─ Action strip (esistente) ──────────────────────────────────────┐
│ 🔄 Review now   🛡 Run final review     3 open findings   ⚙ Disable │
├─ Status bar (NUOVO) ────────────────────────────────────────────┤
│ 🟢 18 ready  🔴 3 missing ▸  ⏱ Coarse: Auto ▼  Last: 3m ago   │
└─────────────────────────────────────────────────────────────────┘
│ (espanso al click su "3 missing")                               │
│ ┌─────────────────────────────────────────────────────────────┐ │
│ │ Copy install cmd:  clippy  tsc  gitleaks                    │ │
│ └─────────────────────────────────────────────────────────────┘ │
```

### Componenti

**1. Status bar (sempre visibile, 1 riga)**

```
🟢 18 ready    — conteggio tool installati e funzionanti
🔴 3 missing ▸ — conteggio tool mancanti, cliccabile per espandere
⏱ Coarse: Auto ▼ — dropdown: Off | Manual | Auto
   Last: 3m ago  — timestamp ultimo coarse (o "never" se mai eseguito)
```

**2. Missing tools (espanso inline, solo al click)**

Sostituisce la riga gigante di bottoni rossi con un riquadro compatto che appare
solo quando l'utente clicca "🔴 3 missing":

```
┌──────────────────────────────────────────────────────────────┐
│ Not installed (click to copy install command):               │
│   clippy  ·  tsc  ·  gitleaks                                │
└──────────────────────────────────────────────────────────────┘
```

Ogni tool è un badge grigio chiaro, cliccabile per copiare il comando di install.
NON rosso — non è un errore, è solo "non presente". Rosso era allarmante.

**3. COARSE policy selector**

Dropdown inline nella status bar:

- `Off` — nessun coarse automatico
- `Manual` — solo trigger umano (Review now)
- `Auto` — cooldown timer nell'executor

Il default è calcolato: `Auto` se Pigeon è attivo, `Manual` altrimenti.
Sovrascrivibile.

**4. Censor LLM status (integrato nella status bar)**

Invece del banner giallo separato, integrato nella barra:

```
🟢 18 ready  🔴 3 missing  🤖 Censor LLM: online  ⏱ Coarse: Auto ▼
```

Se Censor LLM è offline: `🤖 Censor LLM: offline` in grigio.

## Modifiche codice

### Frontend (`CensorPanel.tsx` + `CensorPanelView`)

```tsx
// NUOVO: CensorStatusBar (compact, 1 riga)
function CensorStatusBar({
  readyTools, missingTools, censorLlmAvailable,
  coarsePolicy, lastCoarse, onCoarsePolicyChange,
}: {
  readyTools: string[];
  missingTools: string[];
  censorLlmAvailable: boolean;
  coarsePolicy: "off" | "manual" | "auto";
  lastCoarse: string | null;  // ISO timestamp or null
  onCoarsePolicyChange: (p: "off" | "manual" | "auto") => void;
}) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div>
      {/* Riga compatta */}
      <div className="flex items-center gap-3 text-[11px] text-cream-500">
        <span className="inline-flex items-center gap-1">
          <span className="h-2 w-2 rounded-full bg-emerald" />
          {readyTools.length} ready
        </span>
        {missingTools.length > 0 && (
          <button
            onClick={() => setExpanded(!expanded)}
            className="inline-flex items-center gap-1 hover:text-cream-700"
          >
            <span className="h-2 w-2 rounded-full bg-cream-300" />
            {missingTools.length} missing
            <ChevronDown className={cn("h-3 w-3 transition", expanded && "rotate-180")} />
          </button>
        )}
        <span className="text-cream-300">·</span>
        <span className="inline-flex items-center gap-1">
          {censorLlmAvailable ? "🤖" : "💤"}
          Censor LLM: {censorLlmAvailable ? "online" : "offline"}
        </span>
        <span className="text-cream-300">·</span>
        <CoarsePolicySelector value={coarsePolicy} onChange={onCoarsePolicyChange} />
        {lastCoarse && (
          <>
            <span className="text-cream-300">·</span>
            <span className="text-cream-400">Last: {formatTimeAgo(lastCoarse)}</span>
          </>
        )}
      </div>

      {/* Espanso: tool mancanti */}
      {expanded && missingTools.length > 0 && (
        <div className="mt-2 rounded-lg border border-cream-200 bg-cream-50 px-3 py-2">
          <p className="mb-1.5 text-[10px] text-cream-400">
            Not installed — click to copy install command:
          </p>
          <div className="flex flex-wrap gap-1">
            {missingTools.map((t) => {
              const hint = installHintFor(t);
              return (
                <button
                  key={t}
                  onClick={() => hint && void navigator.clipboard?.writeText(hint).catch(() => {})}
                  title={hint ?? `No install hint for ${t}`}
                  className="rounded border border-cream-200 bg-white px-1.5 py-0.5 text-[10px] text-cream-600 hover:bg-cream-100"
                >
                  {t}
                </button>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}

// NUOVO: CoarsePolicySelector (dropdown inline)
function CoarsePolicySelector({
  value, onChange,
}: {
  value: string;
  onChange: (v: "off" | "manual" | "auto") => void;
}) {
  return (
    <span className="inline-flex items-center gap-1">
      ⏱ Coarse:
      <select
        value={value}
        onChange={(e) => onChange(e.target.value as "off" | "manual" | "auto")}
        className="rounded border border-cream-200 bg-white px-1 py-0 text-[11px] text-cream-600"
      >
        <option value="off">Off</option>
        <option value="manual">Manual</option>
        <option value="auto">Auto</option>
      </select>
    </span>
  );
}
```

### Backend

**Nuovo campo in `CensorStatus`:**

```rust
pub struct CensorStatus {
    // ... existing fields ...
    pub coarse_policy: String,       // "off" | "manual" | "auto"
    pub last_coarse_run: Option<String>, // ISO timestamp
}
```

**Nuovi comandi Tauri:**

- `set_coarse_policy(project_id, policy)` — persiste nel project metadata
- `censor_status` già restituisce i tool; aggiungere i due campi sopra

**COARSE cooldown timer nell'executor** (dal piano v5):

```rust
// In run_pass, dopo aver processato i directive:
if coarse_policy == Auto && coarse_dirty && last_coarse.elapsed() > COARSE_COOLDOWN {
    spawn_coarse_pass();
    last_coarse = Instant::now();
    coarse_dirty = false;
}
```

### UX details

**Copy via Tauri clipboard, not `navigator.clipboard`:**
Il webview Tauri può silenziare `navigator.clipboard.writeText`. Usare:

```tsx
await invokeBackendCommand("clipboard_write", { text: hint });
setCopied(t);  // badge "copied!" per 1.5s
```

**Dropdown custom, non `<select>` nudo:**
Il `<select>` HTML nudo stona col design system cream-*. Usare un dropdown
custom coerente col resto della UI (menu popover, stile AgentSelector/ModelSelector).

## Cosa buttare dal CensorPanelView attuale

Rimuovere:

- Il banner Censor LLM giallo separato → integrato nella status bar
- La sezione `tool-absent hint` col flex-wrap di bottoni rossi → sostituita dall'espansione inline
