import type { AgentSession, ProjectTask } from "../../types/backend";

export type WorkNodeType = "orchestrator" | "coder" | "mini" | "censor";

export interface WorkNode {
  agentId: string;
  type: WorkNodeType;
  file: string | null;
  district: string;
  status: string;
  label: string;
  parentAgentId: string | null;
  taskId: string | null;
  pendingQuestion: string | null;
  live: boolean;
  children: WorkNode[];
}

export interface WorkDistrict {
  name: string;
  nodes: WorkNode[];
}

export interface WorkConsoleModel {
  orchestrator: WorkNode | null;
  districts: WorkDistrict[];
  unplaced: WorkNode[];
}

const TERMINAL_STATUSES = new Set(["idle","done","exited","stopped","error","completed"]);

function deriveDistrict(file: string | null): string {
  if (!file) return "unplaced";
  const normalized = file.replace(/\\/g, "/").replace(/^\/+/, "");
  const parts = normalized.split("/");
  if (parts.length < 2) return "unplaced";
  const parent = parts[parts.length - 2];
  if (!parent) return "unplaced";
  return parent;
}

function buildNode(session: AgentSession, taskMap: Record<string, ProjectTask>, type: WorkNodeType): WorkNode {
  const file = session.currentTaskId ? (taskMap[session.currentTaskId]?.scope?.[0] ?? null) : null;
  const district = deriveDistrict(file);

  const roleWord = type;
  const label = session.client ? `${roleWord} · ${session.client}` : roleWord;

  const statusLower = (session.status ?? "").toLowerCase();
  const live = statusLower.length > 0 && !TERMINAL_STATUSES.has(statusLower);

  return {
    agentId: session.agentId,
    type,
    file,
    district,
    status: session.status,
    label,
    parentAgentId: session.parentAgentId || null,
    taskId: session.currentTaskId || null,
    pendingQuestion: session.pendingQuestion?.question ?? null,
    live,
    children: [],
  };
}

export function buildWorkConsoleModel({
  sessions,
  tasks,
  projectId,
}: {
  sessions: AgentSession[];
  tasks: ProjectTask[];
  projectId: string;
}): WorkConsoleModel {
  const taskMap: Record<string, ProjectTask> = {};
  for (const t of tasks) {
    taskMap[t.id] = t;
  }

  const inProjectSessions = sessions.filter((s) => s.currentProjectId === projectId);

  let orchestratorNode: WorkNode | null = null;
  const topNodes: WorkNode[] = [];
  const miniNodes: WorkNode[] = [];

  for (const session of inProjectSessions) {
    let type: WorkNodeType;
    if (session.client === "orchestrator" || session.role === "orchestrator") {
      type = "orchestrator";
    } else if (session.role === "censor" || session.client === "censor") {
      type = "censor";
    } else if (session.parentAgentId) {
      type = "mini";
    } else {
      type = "coder";
    }

    const node = buildNode(session, taskMap, type);

    if (type === "orchestrator") {
      if (!orchestratorNode) {
        orchestratorNode = node;
      } else {
        node.type = "coder";
        topNodes.push(node);
      }
    } else if (type === "mini") {
      miniNodes.push(node);
    } else {
      topNodes.push(node);
    }
  }

  const allNodes: WorkNode[] = [];
  if (orchestratorNode) allNodes.push(orchestratorNode);
  allNodes.push(...topNodes);
  allNodes.push(...miniNodes);

  const nodeMap = new Map<string, WorkNode>();
  for (const node of allNodes) {
    nodeMap.set(node.agentId, node);
  }

  const attachedMiniAgentIds = new Set<string>();
  for (const mini of miniNodes) {
    const parentId = mini.parentAgentId;
    if (parentId) {
      const parent = nodeMap.get(parentId);
      if (parent && parent.agentId !== mini.agentId) {
        parent.children.push(mini);
        attachedMiniAgentIds.add(mini.agentId);
      }
    }
  }

  const districtMap = new Map<string, WorkNode[]>();
  for (const node of allNodes) {
    if (node === orchestratorNode) continue;
    if (attachedMiniAgentIds.has(node.agentId)) continue;

    const district = node.district;
    if (!districtMap.has(district)) {
      districtMap.set(district, []);
    }
    districtMap.get(district)!.push(node);
  }

  const districts: WorkDistrict[] = [];
  for (const [name, nodes] of districtMap) {
    if (name === "unplaced") continue;
    districts.push({ name, nodes });
  }
  districts.sort((a, b) => a.name.localeCompare(b.name, "en", { sensitivity: "base" }));

  const unplaced = districtMap.get("unplaced") || [];

  return {
    orchestrator: orchestratorNode,
    districts,
    unplaced,
  };
}

/** Find a node anywhere in the model (orchestrator + its children, district nodes +
 *  children recursively, unplaced) by agentId. Returns null if absent. */
export function findWorkNode(model: WorkConsoleModel, agentId: string): WorkNode | null {
  const search = (nodes: readonly WorkNode[]): WorkNode | null => {
    for (const node of nodes) {
      if (node.agentId === agentId) return node;
      const found = search(node.children);
      if (found) return found;
    }
    return null;
  };
  return (
    search(model.orchestrator ? [model.orchestrator] : []) ||
    search(model.districts.flatMap((d) => d.nodes)) ||
    search(model.unplaced)
  );
}
