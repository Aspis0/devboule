# Devboule — Polis Map

## Documento di Progettazione Completo

---

## Visione

Una mappa isometrica vivente che rappresenta l'intera infrastruttura di Devboule come una città romano-imperiale. Ogni file sorgente è un edificio, ogni import è una strada, ogni agente IA attivo è un cittadino visibile sul territorio. La mappa non è decorazione — è il pannello di controllo principale del progetto.

**Riferimento visivo:** Pharaoh (1999), Zeus: Master of Olympus (2000). Movimento fluido e continuo, palette calda crema/terracotta/avorio, edifici isometrici 2.5D con ombre e luci geometriche.

---

## Stack Tecnico

| Layer | Tecnologia |
|---|---|
| Shell applicativa | Tauri 2 (Rust backend) |
| Renderer mappa | PixiJS 8 (WebGL + Canvas fallback) |
| Frontend | React 18 + TypeScript |
| Classificazione semantica | Oracle interno (Qwen + LanceDB) |
| Parser statico | Rust (tree-sitter o regex per import) |
| Stato globale | Zustand |
| Infrastruttura esterna | Scaleway IAM API, Cloudflare API |

---

## Architettura dei Dati

### Schema JSON principale — `CityState`

```typescript
interface CityState {
  version: number;
  project_name: string;
  era: string;                    // "Alpha", "Beta", etc.
  generated_at: string;           // ISO timestamp
  grid_size: { w: number; h: number };  // es. 128x128
  districts: District[];
  buildings: Building[];
  roads: Road[];
  agents: Agent[];
  external_services: ExternalService[];
}

interface District {
  district_id: string;
  name: string;
  type: "cloudflare_worker" | "scaleway_zone" | "scripts" | "core";
  bounds: { x: number; y: number; w: number; h: number };
  wall_style: "roman_wall" | "aqueduct" | "palisade" | "none";
  color_accent: string;
}

interface Building {
  file_id: string;               // UUID stabile, persistito in .aspis-meta.json
  file_path: string;
  district_id: string;
  purpose: BuildingPurpose;
  lines_of_code: number;
  visual_tier: VisualTier;       // determina dimensione, non qualità
  coords: { x: number; y: number };
  status: "normal" | "burning" | "active" | "offline";
  label: string;                 // nome breve per display
  description: string;           // generato da Oracle
  last_modified: string;
  agent_present?: string;        // agent_id se qualcuno ci sta lavorando
  kanban_card_id?: string;       // card correntemente associata (da MCP)
  untracked_change?: boolean;    // modifica rilevata da Rust senza card Kanban
}

// Lo slug è una STABLE ENGLISH machine key (la source of truth sul wire).
// La display label "English (Greek)" è un helper di presentazione (vedi
// PURPOSE_LABELS / purposeLabel in src/types/city.ts) — NON serializzata.
type BuildingPurpose =
  | "townhall"       // Town Hall (Bouleuterion) — Cloudflare worker entry, config critici
  | "temple"         // Temple (Naos) — Oracle queries, LanceDB, prompt layer
  | "fortress"       // Fortress (Phrourion) — Agent core, orchestrator, dispatcher
  | "market"         // Market (Agora) — API clients, integrazioni esterne (Scaleway API)
  | "tower"          // Tower (Pyrgos) — Config: tauri.conf.json, tsconfig, wrangler.toml
  | "house"          // House (Oikos) — UI components, file generici (default onesto)
  | "warehouse"      // Warehouse (Apotheke) — Object store interface, storage layer
  | "workshop"       // Workshop (Ergasterion) — Scripts, utility, tools
  | "conduit"        // Conduit (Agogos) — Middleware, proxy, routing layer
  | "baths"          // Baths (Balaneion) — Auth, session, token management
  | "theater"        // Theater (Theatron) — Logging, telemetry, monitoring
  | "lighthouse"     // Lighthouse (Pharos) — Entry point, main, index — il punto di ingresso visibile da lontano
  | "harbor"         // Harbor (Limen) — Upload/download, file I/O, stream
  | "library"        // Library (Bibliotheke) — Constants, types, enums, shared interfaces
  | "unknown"        // Unclassified — fallback onesto / slug introdotti da Oracle
  | string;          // Estensibile: Oracle può classificare nuovi tipi non previsti

type VisualTier =
  | "kalybe"         // 0–200 righe (hut)
  | "oikia"          // 201–600 righe (house)
  | "synoikia"       // 601–1200 righe (tenement)
  | "megaron"        // 1201–2500 righe (hall)
  | "mnemeion";      // > 2500 righe (monument)

interface Road {
  road_id: string;
  from: string;                  // file_id
  to: string;                    // file_id
  type: "import" | "semantic" | "infrastructure";
  style: "terra_battuta" | "lastricata" | "acquedotto";
  weight: number;                // 1–5, spessore visivo
}

interface Agent {
  agent_id: string;
  // Slug English stabile; display label "English (Greek)" via agentTypeLabel.
  type: "orchestrator" | "coder" | "verifier" | "augur";
  status: "idle" | "walking" | "working" | "reviewing" | "surveying";
  current_file_id: string | null;
  current_task: string | null;
  color: string;                 // colore dell'omino e del glow di sfondo sull'edificio
}

interface ExternalService {
  service_id: string;
  provider: "scaleway" | "cloudflare";
  type: "container" | "gpu_vm" | "cpu_vm" | "object_store" | "llm_api" | "worker";
  name: string;
  status: "running" | "stopped" | "spawning" | "error";
  coords: { x: number; y: number };
  spawnable: boolean;
}
```

### ID Stabile — `.aspis-meta.json`

Nella root del progetto, file non committato (in `.gitignore`):

```json
{
  "file_ids": {
    "src/oracle/client.ts": "uuid-a1b2c3",
    "src/main.tsx": "uuid-d4e5f6"
  }
}
```

Al primo scan, Rust genera gli UUID e li persiste. Agli scan successivi, usa quelli esistenti. Rinominare un file non distrugge la storia.

---

## Backend Rust — Comandi Tauri

### `generate_city_state(project_path: String) -> CityState`

**Fase 1 — Scansione file (Rust puro)**

1. Walk ricorsivo della cartella progetto
2. Filtra: mantieni solo `.ts`, `.tsx`, `.rs`, `.kt`, `.toml`, `.json` critici
3. Escludi pattern: `node_modules`, `dist`, `build`, `.d.ts`, `*.test.*`, `*.spec.*`, `*.md`, `docs/`
4. Per ogni file:
   - Conta righe → `lines_of_code` → `visual_tier`
   - Legge i primi 50 token per hint semantico
   - Estrae tutti gli import statement (regex su `import ... from` e `require(`)
   - Legge `tsconfig.json` per risolvere path aliases (`@/` → `src/`)

**Fase 2 — Classificazione (Oracle)**

- Invia a Oracle batch di file con: path + prime 30 righe + import list
- Oracle restituisce `purpose` e `description` breve per ognuno
- Fallback se Oracle non disponibile: classificazione euristica su path e nome file

**Fase 3 — Strade**

- **Import diretto** (Rust): import risolti → road `type: "import"`, `style: "lastricata"`, `weight` proporzionale a quante volte il file è importato
- **Semantico** (Oracle): embedding similarity > 0.82 tra file dello stesso distretto → road `type: "semantic"`, `style: "terra_battuta"`, `weight: 1`
- **Infrastrutturale** (parser config): binding in `wrangler.toml`, env variables con URL → road `type: "infrastructure"`, `style: "acquedotto"`

**Fase 4 — Layout automatico dinamico**

- La griglia non ha dimensione fissa — cresce in base al numero di file scansionati
- Formula base: `grid_size = ceil(sqrt(n_buildings * SPACING_FACTOR))` con `SPACING_FACTOR = 6` (ogni edificio occupa in media 6×6 tile di "respiro")
- Esempio: 30 file → griglia ~44×44. 100 file → griglia ~78×78. 400 file → griglia ~156×156
- Dentro ogni distretto: force-directed layout con repulsione minima tra edifici — nessuna sovrapposizione, ma spazio libero intorno a ogni edificio
- I distretti crescono proporzionalmente ai file che contengono — un worker con 3 file occupa meno spazio di uno con 12
- Persiste coordinate in `.aspis-meta.json` per stabilità tra scan successivi — un nuovo file appare vicino agli edifici correlati, non in posizione casuale

**Fase 5 — Infrastruttura Scaleway**

- Chiama Scaleway API con IAM key (configurata in Tauri settings)
- Recupera containers, VM, object stores attivi
- Aggiunge come `ExternalService` con status live

---

### Comandi Tauri Atomici

```rust
// Stato file
trigger_file_disaster(file_id: String, disaster_type: String) -> Result<()>
resolve_file_disaster(file_id: String) -> Result<()>

// Agenti
set_agent_location(agent_id: String, file_id: Option<String>, task: Option<String>) -> Result<()>
update_agent_status(agent_id: String, status: String) -> Result<()>

// Infrastruttura Scaleway
spawn_scaleway_resource(service_id: String) -> Result<ExternalService>
stop_scaleway_resource(service_id: String) -> Result<()>
refresh_scaleway_status() -> Result<Vec<ExternalService>>

// Era / Prestige
reset_city_to_new_era(new_era_name: String) -> Result<()>
// Archivia CityState corrente come snapshot immutabile
// Erge monumento con statistiche dell'era precedente
// Reset coordinate e visual_tier al minimo

// Note e log
append_city_note(file_id: String, log_text: String) -> Result<()>
```

Tutti i comandi operano su `Arc<Mutex<CityState>>` condiviso — nessuna race condition.

---

## Frontend PixiJS — Architettura del Renderer

### Setup

```typescript
// PolisMap.tsx
const app = new PIXI.Application({
  width: window.innerWidth,
  height: window.innerHeight,
  backgroundColor: 0xF4F0E6,  // crema base
  resolution: window.devicePixelRatio,
  antialias: true,
});

// Layer stack (z-order esplicito)
const layers = {
  terrain:    new PIXI.Container(),  // terreno, acqua decorativa
  districts:  new PIXI.Container(),  // mura distrettuali
  roads:      new PIXI.Container(),  // strade
  buildings:  new PIXI.Container(),  // edifici (z-sorted per y)
  agents:     new PIXI.Container(),  // omini agenti + glow di sfondo
  effects:    new PIXI.Container(),  // fuoco, particelle
  ui:         new PIXI.Container(),  // label, tooltip
};
```

### Proiezione Isometrica

```typescript
// Tile size: 96px wide, 48px tall (rapporto 2:1 classico)
const TILE_W = 96;
const TILE_H = 48;

function cartToIso(x: number, y: number): { sx: number; sy: number } {
  return {
    sx: (x - y) * (TILE_W / 2),
    sy: (x + y) * (TILE_H / 2),
  };
}

function isoToCart(sx: number, sy: number): { x: number; y: number } {
  return {
    x: (sx / (TILE_W / 2) + sy / (TILE_H / 2)) / 2,
    y: (sy / (TILE_H / 2) - sx / (TILE_W / 2)) / 2,
  };
}
```

### Zoom e Pan

```typescript
// Viewport con PixiJS viewport plugin (pixi-viewport)
// oppure implementazione manuale con DragPlugin + PinchPlugin
const viewport = new Viewport({
  screenWidth: window.innerWidth,
  screenHeight: window.innerHeight,
  worldWidth: 128 * TILE_W,
  worldHeight: 128 * TILE_H,
  events: app.renderer.events,
});

viewport
  .drag()
  .pinch()
  .wheel()
  .clampZoom({ minScale: 0.15, maxScale: 3.0 });
```

### LOD (Level of Detail)

| Zoom | Cosa si vede |
|---|---|
| < 0.3 | Solo sagome colorate per distretto, nessun label |
| 0.3 – 0.7 | Edifici con forme base, label abbreviati |
| 0.7 – 1.5 | Edifici completi con dettagli geometrici |
| > 1.5 | Edifici + animazioni sottili + label completi |

---

## Rendering Edifici — Grafica Procedurale

Tutti gli edifici sono disegnati proceduralmente con `PIXI.Graphics`. Nessuna sprite sheet esterna — puro codice geometrico. Questo significa che la palette è sempre coerente e gli edifici scalano perfettamente.

### Palette

```typescript
const PALETTE = {
  cream:        0xF4F0E6,
  ivory:        0xEDE8D8,
  sand_dark:    0xC8B89A,
  terracotta:   0xC17A5A,
  terracotta_dark: 0x8B4E32,
  shadow:       0x6B5A48,
  stone:        0xA89880,
  stone_dark:   0x7A6855,
  white_marble: 0xF8F4EC,
  gold_accent:  0xD4A843,
};
```

### Funzione base 2.5D

Ogni edificio ha tre facce: top (tetto), left (ombra), right (luce).

```typescript
function drawIsometricBox(
  g: PIXI.Graphics,
  x: number, y: number,
  w: number, h: number, depth: number,
  colorTop: number, colorLeft: number, colorRight: number
) {
  // Faccia superiore (tetto)
  g.beginFill(colorTop);
  g.moveTo(x, y);
  g.lineTo(x + w, y - w * 0.5);
  g.lineTo(x + w, y - w * 0.5 - depth);
  g.lineTo(x, y - depth);
  g.closePath();

  // Faccia sinistra (ombra)
  g.beginFill(colorLeft);
  g.moveTo(x, y);
  g.lineTo(x, y - depth);
  g.lineTo(x - w * 0.5, y - depth + w * 0.25);
  g.lineTo(x - w * 0.5, y + w * 0.25);
  g.closePath();

  // Faccia destra (luce)
  g.beginFill(colorRight);
  g.moveTo(x + w, y - w * 0.5);
  g.lineTo(x + w, y - w * 0.5 - depth);
  g.lineTo(x + w * 0.5, y - depth + w * 0.25);  // corretto
  g.lineTo(x + w * 0.5, y + w * 0.25);
  g.closePath();
}
```

### Registro Edifici — Sistema Estensibile

Il renderer non ha una lista hardcoded di tipi. Ogni `BuildingPurpose` si mappa a un profilo visivo in un registro separato. Oracle può introdurre nuovi tipi in qualsiasi momento — il renderer li disegnerà con il profilo più vicino come fallback, oppure con un profilo generato automaticamente.

```typescript
interface BuildingProfile {
  purpose: string;
  base_color_top:   number;
  base_color_left:  number;
  base_color_right: number;
  roof_style: "flat" | "pitched" | "cone" | "dome" | "merlon";
  has_columns: boolean;
  has_courtyard: boolean;      // atrio interno per edifici grandi
  landmark_element?: string;   // "flame" | "flag" | "antenna" | "beacon"
  min_tier: VisualTier;        // tier minimo per questo tipo (es. il lighthouse è sempre almeno synoikia)
}

// Le chiavi sono gli slug English stabili (display label "English (Greek)"
// via purposeLabel). Lo slug è la machine key; la label è solo presentazione.
const BUILDING_REGISTRY: Record<string, BuildingProfile> = {
  townhall:    { roof_style: "flat",    has_columns: true,  landmark_element: "flag",    ... },  // Town Hall (Bouleuterion)
  temple:      { roof_style: "pitched", has_columns: true,  landmark_element: "flame",   ... },  // Temple (Naos)
  fortress:    { roof_style: "merlon",  has_columns: false, landmark_element: undefined, ... },  // Fortress (Phrourion)
  market:      { roof_style: "flat",    has_columns: false, landmark_element: undefined, ... },  // Market (Agora)
  tower:       { roof_style: "cone",    has_columns: false, landmark_element: "beacon",  ... },  // Tower (Pyrgos)
  lighthouse:  { roof_style: "dome",    has_columns: false, landmark_element: "beacon",  min_tier: "synoikia" },  // Lighthouse (Pharos)
  library:     { roof_style: "flat",    has_columns: true,  landmark_element: undefined, ... },  // Library (Bibliotheke)
  theater:     { roof_style: "pitched", has_columns: true,  has_courtyard: true,         ... },  // Theater (Theatron)
  baths:       { roof_style: "dome",    has_columns: false, has_courtyard: true,         ... },  // Baths (Balaneion)
  harbor:      { roof_style: "flat",    has_columns: false, landmark_element: "antenna", ... },  // Harbor (Limen)
  // Fallback per tipi non previsti — Oracle ha classificato qualcosa di nuovo
  unknown:     { roof_style: "flat",    has_columns: false, landmark_element: undefined, ... },  // Unclassified
};

function getProfile(purpose: string): BuildingProfile {
  return BUILDING_REGISTRY[purpose] ?? BUILDING_REGISTRY['unknown'];
}
```

**Come Oracle introduce nuovi tipi:**
Se Oracle classifica un file come `"laboratorio"` (tipo non ancora nel registro), il sistema lo aggiunge con un profilo generato da similarity con i tipi esistenti. Al prossimo avvio, il profilo è già persistito in `.aspis-meta.json` e l'utente può raffinarlo manualmente se vuole.

**Nuovi tipi previsti al crescere del progetto Devboule:**

| Tipo futuro | File tipici | Edificio suggerito |
|---|---|---|
| `laboratorio` | Pipeline ML, preprocessing dati bio | Struttura con tetto a shed industriale |
| `osservatorio` | Analytics, dashboard data | Torre con cupola trasparente (cerchio) |
| `arco_di_trionfo` | File di versioning, changelog | Struttura decorativa, non funzionale |
| `baths` | Auth, session management | Edificio con cupola e cortile |
| `anfiteatro` | Event bus, message queue | Struttura circolare aperta |

---

**House (Oikos) — file generico UI** _(slug: `house`)_
Prisma semplice, tetto leggermente spiovente color terracotta, finestre geometriche minuscole sulla faccia destra. Varianti sottili per visual_tier.

**Town Hall (Bouleuterion) — Cloudflare Worker entry** _(slug: `townhall`)_
Struttura larga e bassa con colonne frontali verticali disegnate come rettangoli stretti. Tetto piano con cornicione. Bandierina color arancio Cloudflare in cima.

**Temple (Naos) — Oracle / LanceDB** _(slug: `temple`)_
Scalinata frontale (gradini geometrici), colonne più alte, frontone triangolare sopra l'ingresso. Palette avorio + marmo bianco. Piccola fiamma dorata al centro del tetto.

**Fortress (Phrourion) — Agent core** _(slug: `fortress`)_
Mura merlate attorno a struttura centrale, torricino agli angoli. Colore pietra scura. Porta ad arco sull'ingresso.

**Market (Agora) — API clients, Scaleway API** _(slug: `market`)_
Struttura con loggiato aperto, awning color terracotta, casse geometriche davanti. Animazione sottile: ombra oscillante dell'awning.

**Tower (Pyrgos) — Config critici** _(slug: `tower`)_
Alta e stretta, proporzionalmente diversa da tutti gli altri. Tetto conico. Riconoscibile a colpo d'occhio dalla silhouette.

**Warehouse (Apotheke) — Object Store** _(slug: `warehouse`)_
Struttura larga, bassa, tetto a shed. Portone grande. Per Scaleway Object Store — dimensione proporzionale ai GB configurati se disponibile via API.

**Workshop (Ergasterion) — Scripts** _(slug: `workshop`)_
Edificio spartano, mattoni visibili sulla faccia laterale (linee orizzontali sottili), colore sabbia scura.

---

## Strade

## Strade — Tre Stili Visivi

Le strade sono il sistema nervoso della mappa. Ogni tipo di collegamento ha uno stile visivo isometrico distinto e riconoscibile — niente tratteggi, tutto geometria piena.

### Assegnazione automatica

| Tipo collegamento | Stile strada | Logica |
|---|---|---|
| `import` diretto | `lastricata` | Import risolto staticamente da Rust |
| `semantic` (Oracle) | `terra_battuta` | Similarità embedding > 0.82 |
| `infrastructure` (config) | `acquedotto` | Binding in wrangler.toml, env URL |

### Terra Battuta — collegamento semantico

Il percorso più grezzo. Due file che si parlano concettualmente ma non si importano direttamente.

Visivamente: striscia color sabbia chiara (`#D4C9A8`), bordi irregolari ottenuti disegnando tre linee parallele leggermente offset con alpha diversi (0.9 / 0.6 / 0.3). Nessuna texture ripetuta — tutto procedurale. Larghezza: 4–6px base.

```typescript
function drawTerraBattuta(g: PIXI.Graphics, from: IsoPoint, to: IsoPoint, weight: number) {
  // Linea centrale più spessa, opaca
  g.lineStyle(weight * 1.5 + 2, 0xD4C9A8, 0.9);
  g.moveTo(from.x, from.y); g.lineTo(to.x, to.y);
  // Bordo sinistro, più sottile e trasparente — effetto terreno calpestato
  g.lineStyle(weight * 0.8, 0xC4B898, 0.5);
  g.moveTo(from.x - 2, from.y - 1); g.lineTo(to.x - 2, to.y - 1);
  // Bordo destro
  g.lineStyle(weight * 0.8, 0xE4D9B8, 0.4);
  g.moveTo(from.x + 2, from.y + 1); g.lineTo(to.x + 2, to.y + 1);
}
```

### Lastricata — import diretto

La strada romana classica. Import esplicito = connessione certificata, merita le pietre.

Visivamente: serie di rettangoli isometrici alternati color pietra chiara (`#C8B89A`) e pietra scura (`#A89878`) disposti lungo il percorso, con linee di giuntura sottili. L'effetto è una pavimentazione a ciottoli allungati. Larghezza: 8–12px base, spessore proporzionale al `weight` (quante volte il file è importato).

```typescript
function drawLastricata(g: PIXI.Graphics, from: IsoPoint, to: IsoPoint, weight: number) {
  const roadWidth = 6 + weight * 2;
  const stoneLength = 14;
  const totalDist = dist(from, to);
  const steps = Math.floor(totalDist / stoneLength);

  for (let i = 0; i < steps; i++) {
    const t0 = i / steps;
    const t1 = (i + 0.85) / steps;       // gap tra pietra e pietra
    const color = i % 2 === 0 ? 0xC8B89A : 0xB0A080;

    const p0 = lerp(from, to, t0);
    const p1 = lerp(from, to, t1);

    // Rettangolo orientato lungo la direzione della strada
    drawOrientedRect(g, p0, p1, roadWidth, color, 0.92);
  }

  // Bordo laterale continuo — cunei di contenimento
  g.lineStyle(1.5, 0x7A6855, 0.7);
  drawParallelLine(g, from, to, roadWidth / 2);
  drawParallelLine(g, from, to, -roadWidth / 2);
}
```

### Acquedotto — collegamento infrastrutturale

Il collegamento tra mondi diversi: un Worker Cloudflare che chiama un container Scaleway, un config che punta a un endpoint esterno. Fisicamente diverso da una strada — è una struttura sopraelevata.

Visivamente: archi isometrici ripetuti lungo il percorso. Ogni arco è un piccolo trapezio con due "gambe" verticali ai lati e la volta in cima, colore pietra chiara con ombra laterale. Tra un arco e l'altro, il canale di scorrimento — linea stretta color acqua desaturata (`#8AAABB`). È immediatamente riconoscibile come infrastruttura, non come strada.

```typescript
function drawAcquedotto(g: PIXI.Graphics, from: IsoPoint, to: IsoPoint) {
  const archSpacing = 20;
  const archHeight = 10;
  const archWidth = 8;
  const totalDist = dist(from, to);
  const numArches = Math.floor(totalDist / archSpacing);

  // Canale in cima — linea acqua
  g.lineStyle(3, 0x8AAABB, 0.8);
  g.moveTo(from.x, from.y - archHeight);
  g.lineTo(to.x, to.y - archHeight);

  // Archi strutturali
  for (let i = 0; i <= numArches; i++) {
    const t = i / numArches;
    const p = lerp(from, to, t);

    // Gamba sinistra
    g.lineStyle(2.5, 0xC8B89A, 0.9);
    g.moveTo(p.x - archWidth / 2, p.y);
    g.lineTo(p.x - archWidth / 2, p.y - archHeight);

    // Gamba destra
    g.moveTo(p.x + archWidth / 2, p.y);
    g.lineTo(p.x + archWidth / 2, p.y - archHeight);

    // Volta superiore (arco semplificato — linea orizzontale isometrica)
    g.lineStyle(2, 0xA89878, 0.85);
    g.moveTo(p.x - archWidth / 2, p.y - archHeight);
    g.lineTo(p.x + archWidth / 2, p.y - archHeight);
  }
}
```

### RenderTexture statico (performance)

Le strade non cambiano tra un frame e l'altro — solo quando cambia la topologia del codice. Bake su `RenderTexture`:

```typescript
class RoadLayer {
  private texture: PIXI.RenderTexture;
  private sprite: PIXI.Sprite;
  private dirty = true;

  rebake(roads: Road[], buildings: Building[]) {
    if (!this.dirty) return;
    // Disegna tutte le strade su RenderTexture una volta sola
    app.renderer.render(roadGraphics, { renderTexture: this.texture });
    this.dirty = false;
  }

  markDirty() { this.dirty = true; }  // chiamato solo su nuovo scan
}
```

A ogni frame la GPU disegna una singola sprite pre-baked. Costo: quasi zero.

---

## Culling Visivo — Performance Industriale

Il segreto dei city builder isometrici anni 2000 è che non disegnano mai tutta la mappa — solo quello che il giocatore sta guardando in quel momento. Con una mappa 128×128 e centinaia di edifici, questo è non negoziabile.

### Principio

A ogni frame, il renderer controlla quali edifici sono dentro i confini visibili del viewport. Quelli fuori vengono saltati completamente — zero calcoli GPU, zero draw calls. Che la mappa abbia 200 o 2000 edifici, la GPU integrata Intel disegna sempre e solo i ~30–50 edifici visibili sullo schermo.

### Spatial Partitioning — Chunk System

Il culling su singoli oggetti è costoso: 1000 `.visible = false` per frame è comunque 1000 operazioni. La soluzione è raggruppare gli edifici in **chunk** e fare culling sui chunk interi.

```typescript
const CHUNK_SIZE = 16; // 16×16 tile per chunk → 64 chunk su mappa 128×128

class ChunkManager {
  private chunks: Map<string, PIXI.Container> = new Map();

  private getChunkKey(tileX: number, tileY: number): string {
    const cx = Math.floor(tileX / CHUNK_SIZE);
    const cy = Math.floor(tileY / CHUNK_SIZE);
    return `${cx},${cy}`;
  }

  addBuilding(building: Building, sprite: PIXI.Container) {
    const key = this.getChunkKey(building.coords.x, building.coords.y);
    if (!this.chunks.has(key)) {
      const container = new PIXI.Container();
      this.chunks.set(key, container);
      buildingLayer.addChild(container);
    }
    this.chunks.get(key)!.addChild(sprite);
  }

  getChunkBounds(key: string): PIXI.Rectangle {
    const [cx, cy] = key.split(',').map(Number);
    const { sx: x0, sy: y0 } = cartToIso(cx * CHUNK_SIZE, cy * CHUNK_SIZE);
    const { sx: x1, sy: y1 } = cartToIso((cx + 1) * CHUNK_SIZE, (cy + 1) * CHUNK_SIZE);
    return new PIXI.Rectangle(
      Math.min(x0, x1) - TILE_W,
      Math.min(y0, y1) - TILE_H * 4,  // margine per edifici alti
      Math.abs(x1 - x0) + TILE_W * 2,
      Math.abs(y1 - y0) + TILE_H * 6
    );
  }
}
```

Il loop di culling nel ticker confronta 64 bounding box, non 1000 edifici — è microsecondi:

```typescript
app.ticker.add(() => {
  const visibleBounds = viewport.getVisibleBounds();  // rettangolo screen space

  for (const [key, chunk] of chunkManager.chunks) {
    chunk.visible = visibleBounds.intersects(chunkManager.getChunkBounds(key));
  }
});
```

A qualsiasi zoom il viewport copre al massimo 6–9 chunk contemporaneamente. Gli altri 55+ sono nascosti in un'operazione sola.

### LOD per Zoom

Combinato al culling, il LOD riduce ulteriormente il lavoro per chunk lontani ma ancora visibili:

```typescript
viewport.on('zoomed', () => {
  const scale = viewport.scale.x;
  for (const [, chunk] of chunkManager.chunks) {
    if (!chunk.visible) continue;
    // Sotto soglia: disegna solo sagoma colorata, nasconde dettagli
    chunk.getChildByName('details')!.visible = scale > 0.7;
    chunk.getChildByName('labels')!.visible  = scale > 0.9;
    chunk.getChildByName('agents')!.visible  = scale > 0.5;
  }
});
```

### RenderTexture per Strade e Terreno

Strade e terreno sono statici tra un scan e l'altro. Bake su `RenderTexture` una volta sola, poi è una singola sprite in GPU memory:

```typescript
class StaticLayerBaker {
  private roadTexture:    PIXI.RenderTexture;
  private terrainTexture: PIXI.RenderTexture;
  private roadsDirty    = true;
  private terrainDirty  = true;

  bakeIfNeeded() {
    if (this.roadsDirty) {
      app.renderer.render(roadGraphics, { renderTexture: this.roadTexture });
      this.roadsDirty = false;
    }
    if (this.terrainDirty) {
      app.renderer.render(terrainGraphics, { renderTexture: this.terrainTexture });
      this.terrainDirty = false;
    }
  }

  // Chiamato solo quando arriva un nuovo CityState dal backend
  invalidate() {
    this.roadsDirty    = true;
    this.terrainDirty  = true;
  }
}
```

A ogni frame normale: la GPU disegna due sprite pre-baked per strade e terreno. Costo computazionale: quasi zero.

### Risultato Atteso

| Scenario | Draw calls per frame (senza culling) | Draw calls per frame (con culling) |
|---|---|---|
| Mappa 128×128, 400 edifici, zoom out | ~400 | ~25–40 |
| Mappa 128×128, zoom in su distretto | ~400 | ~15–25 |
| Strade (500 segmenti) | ~500 | 1 (RenderTexture) |

GPU Intel integrata: stabile a 60fps in tutti gli scenari realistici.

---

Ogni distretto ha un rettangolo di mura disegnato con `drawIsometricBox` molto sottile (h = 1 tile). I sottomoduli di un Worker Cloudflare sono edifici dentro le mura del distretto padre.

```typescript
function drawDistrictWalls(district: District) {
  const g = new PIXI.Graphics();
  const { x, y, w, h } = district.bounds;
  // Disegna perimetro isometrico
  // Apertura (porta) sul lato sud-est per ingresso visivo
  // Label del distretto in alto a sinistra delle mura
}
```

**Zone sulla griglia — posizionamento dinamico:**

I distretti non hanno coordinate fisse. Al primo scan, Rust calcola il layout ottimale in base a quanti file esistono per categoria. I distretti vengono posizionati con un algoritmo a spirale dal centro verso l'esterno — il Core sempre al centro, i distretti più grandi (più file) vicini, quelli più piccoli in periferia.

```
Centro mappa → Core / Agent Fortress
Anello 1     → Cloudflare Workers (distretti multipli, dimensione variabile)
Anello 1     → Oracle / LanceDB (adiacente al Core per le strade corte)
Anello 2     → Scaleway Zone
Anello 2     → UI / Frontend
Periferia    → Scripts, utility (sparse, non murate)
Margini      → Monumenti ere precedenti (immutabili)
```

Tra un distretto e l'altro: **spazio aperto intenzionale** — campi, terreno vuoto navigabile. Non tutto deve essere costruito. Lo spazio libero è parte del design visivo e lascia room per crescita futura senza dover rifare il layout.

---

## Agenti — Movimento e Lavoro

### Filosofia

Gli agenti camminano sulle strade esistenti, arrivano all'edificio target, e martellano. Niente pathfinding complesso, niente animazioni scheletriche. Tre regole ferree:

1. **Mai fuori dalle strade** — l'agente si muove solo lungo nodi della road graph
2. **Teletrasporto se non c'è strada** — se il file target non è raggiungibile via strade, l'agente sparisce e riappare con fade (non vola, non attraversa edifici)
3. **L'animazione di lavoro è un'icona, non uno sprite animato**

---

### Rappresentazione Visiva

Ogni agente è un omino geometrico minimalista disegnato proceduralmente — cerchio per la testa, rettangolo per il corpo, due linee per le gambe. Tutto in `PIXI.Graphics`, nessuna sprite sheet.

```typescript
const AGENT_COLORS = {
  orchestrator: 0x4A9EFF,    // Orchestrator (Strategos) — blu freddo, coordina, non tocca codice
  coder:        0xFFB347,    // Coder (Tekton) — arancio caldo, scrive
  verifier:     0x7FD47F,    // Verifier (Episkopos) — verde salvia, legge e controlla
};

const AGENT_SIZE = 8; // px, scala con zoom

function drawAgent(g: PIXI.Graphics, color: number, state: AgentState) {
  g.clear();

  // Ombra sotto i piedi — ellisse piatta, evita effetto "che vola"
  g.beginFill(0x000000, 0.18);
  g.drawEllipse(0, AGENT_SIZE * 0.9, AGENT_SIZE * 0.7, AGENT_SIZE * 0.2);
  g.endFill();

  // Corpo
  g.beginFill(color, 1.0);
  g.drawRect(-AGENT_SIZE * 0.3, 0, AGENT_SIZE * 0.6, AGENT_SIZE * 0.55);
  g.endFill();

  // Testa
  g.beginFill(color, 1.0);
  g.drawCircle(0, -AGENT_SIZE * 0.25, AGENT_SIZE * 0.3);
  g.endFill();

  // Gambe — due rettangoli, offset alternato per passo
  const legOffset = state === 'walking' ? Math.sin(Date.now() * 0.012) * 2 : 0;
  g.beginFill(darken(color, 0.25));
  g.drawRect(-AGENT_SIZE * 0.28, AGENT_SIZE * 0.55, AGENT_SIZE * 0.22, AGENT_SIZE * 0.35 + legOffset);
  g.drawRect( AGENT_SIZE * 0.06, AGENT_SIZE * 0.55, AGENT_SIZE * 0.22, AGENT_SIZE * 0.35 - legOffset);
  g.endFill();

  // Martello — visibile solo quando state === 'working'
  if (state === 'working') {
    drawHammer(g, color);
  }
}

function drawHammer(g: PIXI.Graphics, agentColor: number) {
  // Manico — linea diagonale
  g.lineStyle(1.5, 0x8B6914, 1.0);
  g.moveTo(AGENT_SIZE * 0.3, -AGENT_SIZE * 0.1);
  g.lineTo(AGENT_SIZE * 0.7, -AGENT_SIZE * 0.5);

  // Testa del martello — rettangolo piccolo
  g.beginFill(0x888888, 1.0);
  g.drawRect(AGENT_SIZE * 0.6, -AGENT_SIZE * 0.65, AGENT_SIZE * 0.35, AGENT_SIZE * 0.22);
  g.endFill();
}
```

---

### Road Graph — Navigazione Sicura

Prima di muovere qualsiasi agente, Rust costruisce il road graph al momento del scan e lo serializza nel `CityState`. Il frontend lo usa per il pathfinding — nessun calcolo on-the-fly su dati incompleti.

```typescript
interface RoadGraph {
  nodes: Record<string, IsoPoint>;   // file_id → coordinate isometriche
  edges: [string, string][];         // coppie di file_id connessi da strada
}
```

Il path tra due nodi è calcolato una volta sola con BFS (non A* — la griglia non è uniforme ma i nodi sono pochi centinaia al massimo). Il risultato è una sequenza ordinata di `IsoPoint` lungo cui l'agente si muove.

```typescript
function findPath(graph: RoadGraph, fromId: string, toId: string): IsoPoint[] | null {
  // BFS sul grafo delle strade
  // Restituisce array di coordinate isometriche intermedie
  // Restituisce null se i due nodi non sono connessi
}
```

**Se `findPath` restituisce null** → teletrasporto: fade out in 200ms, riposizionamento istantaneo, fade in in 200ms. Nessun bug visivo possibile.

---

### Movement Loop

```typescript
class AgentMover {
  private path: IsoPoint[] = [];
  private pathIndex = 0;
  private speed = 1.2;              // tile/secondo — lento, visibile, non frenetico
  private state: AgentState = 'idle';

  assignTarget(targetFileId: string, graph: RoadGraph, currentFileId: string) {
    const newPath = findPath(graph, currentFileId, targetFileId);

    if (!newPath) {
      // Teletrasporto
      this.teleportTo(graph.nodes[targetFileId]);
      return;
    }

    this.path = newPath;
    this.pathIndex = 0;
    this.state = 'walking';
  }

  update(delta: number, sprite: PIXI.Container) {
    if (this.state === 'working' || this.path.length === 0) return;

    const target = this.path[this.pathIndex];
    const dx = target.x - sprite.x;
    const dy = target.y - sprite.y;
    const dist = Math.sqrt(dx * dx + dy * dy);
    const step = this.speed * delta * TILE_W;

    if (dist <= step) {
      // Arrivato al nodo corrente
      sprite.x = target.x;
      sprite.y = target.y;
      this.pathIndex++;

      if (this.pathIndex >= this.path.length) {
        // Arrivato alla destinazione finale
        this.state = 'working';
        this.path = [];
      }
    } else {
      // Movimento continuo verso il nodo
      sprite.x += (dx / dist) * step;
      sprite.y += (dy / dist) * step;

      // Flip orizzontale in base alla direzione — l'omino guarda dove va
      sprite.scale.x = dx > 0 ? 1 : -1;
    }
  }

  private teleportTo(point: IsoPoint) {
    // Fade out → reposition → fade in
    gsap.to(this.sprite, { alpha: 0, duration: 0.2, onComplete: () => {
      this.sprite.x = point.x;
      this.sprite.y = point.y;
      this.state = 'working';
      gsap.to(this.sprite, { alpha: 1, duration: 0.2 });
    }});
  }
}
```

---

### Animazione Martello

Quando `state === 'working'`, il martello oscilla su e giù. È un singolo valore angolare sul `PIXI.Graphics` del martello — niente sprite, niente spritesheet.

```typescript
// Nel ticker, solo per agenti in stato 'working'
function updateHammerAnimation(agentGraphics: PIXI.Graphics, elapsed: number) {
  // Oscillazione angolare semplice: -30° → +10° → -30° a ~2Hz
  const angle = Math.sin(elapsed * 0.013) * 0.35 - 0.1;
  agentGraphics.rotation = angle;
}
```

Il risultato visivo: l'omino è fermo davanti all'edificio, il braccio con il martello sale e scende ritmicamente. Riconoscibile, non fastidioso.

---

### Glow sull'Edificio — Complementare al Camminatore

Quando l'agente è `working` su un edificio, il glow pulsante sull'edificio resta attivo — indica l'attività anche a zoom out dove l'omino non è visibile.

```typescript
const AGENT_COLORS = {
  orchestrator: 0x4A9EFF,    // Orchestrator (Strategos)
  coder:        0xFFB347,    // Coder (Tekton)
  verifier:     0x7FD47F,    // Verifier (Episkopos)
};

// Ellisse sotto l'edificio, colore agente, alpha oscillante 0.2–0.45
// Visibile a qualsiasi zoom — l'omino solo a zoom > 0.6
```

A zoom out: vedi il glow colorato sull'edificio.
A zoom in: vedi l'omino con il martello davanti alla porta.
I due livelli si complementano senza sovrapporsi.

---

## Fluidità Anni 2000

PixiJS Ticker a 60fps nativo. Il feeling vintage si ottiene non dal framerate ma da:

**1. Easing non-lineare**
Tutti i movimenti usano `easeInOutQuad` — accelerazione e decelerazione morbida ma percettibile, non lineare.

**2. Animazioni sottili sempre presenti**

- Omini agenti: gambe in movimento, martello oscillante quando lavorano
- Fumo edifici attivi: particelle PixiJS leggere che salgono
- Awning market: oscillazione impercettibile
- Fiamma temple: flickering della fiamma dorata

**3. Transizioni di stato**

- Edificio passa a `burning`: flash bianco → comparsa fiamme in 300ms
- Agente arriva all'edificio: omino si ferma, martello appare in 150ms
- Nuovo edificio (scan aggiornato): pop-in dal basso in 400ms

**4. Il terreno respira**
Texture procedurale del terreno con leggero rumore Perlin su alpha — non statica. Impercettibile ma toglie la sensazione di fermo-immagine.

---

## Sidebar di Ispezione

Click su edificio → sidebar destra con animazione slide-in.

```
┌─────────────────────────────┐
│  🏛 oracle_client.ts        │
│  Temple (Naos) — Oracle Q.  │
├─────────────────────────────┤
│  Righe: 1.240               │
│  Distretto: Oracle Zone     │
│  Ultimo commit: 23 min fa   │
├─────────────────────────────┤
│  Importato da:              │
│  · main.tsx                 │
│  · agent_core.ts            │
│                             │
│  Importa:                   │
│  · lancedb                  │
│  · scaleway_api             │
├─────────────────────────────┤
│  Agente attivo:             │
│  🟠 Coder — "refactor       │
│     embedding pipeline"     │
├─────────────────────────────┤
│  [Apri in VS Code]          │
│  [Apri in Cursor]           │
│  [Assegna Task →  Kanban]   │
└─────────────────────────────┘
```

Link editor via URI scheme:

| Editor | URI | Note |
|---|---|---|
| VS Code | `vscode://file/{path}:{line}:{col}` | Apre file a riga esatta |
| VS Code Insiders | `vscode-insiders://file/{path}:{line}` | |
| Cursor | `cursor://file/{path}:{line}` | |
| Android Studio | `idea://open?file={path}&line={line}` | Stesso schema di tutti gli IDE JetBrains |
| IntelliJ IDEA | `idea://open?file={path}&line={line}` | |
| WebStorm | `webstorm://open?file={path}&line={line}` | |
| Fleet (JetBrains) | `fleet://open?file={path}&line={line}` | |
| Zed | `zed://file/{path}:{line}` | |
| Sublime Text | Nessun URI scheme nativo — apribile via CLI: `subl {path}:{line}` | Tauri può invocare il processo direttamente |
| Notepad++ | Nessun URI scheme — CLI: `notepad++ -n{line} {path}` | Solo Windows, Tauri shell command |
| Vim / Neovim | Nessun URI — CLI: `nvim +{line} {path}` | Tauri shell command nel terminale configurato |

**Implementazione in Tauri:**

Gli editor con URI scheme nativo si aprono con `shell::open()` di Tauri — nessun permesso speciale richiesto:

```typescript
import { open } from '@tauri-apps/plugin-shell';

async function openInEditor(filePath: string, line: number, editor: EditorType) {
  const uriMap: Record<EditorType, string> = {
    vscode:          `vscode://file/${filePath}:${line}`,
    cursor:          `cursor://file/${filePath}:${line}`,
    android_studio:  `idea://open?file=${filePath}&line=${line}`,
    webstorm:        `webstorm://open?file=${filePath}&line=${line}`,
    zed:             `zed://file/${filePath}:${line}`,
    fleet:           `fleet://open?file=${filePath}&line=${line}`,
  };

  const cliMap: Record<EditorType, string[]> = {
    notepad_plus:  ['notepad++', `-n${line}`, filePath],
    sublime:       ['subl', `${filePath}:${line}`],
    neovim:        ['nvim', `+${line}`, filePath],
  };

  if (uriMap[editor]) {
    await open(uriMap[editor]);
  } else if (cliMap[editor]) {
    await Command.create(cliMap[editor][0], cliMap[editor].slice(1)).execute();
  }
}
```

**Nella sidebar:** l'utente configura il suo editor preferito nelle impostazioni Tauri una volta sola. Il bottone "Apri in Editor" usa sempre quello. Bottone secondario opzionale per aprire in un secondo editor (es. VS Code per frontend, Android Studio per `.kt`).

Il link Kanban crea una card nel sistema esistente con `file_id` e `file_path` pre-compilati.

---

## Scaleway — Quartiere Speciale

I container e VM spawnable hanno comportamento unico:

**Always-on:** edificio sempre presente, colore normale. Status `running` → leggera emissione luminosa verde sul tetto.

**On-demand (spawnable):**

- Status `stopped` → edificio desaturato, leggermente traslucido (alpha 0.6)
- Status `spawning` → animazione costruzione: edificio cresce dal basso in 800ms
- Status `running` → edificio pieno, pulsazione verde
- Status `stopped` (shutdown) → edificio si "sgonfia" verso il basso in 600ms

GPU VM ha un edificio visivamente più grande e massiccio degli altri nel distretto — riflette il peso computazionale.

Object Store è il warehouse: largo, basso, con dimensione proporzionale se l'API Scaleway espone la metrica di utilizzo.

---

## L'Augur — Il Quarto Agente

L'Augur è un agente invisibile sulla mappa. Non cammina, non martella, non ha un omino. È la forza divina che mantiene la città viva e coerente. Opera in background con due modalità distinte: **aggiornamento semantico** e **sorveglianza urbana**.

---

### Ciclo di Vita

Il Kanban è la fonte di verità primaria per i cambiamenti intenzionali. Il file watcher Rust è il fallback per tutto il resto.

```
Kanban card → "In Progress" (evento MCP)
    ↓ Augur riceve file_id associato alla card
    ↓ pre-evidenzia edificio: bordo luminoso sottile, stato "incoming"
    ↓ attende conferma da Rust file watcher

Rust file watcher rileva modifica sul file atteso
    ↓ aggiornamento meccanico immediato (righe, visual_tier)
    ↓ se modifica sostanziale → Oracle per aggiornamento semantico

Kanban card → "Done"
    ↓ Augur triggera rescan semantico completo del file
    ↓ aggiorna edificio con stato finale
    ↓ spegne eventuali smoke/fire legati a quel task
    ↓ animazione sigillo dorato sull'edificio

File modificato senza card Kanban associata
    ↓ Rust watcher lo rileva comunque
    ↓ Augur aggiorna meccanicamente (righe, visual_tier)
    ↓ marca edificio con piccola icona "modifica non tracciata"
    ↓ NON interroga Oracle — aspetta la card prima del rescan semantico
```

**Il vantaggio chiave:** il file watcher genera eventi su ogni salvataggio intermedio mentre un agente scrive. Con il Kanban come filtro, l'Augur sa che il file è "in lavorazione" e rimanda il rescan semantico costoso (Oracle) a quando la card passa a Done — non su ogni Ctrl+S.

---

### Integrazione MCP — Kanban

L'Augur si connette allo stesso MCP server locale già usato dagli agenti. Ascolta tre eventi:

```typescript
// Card assegnata a file specifico e messa In Progress
interface KanbanCardStarted {
  card_id: string;
  file_paths: string[];      // → risolti in file_id via meta store
  assigned_agent?: string;
}

// Card completata
interface KanbanCardDone {
  card_id: string;
  file_paths: string[];
}

// Nuovo file creato e aggiunto al progetto (card di tipo "new feature")
interface KanbanCardNewFile {
  card_id: string;
  expected_file_path: string;  // Augur pre-riserva coordinate sul layout
}
```

Non legge l'intero Kanban — riceve solo push events sui file che gli interessano. Il resto del Kanban (priorità, commenti, assignee umani) è irrilevante per la mappa.

---

Quando l'Augur interviene, la mappa lo comunica visivamente. Un **sigillo dorato** appare brevemente sopra l'edificio o la zona interessata — un cerchio con raggi che si espande e svanisce in 600ms. Non invasivo, ma riconoscibile: la città ha ricevuto una decisione divina.

```typescript
function playAugurIntervention(target: IsoPoint) {
  const g = new PIXI.Graphics();
  // Cerchio dorato che si espande da r=0 a r=24 in 400ms
  // Alpha 0.8 → 0 in parallelo
  // 8 raggi sottili che si espandono verso l'esterno
  gsap.to(seal, {
    scale: 2.5, alpha: 0, duration: 0.6,
    ease: 'power2.out',
    onComplete: () => effectsLayer.removeChild(g)
  });
}
```

---

### Sorveglianza Urbana — I Peccati Urbani

L'Augur sorveglia una lista di condizioni oggettivamente rilevabili da Rust, senza Oracle. Quando ne trova una, appicca il fuoco all'edificio colpevole.

**La filosofia:** non serve AI per trovare questi problemi. Servono occhi — e l'Augur li ha.

```typescript
interface UrbanSin {
  sin_id: string;
  severity: "smoke" | "fire" | "inferno";  // intensità visiva delle fiamme
  description: string;                      // testo nella sidebar
  auto_detectable: boolean;                 // Rust puro o serve Oracle
}
```

| Peccato | Rilevatore | Severità | Come si trova |
|---|---|---|---|
| API key hardcodata nel codice | Rust regex | `inferno` | Pattern `sk-`, `Bearer`, `api_key =` nel sorgente |
| Secret in `.env` committato | Rust + git | `inferno` | File `.env` presente in `git log` |
| Import ciclico | Rust graph | `fire` | Ciclo nel road graph già costruito |
| File non toccato da 90+ giorni ma con dipendenze attive | Rust + git log | `smoke` | `git log --follow` + data ultimo commit |
| Dipendenza con versione fissa vecchia di 1+ anno | Rust parser | `smoke` | Legge `package.json` / `Cargo.toml`, confronta con data |
| Endpoint Scaleway senza autenticazione (no header IAM) | Rust regex | `fire` | Chiamate HTTP senza header auth nel codice |
| File con > 3 TODO/FIXME/HACK nei commenti | Rust regex | `smoke` | Pattern nei commenti |
| Funzione esportata ma mai importata da nessuno | Rust graph | `smoke` | Nodo nel road graph senza edge in entrata |
| Variabile d'ambiente usata nel codice ma non in `.env.example` | Rust parser | `fire` | Diff tra `process.env.X` nel codice e chiavi in `.env.example` |
| Chiave IAM Scaleway con scadenza entro 30 giorni | Scaleway API | `fire` | Campo `expires_at` dalla IAM API |

---

### Livelli di Fiamma

```typescript
type DisasterLevel = "smoke" | "fire" | "inferno";
```

**Smoke (fumo)** — problema minore, non urgente. Pennacchio di fumo grigio sottile sopra il tetto. L'edificio è intatto. Visibile solo a zoom > 0.5.

**Fire (fuoco)** — problema reale che va risolto. Fiamme arancioni procedurali sul lato dell'edificio. Animazione flickering a 60fps. Visibile a qualsiasi zoom.

**Inferno** — critico, sicurezza compromessa. Fiamme rosse su tutto l'edificio, particelle di brace che salgono, il tetto inizia a cedere geometricamente. Impossibile ignorarlo. Lampeggia anche nel minimap se implementato.

```typescript
function drawDisaster(g: PIXI.Graphics, level: DisasterLevel, elapsed: number) {
  if (level === 'smoke') {
    // 3-4 particelle grigie che salgono lentamente, alpha bassa
    drawSmokeParticles(g, elapsed, { count: 3, color: 0x888888, alpha: 0.4 });
  }
  if (level === 'fire' || level === 'inferno') {
    // Fiamme procedurali: triangoli distorte con noise su vertici
    const flameCount = level === 'inferno' ? 8 : 4;
    const flameColor = level === 'inferno' ? 0xFF2200 : 0xFF8800;
    drawProceduralFlames(g, elapsed, { count: flameCount, color: flameColor });
  }
  if (level === 'inferno') {
    // Brace: piccole particelle rosse che salgono e svaniscono
    drawEmbers(g, elapsed);
    // Tetto che cede: vertici superiori dell'edificio distort con sin wave
    drawCollapsedRoof(g, elapsed);
  }
}
```

---

### Chi Spegne il Fuoco

L'Augur trova i problemi. Non li risolve — non è un coder. Tre possibilità:

**1. Fix automatico** (solo per peccati meccanici semplici): se il peccato è "variabile d'ambiente non in `.env.example`", l'Augur può aggiungerla direttamente. Nessun agente necessario.

**2. Coder inviato dal Kanban**: click sull'edificio in fiamme → sidebar mostra il peccato specifico con descrizione → bottone "Invia Coder" crea card nel Kanban con context pre-compilato → Coder risolve → commit rilevato da Rust → `resolve_file_disaster()` → fiamme si spengono con animazione acqua.

**3. Risoluzione manuale**: l'utente risolve da solo, Rust rileva il fix al prossimo scan dell'Augur → fuoco si spegne automaticamente.

---

### Interfaccia Agent nel CityState — Aggiornata

```typescript
interface Agent {
  agent_id: string;
  // Slug English stabile; display label "English (Greek)" via agentTypeLabel.
  type: "orchestrator" | "coder" | "verifier" | "augur";
  status: "idle" | "walking" | "working" | "reviewing" | "surveying";
  current_file_id: string | null;
  current_task: string | null;
  color: string;   // colore omino per i 3 agenti visibili; inutilizzato per augur
  last_intervention?: string;  // ISO timestamp ultima azione augur
}
```

---

`reset_city_to_new_era("Beta")`:

1. Snapshot immutabile dell'intera `CityState` salvato in `eras/alpha_snapshot.json`
2. Sulla mappa, ai margini del terreno, viene eretto un **Colosseo/Arco di Trionfo** geometrico con label "Era Alpha" e statistiche chiave: n° file, n° commit totali, n° disastri risolti
3. La griglia si svuota progressivamente (fade out edifici in sequenza, 20ms di delay per edificio)
4. I nuovi edifici pop-in dal basso man mano che il nuovo scan viene completato
5. I monumenti delle ere precedenti restano permanentemente ai margini, cumulativi

---

## File da Produrre (Implementazione)

```
aspis-bio-polis-map/
├── src-tauri/
│   ├── src/
│   │   ├── city_scanner.rs      # Scansione file, metriche, import parser
│   │   ├── oracle_client.rs     # Chiamate Oracle per classificazione
│   │   ├── scaleway_client.rs   # IAM API, status container/VM
│   │   ├── city_state.rs        # Strutture dati + Arc<Mutex<CityState>>
│   │   ├── meta_store.rs        # .aspis-meta.json — UUID stabili
│   │   └── commands.rs          # Tutti i comandi Tauri esposti
│   └── tauri.conf.json
├── src/
│   ├── components/
│   │   ├── PolisMap.tsx        # Root component, PixiJS setup
│   │   ├── MapViewport.tsx      # Viewport, zoom, pan
│   │   ├── BuildingRenderer.ts  # Grafica procedurale edifici
│   │   ├── RoadRenderer.ts      # Strade e connessioni
│   │   ├── DistrictRenderer.ts  # Mura distretti
│   │   ├── AgentLayer.tsx       # Omini geometrici, movimento, martello
│   │   ├── AgentMover.ts        # BFS pathfinding + movement loop + teleport
│   │   ├── EffectsLayer.ts      # Fuoco, fumo, particelle
│   │   └── InspectSidebar.tsx   # Pannello ispezione file
│   ├── store/
│   │   └── cityStore.ts         # Zustand store
│   ├── types/
│   │   └── city.ts              # Tutti i tipi TypeScript
│   └── hooks/
│       ├── useCityState.ts      # Polling/event dal backend Tauri
│       └── useAgentSync.ts      # Sync agenti dal MCP server
```

---

## Integrazione MCP Esistente

Il `useAgentSync.ts` si connette al MCP server locale già esistente per ricevere eventi:

```typescript
// Evento ricevuto dal MCP quando un agente inizia a lavorare
interface AgentWorkEvent {
  agent_id: string;
  agent_type: "orchestrator" | "coder" | "verifier";
  file_path: string;        // → risolto in file_id via meta store
  task_description: string;
  kanban_card_id?: string;  // link alla card corrispondente
}
```

Quando il coder completa e fa commit → MCP emette `AgentCompleteEvent` → `set_agent_location(agent_id, null)` → omino scompare con fade out in 300ms, martello sparisce.

---

## Ordine di Implementazione Consigliato

1. **Rust: strutture dati + meta store** (UUID stabili, no regressioni future)
2. **Rust: city scanner** (scansione + import parser + layout)
3. **PixiJS: viewport + proiezione isometrica** (griglia vuota navigabile)
4. **PixiJS: rendering edifici procedurali** (solo house e townhall per iniziare)
5. **Rust: Scaleway client** (status live infrastruttura)
6. **PixiJS: strade** (lastricata per import, terra battuta per semantica, acquedotto per infrastruttura)
7. **PixiJS: distretti murari**
8. **React: sidebar ispezione**
9. **PixiJS: agenti** (omino geometrico → BFS su road graph → movimento continuo → teletrasporto fallback → martello)
10. **Rust: file watcher** (delta accumulator, trigger Augur)
11. **Rust: sorveglianza urbana** (rilevatori peccati — regex, graph, git log, Scaleway API)
12. **PixiJS: sistema disastri** (smoke / fire / inferno procedurali + animazione sigillo Augur)
13. **PixiJS: effetti** (spawn/despawn Scaleway, macerie file eliminati)
14. **Rust + PixiJS: prestige system**
15. **MCP sync integration**
