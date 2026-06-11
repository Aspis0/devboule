// Design preview sandbox bootstrap. Standalone, dependency-free, no Tauri/IPC.
//
// The Rust preview window injects the exported standalone HTML as the string
// `window.__PREVIEW_HTML` via an initialization script that runs BEFORE this
// file. We read it and hand it to a sandboxed iframe through `srcdoc`. The
// iframe (index.html) carries a FULLY OPAQUE sandbox (`sandbox=""` — every
// restriction on, NO allow-scripts and NO allow-same-origin), so the previewed
// markup renders but cannot execute script nor reach this page's origin.
//
// When no content was injected (e.g. the page is opened directly), we flip the
// body into the empty state which reveals the "No preview content" message.
//
// Exported as a named function so a jsdom unit test can drive it with a stubbed
// window/document; calling it again is idempotent.
(function () {
  "use strict";

  function applyPreview(win) {
    var doc = win.document;
    var frame = doc.getElementById("frame");
    var html = win.__PREVIEW_HTML;
    if (typeof html === "string" && html.length > 0) {
      if (frame) frame.setAttribute("srcdoc", html);
      doc.body.setAttribute("data-empty", "false");
    } else {
      // No content injected — show the guidance message, leave the iframe blank.
      if (frame) frame.setAttribute("srcdoc", "");
      doc.body.setAttribute("data-empty", "true");
    }
  }

  // Expose for the unit test; in the real window this is just a side-channel.
  if (typeof window !== "undefined") {
    window.__applyDesignPreview = applyPreview;
    // The injected init script + this file both run after the DOM exists (the
    // <script> tag is at the end of <body>), so we can apply immediately.
    applyPreview(window);
  }
})();
