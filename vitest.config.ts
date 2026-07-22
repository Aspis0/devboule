import { defineConfig } from "vitest/config";

// Unit-test runner config. Scoped to `*.test.ts(x)` under `src/` ONLY.
//
// NOTE: the Polis `*.spec.ts` files (e.g. src/components/polis/roadGraph.spec.ts)
// are NOT vitest suites — they are the repo's older zero-dependency,
// self-asserting modules that export a `run*Spec()` throwing on failure. They
// must NOT be collected here (they contain no `test()`/`describe()` and would
// fail as "no test suite found"). We therefore include only `*.test.ts(x)`.
//
// F40 hang-free default suite:
// - DesignView streaming tests leave open act() updates (0% CPU hang).
// - RolesTableCard.test.tsx deadlocks alone under jsdom/act.
// - globalTeardown force-exits if a handle still holds the event loop.
export default defineConfig({
  test: {
    include: ["src/**/*.test.{ts,tsx}"],
    environment: "node",
    exclude: [
      ...(process.env.VITEST_INCLUDE_DESIGN ? [] : ["src/components/design/**"]),
      ...(process.env.VITEST_INCLUDE_FLAKY
        ? []
        : ["src/components/settings/RolesTableCard.test.tsx"]),
    ],
    // Default multi-fork pool (singleFork caused flaky timer polls in Plans/Help).
    // Hang-prone suites stay excluded; re-enable with VITEST_INCLUDE_* and expect
    // longer runs + possible need for singleFork locally when debugging design.
    pool: "forks",
    testTimeout: 20_000,
    hookTimeout: 20_000,
    teardownTimeout: 10_000,
    globalTeardown: "./vitest.global-teardown.ts",
  },
});
