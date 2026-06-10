# Polis Buildings — one archetype per file

Each building **type** is its own module here, drawn from a shared iso `kit`.
A building on the map = one **real source file**. Its archetype is decided by
the backend classifier (`scanner.rs::classify_purpose_grounded`) from REAL
structural signals (entry-point detection, directory role, import-graph degree,
extension) — never guessed from the filename alone.

## Which code file lives in which building

| Archetype (slug) | Greek | What code file it is | How it's detected (real signal) |
|---|---|---|---|
| **lighthouse** | Pharos | The app's **entry point** — `main.rs`, `lib.rs`, `main.tsx`, `index.html`, the `[[bin]]`/`[lib]` / `package.json main`. The beacon every ship steers by. | real build-entry detection from Cargo/package/index.html |
| **temple** | Naos | The **oracle / knowledge layer** — retrieval, embeddings, LanceDB, prompt/answerer. Where you consult the divine. | path under `oracle/` |
| **fortress** | Phrourion | **Agent core / orchestrator / dispatcher** — the keep that commands. | `agents/`,`orchestrator/` dir OR high out-degree hub (imports many) |
| **tower** | Pyrgos | **Config** — `*.toml`, `tsconfig`, `tauri.conf`, `wrangler`, `*.config.*`. Tall watchtowers overseeing the city. | `.toml` extension / config dirs |
| **library** | Bibliotheke | **Types / models / constants / interfaces / schema** — the shared archive everyone reads. | `types/`,`models/`,`constants/`,`schema/` dir OR high in-degree leaf (imported by many) |
| **market** | Agora | **API clients / external integrations** — Cloudflare/Scaleway/provider clients. Goods (data) traded with the outside world. | provider/client API dir |
| **warehouse** | Apotheke | **Storage / object-store / persistence** layer. | `store/`,`storage/`,`object-store/` dir |
| **conduit** | Agogos | **Middleware / proxy / routing** — the aqueduct carrying the flow. | `middleware/`,`proxy/`,`routing/` dir |
| **baths** | Balaneion | **Auth / session / token / vault / credentials** — the private cleansing house. | `auth/`,`session/` dir |
| **theater** | Theatron | **Logging / telemetry / monitoring** — where the city's events are shown. | `logging/`,`telemetry/`,`monitoring/` dir |
| **workshop** | Ergasterion | **Scripts / tools / build utilities** — where raw work happens. | `scripts/`,`tools/`,`bin/` dir |
| **harbor** | Limen | **Upload / download / stream / file I/O** — the port where things enter and leave. | upload/download/stream dir |
| **townhall** | Bouleuterion | **Cloudflare worker entry / civic admin**. | worker-entry detection |
| **house** | Oikos | **Generic UI component / unclassified source** — the homes that are the bulk of the city. | the honest DEFAULT when no structural signal matches |
| **unknown** | — | Oracle-introduced / truly unclassified. | registry fallback |

## Architecture

The per-archetype drawing now lives in the ported **"Claude Design" kit** under
`../kitcd/` (a faithful 1:1 port of the Polis handoff art). This folder is just
the **adapter seam** between that kit and the renderer's `BuiltBuilding`
contract:

- `../kitcd/iso.ts` — the engine: 2:1 projection (tile 96×48, front-bottom
  anchor, sun NW), textured box/steps/fluted column/colonnade, tiled
  `gableRoof`/`hipRoof` (courses + ridge + antefixes + pediment), `cylinder`,
  the warm `MAT` palette, `shade`/`mix`/`lerp`, and `faceFactor` (reads `SUN`).
- `../kitcd/anims.ts` — `Flame`/`Beacon`/`Flag`/`Smoke`/`Water` classes, each
  owning a `node: Container` + `update(t, dt)` that clears+redraws its own small
  Graphics (the source's per-frame animation).
- `../kitcd/detail.ts` — `PROP` helpers (cypress, urn, amphora, bush, olive,
  gardenBed, statue, …); static scatter uses a seeded sin-hash, not Math.random.
- `../kitcd/buildings.ts` — `BUILDERS: Record<slug, (level 0..4, opt) →
  { container, body, anims, foot }>` for all 15 slugs, plus `BUILD_META`.
- `index.ts` — `buildBuilding(b, profile, scale)`: maps `visualTier` → kit level
  (kalybe→0 … mnemeion→4), calls `BUILDERS[purpose] ?? BUILDERS.unknown`, and
  wraps the result into a `BuiltBuilding` (`display` container + ground `shadow`
  + the kit's live `anims`). The renderer positions `display` at the building's
  iso anchor and ticks the `anims` (visible chunks only) off its step clock.
- `types.ts` — the `BuiltBuilding` shape (the contract the renderer consumes).

## Rules (kept from the renderer)

- **Determinism**: the kit builders use fixed geometry only — no `Math.random`
  for placement. Static scatter goes through `detail.ts`'s seeded sin-hash, so a
  re-scan reproduces the same city. Only ANIMATION phase (flicker/wave/puff) is
  time-based random, which is not part of the deterministic city state.
- **Pure data**: a building is always a real file. The kit decorates that fact;
  it never invents a building. Unknown slugs fall back to `BUILDERS.unknown`.
- **Animation split**: the static silhouette is baked once into the body
  Graphics inside the kit container; the animated parts are separate `anims`
  whose `update()` the 30fps clock drives only for on-screen buildings.
