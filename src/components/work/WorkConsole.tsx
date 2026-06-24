import "./work.css";
import { Suspense, lazy, useEffect, useMemo, useState } from "react";
import { buildWorkConsoleModel, findWorkNode } from "./workConsoleModel";
import { LivingPlan } from "./LivingPlan";
import { FocusStage } from "./FocusStage";
import { agentChannel, type CommsDirection } from "./agentChannel";
import { isMiniManagedSession } from "../projects/projectWorkspaceModel";
import { stripSpoofChars } from "../agents/attentionNotifier";
import { useAgentConsole } from "../agents/useAgentConsole";
import { invokeBackendCommand } from "../../context/AppContext";
import type { AgentSession, ProjectTask } from "../../types/backend";

const AgentTerminalViewer = lazy(() =>
  import("../agents/AgentTerminalViewer").then((m) => ({ default: m.AgentTerminalViewer })),
);

export interface WorkConsoleProps {
  sessions: AgentSession[];
  tasks: ProjectTask[];
  projectId: string;
  ptyAgentIds: Set<string>;
  selectedAgentId: string | null;
  onSelectAgent: (agentId: string) => void;
  readOnly?: boolean;
  dirtyAgentIds?: Set<string>;
}

export function WorkConsole(props: WorkConsoleProps) {
  const { sessions, tasks, projectId, ptyAgentIds, selectedAgentId, onSelectAgent, readOnly, dirtyAgentIds } = props;

  const model = useMemo(() => buildWorkConsoleModel({ sessions, tasks, projectId }), [sessions, tasks, projectId]);
  
  const selectedNode = selectedAgentId ? findWorkNode(model, selectedAgentId) : null;
  
  const [view, setView] = useState<"activity" | "raw">("activity");
  // Reset to Activity when the selection changes, so a Raw view never leaks across agents.
  useEffect(() => {
    setView("activity");
  }, [selectedAgentId]);
  
  const activity = useAgentConsole(selectedAgentId);
  
  // isPty gates the RAW terminal mount (any app-hosted PTY agent); the COMMS channel is
  // chosen by whether the agent is mini_coder-managed (local) vs a cloud PTY worker.
  const isPty = selectedNode ? ptyAgentIds.has(selectedNode.agentId) : false;
  const selectedSession = selectedAgentId
    ? sessions.find((s) => s.agentId === selectedAgentId) ?? null
    : null;
  const miniManaged = selectedSession ? isMiniManagedSession(selectedSession) : true;
  const pendingQuestion = selectedNode?.pendingQuestion
    ? stripSpoofChars(selectedNode.pendingQuestion)
    : null;

  const dispatch = (text: string, dir: CommsDirection) => {
    const t = text.trim();
    if (!t || !selectedNode) return;
    const ch = agentChannel(selectedNode, { miniManaged }, dir);
    if (!ch) return;
    void invokeBackendCommand(ch.command, ch.buildArgs(t)).catch(() => {});
  };

  const onSendMessage = (t: string) => dispatch(t, "message");
  const onAnswer = (t: string) => dispatch(t, "answer");

  const QUICK = { redo: "Redo this round.", narrow: "Narrow the scope to the current file only.", pause: "Pause after the current step." };
  const onQuickAction = (a: "redo" | "narrow" | "pause") => dispatch(QUICK[a], "message");

  const rawSlot = isPty && selectedNode
    ? <Suspense fallback={<div style={{padding:24,textAlign:'center',color:'#9c9488',fontSize:12}}>Loading terminal…</div>}>
        <AgentTerminalViewer key={selectedNode.agentId} agentId={selectedNode.agentId} />
      </Suspense>
    : <div style={{display:'flex',height:'100%',alignItems:'center',justifyContent:'center',color:'#9c9488',fontSize:12,textAlign:'center',padding:16}}>This agent runs in an external console — no in-app terminal to show.</div>;

  return (
    <div style={{ height: 600, border: "1px solid #E4DDD0", borderRadius: 12, overflow: "hidden", display: "flex", flexDirection: "row" }}>
      <div style={{ flex: "none", width: 480, borderRight: "1px solid #EFE7DA" }}>
        <LivingPlan model={model} selectedAgentId={selectedAgentId} onSelect={onSelectAgent} dirtyAgentIds={dirtyAgentIds} />
      </div>
      <div style={{ flex: 1, minWidth: 0 }}>
        {selectedNode ? (
          <FocusStage 
            node={selectedNode} 
            activity={activity} 
            view={view} 
            onViewChange={setView} 
            onSendMessage={onSendMessage} 
            pendingQuestion={pendingQuestion} 
            onAnswer={onAnswer} 
            rawSlot={rawSlot} 
            disabled={!!readOnly} 
            onQuickAction={onQuickAction} 
          />
        ) : (
          <div style={{ display: 'flex', height: '100%', alignItems: 'center', justifyContent: 'center', color: '#9c9488', fontSize: 14, textAlign: 'center', padding: 16 }}>
            Select an agent on the left to focus it.
          </div>
        )}
      </div>
    </div>
  );
}
