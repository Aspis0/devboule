// HandoffModal — Phase D "Save & hand off to agents" modal. Prototype-faithful
// structure (Design-handoff/.../shell.jsx HandoffModal: modal-scrim/handoff,
// ho-head + Cpu + path, ho-flow Design->Repo->Agents wires, ho-tasks rows, ho-foot
// spinner/status + Open terminal + Done), driven by REAL phases via useHandoff.
//
// Single-dispatch: ONE "Coder agent" row, not the prototype's 5 mock tasks. The hook
// owns orchestration; this component is presentational — it renders steps, the
// project/client pickers (dispatch phase), and the footer actions, and forwards
// clicks. The scrim closes only when `closable` (never mid-dispatch).

import {
  Cpu,
  X,
  Palette,
  Save,
  Check,
  Loader,
  Code,
  FileText,
  Camera,
  Terminal,
  AlertTriangle,
  MinusCircle,
} from "lucide-react";

import type { ProjectSummary } from "../../../types/backend";
import type {
  HandoffClient,
  HandoffPhase,
  HandoffStep,
  HandoffStepIcon,
  HandoffFlowState,
} from "./useHandoff";

export interface HandoffModalProps {
  open: boolean;
  /** The design bundle path shown in the header (mono). */
  workingFolderPath: string;
  phase: HandoffPhase;
  steps: HandoffStep[];
  flow: HandoffFlowState;
  projects: ProjectSummary[];
  /** A hint surfaced near the project selector when the project list failed to load. */
  projectsError: string | null;
  selectedProjectId: string | null;
  client: HandoffClient;
  agentId: string | null;
  errorStage: "packaging" | "dispatch" | null;
  errorMessage: string | null;
  dispatching: boolean;
  canDispatch: boolean;
  closable: boolean;
  onClose: () => void;
  onSelectProject: (projectId: string) => void;
  onSelectClient: (client: HandoffClient) => void;
  onRetryPackaging: () => void;
  onDispatch: () => void;
  onOpenTerminal: () => void;
}

function StepIcon({ name }: { name: HandoffStepIcon }) {
  switch (name) {
    case "save":
      return <Save size={14} />;
    case "code":
      return <Code size={14} />;
    case "fileText":
      return <FileText size={14} />;
    case "camera":
      return <Camera size={14} />;
    case "cpu":
    default:
      return <Cpu size={14} />;
  }
}

// Map a step's status to the prototype's ho-task class + its leading icon. "running"
// and "done" are the prototype's two emphasized states; idle/warn/skipped/error are
// rendered with their own affordances on top of the idle (dimmed) visual.
function stepClass(status: HandoffStep["status"]): string {
  if (status === "running") return "ho-task running";
  if (status === "done") return "ho-task done";
  return "ho-task";
}

function StepLead({ step }: { step: HandoffStep }) {
  if (step.status === "done") return <Check size={14} />;
  if (step.status === "running")
    return (
      <span className="spin">
        <Loader size={14} />
      </span>
    );
  if (step.status === "warn") return <AlertTriangle size={14} />;
  if (step.status === "skipped") return <MinusCircle size={14} />;
  if (step.status === "error") return <AlertTriangle size={14} />;
  return <StepIcon name={step.icon} />;
}

export function HandoffModal(props: HandoffModalProps) {
  const {
    open,
    workingFolderPath,
    phase,
    steps,
    flow,
    projects,
    projectsError,
    selectedProjectId,
    client,
    agentId,
    errorStage,
    errorMessage,
    dispatching,
    canDispatch,
    closable,
    onClose,
    onSelectProject,
    onSelectClient,
    onRetryPackaging,
    onDispatch,
    onOpenTerminal,
  } = props;

  if (!open) return null;

  const done = phase === "done";
  const showDispatchControls = phase === "dispatch" && !done;

  return (
    <div
      className="modal-scrim"
      role="dialog"
      aria-modal="true"
      aria-label="Agent handoff"
      data-testid="handoff-modal"
      onClick={closable ? onClose : undefined}
    >
      <div
        className="handoff"
        data-screen-label="Agent handoff"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="ho-head">
          <span className="ho-ic">
            <Cpu size={18} />
          </span>
          <div className="ho-head-t">
            <b>Hand off to local agents</b>
            <span>{workingFolderPath || "No working folder"}</span>
          </div>
          {closable ? (
            <button
              type="button"
              className="ho-close"
              onClick={onClose}
              title="Close"
              aria-label="Close"
            >
              <X size={15} />
            </button>
          ) : null}
        </div>

        <div className="ho-flow">
          <div className="ho-step done">
            <span>
              <Palette size={16} />
            </span>
            Design
          </div>
          <div className="ho-wire" data-on={flow.repoDone} />
          <div className={"ho-step" + (flow.repoDone ? " done" : " run")}>
            <span>
              <Save size={16} />
            </span>
            Repo
          </div>
          <div className="ho-wire" data-on={flow.agentsStarted} />
          <div
            className={
              "ho-step" +
              (flow.done ? " done" : flow.agentsStarted ? " run" : "")
            }
          >
            <span>
              <Cpu size={16} />
            </span>
            Agents
          </div>
        </div>

        <div className="ho-tasks" data-testid="handoff-tasks">
          {steps.map((step) => (
            <div
              key={step.id}
              className={stepClass(step.status)}
              data-step={step.id}
              data-status={step.status}
            >
              <span className="ho-task-ic">
                <StepLead step={step} />
              </span>
              <div className="ho-task-body">
                <b>{step.label}</b>
                <span>{step.detail}</span>
              </div>
              <span className="ho-task-agent">{step.agent}</span>
            </div>
          ))}
        </div>

        {showDispatchControls ? (
          <div className="ho-dispatch" data-testid="handoff-dispatch-controls">
            <label className="ho-field">
              <span className="ho-field-l">Project</span>
              <select
                className="ho-select"
                data-testid="handoff-project-select"
                value={selectedProjectId ?? ""}
                onChange={(e) => onSelectProject(e.target.value)}
              >
                <option value="" disabled>
                  Choose a project…
                </option>
                {projects.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.title}
                  </option>
                ))}
              </select>
              {projectsError ? (
                <span
                  className="ho-field-err"
                  role="alert"
                  data-testid="handoff-projects-error"
                >
                  {projectsError}
                </span>
              ) : null}
            </label>
            <label className="ho-field">
              <span className="ho-field-l">Agent CLI</span>
              <select
                className="ho-select"
                data-testid="handoff-client-select"
                value={client}
                onChange={(e) =>
                  onSelectClient(e.target.value as HandoffClient)
                }
              >
                <option value="claude">claude</option>
                <option value="codex">codex</option>
              </select>
            </label>
          </div>
        ) : null}

        {errorMessage ? (
          <div className="ho-error" role="alert" data-testid="handoff-error">
            <span>{errorMessage}</span>
            <button
              type="button"
              className="btn btn-ghost"
              data-testid="handoff-retry"
              onClick={
                errorStage === "dispatch" ? onDispatch : onRetryPackaging
              }
            >
              Retry
            </button>
          </div>
        ) : null}

        <div className="ho-foot">
          {done ? (
            <>
              <span className="ho-foot-note ok">
                <span className="dot" />
                {agentId
                  ? `${agentId} running in your repo`
                  : "Agent running in your repo"}
              </span>
              <button
                type="button"
                className="btn btn-ghost"
                data-testid="handoff-open-terminal"
                onClick={onOpenTerminal}
              >
                <Terminal size={15} />
                Open terminal
              </button>
              <button
                type="button"
                className="btn btn-primary"
                onClick={onClose}
              >
                Done
              </button>
            </>
          ) : phase === "dispatch" ? (
            <>
              <span className="ho-foot-note">
                {dispatching ? (
                  <>
                    <span className="spin">
                      <Loader size={14} />
                    </span>
                    Dispatching the coder agent…
                  </>
                ) : (
                  "Pick a project, then dispatch the coder agent."
                )}
              </span>
              <button
                type="button"
                className="btn btn-primary"
                data-testid="handoff-dispatch"
                disabled={!canDispatch}
                onClick={onDispatch}
              >
                <Cpu size={15} />
                Dispatch agent
              </button>
            </>
          ) : (
            <span className="ho-foot-note">
              <span className="spin">
                <Loader size={14} />
              </span>
              Packaging project &amp; design tokens…
            </span>
          )}
        </div>
      </div>
    </div>
  );
}

export default HandoffModal;
