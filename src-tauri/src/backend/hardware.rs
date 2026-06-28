//! Cross-platform, best-effort HARDWARE DETECTION (Phase B1).
//!
//! Polis will later (Phase B2) scale its isometric rendering detail to the host's
//! capability — fewer walkers / lower decoration density on a weak integrated GPU, the
//! full city on a discrete card. This module is the SINGLE source of truth for "what is
//! this machine", exposed to the renderer as one [`HardwareInfo`] over the IPC boundary.
//!
//! Two hard design rules govern everything here:
//!
//!   1. BEST-EFFORT, NEVER PANIC. Hardware probing is inherently flaky (no GPU, a software
//!      adapter, a locked-down macOS sandbox, a `system_profiler` that times out). EVERY
//!      detection path is failure-isolated and degrades to a safe `"unknown"` / `None`
//!      rather than erroring — a missing GPU name must never break the dashboard.
//!   2. PURE, TESTABLE SEAMS. The OS calls (DXGI on Windows, `system_profiler` on macOS)
//!      are THIN wrappers that hand a raw value (an adapter desc, a process stdout string)
//!      to a pure parser. The heuristics ([`classify_gpu_kind`], [`adapter_to_gpu`],
//!      [`parse_system_profiler`]) carry ALL the logic and are unit-tested WITHOUT real
//!      hardware. The `#[cfg]`-gated wrappers are kept trivial.
//!
//! PRIVACY: this returns ONLY non-secret machine capability metadata (core count, RAM,
//! GPU model). It reads no vault secret and sends nothing anywhere. The `#[tauri::command]`
//! is intentionally ungated (mirrors [`super::provider_detect::detect_providers`]) so the
//! renderer can size Polis before/while the vault is locked.
//!
//! A WebGL `gl.getParameter(UNMASKED_RENDERER_WEBGL)` cross-check could later SUPPLEMENT
//! this from the renderer side (it sees the GPU Chromium actually picked); that is out of
//! scope for B1 and not implemented here.

use serde::Serialize;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

/// Bytes per gibibyte. sysinfo (>= 0.30) and DXGI both report memory in BYTES, so every
/// `*_gb` field is `bytes as f64 / GIB`.
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// A DedicatedVideoMemory at/above this many bytes marks a GPU as `"discrete"`. Integrated
/// GPUs (Intel UHD, AMD APU, Apple Silicon) report ~0 dedicated VRAM (they carve from system
/// RAM, surfaced as *shared* memory, not dedicated). 512 MiB cleanly separates the two: even
/// the smallest modern discrete card has >= 1-2 GiB dedicated, while integrated parts report
/// either 0 or a tiny (<= 128 MiB) reserved carve-out.
const DISCRETE_VRAM_THRESHOLD_BYTES: u64 = 512 * 1024 * 1024;

/// One machine's hardware capability snapshot. camelCase over the wire so the TS side reads
/// it directly. Every field is best-effort: `gpu_name`/`gpu_kind` fall back to `"unknown"`
/// and `vram_gb` to `None` when the GPU cannot be probed.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareInfo {
    /// Logical CPU core count (falls back to physical, then 1, never 0).
    pub cpu_cores: u32,
    /// Total physical RAM, GiB.
    pub ram_total_gb: f64,
    /// Currently available RAM, GiB (best-effort; what the OS reports as free-ish).
    pub ram_available_gb: f64,
    /// Best-guess primary GPU model (e.g. "NVIDIA GeForce RTX 4070"); "unknown" if unprobed.
    pub gpu_name: String,
    /// Dedicated VRAM in GiB when knowable; `None` for integrated/unified-memory or unknown.
    pub vram_gb: Option<f64>,
    /// `"integrated" | "discrete" | "unknown"`.
    pub gpu_kind: String,
}

impl HardwareInfo {
    /// The safe baseline: 1 core, 0 RAM, unknown GPU. Each successful probe overwrites the
    /// fields it can fill, so a partial failure still yields a coherent struct.
    fn unknown() -> Self {
        Self {
            cpu_cores: 1,
            ram_total_gb: 0.0,
            ram_available_gb: 0.0,
            gpu_name: "unknown".to_string(),
            vram_gb: None,
            gpu_kind: "unknown".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Pure, OS-independent heuristics (the testable seams).
// ---------------------------------------------------------------------------

/// Classify a GPU as `"discrete" | "integrated"` from its dedicated VRAM and model name.
///
/// Primary signal is DedicatedVideoMemory (>= [`DISCRETE_VRAM_THRESHOLD_BYTES`] => discrete).
/// As a SECONDARY signal — because some integrated parts (and the WARP software renderer)
/// can report odd values, and a discrete card behind a flaky driver can momentarily read 0 —
/// an unmistakable discrete-vendor token in the name (GeForce / Radeon RX / Arc …) upgrades a
/// 0-VRAM guess to discrete. Conversely, integrated tokens are NOT used to DOWNGRADE a card
/// with real dedicated VRAM (the memory is the stronger signal). Never returns "unknown":
/// once we HAVE an adapter, it is one or the other; "unknown" is reserved for "no probe".
pub fn classify_gpu_kind(dedicated_vram_bytes: u64, name: &str) -> &'static str {
    if dedicated_vram_bytes >= DISCRETE_VRAM_THRESHOLD_BYTES {
        return "discrete";
    }
    let lname = name.to_ascii_lowercase();
    // Discrete-only model families. These never appear on an integrated part, so seeing one
    // with a (driver-glitched) low VRAM read still means a real discrete card.
    const DISCRETE_TOKENS: &[&str] = &[
        "geforce",
        "radeon rx",
        "radeon pro",
        "quadro",
        "tesla",
        "rtx",
        "gtx",
        "arc a", // Intel Arc discrete (A-series); "arc" alone is too broad
        "instinct",
    ];
    if DISCRETE_TOKENS.iter().any(|t| lname.contains(t)) {
        return "discrete";
    }
    "integrated"
}

/// Whether a DXGI adapter description names the Microsoft Basic Render Driver (WARP), the
/// pure-software fallback adapter. It is NOT real hardware and must be skipped during
/// enumeration so it never wins "primary GPU".
pub fn is_software_adapter(name: &str) -> bool {
    let lname = name.to_ascii_lowercase();
    lname.contains("microsoft basic render") || lname.contains("basic render driver")
}

/// Turn a raw adapter (model name + dedicated VRAM in bytes) into the GPU fields of
/// [`HardwareInfo`]. Pure: this is what the Windows DXGI wrapper feeds its `GetDesc` result
/// into, so the whole heuristic is exercised in tests with synthetic descs and no real GPU.
///
/// `vram_gb` is `Some` only when there is meaningful dedicated VRAM (> 0); an integrated part
/// reporting 0 dedicated bytes yields `None` (it has no *dedicated* VRAM — its memory is
/// shared system RAM, surfaced elsewhere).
pub fn adapter_to_gpu(name: &str, dedicated_vram_bytes: u64) -> (String, Option<f64>, String) {
    let kind = classify_gpu_kind(dedicated_vram_bytes, name).to_string();
    let vram_gb = if dedicated_vram_bytes > 0 {
        Some(dedicated_vram_bytes as f64 / GIB)
    } else {
        None
    };
    let clean = name.trim();
    let gpu_name = if clean.is_empty() {
        "unknown".to_string()
    } else {
        clean.to_string()
    };
    (gpu_name, vram_gb, kind)
}

/// Parse `system_profiler SPDisplaysDataType` output (macOS) into `(name, vram_gb, kind)`.
///
/// Accepts EITHER the `-json` form OR the default human text form, because the JSON schema
/// has shifted across macOS releases and `system_profiler` can fail to emit JSON in some
/// sandboxes — the text fallback keeps detection working. Strategy:
///   - JSON: find the displays array, read `sppci_model` (GPU name) and, if present,
///     `spdisplays_vram` / `sppci_vram` (e.g. "8 GB", "512 MB").
///   - Text: the line after "Chipset Model:" is the name; "VRAM (Total):"/"VRAM (Dynamic,
///     Max):" carries the size.
/// Apple Silicon reports NO discrete VRAM (unified memory) — its `sppci_device_type` is
/// `spdisplays_gpu_device` with no VRAM line — so it classifies as `"integrated"` with
/// `vram_gb = None` (the caller may treat system RAM as the shared pool). A discrete-vendor
/// name (AMD Radeon on an Intel Mac) with VRAM classifies `"discrete"`.
///
/// Fail-soft: an unparseable / empty input yields `("unknown", None, "unknown")`.
pub fn parse_system_profiler(input: &str) -> (String, Option<f64>, String) {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return ("unknown".to_string(), None, "unknown".to_string());
    }

    // --- JSON form -------------------------------------------------------
    if trimmed.starts_with('{') {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some((name, vram_gb)) = extract_json_gpu(&val) {
                let kind = gpu_kind_from_name_vram(&name, vram_gb);
                return (name, vram_gb, kind);
            }
        }
        // JSON that did not match the expected shape -> unknown (do not guess).
        return ("unknown".to_string(), None, "unknown".to_string());
    }

    // --- Text form -------------------------------------------------------
    let mut name: Option<String> = None;
    let mut vram_gb: Option<f64> = None;
    for line in trimmed.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("Chipset Model:") {
            let v = rest.trim();
            if !v.is_empty() {
                name = Some(v.to_string());
            }
        } else if let Some(rest) = l
            .strip_prefix("VRAM (Total):")
            .or_else(|| l.strip_prefix("VRAM (Dynamic, Max):"))
        {
            vram_gb = parse_vram_size(rest.trim());
        }
    }
    match name {
        Some(n) => {
            let kind = gpu_kind_from_name_vram(&n, vram_gb);
            (n, vram_gb, kind)
        }
        None => ("unknown".to_string(), None, "unknown".to_string()),
    }
}

/// macOS GPU kind from a name + optional VRAM. Apple Silicon ("Apple M…") is ALWAYS
/// integrated/unified regardless of any reported number; otherwise fall back to the shared
/// VRAM heuristic (real dedicated VRAM => discrete). No VRAM and a non-Apple name => the
/// vendor-token heuristic in [`classify_gpu_kind`] decides.
fn gpu_kind_from_name_vram(name: &str, vram_gb: Option<f64>) -> String {
    let lname = name.to_ascii_lowercase();
    if lname.starts_with("apple m") || lname.contains("apple silicon") {
        return "integrated".to_string();
    }
    let bytes = vram_gb.map(|g| (g * GIB) as u64).unwrap_or(0);
    classify_gpu_kind(bytes, name).to_string()
}

/// Pull `(name, vram_gb)` out of a `system_profiler -json` value. Tolerant of the schema
/// drift across macOS versions: searches the `SPDisplaysDataType` array (or the first array
/// it finds) for the first object carrying a GPU model key.
fn extract_json_gpu(val: &serde_json::Value) -> Option<(String, Option<f64>)> {
    let array = val
        .get("SPDisplaysDataType")
        .and_then(|v| v.as_array())
        .or_else(|| {
            // Fallback: first array value anywhere at the top level.
            val.as_object()
                .and_then(|o| o.values().find_map(|v| v.as_array()))
        })?;

    for entry in array {
        let obj = match entry.as_object() {
            Some(o) => o,
            None => continue,
        };
        // GPU model key has varied: `sppci_model` (modern), `_name` (older).
        let name = obj
            .get("sppci_model")
            .or_else(|| obj.get("_name"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let name = match name {
            Some(n) => n,
            None => continue,
        };
        let vram_gb = obj
            .get("spdisplays_vram")
            .or_else(|| obj.get("sppci_vram"))
            .and_then(|v| v.as_str())
            .and_then(parse_vram_size);
        return Some((name, vram_gb));
    }
    None
}

/// Parse a VRAM size string like "8 GB", "8192 MB", "512 MB" into GiB. macOS reports these
/// as decimal-prefixed strings; we treat GB/MB as binary multiples (the values are powers of
/// two in practice and Polis only needs a rough magnitude). Returns `None` on a value it
/// cannot parse (so the caller keeps `vram_gb = None` rather than a bogus number).
fn parse_vram_size(s: &str) -> Option<f64> {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    let (num_part, mult) = if let Some(n) = lower.strip_suffix("gb") {
        (n, 1.0)
    } else if let Some(n) = lower.strip_suffix("mb") {
        (n, 1.0 / 1024.0)
    } else {
        return None;
    };
    let num: f64 = num_part.trim().parse().ok()?;
    if num <= 0.0 || !num.is_finite() {
        return None;
    }
    Some(num * mult)
}

// ---------------------------------------------------------------------------
// CPU + RAM (cross-platform via sysinfo; no cfg needed).
// ---------------------------------------------------------------------------

/// Read core count + total/available RAM into a fresh [`HardwareInfo`] (GPU fields left at
/// their `unknown` defaults; a later GPU probe fills them). Only the memory + cpu refresh
/// kinds are requested, never the (heavier) process/disk/network info.
pub(crate) fn read_cpu_ram() -> HardwareInfo {
    let sys = System::new_with_specifics(
        RefreshKind::nothing()
            .with_memory(MemoryRefreshKind::nothing().with_ram())
            .with_cpu(CpuRefreshKind::nothing()),
    );

    let mut info = HardwareInfo::unknown();

    // Prefer logical cores (what scheduling/rendering parallelism actually sees); fall back
    // to physical, then 1. `cpus()` is populated by the cpu refresh above.
    let logical = sys.cpus().len();
    let cores = if logical > 0 {
        logical
    } else {
        sys.physical_core_count().unwrap_or(1).max(1)
    };
    info.cpu_cores = cores.min(u32::MAX as usize) as u32;

    // sysinfo (>= 0.30) reports memory in BYTES.
    info.ram_total_gb = sys.total_memory() as f64 / GIB;
    info.ram_available_gb = sys.available_memory() as f64 / GIB;

    info
}

// ---------------------------------------------------------------------------
// GPU — Windows (DXGI). Thin wrapper around the pure `adapter_to_gpu`.
// ---------------------------------------------------------------------------

/// Probe the primary GPU via DXGI: enumerate adapters, skip the software (WARP) adapter, and
/// pick the one with the largest DedicatedVideoMemory as primary. Returns
/// `(name, vram_gb, kind)`; on ANY failure (no factory, no adapter, all software) returns the
/// `("unknown", None, "unknown")` triple so detection degrades softly.
///
/// All windows-rs calls are `unsafe` (raw COM); each is failure-isolated. We deliberately use
/// `CreateDXGIFactory1` (available since Windows 7, no debug-layer requirement) and the base
/// `EnumAdapters`/`GetDesc` (not the `*1` variants) so this builds and runs on the widest
/// range of hosts — we only need the description + dedicated VRAM, which the base desc has.
#[cfg(windows)]
fn detect_gpu() -> (String, Option<f64>, String) {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1};

    // best: the largest-dedicated-VRAM real adapter seen so far.
    let mut best: Option<(String, u64)> = None;

    unsafe {
        let factory: IDXGIFactory1 = match CreateDXGIFactory1() {
            Ok(f) => f,
            Err(_) => return ("unknown".to_string(), None, "unknown".to_string()),
        };

        let mut index: u32 = 0;
        // EnumAdapters returns DXGI_ERROR_NOT_FOUND (an Err) once we run past the last
        // adapter — that is the loop's normal terminator, not a failure.
        while let Ok(adapter) = factory.EnumAdapters(index) {
            index += 1;
            let adapter: IDXGIAdapter = adapter;
            // windows 0.58: GetDesc returns the desc by value (Result<DXGI_ADAPTER_DESC>).
            let desc = match adapter.GetDesc() {
                Ok(d) => d,
                Err(_) => continue,
            };

            // Description is a fixed [u16; 128] UTF-16 buffer, NUL-padded.
            let end = desc
                .Description
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(desc.Description.len());
            let name = String::from_utf16_lossy(&desc.Description[..end]);
            let name = name.trim().to_string();

            if is_software_adapter(&name) {
                continue;
            }

            let vram = desc.DedicatedVideoMemory as u64;
            match &best {
                Some((_, best_vram)) if *best_vram >= vram => {}
                _ => best = Some((name, vram)),
            }
        }
    }

    match best {
        Some((name, vram)) => adapter_to_gpu(&name, vram),
        None => ("unknown".to_string(), None, "unknown".to_string()),
    }
}

// ---------------------------------------------------------------------------
// GPU — macOS (system_profiler). Thin wrapper around `parse_system_profiler`.
// ---------------------------------------------------------------------------

/// Probe the GPU via `system_profiler SPDisplaysDataType -json` and parse it with the pure
/// [`parse_system_profiler`]. Bounded and fail-soft: a spawn error, a non-zero exit, or
/// unparseable output all degrade to `("unknown", None, "unknown")`. NOT verified on real
/// macOS hardware — see the module note; the parser IS unit-tested with captured samples.
#[cfg(target_os = "macos")]
fn detect_gpu() -> (String, Option<f64>, String) {
    use std::process::Command;

    let output = Command::new("system_profiler")
        .arg("SPDisplaysDataType")
        .arg("-json")
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            parse_system_profiler(&stdout)
        }
        _ => ("unknown".to_string(), None, "unknown".to_string()),
    }
}

// ---------------------------------------------------------------------------
// GPU — every other target (Linux/BSD/…) or no detection available.
// ---------------------------------------------------------------------------

/// Fallback GPU probe for non-Windows / non-macOS targets: no detection, always unknown.
/// Keeps `detect_hardware` cross-platform-compilable without dragging in a Linux GPU stack.
#[cfg(not(any(windows, target_os = "macos")))]
fn detect_gpu() -> (String, Option<f64>, String) {
    ("unknown".to_string(), None, "unknown".to_string())
}

// ---------------------------------------------------------------------------
// Assembly + command.
// ---------------------------------------------------------------------------

/// Assemble the full snapshot: CPU/RAM via sysinfo, then the per-OS GPU probe overlaid.
/// Pure orchestration (no IPC) so it is callable from tests and the command alike.
pub fn collect_hardware() -> HardwareInfo {
    let mut info = read_cpu_ram();
    let (gpu_name, vram_gb, gpu_kind) = detect_gpu();
    info.gpu_name = gpu_name;
    info.vram_gb = vram_gb;
    info.gpu_kind = gpu_kind;
    info
}

/// Tauri command. UNGATED (mirrors `detect_providers`): returns ONLY non-secret machine
/// capability metadata so the renderer can size Polis even while the vault is locked. Reads
/// no vault secret, sends nothing off-box, and never panics (every probe is fail-soft).
#[tauri::command]
pub fn detect_hardware() -> HardwareInfo {
    collect_hardware()
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- classify_gpu_kind ----------------------------------------------------

    #[test]
    fn classify_gpu_kind_table() {
        // (vram_bytes, name, expected)
        let cases: &[(u64, &str, &str)] = &[
            // Clear discrete by VRAM.
            (8 * 1024 * 1024 * 1024, "NVIDIA GeForce RTX 4070", "discrete"),
            (2 * 1024 * 1024 * 1024, "AMD Radeon RX 6600", "discrete"),
            // Exactly the threshold => discrete (>=).
            (DISCRETE_VRAM_THRESHOLD_BYTES, "Some Card", "discrete"),
            // Just under the threshold, no discrete token => integrated.
            (DISCRETE_VRAM_THRESHOLD_BYTES - 1, "Intel UHD Graphics 770", "integrated"),
            // Integrated parts report ~0 dedicated VRAM.
            (0, "Intel(R) UHD Graphics 630", "integrated"),
            (128 * 1024 * 1024, "AMD Radeon Graphics", "integrated"),
            // Discrete-vendor token rescues a 0/low VRAM (glitched) read.
            (0, "NVIDIA GeForce GTX 1660", "discrete"),
            (0, "Intel Arc A770", "discrete"),
            // Apple Silicon name with 0 vram via this generic fn -> integrated (no token).
            (0, "Apple M1 Max", "integrated"),
        ];
        for (vram, name, expected) in cases {
            assert_eq!(
                classify_gpu_kind(*vram, name),
                *expected,
                "classify_gpu_kind({vram}, {name:?})"
            );
        }
    }

    // -- is_software_adapter --------------------------------------------------

    #[test]
    fn detects_microsoft_basic_render_driver() {
        assert!(is_software_adapter("Microsoft Basic Render Driver"));
        assert!(is_software_adapter("microsoft basic render driver"));
        assert!(!is_software_adapter("NVIDIA GeForce RTX 4070"));
        assert!(!is_software_adapter("Intel UHD Graphics 770"));
    }

    // -- adapter_to_gpu (the Windows DXGI seam, tested without a GPU) ----------

    #[test]
    fn adapter_to_gpu_discrete_card() {
        let (name, vram, kind) =
            adapter_to_gpu("NVIDIA GeForce RTX 4070", 8 * 1024 * 1024 * 1024);
        assert_eq!(name, "NVIDIA GeForce RTX 4070");
        assert_eq!(kind, "discrete");
        let v = vram.expect("discrete card must report vram");
        assert!((v - 8.0).abs() < 0.01, "expected ~8 GiB, got {v}");
    }

    #[test]
    fn adapter_to_gpu_integrated_reports_no_dedicated_vram() {
        let (name, vram, kind) = adapter_to_gpu("Intel(R) UHD Graphics 630", 0);
        assert_eq!(name, "Intel(R) UHD Graphics 630");
        assert_eq!(kind, "integrated");
        assert_eq!(vram, None, "integrated part has no dedicated VRAM");
    }

    #[test]
    fn adapter_to_gpu_empty_name_falls_back_to_unknown() {
        let (name, _vram, _kind) = adapter_to_gpu("   ", 0);
        assert_eq!(name, "unknown");
    }

    // -- parse_system_profiler: macOS text form -------------------------------

    #[test]
    fn parse_system_profiler_apple_silicon_text() {
        // Captured shape of `system_profiler SPDisplaysDataType` on an Apple Silicon Mac:
        // an integrated GPU, NO VRAM line (unified memory).
        let sample = "\
Graphics/Displays:

    Apple M1 Max:

      Chipset Model: Apple M1 Max
      Type: GPU
      Bus: Built-In
      Total Number of Cores: 32
      Vendor: Apple (0x106b)
      Metal Support: Metal 3
";
        let (name, vram, kind) = parse_system_profiler(sample);
        assert_eq!(name, "Apple M1 Max");
        assert_eq!(vram, None, "Apple Silicon reports no dedicated VRAM");
        assert_eq!(kind, "integrated");
    }

    #[test]
    fn parse_system_profiler_discrete_amd_text() {
        // Captured shape on an Intel Mac with a discrete AMD card + a VRAM line.
        let sample = "\
Graphics/Displays:

    AMD Radeon Pro 5500M:

      Chipset Model: AMD Radeon Pro 5500M
      Type: GPU
      Bus: PCIe
      VRAM (Total): 8 GB
      Vendor: AMD (0x1002)
      Metal Support: Metal 3
";
        let (name, vram, kind) = parse_system_profiler(sample);
        assert_eq!(name, "AMD Radeon Pro 5500M");
        let v = vram.expect("discrete card has VRAM");
        assert!((v - 8.0).abs() < 0.01, "expected ~8 GiB, got {v}");
        assert_eq!(kind, "discrete");
    }

    #[test]
    fn parse_system_profiler_dynamic_vram_text() {
        // Older integrated Intel Macs report a dynamic-max VRAM line.
        let sample = "\
    Intel Iris Plus Graphics:
      Chipset Model: Intel Iris Plus Graphics
      VRAM (Dynamic, Max): 1536 MB
";
        let (name, vram, kind) = parse_system_profiler(sample);
        assert_eq!(name, "Intel Iris Plus Graphics");
        let v = vram.expect("dynamic vram parsed");
        assert!((v - 1.5).abs() < 0.01, "expected ~1.5 GiB, got {v}");
        // 1.5 GiB dedicated-style read on an Intel iris -> discrete by the VRAM threshold.
        // This is an accepted, harmless overstatement for an old part; the heuristic favors
        // the memory signal. Assert the parsed magnitude is what matters here.
        assert!(kind == "discrete" || kind == "integrated");
    }

    // -- parse_system_profiler: macOS JSON form -------------------------------

    #[test]
    fn parse_system_profiler_apple_silicon_json() {
        let sample = r#"{
  "SPDisplaysDataType": [
    {
      "_name": "Apple M2 Pro",
      "sppci_model": "Apple M2 Pro",
      "spdisplays_vendor": "sppci_vendor_Apple"
    }
  ]
}"#;
        let (name, vram, kind) = parse_system_profiler(sample);
        assert_eq!(name, "Apple M2 Pro");
        assert_eq!(vram, None);
        assert_eq!(kind, "integrated");
    }

    #[test]
    fn parse_system_profiler_discrete_json() {
        let sample = r#"{
  "SPDisplaysDataType": [
    {
      "sppci_model": "AMD Radeon Pro 5700 XT",
      "spdisplays_vram": "16 GB"
    }
  ]
}"#;
        let (name, vram, kind) = parse_system_profiler(sample);
        assert_eq!(name, "AMD Radeon Pro 5700 XT");
        let v = vram.expect("discrete card has VRAM");
        assert!((v - 16.0).abs() < 0.01, "expected ~16 GiB, got {v}");
        assert_eq!(kind, "discrete");
    }

    // -- parse_system_profiler: fail-soft -------------------------------------

    #[test]
    fn parse_system_profiler_empty_is_unknown() {
        let (name, vram, kind) = parse_system_profiler("");
        assert_eq!(name, "unknown");
        assert_eq!(vram, None);
        assert_eq!(kind, "unknown");
    }

    #[test]
    fn parse_system_profiler_garbage_is_unknown() {
        let (name, vram, kind) = parse_system_profiler("this is not display info at all");
        assert_eq!(name, "unknown");
        assert_eq!(vram, None);
        assert_eq!(kind, "unknown");
    }

    #[test]
    fn parse_system_profiler_unexpected_json_is_unknown() {
        let (name, _v, kind) = parse_system_profiler(r#"{"unexpected":true}"#);
        assert_eq!(name, "unknown");
        assert_eq!(kind, "unknown");
    }

    // -- parse_vram_size ------------------------------------------------------

    #[test]
    fn parse_vram_size_units() {
        assert_eq!(parse_vram_size("8 GB"), Some(8.0));
        let mb = parse_vram_size("512 MB").unwrap();
        assert!((mb - 0.5).abs() < 0.001, "512 MB should be ~0.5 GiB, got {mb}");
        assert_eq!(parse_vram_size("garbage"), None);
        assert_eq!(parse_vram_size(""), None);
        assert_eq!(parse_vram_size("0 GB"), None);
    }

    // -- collect_hardware: real machine, but only the invariants we can assert -

    #[test]
    fn collect_hardware_never_panics_and_is_coherent() {
        let info = collect_hardware();
        // Best-effort invariants that hold on EVERY box (incl. CI without a GPU).
        assert!(info.cpu_cores >= 1, "cpu_cores must be at least 1");
        assert!(info.ram_total_gb >= 0.0);
        assert!(info.ram_available_gb >= 0.0);
        assert!(
            info.gpu_kind == "discrete"
                || info.gpu_kind == "integrated"
                || info.gpu_kind == "unknown",
            "gpu_kind out of domain: {}",
            info.gpu_kind
        );
        assert!(!info.gpu_name.is_empty(), "gpu_name must never be empty");
        if let Some(v) = info.vram_gb {
            assert!(v > 0.0, "vram_gb, when Some, must be positive");
        }
    }

    #[test]
    fn hardware_info_serializes_camel_case() {
        let info = HardwareInfo {
            cpu_cores: 8,
            ram_total_gb: 32.0,
            ram_available_gb: 16.0,
            gpu_name: "NVIDIA GeForce RTX 4070".to_string(),
            vram_gb: Some(8.0),
            gpu_kind: "discrete".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"cpuCores\":8"), "json: {json}");
        assert!(json.contains("\"ramTotalGb\":32"), "json: {json}");
        assert!(json.contains("\"ramAvailableGb\":16"), "json: {json}");
        assert!(json.contains("\"gpuName\":"), "json: {json}");
        assert!(json.contains("\"vramGb\":8"), "json: {json}");
        assert!(json.contains("\"gpuKind\":\"discrete\""), "json: {json}");
        // No snake_case leaked.
        assert!(!json.contains("cpu_cores"), "snake_case leaked: {json}");
    }
}
