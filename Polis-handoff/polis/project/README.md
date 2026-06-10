# Polis — Atlante edifici greci isometrici (PixiJS)

Sistema **procedurale** (non sprite PNG): ogni edificio è disegnato via `Pixi.Graphics`
in vista isometrica 2:1. 14 tipi + `unknown` (fallback), 5 taglie per tipo, parti
animabili separate.

## File
| file | ruolo |
|---|---|
| `js/iso.js` | proiezione + shading + primitive ricche: `box`(con `tex`), `texFace`, `steps`, `column`(scanalata), `colonnade`, `gableRoof`/`hipRoof`(a tegole), `cylinder`, `ground`(testurizzato) |
| `js/anims.js` | parti animate: `Flame`, `Beacon`, `Flag`, `Smoke`, `Water` |
| `js/detail.js` | props mediterranei: `cypress`, `bush`, `olive`, `gardenBed`, `hedge`, `amphora`, `urn`, `statue`, `fountain`, `pavers` (namespace `PROP`) |
| `js/people.js` | cittadini animati (solo estetica): `PEOPLE.make(type, opts)` → figura con `.node` + `.update(t,dt)` |
| `js/buildings_a.js` / `js/buildings_b.js` | i 15 generatori + `BUILD_META` |
| `js/app.js` | showcase (galleria, mappa, pan/zoom, controlli) — **solo demo** |
| `index.html` | UI dello showcase |

Per il gioco ti servono solo `iso.js`, `anims.js`, `buildings_*.js`.

## Convenzioni
- **Tile** `96×48`, iso **2:1**. `ISO.HALF_W=48`, `ISO.HALF_H=24`, `ISO.Z_UNIT=56` px per 1.0 di altezza.
- **Proiezione**: `proj.p(gx, gy, gz) → {x,y}`. L'origine `(0,0)` del container coincide con
  l'**ancora front-bottom** (angolo `gx=W, gy=D, z=0` del footprint). Posiziona il container
  sul punto-ancora della cella e tutto combacia.
- **Sole top-left** (`ISO.SUN.dir = 'NW' | 'NE'`): top chiaro, faccia sinistra media, destra scura.
- **Taglie**: indice `0..4` → capanna < casa < insula < palazzo < monumento. Footprint e
  altezza/dettaglio crescono insieme (vedi gli array `sizes` in ogni builder).

## API
```js
// level: 0..4 ; opt: { outline?:bool }
const m = BUILD.temple(2, { outline:false });
// m = { container: PIXI.Container, body: PIXI.Graphics, anims:[...], foot:[W,D] }

m.container.position.set(px, py);   // px,py = punto front-bottom sulla mappa
stage.addChild(m.container);

// nel tuo ticker:
app.ticker.add(() => {
  const dt = app.ticker.deltaMS / 1000;
  T += dt;
  for (const a of m.anims) a.update(T, dt);   // fiamma/beacon/bandiera/fumo/acqua
});
```
`BUILD_META.order` elenca gli slug; `BUILD_META.info[slug]` dà `{name, cat, accent}`.

## Slug → animazione
temple→fiamma · lighthouse→beacon · fortress/tower/warehouse/townhall→bandiera ·
workshop→fumo+fiamma · conduit/baths/harbor→acqua · gli altri sono statici.

## Depth sort (più edifici sulla stessa mappa)
Disegna dal fondo al fronte ordinando per `(x + W + y + D)` crescente (angolo frontale del
footprint). Vedi `buildMap()` in `app.js`.

## Cittadini (people.js) — solo estetica
`PEOPLE.make(type, opts)` → `{ node, update(t,dt) }`. Tipi (nomi greci traslitterati):
`citizen` (Polites), `builder` (Tekton, martella), `firefighter` (Pyrosbestes, secchio
+ spruzzo), `watercarrier` (Hydrophoros, giogo + anfore), `merchant` (Emporos, sacco),
`noble` (Eupatrides, himation). `opts`: `scale`, `face` (±1), `speed`, e per il movimento
o `path: [{x,y}…]` (segue e fa loop) o `range: {x0,x1,y}` (avanti-indietro). Senza moto,
`builder` martella e `firefighter` spruzza sul posto. Nessuna logica di gioco: collega tu
il pathfinding passando i punti a `path`.

## Nomi (greco, alfabeto latino)
Gli slug restano stabili in inglese (`temple`, `house`…). I nomi mostrati sono greci:
Naos, Oikos, Phrourion, Pyrgos, Pharos, Agora, Apotheke, Ergasterion, Hydragogeion,
Balaneion, Theatron, Limen, Bibliotheke, Bouleuterion, Agnoston. Taglie: Kalybe, Oikia,
Synoikia, Megaron, Mnemeion.

## Note
- `preserveDrawingBuffer:true` sull'`Application` serve solo per screenshot/export affidabili.
- Le colonne sono disegnate in screen-space (3 strisce + capitello/base) per un tondo pulito
  ed economico; passa `{ ionic:true }` per le volute.
- I dettagli (porte/finestre/fregi) usano `ISO.panelLeft/panelRight` su una faccia verticale.
