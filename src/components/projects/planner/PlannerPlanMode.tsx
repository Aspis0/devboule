import { useRef, useEffect } from "react";
import gsap from "gsap";
import { Search, ListOrdered, LayoutDashboard } from "lucide-react";
import "./planner.css";
import { useStageRotation } from "./useStageRotation";
import { stripLabel } from "./plannerModel";
import type { PlanCard, StagePage, StageFinding, PlannerMessage } from "./plannerModel";
import { StageWebsearch } from "./StageWebsearch";
import { StagePlan } from "./StagePlan";
import { StageDesign } from "./StageDesign";
import { PlannerChat } from "./PlannerChat";
import { PlannerControls } from "./PlannerControls";

interface PlannerPlanModeProps {
  projectName: string | null;
  hasRoot: boolean;
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
  // Hand-off + auto-create controls (preserved from the old composer — never strip choices).
  coders: { id: string; label: string }[];
  coderId: string;
  onCoderChange: (id: string) => void;
  autoCreate: boolean;
  onAutoCreateToggle: () => void;
}

export function PlannerPlanMode(props: PlannerPlanModeProps) {
  const {
    projectName,
    hasRoot,
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
    coders,
    coderId,
    onCoderChange,
    autoCreate,
    onAutoCreateToggle,
  } = props;

  const { view, auto, pick, toggleAuto } = useStageRotation();
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!ref.current) return;
    const tween = gsap.from(ref.current, {
      scaleY: 0.6,
      opacity: 0,
      transformOrigin: 'top',
      duration: 0.35,
      ease: 'power2.out',
    });
    return () => {
      tween.kill();
    };
  }, []);

  const labels: Array<'searching' | 'planning' | 'designing'> = ['searching', 'planning', 'designing'];
  const currentLabel = stripLabel(view);

  return (
    <div
      ref={ref}
      className="pp-root"
      style={{
        background: '#0E0D0B',
        borderRadius: 14,
        padding: '30px 6px 6px',
        boxShadow: '0 28px 64px -24px rgba(0,0,0,.45)',
        position: 'relative',
      }}
    >
      {/* Traffic Lights */}
      <div
        style={{
          position: 'absolute',
          top: 9,
          left: 12,
          display: 'flex',
          gap: 7,
        }}
      >
        <div style={{ width: 10, height: 10, borderRadius: '50%', background: '#E0664E' }} />
        <div style={{ width: 10, height: 10, borderRadius: '50%', background: '#E3A93C' }} />
        <div style={{ width: 10, height: 10, borderRadius: '50%', background: '#54A867' }} />
      </div>

      {/* Title */}
      <div
        className="pp-mono"
        style={{
          position: 'absolute',
          top: 8,
          left: 0,
          right: 0,
          textAlign: 'center',
          fontSize: 10.5,
          color: '#6b655d',
        }}
      >
        devboule · planner
      </div>

      {/* Inner Cream Panel */}
      <div
        style={{
          background: '#F4F0E9',
          borderRadius: 10,
          padding: 16,
          display: 'flex',
          flexDirection: 'column',
          gap: 13,
        }}
      >
        {/* 1) Goal Echo Row */}
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
            {goal ??
              (hasRoot
                ? `Describe a goal below to plan ${projectName ?? 'this project'}…`
                : 'Select a project with a working folder to start planning.')}
          </span>
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
        </div>

        {/* 2) State Strip */}
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 7,
          }}
        >
          {labels.map((label) => {
            const isActive = currentLabel === label;
            return (
              <div
                className="pp-mono"
                key={label}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 6,
                  fontSize: 10.5,
                  borderRadius: 8,
                  padding: '5px 9px',
                  fontWeight: isActive ? 600 : 400,
                  color: isActive ? '#9A6A2E' : '#A89F90',
                  background: isActive ? '#F1E4D2' : '#fff',
                  border: isActive ? '1px solid #E6D3BB' : '1px solid #ECE6DB',
                  animation: isActive ? 'pp-pulse 1.9s infinite' : 'none',
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
                <span>{label}</span>
              </div>
            );
          })}
        </div>

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

        {/* 4) Chat */}
        <PlannerChat
          messages={messages}
          modelLabel={`Orchestrator · ${plannerModelLabel}`}
          live={live}
          awaitingReply={awaitingReply}
          onSend={onSend}
        />

        {/* 5) Hand-off + auto-create controls (preserved choices) */}
        <PlannerControls
          coders={coders}
          coderId={coderId}
          onCoderChange={onCoderChange}
          autoCreate={autoCreate}
          onAutoCreateToggle={onAutoCreateToggle}
        />
      </div>
    </div>
  );
}
