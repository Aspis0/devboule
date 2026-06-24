import "./work.css";
import type { WorkConsoleModel, WorkNode } from "./workConsoleModel";

export interface LivingPlanProps {
  model: WorkConsoleModel;
  selectedAgentId: string | null;
  onSelect: (agentId: string) => void;
  dirtyAgentIds?: Set<string>;
}

const typeColors: Record<WorkNode["type"], string> = {
  orchestrator: "#2F7E7A",
  coder: "#C0894F",
  mini: "#5B6CC0",
  censor: "#C8945C",
};

export function LivingPlan(props: LivingPlanProps) {
  const { model, selectedAgentId, onSelect, dirtyAgentIds } = props;

  const getMarkerStyle = (node: WorkNode) => {
    const isDirty = dirtyAgentIds?.has(node.agentId);
    const isLive = node.live;
    let bg = typeColors[node.type];
    let border = "none";
    let br = node.type === "mini" ? "2px" : "50%";
    let anim = "";

    if (node.type === "censor") {
      bg = "transparent";
      border = "2px solid #EBD9BF";
      anim = node.live ? "wc-spin 1s linear infinite" : "";
    } else if (isDirty) {
      border = "1.5px solid #C2542F";
      anim = "wc-coral 1.8s infinite";
    } else if (isLive) {
      const liveAnim =
        node.type === "coder"
          ? "wc-terra"
          : node.type === "mini"
            ? "wc-indigo"
            : "wc-teal";
      anim = `${liveAnim} 1.9s infinite`;
    }

    return {
      width: "9px",
      height: "9px",
      flex: "none",
      borderRadius: br,
      background: bg,
      border,
      animation: anim,
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
    };
  };

  const getNodeStyle = (node: WorkNode, isSelected: boolean) => ({
    display: "flex",
    alignItems: "center",
    gap: "9px",
    padding: "7px 10px",
    background: isSelected ? "#fff" : "transparent",
    border: isSelected ? `1.5px solid ${typeColors[node.type]}` : "1px solid transparent",
    boxShadow: isSelected ? "0 2px 8px rgba(0,0,0,0.08)" : "none",
    borderRadius: "8px",
    cursor: "pointer",
    transition: "all 0.2s",
  });

  const getLabel = (node: WorkNode) => {
    if (node.type === "orchestrator") {
      return node.file ? node.file.split("/").filter(Boolean).pop() ?? "src-tauri/" : "src-tauri/";
    }
    return node.file ? node.file.split("/").filter(Boolean).pop() ?? node.label : node.label;
  };

  const renderNode = (node: WorkNode, depth: number) => {
    const isSelected = selectedAgentId === node.agentId;
    const hasAsks = !!node.pendingQuestion;

    const nodeProps = {
      "data-agent-id": node.agentId,
      "data-node-type": node.type,
      "data-selected": isSelected ? "true" : "false",
      "data-live": node.live ? "true" : "false",
      "data-asks": hasAsks ? "true" : "false",
      "data-dirty": dirtyAgentIds?.has(node.agentId) ? "true" : "false",
      onClick: () => onSelect(node.agentId),
      style: getNodeStyle(node, isSelected),
    };

    const statusBadge = () => {
      if (node.type === "coder")
        return (
          <span style={{ marginLeft: "auto", fontSize: "11px", fontWeight: "600", color: "#9A6A2E", background: "#F1E4D2", padding: "3px 9px", borderRadius: "7px" }}>
            {node.label}
          </span>
        );
      if (node.type === "mini")
        return (
          <span style={{ marginLeft: "auto", fontSize: "10.5px", fontWeight: "600", color: "#4a57a8", background: "#E5E8F6", padding: "3px 8px", borderRadius: "7px" }}>
            {node.label}
          </span>
        );
      if (node.type === "censor")
        return (
          <span style={{ marginLeft: "auto", fontSize: "10.5px", fontWeight: "600", color: "#9A6A2E" }}>
            {node.status || "censor · inspecting"}
          </span>
        );
      return null;
    };

    const asksBadge = hasAsks ? (
      <span style={{ fontSize: "9px", fontWeight: "700", color: "#9A6A2E", background: "#F4ECDD", border: "1px solid #E6D3BB", padding: "2px 6px", borderRadius: "5px", animation: "wc-amber 1.8s infinite" }}>
        asks
      </span>
    ) : null;

    const marker = <span style={getMarkerStyle(node)} />;

    const children = node.children?.length ? (
      <div style={{ position: "relative", marginLeft: "26px", marginTop: "2px" }}>
        <span style={{ position: "absolute", left: "-14px", top: "-8px", width: "14px", height: "24px", borderLeft: "1px solid #D7CDBC", borderBottom: "1px solid #D7CDBC", borderBottomLeftRadius: "7px" }} />
        {node.children.map((child) => renderNode(child, depth + 1))}
      </div>
    ) : null;

    return (
      <div key={node.agentId}>
        <div {...nodeProps}>
          {marker}
          <span className="pp-mono" style={{ fontSize: node.type === "orchestrator" ? "12px" : "12.5px", color: "#2A2621", fontWeight: "500" }}>
            {getLabel(node)}
          </span>
          {node.type === "orchestrator" && (
            <span className="pp-mono" style={{ fontSize: "10.5px", color: "#9c9488" }}>
              orchestrator
            </span>
          )}
          {statusBadge()}
          {asksBadge}
        </div>
        {children}
      </div>
    );
  };

  const renderDistrict = (district: { name: string; nodes: WorkNode[] }) => (
    <div key={district.name} data-district={district.name} style={{ border: "1px solid #EFE7DA", background: "#FCFAF6", borderRadius: "11px", padding: "9px 11px 11px", margin: "13px 0 11px" }}>
      <div style={{ display: "flex", alignItems: "center", gap: "7px", marginBottom: "8px", padding: "0 2px" }}>
        <span className="pp-mono" style={{ fontSize: "10px", fontWeight: "700", letterSpacing: ".12em", color: "#9A6A2E" }}>{district.name}</span>
        <span style={{ width: "5px", height: "5px", borderRadius: "50%", background: "#E0D6C5" }} />
        <span className="pp-mono" style={{ fontSize: "10px", color: "#B3AB9C" }}>
          {district.nodes[0]?.file ? district.nodes[0].file.split("/").filter(Boolean).pop() ?? "" : district.nodes[0]?.label || ""}
        </span>
      </div>
      {district.nodes.map((node) => renderNode(node, 0))}
    </div>
  );

  return (
    <div className="wc-scroll" style={{ width: "100%", overflowY: "auto", padding: "16px", background: "#FBF8F2", display: "flex", flexDirection: "column" }}>
      <div style={{ display: "flex", alignItems: "center", gap: "8px", marginBottom: "14px" }}>
        <span className="pp-mono" style={{ fontSize: "10px", fontWeight: "700", letterSpacing: ".16em", color: "#8C8578" }}>LIVING PLAN</span>
        <span style={{ fontSize: "11px", color: "#A89F90" }}>{model.districts.length} active districts</span>
        <div style={{ marginLeft: "auto", display: "flex", gap: "11px" }}>
          <span style={{ display: "flex", alignItems: "center", gap: "5px", fontSize: "10px", color: "#7c766b" }}>
            <span style={{ width: "7px", height: "7px", borderRadius: "50%", background: "#2F7E7A" }} />orch
          </span>
          <span style={{ display: "flex", alignItems: "center", gap: "5px", fontSize: "10px", color: "#7c766b" }}>
            <span style={{ width: "7px", height: "7px", borderRadius: "50%", background: "#C0894F" }} />coder
          </span>
          <span style={{ display: "flex", alignItems: "center", gap: "5px", fontSize: "10px", color: "#7c766b" }}>
            <span style={{ width: "7px", height: "7px", borderRadius: "2px", background: "#5B6CC0" }} />mini
          </span>
          <span style={{ display: "flex", alignItems: "center", gap: "5px", fontSize: "10px", color: "#7c766b" }}>
            <span style={{ width: "7px", height: "7px", borderRadius: "50%", border: "1.5px solid #7FA468" }} />done
          </span>
        </div>
      </div>

      {model.orchestrator && renderNode(model.orchestrator, 0)}

      {model.districts.map((d) => renderDistrict(d))}

      {model.unplaced?.length ? (
        <div data-district="unplaced" style={{ border: "1px solid #EFE7DA", background: "#FCFAF6", borderRadius: "11px", padding: "9px 11px 11px", marginBottom: "11px" }}>
          <div style={{ display: "flex", alignItems: "center", gap: "7px", marginBottom: "8px", padding: "0 2px" }}>
            <span className="pp-mono" style={{ fontSize: "10px", fontWeight: "700", letterSpacing: ".12em", color: "#9A6A2E" }}>unplaced</span>
            <span style={{ width: "5px", height: "5px", borderRadius: "50%", background: "#E0D6C5" }} />
          </div>
          {model.unplaced.map((node) => renderNode(node, 0))}
        </div>
      ) : null}
    </div>
  );
}
