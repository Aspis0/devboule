// Pure utility: resolve an Oracle citation's `fileSource` to a Polis Building.
//
// Citations carry index-root-relative paths (e.g. "src/worker.ts") while Polis
// buildings carry absolute paths (e.g. "/home/user/project/src/worker.ts").
// The lookup therefore tries two strategies:
//   1. EXACT match on `building.filePath` (fast path, covers same-root cases).
//   2. SUFFIX match: normalize separators on both sides and check whether the
//      building's filePath ends with the citation's fileSource. This is the
//      load-bearing case for the common scenario where citations are relative to
//      the indexed workspace root while Polis paths are absolute.
//
// Separator normalization converts all `\` to `/` before comparison so Windows
// and POSIX paths compare correctly.

import type { Building, CityState } from "../../types/city";

/** Replace all backslashes with forward slashes (Windows → POSIX normalization). */
function norm(p: string): string {
  return p.replace(/\\/g, "/");
}

/**
 * Find the Polis {@link Building} that best matches an Oracle citation's
 * `fileSource`. Returns `null` when no building matches (the cited file is not
 * in the current map).
 *
 * @param city       The current CityState (may be null — returns null).
 * @param fileSource The citation's `fileSource` field (index-root-relative path).
 */
export function findBuildingByCitation(
  city: CityState | null,
  fileSource: string,
): Building | null {
  if (!city || !fileSource) return null;

  const normalizedSource = norm(fileSource.trim());
  if (!normalizedSource) return null;

  // Pass 1: exact match.
  for (const building of city.buildings) {
    if (norm(building.filePath) === normalizedSource) {
      return building;
    }
  }

  // Pass 2: suffix match. The building's absolute path must END with the
  // citation's relative path. Guard against accidental partial-name matches by
  // requiring either a path separator or the start of the string before the
  // suffix. E.g. "src/worker.ts" should NOT match "other_src/worker.ts" as a
  // pure suffix — we require that the char before the suffix is "/".
  for (const building of city.buildings) {
    const normalizedPath = norm(building.filePath);
    if (normalizedPath.endsWith(normalizedSource)) {
      const prefixLen = normalizedPath.length - normalizedSource.length;
      // Ensure the match starts at a path boundary (or at position 0).
      if (prefixLen === 0 || normalizedPath[prefixLen - 1] === "/") {
        return building;
      }
    }
  }

  return null;
}
