import type { CSSProperties } from 'react';
import { ArrowRight, Check, ImageOff } from 'lucide-react';

interface StageDesignProps {
  design: { name: string; version: string | null; ago: string | null; thumbnailUri: string | null } | null;
  linkedTask: number | null;
  onOpenInDesign: () => void;
}

export const StageDesign: React.FC<StageDesignProps> = ({ design, linkedTask, onOpenInDesign }) => {
  const designName = design?.name ?? 'No design yet';
  const initial = design?.name?.charAt(0).toUpperCase() ?? 'A';

  const rootStyle: CSSProperties = {
    flex: 1,
    display: 'flex',
    gap: 14,
    minHeight: 0,
  };

  const leftPanelStyle: CSSProperties = {
    flex: 1,
    minWidth: 0,
    background: '#fff',
    border: '1px solid #E4DDD0',
    borderRadius: 10,
    overflow: 'hidden',
    boxShadow: '0 12px 30px -18px rgba(0,0,0,.25)',
    display: 'flex',
    flexDirection: 'column',
  };

  const topBarStyle: CSSProperties = {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    padding: '9px 12px',
    borderBottom: '1px solid #EFE7DA',
    background: '#FBF3E8',
  };

  const badgeStyle: CSSProperties = {
    width: 18,
    height: 18,
    background: '#C0894F',
    color: '#fff',
    borderRadius: 5,
    fontSize: 9,
    fontWeight: 700,
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    flexShrink: 0,
  };

  const titleStyle: CSSProperties = {
    fontSize: 12,
    fontWeight: 700,
    color: '#2A2621',
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  };

  const bodyStyle: CSSProperties = {
    flex: 1,
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    padding: 12,
  };

  const imgStyle: CSSProperties = {
    maxWidth: '100%',
    maxHeight: '100%',
    objectFit: 'contain',
    borderRadius: 6,
  };

  const rightPanelStyle: CSSProperties = {
    flex: 'none',
    width: 178,
    display: 'flex',
    flexDirection: 'column',
  };

  const monoLabelStyle: CSSProperties = {
    fontSize: 9.5,
    letterSpacing: '.14em',
    color: '#A89F90',
  };

  const nameStyle: CSSProperties = {
    fontSize: 13.5,
    fontWeight: 700,
    color: '#2A2621',
    marginTop: 9,
  };

  const metaStyle: CSSProperties = {
    fontSize: 10,
    color: '#9c9488',
    marginTop: 3,
  };

  const badgeContainerStyle: CSSProperties = {
    marginTop: 10,
    alignSelf: 'flex-start',
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    fontSize: 10,
    fontWeight: 600,
    color: '#9A6A2E',
    background: '#F1E4D2',
    border: '1px solid #E6D3BB',
    padding: '4px 9px',
    borderRadius: 7,
  };

  const descriptionStyle: CSSProperties = {
    fontSize: 11.5,
    color: '#7c766b',
    lineHeight: 1.5,
    marginTop: 12,
  };

  const buttonStyle: CSSProperties = {
    marginTop: 'auto',
    height: 38,
    border: 'none',
    background: 'linear-gradient(150deg,#C8945C,#B07D43)',
    borderRadius: 10,
    color: '#FBF6EF',
    fontSize: 12.5,
    fontWeight: 700,
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 7,
    cursor: design ? 'pointer' : 'default',
    opacity: design ? 1 : 0.5,
  };

  const emptyStateStyle: CSSProperties = {
    display: 'flex',
    flexDirection: 'column',
    alignItems: 'center',
    justifyContent: 'center',
    textAlign: 'center',
  };

  return (
    <div className="pp-view-enter" style={rootStyle}>
      {/* LEFT PANEL */}
      <div style={leftPanelStyle}>
        {/* TOP BAR */}
        <div style={topBarStyle}>
          <div style={badgeStyle}>
            {initial}
          </div>
          <div style={titleStyle} title={designName}>
            {designName}
          </div>
        </div>

        {/* BODY */}
        <div style={bodyStyle}>
          {design && design.thumbnailUri ? (
            <img
              src={design.thumbnailUri}
              alt={design.name}
              style={imgStyle}
            />
          ) : (
            <div style={emptyStateStyle}>
              <ImageOff size={28} color="#C9BEA9" />
              <p style={{ fontSize: 11.5, color: '#9c8d77', marginTop: 8 }}>
                No design linked yet — the Orchestrator can pull one from the Design page.
              </p>
            </div>
          )}
        </div>
      </div>

      {/* RIGHT PANEL */}
      <div style={rightPanelStyle}>
        <div className="pp-mono" style={monoLabelStyle}>
          FROM DESIGN
        </div>
        <div style={nameStyle}>
          {design?.name ?? '—'}
        </div>
        <div className="pp-mono" style={metaStyle}>
          {design
            ? `Design · ${design.version ?? 'v1'} · ${design.ago ?? 'recent'}`
            : 'no design'}
        </div>

        {linkedTask != null && (
          <div style={badgeContainerStyle}>
            <Check size={11} />
            <span>linked to task {linkedTask}</span>
          </div>
        )}

        {design != null && (
          <div style={descriptionStyle}>
            The Orchestrator pulled this screen so the matching task ships pixel-matched.
          </div>
        )}

        <button
          style={buttonStyle}
          onClick={onOpenInDesign}
          disabled={!design}
        >
          Open in Design
          <ArrowRight size={14} />
        </button>
      </div>
    </div>
  );
};
