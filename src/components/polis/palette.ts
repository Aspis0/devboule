// Polis palette + building visual registry.
//
// Pure data, no PixiJS. The palette mirrors the design doc (Pharaoh / Zeus
// inspired cream/terracotta/ivory). BUILDING_REGISTRY maps a `purpose` SLUG
// (the machine key on the wire) to a visual profile. Oracle may introduce new
// slugs at runtime, so `getProfile` falls back to the `unknown` profile rather
// than throwing — the renderer stays extensible and never invents data.
//
// COLOR CONTRACT: `PALETTE` is the ONLY source of color. Every other shade used
// anywhere in the renderer (vegetation, rock, smoke, window glass, water
// variants, shadows, banners, seams …) is DERIVED from a PALETTE entry via
// darken()/lighten()/alpha and exported here as a named const in `DERIVED`. No
// module is allowed to introduce a fresh hex literal.

import { darken, lighten, blend, saturate } from "./iso";

// Reference targets used ONLY to bias a PALETTE entry toward a more believable
// natural hue (the blend never adopts the target wholesale — it is a nudge of a
// few tenths). Kept here, next to the derivations, so the "all color is derived"
// audit still holds: these are anchors for blend(), not paint applied raw.
// A warm meadow green and a Mediterranean sea blue — same family, just alive.
const HUE_MEADOW = 0x7a9a3e; // olive-meadow green bias target
const HUE_SEA = 0x3f7fa6; // saturated sea-blue bias target
// Bug-investigation P3 — a cool indigo/violet bias for the "under investigation"
// smoke tint. Blend anchor only (the city has no native purple), same family as
// the Scaleway livery but pushed bluer, so the investigative smoke reads as a
// DISTINCT color from the orange/red disaster fire (the HONESTY invariant: a
// suspect is Oracle's GUESS, never a confirmed disaster).
const HUE_INVESTIGATE = 0x5a6fd8; // indigo-violet bias target
const HUE_JADE = 0x4a9a7a; // jade green bias target for Qwen / legacy MiMo livery
const HUE_INDIGO = 0x4a5a9e; // indigo blue bias target for DeepSeek livery
const HUE_TEAL = 0x2a8a8a; // teal bias target for OpenAI livery
const HUE_STEEL = 0x6a7a8e; // slate/steel bias target for Grok livery

export const PALETTE = {
  cream: 0xf4f0e6,
  ivory: 0xede8d8,
  sandDark: 0xc8b89a,
  terracotta: 0xc17a5a,
  terracottaDark: 0x8b4e32,
  shadow: 0x6b5a48,
  stone: 0xa89880,
  stoneDark: 0x7a6855,
  whiteMarble: 0xf8f4ec,
  goldAccent: 0xd4a843,
  grass: 0xdfe0c4,
  grassDark: 0xc9cbac,
  water: 0x8aaabb,
} as const;

// ---------------------------------------------------------------------------
// DERIVED shades — the ONLY place new colors may be introduced, and every one
// is a pure function of a PALETTE entry. Grouped by what they decorate so the
// "palette is the single source of color" rule is auditable at a glance.
// ---------------------------------------------------------------------------
// A living meadow base: the cream `grass` pulled ~38% toward the meadow-green
// anchor and saturated, then darkened a hair so it reads as GROUND not haze.
// This is the keystone — every ground band derives from it, so the whole floor
// shifts from beige to a warm Mediterranean green in one place.
const MEADOW = saturate(blend(PALETTE.grass, HUE_MEADOW, 0.38), 0.22);

// A living sea: the washed PALETTE.water pulled ~52% toward the sea-blue anchor
// and saturated. Drives harbor water + sea-glass window panes.
const SEA = saturate(blend(PALETTE.water, HUE_SEA, 0.52), 0.18);

export const DERIVED = {
  // Terrain value-noise band (chunky retro mottling of the ground). Widened
  // contrast: a sun-bleached light grass, a meadow mid, and a deep olive-shadow
  // dark so the three flat bands actually read as distinct patches (Zeus-style
  // mottling) instead of one mud sheet.
  groundLight: saturate(lighten(MEADOW, 0.16), 0.12),
  groundMid: MEADOW,
  groundDark: saturate(darken(MEADOW, 0.2), 0.16),
  // Earth/sand patches: warm sandy tones that CONTRAST the green so the ground
  // reads as grass + bare earth, not a uniform field.
  groundDirt: saturate(lighten(PALETTE.sandDark, 0.06), 0.16),
  groundWorn: saturate(darken(PALETTE.sandDark, 0.14), 0.18),
  seam: darken(MEADOW, 0.34),

  // Vegetation (olive micro-flora): the meadow base nudged toward a deeper,
  // richer olive green so bushes pop off the lighter ground.
  olive: saturate(darken(MEADOW, 0.18), 0.18),
  oliveDark: saturate(darken(MEADOW, 0.36), 0.2),
  oliveLight: saturate(darken(MEADOW, 0.04), 0.16),

  // Rocky debris.
  rock: PALETTE.stone,
  rockDark: PALETTE.stoneDark,
  rockLight: lighten(PALETTE.stone, 0.12),

  // Decorative courtyard / market stall awnings.
  awning: PALETTE.terracotta,
  awningDark: PALETTE.terracottaDark,
  courtyard: lighten(PALETTE.sandDark, 0.1),

  // Building details.
  windowGlass: darken(SEA, 0.12), // dark sea-glass panes (now visibly blue)
  windowLit: lighten(PALETTE.goldAccent, 0.12), // warm lit pane
  doorWood: PALETTE.terracottaDark,
  outline: darken(PALETTE.shadow, 0.45), // silhouette outline (darker → buildings pop)
  step: PALETTE.sandDark,
  banner: PALETTE.terracotta,
  bannerAlt: PALETTE.goldAccent,
  pole: PALETTE.stoneDark,
  roofTile: PALETTE.terracottaDark,
  roofTileLight: PALETTE.terracotta,
  crenellation: PALETTE.stoneDark,

  // District boundary walls — sit ON the bounds diamond. Must read clearly
  // against meadow/dirt (stonier / warmer-wood) but stay LIGHTER than building
  // `outline` so walls never overpower roofs/silhouettes. All pure functions of
  // existing PALETTE entries (COLOR CONTRACT).
  wallStone: saturate(darken(PALETTE.stone, 0.04), -0.14),
  wallStoneDark: saturate(darken(PALETTE.stoneDark, 0.14), -0.12),
  wallStoneLight: saturate(lighten(PALETTE.stone, 0.14), -0.1),
  wallWood: saturate(blend(PALETTE.terracottaDark, PALETTE.sandDark, 0.35), 0.04),
  wallWoodDark: darken(PALETTE.terracottaDark, 0.22),
  wallAqueduct: saturate(blend(PALETTE.stone, PALETTE.sandDark, 0.3), -0.1),
  wallAqueductDark: saturate(darken(PALETTE.stoneDark, 0.1), -0.08),

  // Water variants for harbor shimmer (discrete stepped states). The cream-ish
  // PALETTE.water is pulled toward the sea-blue anchor and saturated so harbor
  // patches read as actual BLUE water against the green land — the land/water
  // contrast that defines the city-builder look.
  waterDeep: saturate(darken(SEA, 0.16), 0.12),
  waterMid: SEA,
  waterFoam: saturate(lighten(SEA, 0.28), 0.1),

  // SHORE SAND (Polis terrain frame): the beach/river-bank tiles. Warm dry sand
  // derived from PALETTE.sandDark, lightened so it reads as a bright shore band
  // against both the green land and the blue water. Stays on-palette.
  shoreSand: saturate(lighten(PALETTE.sandDark, 0.14), 0.1),
  shoreSandEdge: darken(PALETTE.sandDark, 0.12),

  // BRIDGE DECK (Polis terrain frame): a raised wooden deck over a river tile.
  // Wood derived from the terracotta family (the city has no native brown) so it
  // reads as timber against the blue water. Two shades for the deck top + the
  // shaded under-rail / posts.
  bridgeWood: saturate(blend(PALETTE.terracottaDark, PALETTE.sandDark, 0.35), 0.04),
  bridgeWoodDark: darken(PALETTE.terracottaDark, 0.22),

  // BRIDGE STONE (Polis terrain frame): raised stone arch bridge piers, arches,
  // side walls, parapets, and end-posts. Derived from the stone family so the
  // bridge reads as weathered limestone against the blue water.
  bridgeStone: saturate(lighten(PALETTE.stone, 0.08), 0.06),
  bridgeStoneDark: saturate(darken(PALETTE.stoneDark, 0.12), 0.08),
  bridgeStoneLight: saturate(lighten(PALETTE.stone, 0.2), 0.05),

  // Smoke / fire (square retro particles + flame shapes).
  smoke: lighten(PALETTE.stoneDark, 0.28),
  // Idle chimney smoke: cool blue-gray so it reads distinct from warm disaster
  // fire smoke (DERIVED.smoke / fire orange family). Blend warm smoke ~18% toward
  // water — still on-palette, no raw hex.
  smokeCool: blend(lighten(PALETTE.stoneDark, 0.28), PALETTE.water, 0.18),
  fireCore: lighten(PALETTE.terracotta, 0.1),
  fireHot: lighten(PALETTE.goldAccent, 0.18),
  ember: PALETTE.terracottaDark,

  // Bug-investigation P3 — the "under investigation" smoke TINT (multiply on the
  // kit `Smoke` container) + a brighter shade for the "?" marker text. Derived by
  // pulling the cool `water` hue ~62% toward the indigo-violet anchor and lightly
  // saturating it, so it stays ON-PALETTE yet reads as a clearly DIFFERENT color
  // family from the orange/red disaster fire — Oracle's guess must never be
  // mistaken for a confirmed disaster (the HONESTY invariant).
  investigate: saturate(blend(PALETTE.water, HUE_INVESTIGATE, 0.62), 0.08),
  investigateMark: lighten(saturate(blend(PALETTE.water, HUE_INVESTIGATE, 0.7), 0.12), 0.22),

  // Provider livery tints — applied to an agent's tunic to hint at the driving
  // model family. Pure derivations from palette hues. No raw hex outside this
  // file. Matched via MODEL_LIVERIES (ordered family → substrings → tint).
  liveryClaude: PALETTE.terracotta,
  liveryOpenai: saturate(blend(PALETTE.water, HUE_TEAL, 0.55), 0.2),
  liveryDeepseek: saturate(blend(PALETTE.water, HUE_INDIGO, 0.6), 0.15),
  // Jade for Qwen; "mimo" remains a match alias only (see MODEL_LIVERIES).
  liveryJade: saturate(blend(PALETTE.grass, HUE_JADE, 0.55), 0.2),
  liveryGrok: saturate(blend(PALETTE.stone, HUE_STEEL, 0.55), 0.08),
} as const;

/**
 * Single source of truth for model-family tunic liveries. Ordered list: first
 * match wins. Matching is case-insensitive against the whole model id
 * (handles OpenRouter-style "provider/model" ids). Unknown → no entry matches
 * → callers keep the default per-seed tunic.
 *
 * Tokens are deliberately specific (e.g. "gpt-4" not "gpt", "mimo-" not "mimo")
 * so unrelated architectures/aliases do not false-match. Exact-id aliases go
 * in `matchExact` (e.g. bare legacy "mimo").
 *
 * Display copy (Legend/Guide) and AgentLayer.liveryTint both consume this table.
 */
export interface ModelLivery {
  /** Human family name shown in legend/guide. */
  family: string;
  /** Case-insensitive substrings; any hit selects this family. */
  match: readonly string[];
  /** Case-insensitive full-id equality matches (after lowercasing). */
  matchExact?: readonly string[];
  /** Palette-derived tunic tint (number 0xRRGGBB). */
  tint: number;
}

export const MODEL_LIVERIES: readonly ModelLivery[] = [
  {
    family: "Claude",
    // Bare aliases ("sonnet"/"opus"/"haiku") can reach Agent.model from some
    // paths; exact-token match avoids substring false-positives (e.g. a model
    // id that merely contains "sonnet" under another family prefix).
    match: ["anthropic", "claude", "opus", "fable"],
    matchExact: ["sonnet", "opus", "haiku"],
    tint: DERIVED.liveryClaude,
  },
  {
    family: "OpenAI",
    match: ["openai/", "gpt-4", "gpt-3", "gpt-5", "o1-", "o3-", "o4-"],
    tint: DERIVED.liveryOpenai,
  },
  {
    family: "DeepSeek",
    match: ["deepseek"],
    tint: DERIVED.liveryDeepseek,
  },
  {
    // "mimo-" / exact "mimo" kept as legacy aliases for the jade family.
    family: "Qwen",
    match: ["qwen", "mimo-"],
    matchExact: ["mimo"],
    tint: DERIVED.liveryJade,
  },
  {
    family: "Grok",
    match: ["x-ai", "grok-", "/grok"],
    tint: DERIVED.liveryGrok,
  },
];

/**
 * Resolve a model id to a livery tint, or `undefined` when unknown / empty.
 * Case-insensitive; ordered first-match. Exact tokens win per-entry before
 * substrings; OpenRouter prefixes ok.
 */
export function modelLiveryTint(model?: string | null): number | undefined {
  if (!model) return undefined;
  const m = model.toLowerCase();
  for (const entry of MODEL_LIVERIES) {
    if (entry.matchExact?.some((ex) => m === ex)) return entry.tint;
    for (const sub of entry.match) {
      if (m.includes(sub)) return entry.tint;
    }
  }
  return undefined;
}

/** Alpha constants reused across the renderer (semantic, not magic numbers). */
export const ALPHA = {
  shadow: 0.32, // drop-shadow under buildings (was 0.18 — buildings were floating)
  districtFill: 0.1,
  districtStroke: 0.5,
  seam: 0.3,
  vignette: 0.22, // peak darkness at screen edges (was 0.42 — was drowning the scene)
} as const;

export type RoofStyle = "flat" | "pitched" | "cone" | "dome" | "merlon";
export type LandmarkElement = "flame" | "flag" | "antenna" | "beacon";

/**
 * Silhouette archetype — drives the per-type geometry upgrades in buildings.ts
 * (temple pediment, tower beacon, fortress turrets, market stall, baths vent,
 * harbor dock, library steps). `plain` = the generic iso box.
 */
export type Silhouette =
  | "plain"
  | "temple"
  | "tower"
  | "fortress"
  | "market"
  | "baths"
  | "harbor"
  | "library"
  | "townhall"
  | "theater";

export interface BuildingProfile {
  /** Single base color the three faces are derived from (lets details re-derive). */
  base: number;
  /** Top (roof) face base color. */
  colorTop: number;
  /** Left (shadowed) face base color. */
  colorLeft: number;
  /** Right (lit) face base color. */
  colorRight: number;
  roofStyle: RoofStyle;
  hasColumns: boolean;
  silhouette: Silhouette;
  landmark?: LandmarkElement;
}

// Build a profile from a single base color, deriving the three faces so the
// 2.5D shading reads consistently across every purpose.
function profile(
  base: number,
  roofStyle: RoofStyle,
  hasColumns: boolean,
  silhouette: Silhouette = "plain",
  landmark?: LandmarkElement,
): BuildingProfile {
  return {
    base,
    colorTop: lighten(base, 0.18),
    colorLeft: darken(base, 0.28),
    colorRight: base,
    roofStyle,
    hasColumns,
    silhouette,
    landmark,
  };
}

// Keys are the stable English slugs. Display labels ("English (Greek)") are a
// separate presentation concern handled by purposeLabel() in types/city.ts.
export const BUILDING_REGISTRY: Record<string, BuildingProfile> = {
  townhall: profile(0xc98f5a, "flat", true, "townhall", "flag"),
  temple: profile(PALETTE.whiteMarble, "pitched", true, "temple", "flame"),
  fortress: profile(PALETTE.stoneDark, "merlon", false, "fortress"),
  market: profile(PALETTE.terracotta, "flat", false, "market"),
  tower: profile(PALETTE.sandDark, "cone", false, "tower", "beacon"),
  house: profile(0xd8a878, "pitched", false, "plain"),
  warehouse: profile(0xb0a080, "flat", false, "plain"),
  workshop: profile(0xb89a70, "pitched", false, "plain"),
  conduit: profile(0x9ab0a8, "flat", false, "plain"),
  baths: profile(0xc9b8d0, "dome", false, "baths"),
  theater: profile(0xe0d2b0, "pitched", true, "theater"),
  lighthouse: profile(PALETTE.ivory, "dome", false, "tower", "beacon"),
  harbor: profile(0x8aaabb, "flat", false, "harbor", "antenna"),
  library: profile(0xd0c8a8, "flat", true, "library"),
  unknown: profile(PALETTE.stone, "flat", false, "plain"),
};

/**
 * Resolve a purpose slug to a visual profile. Unknown / Oracle-introduced
 * slugs fall back to the honest `unknown` profile — never invented geometry.
 */
export function getProfile(purpose: string): BuildingProfile {
  return BUILDING_REGISTRY[purpose] ?? BUILDING_REGISTRY.unknown;
}

// Visual tier → footprint multiplier. Larger files get larger, taller boxes.
// Unknown tiers fall back to the middle tier.
export const TIER_SCALE: Record<string, { w: number; depth: number }> = {
  kalybe: { w: 0.6, depth: 0.5 },
  oikia: { w: 0.8, depth: 0.75 },
  synoikia: { w: 1.0, depth: 1.0 },
  megaron: { w: 1.25, depth: 1.4 },
  mnemeion: { w: 1.55, depth: 1.9 },
};

export function tierScale(tier: string): { w: number; depth: number } {
  return TIER_SCALE[tier] ?? TIER_SCALE.synoikia;
}

/**
 * Approximate "tier rank" 0..4 for detail scaling (window rows, banner odds).
 * Unknown tiers map to the middle rank.
 */
export const TIER_RANK: Record<string, number> = {
  kalybe: 0,
  oikia: 1,
  synoikia: 2,
  megaron: 3,
  mnemeion: 4,
};

export function tierRank(tier: string): number {
  return TIER_RANK[tier] ?? 2;
}

// Agent type → omino + glow color. Mirrors the design doc; augur has no omino
// (invisible agent) but we keep a gold color for its glow if it ever lands on
// a building. `mini` uses a water/aqua hue fitting the watercarrier figure.
// Keep these hex values in lockstep with `agent_color_for_type` in
// src-tauri/src/polis/scanner.rs (frontend wins for glow when type is known).
export const AGENT_COLORS: Record<string, number> = {
  orchestrator: 0x4a9eff, // blue
  coder: 0xffb347, // orange
  verifier: 0x7fd47f, // green
  augur: 0xd4a843, // gold
  mini: 0x5ab8c0, // aqua (watercarrier)
};

// ---------------------------------------------------------------------------
// TECH LIVERY (Polis F4) — provider → roof-pennant accent + a subtle roof tint.
// The 3rd orthogonal channel: district = feature, shape = purpose, LIVERY =
// provider. Only files the Rust scanner tagged with a REAL provider signal
// (`Building.provider`) carry a pennant; pure local code (the common case) has
// none. Procedural pennant drawn once per building in buildings/index.ts.
//
// COLOR CONTRACT: both accents stay ON-PALETTE (cream/terracotta city) — each is
// the brand hue NUDGED via blend() toward a PALETTE anchor + muted, exactly like
// MEADOW/SEA above, so the "all color derives from PALETTE" rule still holds.
//   - cloudflare → its orange, pulled toward terracotta and slightly muted.
//   - scaleway   → its violet, derived by blending the cool `water` toward a
//                  muted violet anchor (PALETTE has no purple of its own).
const HUE_CF_ORANGE = 0xf6821f; // Cloudflare brand orange (blend anchor only)
const HUE_SCW_VIOLET = 0x6f4fd8; // Scaleway brand violet (blend anchor only)

/**
 * Provider → livery accent color (the pennant cloth + roof tint). Muted, derived
 * from the brand hue nudged onto the city palette. Keys are the stable provider
 * slugs (mirror `provider::*` in Rust). Unknown providers fall back via lookup
 * miss → no livery (the caller guards on `provider` being a known key).
 */
export const PROVIDER_LIVERY: Record<string, number> = {
  // Cloudflare orange biased ~30% toward terracotta, then slightly desaturated.
  cloudflare: saturate(blend(PALETTE.terracotta, HUE_CF_ORANGE, 0.62), -0.06),
  // Scaleway violet derived by pulling the cool water hue ~72% toward the violet
  // anchor (the city has no native purple) and lightly saturating it.
  scaleway: saturate(blend(PALETTE.water, HUE_SCW_VIOLET, 0.72), 0.06),
};

/**
 * Resolve a provider slug to its livery accent, or `null` when the building has
 * no provider / an unknown one — the caller then draws NO pennant.
 */
export function providerLivery(provider: string | undefined | null): number | null {
  if (!provider) return null;
  const c = PROVIDER_LIVERY[provider];
  return c === undefined ? null : c;
}

export function agentColor(type: string, fallback?: string): number {
  if (AGENT_COLORS[type] !== undefined) return AGENT_COLORS[type];
  // Honor a backend-provided hex color if present.
  if (fallback && /^#?[0-9a-fA-F]{6}$/.test(fallback)) {
    return parseInt(fallback.replace("#", ""), 16);
  }
  // Keep in lockstep with Rust AGENT_COLOR_DEFAULT (#B0A99F) in scanner.rs.
  return 0xb0a99f;
}
