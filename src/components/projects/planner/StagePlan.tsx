import type { PlanCard } from "./plannerModel";
import { Check } from "lucide-react";

export const StagePlan = ({ cards }: { cards: PlanCard[] }) => {
  return (
    <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minHeight: 0 }} className="pp-view-enter">
      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
        <span className="pp-mono" style={{ fontSize: 9.5, letterSpacing: '.14em', color: '#A89F90' }}>PLAN</span>
        <span className="pp-mono" style={{ color: '#C0894F' }}>{cards.length} tasks</span>
      </div>
      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 7 }}>
        {cards.map((card, index) => {
          const isDone = card.state === 'done';
          const isForming = card.state === 'forming';
          const delay = (index * 0.15 + 0.1) + 's';

          let cardStyle: React.CSSProperties = {
            display: 'flex',
            alignItems: 'center',
            gap: 9,
          };

          if (isDone) {
            cardStyle = {
              ...cardStyle,
              background: '#fff',
              border: '1px solid #E6D8C2',
              borderRadius: 9,
              padding: '8px 10px',
            };
          } else if (isForming) {
            cardStyle = {
              ...cardStyle,
              position: 'relative',
              background: '#FCFAF6',
              border: '1px dashed #D7C6AA',
              borderRadius: 9,
              padding: '8px 10px',
              overflow: 'hidden',
            };
          } else {
            cardStyle = {
              ...cardStyle,
              border: '1px dashed #E0D6C5',
              borderRadius: 9,
              padding: '8px 10px',
            };
          }
          // EVERY card crystallizes in (staggered) when the Plan view mounts — not just
          // 'done' ones, so a fresh plan's todo/wip cards also appear gradually. The
          // pending look comes from its dim border/badge/text colors (not opacity, which
          // pp-crystal's 0→1 would override).
          cardStyle.animation = `pp-crystal .6s ease-out both ${delay}`;

          const badgeBg = isDone ? '#C0894F' : isForming ? '#F1E4D2' : '#F1ECE2';
          const badgeColor = isDone ? '#FBF6EF' : isForming ? '#C0894F' : '#B3AB9C';
          const titleColor = isDone ? '#3B362F' : isForming ? '#9c8d77' : '#B3AB9C';

          const badgeStyle: React.CSSProperties = {
            width: 18,
            height: 18,
            flex: 'none',
            borderRadius: 5,
            fontSize: 10,
            fontWeight: 700,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            background: badgeBg,
            color: badgeColor,
          };

          return (
            <div key={card.n} style={cardStyle}>
              <span className="pp-mono" style={badgeStyle}>{card.n}</span>
              <span style={{ fontSize: 11.5, color: titleColor }}>{card.title}</span>
              {isDone && <Check size={13} color="#5E9A86" style={{ marginLeft: 'auto' }} />}
              {isForming && (
                <span style={{ position: 'absolute', inset: 0, background: 'linear-gradient(90deg,transparent,rgba(192,137,79,.16),transparent)', backgroundSize: '200% 100%', animation: 'pp-shimmer 1.8s linear infinite' }} />
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
};
