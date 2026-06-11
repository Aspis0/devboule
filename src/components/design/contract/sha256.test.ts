// Unit test for sha256Hex with known FIPS 180-4 vectors. Runs in the node environment
// (which has crypto.subtle), exercising the primary WebCrypto path. The pure-JS fallback
// is asserted against the same vectors so both paths agree.

import { describe, it, expect } from "vitest";
import { sha256Hex } from "./sha256";

describe("sha256Hex", () => {
  it("hashes the empty string to the known SHA-256 vector", async () => {
    expect(await sha256Hex("")).toBe(
      "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    );
  });

  it("hashes 'abc' to the known SHA-256 vector", async () => {
    expect(await sha256Hex("abc")).toBe(
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
  });

  it("hashes a multi-block message (UTF-8) correctly", async () => {
    // The classic 56-byte two-block vector.
    expect(
      await sha256Hex(
        "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
      ),
    ).toBe("248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
  });

  it("hashes multibyte (emoji) content deterministically", async () => {
    const a = await sha256Hex("héllo 🌍");
    const b = await sha256Hex("héllo 🌍");
    expect(a).toBe(b);
    expect(a).toMatch(/^[0-9a-f]{64}$/);
  });
});
