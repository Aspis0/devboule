// Exercises the PURE-JS fallback path of sha256Hex by removing crypto.subtle for the
// duration of the test, then asserting it matches the known vectors. This guarantees the
// fallback (used in a jsdom window with no subtle) is correct, not just the WebCrypto path.

import { describe, it, expect, afterEach } from "vitest";
import { sha256Hex } from "./sha256";

const realCrypto = globalThis.crypto;

afterEach(() => {
  Object.defineProperty(globalThis, "crypto", {
    value: realCrypto,
    configurable: true,
  });
});

function disableSubtle() {
  Object.defineProperty(globalThis, "crypto", {
    value: { ...realCrypto, subtle: undefined },
    configurable: true,
  });
}

describe("sha256Hex — pure-JS fallback", () => {
  it("matches the empty-string vector without subtle", async () => {
    disableSubtle();
    expect(await sha256Hex("")).toBe(
      "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    );
  });

  it("matches 'abc' and the multi-block vector without subtle", async () => {
    disableSubtle();
    expect(await sha256Hex("abc")).toBe(
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
    expect(
      await sha256Hex(
        "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
      ),
    ).toBe("248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
  });
});
