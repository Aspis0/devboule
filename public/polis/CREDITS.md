# Polis Art Credits

The isometric city view ("Polis") renders with open-licensed sprite art from the
sources below, curated and in some cases rescaled/recolored to fit the app's
palette. This file ships with the application. The machine-readable per-asset
ledger lives in the repository at `docs/polis-art-ledger.json`; an asset without
a ledger entry must not be committed.

License notes:
- **CC0** assets require no attribution (credited anyway, with thanks).
- **CC-BY** assets are credited per their license.
- **CC-BY-SA** art: any modified sprite files we distribute remain licensed
  CC-BY-SA (the share-alike applies to those art files only, not to this
  application). Modified files are flagged in the ledger.
- **CC-BY-ND** assets are bundled unmodified.

## Sources

- **Screaming Brain Studios** — seamless terrain/material textures (grass,
  dirt, stone, plaster, ashlar/brick, marble, wood planks, terracotta roof
  tiles, thatch, sea water + underwater caustics; `tex:*`) from the "Tiny Texture Pack" series.
  https://opengameart.org/content/tiny-texture-pack — License: **CC0**
  ("released under the CC0/Public Domain License", pack License.txt).
  Used rescaled, some desaturated/brightened to fit the palette. Thank you!

- **Unknown Horizons team** — tree sprites (`prop:tree`, `prop:cypress`),
  the ambient crowd's walk cycles (`walk:citizen*`, 8 directions × 4 frames),
  and the countryside resource art: mountain mine, stone-deposit and stone-pit
  quarries, ambient rocks (`res:mine`, `res:quarry:*`, `prop:rock:*`).
  https://github.com/unknown-horizons/unknown-horizons (content/gfx) —
  License: **CC-BY-SA 3.0** (art, per the project's doc/LICENSE; multiple
  artists — see their doc/AUTHORS.md). Rescaled/re-packed for this app; the
  modified sprite files remain CC-BY-SA per the share-alike note above.

- **FoshyTakashi** — 9-frame fire animation (`fx:fire:*`, the burning-building
  flip-book frames).
  https://opengameart.org/content/9-frame-fire-animation-16x-32x-64x —
  License: **CC-BY 3.0**. Frames cut from the 64px strip, rescaled and tinted
  per fire severity. Thank you!
