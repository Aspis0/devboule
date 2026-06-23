import { Check } from "lucide-react";

interface PlannerControlsProps {
  coders: { id: string; label: string }[];
  coderId: string;
  onCoderChange: (id: string) => void;
  autoCreate: boolean;
  onAutoCreateToggle: () => void;
}

export function PlannerControls(props: PlannerControlsProps) {
  const { coders, coderId, onCoderChange, autoCreate, onAutoCreateToggle } = props;

  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 8,
        flexWrap: 'wrap',
        padding: '9px 12px',
        background: '#F4F0E9',
        border: '1px solid #E4DDD0',
        borderRadius: 10,
      }}
    >
      <span className="pp-mono" style={{
        fontSize: 9.5,
        letterSpacing: '.14em',
        color: '#A89F90',
        fontWeight: 600,
      }}>
        HAND OFF TO
      </span>

      {coders.length === 0 ? (
        <span style={{ fontSize: 11, color: '#B3AB9C' }}>
          No coders configured — add one in Settings
        </span>
      ) : (
        coders.map((c) => {
          const isActive = coderId === c.id;
          return (
            <button
              key={c.id}
              onClick={() => onCoderChange(c.id)}
              style={{
                display: 'inline-flex',
                alignItems: 'center',
                gap: 6,
                padding: '5px 11px',
                borderRadius: 8,
                fontSize: 11.5,
                fontWeight: 600,
                cursor: 'pointer',
                ...(isActive
                  ? {
                      border: '1px solid #C0894F',
                      background: '#fff',
                      color: '#2A2621',
                      boxShadow: '0 1px 3px rgba(0,0,0,.08)',
                    }
                  : {
                      border: '1px solid #E4DDD0',
                      background: '#fff',
                      color: '#7c766b',
                    }),
              }}
            >
              <span
                style={{
                  width: 16,
                  height: 16,
                  background: '#F1E4D2',
                  color: '#9A6A2E',
                  borderRadius: 4,
                  fontSize: 9,
                  fontWeight: 700,
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'center',
                }}
              >
                {c.label[0]?.toUpperCase()}
              </span>
              {c.label}
            </button>
          );
        })
      )}

      <button
        onClick={onAutoCreateToggle}
        style={{
          marginLeft: 'auto',
          display: 'inline-flex',
          alignItems: 'center',
          gap: 6,
          padding: '5px 11px',
          borderRadius: 8,
          fontSize: 11,
          fontWeight: 600,
          cursor: 'pointer',
          ...(autoCreate
            ? {
                border: '1px solid #B7D9A8',
                background: '#F0F6EC',
                color: '#4E7C3C',
              }
            : {
                border: '1px solid #E4DDD0',
                background: '#fff',
                color: '#9c9488',
              }),
        }}
        title={
          autoCreate
            ? 'When you approve the plan, its tasks are created on the board automatically.'
            : 'The plan is drafted but its tasks are not created — you create them.'
        }
      >
        {autoCreate && <Check size={12} />}
        auto-create tasks: {autoCreate ? 'on' : 'off'}
      </button>
    </div>
  );
}
