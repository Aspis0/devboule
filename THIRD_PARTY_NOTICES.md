# Third-Party Notices

Devboule's own code is licensed under Apache-2.0 (see [LICENSE](./LICENSE)). This single file
covers everything third-party and open source:

- **Part 1 — Adapted or vendored source**: code copied or adapted directly into this repository,
  attributed library by library.
- **Part 2 — Bundled dependency inventory**: the full list of npm packages and Rust crates the
  built app depends on, with their SPDX license identifiers.
- **Part 3 — Bundled art & fonts**: sprite art and typefaces shipped inside the app.
- **Part 4 — AI models & inference runtimes**: the embedding model (fetched at runtime) and the
  libraries that run it; plus the model families the app can target but does not distribute.
- **Part 5 — Integrated external tools & agents**: programs the app orchestrates but the user
  installs separately (pi and its extensions, Claude Code, Codex, Ollama, oMLX, Censor linters, …).
- **Part 6 — External services & APIs**: network services the app can call when configured.
- **Part 7 — Adapted algorithm/pattern credits**: prior work whose behavior/patterns were modeled.

---

## Part 1 — Adapted or vendored source

### NextChat / ChatGPTNextWeb — `app/components/artifacts.tsx` (HTMLPreview)

- **License:** MIT
- **Copyright:** Copyright (c) 2023-2025 ChatGPTNextWeb / NextChat
- **Repository:** https://github.com/ChatGPTNextWeb/NextChat

**Usage in this project:**
The `ArtifactView` component (`src/components/projects/artifact/ArtifactView.tsx`,
Phase 1) is adapted from NextChat's `HTMLPreview` component. The core structure — a
sandboxed iframe whose guest posts its layout height back to the parent via postMessage
— is taken from that source. Two intentional departures from the original:

1. The document is served from a separate origin (`artifact://localhost/<id>` or
   `http://artifact.localhost/<id>` on Windows) so it carries its own CSP and runs
   real inline JS without inheriting the app's `script-src 'self'`. NextChat uses
   `srcDoc` (PATH A); we use PATH B.
2. The weaker `id == frameId` string discriminator is replaced with the stronger
   source-identity trust anchor `event.source === iframe.contentWindow`. See
   `artifactProtocol.ts` for the rationale.

---

### picturepan2/devices.css — iPhone 14 Pro + Google Pixel 6 Pro skins

- **License:** MIT
- **Copyright:** Copyright (c) 2017 Yan Zhu
- **Repository:** https://github.com/picturepan2/devices.css

**Usage in this project:**
The CSS rules for `.device`, `.device-screen`, `.device-iphone-14-pro`, and
`.device-google-pixel-6-pro` in
`src/components/projects/artifact/deviceFrames.css` (Phase 4) are taken verbatim from
this library. Adaptations:

- `.device-screen`'s content child is an `<iframe>` instead of `<img>`.
- `overflow: hidden` added to `.device-frame` to clip the iframe to the bezel's
  `border-radius`.
- `transform: translateZ(0)` added to `.device-frame` (Safari / WKWebView clip fix:
  forces GPU compositing so `overflow: hidden + border-radius` correctly clips the
  iframe on macOS WebKit).
- `overflow: hidden` is intentionally NOT placed on the `<iframe>` element itself
  (doing so would kill internal scroll in the artifact).

---

### jhildenbiddle/css-device-frames — browser chrome frame

- **License:** MIT
- **Copyright:** Copyright (c) 2021 John Hildenbiddle
- **Repository:** https://github.com/jhildenbiddle/css-device-frames

**Usage in this project:**
The `.app-frame` browser chrome wrapper in
`src/components/projects/artifact/deviceFrames.css` (Phase 4) is adapted from this
library. Key rules retained:

- `.app-frame > iframe:only-child` fill rule — adapted to `.af-frame-content > *`
  because our `<ArtifactView>` wraps the `<iframe>` in a container div.
- `[data-url]::after { content: attr(data-url) }` address-bar pattern.

The visual chrome bar (traffic-light dots + background) is self-implemented (structural
idea from css-device-frames; no code lines copied for that section).

---

### NOT included — Open WebUI

Open WebUI uses a custom license (anti-rebrand clause for deployments serving > 50
users) that is **incompatible** with this project. Its code is NOT present in this
repository. The earlier web-research notes that mis-labeled it MIT have been corrected.

Concepts drawn from Open WebUI that are uncopyrightable and self-reimplemented here:

- Injecting `<meta http-equiv="Content-Security-Policy">` into the artifact document.
- `event.source === iframe.contentWindow` as the postMessage trust anchor (this is a
  standard web platform pattern documented on MDN/web.dev).

---

### cmdk — command-score

**Source:** https://github.com/pacocoursey/cmdk (`cmdk/src/command-score.ts`)
**License:** MIT — Copyright (c) 2022 Paco Coursey.

```
MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction... (full MIT text). THE SOFTWARE IS PROVIDED
"AS IS", WITHOUT WARRANTY OF ANY KIND.
```

**Usage in this project:**
`src/vendor/commandScore.ts` is a verbatim copy of the file above (zero dependencies).
It scores fuzzy/abbreviation matches and powers the Work Console "Skills & Tools"
library search: `commandScore(item.name, query, [item.description, item.kind])`,
filtering scores `> 0` and sorting descending.

---

## Part 2 — Bundled dependency inventory

This section inventories the open-source dependencies bundled into Devboule's distributable app — the JavaScript frontend (npm) and the Rust backend (Cargo crates across `src-tauri`, `oracle-core`, and `devboule-mcp`). It is auto-generated; regenerate with `license-checker --production` (npm) and `cargo license` (Rust). Copied/adapted source snippets are attributed in Part 1 above.

> Each entry lists its SPDX license identifier. Full license texts live at https://spdx.org/licenses/ ; for a binary release, bundle the full texts of every license below with the installer (e.g. via `cargo about` / `cargo-bundle-licenses`).

### Notes worth reading before publishing

- **GSAP (`gsap`)** ships under GreenSock's *custom* "no-charge" license (<https://gsap.com/standard-license/>), **not** an OSI open-source license. It is free for the vast majority of uses, but has commercial terms for some products — confirm your use qualifies before publishing.

- **`inferno` (CDDL-1.0)** and the **MPL-2.0** crates are *file-level weak copyleft*: you may distribute them in a larger work, but modifications to their own source files must remain under the same license. No changes are made to their sources here.

- No strong-copyleft (GPL/AGPL) dependency is present. All licenses are permissive or file-level weak copyleft, compatible with an Apache-2.0 distribution.

### npm dependencies (41)

| Package | Version | License | Repository |
|---|---|---|---|
| `@fontsource-variable/instrument-sans` | 5.2.8 | OFL-1.1 | https://github.com/fontsource/font-files |
| `@fontsource-variable/source-serif-4` | 5.2.9 | OFL-1.1 | https://github.com/fontsource/font-files |
| `@pixi/colord` | 2.9.6 | MIT | https://github.com/omgovich/colord |
| `@tauri-apps/api` | 2.11.0 | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri |
| `@tauri-apps/plugin-dialog` | 2.7.1 | MIT OR Apache-2.0 | https://github.com/tauri-apps/plugins-workspace |
| `@tauri-apps/plugin-notification` | 2.3.3 | MIT OR Apache-2.0 | https://github.com/tauri-apps/plugins-workspace |
| `@types/earcut` | 3.0.0 | MIT | https://github.com/DefinitelyTyped/DefinitelyTyped |
| `@types/gradient-parser` | 0.1.5 | MIT | https://github.com/DefinitelyTyped/DefinitelyTyped |
| `@types/prop-types` | 15.7.15 | MIT | https://github.com/DefinitelyTyped/DefinitelyTyped |
| `@types/react` | 18.3.29 | MIT | https://github.com/DefinitelyTyped/DefinitelyTyped |
| `@types/trusted-types` | 2.0.7 | MIT | https://github.com/DefinitelyTyped/DefinitelyTyped |
| `@webgpu/types` | 0.1.70 | BSD-3-Clause | https://github.com/gpuweb/types |
| `@xmldom/xmldom` | 0.8.13 | MIT | https://github.com/xmldom/xmldom |
| `@xterm/addon-fit` | 0.11.0 | MIT | https://github.com/xtermjs/xterm.js/tree/master/addons/addon-fit |
| `@xterm/xterm` | 6.0.0 | MIT | https://github.com/xtermjs/xterm.js |
| `csstype` | 3.2.3 | MIT | https://github.com/frenic/csstype |
| `d3-dispatch` | 3.0.1 | ISC | https://github.com/d3/d3-dispatch |
| `d3-force` | 3.0.0 | ISC | https://github.com/d3/d3-force |
| `d3-quadtree` | 3.0.1 | ISC | https://github.com/d3/d3-quadtree |
| `d3-timer` | 3.0.1 | ISC | https://github.com/d3/d3-timer |
| `dompurify` | 3.4.12 | (MPL-2.0 OR Apache-2.0) | https://github.com/cure53/DOMPurify |
| `earcut` | 3.0.2 | ISC | https://github.com/mapbox/earcut |
| `eventemitter3` | 5.0.4 | MIT | https://github.com/primus/eventemitter3 |
| `gifuct-js` | 2.1.2 | MIT | https://github.com/matt-way/gifuct-js |
| `gsap` | 3.15.0 | Custom: https://gsap.com/GSAP-share-image.png | https://github.com/greensock/GSAP |
| `ismobilejs` | 1.1.1 | MIT | https://github.com/kaimallea/isMobile |
| `js-binary-schema-parser` | 2.0.3 | MIT | https://github.com/matt-way/jsBinarySchemaParser |
| `js-tokens` | 4.0.0 | MIT | https://github.com/lydell/js-tokens |
| `loose-envify` | 1.4.0 | MIT | https://github.com/zertosh/loose-envify |
| `lucide-react` | 0.460.0 | ISC | https://github.com/lucide-icons/lucide |
| `parse-svg-path` | 0.1.2 | MIT | https://github.com/jkroso/parse-svg-path |
| `perfect-arrows` | 0.3.7 | MIT | https://github.com/steveruizok/perfect-arrows |
| `pixi-filters` | 6.1.5 | MIT | https://github.com/pixijs/filters |
| `pixi-viewport` | 6.0.3 | MIT | https://github.com/davidfig/pixi-viewport |
| `pixi.js` | 8.18.1 | MIT | https://github.com/pixijs/pixijs |
| `react` | 18.3.1 | MIT | https://github.com/facebook/react |
| `react-dom` | 18.3.1 | MIT | https://github.com/facebook/react |
| `react-resizable-panels` | 2.1.9 | MIT | https://github.com/bvaughn/react-resizable-panels |
| `scheduler` | 0.23.2 | MIT | https://github.com/facebook/react |
| `tiny-lru` | 11.4.7 | BSD-3-Clause | https://github.com/avoidwork/tiny-lru |
| `zustand` | 5.0.14 | MIT | https://github.com/pmndrs/zustand |

### Rust crates (1177)

| Crate | Version | License |
|---|---|---|
| `addr2line` | 0.25.1 | Apache-2.0 OR MIT |
| `adler2` | 2.0.1 | 0BSD OR Apache-2.0 OR MIT |
| `aead` | 0.5.2 | Apache-2.0 OR MIT |
| `aes` | 0.8.4 | Apache-2.0 OR MIT |
| `aes-gcm` | 0.10.3 | Apache-2.0 OR MIT |
| `ahash` | 0.8.12 | Apache-2.0 OR MIT |
| `aho-corasick` | 1.1.4 | MIT OR Unlicense |
| `aligned` | 0.4.3 | Apache-2.0 OR MIT |
| `aligned-vec` | 0.6.4 | MIT |
| `alloc-no-stdlib` | 2.0.4 | BSD-3-Clause |
| `alloc-stdlib` | 0.2.2 | BSD-3-Clause |
| `alloca` | 0.4.0 | MIT |
| `allocator-api2` | 0.2.21 | Apache-2.0 OR MIT |
| `android_system_properties` | 0.1.5 | Apache-2.0 OR MIT |
| `anes` | 0.1.6 | Apache-2.0 OR MIT |
| `anstream` | 1.0.0 | Apache-2.0 OR MIT |
| `anstyle` | 1.0.14 | Apache-2.0 OR MIT |
| `anstyle-parse` | 1.0.0 | Apache-2.0 OR MIT |
| `anstyle-query` | 1.1.5 | Apache-2.0 OR MIT |
| `anstyle-wincon` | 3.0.11 | Apache-2.0 OR MIT |
| `anyhow` | 1.0.102 | Apache-2.0 OR MIT |
| `anyhow` | 1.0.103 | Apache-2.0 OR MIT |
| `anyhow` | 1.0.104 | Apache-2.0 OR MIT |
| `arbitrary` | 1.4.2 | Apache-2.0 OR MIT |
| `arc-swap` | 1.9.2 | Apache-2.0 OR MIT |
| `arg_enum_proc_macro` | 0.3.4 | MIT |
| `arrayref` | 0.3.9 | BSD-2-Clause |
| `arrayvec` | 0.7.8 | Apache-2.0 OR MIT |
| `arrow` | 58.3.0 | Apache-2.0 |
| `arrow-arith` | 58.3.0 | Apache-2.0 |
| `arrow-array` | 58.3.0 | Apache-2.0 AND MIT |
| `arrow-buffer` | 58.3.0 | Apache-2.0 |
| `arrow-cast` | 58.3.0 | Apache-2.0 |
| `arrow-csv` | 58.3.0 | Apache-2.0 |
| `arrow-data` | 58.3.0 | Apache-2.0 |
| `arrow-ipc` | 58.3.0 | Apache-2.0 |
| `arrow-json` | 58.3.0 | Apache-2.0 |
| `arrow-ord` | 58.3.0 | Apache-2.0 |
| `arrow-row` | 58.3.0 | Apache-2.0 |
| `arrow-schema` | 58.3.0 | Apache-2.0 |
| `arrow-select` | 58.3.0 | Apache-2.0 |
| `arrow-string` | 58.3.0 | Apache-2.0 |
| `as-slice` | 0.2.1 | Apache-2.0 OR MIT |
| `async-broadcast` | 0.7.2 | Apache-2.0 OR MIT |
| `async-channel` | 2.5.0 | Apache-2.0 OR MIT |
| `async-compression` | 0.4.42 | Apache-2.0 OR MIT |
| `async-executor` | 1.14.0 | Apache-2.0 OR MIT |
| `async-io` | 2.6.0 | Apache-2.0 OR MIT |
| `async-lock` | 3.4.2 | Apache-2.0 OR MIT |
| `async-process` | 2.5.0 | Apache-2.0 OR MIT |
| `async-recursion` | 1.1.1 | Apache-2.0 OR MIT |
| `async-signal` | 0.2.14 | Apache-2.0 OR MIT |
| `async-task` | 4.7.1 | Apache-2.0 OR MIT |
| `async-trait` | 0.1.89 | Apache-2.0 OR MIT |
| `async_cell` | 0.2.3 | MIT |
| `atk` | 0.18.2 | MIT |
| `atk-sys` | 0.18.2 | MIT |
| `atoi` | 2.0.0 | MIT |
| `atomic-waker` | 1.1.2 | Apache-2.0 OR MIT |
| `autocfg` | 1.5.1 | Apache-2.0 OR MIT |
| `av-scenechange` | 0.14.1 | MIT |
| `av1-grain` | 0.2.5 | BSD-2-Clause |
| `avif-serialize` | 0.8.9 | BSD-3-Clause |
| `axum` | 0.8.9 | MIT |
| `axum-core` | 0.5.6 | MIT |
| `backtrace` | 0.3.76 | Apache-2.0 OR MIT |
| `base64` | 0.13.1 | Apache-2.0 OR MIT |
| `base64` | 0.21.7 | Apache-2.0 OR MIT |
| `base64` | 0.22.1 | Apache-2.0 OR MIT |
| `base64ct` | 1.8.3 | Apache-2.0 OR MIT |
| `bigdecimal` | 0.4.10 | Apache-2.0 OR MIT |
| `bit-set` | 0.8.0 | Apache-2.0 OR MIT |
| `bit-vec` | 0.8.0 | Apache-2.0 OR MIT |
| `bit_field` | 0.10.3 | Apache-2.0 OR MIT |
| `bitflags` | 1.3.2 | Apache-2.0 OR MIT |
| `bitflags` | 2.11.1 | Apache-2.0 OR MIT |
| `bitflags` | 2.13.0 | Apache-2.0 OR MIT |
| `bitflags` | 2.13.1 | Apache-2.0 OR MIT |
| `bitpacking` | 0.9.3 | MIT |
| `bitstream-io` | 4.10.0 | Apache-2.0 OR MIT |
| `bitvec` | 1.1.1 | MIT |
| `blake2` | 0.10.6 | Apache-2.0 OR MIT |
| `blake3` | 1.8.5 | Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR CC0-1.0 |
| `block` | 0.1.6 | MIT |
| `block-buffer` | 0.10.4 | Apache-2.0 OR MIT |
| `block2` | 0.5.1 | MIT |
| `block2` | 0.6.2 | MIT |
| `blocking` | 1.6.2 | Apache-2.0 OR MIT |
| `brotli` | 8.0.2 | BSD-3-Clause AND MIT |
| `brotli-decompressor` | 5.0.0 | BSD-3-Clause OR MIT |
| `bs58` | 0.5.1 | Apache-2.0 OR MIT |
| `bstr` | 1.12.1 | Apache-2.0 OR MIT |
| `built` | 0.8.1 | MIT |
| `bumpalo` | 3.20.3 | Apache-2.0 OR MIT |
| `bytecheck` | 0.8.2 | MIT |
| `bytecheck_derive` | 0.8.2 | MIT |
| `bytecount` | 0.6.9 | Apache-2.0 OR MIT |
| `bytemuck` | 1.25.0 | Apache-2.0 OR MIT OR Zlib |
| `bytemuck` | 1.25.1 | Apache-2.0 OR MIT OR Zlib |
| `bytemuck_derive` | 1.11.0 | Apache-2.0 OR MIT OR Zlib |
| `byteorder` | 1.5.0 | MIT OR Unlicense |
| `byteorder-lite` | 0.1.0 | MIT OR Unlicense |
| `bytes` | 1.11.1 | MIT |
| `bytes` | 1.12.1 | MIT |
| `cairo-rs` | 0.18.5 | MIT |
| `cairo-sys-rs` | 0.18.2 | MIT |
| `camino` | 1.2.2 | Apache-2.0 OR MIT |
| `candle-core` | 0.10.2 | Apache-2.0 OR MIT |
| `candle-metal-kernels` | 0.10.2 | Apache-2.0 OR MIT |
| `candle-nn` | 0.10.2 | Apache-2.0 OR MIT |
| `candle-ug` | 0.10.2 | Apache-2.0 OR MIT |
| `cargo-platform` | 0.1.9 | Apache-2.0 OR MIT |
| `cargo_metadata` | 0.19.2 | MIT |
| `cargo_toml` | 0.22.3 | Apache-2.0 OR MIT |
| `cast` | 0.3.0 | Apache-2.0 OR MIT |
| `castaway` | 0.2.4 | MIT |
| `cc` | 1.2.62 | Apache-2.0 OR MIT |
| `cc` | 1.2.67 | Apache-2.0 OR MIT |
| `cc` | 1.3.0 | Apache-2.0 OR MIT |
| `cedarwood` | 0.5.0 | BSD-2-Clause |
| `cesu8` | 1.1.0 | Apache-2.0 OR MIT |
| `cfb` | 0.7.3 | MIT |
| `cfg-expr` | 0.15.8 | Apache-2.0 OR MIT |
| `cfg-if` | 1.0.4 | Apache-2.0 OR MIT |
| `cfg_aliases` | 0.1.1 | MIT |
| `cfg_aliases` | 0.2.1 | MIT |
| `cfg_aliases` | 0.2.2 | MIT |
| `chacha20` | 0.10.1 | Apache-2.0 OR MIT |
| `chrono` | 0.4.44 | Apache-2.0 OR MIT |
| `chrono` | 0.4.45 | Apache-2.0 OR MIT |
| `chrono-tz` | 0.10.4 | Apache-2.0 OR MIT |
| `ciborium` | 0.2.2 | Apache-2.0 |
| `ciborium-io` | 0.2.2 | Apache-2.0 |
| `ciborium-ll` | 0.2.2 | Apache-2.0 |
| `cipher` | 0.4.4 | Apache-2.0 OR MIT |
| `clap` | 4.6.1 | Apache-2.0 OR MIT |
| `clap_builder` | 4.6.0 | Apache-2.0 OR MIT |
| `clap_derive` | 4.6.1 | Apache-2.0 OR MIT |
| `clap_lex` | 1.1.0 | Apache-2.0 OR MIT |
| `color_quant` | 1.1.0 | MIT |
| `colorchoice` | 1.0.5 | Apache-2.0 OR MIT |
| `combine` | 4.6.7 | MIT |
| `comfy-table` | 7.2.2 | MIT |
| `compact_str` | 0.9.1 | MIT |
| `compression-codecs` | 0.4.38 | Apache-2.0 OR MIT |
| `compression-core` | 0.4.32 | Apache-2.0 OR MIT |
| `concurrent-queue` | 2.5.0 | Apache-2.0 OR MIT |
| `console` | 0.15.11 | MIT |
| `console` | 0.16.4 | MIT |
| `const-oid` | 0.9.6 | Apache-2.0 OR MIT |
| `const-random` | 0.1.18 | Apache-2.0 OR MIT |
| `const-random-macro` | 0.1.16 | Apache-2.0 OR MIT |
| `constant_time_eq` | 0.4.2 | Apache-2.0 OR CC0-1.0 OR MIT-0 |
| `cookie` | 0.18.1 | Apache-2.0 OR MIT |
| `cookie_store` | 0.22.1 | Apache-2.0 OR MIT |
| `core-foundation` | 0.9.4 | Apache-2.0 OR MIT |
| `core-foundation` | 0.10.1 | Apache-2.0 OR MIT |
| `core-foundation-sys` | 0.8.7 | Apache-2.0 OR MIT |
| `core-graphics` | 0.25.0 | Apache-2.0 OR MIT |
| `core-graphics-types` | 0.1.3 | Apache-2.0 OR MIT |
| `core-graphics-types` | 0.2.0 | Apache-2.0 OR MIT |
| `cpp_demangle` | 0.4.5 | Apache-2.0 OR MIT |
| `cpufeatures` | 0.2.17 | Apache-2.0 OR MIT |
| `cpufeatures` | 0.3.0 | Apache-2.0 OR MIT |
| `crc32fast` | 1.5.0 | Apache-2.0 OR MIT |
| `criterion` | 0.8.2 | Apache-2.0 OR MIT |
| `criterion-plot` | 0.8.2 | Apache-2.0 OR MIT |
| `crossbeam-channel` | 0.5.15 | Apache-2.0 OR MIT |
| `crossbeam-channel` | 0.5.16 | Apache-2.0 OR MIT |
| `crossbeam-deque` | 0.8.6 | Apache-2.0 OR MIT |
| `crossbeam-deque` | 0.8.7 | Apache-2.0 OR MIT |
| `crossbeam-epoch` | 0.9.18 | Apache-2.0 OR MIT |
| `crossbeam-epoch` | 0.9.20 | Apache-2.0 OR MIT |
| `crossbeam-queue` | 0.3.13 | Apache-2.0 OR MIT |
| `crossbeam-skiplist` | 0.1.3 | Apache-2.0 OR MIT |
| `crossbeam-utils` | 0.8.21 | Apache-2.0 OR MIT |
| `crossbeam-utils` | 0.8.22 | Apache-2.0 OR MIT |
| `crunchy` | 0.2.4 | MIT |
| `crypto-common` | 0.1.7 | Apache-2.0 OR MIT |
| `cssparser` | 0.36.0 | MPL-2.0 |
| `cssparser-macros` | 0.6.1 | MPL-2.0 |
| `csv` | 1.4.0 | MIT OR Unlicense |
| `csv-core` | 0.1.13 | MIT OR Unlicense |
| `ctor` | 0.8.0 | Apache-2.0 OR MIT |
| `ctor-proc-macro` | 0.0.7 | Apache-2.0 OR MIT |
| `ctr` | 0.9.2 | Apache-2.0 OR MIT |
| `curve25519-dalek` | 4.1.3 | BSD-3-Clause |
| `curve25519-dalek-derive` | 0.1.1 | Apache-2.0 OR MIT |
| `daachorse` | 2.1.1 | Apache-2.0 OR MIT |
| `darling` | 0.20.11 | MIT |
| `darling` | 0.21.3 | MIT |
| `darling` | 0.23.0 | MIT |
| `darling_core` | 0.20.11 | MIT |
| `darling_core` | 0.21.3 | MIT |
| `darling_core` | 0.23.0 | MIT |
| `darling_macro` | 0.20.11 | MIT |
| `darling_macro` | 0.21.3 | MIT |
| `darling_macro` | 0.23.0 | MIT |
| `dary_heap` | 0.3.9 | Apache-2.0 OR MIT |
| `dashmap` | 6.2.1 | MIT |
| `datafusion` | 53.1.0 | Apache-2.0 |
| `datafusion-catalog` | 53.1.0 | Apache-2.0 |
| `datafusion-catalog-listing` | 53.1.0 | Apache-2.0 |
| `datafusion-common` | 53.1.0 | Apache-2.0 |
| `datafusion-common-runtime` | 53.1.0 | Apache-2.0 |
| `datafusion-datasource` | 53.1.0 | Apache-2.0 |
| `datafusion-datasource-arrow` | 53.1.0 | Apache-2.0 |
| `datafusion-datasource-csv` | 53.1.0 | Apache-2.0 |
| `datafusion-datasource-json` | 53.1.0 | Apache-2.0 |
| `datafusion-doc` | 53.1.0 | Apache-2.0 |
| `datafusion-execution` | 53.1.0 | Apache-2.0 |
| `datafusion-expr` | 53.1.0 | Apache-2.0 |
| `datafusion-expr-common` | 53.1.0 | Apache-2.0 |
| `datafusion-functions` | 53.1.0 | Apache-2.0 |
| `datafusion-functions-aggregate` | 53.1.0 | Apache-2.0 |
| `datafusion-functions-aggregate-common` | 53.1.0 | Apache-2.0 |
| `datafusion-functions-nested` | 53.1.0 | Apache-2.0 |
| `datafusion-functions-table` | 53.1.0 | Apache-2.0 |
| `datafusion-functions-window` | 53.1.0 | Apache-2.0 |
| `datafusion-functions-window-common` | 53.1.0 | Apache-2.0 |
| `datafusion-macros` | 53.1.0 | Apache-2.0 |
| `datafusion-optimizer` | 53.1.0 | Apache-2.0 |
| `datafusion-physical-expr` | 53.1.0 | Apache-2.0 |
| `datafusion-physical-expr-adapter` | 53.1.0 | Apache-2.0 |
| `datafusion-physical-expr-common` | 53.1.0 | Apache-2.0 |
| `datafusion-physical-optimizer` | 53.1.0 | Apache-2.0 |
| `datafusion-physical-plan` | 53.1.0 | Apache-2.0 |
| `datafusion-pruning` | 53.1.0 | Apache-2.0 |
| `datafusion-session` | 53.1.0 | Apache-2.0 |
| `datafusion-sql` | 53.1.0 | Apache-2.0 |
| `dbus` | 0.9.11 | Apache-2.0 OR MIT |
| `debugid` | 0.8.0 | Apache-2.0 |
| `defmt` | 1.1.1 | Apache-2.0 OR MIT |
| `defmt-macros` | 1.1.1 | Apache-2.0 OR MIT |
| `defmt-parser` | 1.0.0 | Apache-2.0 OR MIT |
| `der` | 0.7.10 | Apache-2.0 OR MIT |
| `der` | 0.8.1 | Apache-2.0 OR MIT |
| `deranged` | 0.5.8 | Apache-2.0 OR MIT |
| `derive_builder` | 0.20.2 | Apache-2.0 OR MIT |
| `derive_builder_core` | 0.20.2 | Apache-2.0 OR MIT |
| `derive_builder_macro` | 0.20.2 | Apache-2.0 OR MIT |
| `derive_more` | 2.1.1 | MIT |
| `derive_more-impl` | 2.1.1 | MIT |
| `digest` | 0.10.7 | Apache-2.0 OR MIT |
| `dirs` | 6.0.0 | Apache-2.0 OR MIT |
| `dirs-sys` | 0.5.0 | Apache-2.0 OR MIT |
| `dispatch2` | 0.3.1 | Apache-2.0 OR MIT OR Zlib |
| `displaydoc` | 0.2.6 | Apache-2.0 OR MIT |
| `dlopen2` | 0.8.2 | MIT |
| `dlopen2_derive` | 0.4.3 | MIT |
| `document-features` | 0.2.12 | Apache-2.0 OR MIT |
| `dom_query` | 0.27.0 | MIT |
| `downcast-rs` | 1.2.1 | Apache-2.0 OR MIT |
| `dpi` | 0.1.2 | Apache-2.0 AND MIT |
| `dtoa` | 1.0.11 | Apache-2.0 OR MIT |
| `dtoa-short` | 0.3.5 | MPL-2.0 |
| `dtor` | 0.3.0 | Apache-2.0 OR MIT |
| `dtor-proc-macro` | 0.0.6 | Apache-2.0 OR MIT |
| `dunce` | 1.0.5 | Apache-2.0 OR CC0-1.0 OR MIT-0 |
| `dyn-clone` | 1.0.20 | Apache-2.0 OR MIT |
| `dyn-stack` | 0.13.2 | MIT |
| `dyn-stack-macros` | 0.1.3 | MIT |
| `ed25519` | 2.2.3 | Apache-2.0 OR MIT |
| `ed25519-dalek` | 2.2.0 | BSD-3-Clause |
| `either` | 1.16.0 | Apache-2.0 OR MIT |
| `embed-resource` | 3.0.9 | MIT |
| `embed_plist` | 1.2.2 | Apache-2.0 OR MIT |
| `encode_unicode` | 1.0.0 | Apache-2.0 OR MIT |
| `encoding_rs` | 0.8.35 | (Apache-2.0 OR MIT) AND BSD-3-Clause |
| `encoding_rs_io` | 0.1.7 | Apache-2.0 OR MIT |
| `endi` | 1.1.1 | MIT |
| `enum-as-inner` | 0.6.1 | Apache-2.0 OR MIT |
| `enumflags2` | 0.7.12 | Apache-2.0 OR MIT |
| `enumflags2_derive` | 0.7.12 | Apache-2.0 OR MIT |
| `equator` | 0.4.2 | MIT |
| `equator-macro` | 0.4.2 | MIT |
| `equivalent` | 1.0.2 | Apache-2.0 OR MIT |
| `erased-serde` | 0.4.10 | Apache-2.0 OR MIT |
| `errno` | 0.3.14 | Apache-2.0 OR MIT |
| `esaxx-rs` | 0.1.10 | Apache-2.0 |
| `ethnum` | 1.5.3 | Apache-2.0 OR MIT |
| `event-listener` | 5.4.1 | Apache-2.0 OR MIT |
| `event-listener-strategy` | 0.5.4 | Apache-2.0 OR MIT |
| `exr` | 1.74.2 | BSD-3-Clause |
| `fallible-iterator` | 0.3.0 | Apache-2.0 OR MIT |
| `fallible-streaming-iterator` | 0.1.9 | Apache-2.0 OR MIT |
| `fast-float2` | 0.2.3 | Apache-2.0 OR MIT |
| `fastembed` | 5.17.2 | Apache-2.0 |
| `fastrand` | 2.4.1 | Apache-2.0 OR MIT |
| `fastrand` | 2.5.0 | Apache-2.0 OR MIT |
| `fax` | 0.2.7 | MIT |
| `fdeflate` | 0.3.7 | Apache-2.0 OR MIT |
| `fiat-crypto` | 0.2.9 | Apache-2.0 OR BSD-1-Clause OR MIT |
| `field-offset` | 0.3.6 | Apache-2.0 OR MIT |
| `filedescriptor` | 0.8.3 | MIT |
| `filetime` | 0.2.29 | Apache-2.0 OR MIT |
| `find-msvc-tools` | 0.1.9 | Apache-2.0 OR MIT |
| `findshlibs` | 0.10.2 | Apache-2.0 OR MIT |
| `fixedbitset` | 0.5.7 | Apache-2.0 OR MIT |
| `flatbuffers` | 25.12.19 | Apache-2.0 |
| `flate2` | 1.1.9 | Apache-2.0 OR MIT |
| `float8` | 0.7.0 | MIT |
| `fnv` | 1.0.7 | Apache-2.0 OR MIT |
| `foldhash` | 0.1.5 | Zlib |
| `foldhash` | 0.2.0 | Zlib |
| `foreign-types` | 0.3.2 | Apache-2.0 OR MIT |
| `foreign-types` | 0.5.0 | Apache-2.0 OR MIT |
| `foreign-types-macros` | 0.2.3 | Apache-2.0 OR MIT |
| `foreign-types-shared` | 0.1.1 | Apache-2.0 OR MIT |
| `foreign-types-shared` | 0.3.1 | Apache-2.0 OR MIT |
| `form_urlencoded` | 1.2.2 | Apache-2.0 OR MIT |
| `fs2` | 0.4.3 | Apache-2.0 OR MIT |
| `fsevent-sys` | 4.1.0 | MIT |
| `fsst` | 8.0.0 | Apache-2.0 |
| `fst` | 0.4.7 | MIT OR Unlicense |
| `funty` | 2.0.0 | MIT |
| `futures` | 0.3.32 | Apache-2.0 OR MIT |
| `futures` | 0.3.33 | Apache-2.0 OR MIT |
| `futures-channel` | 0.3.32 | Apache-2.0 OR MIT |
| `futures-channel` | 0.3.33 | Apache-2.0 OR MIT |
| `futures-core` | 0.3.32 | Apache-2.0 OR MIT |
| `futures-core` | 0.3.33 | Apache-2.0 OR MIT |
| `futures-executor` | 0.3.32 | Apache-2.0 OR MIT |
| `futures-executor` | 0.3.33 | Apache-2.0 OR MIT |
| `futures-io` | 0.3.32 | Apache-2.0 OR MIT |
| `futures-io` | 0.3.33 | Apache-2.0 OR MIT |
| `futures-lite` | 2.6.1 | Apache-2.0 OR MIT |
| `futures-macro` | 0.3.32 | Apache-2.0 OR MIT |
| `futures-macro` | 0.3.33 | Apache-2.0 OR MIT |
| `futures-sink` | 0.3.32 | Apache-2.0 OR MIT |
| `futures-sink` | 0.3.33 | Apache-2.0 OR MIT |
| `futures-task` | 0.3.32 | Apache-2.0 OR MIT |
| `futures-task` | 0.3.33 | Apache-2.0 OR MIT |
| `futures-util` | 0.3.32 | Apache-2.0 OR MIT |
| `futures-util` | 0.3.33 | Apache-2.0 OR MIT |
| `gdk` | 0.18.2 | MIT |
| `gdk-pixbuf` | 0.18.5 | MIT |
| `gdk-pixbuf-sys` | 0.18.0 | MIT |
| `gdk-sys` | 0.18.2 | MIT |
| `gdkwayland-sys` | 0.18.2 | MIT |
| `gdkx11` | 0.18.2 | MIT |
| `gdkx11-sys` | 0.18.2 | MIT |
| `gemm` | 0.18.2 | MIT |
| `gemm` | 0.19.0 | MIT |
| `gemm-c32` | 0.18.2 | MIT |
| `gemm-c32` | 0.19.0 | MIT |
| `gemm-c64` | 0.18.2 | MIT |
| `gemm-c64` | 0.19.0 | MIT |
| `gemm-common` | 0.18.2 | MIT |
| `gemm-common` | 0.19.0 | MIT |
| `gemm-f16` | 0.18.2 | MIT |
| `gemm-f16` | 0.19.0 | MIT |
| `gemm-f32` | 0.18.2 | MIT |
| `gemm-f32` | 0.19.0 | MIT |
| `gemm-f64` | 0.18.2 | MIT |
| `gemm-f64` | 0.19.0 | MIT |
| `generator` | 0.8.9 | Apache-2.0 OR MIT |
| `generic-array` | 0.14.7 | MIT |
| `getrandom` | 0.2.17 | Apache-2.0 OR MIT |
| `getrandom` | 0.3.4 | Apache-2.0 OR MIT |
| `getrandom` | 0.4.2 | Apache-2.0 OR MIT |
| `getrandom` | 0.4.3 | Apache-2.0 OR MIT |
| `ghash` | 0.5.1 | Apache-2.0 OR MIT |
| `gif` | 0.14.2 | Apache-2.0 OR MIT |
| `gimli` | 0.32.3 | Apache-2.0 OR MIT |
| `gio` | 0.18.4 | MIT |
| `gio-sys` | 0.18.1 | MIT |
| `glib` | 0.18.5 | MIT |
| `glib-macros` | 0.18.5 | MIT |
| `glib-sys` | 0.18.1 | MIT |
| `glob` | 0.3.3 | Apache-2.0 OR MIT |
| `globset` | 0.4.18 | MIT OR Unlicense |
| `gobject-sys` | 0.18.0 | MIT |
| `gtk` | 0.18.2 | MIT |
| `gtk-sys` | 0.18.2 | MIT |
| `gtk3-macros` | 0.18.2 | MIT |
| `h2` | 0.4.15 | MIT |
| `half` | 2.7.1 | Apache-2.0 OR MIT |
| `hashbrown` | 0.12.3 | Apache-2.0 OR MIT |
| `hashbrown` | 0.14.5 | Apache-2.0 OR MIT |
| `hashbrown` | 0.15.5 | Apache-2.0 OR MIT |
| `hashbrown` | 0.16.1 | Apache-2.0 OR MIT |
| `hashbrown` | 0.17.1 | Apache-2.0 OR MIT |
| `hashlink` | 0.9.1 | Apache-2.0 OR MIT |
| `heck` | 0.4.1 | Apache-2.0 OR MIT |
| `heck` | 0.5.0 | Apache-2.0 OR MIT |
| `hermit-abi` | 0.5.2 | Apache-2.0 OR MIT |
| `hex` | 0.4.3 | Apache-2.0 OR MIT |
| `hf-hub` | 0.5.0 | Apache-2.0 |
| `hkdf` | 0.12.4 | Apache-2.0 OR MIT |
| `hmac` | 0.12.1 | Apache-2.0 OR MIT |
| `hmac-sha256` | 1.1.14 | ISC |
| `html5ever` | 0.38.0 | Apache-2.0 OR MIT |
| `http` | 1.4.1 | Apache-2.0 OR MIT |
| `http` | 1.4.2 | Apache-2.0 OR MIT |
| `http-body` | 1.0.1 | MIT |
| `http-body` | 1.1.0 | MIT |
| `http-body-util` | 0.1.3 | MIT |
| `http-body-util` | 0.1.4 | MIT |
| `httparse` | 1.10.1 | Apache-2.0 OR MIT |
| `httpdate` | 1.0.3 | Apache-2.0 OR MIT |
| `humantime` | 2.4.0 | Apache-2.0 OR MIT |
| `hyper` | 1.9.0 | MIT |
| `hyper` | 1.10.1 | MIT |
| `hyper` | 1.11.0 | MIT |
| `hyper-rustls` | 0.27.9 | Apache-2.0 OR ISC OR MIT |
| `hyper-tls` | 0.6.0 | Apache-2.0 OR MIT |
| `hyper-util` | 0.1.20 | MIT |
| `hyperloglogplus` | 0.4.1 | MIT |
| `iana-time-zone` | 0.1.65 | Apache-2.0 OR MIT |
| `iana-time-zone-haiku` | 0.1.2 | Apache-2.0 OR MIT |
| `ico` | 0.5.0 | MIT |
| `icu_collections` | 2.2.0 | Unicode-3.0 |
| `icu_locale` | 2.2.0 | Unicode-3.0 |
| `icu_locale_core` | 2.2.0 | Unicode-3.0 |
| `icu_locale_data` | 2.2.0 | Unicode-3.0 |
| `icu_normalizer` | 2.2.0 | Unicode-3.0 |
| `icu_normalizer_data` | 2.2.0 | Unicode-3.0 |
| `icu_properties` | 2.2.0 | Unicode-3.0 |
| `icu_properties_data` | 2.2.0 | Unicode-3.0 |
| `icu_provider` | 2.2.0 | Unicode-3.0 |
| `icu_segmenter` | 2.2.0 | Unicode-3.0 |
| `icu_segmenter_data` | 2.2.0 | Unicode-3.0 |
| `id-arena` | 2.3.0 | Apache-2.0 OR MIT |
| `ident_case` | 1.0.1 | Apache-2.0 OR MIT |
| `idna` | 1.1.0 | Apache-2.0 OR MIT |
| `idna_adapter` | 1.2.2 | Apache-2.0 OR MIT |
| `ignore` | 0.4.26 | MIT OR Unlicense |
| `image` | 0.25.10 | Apache-2.0 OR MIT |
| `image-webp` | 0.2.4 | Apache-2.0 OR MIT |
| `imgref` | 1.12.2 | Apache-2.0 OR CC0-1.0 |
| `indexmap` | 1.9.3 | Apache-2.0 OR MIT |
| `indexmap` | 2.14.0 | Apache-2.0 OR MIT |
| `indicatif` | 0.17.11 | MIT |
| `indicatif` | 0.18.6 | MIT |
| `infer` | 0.19.0 | MIT |
| `inferno` | 0.11.21 | CDDL-1.0 |
| `inotify` | 0.9.6 | ISC |
| `inotify` | 0.10.2 | ISC |
| `inotify-sys` | 0.1.5 | ISC |
| `inotify-sys` | 0.1.8 | ISC |
| `inout` | 0.1.4 | Apache-2.0 OR MIT |
| `instant` | 0.1.13 | BSD-3-Clause |
| `interpolate_name` | 0.2.4 | MIT |
| `io-uring` | 0.7.13 | Apache-2.0 OR MIT |
| `ipnet` | 2.12.0 | Apache-2.0 OR MIT |
| `is-docker` | 0.2.0 | MIT |
| `is-terminal` | 0.4.17 | MIT |
| `is-wsl` | 0.4.0 | MIT |
| `is_terminal_polyfill` | 1.70.2 | Apache-2.0 OR MIT |
| `itertools` | 0.13.0 | Apache-2.0 OR MIT |
| `itertools` | 0.14.0 | Apache-2.0 OR MIT |
| `itoa` | 1.0.18 | Apache-2.0 OR MIT |
| `javascriptcore-rs` | 1.1.2 | MIT |
| `javascriptcore-rs-sys` | 1.1.1 | MIT |
| `jieba-macros` | 0.10.2 | MIT |
| `jieba-rs` | 0.10.2 | MIT |
| `jiff` | 0.2.32 | MIT OR Unlicense |
| `jiff-static` | 0.2.32 | MIT OR Unlicense |
| `jiff-tzdb` | 0.1.8 | MIT OR Unlicense |
| `jiff-tzdb-platform` | 0.1.3 | MIT OR Unlicense |
| `jni` | 0.21.1 | Apache-2.0 OR MIT |
| `jni-sys` | 0.3.1 | Apache-2.0 OR MIT |
| `jni-sys` | 0.4.1 | Apache-2.0 OR MIT |
| `jni-sys-macros` | 0.4.1 | Apache-2.0 OR MIT |
| `jobserver` | 0.1.35 | Apache-2.0 OR MIT |
| `js-sys` | 0.3.99 | Apache-2.0 OR MIT |
| `js-sys` | 0.3.103 | Apache-2.0 OR MIT |
| `json-patch` | 3.0.1 | Apache-2.0 OR MIT |
| `jsonb` | 0.5.6 | Apache-2.0 |
| `jsonptr` | 0.6.3 | Apache-2.0 OR MIT |
| `kanaria` | 0.2.0 | MIT |
| `keyboard-types` | 0.7.0 | Apache-2.0 OR MIT |
| `keyring` | 3.6.3 | Apache-2.0 OR MIT |
| `kqueue` | 1.1.1 | MIT |
| `kqueue` | 1.2.0 | MIT |
| `kqueue-sys` | 1.1.2 | MIT |
| `lance` | 8.0.0 | Apache-2.0 |
| `lance-arrow` | 8.0.0 | Apache-2.0 |
| `lance-arrow-scalar` | 58.0.0 | Apache-2.0 |
| `lance-arrow-stats` | 58.0.0 | Apache-2.0 |
| `lance-bitpacking` | 8.0.0 | Apache-2.0 |
| `lance-core` | 8.0.0 | Apache-2.0 |
| `lance-datafusion` | 8.0.0 | Apache-2.0 |
| `lance-datagen` | 8.0.0 | Apache-2.0 |
| `lance-derive` | 8.0.0 | Apache-2.0 |
| `lance-encoding` | 8.0.0 | Apache-2.0 |
| `lance-file` | 8.0.0 | Apache-2.0 |
| `lance-index` | 8.0.0 | Apache-2.0 |
| `lance-io` | 8.0.0 | Apache-2.0 |
| `lance-linalg` | 8.0.0 | Apache-2.0 |
| `lance-namespace` | 8.0.0 | Apache-2.0 |
| `lance-namespace-impls` | 8.0.0 | Apache-2.0 |
| `lance-namespace-reqwest-client` | 0.8.6 | Apache-2.0 |
| `lance-select` | 8.0.0 | Apache-2.0 |
| `lance-table` | 8.0.0 | Apache-2.0 |
| `lance-testing` | 8.0.0 | Apache-2.0 |
| `lance-tokenizer` | 8.0.0 | Apache-2.0 |
| `lancedb` | 0.31.0 | Apache-2.0 |
| `lazy_static` | 1.5.0 | Apache-2.0 OR MIT |
| `leb128fmt` | 0.1.0 | Apache-2.0 OR MIT |
| `lebe` | 0.5.3 | BSD-3-Clause |
| `lexical-core` | 1.0.6 | Apache-2.0 OR MIT |
| `lexical-parse-float` | 1.0.6 | Apache-2.0 OR MIT |
| `lexical-parse-integer` | 1.0.6 | Apache-2.0 OR MIT |
| `lexical-util` | 1.0.7 | Apache-2.0 OR MIT |
| `lexical-write-float` | 1.0.6 | Apache-2.0 OR MIT |
| `lexical-write-integer` | 1.0.6 | Apache-2.0 OR MIT |
| `libappindicator` | 0.9.0 | Apache-2.0 OR MIT |
| `libappindicator-sys` | 0.9.0 | Apache-2.0 OR MIT |
| `libc` | 0.2.186 | Apache-2.0 OR MIT |
| `libdbus-sys` | 0.2.7 | Apache-2.0 OR MIT |
| `libfuzzer-sys` | 0.4.13 | (Apache-2.0 OR MIT) AND NCSA |
| `libloading` | 0.7.4 | ISC |
| `libloading` | 0.8.9 | ISC |
| `libm` | 0.2.16 | MIT |
| `libredox` | 0.1.16 | MIT |
| `libredox` | 0.1.18 | MIT |
| `libsais-rs` | 0.2.0 | Apache-2.0 |
| `libsqlite3-sys` | 0.30.1 | MIT |
| `lindera` | 3.0.7 | MIT |
| `lindera-dictionary` | 3.0.7 | MIT |
| `linux-raw-sys` | 0.12.1 | Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT |
| `litemap` | 0.8.2 | Unicode-3.0 |
| `litrs` | 1.0.0 | Apache-2.0 OR MIT |
| `lock_api` | 0.4.14 | Apache-2.0 OR MIT |
| `log` | 0.4.30 | Apache-2.0 OR MIT |
| `log` | 0.4.33 | Apache-2.0 OR MIT |
| `loom` | 0.7.2 | MIT |
| `loop9` | 0.1.5 | MIT |
| `lru-slab` | 0.1.2 | Apache-2.0 OR MIT OR Zlib |
| `lz4` | 1.28.1 | MIT |
| `lz4-sys` | 1.11.1+lz4-1.10.0 | MIT |
| `lz4_flex` | 0.13.1 | MIT |
| `lzma-rust2` | 0.15.8 | Apache-2.0 |
| `mac-notification-sys` | 0.6.12 | Apache-2.0 OR MIT |
| `macro_rules_attribute` | 0.2.2 | Apache-2.0 OR MIT OR Zlib |
| `macro_rules_attribute-proc_macro` | 0.2.2 | Apache-2.0 OR MIT OR Zlib |
| `malloc_buf` | 0.0.6 | MIT |
| `markup5ever` | 0.38.0 | Apache-2.0 OR MIT |
| `matchers` | 0.2.0 | MIT |
| `matchit` | 0.8.4 | BSD-3-Clause AND MIT |
| `matrixmultiply` | 0.3.10 | Apache-2.0 OR MIT |
| `maybe-rayon` | 0.1.1 | MIT |
| `md-5` | 0.10.6 | Apache-2.0 OR MIT |
| `memchr` | 2.8.1 | MIT OR Unlicense |
| `memchr` | 2.8.3 | MIT OR Unlicense |
| `memmap2` | 0.9.11 | Apache-2.0 OR MIT |
| `memoffset` | 0.9.1 | MIT |
| `metal` | 0.29.0 | Apache-2.0 OR MIT |
| `mime` | 0.3.17 | Apache-2.0 OR MIT |
| `mime_guess` | 2.0.5 | MIT |
| `minimal-lexical` | 0.2.1 | Apache-2.0 OR MIT |
| `miniz_oxide` | 0.8.9 | Apache-2.0 OR MIT OR Zlib |
| `mio` | 0.8.11 | MIT |
| `mio` | 1.2.0 | MIT |
| `mio` | 1.2.1 | MIT |
| `mio` | 1.2.2 | MIT |
| `moka` | 0.12.15 | (Apache-2.0 OR MIT) AND Apache-2.0 |
| `monostate` | 0.1.18 | Apache-2.0 OR MIT |
| `monostate-impl` | 0.1.18 | Apache-2.0 OR MIT |
| `moxcms` | 0.8.1 | Apache-2.0 OR BSD-3-Clause |
| `muda` | 0.19.2 | Apache-2.0 OR MIT |
| `multimap` | 0.10.1 | Apache-2.0 OR MIT |
| `munge` | 0.4.7 | MIT |
| `munge_macro` | 0.4.7 | MIT |
| `native-tls` | 0.2.18 | Apache-2.0 OR MIT |
| `ndarray` | 0.16.1 | Apache-2.0 OR MIT |
| `ndarray` | 0.17.2 | Apache-2.0 OR MIT |
| `ndk` | 0.9.0 | Apache-2.0 OR MIT |
| `ndk-sys` | 0.6.0+11769913 | Apache-2.0 OR MIT |
| `new_debug_unreachable` | 1.0.6 | MIT |
| `nix` | 0.26.4 | MIT |
| `nix` | 0.28.0 | MIT |
| `no_std_io2` | 0.9.4 | Apache-2.0 OR MIT |
| `nom` | 7.1.3 | MIT |
| `nom` | 8.0.0 | MIT |
| `noop_proc_macro` | 0.3.0 | MIT |
| `notify` | 6.1.1 | CC0-1.0 |
| `notify` | 7.0.0 | CC0-1.0 |
| `notify-rust` | 4.17.0 | Apache-2.0 OR MIT |
| `notify-types` | 1.0.1 | Apache-2.0 OR MIT |
| `ntapi` | 0.4.3 | Apache-2.0 OR MIT |
| `nu-ansi-term` | 0.50.3 | MIT |
| `num` | 0.4.3 | Apache-2.0 OR MIT |
| `num-bigint` | 0.4.8 | Apache-2.0 OR MIT |
| `num-complex` | 0.4.6 | Apache-2.0 OR MIT |
| `num-conv` | 0.2.2 | Apache-2.0 OR MIT |
| `num-derive` | 0.4.2 | Apache-2.0 OR MIT |
| `num-format` | 0.4.4 | Apache-2.0 OR MIT |
| `num-integer` | 0.1.46 | Apache-2.0 OR MIT |
| `num-iter` | 0.1.46 | Apache-2.0 OR MIT |
| `num-rational` | 0.4.2 | Apache-2.0 OR MIT |
| `num-traits` | 0.2.19 | Apache-2.0 OR MIT |
| `num_cpus` | 1.17.0 | Apache-2.0 OR MIT |
| `num_enum` | 0.7.6 | Apache-2.0 OR BSD-3-Clause OR MIT |
| `num_enum_derive` | 0.7.6 | Apache-2.0 OR BSD-3-Clause OR MIT |
| `number_prefix` | 0.4.0 | MIT |
| `objc` | 0.2.7 | MIT |
| `objc-sys` | 0.3.5 | MIT |
| `objc2` | 0.5.2 | MIT |
| `objc2` | 0.6.4 | MIT |
| `objc2-app-kit` | 0.3.2 | Apache-2.0 OR MIT OR Zlib |
| `objc2-cloud-kit` | 0.3.2 | Apache-2.0 OR MIT OR Zlib |
| `objc2-core-data` | 0.3.2 | Apache-2.0 OR MIT OR Zlib |
| `objc2-core-foundation` | 0.3.2 | Apache-2.0 OR MIT OR Zlib |
| `objc2-core-graphics` | 0.3.2 | Apache-2.0 OR MIT OR Zlib |
| `objc2-core-image` | 0.3.2 | Apache-2.0 OR MIT OR Zlib |
| `objc2-core-location` | 0.3.2 | Apache-2.0 OR MIT OR Zlib |
| `objc2-core-text` | 0.3.2 | Apache-2.0 OR MIT OR Zlib |
| `objc2-encode` | 4.1.0 | MIT |
| `objc2-exception-helper` | 0.1.1 | Apache-2.0 OR MIT OR Zlib |
| `objc2-foundation` | 0.2.2 | MIT |
| `objc2-foundation` | 0.3.2 | MIT |
| `objc2-io-surface` | 0.3.2 | Apache-2.0 OR MIT OR Zlib |
| `objc2-local-authentication` | 0.2.2 | MIT |
| `objc2-metal` | 0.3.2 | Apache-2.0 OR MIT OR Zlib |
| `objc2-quartz-core` | 0.3.2 | Apache-2.0 OR MIT OR Zlib |
| `objc2-ui-kit` | 0.3.2 | Apache-2.0 OR MIT OR Zlib |
| `objc2-user-notifications` | 0.3.2 | Apache-2.0 OR MIT OR Zlib |
| `objc2-web-kit` | 0.3.2 | Apache-2.0 OR MIT OR Zlib |
| `object` | 0.37.3 | Apache-2.0 OR MIT |
| `object_store` | 0.13.2 | Apache-2.0 OR MIT |
| `once_cell` | 1.21.4 | Apache-2.0 OR MIT |
| `once_cell_polyfill` | 1.70.2 | Apache-2.0 OR MIT |
| `onig` | 6.5.3 | MIT |
| `onig_sys` | 69.9.3 | MIT |
| `oorandom` | 11.1.5 | MIT |
| `opaque-debug` | 0.3.1 | Apache-2.0 OR MIT |
| `open` | 5.3.5 | MIT |
| `openssl` | 0.10.81 | Apache-2.0 |
| `openssl-macros` | 0.1.1 | Apache-2.0 OR MIT |
| `openssl-probe` | 0.2.1 | Apache-2.0 OR MIT |
| `openssl-sys` | 0.9.117 | MIT |
| `option-ext` | 0.2.0 | MPL-2.0 |
| `ordered-float` | 5.3.0 | MIT |
| `ordered-stream` | 0.2.0 | Apache-2.0 OR MIT |
| `ort` | 2.0.0-rc.12 | Apache-2.0 OR MIT |
| `ort-sys` | 2.0.0-rc.12 | Apache-2.0 OR MIT |
| `page_size` | 0.6.0 | Apache-2.0 OR MIT |
| `pango` | 0.18.3 | MIT |
| `pango-sys` | 0.18.0 | MIT |
| `parking` | 2.2.1 | Apache-2.0 OR MIT |
| `parking_lot` | 0.12.5 | Apache-2.0 OR MIT |
| `parking_lot_core` | 0.9.12 | Apache-2.0 OR MIT |
| `paste` | 1.0.15 | Apache-2.0 OR MIT |
| `pastey` | 0.1.1 | Apache-2.0 OR MIT |
| `path_abs` | 0.5.1 | Apache-2.0 OR MIT |
| `pathdiff` | 0.2.3 | Apache-2.0 OR MIT |
| `pem-rfc7468` | 1.0.0 | Apache-2.0 OR MIT |
| `percent-encoding` | 2.3.2 | Apache-2.0 OR MIT |
| `permutation` | 0.4.1 | Apache-2.0 OR MIT |
| `petgraph` | 0.8.3 | Apache-2.0 OR MIT |
| `phf` | 0.12.1 | MIT |
| `phf` | 0.13.1 | MIT |
| `phf_codegen` | 0.13.1 | MIT |
| `phf_generator` | 0.13.1 | MIT |
| `phf_macros` | 0.13.1 | MIT |
| `phf_shared` | 0.12.1 | MIT |
| `phf_shared` | 0.13.1 | MIT |
| `pin-project` | 1.1.13 | Apache-2.0 OR MIT |
| `pin-project-internal` | 1.1.13 | Apache-2.0 OR MIT |
| `pin-project-lite` | 0.2.17 | Apache-2.0 OR MIT |
| `piper` | 0.2.5 | Apache-2.0 OR MIT |
| `pkcs8` | 0.10.2 | Apache-2.0 OR MIT |
| `pkg-config` | 0.3.33 | Apache-2.0 OR MIT |
| `plist` | 1.9.0 | MIT |
| `plotters` | 0.3.7 | MIT |
| `plotters-backend` | 0.3.7 | MIT |
| `plotters-svg` | 0.3.7 | MIT |
| `png` | 0.17.16 | Apache-2.0 OR MIT |
| `png` | 0.18.1 | Apache-2.0 OR MIT |
| `polling` | 3.11.0 | Apache-2.0 OR MIT |
| `polyval` | 0.6.2 | Apache-2.0 OR MIT |
| `portable-atomic` | 1.13.1 | Apache-2.0 OR MIT |
| `portable-atomic-util` | 0.2.7 | Apache-2.0 OR MIT |
| `portable-pty` | 0.9.0 | MIT |
| `potential_utf` | 0.1.5 | Unicode-3.0 |
| `powerfmt` | 0.2.0 | Apache-2.0 OR MIT |
| `pprof` | 0.15.0 | Apache-2.0 |
| `ppv-lite86` | 0.2.21 | Apache-2.0 OR MIT |
| `precomputed-hash` | 0.1.1 | MIT |
| `prettyplease` | 0.2.37 | Apache-2.0 OR MIT |
| `proc-macro-crate` | 1.3.1 | Apache-2.0 OR MIT |
| `proc-macro-crate` | 2.0.2 | Apache-2.0 OR MIT |
| `proc-macro-crate` | 3.5.0 | Apache-2.0 OR MIT |
| `proc-macro-error` | 1.0.4 | Apache-2.0 OR MIT |
| `proc-macro-error-attr` | 1.0.4 | Apache-2.0 OR MIT |
| `proc-macro2` | 1.0.106 | Apache-2.0 OR MIT |
| `proc-macro2` | 1.0.107 | Apache-2.0 OR MIT |
| `profiling` | 1.0.18 | Apache-2.0 OR MIT |
| `profiling-procmacros` | 1.0.18 | Apache-2.0 OR MIT |
| `prost` | 0.14.4 | Apache-2.0 |
| `prost-build` | 0.14.4 | Apache-2.0 |
| `prost-derive` | 0.14.4 | Apache-2.0 |
| `prost-types` | 0.14.4 | Apache-2.0 |
| `ptr_meta` | 0.3.1 | MIT |
| `ptr_meta_derive` | 0.3.1 | MIT |
| `pulp` | 0.21.5 | MIT |
| `pulp` | 0.22.3 | MIT |
| `pulp-wasm-simd-flag` | 0.1.1 | MIT |
| `pxfm` | 0.1.30 | Apache-2.0 OR BSD-3-Clause |
| `qoi` | 0.4.1 | Apache-2.0 OR MIT |
| `quick-error` | 2.0.1 | Apache-2.0 OR MIT |
| `quick-xml` | 0.26.0 | MIT |
| `quick-xml` | 0.37.5 | MIT |
| `quick-xml` | 0.39.4 | MIT |
| `quinn` | 0.11.9 | Apache-2.0 OR MIT |
| `quinn` | 0.11.11 | Apache-2.0 OR MIT |
| `quinn-proto` | 0.11.14 | Apache-2.0 OR MIT |
| `quinn-proto` | 0.11.16 | Apache-2.0 OR MIT |
| `quinn-udp` | 0.5.14 | Apache-2.0 OR MIT |
| `quinn-udp` | 0.5.15 | Apache-2.0 OR MIT |
| `quote` | 1.0.45 | Apache-2.0 OR MIT |
| `quote` | 1.0.46 | Apache-2.0 OR MIT |
| `quote` | 1.0.47 | Apache-2.0 OR MIT |
| `r-efi` | 5.3.0 | Apache-2.0 OR LGPL-2.1-or-later OR MIT |
| `r-efi` | 6.0.0 | Apache-2.0 OR LGPL-2.1-or-later OR MIT |
| `radium` | 0.7.0 | MIT |
| `rancor` | 0.1.2 | MIT |
| `rand` | 0.9.4 | Apache-2.0 OR MIT |
| `rand` | 0.9.5 | Apache-2.0 OR MIT |
| `rand` | 0.10.2 | Apache-2.0 OR MIT |
| `rand_chacha` | 0.9.0 | Apache-2.0 OR MIT |
| `rand_core` | 0.6.4 | Apache-2.0 OR MIT |
| `rand_core` | 0.9.5 | Apache-2.0 OR MIT |
| `rand_core` | 0.10.1 | Apache-2.0 OR MIT |
| `rand_distr` | 0.5.1 | Apache-2.0 OR MIT |
| `rand_pcg` | 0.10.2 | Apache-2.0 OR MIT |
| `rand_xoshiro` | 0.7.0 | Apache-2.0 OR MIT |
| `rangemap` | 1.7.1 | Apache-2.0 OR MIT |
| `rav1e` | 0.8.1 | BSD-2-Clause |
| `ravif` | 0.13.0 | BSD-3-Clause |
| `raw-cpuid` | 11.6.0 | MIT |
| `raw-window-handle` | 0.6.2 | Apache-2.0 OR MIT OR Zlib |
| `rawpointer` | 0.2.1 | Apache-2.0 OR MIT |
| `rayon` | 1.12.0 | Apache-2.0 OR MIT |
| `rayon-cond` | 0.4.0 | Apache-2.0 OR MIT |
| `rayon-core` | 1.13.0 | Apache-2.0 OR MIT |
| `reborrow` | 0.5.5 | MIT |
| `redox_syscall` | 0.5.18 | MIT |
| `redox_users` | 0.5.2 | MIT |
| `ref-cast` | 1.0.25 | Apache-2.0 OR MIT |
| `ref-cast` | 1.0.26 | Apache-2.0 OR MIT |
| `ref-cast-impl` | 1.0.25 | Apache-2.0 OR MIT |
| `ref-cast-impl` | 1.0.26 | Apache-2.0 OR MIT |
| `regex` | 1.12.3 | Apache-2.0 OR MIT |
| `regex` | 1.13.0 | Apache-2.0 OR MIT |
| `regex` | 1.13.1 | Apache-2.0 OR MIT |
| `regex-automata` | 0.4.14 | Apache-2.0 OR MIT |
| `regex-automata` | 0.4.15 | Apache-2.0 OR MIT |
| `regex-automata` | 0.4.16 | Apache-2.0 OR MIT |
| `regex-syntax` | 0.8.10 | Apache-2.0 OR MIT |
| `regex-syntax` | 0.8.11 | Apache-2.0 OR MIT |
| `rend` | 0.5.4 | MIT |
| `reqwest` | 0.12.28 | Apache-2.0 OR MIT |
| `reqwest` | 0.13.4 | Apache-2.0 OR MIT |
| `rfd` | 0.16.0 | MIT |
| `rgb` | 0.8.53 | MIT |
| `ring` | 0.17.14 | Apache-2.0 AND ISC |
| `rkyv` | 0.8.17 | MIT |
| `rkyv_derive` | 0.8.17 | MIT |
| `rmcp` | 0.7.0 | MIT |
| `rmcp-macros` | 0.7.0 | MIT |
| `roaring` | 0.11.4 | Apache-2.0 OR MIT |
| `rusqlite` | 0.32.1 | MIT |
| `rust-stemmers` | 1.2.0 | BSD-3-Clause OR MIT |
| `rustc-demangle` | 0.1.28 | Apache-2.0 OR MIT |
| `rustc-hash` | 2.1.2 | Apache-2.0 OR MIT |
| `rustc-hash` | 2.1.3 | Apache-2.0 OR MIT |
| `rustc_version` | 0.4.1 | Apache-2.0 OR MIT |
| `rustix` | 1.1.4 | Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT |
| `rustls` | 0.23.40 | Apache-2.0 OR ISC OR MIT |
| `rustls` | 0.23.41 | Apache-2.0 OR ISC OR MIT |
| `rustls` | 0.23.42 | Apache-2.0 OR ISC OR MIT |
| `rustls-native-certs` | 0.8.4 | Apache-2.0 OR ISC OR MIT |
| `rustls-pki-types` | 1.14.1 | Apache-2.0 OR MIT |
| `rustls-pki-types` | 1.15.0 | Apache-2.0 OR MIT |
| `rustls-webpki` | 0.103.13 | ISC |
| `rustversion` | 1.0.22 | Apache-2.0 OR MIT |
| `rustversion` | 1.0.23 | Apache-2.0 OR MIT |
| `ryu` | 1.0.23 | Apache-2.0 OR BSL-1.0 |
| `safetensors` | 0.4.5 | Apache-2.0 |
| `safetensors` | 0.7.0 | Apache-2.0 |
| `safetensors` | 0.8.0 | Apache-2.0 |
| `same-file` | 1.0.6 | MIT OR Unlicense |
| `schannel` | 0.1.29 | MIT |
| `schemars` | 0.8.22 | MIT |
| `schemars` | 0.9.0 | MIT |
| `schemars` | 1.2.1 | MIT |
| `schemars_derive` | 0.8.22 | MIT |
| `schemars_derive` | 1.2.1 | MIT |
| `scoped-tls` | 1.0.1 | Apache-2.0 OR MIT |
| `scopeguard` | 1.2.0 | Apache-2.0 OR MIT |
| `security-framework` | 2.11.1 | Apache-2.0 OR MIT |
| `security-framework` | 3.7.0 | Apache-2.0 OR MIT |
| `security-framework-sys` | 2.17.0 | Apache-2.0 OR MIT |
| `selectors` | 0.36.1 | MPL-2.0 |
| `semver` | 1.0.28 | Apache-2.0 OR MIT |
| `seq-macro` | 0.3.6 | Apache-2.0 OR MIT |
| `serde` | 1.0.228 | Apache-2.0 OR MIT |
| `serde` | 1.0.229 | Apache-2.0 OR MIT |
| `serde-untagged` | 0.1.9 | Apache-2.0 OR MIT |
| `serde_core` | 1.0.228 | Apache-2.0 OR MIT |
| `serde_core` | 1.0.229 | Apache-2.0 OR MIT |
| `serde_derive` | 1.0.228 | Apache-2.0 OR MIT |
| `serde_derive` | 1.0.229 | Apache-2.0 OR MIT |
| `serde_derive_internals` | 0.29.1 | Apache-2.0 OR MIT |
| `serde_json` | 1.0.150 | Apache-2.0 OR MIT |
| `serde_json` | 1.0.151 | Apache-2.0 OR MIT |
| `serde_path_to_error` | 0.1.20 | Apache-2.0 OR MIT |
| `serde_repr` | 0.1.20 | Apache-2.0 OR MIT |
| `serde_spanned` | 0.6.9 | Apache-2.0 OR MIT |
| `serde_spanned` | 1.1.1 | Apache-2.0 OR MIT |
| `serde_urlencoded` | 0.7.1 | Apache-2.0 OR MIT |
| `serde_with` | 3.20.0 | Apache-2.0 OR MIT |
| `serde_with` | 3.21.0 | Apache-2.0 OR MIT |
| `serde_with_macros` | 3.20.0 | Apache-2.0 OR MIT |
| `serde_with_macros` | 3.21.0 | Apache-2.0 OR MIT |
| `serde_yaml_ng` | 0.10.0 | MIT |
| `serial2` | 0.2.37 | Apache-2.0 OR BSD-2-Clause |
| `serialize-to-javascript` | 0.1.2 | Apache-2.0 OR MIT |
| `serialize-to-javascript-impl` | 0.1.2 | Apache-2.0 OR MIT |
| `servo_arc` | 0.4.3 | Apache-2.0 OR MIT |
| `sha1_smol` | 1.0.1 | BSD-3-Clause |
| `sha2` | 0.10.9 | Apache-2.0 OR MIT |
| `sharded-slab` | 0.1.7 | MIT |
| `shared_library` | 0.1.9 | Apache-2.0 OR MIT |
| `shell-words` | 1.1.1 | Apache-2.0 OR MIT |
| `shlex` | 1.3.0 | Apache-2.0 OR MIT |
| `shlex` | 2.0.1 | Apache-2.0 OR MIT |
| `signal-hook-registry` | 1.4.8 | Apache-2.0 OR MIT |
| `signature` | 2.2.0 | Apache-2.0 OR MIT |
| `simd-adler32` | 0.3.9 | MIT |
| `simd_helpers` | 0.1.0 | MIT |
| `simdutf8` | 0.1.5 | Apache-2.0 OR MIT |
| `similar` | 2.7.0 | Apache-2.0 |
| `siphasher` | 1.0.3 | Apache-2.0 OR MIT |
| `slab` | 0.4.12 | MIT |
| `smallvec` | 1.15.1 | Apache-2.0 OR MIT |
| `smallvec` | 1.15.2 | Apache-2.0 OR MIT |
| `snafu` | 0.8.9 | Apache-2.0 OR MIT |
| `snafu` | 0.9.1 | Apache-2.0 OR MIT |
| `snafu-derive` | 0.8.9 | Apache-2.0 OR MIT |
| `snafu-derive` | 0.9.1 | Apache-2.0 OR MIT |
| `socket2` | 0.6.3 | Apache-2.0 OR MIT |
| `socket2` | 0.6.4 | Apache-2.0 OR MIT |
| `socket2` | 0.6.5 | Apache-2.0 OR MIT |
| `socks` | 0.3.4 | Apache-2.0 OR MIT |
| `softbuffer` | 0.4.8 | Apache-2.0 OR MIT |
| `soup3` | 0.5.0 | MIT |
| `soup3-sys` | 0.5.0 | MIT |
| `spin` | 0.10.0 | MIT |
| `spki` | 0.7.3 | Apache-2.0 OR MIT |
| `spm_precompiled` | 0.1.4 | Apache-2.0 |
| `sqlparser` | 0.61.0 | Apache-2.0 |
| `sqlparser_derive` | 0.5.0 | Apache-2.0 |
| `stable_deref_trait` | 1.2.1 | Apache-2.0 OR MIT |
| `static_assertions` | 1.1.0 | Apache-2.0 OR MIT |
| `std_prelude` | 0.2.12 | MIT |
| `stfu8` | 0.2.7 | Apache-2.0 OR MIT |
| `stop-words` | 0.10.0 | Apache-2.0 OR MIT |
| `str_stack` | 0.1.1 | Apache-2.0 OR MIT |
| `streaming-iterator` | 0.1.9 | Apache-2.0 OR MIT |
| `string_cache` | 0.9.0 | Apache-2.0 OR MIT |
| `string_cache_codegen` | 0.6.1 | Apache-2.0 OR MIT |
| `strsim` | 0.11.1 | MIT |
| `strum` | 0.26.3 | MIT |
| `strum` | 0.28.0 | MIT |
| `strum_macros` | 0.26.4 | MIT |
| `strum_macros` | 0.28.0 | MIT |
| `subtle` | 2.6.1 | BSD-3-Clause |
| `swift-rs` | 1.0.7 | Apache-2.0 OR MIT |
| `symbolic-common` | 12.18.3 | MIT |
| `symbolic-demangle` | 12.18.3 | MIT |
| `syn` | 1.0.109 | Apache-2.0 OR MIT |
| `syn` | 2.0.117 | Apache-2.0 OR MIT |
| `syn` | 2.0.118 | Apache-2.0 OR MIT |
| `syn` | 2.0.119 | Apache-2.0 OR MIT |
| `syn` | 3.0.2 | Apache-2.0 OR MIT |
| `sync_wrapper` | 1.0.2 | Apache-2.0 |
| `synstructure` | 0.13.2 | MIT |
| `sysctl` | 0.6.0 | MIT |
| `sysinfo` | 0.33.1 | MIT |
| `system-configuration` | 0.7.0 | Apache-2.0 OR MIT |
| `system-configuration-sys` | 0.6.0 | Apache-2.0 OR MIT |
| `system-deps` | 6.2.2 | Apache-2.0 OR MIT |
| `tagptr` | 0.2.0 | Apache-2.0 OR MIT |
| `tao` | 0.35.3 | Apache-2.0 |
| `tao-macros` | 0.1.3 | Apache-2.0 OR MIT |
| `tap` | 1.0.1 | MIT |
| `tar` | 0.4.46 | Apache-2.0 OR MIT |
| `target-lexicon` | 0.12.16 | Apache-2.0 WITH LLVM-exception |
| `tauri` | 2.11.2 | Apache-2.0 OR MIT |
| `tauri-build` | 2.6.2 | Apache-2.0 OR MIT |
| `tauri-codegen` | 2.6.2 | Apache-2.0 OR MIT |
| `tauri-macros` | 2.6.2 | Apache-2.0 OR MIT |
| `tauri-plugin` | 2.6.2 | Apache-2.0 OR MIT |
| `tauri-plugin-dialog` | 2.7.1 | Apache-2.0 OR MIT |
| `tauri-plugin-fs` | 2.5.1 | Apache-2.0 OR MIT |
| `tauri-plugin-notification` | 2.3.3 | Apache-2.0 OR MIT |
| `tauri-runtime` | 2.11.2 | Apache-2.0 OR MIT |
| `tauri-runtime-wry` | 2.11.2 | Apache-2.0 OR MIT |
| `tauri-utils` | 2.9.2 | Apache-2.0 OR MIT |
| `tauri-winres` | 0.3.6 | MIT |
| `tauri-winrt-notification` | 0.7.2 | Apache-2.0 OR MIT |
| `tempfile` | 3.27.0 | Apache-2.0 OR MIT |
| `tendril` | 0.5.0 | Apache-2.0 OR MIT |
| `thiserror` | 1.0.69 | Apache-2.0 OR MIT |
| `thiserror` | 2.0.18 | Apache-2.0 OR MIT |
| `thiserror` | 2.0.19 | Apache-2.0 OR MIT |
| `thiserror-impl` | 1.0.69 | Apache-2.0 OR MIT |
| `thiserror-impl` | 2.0.18 | Apache-2.0 OR MIT |
| `thiserror-impl` | 2.0.19 | Apache-2.0 OR MIT |
| `thread-tree` | 0.3.3 | Apache-2.0 OR MIT |
| `thread_local` | 1.1.10 | Apache-2.0 OR MIT |
| `tiff` | 0.11.3 | MIT |
| `time` | 0.3.47 | Apache-2.0 OR MIT |
| `time-core` | 0.1.8 | Apache-2.0 OR MIT |
| `time-macros` | 0.2.27 | Apache-2.0 OR MIT |
| `tiny-keccak` | 2.0.2 | CC0-1.0 |
| `tinystr` | 0.8.3 | Unicode-3.0 |
| `tinytemplate` | 1.2.1 | Apache-2.0 OR MIT |
| `tinyvec` | 1.11.0 | Apache-2.0 OR MIT OR Zlib |
| `tinyvec` | 1.12.0 | Apache-2.0 OR MIT OR Zlib |
| `tinyvec_macros` | 0.1.1 | Apache-2.0 OR MIT OR Zlib |
| `tokenizers` | 0.21.4 | Apache-2.0 |
| `tokenizers` | 0.22.2 | Apache-2.0 |
| `tokio` | 1.52.3 | MIT |
| `tokio` | 1.53.1 | MIT |
| `tokio-macros` | 2.7.0 | MIT |
| `tokio-macros` | 2.7.1 | MIT |
| `tokio-native-tls` | 0.3.1 | MIT |
| `tokio-rustls` | 0.26.4 | Apache-2.0 OR MIT |
| `tokio-stream` | 0.1.18 | MIT |
| `tokio-util` | 0.7.18 | MIT |
| `toml` | 0.8.2 | Apache-2.0 OR MIT |
| `toml` | 0.9.12+spec-1.1.0 | Apache-2.0 OR MIT |
| `toml` | 1.1.2+spec-1.1.0 | Apache-2.0 OR MIT |
| `toml_datetime` | 0.6.3 | Apache-2.0 OR MIT |
| `toml_datetime` | 0.7.5+spec-1.1.0 | Apache-2.0 OR MIT |
| `toml_datetime` | 1.1.1+spec-1.1.0 | Apache-2.0 OR MIT |
| `toml_edit` | 0.19.15 | Apache-2.0 OR MIT |
| `toml_edit` | 0.20.2 | Apache-2.0 OR MIT |
| `toml_edit` | 0.25.11+spec-1.1.0 | Apache-2.0 OR MIT |
| `toml_parser` | 1.1.2+spec-1.1.0 | Apache-2.0 OR MIT |
| `toml_writer` | 1.1.1+spec-1.1.0 | Apache-2.0 OR MIT |
| `tower` | 0.5.3 | MIT |
| `tower-http` | 0.6.11 | MIT |
| `tower-layer` | 0.3.3 | MIT |
| `tower-service` | 0.3.3 | MIT |
| `tracing` | 0.1.44 | MIT |
| `tracing-attributes` | 0.1.31 | MIT |
| `tracing-core` | 0.1.36 | MIT |
| `tracing-log` | 0.2.0 | MIT |
| `tracing-subscriber` | 0.3.23 | MIT |
| `tray-icon` | 0.23.1 | Apache-2.0 OR MIT |
| `tree-sitter` | 0.25.10 | MIT |
| `tree-sitter-cpp` | 0.23.4 | MIT |
| `tree-sitter-go` | 0.25.0 | MIT |
| `tree-sitter-html` | 0.23.2 | MIT |
| `tree-sitter-kotlin-ng` | 1.1.0 | MIT |
| `tree-sitter-language` | 0.1.7 | MIT |
| `tree-sitter-python` | 0.25.0 | MIT |
| `tree-sitter-rust` | 0.24.2 | MIT |
| `tree-sitter-typescript` | 0.23.2 | MIT |
| `try-lock` | 0.2.5 | MIT |
| `twox-hash` | 2.1.2 | MIT |
| `typed-path` | 0.12.3 | Apache-2.0 OR MIT |
| `typeid` | 1.0.3 | Apache-2.0 OR MIT |
| `typenum` | 1.20.0 | Apache-2.0 OR MIT |
| `typenum` | 1.20.1 | Apache-2.0 OR MIT |
| `uds_windows` | 1.2.1 | MIT |
| `ug` | 0.5.0 | Apache-2.0 OR MIT |
| `ug-metal` | 0.5.0 | Apache-2.0 OR MIT |
| `unic-char-property` | 0.9.0 | Apache-2.0 OR MIT |
| `unic-char-range` | 0.9.0 | Apache-2.0 OR MIT |
| `unic-common` | 0.9.0 | Apache-2.0 OR MIT |
| `unic-ucd-ident` | 0.9.0 | Apache-2.0 OR MIT |
| `unic-ucd-version` | 0.9.0 | Apache-2.0 OR MIT |
| `unicase` | 2.9.0 | Apache-2.0 OR MIT |
| `unicode-blocks` | 0.1.9 | MIT |
| `unicode-ident` | 1.0.24 | (Apache-2.0 OR MIT) AND Unicode-3.0 |
| `unicode-normalization` | 0.1.25 | Apache-2.0 OR MIT |
| `unicode-normalization-alignments` | 0.1.12 | Apache-2.0 OR MIT |
| `unicode-segmentation` | 1.13.2 | Apache-2.0 OR MIT |
| `unicode-segmentation` | 1.13.3 | Apache-2.0 OR MIT |
| `unicode-width` | 0.2.2 | Apache-2.0 OR MIT |
| `unicode-xid` | 0.2.6 | Apache-2.0 OR MIT |
| `unicode_categories` | 0.1.1 | Apache-2.0 OR MIT |
| `unit-prefix` | 0.5.2 | MIT |
| `universal-hash` | 0.5.1 | Apache-2.0 OR MIT |
| `unsafe-libyaml` | 0.2.11 | MIT |
| `untrusted` | 0.9.0 | ISC |
| `ureq` | 3.3.0 | Apache-2.0 OR MIT |
| `ureq-proto` | 0.6.0 | Apache-2.0 OR MIT |
| `url` | 2.5.8 | Apache-2.0 OR MIT |
| `urlencoding` | 2.1.3 | MIT |
| `urlpattern` | 0.3.0 | MIT |
| `utf-8` | 0.7.6 | Apache-2.0 OR MIT |
| `utf8-ranges` | 1.0.5 | MIT OR Unlicense |
| `utf8-zero` | 0.8.1 | Apache-2.0 OR MIT |
| `utf8_iter` | 1.0.4 | Apache-2.0 OR MIT |
| `utf8parse` | 0.2.2 | Apache-2.0 OR MIT |
| `uuid` | 1.23.1 | Apache-2.0 OR MIT |
| `uuid` | 1.23.4 | Apache-2.0 OR MIT |
| `uuid` | 1.24.0 | Apache-2.0 OR MIT |
| `v_frame` | 0.3.9 | BSD-2-Clause |
| `valuable` | 0.1.1 | MIT |
| `vcpkg` | 0.2.15 | Apache-2.0 OR MIT |
| `version-compare` | 0.2.1 | MIT |
| `version_check` | 0.9.5 | Apache-2.0 OR MIT |
| `vswhom` | 0.1.0 | MIT |
| `vswhom-sys` | 0.1.3 | MIT |
| `walkdir` | 2.5.0 | MIT OR Unlicense |
| `want` | 0.3.1 | MIT |
| `wasi` | 0.11.1+wasi-snapshot-preview1 | Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT |
| `wasip2` | 1.0.3+wasi-0.2.9 | Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT |
| `wasip2` | 1.0.4+wasi-0.2.12 | Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT |
| `wasip3` | 0.4.0+wasi-0.3.0-rc-2026-01-06 | Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT |
| `wasm-bindgen` | 0.2.122 | Apache-2.0 OR MIT |
| `wasm-bindgen` | 0.2.126 | Apache-2.0 OR MIT |
| `wasm-bindgen-futures` | 0.4.72 | Apache-2.0 OR MIT |
| `wasm-bindgen-futures` | 0.4.76 | Apache-2.0 OR MIT |
| `wasm-bindgen-macro` | 0.2.122 | Apache-2.0 OR MIT |
| `wasm-bindgen-macro` | 0.2.126 | Apache-2.0 OR MIT |
| `wasm-bindgen-macro-support` | 0.2.122 | Apache-2.0 OR MIT |
| `wasm-bindgen-macro-support` | 0.2.126 | Apache-2.0 OR MIT |
| `wasm-bindgen-shared` | 0.2.122 | Apache-2.0 OR MIT |
| `wasm-bindgen-shared` | 0.2.126 | Apache-2.0 OR MIT |
| `wasm-encoder` | 0.244.0 | Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT |
| `wasm-metadata` | 0.244.0 | Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT |
| `wasm-streams` | 0.4.2 | Apache-2.0 OR MIT |
| `wasm-streams` | 0.5.0 | Apache-2.0 OR MIT |
| `wasmparser` | 0.244.0 | Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT |
| `web-sys` | 0.3.99 | Apache-2.0 OR MIT |
| `web-sys` | 0.3.103 | Apache-2.0 OR MIT |
| `web-time` | 1.1.0 | Apache-2.0 OR MIT |
| `web_atoms` | 0.2.4 | Apache-2.0 OR MIT |
| `webkit2gtk` | 2.0.2 | MIT |
| `webkit2gtk-sys` | 2.0.2 | MIT |
| `webpki-root-certs` | 1.0.8 | CDLA-Permissive-2.0 |
| `webpki-roots` | 1.0.7 | CDLA-Permissive-2.0 |
| `webpki-roots` | 1.0.8 | CDLA-Permissive-2.0 |
| `webpki-roots` | 1.0.9 | CDLA-Permissive-2.0 |
| `webview2-com` | 0.38.2 | MIT |
| `webview2-com-macros` | 0.8.1 | MIT |
| `webview2-com-sys` | 0.38.2 | MIT |
| `weezl` | 0.1.12 | Apache-2.0 OR MIT |
| `winapi` | 0.3.9 | Apache-2.0 OR MIT |
| `winapi-i686-pc-windows-gnu` | 0.4.0 | Apache-2.0 OR MIT |
| `winapi-util` | 0.1.11 | MIT OR Unlicense |
| `winapi-x86_64-pc-windows-gnu` | 0.4.0 | Apache-2.0 OR MIT |
| `window-vibrancy` | 0.6.0 | Apache-2.0 OR MIT |
| `windows` | 0.57.0 | Apache-2.0 OR MIT |
| `windows` | 0.58.0 | Apache-2.0 OR MIT |
| `windows` | 0.61.3 | Apache-2.0 OR MIT |
| `windows-collections` | 0.2.0 | Apache-2.0 OR MIT |
| `windows-core` | 0.57.0 | Apache-2.0 OR MIT |
| `windows-core` | 0.58.0 | Apache-2.0 OR MIT |
| `windows-core` | 0.61.2 | Apache-2.0 OR MIT |
| `windows-core` | 0.62.2 | Apache-2.0 OR MIT |
| `windows-future` | 0.2.1 | Apache-2.0 OR MIT |
| `windows-implement` | 0.57.0 | Apache-2.0 OR MIT |
| `windows-implement` | 0.58.0 | Apache-2.0 OR MIT |
| `windows-implement` | 0.60.2 | Apache-2.0 OR MIT |
| `windows-interface` | 0.57.0 | Apache-2.0 OR MIT |
| `windows-interface` | 0.58.0 | Apache-2.0 OR MIT |
| `windows-interface` | 0.59.3 | Apache-2.0 OR MIT |
| `windows-link` | 0.1.3 | Apache-2.0 OR MIT |
| `windows-link` | 0.2.1 | Apache-2.0 OR MIT |
| `windows-numerics` | 0.2.0 | Apache-2.0 OR MIT |
| `windows-registry` | 0.6.1 | Apache-2.0 OR MIT |
| `windows-result` | 0.1.2 | Apache-2.0 OR MIT |
| `windows-result` | 0.2.0 | Apache-2.0 OR MIT |
| `windows-result` | 0.3.4 | Apache-2.0 OR MIT |
| `windows-result` | 0.4.1 | Apache-2.0 OR MIT |
| `windows-strings` | 0.1.0 | Apache-2.0 OR MIT |
| `windows-strings` | 0.4.2 | Apache-2.0 OR MIT |
| `windows-strings` | 0.5.1 | Apache-2.0 OR MIT |
| `windows-sys` | 0.45.0 | Apache-2.0 OR MIT |
| `windows-sys` | 0.48.0 | Apache-2.0 OR MIT |
| `windows-sys` | 0.52.0 | Apache-2.0 OR MIT |
| `windows-sys` | 0.59.0 | Apache-2.0 OR MIT |
| `windows-sys` | 0.60.2 | Apache-2.0 OR MIT |
| `windows-sys` | 0.61.2 | Apache-2.0 OR MIT |
| `windows-targets` | 0.42.2 | Apache-2.0 OR MIT |
| `windows-targets` | 0.48.5 | Apache-2.0 OR MIT |
| `windows-targets` | 0.52.6 | Apache-2.0 OR MIT |
| `windows-targets` | 0.53.5 | Apache-2.0 OR MIT |
| `windows-threading` | 0.1.0 | Apache-2.0 OR MIT |
| `windows-version` | 0.1.7 | Apache-2.0 OR MIT |
| `windows_aarch64_gnullvm` | 0.42.2 | Apache-2.0 OR MIT |
| `windows_aarch64_gnullvm` | 0.48.5 | Apache-2.0 OR MIT |
| `windows_aarch64_gnullvm` | 0.52.6 | Apache-2.0 OR MIT |
| `windows_aarch64_gnullvm` | 0.53.1 | Apache-2.0 OR MIT |
| `windows_aarch64_msvc` | 0.42.2 | Apache-2.0 OR MIT |
| `windows_aarch64_msvc` | 0.48.5 | Apache-2.0 OR MIT |
| `windows_aarch64_msvc` | 0.52.6 | Apache-2.0 OR MIT |
| `windows_aarch64_msvc` | 0.53.1 | Apache-2.0 OR MIT |
| `windows_i686_gnu` | 0.42.2 | Apache-2.0 OR MIT |
| `windows_i686_gnu` | 0.48.5 | Apache-2.0 OR MIT |
| `windows_i686_gnu` | 0.52.6 | Apache-2.0 OR MIT |
| `windows_i686_gnu` | 0.53.1 | Apache-2.0 OR MIT |
| `windows_i686_gnullvm` | 0.52.6 | Apache-2.0 OR MIT |
| `windows_i686_gnullvm` | 0.53.1 | Apache-2.0 OR MIT |
| `windows_i686_msvc` | 0.42.2 | Apache-2.0 OR MIT |
| `windows_i686_msvc` | 0.48.5 | Apache-2.0 OR MIT |
| `windows_i686_msvc` | 0.52.6 | Apache-2.0 OR MIT |
| `windows_i686_msvc` | 0.53.1 | Apache-2.0 OR MIT |
| `windows_x86_64_gnu` | 0.42.2 | Apache-2.0 OR MIT |
| `windows_x86_64_gnu` | 0.48.5 | Apache-2.0 OR MIT |
| `windows_x86_64_gnu` | 0.52.6 | Apache-2.0 OR MIT |
| `windows_x86_64_gnu` | 0.53.1 | Apache-2.0 OR MIT |
| `windows_x86_64_gnullvm` | 0.42.2 | Apache-2.0 OR MIT |
| `windows_x86_64_gnullvm` | 0.48.5 | Apache-2.0 OR MIT |
| `windows_x86_64_gnullvm` | 0.52.6 | Apache-2.0 OR MIT |
| `windows_x86_64_gnullvm` | 0.53.1 | Apache-2.0 OR MIT |
| `windows_x86_64_msvc` | 0.42.2 | Apache-2.0 OR MIT |
| `windows_x86_64_msvc` | 0.48.5 | Apache-2.0 OR MIT |
| `windows_x86_64_msvc` | 0.52.6 | Apache-2.0 OR MIT |
| `windows_x86_64_msvc` | 0.53.1 | Apache-2.0 OR MIT |
| `winnow` | 0.5.40 | MIT |
| `winnow` | 0.7.15 | MIT |
| `winnow` | 1.0.3 | MIT |
| `winreg` | 0.10.1 | MIT |
| `winreg` | 0.55.0 | MIT |
| `wit-bindgen` | 0.51.0 | Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT |
| `wit-bindgen` | 0.57.1 | Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT |
| `wit-bindgen-core` | 0.51.0 | Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT |
| `wit-bindgen-rust` | 0.51.0 | Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT |
| `wit-bindgen-rust-macro` | 0.51.0 | Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT |
| `wit-component` | 0.244.0 | Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT |
| `wit-parser` | 0.244.0 | Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT |
| `writeable` | 0.6.3 | Unicode-3.0 |
| `wry` | 0.55.1 | Apache-2.0 OR MIT |
| `wyz` | 0.5.1 | MIT |
| `x11` | 2.21.0 | MIT |
| `x11-dl` | 2.21.0 | MIT |
| `x25519-dalek` | 2.0.1 | BSD-3-Clause |
| `xattr` | 1.6.1 | Apache-2.0 OR MIT |
| `xxhash-rust` | 0.8.16 | BSL-1.0 |
| `y4m` | 0.8.0 | MIT |
| `yoke` | 0.7.5 | Unicode-3.0 |
| `yoke` | 0.8.2 | Unicode-3.0 |
| `yoke` | 0.8.3 | Unicode-3.0 |
| `yoke-derive` | 0.7.5 | Unicode-3.0 |
| `yoke-derive` | 0.8.2 | Unicode-3.0 |
| `zbus` | 5.16.0 | MIT |
| `zbus_macros` | 5.16.0 | MIT |
| `zbus_names` | 4.3.2 | MIT |
| `zerocopy` | 0.8.48 | Apache-2.0 OR BSD-2-Clause OR MIT |
| `zerocopy` | 0.8.54 | Apache-2.0 OR BSD-2-Clause OR MIT |
| `zerocopy` | 0.8.55 | Apache-2.0 OR BSD-2-Clause OR MIT |
| `zerocopy-derive` | 0.8.48 | Apache-2.0 OR BSD-2-Clause OR MIT |
| `zerocopy-derive` | 0.8.54 | Apache-2.0 OR BSD-2-Clause OR MIT |
| `zerocopy-derive` | 0.8.55 | Apache-2.0 OR BSD-2-Clause OR MIT |
| `zerofrom` | 0.1.8 | Unicode-3.0 |
| `zerofrom-derive` | 0.1.7 | Unicode-3.0 |
| `zeroize` | 1.8.2 | Apache-2.0 OR MIT |
| `zeroize` | 1.9.0 | Apache-2.0 OR MIT |
| `zeroize_derive` | 1.4.3 | Apache-2.0 OR MIT |
| `zerotrie` | 0.2.4 | Unicode-3.0 |
| `zerovec` | 0.11.6 | Unicode-3.0 |
| `zerovec-derive` | 0.11.3 | Unicode-3.0 |
| `zip` | 7.2.0 | MIT |
| `zmij` | 1.0.21 | MIT |
| `zmij` | 1.0.23 | MIT |
| `zstd` | 0.13.3 | MIT |
| `zstd-safe` | 7.2.4 | Apache-2.0 OR MIT |
| `zstd-sys` | 2.0.16+zstd.1.5.7 | Apache-2.0 OR MIT |
| `zune-core` | 0.5.1 | Apache-2.0 OR MIT OR Zlib |
| `zune-inflate` | 0.2.54 | Apache-2.0 OR MIT OR Zlib |
| `zune-jpeg` | 0.5.15 | Apache-2.0 OR MIT OR Zlib |
| `zvariant` | 5.12.0 | MIT |
| `zvariant_derive` | 5.12.0 | MIT |
| `zvariant_utils` | 3.4.0 | MIT |

---

## Part 3 — Bundled art & fonts

Assets shipped inside the app, each under its own license. **CC-BY / CC-BY-SA assets require
attribution; they are credited here (and in `public/polis/CREDITS.md`).**

### Polis isometric art

The "Polis" city view renders open-licensed sprite art (curated, sometimes rescaled/recolored to fit
the palette). Full per-source notes live in [`public/polis/CREDITS.md`](./public/polis/CREDITS.md).

- **Screaming Brain Studios — "Tiny Texture Pack"** — seamless terrain/material textures (`tex:*`:
  grass, dirt, stone, plaster, brick, marble, wood, roof tile, thatch, water, …).
  <https://opengameart.org/content/tiny-texture-pack> — **CC0 1.0** (public domain; credited with thanks).
- **Unknown Horizons team** — tree/cypress sprites, the ambient citizen walk cycles, and countryside
  resource art (mines, quarries, rocks). <https://github.com/unknown-horizons/unknown-horizons>
  (content/gfx; multiple artists, see their `doc/AUTHORS.md`) — **CC-BY-SA 3.0**. Sprites were
  rescaled/re-packed; the modified art files remain CC-BY-SA (share-alike applies to those art files,
  not to the application).
- **FoshyTakashi** — 9-frame fire animation (burning-building flip-book, `fx:fire:*`).
  <https://opengameart.org/content/9-frame-fire-animation-16x-32x-64x> — **CC-BY 3.0** (frames cut,
  rescaled, tinted per fire severity).

### Fonts

Bundled via the npm `@fontsource-variable/*` packages (also listed in Part 2):

- **Instrument Sans (Variable)** — UI/design typeface. <https://github.com/fontsource/font-files> —
  **SIL Open Font License 1.1**.
- **Source Serif 4 (Variable)** — serif typeface (Adobe / Google Fonts). — **SIL Open Font License 1.1**.

(Other font families named in CSS — Inter, JetBrains Mono, Cascadia Code, SF Mono, Menlo — are
system/fallback references only and are **not** bundled.)

---

## Part 4 — AI models & inference runtimes

- **Qwen3-Embedding-0.6B** (Qwen team, Alibaba) — the Oracle code-memory semantic embedder. The model
  weights are **not committed to this repo**; they are downloaded on first use from the Hugging Face
  Hub (`onnx-community/Qwen3-Embedding-0.6B-ONNX` and `Qwen/Qwen3-Embedding-0.6B`) into a local,
  git-ignored cache. Model + tokenizer are used under the **Apache-2.0** license stated on the model
  card. <https://huggingface.co/Qwen/Qwen3-Embedding-0.6B>
- **ONNX Runtime** (Microsoft) via the `ort` crate, **fastembed** (Qdrant), and **candle** +
  **tokenizers** (Hugging Face) — the inference/embedding libraries that load and run the model above.
  These are Rust crates and are also covered by the Part 2 inventory (MIT / Apache-2.0).

Other model families are only **referenced** (selectable by the user, supplied through their own
Ollama / oMLX / Apple-Foundation-Models install or a cloud API key) and are **not distributed by this
repository** — e.g. Google **Gemma**, **Qwen2.5-Coder**, NVIDIA **Nemotron**, OpenAI **GPT-4o**,
**Claude**, **DeepSeek**, **GLM**, **MiMo**. Each is governed by its own model license/terms; this
project ships none of their weights.

---

## Part 5 — Integrated external tools & agents (not bundled)

Devboule orchestrates external programs that the **user installs separately**. Their code is not
included or redistributed here; each is used under its own license/terms. They are acknowledged
because the product is built around integrating them:

**Agent CLIs / runtimes**
- **pi** — the [`@earendil-works/pi-coding-agent`](https://www.npmjs.com/package/@earendil-works/pi-coding-agent)
  coding-agent CLI/SDK — the default agent harness (Node sidecar).
- **Claude Code** (`claude`, Anthropic), **OpenAI Codex** (`codex`, OpenAI), **Grok** (xAI, as an MCP
  client) — cloud coding agents the app can drive.
- **Ollama**, **oMLX** (OpenAI-compatible MLX server), **Apple Foundation Models** (`fm`, macOS) —
  local model runners the app can target.
- **Node.js**, **Python 3**, **Git** — required host toolchain the backend shells out to.

**pi extensions the app can install for you** (fetched from the npm registry / GitHub on request):
`@tintinweb/pi-subagents` (multi-agent orchestration), `pi-lens` (real-time LSP/lint feedback),
`@pi-unipi/compactor` (context compaction), `pi-web-access` (web search). Each is under its own
publisher's license.

**Censor code-quality runners** — the optional local review pass invokes whichever of these are
installed; none are bundled: `ruff`, `bandit`, `oxlint`, `eslint`, `pyright`, `clippy` / `cargo`
(`check`/`fmt`/`audit`/`deny`), `shellcheck`, `semgrep`, `gitleaks`, `tsc`, `prettier`, `stylelint`,
`hadolint`, `actionlint`, `yamllint`, `sqlfluff`, `tidy`, `cppcheck`, `go vet`/`gofmt`, `ktlint`,
`lizard`, `jscpd`, `knip`, `npm audit`, `pip audit`, `vulture`, `zizmor`, and `xh`. Each is used under
its own license.

---

## Part 6 — External services & APIs

Network services the app can call when the user configures/enables them (used under each provider's
Terms of Service; no code of theirs is included):

- **LLM / inference APIs**: OpenAI (`api.openai.com`), Anthropic (`api.anthropic.com`), OpenRouter
  (`openrouter.ai`), DeepSeek (`api.deepseek.com`), Scaleway AI, Infomaniak AI, Mistral AI, Nebius.
- **Web search**: Exa, and (via `pi-web-access`) Brave, Tavily, Perplexity, Gemini, Parallel.
- **Model / package hosts**: Hugging Face Hub (embedding-model download), the npm registry (pi
  extension marketplace), and the GitHub API (auth + in-app git push).

---

## Part 7 — Adapted algorithm/pattern credits

Not copied source, but behavior/patterns modeled on prior work, acknowledged for good faith:

- **terax-ai** — the app-hosted agent PTY handling (open/spawn/reader/kill-on-failure via
  `portable-pty`) follows terax-ai's patterns; noted **Apache-2.0** in `src-tauri/src/backend/agent_pty.rs`.
- **Aider** — the mini-editor's fuzzy "near-miss" edit fallback (a `difflib.SequenceMatcher`-style
  ratio threshold, computed with the `similar` crate) mirrors Aider's approach.
