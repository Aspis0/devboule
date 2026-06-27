// Dev harness for the Phase-1 interactive-artifact render surface (plan
// `bubbly-hopping-valiant.md`). Mounted ONLY when the app is opened with `?artifact=1` (see
// the guard in App.tsx), BEFORE the vault-unlock gate, so the owner can SEE a real interactive
// artifact running + resizing independent of Phase 2's generation pipeline.
//
// It hosts `ArtifactView` pointed at the reserved `__sample__` route, which the Rust
// `artifact:` scheme handler serves as a known-good interactive document (a clickable counter
// that grows the body → exercises `artifact:resize`, a cdnjs library load → proves the CDN
// allowlist, and a `fetch` that must be blocked → proves `connect-src 'none'`). Delete this
// file + the lazy import/guard in App.tsx to remove the harness.

import { useState } from "react";
import { ArtifactView } from "../projects/artifact/ArtifactView";
import { buildArtifactSrc } from "../projects/artifact/artifactProtocol";

const SAMPLE_ID = "__sample__";

export function ArtifactDevPanel() {
  const [ready, setReady] = useState(false);
  const [lastError, setLastError] = useState<string | null>(null);
  // Bump to force a full remount (new iframe load) of the artifact.
  const [reloadKey, setReloadKey] = useState(0);

  const hasTauriInternals =
    typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  const origin = typeof window !== "undefined" ? window.location.origin : "(none)";
  const src = buildArtifactSrc(SAMPLE_ID);

  return (
    <div style={{ padding: 24, font: "14px system-ui, sans-serif", color: "#1b1721", maxWidth: 900 }}>
      <h1 style={{ fontSize: 20, margin: "0 0 4px" }}>Interactive artifact — Phase 1 render surface</h1>
      <p style={{ margin: "0 0 16px", color: "#555" }}>
        Hosts <code>ArtifactView</code> on the separate-origin <code>artifact:</code> scheme. The
        document runs real JS, loads a CDN library, and is blocked from any network exfiltration.
      </p>

      <div
        style={{
          background: "#f4f1ea",
          border: "1px solid #e3ddd2",
          borderRadius: 8,
          padding: "10px 14px",
          marginBottom: 16,
          fontSize: 12,
          lineHeight: 1.7,
        }}
      >
        <strong>Diagnostics</strong>
        <br />
        host origin: <code>{origin}</code>
        <br />
        host has <code>__TAURI_INTERNALS__</code>:{" "}
        <strong style={{ color: hasTauriInternals ? "#137a3f" : "#9a9a9a" }}>
          {hasTauriInternals ? "yes" : "no"}
        </strong>{" "}
        (the capability-bearing parent the artifact must NOT reach)
        <br />
        artifact src: <code>{src}</code>
        <br />
        guest ready:{" "}
        <strong style={{ color: ready ? "#137a3f" : "#c0392b" }}>{ready ? "yes" : "waiting…"}</strong>
        <br />
        last guest error:{" "}
        <code style={{ fontSize: 11 }}>{lastError ?? "(none)"}</code>{" "}
        <button
          type="button"
          onClick={() => {
            setReady(false);
            setLastError(null);
            setReloadKey((k) => k + 1);
          }}
        >
          Reload artifact
        </button>
      </div>

      <p style={{ fontSize: 12, color: "#555", margin: "0 0 8px" }}>
        Click the counter button repeatedly — each click appends a row, the document grows, and the
        iframe should resize to fit (proving the <code>artifact:resize</code> bridge).
      </p>

      <ArtifactView
        key={reloadKey}
        artifactId={SAMPLE_ID}
        title="Interactive artifact sample"
        onReady={() => setReady(true)}
        onError={(m) => setLastError(m)}
      />
    </div>
  );
}
