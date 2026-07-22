/**
 * F40: vitest full suite hung at 0% CPU after every test file reported pass
 * (open timers/listeners from DesignView / consent pollers). Force-exit so CI
 * and local `npm test` always terminate with a known status.
 */
export default async function globalTeardown() {
  // Give pending microtasks a tick, then hard-exit the process group.
  await new Promise((r) => setTimeout(r, 50));
  // 0 = success path when vitest reached teardown after a green run.
  // Vitest only invokes globalTeardown after the run finishes collecting
  // results; non-zero test failures already set process.exitCode.
  if (typeof process.exitCode === "number" && process.exitCode !== 0) {
    process.exit(process.exitCode);
  }
  process.exit(0);
}
