import { Check } from "lucide-react";

interface PlannerControlsProps {
  coders: { id: string; label: string }[];
  coderId: string;
  onCoderChange: (id: string) => void;
  autoCreate: boolean;
  onAutoCreateToggle: () => void;
  // B10: explicit "Create plan" trigger. The orchestrator discusses first (no
  // auto-plan on turn 1); the user clicks this when the conversation has converged
  // to draft the plan + create the tasks. Enabled only while an orchestrator is live.
  onCreatePlan?: () => void;
  canCreatePlan?: boolean;
}

export function PlannerControls(props: PlannerControlsProps) {
  const {
    coders,
    coderId,
    onCoderChange,
    autoCreate,
    onAutoCreateToggle,
    onCreatePlan,
    canCreatePlan,
  } = props;

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
        cursor: 'help',
      }}
        data-help-title="The main coder is the agent that builds the plan into code."
        data-help-lines="After the orchestrator drafts the plan, it hands the tasks off to this coder.|It can be Local (Devboule), Claude, or Codex — the same three backends as the orchestrator.|The coder delegates one-shot edits to mini-coders and moves tasks toward Review.|Pick the backend that fits the job's difficulty and your token budget."
      >
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

      {onCreatePlan && (
        <button
          onClick={onCreatePlan}
          disabled={!canCreatePlan}
          title={
            canCreatePlan
              ? 'Draft the plan from the conversation and create the tasks on the board.'
              : 'Start the orchestrator (describe a goal) before creating the plan.'
          }
          data-help-title="Create the plan when the conversation has converged."
          data-help-lines="The orchestrator discusses the goal with you first — it does NOT plan on the first message.|Click this when you're happy with the direction: it asks the orchestrator to draft the plan and create the Kanban tasks.|Use the auto-create toggle instead if you want it to plan + create eagerly without waiting for this click."
          style={{
            marginLeft: 'auto',
            display: 'inline-flex',
            alignItems: 'center',
            gap: 6,
            padding: '5px 13px',
            borderRadius: 8,
            fontSize: 11.5,
            fontWeight: 700,
            cursor: canCreatePlan ? 'pointer' : 'not-allowed',
            border: '1px solid #C0894F',
            background: canCreatePlan ? '#C0894F' : '#EDE6DA',
            color: canCreatePlan ? '#fff' : '#B3AB9C',
          }}
        >
          Create plan
        </button>
      )}

      <button
        onClick={onAutoCreateToggle}
        style={{
          marginLeft: onCreatePlan ? 0 : 'auto',
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
