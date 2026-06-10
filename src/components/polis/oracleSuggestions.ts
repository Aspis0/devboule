// Pure utility: derive Oracle suggestion chips from the current CityState.
//
// Also exports the seed questions previously local to OracleView; OracleView
// imports from here after Phase 4(a) so both surfaces share the same base set.

import type { CityState } from "../../types/city";

// Corpus-agnostic example questions: Oracle indexes whatever folder the user
// maps, so prompts must not assume a specific subsystem of this repo. These
// read as generic "understand/locate code" tasks that hold for any codebase.
export const seedQuestions = [
  "How does Cloudflare Worker secret rotation work?",
  "Which files control Scaleway GPU and CPU VM lifecycle actions?",
  "Which files should I read to understand how the main components fit together?",
];

/**
 * Derive 4–6 Oracle query suggestions from the loaded CityState.
 *
 * Strategy:
 *  1. Collect distinct district/feature labels that look meaningful (non-empty,
 *     not the catch-all "default" / "commons" label).
 *  2. Collect the labels of the largest buildings (by linesOfCode) — the
 *     heaviest files are the most likely targets for a focused question.
 *  3. Build one suggestion per distinct label (capped at 3 each, to avoid
 *     overwhelming the chip bar).
 *  4. Pad with seed questions until we have at least 3 total; cap at 6.
 *
 * Pure: no Date, no Math.random, no side-effects. Same input → same output.
 */
export function buildOracleSuggestions(city: CityState | null): string[] {
  if (!city) return [...seedQuestions];

  const suggestions: string[] = [];
  const seen = new Set<string>();

  // Helper: add a suggestion if not already present (deduplication).
  const add = (s: string) => {
    const trimmed = s.trim();
    if (!trimmed || seen.has(trimmed)) return;
    seen.add(trimmed);
    suggestions.push(trimmed);
  };

  // --- District / feature labels ---------------------------------------
  // Prefer oracle-named features (F2) as they carry richer human descriptions.
  const featureLabels: string[] = [];
  if (city.features && city.features.length > 0) {
    for (const feature of city.features) {
      const label = feature.label?.trim();
      if (
        label &&
        label !== "default" &&
        label !== "commons" &&
        label !== "Default" &&
        label !== "Commons"
      ) {
        featureLabels.push(label);
      }
    }
  } else {
    // Fall back to district names when no features are present.
    for (const district of city.districts) {
      const name = district.name?.trim();
      if (name && name !== "default" && name !== "Default") {
        featureLabels.push(name);
      }
    }
  }

  // Deterministic order: alphabetical, then cap at 3.
  const sortedFeatureLabels = [...featureLabels].sort().slice(0, 3);
  for (const label of sortedFeatureLabels) {
    add(`What does the ${label} area do?`);
  }

  // --- Heaviest buildings (most lines of code) -------------------------
  // Sort descending by linesOfCode; take the top 3 by distinct label.
  const sorted = [...city.buildings].sort(
    (a, b) => (b.linesOfCode ?? 0) - (a.linesOfCode ?? 0),
  );
  let buildingCount = 0;
  for (const building of sorted) {
    if (buildingCount >= 3) break;
    const label = building.label?.trim();
    if (!label) continue;
    add(`What is the role of ${label}?`);
    buildingCount++;
  }

  // --- Pad with seed questions -----------------------------------------
  for (const q of seedQuestions) {
    if (suggestions.length >= 6) break;
    add(q);
  }

  // Return at least the seed questions if derivation produced nothing.
  return suggestions.length > 0 ? suggestions.slice(0, 6) : [...seedQuestions];
}
