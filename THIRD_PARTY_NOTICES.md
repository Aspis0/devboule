# Third-Party Notices

This file records the open-source libraries whose code (or code adapted from them) is
included in this project. All entries are MIT-licensed.

---

## NextChat / ChatGPTNextWeb — `app/components/artifacts.tsx` (HTMLPreview)

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

## picturepan2/devices.css — iPhone 14 Pro + Google Pixel 6 Pro skins

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

## jhildenbiddle/css-device-frames — browser chrome frame

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

## NOT included — Open WebUI

Open WebUI uses a custom license (anti-rebrand clause for deployments serving > 50
users) that is **incompatible** with this project. Its code is NOT present in this
repository. The earlier web-research notes that mis-labeled it MIT have been corrected.

Concepts drawn from Open WebUI that are uncopyrightable and self-reimplemented here:

- Injecting `<meta http-equiv="Content-Security-Policy">` into the artifact document.
- `event.source === iframe.contentWindow` as the postMessage trust anchor (this is a
  standard web platform pattern documented on MDN/web.dev).
