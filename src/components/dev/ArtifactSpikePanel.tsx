// THROWAWAY Phase-0 spike host panel (plan `bubbly-hopping-valiant.md`, Phase 0).
//
// Renders BOTH candidate artifact-render paths side by side and reports, in a table,
// whether each one's inline <script> executed under the app's CSP (`script-src 'self'`):
//
//   PATH A — srcdoc: `<iframe sandbox="allow-scripts">` whose srcDoc carries the inline
//            script. A srcdoc document INHERITS the host page CSP, so the inline script
//            is EXPECTED TO BE BLOCKED. A `<meta http-equiv="Content-Security-Policy"
//            content="connect-src 'none'">` is included to test that a meta can only
//            TIGHTEN (never loosen) the inherited policy.
//   PATH B — separate origin: `<iframe src="artifact://localhost/spike">` served by the
//            Rust `artifact:` scheme handler with its OWN CSP header, so its inline
//            script does NOT inherit `script-src 'self'` and is EXPECTED TO RUN.
//
// Trust model for messages: a sandboxed/opaque frame reports `event.origin === "null"`,
// so the ONLY safe discriminator is object identity `event.source === iframe.contentWindow`
// — never `event.origin`. Messages are schema-validated before they touch state.
//
// This component is mounted ONLY when the app is opened with `?spike=1` (see App.tsx),
// before the vault-unlock gate, so it is trivially reachable in dev and never wired into
// product flows. Delete this file + the lazy import/guard in App.tsx to remove it.

import { useEffect, useMemo, useState } from "react";

type SpikeResult = {
  /** Inline <script> executed (a well-formed spike message was received). */
  ran: boolean;
  /** fetch() to a remote origin was blocked by CSP `connect-src 'none'`. */
  fetchBlocked: boolean | null;
  /** window.parent.* / __TAURI_INTERNALS__ was unreachable (opaque origin). */
  ipcUnreachable: boolean | null;
  note: string;
};

type SpikeMessage = {
  t: "spike";
  path: "A" | "B";
  ok: boolean;
  fetchBlocked?: boolean;
  ipcUnreachable?: boolean;
  note?: string;
};

const RESULT_TIMEOUT_MS = 2500;

// PATH A document, rendered via the iframe `srcDoc` attribute. Mirror of the Rust PATH B
// doc except: path label "A" and the network gate comes from a <meta> CSP (PATH B's comes
// from its response header). postMessage target MUST be '*' (a custom-scheme target throws
// `SyntaxError: Invalid target origin` in WebView2).
const PATH_A_SRCDOC = `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta http-equiv="Content-Security-Policy" content="connect-src 'none'" />
  <title>Artifact spike — PATH A</title>
  <style>body{font:13px system-ui,sans-serif;margin:0;padding:12px;color:#1b1721}</style>
</head>
<body>
  <p>PATH A srcdoc (inherits host CSP). Inline script ran if the host table shows a check mark.</p>
  <script>
    (function () {
      function send(payload) { try { parent.postMessage(payload, '*'); } catch (e) {} }
      var ipcUnreachable;
      try {
        var probe = window.parent.__TAURI_INTERNALS__;
        var url = window.parent.location.href;
        ipcUnreachable = (probe === undefined && url === undefined);
      } catch (e) { ipcUnreachable = true; }
      function finish(fetchBlocked, note) {
        send({ t: 'spike', path: 'A', ok: true, fetchBlocked: fetchBlocked, ipcUnreachable: ipcUnreachable, note: note });
      }
      try {
        fetch('https://example.com', { mode: 'no-cors' }).then(function () {
          finish(false, 'fetch resolved (NOT blocked)');
        }).catch(function (err) {
          finish(true, 'fetch rejected: ' + String(err && err.message || err));
        });
      } catch (e) { finish(true, 'fetch threw: ' + String(e && e.message || e)); }
    })();
  </script>
</body>
</html>`;

/** Probe whether THIS host page actually enforces `script-src 'self'` (i.e. the app CSP
 *  is live in the current context). Appends a runtime inline <script> with no nonce/hash:
 *  under `script-src 'self'` it is blocked and never sets the flag. Returns true when the
 *  inline script was BLOCKED (CSP active). If this returns false, the host CSP is NOT
 *  enforced here and the PATH A result below is a FALSE POSITIVE — read the warning. */
function probeHostInlineBlocked(): boolean {
  // Index via a string key on an `unknown`-valued bag so TS does NOT narrow the slot to
  // the literal `false` we seed it with (the inline script mutates it out-of-band).
  const bag = window as unknown as Record<string, unknown>;
  bag.__spikeHostInline = false;
  try {
    const s = document.createElement("script");
    s.textContent = "window.__spikeHostInline = true;";
    document.head.appendChild(s);
    document.head.removeChild(s);
  } catch {
    return true; // insertion itself rejected ⇒ CSP active
  }
  return bag.__spikeHostInline !== true; // never set ⇒ inline blocked ⇒ CSP active
}

function isSpikeMessage(data: unknown): data is SpikeMessage {
  if (typeof data !== "object" || data === null) return false;
  const d = data as Record<string, unknown>;
  return d.t === "spike" && (d.path === "A" || d.path === "B") && typeof d.ok === "boolean";
}

function toResult(d: SpikeMessage): SpikeResult {
  return {
    ran: d.ok === true,
    fetchBlocked: typeof d.fetchBlocked === "boolean" ? d.fetchBlocked : null,
    ipcUnreachable: typeof d.ipcUnreachable === "boolean" ? d.ipcUnreachable : null,
    note: typeof d.note === "string" ? d.note.slice(0, 300) : "",
  };
}

function YesNo({ value }: { value: boolean | null }) {
  if (value === null) return <span style={{ color: "#9a9a9a" }}>—</span>;
  return (
    <span style={{ color: value ? "#137a3f" : "#c0392b", fontWeight: 700 }}>
      {value ? "✓" : "✗"}
    </span>
  );
}

function ResultRow({ label, result }: { label: string; result: SpikeResult | null }) {
  return (
    <tr>
      <td style={cellStyle}>{label}</td>
      <td style={cellStyle}>
        {result === null ? <em style={{ color: "#9a9a9a" }}>waiting…</em> : <YesNo value={result.ran} />}
      </td>
      <td style={cellStyle}>
        <YesNo value={result?.fetchBlocked ?? null} />
      </td>
      <td style={cellStyle}>
        <YesNo value={result?.ipcUnreachable ?? null} />
      </td>
      <td style={{ ...cellStyle, color: "#555", fontFamily: "ui-monospace, monospace", fontSize: 11 }}>
        {result?.note ?? ""}
      </td>
    </tr>
  );
}

const cellStyle: React.CSSProperties = {
  border: "1px solid #e3ddd2",
  padding: "6px 10px",
  textAlign: "left",
  verticalAlign: "top",
};

export function ArtifactSpikePanel() {
  const [resultA, setResultA] = useState<SpikeResult | null>(null);
  const [resultB, setResultB] = useState<SpikeResult | null>(null);
  const [hostInlineBlocked, setHostInlineBlocked] = useState<boolean | null>(null);

  // Per-platform artifact origin. macOS/Linux: `artifact://localhost/…`. Windows
  // (WebView2): Tauri serves custom schemes at `http://<scheme>.localhost/…`.
  const artifactUrl = useMemo(() => {
    const isWindows = /Windows/i.test(navigator.userAgent);
    return isWindows ? "http://artifact.localhost/spike" : "artifact://localhost/spike";
  }, []);

  const hasTauriInternals =
    typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  const origin = typeof window !== "undefined" ? window.location.origin : "(none)";
  const userAgent = typeof navigator !== "undefined" ? navigator.userAgent : "(none)";

  useEffect(() => {
    setHostInlineBlocked(probeHostInlineBlocked());

    // Resolve the iframes by id at message time (refs would force a re-render dance with
    // the lazy/Suspense mount); ids are unique to this throwaway panel.
    function frameWindow(id: string): Window | null {
      const el = document.getElementById(id);
      return el instanceof HTMLIFrameElement ? el.contentWindow : null;
    }

    function onMessage(event: MessageEvent) {
      // TRUST ANCHOR: source-window object identity. NEVER event.origin (it is the
      // string "null" for every opaque/sandboxed frame and cannot be trusted).
      const winA = frameWindow("artifact-spike-a");
      const winB = frameWindow("artifact-spike-b");
      const fromA = winA !== null && event.source === winA;
      const fromB = winB !== null && event.source === winB;
      if (!fromA && !fromB) return;
      if (!isSpikeMessage(event.data)) return;
      const result = toResult(event.data);
      if (fromA) setResultA(result);
      else if (fromB) setResultB(result);
    }

    window.addEventListener("message", onMessage);

    // If no message arrives within the window, the inline script was blocked (or the
    // frame failed to load). Functional update so a real message that already landed wins.
    const timedOut: SpikeResult = {
      ran: false,
      fetchBlocked: null,
      ipcUnreachable: null,
      note: `No message within ${RESULT_TIMEOUT_MS}ms — inline <script> blocked or frame did not load.`,
    };
    const timerA = window.setTimeout(() => setResultA((prev) => prev ?? timedOut), RESULT_TIMEOUT_MS);
    const timerB = window.setTimeout(() => setResultB((prev) => prev ?? timedOut), RESULT_TIMEOUT_MS);

    return () => {
      window.removeEventListener("message", onMessage);
      window.clearTimeout(timerA);
      window.clearTimeout(timerB);
    };
  }, []);

  return (
    <div style={{ padding: 24, font: "14px system-ui, sans-serif", color: "#1b1721", maxWidth: 1100 }}>
      <h1 style={{ fontSize: 20, margin: "0 0 4px" }}>Artifact render spike — PATH A vs PATH B</h1>
      <p style={{ margin: "0 0 16px", color: "#555" }}>
        Throwaway Phase-0 test. Decides whether <code>&lt;iframe sandbox=&quot;allow-scripts&quot;&gt;</code>{" "}
        runs inline <code>&lt;script&gt;</code> under the app CSP <code>script-src &apos;self&apos;</code>.
      </p>

      <div
        style={{
          background: hostInlineBlocked === false ? "#fdecec" : "#f4f1ea",
          border: "1px solid #e3ddd2",
          borderRadius: 8,
          padding: "10px 14px",
          marginBottom: 16,
          fontSize: 12,
          lineHeight: 1.6,
        }}
      >
        <strong>Host diagnostics</strong>
        <br />
        origin: <code>{origin}</code>
        <br />
        host CSP active (runtime inline script blocked):{" "}
        <YesNo value={hostInlineBlocked} />{" "}
        {hostInlineBlocked === false && (
          <strong style={{ color: "#c0392b" }}>
            — WARNING: the app CSP is NOT enforced here, so the PATH A result below is a
            FALSE POSITIVE. Re-test in a context where this shows ✓.
          </strong>
        )}
        <br />
        host has <code>__TAURI_INTERNALS__</code>: <YesNo value={hasTauriInternals} /> (this is the
        capability-bearing parent the artifact must NOT reach)
        <br />
        PATH B artifact URL: <code>{artifactUrl}</code>
        <br />
        userAgent: <code style={{ fontSize: 11 }}>{userAgent}</code>
      </div>

      <table style={{ borderCollapse: "collapse", width: "100%", marginBottom: 16, fontSize: 13 }}>
        <thead>
          <tr style={{ background: "#f4f1ea" }}>
            <th style={cellStyle}>path</th>
            <th style={cellStyle}>script ran?</th>
            <th style={cellStyle}>fetch blocked?</th>
            <th style={cellStyle}>IPC unreachable?</th>
            <th style={cellStyle}>note</th>
          </tr>
        </thead>
        <tbody>
          <ResultRow label="PATH A (srcdoc)" result={resultA} />
          <ResultRow label="PATH B (artifact://)" result={resultB} />
        </tbody>
      </table>

      <p style={{ fontSize: 12, color: "#555", marginBottom: 20 }}>
        Decision rule: if <strong>PATH A “script ran?”</strong> is ✓ on BOTH macOS (WKWebView)
        and Windows (WebView2) → choose <strong>PATH A</strong> (simplest, srcdoc). If it is ✗ on
        either → choose <strong>PATH B</strong> (separate origin). Both must show fetch blocked ✓ and
        IPC unreachable ✓. <button type="button" onClick={() => window.location.reload()}>Re-run</button>
      </p>

      <div style={{ display: "flex", gap: 24, flexWrap: "wrap" }}>
        <figure style={{ margin: 0 }}>
          <figcaption style={{ fontSize: 12, fontWeight: 700, marginBottom: 6 }}>PATH A — srcdoc</figcaption>
          <iframe
            id="artifact-spike-a"
            title="artifact-spike-a"
            sandbox="allow-scripts"
            srcDoc={PATH_A_SRCDOC}
            style={{ width: 460, height: 140, border: "1px solid #e3ddd2", borderRadius: 8 }}
          />
        </figure>
        <figure style={{ margin: 0 }}>
          <figcaption style={{ fontSize: 12, fontWeight: 700, marginBottom: 6 }}>
            PATH B — separate origin
          </figcaption>
          <iframe
            id="artifact-spike-b"
            title="artifact-spike-b"
            sandbox="allow-scripts"
            src={artifactUrl}
            style={{ width: 460, height: 140, border: "1px solid #e3ddd2", borderRadius: 8 }}
          />
        </figure>
      </div>
    </div>
  );
}
