// Path-B canvas: a same-origin `srcDoc` iframe (CSS isolation) the trusted parent
// reaches into. The iframe is sandboxed WITHOUT `allow-scripts` — the content has
// none, so this is pure defense in depth. On `load` (and whenever the project
// changes) the parent injects sanitized, positioned host elements into the
// iframe's `contentDocument`; drag/resize mutates the live host style and commits
// once on pointer-up via the pure engine.

import { useCallback, useEffect, useRef } from "react";
import type { DesignManifest, DesignProject } from "../../types/design";
import {
  buildShellHtml,
  CANVAS_ROOT_ID,
  injectNodes,
  NODE_ID_ATTR,
} from "./iframeInject";
import { useDrag, type DragMode } from "./useDrag";

export interface CanvasProps {
  project: DesignProject;
  /** Commit a new manifest (drag/resize/bring-to-front). */
  onManifestChange: (next: DesignManifest) => void;
  /** Fired on pointerdown over a node host (its id) or empty canvas (null). */
  onSelect?: (id: string | null) => void;
}

const SHELL_HTML = buildShellHtml();

export function Canvas({ project, onManifestChange, onSelect }: CanvasProps) {
  const iframeRef = useRef<HTMLIFrameElement>(null);
  // Unbinder for the current pointer-delegation listener (null when none bound).
  // Held in a ref so a reload/remount can detach the PREVIOUS listener before
  // binding a new one — never stacking handlers.
  const unbindRef = useRef<(() => void) | null>(null);

  // Read the live iframe document (null until loaded / after a reload). The
  // sandbox is same-origin, so `contentDocument` is reachable.
  const getDoc = useCallback((): Document | null => {
    return iframeRef.current?.contentDocument ?? null;
  }, []);

  // Keep a live project ref so the long-lived drag/getManifest closures never
  // read a stale manifest.
  const projectRef = useRef(project);
  projectRef.current = project;

  // Live onSelect ref so the delegated pointer handler stays stable (never
  // re-binds just because the parent passed a new callback identity).
  const onSelectRef = useRef(onSelect);
  onSelectRef.current = onSelect;

  const { beginDrag } = useDrag({
    getDoc,
    getManifest: () => projectRef.current.manifest,
    grid: project.meta.canvas.grid,
    onCommit: onManifestChange,
  });

  // The document a delegation listener is currently bound to. Lets the
  // ensure-bind effect detect a live-document swap (srcDoc reload yields a NEW
  // contentDocument) and re-bind, while staying a no-op when the document is
  // unchanged — so it binds exactly once per live document, not once per render.
  const boundDocRef = useRef<Document | null>(null);

  // Delegate pointerdown on hosts to start a drag. A single delegated listener on
  // the iframe document avoids per-host listeners (and re-binding on every
  // inject). Rebroadcast clientX/Y are iframe-relative but the drag math only
  // uses deltas, so the origin cancels out.
  const bindPointerDelegation = useCallback(
    (doc: Document) => {
      const handler = (ev: Event) => {
        const e = ev as PointerEvent;
        const target = e.target as Element | null;
        const host = target?.closest(`[${NODE_ID_ATTR}]`) as HTMLElement | null;
        if (!host) {
          onSelectRef.current?.(null); // pointerdown on empty canvas clears selection
          return;
        }
        const id = host.getAttribute(NODE_ID_ATTR);
        if (!id) return;
        onSelectRef.current?.(id);
        // A pointerdown near the bottom-right corner (within 16px) resizes;
        // otherwise it moves. Kept deliberately simple for Phase 1b.
        const rect = host.getBoundingClientRect();
        const nearCorner =
          e.clientX >= rect.right - 16 && e.clientY >= rect.bottom - 16;
        const mode: DragMode = nearCorner ? "resize" : "move";
        e.preventDefault();
        beginDrag(id, mode, e);
      };
      doc.addEventListener("pointerdown", handler);
      return () => doc.removeEventListener("pointerdown", handler);
    },
    [beginDrag],
  );

  // The single gated path that both injects nodes AND ensures the pointer
  // delegation listener is bound on the LIVE document — so the two can never
  // diverge (the original bug: nodes injected by `reinject` while the listener,
  // bound only in `onLoad`, was never wired because `contentDocument` was null at
  // the `onLoad` instant). Returns true once the canvas root exists so a caller
  // can stop polling.
  //
  // WHY not rely on `onLoad`: in WebView2 a `srcDoc` + `sandbox="allow-same-origin"`
  // iframe can fire `load` before `contentDocument` is populated (or with a
  // transient `about:blank`), and the old handler `return`ed early with no retry —
  // leaving the listener unbound forever. This path is timing-independent: it runs
  // on project change AND on an interval poll until it succeeds.
  const ensureReady = useCallback((): boolean => {
    const doc = getDoc();
    if (!doc || !doc.getElementById(CANVAS_ROOT_ID)) return false;
    // Inject is idempotent (keyed reconcile) — safe to call on every tick.
    injectNodes(doc, projectRef.current);
    // Bind exactly once per live document. A srcDoc reload yields a NEW document
    // object; detach the previous listener (it lived on the dead document) and
    // bind on the new one. No-op when already bound to this same document, so the
    // poll never stacks handlers.
    if (boundDocRef.current !== doc) {
      unbindRef.current?.();
      unbindRef.current = bindPointerDelegation(doc);
      boundDocRef.current = doc;
    }
    return true;
  }, [getDoc, bindPointerDelegation]);

  // `onLoad` is now a best-effort fast path (not the only one). It just kicks
  // `ensureReady`; if the document isn't ready yet the poll below covers it.
  const handleLoad = useCallback(() => {
    ensureReady();
  }, [ensureReady]);

  // Inject + ensure-bind on every project change, and — crucially —
  // independently of `onLoad` via a short poll that survives WebView2 srcDoc
  // timing. The poll self-stops as soon as the canvas root is available and the
  // listener is bound; if the document is later swapped (reload) the project
  // effect re-runs this and `ensureReady` re-binds to the new document.
  useEffect(() => {
    if (ensureReady()) return;
    let cancelled = false;
    const timer = setInterval(() => {
      if (cancelled || ensureReady()) {
        clearInterval(timer);
      }
    }, 50);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [project, ensureReady]);

  // Unbind pointer delegation on unmount (reads the live ref, never a stale one).
  useEffect(() => {
    return () => {
      unbindRef.current?.();
      unbindRef.current = null;
      boundDocRef.current = null;
    };
  }, []);

  return (
    <div className="relative h-full w-full overflow-auto rounded-2xl border border-cream-200 bg-white">
      <iframe
        ref={iframeRef}
        title="Design canvas"
        onLoad={handleLoad}
        // No `allow-scripts`: the sanitized content has no scripts; this blocks
        // execution even if a sanitizer miss ever slipped through (defense in depth).
        sandbox="allow-same-origin"
        srcDoc={SHELL_HTML}
        className="h-full w-full border-0"
        style={{ minHeight: project.meta.canvas.h }}
      />
    </div>
  );
}
