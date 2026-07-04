// Ambient declaration for the Polis dev harness's only non-resolvable import.
// `polis-dev-city.json` is a gitignored, runtime-generated dev-only fixture
// (produced by the backend `dump_real_city_state` test, see devHarness.ts) and
// is intentionally absent on a fresh checkout / in CI, which would otherwise make
// `tsc --noEmit` fail with TS2307. Typed `unknown`; devHarness casts it to CityState.
declare module "*/polis-dev-city.json" {
  const value: unknown;
  export default value;
}
