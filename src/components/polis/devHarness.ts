// Dev harness for the Polis renderer.
//
// Lets the isometric map be loaded and visually verified in a PLAIN browser —
// no Tauri, no login — against a real dumped CityState. The backend test
// `dump_real_city_state` writes `polis-dev-city.json` at the repo ROOT; this
// harness imports it directly so Vite bundles it into the gated, dev-only
// polis-dev chunk and it NEVER lands in dist/ for production builds.
//
// SECURITY: the fixture is a real structural dump of THIS repo. It must not be
// served as a static asset (that is why it lives at the root, not in public/).
// It only reaches a build when POLIS_DEV=1 adds the polis-dev entry (see
// vite.config.ts).
//
// Entry point: polis-dev.tsx → mounts this into #polis-dev-root.
// See polis-dev.html at the repo root.

import type { CityState, Building } from "../../types/city";
import { createPolis, type PolisHandle } from "./createPolis";
// Bundled at build time (dev-only chunk), not fetched. Root fixture produced by
// the backend `dump_real_city_state` test.
import cityFixture from "../../../polis-dev-city.json";

function setStatus(text: string, isError = false): void {
  const el = document.getElementById("polis-dev-status");
  if (!el) return;
  el.textContent = text;
  el.style.color = isError ? "#A45A4A" : "#8A8580";
}

function setInfo(building: Building | null): void {
  const el = document.getElementById("polis-dev-info");
  if (!el) return;
  if (!building) {
    el.textContent = "Click a building to inspect it.";
    return;
  }
  el.textContent = `${building.label} · ${building.purpose} · ${building.linesOfCode} LOC · ${building.filePath}`;
}

export async function mountDevHarness(host: HTMLElement): Promise<void> {
  setStatus("Loading fixture…");
  // Imported (bundled) at build time — no network fetch, no static asset.
  const city = cityFixture as unknown as CityState;

  let handle: PolisHandle;
  try {
    handle = await createPolis(host, {
      onSelectBuilding: (b) => setInfo(b),
    });
  } catch (e) {
    setStatus(
      e instanceof Error ? e.message : "Failed to create the renderer.",
      true,
    );
    return;
  }

  handle.setCity(city);
  setInfo(null);
  setStatus(
    `${city.projectName} · ${city.buildings.length} buildings · ${city.roads.length} roads · ${city.agents.length} agents`,
  );

  // Tear down on hot-reload / navigation to avoid leaking the PIXI app.
  window.addEventListener("beforeunload", () => handle.destroy(), {
    once: true,
  });
}
