import { useState } from "react";
import type { CSSProperties } from 'react';
import type { StagePage, StageFinding } from "./plannerModel";
import { pageHostname } from "./plannerModel";
import { Search, Zap, Hand, Loader2 } from "lucide-react";

type StageWebsearchProps = {
  pages: StagePage[];
  findings: StageFinding[];
  mode: 'auto' | 'manual';
  // Whether the orchestrator is actually working. When false AND no pages have
  // arrived, the view shows a calm idle state — no fake scanline/skeleton activity.
  live: boolean;
  onModeChange: (m: 'auto' | 'manual') => void;
  onManualSearch: (query: string) => void;
};

export function StageWebsearch({
  pages,
  findings,
  mode,
  live,
  onModeChange,
  onManualSearch,
}: StageWebsearchProps) {
  const [query, setQuery] = useState("");
  const idle = !live && pages.length === 0;
  const total = Math.max(pages.length, 3);
  const shown = Math.min(pages.length, 3);
  const isAuto = mode === 'auto';

  // Styles
  const rootStyle: CSSProperties = { flex: 1, display: 'flex', flexDirection: 'column', gap: 10, minHeight: 0 };
  const headerRowStyle: CSSProperties = { display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 4 };
  const columnsRowStyle: CSSProperties = { display: 'flex', gap: 14, flex: 1, minHeight: 0 };
  const leftColStyle: CSSProperties = { flex: 'none', width: 336, display: 'flex', flexDirection: 'column', minHeight: 0 };
  const leftContainerStyle: CSSProperties = { flex: 1, background: '#fff', border: '1px solid #E4DDD0', borderRadius: 9, overflow: 'hidden', boxShadow: '0 14px 32px -18px rgba(192,137,79,.55)', position: 'relative' };
  const pillActiveStyle: CSSProperties = { display: 'inline-flex', alignItems: 'center', gap: 5, fontSize: 10.5, fontWeight: 600, padding: '4px 9px', borderRadius: 8, cursor: 'pointer', background: '#C8945C', color: '#FBF6EF' };
  const pillInactiveStyle: CSSProperties = { display: 'inline-flex', alignItems: 'center', gap: 5, fontSize: 10.5, fontWeight: 600, padding: '4px 9px', borderRadius: 8, cursor: 'pointer', background: '#fff', color: '#9c9488', border: '1px solid #ECE6DB' };
  const searchInputStyle: CSSProperties = { display: 'flex', alignItems: 'center', border: '1px solid #E4DDD0', borderRadius: 8, padding: '4px 8px', fontSize: 11, gap: 6, flex: 1, maxWidth: 200 };
  const scanlineStyle: CSSProperties = { position: 'absolute', left: 0, right: 0, top: 0, height: 26, background: 'linear-gradient(180deg,transparent,rgba(192,137,79,.28),transparent)', borderTop: '1px solid rgba(192,137,79,.7)', boxShadow: '0 0 18px 2px rgba(192,137,79,.35)', animation: 'pp-scan 3s linear infinite', pointerEvents: 'none' };
  const chipStyle: CSSProperties = { position: 'absolute', right: 7, bottom: 6, fontFamily: 'monospace', fontSize: 8, color: '#9A6A2E', background: 'rgba(255,255,255,.85)', border: '1px solid #EFE0C8', padding: '2px 7px', borderRadius: 6, display: 'flex', alignItems: 'center', gap: 4 };
  const findingRowStyle: CSSProperties = { display: 'flex', gap: 9, background: '#fff', border: '1px solid #E9E3D8', borderRadius: 9, padding: '8px 10px', animation: 'pp-feed .5s ease-out both' };
  const placeholderStyle: CSSProperties = { display: 'flex', alignItems: 'center', gap: 8, border: '1px dashed #D7C6AA', background: '#FCFAF6', borderRadius: 9, padding: '12px', color: '#9c8d77', fontSize: 11 };

  return (
    <div style={rootStyle}>
      {/* Header Strip */}
      <div style={headerRowStyle}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <span className="pp-mono" style={{ fontSize: 9.5, letterSpacing: '.14em', color: '#A89F90' }}>
            WEBSEARCH
          </span>
        </div>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          {mode === 'manual' && (
            <div style={searchInputStyle}>
              <Search size={12} color="#9A8E78" />
              <input
                type="text"
                placeholder="Query..."
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                style={{ border: 'none', background: 'transparent', outline: 'none', fontSize: 11, flex: 1, fontFamily: 'inherit', color: '#3B362F' }}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    const q = query.trim();
                    if (q) {
                      onManualSearch(q);
                      setQuery('');
                    }
                  }
                }}
              />
            </div>
          )}
          <div style={{ display: 'flex', gap: 6 }}>
            <button
              style={isAuto ? pillActiveStyle : pillInactiveStyle}
              onClick={() => onModeChange('auto')}
            >
              <Zap size={12} />
              Auto
            </button>
            <button
              style={!isAuto ? pillActiveStyle : pillInactiveStyle}
              onClick={() => onModeChange('manual')}
            >
              <Hand size={12} />
              Manual
            </button>
          </div>
        </div>
      </div>

      {/* Columns row — or a calm idle state when nothing is running (no fake activity). */}
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
              return (
                <div
                  key={i}
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
                      {isSkeleton ? 'loading…' : pageHostname(page.url)}
                    </span>
                  </div>
                  {/* Body */}
                  <div style={{ flex: 1, padding: 12, overflow: 'hidden' }}>
                    {isSkeleton ? (
                      <>
                        <div style={{ height: 6, width: '80%', background: '#EDE4D5', borderRadius: 2, margin: '6px 0' }} />
                        <div style={{ height: 6, width: '62%', background: '#EDE4D5', borderRadius: 2, margin: '6px 0' }} />
                        <div style={{ height: 6, width: '90%', background: '#EDE4D5', borderRadius: 2, margin: '6px 0' }} />
                        <div style={{ height: 6, width: '75%', background: '#EDE4D5', borderRadius: 2, margin: '6px 0' }} />
                      </>
                    ) : (
                      <>
                        <div style={{ fontSize: 8, color: '#3B362F', fontWeight: 'bold', padding: '10px 0' }}>
                          {page.title}
                        </div>
                        <div style={{ height: 6, width: '80%', background: '#EDE4D5', borderRadius: 2, margin: '6px 0' }} />
                        <div style={{ height: 6, width: '62%', background: '#EDE4D5', borderRadius: 2, margin: '6px 0' }} />
                        <div style={{ height: 6, width: '90%', background: '#EDE4D5', borderRadius: 2, margin: '6px 0' }} />
                      </>
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
            {findings.length === 0 ? (
              <div style={placeholderStyle}>
                <Loader2 size={11} style={{ animation: 'pp-spin .8s linear infinite' }} />
                <span>Distilling findings…</span>
              </div>
            ) : (
              findings.map((f, i) => (
                <div
                  key={i}
                  style={{ ...findingRowStyle, animationDelay: `${i * 0.15 + 0.05}s` }}
                >
                  <div style={{ width: 6, height: 6, borderRadius: '50%', background: '#C0894F', marginTop: 5, flex: 'none' }} />
                  <div style={{ flex: 1, overflow: 'hidden' }}>
                    <div style={{ fontSize: 11.5, color: '#3B362F', lineHeight: 1.4 }}>
                      {f.text}
                    </div>
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
