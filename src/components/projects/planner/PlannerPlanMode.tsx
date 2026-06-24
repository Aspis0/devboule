import { useRef, useEffect } from "react";
import gsap from "gsap";
import { Search, ListOrdered, LayoutDashboard } from "lucide-react";
import "./planner.css";
import { useStageRotation } from "./useStageRotation";
import type { PlanCard, StagePage, StageFinding, PlannerMessage } from "./plannerModel";
import { StageWebsearch } from "./StageWebsearch";
import { StagePlan } from "./StagePlan";
import { StageDesign } from "./StageDesign";
import { PlannerChat } from "./PlannerChat";
import { PlannerControls } from "./PlannerControls";
import { AgentTerminalViewer } from "../../agents/AgentTerminalViewer";

interface PlannerPlanModeProps {
  goal: string | null;
  contextLabel: string;
  plannerModelLabel: string;
  live: boolean;
  planCards: PlanCard[];
  pages: StagePage[];
  findings: StageFinding[];
  webMode: 'auto' | 'manual';
  onWebModeChange: (m: 'auto' | 'manual') => void;
  onManualSearch: (q: string) => void;
  design: { name: string; version: string | null; ago: string | null; thumbnailUri: string | null } | null;
  linkedTask: number | null;
  onOpenInDesign: () => void;
  messages: PlannerMessage[];
  awaitingReply: boolean;
  onSend: (text: string) => void;
  // Orchestrator backend selector — who you TALK TO (the planner). Replaces the redundant
  // status strip (searching/planning/designing duplicated the view tabs). The active one
  // pulses. Local = our Stage/TUI; Claude/Codex run their own CLI (their terminal is shown).
  orchestrators: { id: string; label: string }[];
  orchestratorId: string;
  onOrchestratorChange: (id: string) => void;
  // When a CLOUD orchestrator (Claude/Codex) is running, its agent id — we show ITS terminal
  // instead of the local Stage (the Stage bridges are local-devboule only). null = local/none.
  cloudTerminalAgentId: string | null;
  // Hand-off + auto-create controls (preserved from the old composer — never strip choices).
  coders: { id: string; label: string }[];
  coderId: string;
  onCoderChange: (id: string) => void;
  autoCreate: boolean;
  onAutoCreateToggle: () => void;
  // B10: explicit "Create plan" trigger (discuss-first; plan on demand).
  onCreatePlan?: () => void;
  canCreatePlan?: boolean;
}

export function PlannerPlanMode(props: PlannerPlanModeProps) {
  const {
    goal,
    contextLabel,
    plannerModelLabel,
    live,
    planCards,
    pages,
    findings,
    webMode,
    onWebModeChange,
    onManualSearch,
    design,
    linkedTask,
    onOpenInDesign,
    messages,
    awaitingReply,
    onSend,
    orchestrators,
    orchestratorId,
    onOrchestratorChange,
    cloudTerminalAgentId,
    coders,
    coderId,
    onCoderChange,
    autoCreate,
    onAutoCreateToggle,
    onCreatePlan,
    canCreatePlan,
  } = props;

  const { view, auto, pick, toggleAuto } = useStageRotation(3800, live);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    // fromTo with EXPLICIT end values (not gsap.from, which reads the current value
    // as the destination): under React StrictMode the effect runs twice and the
    // cleanup kills the first tween mid-flight at opacity:0 — gsap.from would then
    // animate 0 -> 0 and leave the panel invisible. fromTo always ends visible.
    const tween = gsap.fromTo(
      el,
      { scaleY: 0.6, opacity: 0 },
      {
        scaleY: 1,
        opacity: 1,
        transformOrigin: 'top',
        duration: 0.35,
        ease: 'power2.out',
      },
    );
    return () => {
      tween.kill();
      // Guarantee the panel is left visible even if killed mid-flight.
      gsap.set(el, { clearProps: 'opacity,transform' });
    };
  }, []);

  return (
    <div
      ref={ref}
      className="pp-root rounded-2xl border border-cream-200 bg-white shadow-sm"
      style={{ padding: 16 }}
    >
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          gap: 13,
        }}
      >
        {/* 1) Goal echo — shown only while a REAL goal is being planned. No fake
        placeholder when idle (the chat composer carries the "describe a goal" affordance). */}
        {goal && (
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 10,
              padding: '10px 12px',
              background: '#FCFAF6',
              border: '1px solid #E4DDD0',
              borderRadius: 12,
            }}
          >
            <div
              className="pp-mono"
              style={{
                fontSize: 12,
                fontWeight: 600,
                color: '#9A6A2E',
                background: '#F1E4D2',
                border: '1px solid #E6D3BB',
                padding: '4px 11px',
                borderRadius: 8,
              }}
            >
              plan
            </div>
            <span
              style={{
                flex: 1,
                fontSize: 13.5,
                color: '#2A2621',
                whiteSpace: 'nowrap',
                overflow: 'hidden',
                textOverflow: 'ellipsis',
              }}
            >
              {goal}
            </span>
            {contextLabel && (
              <div
                className="pp-mono"
                style={{
                  fontSize: 11,
                  color: '#7c766b',
                  background: '#fff',
                  border: '1px solid #E9E3D8',
                  padding: '4px 8px',
                  borderRadius: 7,
                }}
              >
                {contextLabel}
              </div>
            )}
          </div>
        )}

        {/* 2) Orchestrator selector — WHO YOU TALK TO. Replaces the old status strip
            (searching/planning/designing duplicated the view tabs below). The active backend
            pulses while it's live. Local = our Stage; Claude/Codex run their own CLI. */}
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 7,
          }}
        >
          <span
            className="pp-mono"
            style={{ fontSize: 10, color: '#A89F90', marginRight: 1, cursor: 'help' }}
            data-help-title="The orchestrator is the agent you talk to here to shape the plan."
            data-help-lines="A project is a plan: you discuss the goal with the orchestrator, and it drafts the tasks.|Local (Devboule) keeps this rich Stage (chat + websearch + plan + design); Claude/Codex run in their own embedded terminal.|Once the plan is ready you hand it off to a main coder, which writes the code.|It is the create-time conversation — not a permanent agent of the project."
          >
            orchestrator
          </span>
          {orchestrators.map((o) => {
            const isActive = o.id === orchestratorId;
            return (
              <button
                type="button"
                className="pp-mono"
                key={o.id}
                onClick={() => onOrchestratorChange(o.id)}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 6,
                  fontSize: 10.5,
                  borderRadius: 8,
                  padding: '5px 9px',
                  cursor: 'pointer',
                  fontWeight: isActive ? 600 : 400,
                  color: isActive ? '#9A6A2E' : '#A89F90',
                  background: isActive ? '#F1E4D2' : '#fff',
                  border: isActive ? '1px solid #E6D3BB' : '1px solid #ECE6DB',
                  animation: isActive && live ? 'pp-pulse 1.9s infinite' : 'none',
                }}
              >
                <div
                  style={{
                    width: 6,
                    height: 6,
                    borderRadius: '50%',
                    background: isActive ? '#C0894F' : '#CFC6B6',
                  }}
                />
                <span>{o.label}</span>
              </button>
            );
          })}
        </div>

        {cloudTerminalAgentId ? (
          /* Cloud orchestrator (Claude/Codex) runs its OWN CLI — show ITS terminal here. The
             Stage bridges (chat/websearch/design events) are local-devboule only (Phase D), so
             for a cloud orchestrator the terminal IS the interaction. Bounded; scrolls internally. */
          <div
            style={{
              height: 316,
              borderRadius: 12,
              overflow: 'hidden',
              border: '1px solid #ECE6DB',
            }}
          >
            <AgentTerminalViewer agentId={cloudTerminalAgentId} />
          </div>
        ) : (
          <>
        {/* 3) Stage Container */}
        <div
          style={{
            background: '#FAF7F1',
            border: '1px solid #ECE6DB',
            borderRadius: 12,
            padding: 13,
            height: 316,
            display: 'flex',
            flexDirection: 'column',
          }}
        >
          {/* Tab Row */}
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 9,
              marginBottom: 12,
            }}
          >
            <div
              style={{
                display: 'flex',
                background: '#F1E9DC',
                borderRadius: 10,
                padding: 3,
              }}
            >
              {[
                { v: 'exa' as const, icon: Search, label: 'Websearch' },
                { v: 'plan' as const, icon: ListOrdered, label: 'Plan' },
                { v: 'design' as const, icon: LayoutDashboard, label: 'Design' },
              ].map(({ v, icon: Icon, label }) => {
                const isActive = view === v;
                return (
                  <div
                    key={v}
                    onClick={() => pick(v)}
                    className="pp-mono"
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 6,
                      padding: '6px 12px',
                      borderRadius: 8,
                      fontSize: 12,
                      fontWeight: 600,
                      cursor: 'pointer',
                      background: isActive ? '#C8945C' : 'transparent',
                      color: isActive ? '#FBF6EF' : '#9c9488',
                    }}
                  >
                    <Icon size={13} />
                    <span>{label}</span>
                  </div>
                );
              })}
            </div>

            {/* Auto Toggle */}
            <div
              onClick={toggleAuto}
              className="pp-mono"
              style={{
                marginLeft: 'auto',
                fontSize: 9,
                cursor: 'pointer',
                padding: '3px 8px',
                borderRadius: 7,
                display: 'flex',
                alignItems: 'center',
                gap: 5,
                ...(auto
                  ? { color: '#B3AB9C' }
                  : { color: '#9A6A2E', background: '#F1E4D2', border: '1px solid #E6D3BB' }),
              }}
            >
              <div
                style={{
                  width: 5,
                  height: 5,
                  borderRadius: '50%',
                  background: auto ? '#B3AB9C' : '#C0894F',
                }}
              />
              <span>{auto ? 'auto' : 'paused · resume'}</span>
            </div>
          </div>

          {/* Active View */}
          <div style={{ flex: 1, overflow: 'hidden' }}>
            {view === 'exa' && (
              <StageWebsearch
                pages={pages}
                findings={findings}
                mode={webMode}
                live={live}
                onModeChange={onWebModeChange}
                onManualSearch={onManualSearch}
              />
            )}
            {view === 'plan' && <StagePlan cards={planCards} />}
            {view === 'design' && (
              <StageDesign
                design={design}
                linkedTask={linkedTask}
                onOpenInDesign={onOpenInDesign}
              />
            )}
          </div>
        </div>

        {/* 4) Chat (local Stage / pre-launch composer; the cloud terminal replaces it above) */}
        <PlannerChat
          messages={messages}
          modelLabel={`Orchestrator · ${plannerModelLabel}`}
          live={live}
          awaitingReply={awaitingReply}
          onSend={onSend}
        />
          </>
        )}

        {/* 5) Hand-off + auto-create controls (preserved choices) */}
        <PlannerControls
          coders={coders}
          coderId={coderId}
          onCoderChange={onCoderChange}
          autoCreate={autoCreate}
          onAutoCreateToggle={onAutoCreateToggle}
          onCreatePlan={onCreatePlan}
          canCreatePlan={canCreatePlan}
        />
      </div>
    </div>
  );
}
