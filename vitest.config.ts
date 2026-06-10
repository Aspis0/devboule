import { defineConfig } from "vitest/config";

// Unit-test runner config. Scoped to `*.test.ts(x)` under `src/` ONLY.
//
// NOTE: the Polis `*.spec.ts` files (e.g. src/components/polis/roadGraph.spec.ts)
// are NOT vitest suites — they are the repo's older zero-dependency,
// self-asserting modules that export a `run*Spec()` throwing on failure. They
// must NOT be collected here (they contain no `test()`/`describe()` and would
// fail as "no test suite found"). We therefore include only `*.test.ts(x)`.
export default defineConfig({
  test: {
    include: ["src/**/*.test.{ts,tsx}"],
    environment: "node",
  },
});
