// renderProfile.ts — HARDWARE-ADAPTIVE Polis render profile (Phase B2c).
//
// PURE + headless-testable. Given the host's hardware capability (the
// `detect_hardware` Tauri command's wire shape, mirrored here as `HardwareInfo`),
// `profileFor` picks ONE of three tiers — `rich` / `lean` / `minimal` — and
// returns the concrete knobs the renderer reads: LOD zoom thresholds, the B2b
// preload ring, the building-atlas resolution cap, the ambient-walker cap, and the
// WebGL antialias flag.
//
// DESIGN RULES
//   - PURE. No PIXI, no IPC, no globals. `profileFor` is a total function of its
//     input (a HardwareInfo or null) so the whole tier policy is unit-tested with
//     synthetic hardware and no real GPU.
//   - SAFE DEFAULT. `null` hardware (detection failed / still loading) maps to the
//     MIDDLE tier (`lean`) — never the most demanding one. A box we cannot measure
//     gets the conservative profile, so a weak machine we failed to probe is never
//     handed the full-detail city.
//   - MONOTONIC LOD. The label/detail/agent zoom thresholds are NON-DECREASING from
//     rich → lean → minimal: a weaker tier reveals labels/detail LATER (you must
//     zoom in further), so the far overview of a big city stays cheap. Tests pin
//     this ordering so a future edit can't accidentally make `minimal` reveal more
//     than `rich`.
//
// The `rich` tier reproduces the renderer's historical hard-coded defaults exactly
// (LOD_LABELS_IN 0.62 / OUT 0.58, LOD_DETAILS 0.4, LOD_AGENTS 0.35, atlas cap 2,
// antialias on, MAX_AMBIENT 40), so a discrete-GPU box renders byte-for-byte what
// it did before B2c — the profile only ever RELAXES detail on weaker hardware.

/** The host hardware capability snapshot. Mirrors the Rust `HardwareInfo` wire
 *  shape (serde camelCase) returned by the `detect_hardware` Tauri command. Every
 *  field is best-effort: `gpuName`/`gpuKind` fall back to `"unknown"` and `vramGb`
 *  to `null` when the GPU could not be probed. */
export interface HardwareInfo {
  /** Logical CPU core count (>= 1, never 0). */
  cpuCores: number;
  /** Total physical RAM, GiB. */
  ramTotalGb: number;
  /** Currently available RAM, GiB (best-effort). */
  ramAvailableGb: number;
  /** Best-guess primary GPU model, or "unknown" if unprobed. */
  gpuName: string;
  /** Dedicated VRAM in GiB when knowable; `null` for integrated / unified-memory
   *  / unknown. */
  vramGb: number | null;
  /** "integrated" | "discrete" | "unknown". */
  gpuKind: string;
}

/** The render tier. `rich` = full historical detail (discrete GPU); `lean` = the
 *  conservative middle / safe default; `minimal` = the lowest-detail floor. */
export type RenderTier = "rich" | "lean" | "minimal";

/** The concrete render knobs the PolisRenderer reads. Pure data — no PIXI. */
export interface RenderProfile {
  /** Tier label (for the debugLog `PROFILE …` line + tests). */
  tier: RenderTier;
  /** Zoom at/above which building labels are CREATED (LOD label hysteresis IN). */
  lodLabelsIn: number;
  /** Zoom below which building labels are DESTROYED (LOD label hysteresis OUT).
   *  Always strictly below `lodLabelsIn` so the dead-band is non-empty. */
  lodLabelsOut: number;
  /** Zoom at/above which fine building detail reads (below it the container alpha
   *  dips). */
  lodDetails: number;
  /** Zoom at/above which agents/ambient crowd/outposts/disasters show. */
  lodAgents: number;
  /** B2b: how many chunk-rings AROUND the viewport build in the first (priority)
   *  pass. 0 = only the chunks the viewport actually intersects. */
  preloadRing: number;
  /** Cap for the building-atlas texture resolution (combined with the device
   *  pixel ratio: the atlas uses min(dpr, this)). 1 = no HiDPI super-sampling. */
  atlasResolutionCap: number;
  /** Hard cap on the DECORATIVE ambient walker count (fed alongside the city-size
   *  derivation so a weak box renders a smaller crowd). */
  maxAmbientWalkers: number;
  /** WebGL antialias flag for `app.init` (off on weak tiers to save fill-rate). */
  antialias: boolean;
}

// ---------------------------------------------------------------------------
// Tier definitions. The numbers ARE the policy — documented inline. `rich` is the
// historical default set; `lean`/`minimal` relax detail monotonically.
// ---------------------------------------------------------------------------

/** RICH — discrete GPU with real VRAM. The renderer's historical defaults
 *  verbatim, so a capable box is unchanged by B2c. */
const RICH: RenderProfile = {
  tier: "rich",
  lodLabelsIn: 0.62,
  lodLabelsOut: 0.58,
  lodDetails: 0.4,
  lodAgents: 0.35,
  preloadRing: 2,
  atlasResolutionCap: 2,
  maxAmbientWalkers: 40,
  antialias: true,
};

/** LEAN — integrated / unknown GPU, low VRAM, or a modest CPU. Also the SAFE
 *  DEFAULT for `null` (unprobed) hardware. Labels/detail reveal a touch later, a
 *  single preload ring, no HiDPI atlas super-sampling, antialias off, a smaller
 *  crowd. Thresholds are >= the rich ones (monotonic). */
const LEAN: RenderProfile = {
  tier: "lean",
  lodLabelsIn: 0.85,
  lodLabelsOut: 0.8,
  lodDetails: 0.55,
  lodAgents: 0.45,
  preloadRing: 1,
  atlasResolutionCap: 1,
  maxAmbientWalkers: 18,
  antialias: false,
};

/** MINIMAL — the lowest floor (tiny VRAM or a 1-4 core box). Labels/detail only
 *  when zoomed in CLOSE, NO preload ring (only the visible chunks build first), a
 *  minimal crowd. Thresholds are >= the lean ones (monotonic). */
const MINIMAL: RenderProfile = {
  tier: "minimal",
  lodLabelsIn: 1.1,
  lodLabelsOut: 1.0,
  lodDetails: 0.75,
  lodAgents: 0.6,
  preloadRing: 0,
  atlasResolutionCap: 1,
  maxAmbientWalkers: 6,
  antialias: false,
};

// ---------------------------------------------------------------------------
// Tier classification thresholds (documented heuristic).
// ---------------------------------------------------------------------------

/** A discrete GPU needs at least this much dedicated VRAM (GiB) to earn `rich`. */
const RICH_MIN_VRAM_GB = 4;
/** A box drops to `minimal` below this VRAM (GiB) — a tiny/old discrete part. */
const MINIMAL_MAX_VRAM_GB = 1.5;
/** Below this core count a box can never be `rich` (and at/below the minimal
 *  floor it is forced to `minimal`). */
const RICH_MIN_CORES = 8;
/** At/below this core count the box is forced to `minimal` regardless of GPU. */
const MINIMAL_MAX_CORES = 4;
/** Apple Silicon unified-memory RICH gate: minimum total RAM (GiB). Unified memory doubles as the VRAM pool, so total RAM is the capability signal. */
const APPLE_SILICON_RICH_MIN_RAM_GB = 32;

/**
 * Pick the render tier for a hardware snapshot. PURE.
 *
 * Heuristic (most-restrictive wins):
 *   - `null` hw → `lean` (the SAFE middle default — never `rich`, never the
 *     cheapest; an unmeasured box gets the conservative profile).
 *   - `minimal` when the box is genuinely weak: a known VRAM below
 *     {@link MINIMAL_MAX_VRAM_GB}, OR a core count at/below {@link MINIMAL_MAX_CORES}.
 *   - `rich` for Apple Silicon unified-memory boxes: integrated GPU with an
 *     "Apple M*" name, ramTotalGb >= {@link APPLE_SILICON_RICH_MIN_RAM_GB}, and at least {@link RICH_MIN_CORES} cores.
 *     Apple Silicon's unified memory IS the VRAM pool, so these machines deserve
 *     full detail even though gpuKind is "integrated".
 *   - `rich` ALSO for a discrete GPU with VRAM at/above {@link RICH_MIN_VRAM_GB}
 *     AND at least {@link RICH_MIN_CORES} cores.
 *   - everything else (non-Apple integrated/unknown GPU, <4GB VRAM, <8 cores)
 *     → `lean`.
 *
 * The `minimal` test runs FIRST so a tiny-VRAM discrete part (or a 4-core box)
 * lands on the floor even if it would otherwise look discrete; `rich` is the most
 * demanding gate and runs only after the floor is ruled out.
 */
export function profileFor(hw: HardwareInfo | null): RenderProfile {
  // Unprobed / detection failed → the safe middle tier. Never the richest.
  if (!hw) return LEAN;

  const cores = Number.isFinite(hw.cpuCores) ? hw.cpuCores : 1;
  // A null/NaN VRAM is treated as "no dedicated VRAM" (integrated / unknown), NOT
  // as "tiny" — `minimal` is reserved for a KNOWN small dedicated card. So the
  // VRAM floor test only fires when vramGb is a real positive number below the cap.
  const vram =
    typeof hw.vramGb === "number" && Number.isFinite(hw.vramGb) && hw.vramGb > 0
      ? hw.vramGb
      : null;

  // FLOOR first: a genuinely weak box (tiny known VRAM, or a 1-4 core CPU).
  if (cores <= MINIMAL_MAX_CORES) return MINIMAL;
  if (vram !== null && vram < MINIMAL_MAX_VRAM_GB) return MINIMAL;

  // RICH — Apple Silicon unified memory. Apple Silicon's unified memory IS the
  // VRAM pool: the GPU shares the full system RAM, so an M1 Max with 64GB RAM
  // has far more usable VRAM than a 4GB discrete card. The old conservative
  // default starved these machines (lean: antialias off, 18 walkers, half-res
  // atlas) once hero-fire counts became tier-gated. We require all four signals
  // — integrated kind, Apple M* name, ramTotalGb >= APPLE_SILICON_RICH_MIN_RAM_GB,
  // at least RICH_MIN_CORES cores — to avoid a false-positive classification
  // on low-RAM M-series or non-Apple integrated GPUs.
  const ram = Number.isFinite(hw.ramTotalGb) && hw.ramTotalGb > 0 ? hw.ramTotalGb : 0;
  if (
    hw.gpuKind === "integrated"
    && /^Apple M/.test(hw.gpuName) // exact-case prefix: the probe emits "Apple M…" verbatim; anomalous casing = ambiguous signal = stay conservative
    && ram >= APPLE_SILICON_RICH_MIN_RAM_GB
    && cores >= RICH_MIN_CORES
  ) {
    return RICH;
  }

  // RICH: only a discrete card with real VRAM on a capable CPU.
  if (
    hw.gpuKind === "discrete" &&
    vram !== null &&
    vram >= RICH_MIN_VRAM_GB &&
    cores >= RICH_MIN_CORES
  ) {
    return RICH;
  }

  // FIX 5 (policy, no behavior change) — a DISCRETE GPU reporting vramGb=0/unknown
  // is DELIBERATELY classified LEAN here, not RICH. The `vram !== null` guard above
  // fails for it (vram was nulled when vramGb<=0/NaN), so it falls through. This is
  // the Optimus/DXGI quirk: a laptop's discrete NVIDIA part advertises 0 dedicated
  // VRAM through the DXGI adapter the probe reads, even though it is a real card.
  // We choose CONSERVATIVE (LEAN) over optimistic (RICH) for that ambiguous signal
  // — a wrongly-LEAN box just renders a slightly lighter scene; a wrongly-RICH one
  // can stutter. The `PROFILE …` debugLog line carries gpuName/gpuKind/tier so a
  // misclassified box is diagnosable from the log without code changes.
  // Everything else (integrated/unknown GPU, sub-threshold VRAM/cores): the
  // conservative middle.
  return LEAN;
}

/** The three tier profiles, exported read-only so consumers/tests can reference
 *  the canonical objects (e.g. to assert the renderer applied a tier's exact
 *  thresholds) without re-deriving them. */
export const RENDER_PROFILES: Readonly<Record<RenderTier, RenderProfile>> =
  Object.freeze({ rich: RICH, lean: LEAN, minimal: MINIMAL });
