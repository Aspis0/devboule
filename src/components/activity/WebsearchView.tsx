import type { CSSProperties } from 'react';
import { useMemo } from "react";
import type { StagePage, StageFinding } from "../projects/planner/plannerModel";
import { pageHostname } from "../projects/planner/plannerModel";
import { Loader2 } from "lucide-react";
import { usePagePreviews } from "./usePagePreviews";
import {
  buildDisplayFindings,
  PREVIEW_LAYOUT_WIDTH,
  previewScale,
} from "./websearchPreview";

interface WebsearchViewProps {
  pages: StagePage[];
  findings: StageFinding[];
  isAuto: boolean;
  live: boolean;
}

/** Fixed outer frame width for the left-column thumbnail stack (matches container). */
const FRAME_WIDTH_PX = 336;

export function WebsearchView({ pages, findings, isAuto, live: _live }: WebsearchViewProps) {
  // F25: idle when there is no websearch payload. A merely-alive orchestrator
  // (parent passed live=!!orchestratorAgentId) must NOT show the "READING LIVE
  // PAGES / loading…" skeleton — that looked stuck when no search was running.
  // `_live` kept for API stability / future "search in flight" signal.
  void _live;
  const idle = pages.length === 0 && findings.length === 0;
  const total = Math.max(pages.length, 3);
  const shown = Math.min(pages.length, 3);

  const previewUrls = useMemo(
    () => pages.slice(0, 3).map((p) => p.url).filter((u) => u.trim().length > 0),
    [pages],
  );
  const previews = usePagePreviews(previewUrls);

  // FINDINGS: keep provider summaries when present; fill empty summaries from
  // lazily-fetched text_excerpt so the Claude path (often snippet-less) still
  // surfaces real read-content.
  const displayFindings = useMemo(
    () => buildDisplayFindings(pages, findings, previews),
    [pages, findings, previews],
  );

  const columnsRowStyle: CSSProperties = { display: 'flex', gap: 14, flex: 1, minHeight: 0 };
  const leftColStyle: CSSProperties = { flex: 'none', width: FRAME_WIDTH_PX, display: 'flex', flexDirection: 'column', minHeight: 0 };
  const leftContainerStyle: CSSProperties = { flex: 1, background: '#fff', border: '1px solid #E4DDD0', borderRadius: 9, overflow: 'hidden', boxShadow: '0 14px 32px -18px rgba(192,137,79,.55)', position: 'relative' };
  const scanlineStyle: CSSProperties = { position: 'absolute', left: 0, right: 0, top: 0, height: 26, background: 'linear-gradient(180deg,transparent,rgba(192,137,79,.28),transparent)', borderTop: '1px solid rgba(192,137,79,.7)', boxShadow: '0 0 18px 2px rgba(192,137,79,.35)', animation: 'pp-scan 3s linear infinite', pointerEvents: 'none' };
  const chipStyle: CSSProperties = { position: 'absolute', right: 7, bottom: 6, fontFamily: 'monospace', fontSize: 8, color: '#9A6A2E', background: 'rgba(255,255,255,.85)', border: '1px solid #EFE0C8', padding: '2px 7px', borderRadius: 6, display: 'flex', alignItems: 'center', gap: 4 };
  const findingRowStyle: CSSProperties = { display: 'flex', gap: 9, background: '#fff', border: '1px solid #E9E3D8', borderRadius: 9, padding: '8px 10px', animation: 'pp-feed .5s ease-out both' };
  const placeholderStyle: CSSProperties = { display: 'flex', alignItems: 'center', gap: 8, border: '1px dashed #D7C6AA', background: '#FCFAF6', borderRadius: 9, padding: '12px', color: '#9c8d77', fontSize: 11 };

  const scale = previewScale(FRAME_WIDTH_PX - 2 /* border */);

  return (
    <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minHeight: 0 }}>
      {idle ? (
        <div
          style={{
            flex: 1,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            textAlign: 'center',
            color: '#9c8d77',
            fontSize: 12,
            lineHeight: 1.5,
            padding: 20,
          }}
        >
          The Orchestrator isn&apos;t searching the web right now — switch to
          Manual to run a one-off search, or describe a goal below to start
          planning.
        </div>
      ) : (
      <div style={columnsRowStyle}>
        {/* LEFT COLUMN */}
        <div style={leftColStyle}>
          <div className="pp-mono" style={{ fontSize: 9.5, letterSpacing: '.14em', color: '#A89F90', marginBottom: 6 }}>
            {pages.length === 0 ? 'READING LIVE PAGES' : `READING LIVE PAGES · ${shown} of ${total}`}
          </div>
          
          <div style={leftContainerStyle}>
            {/* Page Frames */}
            {[0, 1, 2].map((i) => {
              const page = pages[i];
              const isSkeleton = !page;
              const status = page ? previews[page.url] : undefined;
              const ready =
                status?.state === "ready" ? status.preview : undefined;
              const showIframe = Boolean(ready?.sanitizedHtml);
              // Chrome hostname: prefer final_url when we have a successful preview.
              const hostLabel = page
                ? pageHostname(ready?.finalUrl ?? page.url)
                : "loading…";

              return (
                <div
                  key={page?.url ?? `slot-${i}`}
                  style={{
                    position: 'absolute',
                    inset: 0,
                    display: 'flex',
                    flexDirection: 'column',
                    animation: `pp-page 9s ease-in-out infinite ${i * 3}s`,
                  }}
                >
                  {/* Chrome */}
                  <div style={{ height: 20, background: '#F4EFE7', borderBottom: '1px solid #EFE7DA', display: 'flex', alignItems: 'center', padding: '0 8px', gap: 5 }}>
                    <div style={{ width: 5, height: 5, borderRadius: '50%', background: '#D9C3A6' }} />
                    <div style={{ width: 5, height: 5, borderRadius: '50%', background: '#E4D3B6' }} />
                    <span className="pp-mono" style={{ fontSize: 7.5, color: '#9A8E78', marginLeft: 4 }}>
                      {isSkeleton ? 'loading…' : hostLabel}
                    </span>
                  </div>
                  {/* Body: real sandboxed preview, or title+skeleton fallback */}
                  <div style={{ flex: 1, overflow: 'hidden', position: 'relative', background: '#fff' }}>
                    {isSkeleton ? (
                      <div style={{ padding: 12 }}>
                        <div style={{ height: 6, width: '80%', background: '#EDE4D5', borderRadius: 2, margin: '6px 0' }} />
                        <div style={{ height: 6, width: '62%', background: '#EDE4D5', borderRadius: 2, margin: '6px 0' }} />
                        <div style={{ height: 6, width: '90%', background: '#EDE4D5', borderRadius: 2, margin: '6px 0' }} />
                        <div style={{ height: 6, width: '75%', background: '#EDE4D5', borderRadius: 2, margin: '6px 0' }} />
                      </div>
                    ) : showIframe && ready ? (
                      <div
                        style={{
                          position: 'absolute',
                          inset: 0,
                          overflow: 'hidden',
                          pointerEvents: 'none',
                        }}
                      >
                        <iframe
                          title={ready.title || page.title || hostLabel}
                          // Hard sandbox: NO allow-scripts, NO allow-same-origin.
                          sandbox=""
                          srcDoc={ready.sanitizedHtml}
                          tabIndex={-1}
                          style={{
                            width: PREVIEW_LAYOUT_WIDTH,
                            height: `${Math.round(100 / scale)}%`,
                            border: 0,
                            transform: `scale(${scale})`,
                            transformOrigin: 'top left',
                            background: '#fff',
                          }}
                        />
                      </div>
                    ) : (
                      // Loading or error: keep the title card + skeleton bars
                      // (failed fetch never blanks the panel).
                      <div style={{ padding: 12 }}>
                        <div style={{ fontSize: 8, color: '#3B362F', fontWeight: 'bold', padding: '10px 0' }}>
                          {page.title || hostLabel}
                        </div>
                        <div style={{ height: 6, width: '80%', background: '#EDE4D5', borderRadius: 2, margin: '6px 0' }} />
                        <div style={{ height: 6, width: '62%', background: '#EDE4D5', borderRadius: 2, margin: '6px 0' }} />
                        <div style={{ height: 6, width: '90%', background: '#EDE4D5', borderRadius: 2, margin: '6px 0' }} />
                        {status?.state === "loading" && (
                          <div className="pp-mono" style={{ fontSize: 7.5, color: '#A89F90', marginTop: 8 }}>
                            loading preview…
                          </div>
                        )}
                      </div>
                    )}
                  </div>
                </div>
              );
            })}

            {/* Scanline */}
            <div style={scanlineStyle} />

            {/* Fetching Chip (Auto Mode Only) */}
            {isAuto && (
              <div style={chipStyle}>
                <Loader2 size={9} style={{ animation: 'pp-spin .8s linear infinite' }} />
                fetching next
              </div>
            )}
          </div>
        </div>

        {/* RIGHT COLUMN */}
        <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minWidth: 0 }}>
          {/* Findings Header */}
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 8 }}>
            <span className="pp-mono" style={{ fontSize: 9.5, color: '#A89F90' }}>FINDINGS</span>
            <span className="pp-mono" style={{ fontSize: 9.5, color: '#C0894F' }}>feeding the plan</span>
          </div>

          {/* Findings List */}
          <div style={{ display: 'flex', flexDirection: 'column', gap: 7, flex: 1, overflow: 'auto' }}>
            {displayFindings.length === 0 ? (
              <div style={placeholderStyle}>
                <Loader2 size={11} style={{ animation: 'pp-spin .8s linear infinite' }} />
                <span>Distilling findings…</span>
              </div>
            ) : (
              displayFindings.map((f, i) => (
                <div
                  key={i}
                  style={{ ...findingRowStyle, animationDelay: `${i * 0.15 + 0.05}s` }}
                >
                  <div style={{ width: 6, height: 6, borderRadius: '50%', background: '#C0894F', marginTop: 5, flex: 'none' }} />
                  <div style={{ flex: 1, overflow: 'hidden' }}>
                    <div style={{ fontSize: 11.5, color: '#3B362F', lineHeight: 1.4 }}>
                      {f.text}
                    </div>
                    {f.source === "excerpt" && (
                      <div className="pp-mono" style={{ fontSize: 8.5, color: '#A89F90', marginTop: 2 }}>
                        read content
                      </div>
                    )}
                    {f.task != null && (
                      <div className="pp-mono" style={{ fontSize: 8.5, color: '#A89F90', marginTop: 2 }}>
                        → task {f.task}
                      </div>
                    )}
                  </div>
                </div>
              ))
            )}
          </div>
        </div>
      </div>
      )}
    </div>
  );
}
