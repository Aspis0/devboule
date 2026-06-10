import { describe, expect, it } from "vitest";

import type { CensorLocalAi } from "./config";

// NIT 6: the CensorLocalAi TS type exposes `ollamaModel` (camelCase, matching the Rust
// `ollama_model`). This is a type-level regression: reading `.ollamaModel` must compile to
// `string | undefined` WITHOUT an `unknown` coercion. The runtime assertion is incidental;
// the value of this test is that `tsc --noEmit` fails if the field is removed from the type.

describe("CensorLocalAi.ollamaModel type", () => {
  it("reads ollamaModel as a typed optional string (no unknown coercion)", () => {
    const cfg: CensorLocalAi = { provider: "ollama", ollamaModel: "gemma4:x" };
    // Typed read: the binding is `string | undefined`, not `unknown`. If the field were
    // missing from the type this line would fail to compile.
    const tag: string | undefined = cfg.ollamaModel;
    expect(tag).toBe("gemma4:x");
  });
});
