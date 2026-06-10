# HANDOFF — Polis (Tauri / Rust) · `src-tauri/src/polis/`

Inquadramento: **Tauri** = backend **Rust** + frontend **web**. Conviene che il Rust possieda il
**modello e la simulazione** (terreno, fiumi/mare, edifici, meraviglie, cittadini + pathfinding) ed
esponga lo stato via **comandi/eventi IPC**; il **rendering** può riusare il `js/` di questo progetto
(`iso.js`, `buildings_*`, `monuments_*`, `figures.js`, `people.js`) come *renderer del frontend*.

Sotto: struttura dei moduli `.rs`, modelli con `serde`, generazione, navigazione A\*, comandi Tauri.
I file `js/` di questo progetto restano l'implementazione di riferimento per *matematica* e *regole*.

---

## ⚑ Cosa inviare a Claude per il port

- **Minimo indispensabile**: questo `HANDOFF.md` **+ la cartella `js/`** di questo progetto. L'handoff è
  la specifica ma rimanda a `js/` per la matematica e le regole esatte (proiezione, footprint, terreno,
  percorsi). Senza `js/`, Claude deve indovinare i dettagli. Le 3 pagine HTML (`index.html`,
  `Meraviglie.html`, `Mappa.html`) sono utili come reference visivo, non indispensabili.
- **Per integrarsi nel codice esistente** (consigliato): aggiungi anche i tuoi **`.rs` di
  `src-tauri/src/polis/`** (almeno i modelli + `mod.rs` + un paio di file rappresentativi). Così Claude
  allinea nomi reali di struct/campi/moduli invece dello schema *proposto* qui sotto, e non duplica ciò
  che esiste già. Basta il modulo `polis/`, non tutto il repo.

| obiettivo | cosa allegare |
|---|---|
| capire il sistema / prototipo nuovo | `HANDOFF.md` + `js/` |
| integrare nell'app esistente | `HANDOFF.md` + `js/` + cartella `polis/` (o l'intero `src-tauri/src/`) |

> **Dirlo esplicitamente a Claude**: in Tauri il **rendering può restare nel frontend web** riusando
> `js/` (generatori `MON.*`/`BUILD.*`, `figures.js`, `people.js`); il **Rust fa solo modello,
> simulazione e pathfinding**. Senza questa indicazione Claude rischia di riscrivere da zero anche la
> grafica in Rust, inutilmente. Lo schema di moduli qui sotto è una **proposta**: adattalo ai nomi reali
> del progetto.

---

```
src-tauri/src/polis/
├── mod.rs          // World + re-export, tick()
├── iso.rs          // proiezione iso, depth-key, tipi coordinate
├── terrain.rs      // Terrain, TileMap, generazione (terra/fiumi/mare/rive/ponti)
├── buildings.rs    // registro edifici + footprint per tier
├── monuments.rs    // registro 12 Meraviglie + footprint + parte animata
├── placement.rs    // Placed { kind, gx, gy } + validazione (no overlap/acqua)
├── nav.rs          // griglia di calpestabilità + A*
├── agents.rs       // Citizen + movimento lungo i waypoint
└── commands.rs     // #[tauri::command] snapshot/tick verso il frontend
```

---

## `iso.rs` — fondamenta isometriche

Stessa matematica di `js/iso.js`. Serve a Rust per l'**ordine di disegno** (depth sort) e per
proiettare i centri-tile dei waypoint da inviare al frontend.

```rust
pub const HALF_W: f32 = 48.0;
pub const HALF_H: f32 = 24.0;
pub const Z_UNIT: f32 = 56.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Tile { pub gx: i32, pub gy: i32 }

#[derive(Clone, Copy)]
pub struct Proj { ax: f32, ay: f32 }
impl Proj {
    pub fn new(w: f32, d: f32) -> Self { Self { ax: (w - d) * HALF_W, ay: (w + d) * HALF_H } }
    /// punto griglia -> schermo, ancora front-bottom del footprint
    pub fn p(&self, gx: f32, gy: f32, gz: f32) -> (f32, f32) {
        ((gx - gy) * HALF_W - self.ax, (gx + gy) * HALF_H - self.ay - gz * Z_UNIT)
    }
}

/// chiave di profondità: ordina CRESCENTE (dal fondo al fronte).
/// tile: w=d=1; edificio/meraviglia: usa il footprint.
#[inline]
pub fn depth_key(gx: i32, gy: i32, w: i32, d: i32) -> i32 { gx + w + gy + d }
```

> Sole top-left (`NW|NE`): lo shading (`shade(color, factor)`, 3 facce) può restare **lato frontend**
> se renderizzi in JS. Se renderizzi lato Rust, porta `shade`/`faceFactor` da `iso.js`.

---

## `terrain.rs` — terra, fiumi, mare, rive, ponti

```rust
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Terrain { Grass, Sand, Road, Plaza, River, Sea, Bridge }

pub struct TileMap {
    pub n: i32,                 // mappa n×n (app: grande, es. 128+)
    pub sea_y: i32,             // gy >= sea_y => mare aperto
    pub tiles: Vec<Terrain>,    // len = n*n, indicizzata idx(gx,gy)
    pub rivers: Vec<(i32, i32)>,// colonne-fiume [gx_min, gx_max]
}

impl TileMap {
    #[inline] pub fn idx(&self, gx: i32, gy: i32) -> usize { (gy * self.n + gx) as usize }
    pub fn get(&self, gx: i32, gy: i32) -> Terrain { self.tiles[self.idx(gx, gy)] }

    fn is_river(&self, gx: i32, gy: i32) -> bool {
        gy < self.sea_y && self.rivers.iter().any(|&(a, b)| gx >= a && gx <= b)
    }
}
```

**Regole di classificazione** (priorità decrescente), identiche a `landType`/`isRiver` di `map_app.js`:

1. `gy >= sea_y` → `Sea` (*thalassa*) — tutto davanti alla costa.
2. colonna-fiume e `gy < sea_y` → `River` (*potamos*). **I fiumi sono canali interni che sfociano nel
   mare e devono avere terra su entrambi i lati** (non sul bordo mappa).
3. tile in `road_set` → `Road`; in `plaza_set` → `Plaza`.
4. `gy == sea_y - 1` oppure adiacente a colonna-fiume → `Sand` (riva/*aigialos*).
5. altrimenti `Grass` (*chora*).
6. **Ponte**: dove una strada attraversa un fiume, il tile resta `River` (acqua sotto) **ma** è marcato
   `Bridge` → impalcato rialzato e **calpestabile**. Tieni un `bridge_set: HashSet<Tile>`.

**Generazione (app, mappa grande)**: invece di autorare a mano come la demo, genera:
- terra/coste con rumore (Perlin/Simplex) per `sea_y`/baie;
- fiumi tracciati da sorgente→mare (carve di colonne/percorso) con `Sand` sulle rive;
- strade dal layout urbano (griglia/avenue) + `bridge_set` agli incroci coi fiumi.
Performance: **chunking** (es. 32×32) + **culling** sui bounds proiettati; ordina i chunk per `depth_key`
e dentro ogni chunk ordina gli elementi.

---

## `buildings.rs` & `monuments.rs` — registri + footprint

Gli edifici hanno footprint variabile per **tier** (0..4): vedi `js/buildings_*` e `README.md`.
Le **12 Meraviglie** hanno footprint fisso. Tienili come dati:

```rust
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum AnimPart { None, Flame, Beacon, Flag, Smoke, Water }

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Footprint { pub w: i32, pub d: i32 }

pub struct WonderDef { pub slug: &'static str, pub name: &'static str, pub foot: Footprint, pub anim: AnimPart }

// registro delle 12 Meraviglie (W×D, parte animata)
pub const WONDERS: &[WonderDef] = &[
    WonderDef{slug:"parthenon",  name:"Parthenōn",        foot:Footprint{w:5,d:8}, anim:AnimPart::None},
    WonderDef{slug:"erechtheion",name:"Erechtheion",      foot:Footprint{w:5,d:5}, anim:AnimPart::None},
    WonderDef{slug:"artemision", name:"Artemision",       foot:Footprint{w:6,d:9}, anim:AnimPart::None},   // ⭐
    WonderDef{slug:"tholos",     name:"Tholos",           foot:Footprint{w:4,d:4}, anim:AnimPart::None},
    WonderDef{slug:"horologion", name:"Horologion",       foot:Footprint{w:3,d:3}, anim:AnimPart::Flag},
    WonderDef{slug:"mausoleion", name:"Mausōleion",       foot:Footprint{w:4,d:4}, anim:AnimPart::None},   // ⭐
    WonderDef{slug:"propylaia",  name:"Propylaia",        foot:Footprint{w:5,d:3}, anim:AnimPart::None},
    WonderDef{slug:"bomos",      name:"Bōmos",            foot:Footprint{w:5,d:5}, anim:AnimPart::Flame},
    WonderDef{slug:"olympieion", name:"Olympieion",       foot:Footprint{w:5,d:7}, anim:AnimPart::None},
    WonderDef{slug:"kolossos",   name:"Kolossos",         foot:Footprint{w:3,d:3}, anim:AnimPart::Beacon}, // ⭐
    WonderDef{slug:"zeus",       name:"Zeus Olympios",    foot:Footprint{w:4,d:4}, anim:AnimPart::Water},  // ⭐
    WonderDef{slug:"athena",     name:"Athēna Parthenos", foot:Footprint{w:3,d:3}, anim:AnimPart::None},
];
```

**Nomenclatura**: in greco *Monumento = Mnemeion* è l'**ultimo tier degli edifici** normali; perciò la
collezione si chiama **Meraviglie**, non "Monumenti". ⭐ = tra le Sette Meraviglie antiche storiche.

**Rendering delle Meraviglie**: il *come* disegnarle vive lato frontend (i generatori `MON.*` in
`monuments_*.js` + le statue billboard in `figures.js`). Rust invia solo `Placed { slug, gx, gy }`; il
frontend istanzia il modello all'ancora `proj.p(gx+W, gy+D, 0)`. Se invece renderizzi lato Rust,
**bakea** corpi+statue a sprite atlas e anima solo la `AnimPart` (torcia del Colosso, fiamma del Bōmos…).

---

## `placement.rs` — posizionamento valido

```rust
pub struct Placed { pub kind: Kind, pub gx: i32, pub gy: i32 } // Kind::Building{slug,tier} | Kind::Wonder{slug}

/// un footprint è valido se: dentro mappa, tutti i tile NON sono acqua (Sea/River senza ponte)
/// e non si sovrappone ad altri footprint. (Le rive Sand vanno bene; l'acqua no, salvo edifici-porto.)
pub fn can_place(map: &TileMap, occ: &HashSet<Tile>, f: Footprint, gx: i32, gy: i32) -> bool { /* ... */ }
```

> Eccezione: il **porto** (e basi costiere come il faro/Colosso) può estendersi sul mare in fronte —
> gestiscilo con un flag `coastal` sul def dell'edificio.

---

## `nav.rs` — calpestabilità + A\*

**Vincolo chiave: i cittadini camminano SOLO su strade/ponti, MAI su mare/fiume/edifici.**

```rust
#[inline]
pub fn walkable(t: Terrain) -> bool {
    matches!(t, Terrain::Road | Terrain::Plaza | Terrain::Bridge)
}

/// nodo calpestabile = tile walkable E non occupato da un footprint
pub fn is_node(map: &TileMap, occ: &HashSet<Tile>, t: Tile) -> bool {
    in_bounds(map, t) && walkable(map.get(t.gx, t.gy)) && !occ.contains(&t)
}

/// A* a 4-vicini (niente diagonali: non si "taglia" un angolo d'acqua)
pub fn astar(map: &TileMap, occ: &HashSet<Tile>, start: Tile, goal: Tile) -> Option<Vec<Tile>> {
    // vicini = [(+1,0),(-1,0),(0,+1),(0,-1)] filtrati da is_node; euristica = distanza Manhattan
    // ...
}
```

- Il grafo esclude `Sea`/`River` e i footprint ⇒ **per costruzione** un percorso valido non passa mai su
  acqua o edifici. I **ponti** (`Bridge`) sono l'**unico** attraversamento dei fiumi.
- Se `astar` ritorna `None`, l'agente resta fermo (non sconfina mai).
- `start`/`goal` devono essere nodi calpestabili: per andare a un edificio, usa il **tile-strada
  adiacente alla porta** come punto d'aggancio.

---

## `agents.rs` — cittadini

```rust
pub enum CitizenKind { Polites, Tekton, Pyrosbestes, Hydrophoros, Emporos, Eupatrides } // vedi people.js

pub struct Citizen {
    pub kind: CitizenKind,
    pub tile: Tile,            // tile corrente (sempre walkable)
    pub path: Vec<Tile>,       // waypoint A* (solo nodi calpestabili)
    pub seg: usize,            // segmento corrente
    pub t: f32,                // 0..1 interpolazione nel segmento
    pub speed: f32,            // tile/sec
    pub face: i8,              // direzione sprite ±1
}
```

- `tick(dt)`: avanza `t` lungo `path[seg] -> path[seg+1]`; a fine path scegli una nuova meta
  (agorà/plaza, porto, casa) e ricalcola con `astar`. Movimento ambientale: loop o nuova meta casuale
  **tra tile calpestabili**.
- Posizione schermo per il frontend: interpola i centri-tile `proj.p(gx+0.5, gy+0.5, 0.05)`.
- I tipi/animazioni del personaggio (martello del Tekton, secchio del Pyrosbestes…) sono **estetica
  frontend** (`people.js`): Rust invia solo `kind`, posizione e `face`.

---

## `commands.rs` — IPC verso il frontend

```rust
#[derive(serde::Serialize)]
pub struct MapSnapshot { pub n: i32, pub sea_y: i32, pub tiles: Vec<u8>, pub bridges: Vec<Tile>,
                         pub placed: Vec<PlacedDto>, pub rivers: Vec<(i32,i32)> }

#[derive(serde::Serialize)]
pub struct AgentDto { pub kind: u8, pub x: f32, pub y: f32, pub face: i8 } // x,y già proiettati

#[tauri::command] pub fn get_map(state: tauri::State<World>) -> MapSnapshot { /* ... */ }
#[tauri::command] pub fn tick(state: tauri::State<World>, dt: f32) -> Vec<AgentDto> { /* avanza agenti */ }
#[tauri::command] pub fn place(state: tauri::State<World>, slug: String, gx: i32, gy: i32) -> bool { /* can_place + push */ }
```

- Pattern consigliato: `get_map` una volta (terreno + edifici), poi `tick(dt)` a ~30–60 Hz che ritorna
  solo gli **agenti** (leggero). In alternativa, emetti un **evento** Tauri `agents:update` dal loop di
  simulazione invece di pollare.
- Il frontend (riusa `iso.js`) ridisegna terreno una volta e aggiorna gli sprite-cittadino ogni frame.

---

## `mod.rs` — World & loop

```rust
pub struct World {
    pub map: TileMap,
    pub occ: HashSet<Tile>,        // tile occupati da footprint (bloccati per la nav)
    pub placed: Vec<Placed>,
    pub agents: Vec<Citizen>,
}
impl World {
    pub fn tick(&mut self, dt: f32) { for c in &mut self.agents { /* avanza + re-path se serve */ } }
}
```

---

## Checklist di accettazione

- [ ] `iso::depth_key = gx+w+gy+d`, ordine di disegno corretto a qualsiasi `n`.
- [ ] `Terrain` con Grass/Sand/Road/Plaza/River/Sea/Bridge; fiumi interni con riva su entrambi i lati che
      sfociano nel mare; ponti calpestabili sopra l'acqua.
- [ ] 12 Meraviglie con footprint della tabella `monuments.rs`; `AnimPart` corretta (Flag/Flame/Beacon/Water).
- [ ] `can_place` blocca overlap e acqua (eccetto edifici `coastal`).
- [ ] `nav::walkable` = Road/Plaza/Bridge; footprint in `occ` bloccati; A\* a 4-vicini.
- [ ] **Test**: nessun `Citizen.tile` mai su `Sea`/`River`/footprint; unico attraversamento fiumi = `Bridge`.
- [ ] Mappa grande: chunking + culling, nessun calo di frame ad `n` alto.

_Reference visivo navigabile: `index.html` (atlante) · `Meraviglie.html` · `Mappa.html`._
