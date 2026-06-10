// canvas.jsx — canvas viewport: drag, resize, snap, smart guides, zoom, layers

const GRID = 8;
const SNAP_T = 6;

/* Pure-ish snap helper: returns {x, y, guides[]} for a moving rect vs others */
function computeSnap(rect, others) {
  const guides = [];
  let { x, y } = rect;
  const movEdgesX = (px) => [px, px + rect.w / 2, px + rect.w];
  const movEdgesY = (py) => [py, py + rect.h / 2, py + rect.h];

  let bestX = null, bestY = null;
  others.forEach((o) => {
    const ox = [o.x, o.x + o.w / 2, o.x + o.w];
    const oy = [o.y, o.y + o.h / 2, o.y + o.h];
    movEdgesX(x).forEach((me, mi) => {
      ox.forEach((oe) => {
        const d = Math.abs(me - oe);
        if (d < SNAP_T && (bestX === null || d < bestX.d)) {
          bestX = { d, shift: oe - me, line: oe, o };
        }
      });
    });
    movEdgesY(y).forEach((me) => {
      oy.forEach((oe) => {
        const d = Math.abs(me - oe);
        if (d < SNAP_T && (bestY === null || d < bestY.d)) {
          bestY = { d, shift: oe - me, line: oe, o };
        }
      });
    });
  });

  if (bestX) {
    x += bestX.shift;
    const top = Math.min(y, bestX.o.y) - 24;
    const bot = Math.max(y + rect.h, bestX.o.y + bestX.o.h) + 24;
    guides.push({ type: "v", x: bestX.line, y1: top, y2: bot });
  } else {
    x = Math.round(x / GRID) * GRID;
  }
  if (bestY) {
    y += bestY.shift;
    const left = Math.min(x, bestY.o.x) - 24;
    const right = Math.max(x + rect.w, bestY.o.x + bestY.o.w) + 24;
    guides.push({ type: "h", y: bestY.line, x1: left, x2: right });
  } else {
    y = Math.round(y / GRID) * GRID;
  }
  return { x, y, guides };
}

function DesignCanvas({ nodes, setNodes, selectedId, setSelectedId, onDirty, onBeginChange, onRegionAnalyze }) {
  const { useState, useRef, useEffect, useCallback } = React;
  const [zoom, setZoom] = useState(0.85);
  const [pan, setPan] = useState({ x: 40, y: 24 });
  const [guides, setGuides] = useState([]);
  const [layersOpen, setLayersOpen] = useState(true);
  const [ceNodeId, setCeNodeId] = useState(null);   // node in content-edit mode
  const [ceEl, setCeEl] = useState(null);            // selected inner element
  const [ceVer, setCeVer] = useState(0);             // bump to reposition toolbar
  const ceContainerRef = useRef(null);
  const [tool, setTool] = useState("move");          // move | ai
  const [region, setRegion] = useState(null);         // {x,y,w,h} world coords
  const [regionBusy, setRegionBusy] = useState(false);
  const [regionPrompt, setRegionPrompt] = useState("");
  const regionDrag = useRef(null);
  const viewportRef = useRef(null);
  const worldRef = useRef(null);
  const dragRef = useRef(null);
  const measuredH = useRef({});

  /* measure node heights for snap math */
  const measure = useCallback(() => {
    if (!worldRef.current) return;
    worldRef.current.querySelectorAll("[data-node-id]").forEach((el) => {
      measuredH.current[el.getAttribute("data-node-id")] = el.offsetHeight;
    });
  }, []);
  useEffect(() => { measure(); });

  /* wheel: pan, ctrl+wheel: zoom (non-passive) */
  useEffect(() => {
    const vp = viewportRef.current;
    if (!vp) return;
    const onWheel = (e) => {
      e.preventDefault();
      if (e.ctrlKey || e.metaKey) {
        setZoom((z) => {
          const nz = Math.min(2, Math.max(0.3, z * (e.deltaY < 0 ? 1.08 : 0.92)));
          const r = vp.getBoundingClientRect();
          const cx = e.clientX - r.left, cy = e.clientY - r.top;
          setPan((p) => ({ x: cx - ((cx - p.x) / z) * nz, y: cy - ((cy - p.y) / z) * nz }));
          return nz;
        });
      } else {
        setPan((p) => ({ x: p.x - e.deltaX, y: p.y - e.deltaY }));
      }
    };
    vp.addEventListener("wheel", onWheel, { passive: false });
    return () => vp.removeEventListener("wheel", onWheel);
  }, []);

  /* ---- content-edit mode helpers ---- */
  const ceCommitExit = () => {
    if (ceContainerRef.current && ceNodeId) {
      const html = cleanSerialize(ceContainerRef.current);
      const cur = nodes.find((n) => n.id === ceNodeId);
      if (cur && cur.html !== html) {
        onBeginChange();
        setNodes((ns) => ns.map((n) => (n.id === ceNodeId ? { ...n, html } : n)));
        onDirty();
      }
    }
    if (ceEl) ceEl.classList.remove("ce-sel");
    setCeNodeId(null);
    setCeEl(null);
  };
  const ceSelect = (target, container) => {
    if (target === container) return;
    if (ceEl && ceEl !== target) ceEl.classList.remove("ce-sel");
    target.classList.add("ce-sel");
    setCeEl(target);
    setCeVer((v) => v + 1);
  };
  const ceStartText = (el) => {
    if (!elHasText(el)) return;
    startInlineTextEdit(el, () => { setCeVer((v) => v + 1); onDirty(); });
  };

  /* ---- Spot Edit region helpers (polygon) ---- */
  const rectToPts = (x, y, w, h) => [{ x, y }, { x: x + w, y }, { x: x + w, y: y + h }, { x, y: y + h }];
  const bboxOf = (pts) => {
    const xs = pts.map((p) => p.x), ys = pts.map((p) => p.y);
    const x = Math.min(...xs), y = Math.min(...ys);
    return { x, y, w: Math.max(...xs) - x, h: Math.max(...ys) - y };
  };
  const toWorld = (clientX, clientY) => {
    const wr = viewportRef.current.getBoundingClientRect();
    return { x: (clientX - wr.left - pan.x) / zoom, y: (clientY - wr.top - pan.y) / zoom };
  };
  const startRegionDraw = (e) => {
    if (regionBusy) return;
    const p = toWorld(e.clientX, e.clientY);
    regionDrag.current = { sx: p.x, sy: p.y };
    setRegion({ pts: rectToPts(p.x, p.y, 0, 0) });
    const onMove = (ev) => {
      const c = toWorld(ev.clientX, ev.clientY);
      const d = regionDrag.current;
      setRegion({ pts: rectToPts(Math.min(d.sx, c.x), Math.min(d.sy, c.y), Math.abs(c.x - d.sx), Math.abs(c.y - d.sy)) });
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      regionDrag.current = null;
      setRegion((r) => {
        if (!r) return null;
        const bb = bboxOf(r.pts);
        /* return a NEW object so React re-renders (drawing flag must clear) */
        return bb.w > 24 && bb.h > 24 ? { pts: [...r.pts] } : null;
      });
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };
  const dragWindow = (onMove) => {
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };
  const startVertexDrag = (e, idx) => {
    e.stopPropagation();
    e.preventDefault();
    dragWindow((ev) => {
      const c = toWorld(ev.clientX, ev.clientY);
      setRegion((r) => ({ pts: r.pts.map((pt, i) => (i === idx ? c : pt)) }));
    });
  };
  const startMidDrag = (e, edgeIdx) => {
    e.stopPropagation();
    e.preventDefault();
    const p = region.pts[edgeIdx];
    const q = region.pts[(edgeIdx + 1) % region.pts.length];
    const newIdx = edgeIdx + 1;
    const mid = { x: (p.x + q.x) / 2, y: (p.y + q.y) / 2 };
    setRegion({ pts: [...region.pts.slice(0, newIdx), mid, ...region.pts.slice(newIdx)] });
    dragWindow((ev) => {
      const c = toWorld(ev.clientX, ev.clientY);
      setRegion((r) => ({ pts: r.pts.map((pt, i) => (i === newIdx ? c : pt)) }));
    });
  };
  const startRegionMove = (e) => {
    if (regionBusy) return;
    e.stopPropagation();
    e.preventDefault();
    const pts0 = region.pts.map((p) => ({ ...p }));
    const s = toWorld(e.clientX, e.clientY);
    dragWindow((ev) => {
      const c = toWorld(ev.clientX, ev.clientY);
      setRegion({ pts: pts0.map((p) => ({ x: p.x + (c.x - s.x), y: p.y + (c.y - s.y) })) });
    });
  };
  const analyzeRegion = () => {
    if (!region || regionBusy) return;
    setRegionBusy(true);
    const bb = bboxOf(region.pts);
    const hits = nodes
      .filter((n) => {
        const h = measuredH.current[n.id] || 200;
        return n.x < bb.x + bb.w && n.x + n.w > bb.x && n.y < bb.y + bb.h && n.y + h > bb.y;
      })
      .map((n) => ({ id: n.id, name: n.name }));
    onRegionAnalyze(bb, regionPrompt.trim(), hits, () => {
      setRegionBusy(false);
      setRegion(null);
      setRegionPrompt("");
      setTool("move");
    });
  };

  /* keyboard: esc deselect, delete remove */
  useEffect(() => {
    const onKey = (e) => {
      const tag = (e.target.tagName || "").toLowerCase();
      if (tag === "input" || tag === "textarea" || e.target.isContentEditable) return;
      if (e.key === "Escape") {
        if (ceNodeId) ceCommitExit();
        else if (region || tool === "ai") { setRegion(null); setRegionPrompt(""); setTool("move"); }
        else setSelectedId(null);
      }
      if ((e.key === "[" || e.key === "]") && selectedId && !ceNodeId) {
        onBeginChange();
        setNodes((ns) => zOrderOp(ns, selectedId, e.key === "]" ? "forward" : "backward"));
        onDirty();
      }
      if (e.key === "Delete" || e.key === "Backspace") {
        if (ceNodeId) {
          if (ceEl) {
            ceEl.remove();
            setCeEl(null);
            setCeVer((v) => v + 1);
            onDirty();
          }
        } else if (selectedId) {
          onBeginChange();
          setNodes((ns) => ns.filter((n) => n.id !== selectedId));
          setSelectedId(null);
          onDirty();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selectedId, ceNodeId, ceEl, region, tool]);

  const startDrag = (e, node, mode) => {
    e.preventDefault();
    e.stopPropagation();
    setSelectedId(node.id);
    dragRef.current = {
      id: node.id, mode,
      startX: e.clientX, startY: e.clientY,
      origX: node.x, origY: node.y, origW: node.w,
      moved: false,
    };
    const onMove = (ev) => {
      const d = dragRef.current;
      if (!d) return;
      const dx = (ev.clientX - d.startX) / zoom;
      const dy = (ev.clientY - d.startY) / zoom;
      if (Math.abs(dx) + Math.abs(dy) > 1) {
        if (!d.moved) { onBeginChange(); d.moved = true; }
      }
      if (d.mode === "move") {
        const h = measuredH.current[d.id] || 200;
        const others = nodes.filter((n) => n.id !== d.id && !n.hidden).map((n) => ({
          x: n.x, y: n.y, w: n.w, h: measuredH.current[n.id] || 200,
        }));
        const snapped = computeSnap({ x: d.origX + dx, y: d.origY + dy, w: nodes.find(n => n.id === d.id).w, h }, others);
        setGuides(snapped.guides);
        setNodes((ns) => ns.map((n) => n.id === d.id ? { ...n, x: snapped.x, y: snapped.y } : n));
      } else {
        const w = Math.max(240, Math.round((d.origW + dx) / GRID) * GRID);
        setNodes((ns) => ns.map((n) => n.id === d.id ? { ...n, w } : n));
      }
    };
    const onUp = () => {
      const d = dragRef.current;
      dragRef.current = null;
      setGuides([]);
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      if (d && d.moved) {
        // bring to front on commit: z = max+1
        setNodes((ns) => {
          const maxZ = Math.max(...ns.map((n) => n.z));
          return ns.map((n) => n.id === d.id ? { ...n, z: maxZ + 1 } : n);
        });
        onDirty();
      }
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };

  const bringToFront = (id) => {
    onBeginChange();
    setNodes((ns) => {
      const maxZ = Math.max(...ns.map((n) => n.z));
      return ns.map((n) => n.id === id ? { ...n, z: maxZ + 1 } : n);
    });
    onDirty();
  };

  const toggleHidden = (id) => {
    onBeginChange();
    setNodes((ns) => ns.map((n) => (n.id === id ? { ...n, hidden: !n.hidden } : n)));
    onDirty();
  };

  const removeLayer = (id) => {
    onBeginChange();
    setNodes((ns) => ns.filter((n) => n.id !== id));
    if (selectedId === id) setSelectedId(null);
    onDirty();
  };

  const zoomTo = (nz) => setZoom(Math.min(2, Math.max(0.3, nz)));
  const fit = () => { setZoom(0.85); setPan({ x: 40, y: 24 }); };

  const dotSize = 22 * zoom;

  return (
    <div className="canvas-wrap" data-screen-label="Canvas" ref={(el) => { if (el) viewportRef.wrap = el; }}>
      <div
        ref={viewportRef}
        className="canvas-viewport"
        data-tool={tool}
        style={{
          backgroundImage: "radial-gradient(circle, #DDD1BE 1px, transparent 1px)",
          backgroundSize: `${dotSize}px ${dotSize}px`,
          backgroundPosition: `${pan.x}px ${pan.y}px`,
        }}
        onPointerDown={(e) => {
          if (tool === "ai") { startRegionDraw(e); return; }
          if (ceNodeId) ceCommitExit();
          setSelectedId(null);
          setRegion(null);
        }}
      >
        <div
          ref={worldRef}
          className="canvas-world"
          style={{ transform: `translate(${pan.x}px, ${pan.y}px) scale(${zoom})` }}
        >
          {nodes.filter((n) => !n.hidden).map((node) => {
            const rad = node.radius == null ? 14 : node.radius;
            const inCe = node.id === ceNodeId;
            return (
            <div
              key={node.id}
              data-node-id={node.id}
              className={"node-host" + (node.id === selectedId && tool !== "ai" ? " sel" : "") + (inCe ? " content" : "")}
              style={{ left: node.x, top: node.y, width: node.w, zIndex: node.z, cursor: inCe ? "default" : "grab" }}
              onPointerDown={(e) => {
                if (tool === "ai") return; /* let it bubble: draw region over nodes */
                if (inCe) { e.stopPropagation(); return; }
                if (ceNodeId) ceCommitExit();
                startDrag(e, node, "move");
              }}
              onDoubleClick={(e) => {
                if (tool === "ai") return;
                if (!inCe) {
                  e.stopPropagation();
                  setSelectedId(node.id);
                  setCeNodeId(node.id);
                }
              }}
            >
              <div className="node-tag">
                {inCe ? node.name + " · editing content — Esc to finish" : node.name}
                {!inCe && <span className="kind">{(node.kind || "html").toUpperCase()}</span>}
              </div>
              <div className="node-card" style={{ borderRadius: rad, boxShadow: node.flat ? "none" : undefined }}>
                <div
                  className="node-content"
                  ref={inCe ? (el) => { ceContainerRef.current = el; } : null}
                  onMouseOver={inCe ? (e) => { if (e.target !== e.currentTarget && !e.target.isContentEditable) e.target.classList.add("ce-hover"); } : null}
                  onMouseOut={inCe ? (e) => e.target.classList.remove("ce-hover") : null}
                  onClick={inCe ? (e) => { e.stopPropagation(); ceSelect(e.target, e.currentTarget); } : null}
                  onDoubleClick={inCe ? (e) => { e.stopPropagation(); ceSelect(e.target, e.currentTarget); ceStartText(e.target); } : null}
                  dangerouslySetInnerHTML={{ __html: node.generating && !node.html ? SKELETON : node.html }}
                ></div>
              </div>
              {node.generating && <div className="node-shimmer" style={{ borderRadius: rad }}></div>}
              <div className="node-ring" style={{ borderRadius: rad + 3 }}></div>
              <div className="node-handle" onPointerDown={(e) => { if (tool === "ai") return; startDrag(e, node, "resize"); }}></div>
            </div>
          );})}
          {guides.map((g, i) => g.type === "v"
            ? <div key={i} className="guide v" style={{ left: g.x, top: g.y1, height: g.y2 - g.y1 }}></div>
            : <div key={i} className="guide h" style={{ top: g.y, left: g.x1, width: g.x2 - g.x1 }}></div>
          )}
          {region && (() => {
            const bb = bboxOf(region.pts);
            const bw = Math.max(bb.w, 1), bh = Math.max(bb.h, 1);
            const clip = "polygon(" + region.pts.map((p) => `${((p.x - bb.x) / bw) * 100}% ${((p.y - bb.y) / bh) * 100}%`).join(",") + ")";
            const drawing = !!regionDrag.current;
            return (
              <React.Fragment>
                <div
                  className="ai-region"
                  style={{ left: bb.x, top: bb.y, width: bw, height: bh, clipPath: clip, pointerEvents: regionBusy || drawing ? "none" : "auto" }}
                  onPointerDown={startRegionMove}
                  title="Drag to move the region"
                >
                  {regionBusy && <div className="node-shimmer"></div>}
                </div>
                <svg
                  className="ai-outline"
                  style={{ left: bb.x, top: bb.y, width: bw, height: bh }}
                  viewBox={`0 0 ${bw} ${bh}`}
                  preserveAspectRatio="none"
                >
                  <polygon points={region.pts.map((p) => `${p.x - bb.x},${p.y - bb.y}`).join(" ")} />
                </svg>
                {!regionBusy && !drawing && region.pts.map((p, i) => {
                  const q = region.pts[(i + 1) % region.pts.length];
                  return (
                    <div
                      key={"m" + i}
                      className="ai-mid"
                      style={{ left: (p.x + q.x) / 2, top: (p.y + q.y) / 2 }}
                      onPointerDown={(e) => startMidDrag(e, i)}
                      title="Drag to add a point"
                    ></div>
                  );
                })}
                {!regionBusy && !drawing && region.pts.map((p, i) => (
                  <div
                    key={"v" + i}
                    className="ai-handle vtx"
                    style={{ left: p.x, top: p.y }}
                    onPointerDown={(e) => startVertexDrag(e, i)}
                    onDoubleClick={(e) => {
                      e.stopPropagation();
                      if (region.pts.length > 3) setRegion({ pts: region.pts.filter((_, j) => j !== i) });
                    }}
                    title="Drag to reshape · double-click to remove"
                  ></div>
                ))}
              </React.Fragment>
            );
          })()}
        </div>
      </div>

      {/* Empty canvas state */}
      {nodes.length === 0 && (
        <div className="canvas-empty">
          <div className="ce-card">
            <span className="ce-ic"><Icon name="sparkles" size={22} /></span>
            <b>No sections yet</b>
            <p>Describe a section in the assistant — it's generated grounded on your codebase, with your real components, tokens and copy.</p>
            <button className="btn btn-primary" onClick={() => { const ta = document.querySelector(".composer textarea"); if (ta) ta.focus(); }}>
              <Icon name="wand" size={15} />
              Generate a section
            </button>
          </div>
        </div>
      )}

      {/* Spot-edit dim overlay (screen space) */}
      {region && viewportRef.wrap && (
        <svg className="ai-dim" width={viewportRef.wrap.clientWidth} height={viewportRef.wrap.clientHeight}>
          <path
            fillRule="evenodd"
            fill="rgba(62,47,24,.10)"
            d={
              `M0 0H${viewportRef.wrap.clientWidth}V${viewportRef.wrap.clientHeight}H0Z ` +
              "M" + region.pts.map((p) => `${pan.x + p.x * zoom} ${pan.y + p.y * zoom}`).join("L") + "Z"
            }
          />
        </svg>
      )}

      {/* Spot-edit prompt bar */}
      {region && !regionDrag.current && (() => {
        const bb = bboxOf(region.pts);
        return (
        <div
          className="ai-bar"
          style={{
            left: Math.max(8, Math.min(pan.x + bb.x * zoom, (viewportRef.wrap ? viewportRef.wrap.clientWidth : 800) - 400)),
            top: pan.y + (bb.y + bb.h) * zoom + 14,
          }}
          onPointerDown={(e) => e.stopPropagation()}
        >
          <Icon name="sparkles" size={15} style={{ color: "var(--accent)", flex: "none" }} />
          <input
            placeholder="Describe the problem — or leave blank to auto-detect"
            value={regionPrompt}
            disabled={regionBusy}
            onChange={(e) => setRegionPrompt(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter") analyzeRegion(); }}
          />
          <button className="go" disabled={regionBusy} onClick={analyzeRegion}>
            {regionBusy ? "Analyzing…" : "Analyze"}
          </button>
          <button className="tb-x" title="Cancel" onClick={() => { setRegion(null); setRegionPrompt(""); }} disabled={regionBusy}>
            <Icon name="x" size={13} />
          </button>
        </div>
        );
      })()}

      {/* Tools */}
      <div className="float-card tool-pill">
        <button className={tool === "move" ? "sel" : ""} title="Move / select" onClick={() => { setTool("move"); setRegion(null); }}>
          <Icon name="cursor" size={14} />
          Move
        </button>
        <button className={tool === "ai" ? "sel" : ""} title="Drag a region, then let the AI analyze and fix it" onClick={() => { setTool("ai"); setSelectedId(null); if (ceNodeId) ceCommitExit(); }}>
          <Icon name="marquee" size={14} />
          Spot Edit
        </button>
      </div>

      {/* Content-edit toolbar */}
      {ceEl && (
        <ContentToolbar
          el={ceEl}
          wrapRef={{ current: viewportRef.wrap }}
          version={ceVer}
          pan={pan}
          zoom={zoom}
          onEditText={() => ceStartText(ceEl)}
          onColor={(v, mode) => {
            if (mode === "fill") ceEl.style.background = v;
            else ceEl.style.color = v;
            setCeVer((x) => x + 1);
            onDirty();
          }}
          onMove={(dir) => {
            const p = ceEl.parentElement;
            if (dir === "up" && ceEl.previousElementSibling) p.insertBefore(ceEl, ceEl.previousElementSibling);
            if (dir === "down" && ceEl.nextElementSibling) p.insertBefore(ceEl.nextElementSibling, ceEl);
            setCeVer((x) => x + 1);
            onDirty();
          }}
          onRemove={() => {
            ceEl.remove();
            setCeEl(null);
            setCeVer((x) => x + 1);
            onDirty();
          }}
          onAskAi={() => {
            ceCommitExit();
            const ta = document.querySelector(".composer textarea");
            if (ta) ta.focus();
          }}
        />
      )}

      {/* Inspector */}
      <Inspector
        node={tool === "ai" ? null : nodes.find((n) => n.id === selectedId) || null}
        nodes={nodes}
        setNodes={setNodes}
        setSelectedId={setSelectedId}
        onDirty={onDirty}
        onBeginChange={onBeginChange}
        height={measuredH.current[selectedId]}
      />

      {/* Layers */}
      <div className="float-card layers">
        <button className="layers-head" onClick={() => setLayersOpen(!layersOpen)}>
          <Icon name="layers" size={14} />
          LAYERS
          <span className="n">{nodes.length}</span>
          <Icon name={layersOpen ? "chevDown" : "chevRight"} size={13} />
        </button>
        {layersOpen && (
          <div className="layers-list">
            {nodes.length === 0 && <div className="layers-empty">No layers yet</div>}
            {[...nodes].sort((a, b) => b.z - a.z).map((n) => (
              <button
                key={n.id}
                className={"layer-row" + (n.id === selectedId ? " sel" : "") + (n.hidden ? " is-hidden" : "")}
                onClick={() => setSelectedId(n.id)}
              >
                <Icon name={n.kind === "svg" ? "image" : "code"} size={13} style={{ opacity: .7, flex: "none" }} />
                <span className="lr-name">{n.name}</span>
                <span className="layer-acts">
                  <span className="lr-act vis" title={n.hidden ? "Show" : "Hide"} onClick={(e) => { e.stopPropagation(); toggleHidden(n.id); }}>
                    <Icon name={n.hidden ? "eyeOff" : "eye"} size={13} />
                  </span>
                  <span className="lr-act" title="Bring to front" onClick={(e) => { e.stopPropagation(); bringToFront(n.id); }}>
                    <Icon name="front" size={13} />
                  </span>
                  <span className="lr-act danger" title="Delete" onClick={(e) => { e.stopPropagation(); removeLayer(n.id); }}>
                    <Icon name="trash" size={13} />
                  </span>
                </span>
              </button>
            ))}
          </div>
        )}
      </div>

      {/* Zoom */}
      <div className="float-card zoom-pill">
        <button onClick={() => zoomTo(zoom - 0.1)} title="Zoom out"><Icon name="minus" size={14} /></button>
        <button className="zoom-pct" onClick={() => zoomTo(1)} title="Reset to 100%">{Math.round(zoom * 100)}%</button>
        <button onClick={() => zoomTo(zoom + 0.1)} title="Zoom in"><Icon name="plus" size={14} /></button>
        <button onClick={fit} title="Fit"><Icon name="fit" size={14} /></button>
      </div>

      <div className="canvas-hint">
        {ceNodeId ? (
          <React.Fragment>
            <span><b>Click</b> an element · <b>double-click</b> text to type</span>
            <span><b>Del</b> removes element · <b>Esc</b> to finish</span>
          </React.Fragment>
        ) : tool === "ai" ? (
          <React.Fragment>
            <span><b>Drag</b> a region · <b>corners</b> reshape · <b>edge dots</b> add points</span>
            <span><b>Double-click</b> a point removes it · <b>Esc</b> to cancel</span>
          </React.Fragment>
        ) : (
          <React.Fragment>
            <span><b>Drag</b> to move · corner to resize · <b>double-click</b> to edit content</span>
            <span><b>Scroll</b> to pan · <b>⌘ scroll</b> to zoom · <b>[ ]</b> to reorder</span>
          </React.Fragment>
        )}
      </div>
    </div>
  );
}

Object.assign(window, { DesignCanvas, computeSnap });
