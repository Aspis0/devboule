// Thin, DOM-free wrapper over the `detect_hardware` Rust command. Phase B1 only makes the
// host's hardware snapshot CALLABLE from the renderer; Phase B2 will wire it into Polis to
// scale rendering detail (walker count, decoration density) to the machine.
//
// The result is best-effort and advisory: on a box where the GPU cannot be probed, `gpuName`
// / `gpuKind` come back "unknown" and `vramGb` null — callers must degrade gracefully and
// never assume a discrete card. The command reads only non-secret machine metadata and sends
// nothing off-box.
//
// NOTE (future, not implemented): a WebGL `gl.getParameter(UNMASKED_RENDERER_WEBGL)`
// cross-check could supplement this from the renderer side (it sees the GPU Chromium actually
// selected). Out of scope for B1.

import { invokeBackendCommand } from "../context/AppContext";
import type { HardwareInfo } from "../types/backend";

export type { HardwareInfo } from "../types/backend";

/**
 * Detect the host machine's hardware (CPU cores, RAM, primary GPU). Best-effort: the Rust
 * side never panics and fills unprobeable fields with "unknown" / null rather than failing.
 * Rejects only if the IPC bridge itself is unavailable (e.g. opened outside the Tauri app).
 */
export async function detectHardware(): Promise<HardwareInfo> {
  return invokeBackendCommand<HardwareInfo>("detect_hardware");
}
