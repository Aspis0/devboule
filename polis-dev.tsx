// Standalone dev entry for the Polis renderer.
//
// Loaded by polis-dev.html. Mounts the dev harness against the fixture at
// /polis-dev-city.json. No React app, no Tauri, no login — just the renderer,
// so the isometric map can be visually verified in a plain browser.

import { mountDevHarness } from "./src/components/polis/devHarness";

const host = document.getElementById("polis-dev-host");
if (host) {
  void mountDevHarness(host as HTMLElement);
}
