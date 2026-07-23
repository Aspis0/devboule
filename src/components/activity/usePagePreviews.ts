/**
 * Lazy per-URL page-preview fetch for the Websearch console.
 * Module-level cache so remounts / page rotation don't re-hit the network.
 * Abort-on-unmount: in-flight invokes are ignored after the hook unmounts
 * (Tauri invoke has no AbortSignal; we gate the setState with a cancelled flag).
 */

import { useEffect, useRef, useState } from "react";
import type { PagePreview } from "./websearchPreview";

export type PreviewStatus =
  | { state: "idle" }
  | { state: "loading" }
  | { state: "ready"; preview: PagePreview }
  | { state: "error"; message: string };

const cache = new Map<string, PreviewStatus>();

/** Test/reset seam — not used in production UI. */
export function __resetPagePreviewCacheForTests(): void {
  cache.clear();
}

async function fetchPreview(url: string): Promise<PagePreview> {
  const { invokeBackendCommand } = await import("../../context/AppContext");
  return invokeBackendCommand<PagePreview>("fetch_page_preview", { url });
}

/**
 * Returns a map of url → PreviewStatus for the given URLs.
 * Fetches missing ones in parallel; never throws (errors become `error` status).
 */
export function usePagePreviews(urls: string[]): Record<string, PreviewStatus> {
  const [statuses, setStatuses] = useState<Record<string, PreviewStatus>>(() => {
    const init: Record<string, PreviewStatus> = {};
    for (const u of urls) {
      init[u] = cache.get(u) ?? { state: "idle" };
    }
    return init;
  });
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  useEffect(() => {
    const unique = Array.from(new Set(urls.filter((u) => u.trim().length > 0)));
    if (unique.length === 0) return;

    // Seed from cache immediately so rotation doesn't flash idle.
    setStatuses((prev) => {
      const next = { ...prev };
      let changed = false;
      for (const u of unique) {
        const cached = cache.get(u);
        if (cached && prev[u] !== cached) {
          next[u] = cached;
          changed = true;
        } else if (!prev[u]) {
          next[u] = { state: "idle" };
          changed = true;
        }
      }
      return changed ? next : prev;
    });

    let cancelled = false;

    for (const url of unique) {
      const existing = cache.get(url);
      if (existing?.state === "ready" || existing?.state === "loading") continue;
      if (existing?.state === "error") continue; // don't hammer on hard failures

      const loading: PreviewStatus = { state: "loading" };
      cache.set(url, loading);
      if (mounted.current) {
        setStatuses((prev) => ({ ...prev, [url]: loading }));
      }

      void fetchPreview(url)
        .then((preview) => {
          const ready: PreviewStatus = { state: "ready", preview };
          cache.set(url, ready);
          if (!cancelled && mounted.current) {
            setStatuses((prev) => ({ ...prev, [url]: ready }));
          }
        })
        .catch((err: unknown) => {
          const message =
            err instanceof Error ? err.message : typeof err === "string" ? err : "preview failed";
          const error: PreviewStatus = { state: "error", message };
          cache.set(url, error);
          if (!cancelled && mounted.current) {
            setStatuses((prev) => ({ ...prev, [url]: error }));
          }
        });
    }

    return () => {
      cancelled = true;
    };
  }, [urls.join("\0")]); // stable key for the url set

  return statuses;
}
