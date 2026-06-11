import { describe, expect, it } from "vitest";

import { normFolderKey } from "./DesignView";

// V1: a registry entry persisted with the Windows verbatim (`\\?\`) prefix (legacy, before
// the Rust storage-side strip existed) must compare EQUAL to the same folder picked in its
// plain form, so `findRecordedSha` still matches it (defense in depth).
describe("normFolderKey — V1 verbatim prefix strip", () => {
  it("treats the \\\\?\\ drive-letter prefix as equal to the plain path", () => {
    expect(normFolderKey("\\\\?\\C:\\x")).toBe(normFolderKey("C:\\x"));
  });

  it("treats the \\\\?\\UNC\\ prefix as equal to the plain UNC path", () => {
    expect(normFolderKey("\\\\?\\UNC\\server\\share\\proj")).toBe(
      normFolderKey("\\\\server\\share\\proj"),
    );
  });

  it("leaves a plain path's normalization unchanged (trim/sep/case still apply)", () => {
    expect(normFolderKey("  C:\\Users\\Me\\Proj\\  ")).toBe("c:/users/me/proj");
    // POSIX path is untouched by the strip.
    expect(normFolderKey("/Home/Me/Proj/")).toBe("/home/me/proj");
  });
});
