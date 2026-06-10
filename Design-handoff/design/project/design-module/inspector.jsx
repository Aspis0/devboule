// inspector.jsx — Adobe-style (but simple) node inspector: transform, radius tokens, elevation, arrange

const RADIUS_TOKENS = [
  { tok: "none", v: 0 },
  { tok: "sm", v: 8 },
  { tok: "md", v: 14 },
  { tok: "lg", v: 22 },
];

/* pure z-order ops: front/back jump, forward/backward swap with neighbour */
function zOrderOp(ns, id, op) {
  const me = ns.find((n) => n.id === id);
  if (!me) return ns;
  const zs = ns.map((n) => n.z);
  if (op === "front") {
    const m = Math.max(...zs);
    return me.z === m ? ns : ns.map((n) => (n.id === id ? { ...n, z: m + 1 } : n));
  }
  if (op === "back") {
    const m = Math.min(...zs);
    return me.z === m ? ns : ns.map((n) => (n.id === id ? { ...n, z: m - 1 } : n));
  }
  const sorted = [...ns].sort((a, b) => a.z - b.z);
  const i = sorted.findIndex((n) => n.id === id);
  const j = op === "forward" ? i + 1 : i - 1;
  if (j < 0 || j >= sorted.length) return ns;
  const other = sorted[j];
  return ns.map((n) =>
    n.id === id ? { ...n, z: other.z } : n.id === other.id ? { ...n, z: me.z } : n
  );
}

function NumField({ label, value, onChange, auto }) {
  return (
    <div className="numf">
      <label>{label}</label>
      {auto ? (
        <React.Fragment>
          <input value={value} readOnly tabIndex={-1} />
          <span className="auto">HUG</span>
        </React.Fragment>
      ) : (
        <input
          type="number"
          value={value}
          onChange={(e) => onChange(parseInt(e.target.value || "0", 10))}
        />
      )}
    </div>
  );
}

function Inspector({ node, nodes, setNodes, setSelectedId, onDirty, onBeginChange, height }) {
  const { useState } = React;
  const [pos, setPos] = useState(null); // null = default top-right; {left,top} once dragged
  if (!node) return null;
  const radius = node.radius == null ? 14 : node.radius;
  const sorted = [...nodes].sort((a, b) => a.z - b.z);
  const idx = sorted.findIndex((n) => n.id === node.id);
  const atBottom = idx === 0;
  const atTop = idx === sorted.length - 1;
  const radTok = (RADIUS_TOKENS.find((r) => r.v === radius) || { tok: "custom" }).tok;

  const patch = (p) => {
    if (onBeginChange) onBeginChange();
    setNodes((ns) => ns.map((n) => (n.id === node.id ? { ...n, ...p } : n)));
    onDirty();
  };
  const arrange = (op) => {
    if (onBeginChange) onBeginChange();
    setNodes((ns) => zOrderOp(ns, node.id, op));
    onDirty();
  };
  const duplicate = () => {
    if (onBeginChange) onBeginChange();
    const copy = {
      ...node,
      id: node.id + "-copy-" + Math.random().toString(36).slice(2, 6),
      name: node.name + " copy",
      x: node.x + 32, y: node.y + 32,
      z: Math.max(...nodes.map((n) => n.z)) + 1,
    };
    setNodes((ns) => [...ns, copy]);
    setSelectedId(copy.id);
    onDirty();
  };
  const remove = () => {
    if (onBeginChange) onBeginChange();
    setNodes((ns) => ns.filter((n) => n.id !== node.id));
    setSelectedId(null);
    onDirty();
  };

  const startMove = (e) => {
    if (e.target.closest("button")) return;
    e.preventDefault();
    const wrap = document.querySelector(".canvas-wrap");
    const card = e.currentTarget.parentElement;
    if (!wrap || !card) return;
    const wr = wrap.getBoundingClientRect();
    const cr = card.getBoundingClientRect();
    const ox = e.clientX - cr.left;
    const oy = e.clientY - cr.top;
    const onMove = (ev) => {
      let left = ev.clientX - wr.left - ox;
      let top = ev.clientY - wr.top - oy;
      left = Math.max(8, Math.min(left, wr.width - cr.width - 8));
      top = Math.max(8, Math.min(top, wr.height - 64));
      setPos({ left, top });
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };

  return (
    <div className="float-card inspector" data-screen-label="Inspector" style={pos ? { left: pos.left, top: pos.top, right: "auto" } : null}>
      <div className="insp-head" onPointerDown={startMove} title="Drag to move this panel">
        <b>{node.name}</b>
        <span className="kind">{(node.kind || "html").toUpperCase()}</span>
        <button className="close" onClick={() => setSelectedId(null)} title="Close (Esc)">
          <Icon name="x" size={13} />
        </button>
      </div>

      <div className="insp-sec">
        <div className="insp-label">TRANSFORM</div>
        <div className="insp-grid">
          <NumField label="X" value={node.x} onChange={(v) => patch({ x: v })} />
          <NumField label="Y" value={node.y} onChange={(v) => patch({ y: v })} />
          <NumField label="W" value={node.w} onChange={(v) => patch({ w: Math.max(240, v) })} />
          <NumField label="H" value={height || "—"} auto />
        </div>
      </div>

      <div className="insp-sec">
        <div className="insp-label">
          CORNERS
          <span className="tok">radius.{radTok}</span>
        </div>
        <div className="radius-row">
          {RADIUS_TOKENS.map((r) => (
            <button
              key={r.tok}
              className={"rad-btn" + (radius === r.v ? " sel" : "")}
              title={`radius.${r.tok} · ${r.v}px`}
              onClick={() => patch({ radius: r.v })}
            >
              <i style={{ borderTopLeftRadius: Math.min(r.v, 12) }}></i>
            </button>
          ))}
        </div>
      </div>

      <div className="insp-sec">
        <div className="insp-label">
          ELEVATION
          <span className="tok">shadow.{node.flat ? "none" : "soft"}</span>
        </div>
        <div className="seg sm">
          <button className={!node.flat ? "sel" : ""} onClick={() => patch({ flat: false })}>Soft</button>
          <button className={node.flat ? "sel" : ""} onClick={() => patch({ flat: true })}>Flat</button>
        </div>
      </div>

      <div className="insp-sec">
        <div className="insp-label">ARRANGE</div>
        <div className="arr-row">
          <button className="arr-btn" disabled={atBottom} title="Send to back" onClick={() => arrange("back")}>
            <Icon name="toBack" size={15} />
          </button>
          <button className="arr-btn" disabled={atBottom} title="Move backward  [" onClick={() => arrange("backward")}>
            <Icon name="down" size={15} />
          </button>
          <button className="arr-btn" disabled={atTop} title="Move forward  ]" onClick={() => arrange("forward")}>
            <Icon name="up" size={15} />
          </button>
          <button className="arr-btn" disabled={atTop} title="Bring to front" onClick={() => arrange("front")}>
            <Icon name="front" size={15} />
          </button>
        </div>
      </div>

      <div className="insp-sec">
        <div className="insp-actions">
          <button className="mini-btn" onClick={duplicate}>
            <Icon name="copy" size={13} />
            Duplicate
          </button>
          <button className="mini-btn danger" onClick={remove}>
            <Icon name="trash" size={13} />
            Delete
          </button>
        </div>
      </div>

      <div className="insp-foot">
        <Icon name="check" size={12} />
        Values snap to devboule tokens (DTCG)
      </div>
    </div>
  );
}

Object.assign(window, { Inspector, zOrderOp, RADIUS_TOKENS });
