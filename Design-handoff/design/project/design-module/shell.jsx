// shell.jsx — Sidebar, TopBar, project/oracle/export popovers, toast

const { useState, useEffect, useRef } = React;

/* Generic popover wrapper: click-outside via full-screen catch layer */
function Popover({ open, onClose, children, className }) {
  if (!open) return null;
  return (
    <React.Fragment>
      <div className="overlay-catch" onClick={onClose}></div>
      <div className={"pop " + (className || "right")}>{children}</div>
    </React.Fragment>
  );
}

function AppSidebar() {
  const items = [
    { id: "projects", label: "Projects", icon: "folder" },
    { id: "oracle", label: "Oracle", icon: "compass" },
    { id: "dashboard", label: "Dashboard", icon: "grid" },
    { id: "providers", label: "Providers", icon: "plug" },
    { id: "polis", label: "Polis", icon: "box" },
    { id: "design", label: "Design", icon: "palette" },
  ];
  return (
    <nav className="sidebar" data-screen-label="Sidebar">
      <div className="side-brand">
        <div className="logo"><Icon name="shield" size={19} sw={1.9} /></div>
        <div>
          <b>Devboule</b>
          <span>MANAGEMENT</span>
        </div>
      </div>
      <div className="side-label">MENU</div>
      <div className="side-nav">
        {items.map((it) => (
          <button key={it.id} className={"side-item" + (it.id === "design" ? " active" : "")}>
            <Icon name={it.icon} size={18} />
            {it.label}
          </button>
        ))}
      </div>
      <div className="side-foot">
        <div className="avatar">MG</div>
        <div>
          <b>Administrator</b>
          <span>Settings</span>
        </div>
        <button className="gear"><Icon name="gear" size={17} /></button>
      </div>
    </nav>
  );
}

function ProjectPopover({ open, onClose }) {
  return (
    <Popover open={open} onClose={onClose} className="left">
      <div className="pop-head">DESIGN PROJECTS</div>
      {PROJECTS.map((p, i) => (
        <button key={p.id} className={"pop-row" + (i === 0 ? " sel" : "")} onClick={onClose}>
          <div className="thumb" style={{ background: p.color }}></div>
          <div>
            <b>{p.name}</b>
            <span>{p.meta}</span>
          </div>
          {i === 0 && <span className="check"><Icon name="check" size={15} /></span>}
        </button>
      ))}
      <div className="pop-sep"></div>
      <button className="pop-row" onClick={onClose}>
        <span style={{ color: "var(--accent)", display: "grid", placeItems: "center", width: 30 }}><Icon name="plus" size={16} /></span>
        <b style={{ fontWeight: 600 }}>New project</b>
      </button>
      <button className="pop-row" onClick={onClose}>
        <span style={{ color: "var(--ink-2)", display: "grid", placeItems: "center", width: 30 }}><Icon name="folder" size={16} /></span>
        <b style={{ fontWeight: 600 }}>Open working folder…</b>
      </button>
    </Popover>
  );
}

function OraclePopover({ open, onClose }) {
  return (
    <Popover open={open} onClose={onClose} className="right oracle-pop">
      <div className="op-head">
        <span className="dot"></span>
        <div>
          <b>Grounded on devboule</b>
          <span>Designs reuse this codebase's components &amp; tokens</span>
        </div>
      </div>
      <div className="op-stats">
        <div className="op-stat"><b>1,284</b><span>chunks indexed</span></div>
        <div className="op-stat"><b>212</b><span>files</span></div>
        <div className="op-stat"><b>2m</b><span>last sync</span></div>
      </div>
      <div className="op-tokens">
        <Icon name="palette" size={15} style={{ color: "var(--accent)" }} />
        <span><b>Design tokens</b> seeded from target</span>
        <span className="sw">
          <i style={{ background: "#C14B1B" }}></i>
          <i style={{ background: "#3B2D1D" }}></i>
          <i style={{ background: "#F3E3CB" }}></i>
          <i style={{ background: "#FBF6EE" }}></i>
        </span>
      </div>
    </Popover>
  );
}

function ExportPopover({ open, onClose, onPick }) {
  const rows = [
    { id: "html", icon: "code", t: "Standalone HTML", d: "Absolute layout, single file" },
    { id: "scaffold", icon: "grid", t: "Flex/grid scaffold", d: "Responsive starting point" },
    { id: "tokens", icon: "palette", t: "Design tokens", d: "W3C DTCG JSON" },
  ];
  return (
    <Popover open={open} onClose={onClose} className="right">
      <div className="pop-head">EXPORT</div>
      {rows.map((r) => (
        <button key={r.id} className="pop-row" onClick={() => { onPick(r.t); onClose(); }}>
          <span style={{ color: "var(--ink-2)", display: "grid", placeItems: "center", width: 30 }}><Icon name={r.icon} size={16} /></span>
          <div>
            <b>{r.t}</b>
            <span>{r.d}</span>
          </div>
        </button>
      ))}
    </Popover>
  );
}

function SaveMenuPopover({ open, onClose, onSave, onHandoff }) {
  return (
    <Popover open={open} onClose={onClose} className="right save-pop">
      <div className="pop-head">DELIVER</div>
      <button className="pop-row" onClick={() => { onClose(); onSave(); }}>
        <span style={{ color: "var(--ink-2)", display: "grid", placeItems: "center", width: 30 }}><Icon name="save" size={16} /></span>
        <div>
          <b>Save to repo</b>
          <span>Write the design back to the working folder</span>
        </div>
      </button>
      <button className="pop-row agents" onClick={() => { onClose(); onHandoff(); }}>
        <span className="agents-ic"><Icon name="cpu" size={16} /></span>
        <div>
          <b>Save &amp; hand off to agents</b>
          <span>Local agents wire up the backend &amp; config</span>
        </div>
        <span className="new-badge">NEW</span>
      </button>
    </Popover>
  );
}

function HandoffModal({ handoff, onClose }) {
  if (!handoff) return null;
  const { phase, tasks } = handoff;
  const doneCount = tasks.filter((t) => t.status === "done").length;
  const done = phase === "done";
  const repoDone = phase !== "packaging";
  return (
    <div className="modal-scrim" onClick={done ? onClose : undefined}>
      <div className="handoff" onClick={(e) => e.stopPropagation()} data-screen-label="Agent handoff">
        <div className="ho-head">
          <span className="ho-ic"><Icon name="cpu" size={18} /></span>
          <div className="ho-head-t">
            <b>Hand off to local agents</b>
            <span>devboule/.devboule-design/demo-landing</span>
          </div>
          {done && <button className="ho-close" onClick={onClose} title="Close"><Icon name="x" size={15} /></button>}
        </div>

        <div className="ho-flow">
          <div className="ho-step done"><span><Icon name="palette" size={16} /></span>Design</div>
          <div className="ho-wire" data-on={repoDone}></div>
          <div className={"ho-step" + (repoDone ? " done" : " run")}><span><Icon name="save" size={16} /></span>Repo</div>
          <div className="ho-wire" data-on={doneCount > 0}></div>
          <div className={"ho-step" + (done ? " done" : doneCount > 0 ? " run" : "")}><span><Icon name="cpu" size={16} /></span>Agents</div>
        </div>

        <div className="ho-tasks">
          {tasks.map((t, i) => (
            <div key={i} className={"ho-task " + t.status}>
              <span className="ho-task-ic">
                {t.status === "done"
                  ? <Icon name="check" size={14} />
                  : t.status === "running"
                    ? <span className="spin"><Icon name="loader" size={14} /></span>
                    : <Icon name={t.icon} size={14} />}
              </span>
              <div className="ho-task-body">
                <b>{t.label}</b>
                <span>{t.detail}</span>
              </div>
              <span className="ho-task-agent">{t.agent} agent</span>
            </div>
          ))}
        </div>

        <div className="ho-foot">
          {done ? (
            <React.Fragment>
              <span className="ho-foot-note ok"><span className="dot"></span>{tasks.length} agents running in your repo</span>
              <button className="btn btn-ghost" onClick={onClose}><Icon name="terminal" size={15} />Open terminal</button>
              <button className="btn btn-primary" onClick={onClose}>Done</button>
            </React.Fragment>
          ) : (
            <span className="ho-foot-note">
              <span className="spin"><Icon name="loader" size={14} /></span>
              {phase === "packaging" ? "Packaging project & design tokens…" : `Dispatching · ${doneCount}/${tasks.length} complete`}
            </span>
          )}
        </div>
      </div>
    </div>
  );
}

function TopBar({ saveState, onConsolidate, onHandoff, toast, fullscreen, onToggleFullscreen, onUndo, onRedo, canUndo, canRedo }) {
  const [projOpen, setProjOpen] = useState(false);
  const [oracleOpen, setOracleOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [saveOpen, setSaveOpen] = useState(false);

  const statusText = saveState === "saved" ? "Saved" : saveState === "writing" ? "Saving…" : "Unsaved changes";

  return (
    <header className="topbar" data-screen-label="Top bar">
      <div className="tb-title">
        <div className="pop-wrap">
          <button className="tb-proj" data-open={projOpen} onClick={() => setProjOpen(!projOpen)}>
            <Icon name="palette" size={17} style={{ color: "var(--accent)" }} />
            Demo landing
            <span className="chev"><Icon name="chevDown" size={14} /></span>
          </button>
          <ProjectPopover open={projOpen} onClose={() => setProjOpen(false)} />
        </div>
        <span className="tb-path">
          <Icon name="folder" size={13} />
          devboule/.devboule-design/demo-landing
        </span>
        <span className="tb-status" data-state={saveState === "saved" ? "clean" : saveState}>
          <span className="dot"></span>
          {statusText}
        </span>
      </div>
      <div className="tb-right">
        <div className="hist-group">
          <button className="icon-btn-tb" disabled={!canUndo} onClick={onUndo} title="Undo (⌘Z)"><Icon name="undo" size={16} /></button>
          <button className="icon-btn-tb" disabled={!canRedo} onClick={onRedo} title="Redo (⌘⇧Z)"><Icon name="redo" size={16} /></button>
        </div>
        <span className="tb-div"></span>
        <div className="pop-wrap">
          <button className="chip-oracle" data-open={oracleOpen} onClick={() => setOracleOpen(!oracleOpen)} title="Oracle grounding">
            <span className="dot"></span>
            Grounded · devboule
            <Icon name="chevDown" size={13} style={{ color: "var(--muted)" }} />
          </button>
          <OraclePopover open={oracleOpen} onClose={() => setOracleOpen(false)} />
        </div>
        <div className="pop-wrap">
          <button className="btn btn-ghost" onClick={() => setExportOpen(!exportOpen)}>
            <Icon name="code" size={15} />
            Export
          </button>
          <ExportPopover open={exportOpen} onClose={() => setExportOpen(false)} onPick={(t) => toast(t + " exported to working folder")} />
        </div>
        <div className="pop-wrap split-primary">
          <button className="btn btn-primary split-main" onClick={onConsolidate} title="Write the design back to the working folder">
            <Icon name="save" size={15} />
            Save to repo
          </button>
          <button className="btn btn-primary split-caret" data-open={saveOpen} onClick={() => setSaveOpen(!saveOpen)} title="More save options">
            <Icon name="chevDown" size={14} />
          </button>
          <SaveMenuPopover open={saveOpen} onClose={() => setSaveOpen(false)} onSave={onConsolidate} onHandoff={onHandoff} />
        </div>
        <button className="btn btn-quiet" style={{ padding: "0 8px" }} title={fullscreen ? "Exit focus mode" : "Focus mode — hide the management menu"} onClick={onToggleFullscreen}>
          <Icon name={fullscreen ? "collapse" : "expand"} size={17} />
        </button>
      </div>
    </header>
  );
}

function Toast({ msg }) {
  if (!msg) return null;
  return (
    <div className="toast">
      <Icon name="check" size={15} />
      {msg}
    </div>
  );
}

Object.assign(window, { AppSidebar, TopBar, Toast, Popover, HandoffModal });
