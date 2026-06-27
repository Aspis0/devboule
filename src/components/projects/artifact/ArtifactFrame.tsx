// ArtifactFrame — pure presentational wrapper that renders a device-frame bezel around the
// interactive-artifact render host (Phase 4, plan `bubbly-hopping-valiant.md`). Chrome is
// entirely visual; the artifact HTML is IDENTICAL across all four skins.
//
// CSS sources (both MIT — see `deviceFrames.css` header and `THIRD_PARTY_NOTICES.md`):
//   - Phone skins: picturepan2/devices.css (© 2017 Yan Zhu)
//   - Browser chrome: jhildenbiddle/css-device-frames (© 2021 John Hildenbiddle)
//
// SCALING STRATEGY (à la Gutenberg PR #33342): the device frame keeps its REAL CSS pixel
// width (so the artifact's own @media queries fire correctly) and is scaled with
// `transform: scale(factor)` computed by `computeViewportScale`. The outer container
// reserves the scaled height via an inline pixel value so no layout whitespace is left.

import { type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { ArtifactFrameKind } from "../../../types/design";
import { computeViewportScale } from "./frameHeuristic";
import "./deviceFrames.css";

// Natural device dimensions (CSS pixels) — must match the verbatim values in deviceFrames.css.
const IPHONE_W = 428;
const IPHONE_H = 868;
const PIXEL_W = 404;
const PIXEL_H = 862;

export interface ArtifactFrameProps {
  /** Which device / browser chrome skin to render. */
  kind: ArtifactFrameKind;
  /**
   * Viewport mode controls the `transform:scale()` factor applied to phone skins.
   * - `"mobile"`  (default) — scale down to fit container width, never upscale.
   * - `"tablet"`  — allow gentle upscale up to 1.25×.
   * - `"desktop"` — always 1× (natural device size, no scale applied).
   * Has no visual effect on `"web"` and `"component"` skins (those fill their container).
   */
  viewport?: "mobile" | "tablet" | "desktop";
  /** The artifact render host — typically an `<ArtifactView>` element. */
  children: ReactNode;
}

/**
 * Presentational device-bezel wrapper. Renders one of four skins around the artifact
 * iframe host. The `autoResize` prop on `<ArtifactView>` must be set to `false` by the
 * caller for fixed-dimension skins (`android`/`ios`/`web`) so the iframe fills and
 * scrolls internally rather than bursting the bezel.
 */
export function ArtifactFrame({
  kind,
  viewport = "mobile",
  children,
}: ArtifactFrameProps): ReactNode {
  // Container measurement for the phone viewport scaler.
  const containerRef = useRef<HTMLDivElement>(null);
  const [containerWidth, setContainerWidth] = useState(0);

  // ResizeObserver: fires once after mount (initial measurement) and on every size change.
  // The cleanup disconnects the observer on unmount — mirrors the dispose discipline used
  // across the codebase (no stale observer reference after teardown).
  //
  // useLayoutEffect (Fix 3): running in the layout phase means the first scale factor is
  // computed and applied BEFORE the browser paints, eliminating the first-paint jump where
  // the phone skin momentarily renders at scale 1.0 (full native device height).
  // `typeof window` guard keeps the import clean for any SSR path (Tauri desktop-only, but
  // good hygiene).
  //
  // `kind` in deps (Fix 4): when the user switches the frame dropdown the containerRef is
  // only attached in the phone-skin branch. Re-running on kind change disconnects the old
  // observer and re-observes the current node (the `if (!el) return` guard covers non-phone
  // kinds where the ref resolves to null after a kind switch).
  const useIsomorphicLayoutEffect =
    typeof window !== "undefined" ? useLayoutEffect : useEffect;
  useIsomorphicLayoutEffect(() => {
    const el = containerRef.current;
    if (!el) return; // component / web skins don't attach containerRef
    setContainerWidth(el.clientWidth);
    const obs = new ResizeObserver((entries) => {
      setContainerWidth(entries[0].contentRect.width);
    });
    obs.observe(el);
    return () => obs.disconnect();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [kind]);

  // -----------------------------------------------------------------------
  // component skin — bare, no chrome
  // -----------------------------------------------------------------------
  if (kind === "component") {
    return <div className="af-component-bare">{children}</div>;
  }

  // -----------------------------------------------------------------------
  // web skin — browser chrome (toolbar + address bar)
  // -----------------------------------------------------------------------
  if (kind === "web") {
    return (
      <div className="app-frame" style={{ width: "100%", height: "100%" }}>
        <div className="af-frame-content">{children}</div>
      </div>
    );
  }

  // -----------------------------------------------------------------------
  // Phone skins (android → Pixel 6 Pro; ios → iPhone 14 Pro)
  // -----------------------------------------------------------------------
  const isIos = kind === "ios";
  const deviceW = isIos ? IPHONE_W : PIXEL_W;
  const deviceH = isIos ? IPHONE_H : PIXEL_H;
  const deviceClass = isIos ? "device-iphone-14-pro" : "device-google-pixel-6-pro";

  const scale = computeViewportScale(containerWidth, deviceW, viewport);
  // Reserve exactly the scaled height so the document flow doesn't leave whitespace.
  const scaledH = Math.round(deviceH * scale);

  return (
    // Outer: measures the available width; clips any pixel rounding overflow.
    <div
      ref={containerRef}
      className="af-scaler-outer"
      style={{ width: "100%", height: `${scaledH}px` }}
    >
      {/* Inner: sits at natural device size, then scaled from the top-left corner. */}
      <div
        className="af-scaler-inner"
        style={{
          transform: `scale(${scale})`,
          width: deviceW,
          height: deviceH,
        }}
      >
        {/* Device bezel — see deviceFrames.css for the MIT-sourced CSS rules. */}
        <div className={`device ${deviceClass}`}>
          <div className="device-frame">
            {/* device-screen: fixed px dimensions + overflow:hidden clips the iframe
                to the rounded screen area; never set overflow:hidden on the iframe
                itself (that would break internal artifact scroll). */}
            <div className="device-screen">{children}</div>
          </div>
        </div>
      </div>
    </div>
  );
}
