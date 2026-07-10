import { describe, it, expect, afterEach } from "vitest";
import { authMethodLabel, detectApplePlatform, isAppleHost } from "./platform";

describe("platform helpers", () => {
  afterEach(() => {
    // @ts-expect-error restore real navigator between tests
    delete global.navigator;
  });

  it("authMethodLabel returns Touch ID on Apple hosts", () => {
    expect(authMethodLabel(true)).toBe("Touch ID");
  });

  it("authMethodLabel returns Windows Hello on non-Apple hosts", () => {
    expect(authMethodLabel(false)).toBe("Windows Hello");
  });

  it("isAppleHost respects a mocked navigator.platform (Mac)", () => {
    // @ts-expect-error simulate a macOS host
    global.navigator = { platform: "MacIntel", userAgent: "Macintosh" };
    expect(isAppleHost()).toBe(true);
  });

  it("isAppleHost respects a mocked navigator.platform (Windows)", () => {
    // @ts-expect-error simulate a Windows host
    global.navigator = { platform: "Win32", userAgent: "Windows" };
    expect(isAppleHost()).toBe(false);
  });

  it("detectApplePlatform returns null on an unidentifiable platform", () => {
    // @ts-expect-error simulate an unusual host (no mac/win/linux/iOS marker)
    global.navigator = { platform: "FreeBSD", userAgent: "Gecko/20100101" };
    expect(detectApplePlatform()).toBe(null);
    expect(isAppleHost()).toBe(false);
  });
});
