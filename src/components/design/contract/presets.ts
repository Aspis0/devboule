// Design-contract PRESETS — generic starting points for a project's design.md +
// matching DTCG tokens (Phase C).
//
// When a target has NO extractable design signal (no chunks, or extraction came up
// empty), the contract editor offers these presets so the user starts from a sane,
// concrete aesthetic instead of a blank page. They are GENERIC (Tailwind-ish /
// Material-ish / minimal-neutral) — NOT Devboule, NOT the management UI's own theme.
//
// PURE DATA: no DOM, no clock, no random. Each preset's `designMd` is a small
// markdown contract (palette w/ concrete hex, type scale, spacing, radii, component
// conventions, tone) and its `tokens` is a DTCG document with REAL `$value`s that
// round-trips through engine/tokens.ts (validateTokensDoc / colorSwatches).

import type { DtcgDocument } from "../engine/tokens";

/** Logical alias for a DTCG token document (the spec's `DesignTokensDoc`). The engine
 * models this as `DtcgDocument`; this alias keeps the contract code self-documenting. */
export type DesignTokensDoc = DtcgDocument;

/** Bump when ANY preset's wording / tokens change, so a saved contract can be traced
 * to the preset revision it came from (audit parity with DESIGN_PROMPT_VERSION). */
export const PRESETS_VERSION = 1;

/** One selectable preset: an id, a human label/description, a markdown contract, and
 * the matching DTCG token document written alongside it on Save. */
export interface DesignPreset {
  id: string;
  name: string;
  description: string;
  /** The design.md contract body (markdown). Kept WELL under 4 KiB. */
  designMd: string;
  /** The DTCG tokens written to tokens.json when this preset is chosen. */
  tokens: DesignTokensDoc;
}

// --- token-doc builders (concrete $values; color/typography/spacing/radius) -------

function colorTok(value: string, description?: string) {
  return description
    ? { $value: value, $type: "color", $description: description }
    : { $value: value, $type: "color" };
}
function dimTok(value: string) {
  return { $value: value, $type: "dimension" };
}
function fontTok(value: string) {
  return { $value: value, $type: "fontFamily" };
}

// ----------------------------------------------------------------------------------
// 1) Tailwind-ish defaults — the familiar slate/indigo system, 4px spacing rhythm.
// ----------------------------------------------------------------------------------

const TAILWIND_TOKENS: DesignTokensDoc = {
  color: {
    brand: colorTok("#4f46e5", "Indigo 600 — primary action"),
    "brand-strong": colorTok("#4338ca"),
    accent: colorTok("#0ea5e9"),
    ink: colorTok("#0f172a", "Slate 900 — body text"),
    muted: colorTok("#64748b"),
    border: colorTok("#e2e8f0"),
    surface: colorTok("#ffffff"),
    "surface-alt": colorTok("#f8fafc"),
    danger: colorTok("#dc2626"),
    success: colorTok("#16a34a"),
  },
  typography: {
    sans: fontTok("ui-sans-serif, system-ui, 'Segoe UI', Roboto, Helvetica, Arial"),
    mono: fontTok("ui-monospace, 'Cascadia Code', 'SF Mono', Menlo, monospace"),
  },
  spacing: {
    xs: dimTok("4px"),
    sm: dimTok("8px"),
    md: dimTok("16px"),
    lg: dimTok("24px"),
    xl: dimTok("40px"),
  },
  radius: {
    sm: dimTok("4px"),
    md: dimTok("8px"),
    lg: dimTok("12px"),
    full: dimTok("9999px"),
  },
};

const TAILWIND_MD = `# Design contract — Tailwind-ish defaults

A clean, familiar utility-first system: slate neutrals, an indigo primary, generous
whitespace on a 4px rhythm. Modern, accessible, slightly rounded.

## Palette
- Primary: \`#4f46e5\` (indigo 600); hover/active \`#4338ca\`.
- Accent: \`#0ea5e9\` (sky 500) for highlights and links.
- Text: \`#0f172a\` (slate 900); muted/secondary \`#64748b\`.
- Surfaces: \`#ffffff\` cards on a \`#f8fafc\` page; borders \`#e2e8f0\`.
- Status: danger \`#dc2626\`, success \`#16a34a\`.

## Typography
- Sans: system UI stack (\`ui-sans-serif, system-ui, 'Segoe UI', Roboto\`).
- Scale: 12 / 14 / 16 / 20 / 24 / 30 / 36 px. Body 16px, line-height 1.5.
- Weights: 400 body, 500 labels, 600 headings.

## Spacing & radii
- Spacing scale (4px base): 4, 8, 16, 24, 40 px.
- Radii: 4 (inputs), 8 (cards), 12 (modals), 9999 (pills).

## Components
- Buttons: solid indigo primary, 8px radius, 10/16 padding; ghost = transparent
  with slate text. Focus ring 2px accent.
- Cards: white surface, 1px \`#e2e8f0\` border, 8px radius, soft shadow.
- Inputs: 1px border, 4px radius, 8/12 padding, accent focus ring.

## Tone
Crisp, professional, neutral. Prefer clarity over decoration.`;

// ----------------------------------------------------------------------------------
// 2) Material-ish — elevated surfaces, a teal/purple pair, larger radii.
// ----------------------------------------------------------------------------------

const MATERIAL_TOKENS: DesignTokensDoc = {
  color: {
    brand: colorTok("#6750a4", "Primary (Material purple)"),
    "brand-strong": colorTok("#52439a"),
    accent: colorTok("#03dac6", "Secondary / teal"),
    ink: colorTok("#1c1b1f"),
    muted: colorTok("#49454f"),
    border: colorTok("#cac4d0"),
    surface: colorTok("#fffbfe"),
    "surface-alt": colorTok("#f4eff4"),
    danger: colorTok("#b3261e"),
    success: colorTok("#2e7d32"),
  },
  typography: {
    sans: fontTok("Roboto, 'Segoe UI', system-ui, sans-serif"),
    mono: fontTok("'Roboto Mono', ui-monospace, monospace"),
  },
  spacing: {
    xs: dimTok("4px"),
    sm: dimTok("8px"),
    md: dimTok("16px"),
    lg: dimTok("24px"),
    xl: dimTok("48px"),
  },
  radius: {
    sm: dimTok("8px"),
    md: dimTok("16px"),
    lg: dimTok("28px"),
    full: dimTok("9999px"),
  },
};

const MATERIAL_MD = `# Design contract — Material-ish

A Material-flavoured system: a purple primary with a teal secondary, elevated
surfaces, soft shadows, and noticeably rounded corners.

## Palette
- Primary: \`#6750a4\`; pressed \`#52439a\`.
- Secondary / accent: \`#03dac6\` (teal).
- Text: \`#1c1b1f\`; muted \`#49454f\`.
- Surfaces: \`#fffbfe\` on \`#f4eff4\`; outline \`#cac4d0\`.
- Status: error \`#b3261e\`, success \`#2e7d32\`.

## Typography
- Sans: \`Roboto, 'Segoe UI', system-ui\`.
- Scale: 12 / 14 / 16 / 22 / 28 / 36 px. Body 16px, line-height 1.5.
- Weights: 400 body, 500 labels, 700 headings.

## Spacing & radii
- Spacing (4px base): 4, 8, 16, 24, 48 px.
- Radii: 8 (chips/inputs), 16 (cards), 28 (sheets/FAB), 9999 (pills).

## Components
- Buttons: filled purple with elevation, 16px radius; tonal = surface-alt fill.
  Ripple-style press affordance.
- Cards: \`#fffbfe\` surface, 16px radius, layered shadow (no hard border).
- Inputs: filled or outlined, 8px radius, label that floats on focus.

## Tone
Tactile and friendly with clear elevation hierarchy.`;

// ----------------------------------------------------------------------------------
// 3) Minimal neutral — monochrome, hairline borders, almost no radius.
// ----------------------------------------------------------------------------------

const MINIMAL_TOKENS: DesignTokensDoc = {
  color: {
    brand: colorTok("#111111", "Near-black primary"),
    "brand-strong": colorTok("#000000"),
    accent: colorTok("#2563eb"),
    ink: colorTok("#111111"),
    muted: colorTok("#6b7280"),
    border: colorTok("#e5e5e5"),
    surface: colorTok("#ffffff"),
    "surface-alt": colorTok("#fafafa"),
    danger: colorTok("#991b1b"),
    success: colorTok("#166534"),
  },
  typography: {
    sans: fontTok("system-ui, -apple-system, 'Segoe UI', Helvetica, Arial"),
    mono: fontTok("ui-monospace, Menlo, Consolas, monospace"),
  },
  spacing: {
    xs: dimTok("4px"),
    sm: dimTok("8px"),
    md: dimTok("16px"),
    lg: dimTok("32px"),
    xl: dimTok("64px"),
  },
  radius: {
    sm: dimTok("2px"),
    md: dimTok("4px"),
    lg: dimTok("6px"),
    full: dimTok("9999px"),
  },
};

const MINIMAL_MD = `# Design contract — Minimal neutral

A restrained monochrome system: near-black on white, hairline borders, almost no
corner radius, lots of breathing room. Let typography and spacing do the work.

## Palette
- Primary: \`#111111\`; strongest \`#000000\`.
- Accent (links only): \`#2563eb\`.
- Text: \`#111111\`; muted \`#6b7280\`.
- Surfaces: \`#ffffff\` on \`#fafafa\`; hairline border \`#e5e5e5\`.
- Status: danger \`#991b1b\`, success \`#166534\`.

## Typography
- Sans: \`system-ui, -apple-system, 'Segoe UI'\`.
- Scale: 13 / 15 / 17 / 21 / 28 / 40 px. Body 15px, line-height 1.6.
- Weights: 400 body, 500 emphasis, 600 headings. Tight tracking on headings.

## Spacing & radii
- Spacing (generous, 4px base): 4, 8, 16, 32, 64 px.
- Radii: 2 (inputs), 4 (cards), 6 (modals), 9999 (pills). Keep it square-ish.

## Components
- Buttons: solid black primary, 4px radius, 9/14 padding; secondary = 1px border.
- Cards: white surface, 1px \`#e5e5e5\` border, 4px radius, NO shadow.
- Inputs: bottom-border or 1px box, 2px radius, generous 10/12 padding.

## Tone
Quiet, editorial, confident. Subtract until only the essentials remain.`;

/** The versioned preset catalog surfaced by the contract editor's preset picker. */
export const PRESET_CATALOG: readonly DesignPreset[] = [
  {
    id: "tailwind-defaults",
    name: "Tailwind defaults",
    description: "Slate neutrals, indigo primary, 4px rhythm. Familiar utility look.",
    designMd: TAILWIND_MD,
    tokens: TAILWIND_TOKENS,
  },
  {
    id: "material-ish",
    name: "Material-ish",
    description: "Purple + teal, elevated surfaces, rounded corners.",
    designMd: MATERIAL_MD,
    tokens: MATERIAL_TOKENS,
  },
  {
    id: "minimal-neutral",
    name: "Minimal neutral",
    description: "Monochrome, hairline borders, near-zero radius, airy spacing.",
    designMd: MINIMAL_MD,
    tokens: MINIMAL_TOKENS,
  },
] as const;

/** Look a preset up by id (used when Save needs the chosen preset's tokens). */
export function presetById(id: string): DesignPreset | undefined {
  return PRESET_CATALOG.find((p) => p.id === id);
}
