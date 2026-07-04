import { describe, expect, it } from "vitest";
import {
  ARTIFACT_ERROR_MAX_CHARS,
  ARTIFACT_MAX_HEIGHT,
  buildArtifactSrc,
  clampArtifactHeight,
  isFromFrame,
  parseArtifactMessage,
} from "./artifactProtocol";

describe("parseArtifactMessage — schema validation (allowlist)", () => {
  it("accepts the three allowlisted types", () => {
    expect(parseArtifactMessage({ type: "artifact:ready" })).toEqual({
      type: "artifact:ready",
    });
    expect(parseArtifactMessage({ type: "artifact:resize", height: 321 })).toEqual({
      type: "artifact:resize",
      height: 321,
    });
    expect(parseArtifactMessage({ type: "artifact:error", message: "boom" })).toEqual({
      type: "artifact:error",
      message: "boom",
    });
  });

  it("rejects non-objects, null, and unknown types", () => {
    expect(parseArtifactMessage(null)).toBeNull();
    expect(parseArtifactMessage(undefined)).toBeNull();
    expect(parseArtifactMessage("artifact:ready")).toBeNull();
    expect(parseArtifactMessage(42)).toBeNull();
    expect(parseArtifactMessage({ type: "artifact:navigate" })).toBeNull();
    expect(parseArtifactMessage({ type: "artifact:link", href: "x" })).toBeNull();
    expect(parseArtifactMessage({})).toBeNull();
  });

  it("rejects a resize without a finite numeric height", () => {
    expect(parseArtifactMessage({ type: "artifact:resize" })).toBeNull();
    expect(parseArtifactMessage({ type: "artifact:resize", height: "300" })).toBeNull();
    expect(parseArtifactMessage({ type: "artifact:resize", height: NaN })).toBeNull();
    expect(parseArtifactMessage({ type: "artifact:resize", height: Infinity })).toBeNull();
  });

  it("clamps a negative height to 0 at parse time", () => {
    // Negative heights are a contract violation; clamp at the parse boundary so no
    // downstream consumer (ArtifactView, CSS) ever sees a negative value.
    expect(parseArtifactMessage({ type: "artifact:resize", height: -9999 })).toEqual({
      type: "artifact:resize",
      height: 0,
    });
    expect(parseArtifactMessage({ type: "artifact:resize", height: -1 })).toEqual({
      type: "artifact:resize",
      height: 0,
    });
    // Zero is already valid — must pass through unchanged.
    expect(parseArtifactMessage({ type: "artifact:resize", height: 0 })).toEqual({
      type: "artifact:resize",
      height: 0,
    });
    // Positive heights are unaffected.
    expect(parseArtifactMessage({ type: "artifact:resize", height: 500 })).toEqual({
      type: "artifact:resize",
      height: 500,
    });
    // Non-finite values still return null (not zero) — distinct code path.
    expect(parseArtifactMessage({ type: "artifact:resize", height: NaN })).toBeNull();
    expect(parseArtifactMessage({ type: "artifact:resize", height: Infinity })).toBeNull();
    expect(parseArtifactMessage({ type: "artifact:resize", height: -Infinity })).toBeNull();
  });

  it("coerces + truncates an error message to the cap", () => {
    const long = "x".repeat(ARTIFACT_ERROR_MAX_CHARS + 500);
    const parsed = parseArtifactMessage({ type: "artifact:error", message: long });
    expect(parsed).not.toBeNull();
    if (parsed && parsed.type === "artifact:error") {
      expect(parsed.message.length).toBe(ARTIFACT_ERROR_MAX_CHARS);
    }
    // A missing message becomes an empty string, not a crash.
    expect(parseArtifactMessage({ type: "artifact:error" })).toEqual({
      type: "artifact:error",
      message: "",
    });
  });
});

describe("clampArtifactHeight — resize clamp", () => {
  it("clamps to [min, MAX]", () => {
    expect(clampArtifactHeight(500, 120)).toBe(500);
    expect(clampArtifactHeight(50, 120)).toBe(120); // below min → min
    expect(clampArtifactHeight(ARTIFACT_MAX_HEIGHT + 9999, 120)).toBe(ARTIFACT_MAX_HEIGHT);
  });

  it("collapses non-finite / negative to min", () => {
    expect(clampArtifactHeight(NaN, 120)).toBe(120);
    expect(clampArtifactHeight(Infinity, 120)).toBe(120); // non-finite collapses to min
    expect(clampArtifactHeight(-1000, 120)).toBe(120);
    expect(clampArtifactHeight(-1000, 0)).toBe(0);
  });
});

describe("isFromFrame — source-identity trust", () => {
  it("trusts ONLY the iframe's own contentWindow", () => {
    const win = {} as Window;
    const frame = { contentWindow: win } as HTMLIFrameElement;
    expect(isFromFrame(win, frame)).toBe(true);
  });

  it("rejects a mismatched source, a null frame, and a null contentWindow", () => {
    const win = {} as Window;
    const other = {} as Window;
    const frame = { contentWindow: win } as HTMLIFrameElement;
    expect(isFromFrame(other, frame)).toBe(false);
    expect(isFromFrame(win, null)).toBe(false);
    expect(isFromFrame(null, frame)).toBe(false);
    expect(isFromFrame(win, { contentWindow: null } as HTMLIFrameElement)).toBe(false);
  });
});

describe("buildArtifactSrc — per-platform origin", () => {
  it("uses the artifact: scheme on macOS/Linux", () => {
    expect(buildArtifactSrc("p1-2", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")).toBe(
      "artifact://localhost/p1-2",
    );
  });

  it("uses http://artifact.localhost on Windows (WebView2)", () => {
    expect(buildArtifactSrc("p1-2", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe(
      "http://artifact.localhost/p1-2",
    );
  });

  it("URL-encodes the id", () => {
    expect(buildArtifactSrc("__sample__", "Macintosh")).toBe(
      "artifact://localhost/__sample__",
    );
    expect(buildArtifactSrc("a b", "Macintosh")).toBe("artifact://localhost/a%20b");
  });
});
