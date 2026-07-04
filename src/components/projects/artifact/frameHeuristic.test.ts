// TDD — written BEFORE implementation (Phase 4, plan `bubbly-hopping-valiant.md`).
// Tests the pure `inferFrameKind` heuristic and the `computeViewportScale` helper.
import { describe, expect, it } from "vitest";
import { inferFrameKind, computeViewportScale } from "./frameHeuristic";

// ---------------------------------------------------------------------------
// inferFrameKind
// ---------------------------------------------------------------------------

describe("inferFrameKind — keyword routing", () => {
  it("returns 'android' for Android-specific prompt keywords", () => {
    expect(inferFrameKind("Build an Android settings screen")).toBe("android");
    expect(inferFrameKind("Kotlin composable with Jetpack Compose")).toBe("android");
    expect(inferFrameKind("Material Design dialog for Google Play Store")).toBe("android");
    expect(inferFrameKind("material design button")).toBe("android");
  });

  it("returns 'ios' for iOS-specific prompt keywords", () => {
    expect(inferFrameKind("SwiftUI onboarding screen")).toBe("ios");
    expect(inferFrameKind("iOS navigation bar in Swift")).toBe("ios");
    expect(inferFrameKind("iPhone 15 lock screen design")).toBe("ios");
    expect(inferFrameKind("Cupertino date picker")).toBe("ios");
    expect(inferFrameKind("App Store listing page")).toBe("ios");
  });

  it("returns 'web' when html contains a full HTML document marker", () => {
    expect(inferFrameKind("a landing page", "<!DOCTYPE html><html><body>hi</body></html>")).toBe("web");
    expect(inferFrameKind("dashboard", "<html lang='en'><head></head></html>")).toBe("web");
  });

  it("returns 'component' for generic prompts with no full-document html", () => {
    expect(inferFrameKind("a button component")).toBe("component");
    expect(inferFrameKind("pricing card", "<div class='card'>Hello</div>")).toBe("component");
    expect(inferFrameKind("")).toBe("component");
  });

  it("Android keyword wins over html full-doc marker (prompt keyword takes precedence)", () => {
    // The prompt says android explicitly; html may have <!DOCTYPE> but prompt wins.
    expect(inferFrameKind("android settings", "<!DOCTYPE html><html></html>")).toBe("android");
  });

  it("iOS keyword wins over html full-doc marker", () => {
    expect(inferFrameKind("SwiftUI screen for iOS", "<!DOCTYPE html><html></html>")).toBe("ios");
  });

  it("is case-insensitive for prompt matching", () => {
    expect(inferFrameKind("ANDROID app")).toBe("android");
    expect(inferFrameKind("IOS screen")).toBe("ios");
    expect(inferFrameKind("KOTLIN composable")).toBe("android");
    expect(inferFrameKind("SWIFTUI button")).toBe("ios");
  });
});

// ---------------------------------------------------------------------------
// computeViewportScale
// ---------------------------------------------------------------------------

describe("computeViewportScale — scale factor math", () => {
  it("returns 1.0 for desktop regardless of container / device width", () => {
    expect(computeViewportScale(800, 428, "desktop")).toBe(1.0);
    expect(computeViewportScale(200, 428, "desktop")).toBe(1.0);
  });

  it("mobile: scales down when container < device, never upscales", () => {
    // Container exactly matches device → scale 1.0
    expect(computeViewportScale(428, 428, "mobile")).toBe(1.0);
    // Container smaller → scale down
    expect(computeViewportScale(214, 428, "mobile")).toBeCloseTo(0.5, 5);
    // Container larger than device → scale capped at 1 (no upscale)
    expect(computeViewportScale(856, 428, "mobile")).toBe(1.0);
  });

  it("tablet: scales proportionally up to 1.25, then caps", () => {
    // Exactly fits → scale 1.0
    expect(computeViewportScale(428, 428, "tablet")).toBe(1.0);
    // Slightly wider → can upscale, but capped at 1.25
    expect(computeViewportScale(535, 428, "tablet")).toBeCloseTo(1.25, 5);
    // Huge container → capped at 1.25
    expect(computeViewportScale(2000, 428, "tablet")).toBe(1.25);
    // Smaller container → scale down proportionally
    expect(computeViewportScale(214, 428, "tablet")).toBeCloseTo(0.5, 5);
  });

  it("returns 1.0 when containerWidth is 0 (SSR / before measurement)", () => {
    expect(computeViewportScale(0, 428, "mobile")).toBe(1.0);
    expect(computeViewportScale(0, 428, "tablet")).toBe(1.0);
    expect(computeViewportScale(0, 428, "desktop")).toBe(1.0);
  });

  it("returns 1.0 when deviceWidth is 0 (guard against divide-by-zero)", () => {
    expect(computeViewportScale(400, 0, "mobile")).toBe(1.0);
  });
});
