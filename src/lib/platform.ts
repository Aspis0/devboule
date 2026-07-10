// Platform detection + auth-copy helpers shared across the app.
//
// These replace the two previously-inline, duplicated helpers
// (ProvidersModelsTab.tsx and WorkspaceView.tsx) so macOS shows "Touch ID"
// copy and Windows shows "Windows Hello" copy from a single source of truth.

/**
 * Tri-state Apple host detection.
 *
 * Returns:
 *  - `true`  when the host is identifiable as an Apple macOS host
 *             (`mac` / `darwin` in platform or userAgent),
 *  - `false` when the host is identifiable as a NON-macOS host
 *             (`win` / `linux` / `android` / `iphone` / `ipad`),
 *  - `null`  when the platform cannot be identified (e.g. an unusual
 *             userAgent / platform string we do not recognise).
 *
 * This mirrors the ORIGINAL `inferIsAppleHostMac()` that used to live inline in
 * WorkspaceView.tsx before platform detection was de-duplicated into this module.
 *
 * The tri-state is load-bearing: an UNKNOWN platform must NOT be collapsed to
 * `false`. Doing so silently disables Apple on-device Foundation Models on hosts
 * we merely failed to recognise, and turns the "requires macOS 27+; saving is
 * still allowed for cross-machine use" fallthrough copy into dead code. Unknown
 * platforms must fall through to that branch, exactly as before the de-dup.
 */
export function detectApplePlatform(): boolean | null {
  if (typeof navigator === "undefined") return null;
  const platform = (navigator.platform ?? "").toLowerCase();
  const userAgent = (navigator.userAgent ?? "").toLowerCase();
  const haystack = `${platform} ${userAgent}`;
  if (haystack.includes("mac") || haystack.includes("darwin")) return true;
  if (
    haystack.includes("win") ||
    haystack.includes("linux") ||
    haystack.includes("android") ||
    haystack.includes("iphone") ||
    haystack.includes("ipad")
  ) {
    return false;
  }
  return null;
}

/**
 * Plain boolean Apple-host check. Use where a tri-state is not needed (e.g.
 * device-authentication copy). Equivalent to `detectApplePlatform() === true`.
 */
export function isAppleHost(): boolean {
  return detectApplePlatform() === true;
}

/**
 * Returns the device-authentication label for the current host.
 * Apple hosts use Touch ID; everything else uses Windows Hello.
 */
export function authMethodLabel(isApple: boolean): string {
  return isApple ? "Touch ID" : "Windows Hello";
}
