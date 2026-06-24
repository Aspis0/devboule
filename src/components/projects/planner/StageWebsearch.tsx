import { useState } from "react";
import type { CSSProperties } from 'react';
import type { StagePage, StageFinding } from "./plannerModel";
import { Search, Zap, Hand } from "lucide-react";
import { WebsearchView } from "../../activity/WebsearchView";

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
  const isAuto = mode === 'auto';

  const rootStyle: CSSProperties = { flex: 1, display: 'flex', flexDirection: 'column', gap: 10, minHeight: 0 };
  const headerRowStyle: CSSProperties = { display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 4 };
  const pillActiveStyle: CSSProperties = { display: 'inline-flex', alignItems: 'center', gap: 5, fontSize: 10.5, fontWeight: 600, padding: '4px 9px', borderRadius: 8, cursor: 'pointer', background: '#C8945C', color: '#FBF6EF' };
  const pillInactiveStyle: CSSProperties = { display: 'inline-flex', alignItems: 'center', gap: 5, fontSize: 10.5, fontWeight: 600, padding: '4px 9px', borderRadius: 8, cursor: 'pointer', background: '#fff', color: '#9c9488', border: '1px solid #ECE6DB' };
  const searchInputStyle: CSSProperties = { display: 'flex', alignItems: 'center', border: '1px solid #E4DDD0', borderRadius: 8, padding: '4px 8px', fontSize: 11, gap: 6, flex: 1, maxWidth: 200 };

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

      <WebsearchView pages={pages} findings={findings} isAuto={isAuto} live={live} />
    </div>
  );
}
