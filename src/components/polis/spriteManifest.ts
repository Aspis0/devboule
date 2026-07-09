// Polis sprite manifest — the typed index of every real-art sprite the renderer
// may use, keyed by SEMANTIC key, decoupled from atlas layout.
//
// KEY GRAMMAR (stable API for the whole sprite round; consumers never touch
// frame names):
//   bld:{purpose}:{level}:v{n}   whole-building sprite variant (A5b)
//   tex:{surface}                seamless fill texture (grass/cobble/plaster/...)
//   prop:{kind}:v{n}             scatter prop (olive/rock/stall/...)
//   field:{kind}                 farmland parcel texture per fields.ts kind
//   walk:{type}:{dir}:f{n}       walker animation frame (A6)
//   fx:{effect}:f{n}             flipbook effect frame (fire/smoke, A7)
//
// GENERATION: this file is emitted by tools/polis-art/manifest.py (phase A2)
// from docs/polis-art-ledger.json + the packed atlases in public/polis/atlas/.
// Until A2 lands it is a hand-authored placeholder; keep the shapes in sync
// with the generator. An EMPTY manifest is the shipped state of record until
// assets land — loadPolisSprites returns null and every draw path stays on the
// procedural kit.

/**
 * Per-sprite metadata carried alongside the atlas frame reference. Deliberately
 * deep-readonly: entries are shared by reference between the manifest and every
 * SpriteBank, so a writable field here would let one consumer silently corrupt
 * the anchor/footprint of every building using the same key.
 */
export interface SpriteEntryMeta {
  /** Frame name inside the owning atlas spritesheet. */
  readonly frame: string;
  /** Id of the atlas page (key into SpriteManifest.atlases) holding the frame. */
  readonly atlas: string;
  /** Building footprint in tiles [W, D] — building sprites only. */
  readonly foot?: readonly [number, number];
  /**
   * Anchor in normalized texture coords. Default [0.5, 1]: bottom-center, so a
   * sprite placed at the footprint's front-corner iso point sits on the ground
   * like the kit containers do.
   */
  readonly anchor?: readonly [number, number];
  /**
   * True when the source art bakes its own ground shadow — the renderer must
   * then skip our contact-shadow sprite for this variant or shadows double up.
   */
  readonly hasBakedShadow?: boolean;
}

export interface SpriteManifest {
  version: 1;
  /**
   * Atlas pages: page id -> spritesheet JSON url (app-root relative, served
   * from public/polis/atlas/, CSP 'self').
   */
  atlases: Record<string, string>;
  /** Semantic key -> frame metadata. */
  entries: Record<string, SpriteEntryMeta>;
}

/** Shipped manifest. Empty until phase A2/A3 land the first packed atlases. */
export const SPRITE_MANIFEST: SpriteManifest = {
  version: 1,
  atlases: {},
  entries: {},
};
