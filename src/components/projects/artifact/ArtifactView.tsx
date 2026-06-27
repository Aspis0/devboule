// ArtifactView — the interactive-artifact render host (plan `bubbly-hopping-valiant.md`,
// Phase 1). Renders a stored interactive artifact in a sandboxed, separate-origin iframe.
//
// Adapted from NextChat's `app/components/artifacts.tsx` HTMLPreview (MIT, Copyright
// 2023-2025 ChatGPTNextWeb / NextChat). We keep its structure — a sandboxed iframe whose
// guest posts its layout height back to the parent, which sizes the frame — but DELIVER the
// document from a separate origin (PATH B: `src`, not `srcDoc`) so the artifact carries its
// OWN CSP and runs real inline JS without inheriting the app's `script-src 'self'`, and we
// REPLACE NextChat's weaker `id == frameId` string check with the source-identity trust
// anchor `event.source === iframe.contentWindow` (see `artifactProtocol.ts`). A
// THIRD_PARTY_NOTICES.md entry will be added at the feature's completion.
//
// SECURITY: `sandbox="allow-scripts"` WITHOUT `allow-same-origin` ⇒ opaque origin (the guest
// cannot reach `window.parent`, `__TAURI_INTERNALS__`, app cookies/storage); the served CSP
// adds `connect-src 'none'` (no exfiltration). NO `allow-same-origin`, NO `allow-top-navigation`.

import { useEffect, useMemo, useRef, useState } from "react";
import {
  ARTIFACT_MIN_HEIGHT,
  buildArtifactSrc,
  clampArtifactHeight,
  isFromFrame,
  parseArtifactMessage,
} from "./artifactProtocol";

export interface ArtifactViewProps {
  /** Design id resolved by the `artifact:` scheme handler to a stored `artifact/index.html`.
   *  Reserved dev routes (`__sample__`, `__spike__`) are also valid ids here. */
  artifactId: string;
  /** Accessible iframe title. */
  title?: string;
  /** Minimum height (px) before the guest reports its real layout height. */
  minHeight?: number;
  /** Wrapper className (the iframe fills the wrapper's width). */
  className?: string;
  /** Fired once when the guest signals `artifact:ready`. */
  onReady?: () => void;
  /** Fired (non-fatally) on every `artifact:error` from the guest (already ≤300 chars). */
  onError?: (message: string) => void;
  /**
   * When `false`, the iframe fills its container (`height:100%`) and `artifact:resize` height
   * updates are IGNORED — the correct mode inside a fixed-dimension device bezel (android/ios/web)
   * where the device screen area is already fixed and the artifact should scroll internally.
   * When `true` (the default), the iframe grows to match the artifact's reported content height
   * (the existing auto-resize behaviour). All security/trust/dispose logic is UNCHANGED by this
   * flag; resize messages are still RECEIVED and parsed — they are just not applied to the height.
   */
  autoResize?: boolean;
  /**
   * Pre-populate the error banner with this message. Useful for restoring persisted error
   * state or for testing the overlay positioning without triggering a postMessage flow.
   */
  defaultError?: string;
}

export function ArtifactView({
  artifactId,
  title = "Interactive artifact",
  minHeight = ARTIFACT_MIN_HEIGHT,
  className,
  onReady,
  onError,
  autoResize = true,
  defaultError,
}: ArtifactViewProps) {
  const iframeRef = useRef<HTMLIFrameElement | null>(null);
  const [height, setHeight] = useState(minHeight);
  const [error, setError] = useState<string | null>(defaultError ?? null);

  const src = useMemo(() => buildArtifactSrc(artifactId), [artifactId]);

  // Keep the latest callbacks / min / autoResize in refs so the single 'message' subscription
  // below never goes stale and never needs to re-subscribe (which would drop in-flight guest
  // messages). Matches the dispose discipline from `useDesignStream.ts:133-143`.
  const onReadyRef = useRef(onReady);
  const onErrorRef = useRef(onError);
  const minHeightRef = useRef(minHeight);
  const autoResizeRef = useRef(autoResize);
  useEffect(() => {
    onReadyRef.current = onReady;
    onErrorRef.current = onError;
    minHeightRef.current = minHeight;
    autoResizeRef.current = autoResize;
  });

  // Reset per-artifact state when the id changes (the iframe is also remounted via `key`).
  useEffect(() => {
    setHeight(minHeightRef.current);
    setError(null);
  }, [artifactId]);

  // Single message subscription for the component's lifetime. Mirrors `useDesignStream.ts`'s
  // dispose discipline: the cleanup removes the listener exactly once on unmount.
  useEffect(() => {
    function onMessage(event: MessageEvent) {
      // TRUST ANCHOR: object identity of the source window. NEVER event.origin (it is the
      // string "null" for every sandboxed/opaque frame and is forgeable across frames).
      if (!isFromFrame(event.source, iframeRef.current)) return;
      const msg = parseArtifactMessage(event.data);
      if (!msg) return;
      switch (msg.type) {
        case "artifact:ready":
          onReadyRef.current?.();
          break;
        case "artifact:resize":
          // Only apply the height update in auto-grow mode. In fixed-frame mode
          // (autoResize=false) the iframe fills the device screen via height:100%; the
          // resize message is still received and parsed (security channel intact) but the
          // height state must NOT be mutated to avoid bursting the device bezel layout.
          if (autoResizeRef.current) {
            setHeight(clampArtifactHeight(msg.height, minHeightRef.current));
          }
          break;
        case "artifact:error":
          setError(msg.message);
          onErrorRef.current?.(msg.message);
          break;
      }
    }
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, []);

  return (
    <div
      className={className}
      style={{
        position: "relative",
        width: "100%",
        // In fixed-frame mode the wrapper must stretch to fill the device screen slot
        // so the iframe's own height:100% resolves correctly against a definite parent.
        ...(autoResize ? {} : { height: "100%" }),
      }}
    >
      {error !== null && (
        <div
          role="alert"
          style={{
            // In fixed-frame mode the wrapper has a definite pixel height so a
            // normal-flow banner would push the iframe down and overflow:hidden on the
            // device bezel would silently clip the bottom of the live artifact.  Use an
            // absolute overlay instead so the iframe still fills 100% of the screen
            // slot and the banner simply floats on top.  In auto-grow mode there is no
            // height constraint, so normal flow is fine — but we unify to the overlay
            // for consistency.
            position: "absolute",
            top: 0,
            left: 0,
            right: 0,
            zIndex: 10,
            display: "flex",
            alignItems: "flex-start",
            gap: 8,
            margin: 0,
            padding: "8px 10px",
            borderRadius: autoResize ? "0 0 8px 8px" : 0,
            border: "1px solid #f0c9c2",
            borderTop: "none",
            background: "#fdecec",
            color: "#8a2a1d",
            fontSize: 12,
            lineHeight: 1.5,
          }}
        >
          <span style={{ flex: 1, wordBreak: "break-word" }}>
            Artifact runtime error: {error}
          </span>
          <button
            type="button"
            onClick={() => setError(null)}
            aria-label="Dismiss error"
            style={{
              border: "none",
              background: "transparent",
              color: "#8a2a1d",
              cursor: "pointer",
              fontWeight: 700,
              lineHeight: 1,
              padding: 0,
            }}
          >
            ×
          </button>
        </div>
      )}
      <iframe
        key={artifactId}
        ref={iframeRef}
        title={title}
        src={src}
        // ONLY allow-scripts: NO allow-same-origin (keeps the opaque origin that blocks
        // window.parent / __TAURI_INTERNALS__ reads) and NO allow-top-navigation.
        sandbox="allow-scripts"
        loading="lazy"
        style={{
          display: "block",
          width: "100%",
          // Fixed-frame mode (autoResize=false): fill the device screen area; the iframe
          // scrolls internally. Auto-grow mode: height driven by artifact:resize messages.
          height: autoResize ? height : "100%",
          // In fixed-frame mode the deviceFrames.css already enforces border:0 on the
          // screen slot; an inline border here beats that reset and draws a nested
          // rectangle inside the phone bezel screen.  Only apply it in bare auto-grow
          // mode where no outer bezel is present.
          border: autoResize ? "1px solid #e3ddd2" : "none",
          borderRadius: autoResize ? 8 : 0,
          background: "#fff",
        }}
      />
    </div>
  );
}
