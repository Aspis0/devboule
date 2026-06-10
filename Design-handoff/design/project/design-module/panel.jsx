// panel.jsx — Assistant panel: messages, suggestions, composer, model picker

function ModelPopover({ open, onClose, model, setModel }) {
  const { useState } = React;
  if (!open) return null;
  const prov = DESIGN_PROVIDERS.find((p) => p.id === model.provider);
  return (
    <React.Fragment>
      <div className="overlay-catch" onClick={onClose}></div>
      <div className="pop model-pop">
        <div className="mp-label">PROVIDER</div>
        <div className="mp-prov">
          {DESIGN_PROVIDERS.map((p) => (
            <button
              key={p.id}
              className={"mp-row" + (p.id === model.provider ? " sel" : "")}
              onClick={() => setModel({ ...model, provider: p.id })}
            >
              <span className="ico"><Icon name={p.icon} size={15} /></span>
              <div>
                <b>{p.name}</b>
                <span>{p.desc}</span>
              </div>
              <span className="mp-badge">{p.badge}</span>
            </button>
          ))}
        </div>
        <div className="mp-label">EFFORT</div>
        <div className="seg">
          {EFFORT_LEVELS.map((lv) => (
            <button
              key={lv}
              className={lv === model.effort ? "sel" : ""}
              onClick={() => setModel({ ...model, effort: lv })}
            >{lv}</button>
          ))}
        </div>
        <div className="mp-label">TIMEOUT</div>
        <div className="mp-slider">
          <Icon name="clock" size={15} style={{ color: "var(--muted)" }} />
          <input
            type="range" min="60" max="600" step="30"
            value={model.timeout}
            onChange={(e) => setModel({ ...model, timeout: +e.target.value })}
          />
          <span className="val">{model.timeout}s</span>
        </div>
      </div>
    </React.Fragment>
  );
}

function AssistantMessages({ messages, onSuggest, onRetry, scrollRef }) {
  return (
    <div className="assist-scroll" ref={scrollRef}>
      {messages.length === 0 && (
        <React.Fragment>
          <p className="sugg-intro">
            Describe a section and it's generated <b style={{ fontWeight: 600, color: "var(--ink-2)" }}>grounded in your real codebase</b> — components, palette and copy included. Select a node on the canvas to edit just that node.
          </p>
          {SUGGESTIONS.map((s, i) => (
            <button key={i} className="sugg" onClick={() => onSuggest(s.text)}>
              <Icon name={s.icon} size={16} />
              {s.text}
            </button>
          ))}
        </React.Fragment>
      )}
      {messages.map((m, i) =>
        m.role === "user" ? (
          <div key={i} className="msg-user">
            {m.context && (
              <div className="ctx">
                <span className="ctx-chip"><Icon name="wand" size={11} />{m.context}</span>
              </div>
            )}
            {m.image && <img className="att-img" src={m.image} alt="attachment" />}
            <div className="bubble">{m.text}</div>
          </div>
        ) : (
          <div key={i} className={"msg-ai" + (m.status === "error" ? " err" : "")}>
            <div className="card">
              <div className="head">
                <span className={"ic" + (m.status === "working" ? " spin" : "") + (m.status === "error" ? " alert" : "")}>
                  <Icon name={m.status === "working" ? "loader" : m.status === "error" ? "alert" : "check"} size={15} />
                </span>
                {m.title}
              </div>
              {m.desc && <div className="desc">{m.desc}</div>}
              {m.sources && m.sources.length > 0 && (
                <div className="src-list">
                  {m.sources.map((s, j) => (
                    <span key={j} className="src-chip"><Icon name="file" size={11} />{s}</span>
                  ))}
                </div>
              )}
              {m.status === "done" && m.nodeName && (
                <div className="foot">
                  <button className="mini-btn" onClick={m.onLocate}>Select on canvas</button>
                  <button className="mini-btn">Regenerate</button>
                </div>
              )}
              {m.status === "error" && (
                <div className="foot">
                  <button className="mini-btn" onClick={() => onRetry && onRetry(m.retry)}>
                    <Icon name="loader" size={12} />
                    Retry
                  </button>
                </div>
              )}
            </div>
          </div>
        )
      )}
    </div>
  );
}

function Composer({ selectedNode, onClearContext, onSend, busy, model, setModel, draft, setDraft }) {
  const { useState, useRef } = React;
  const [modelOpen, setModelOpen] = useState(false);
  const [attachment, setAttachment] = useState(null);
  const fileRef = useRef(null);
  const taRef = useRef(null);

  const prov = DESIGN_PROVIDERS.find((p) => p.id === model.provider);

  const send = () => {
    const text = draft.trim();
    if (!text || busy) return;
    onSend(text, attachment);
    setDraft("");
    setAttachment(null);
  };

  const onFile = (e) => {
    const f = e.target.files && e.target.files[0];
    if (!f) return;
    const reader = new FileReader();
    reader.onload = () => setAttachment({ name: f.name, url: reader.result });
    reader.readAsDataURL(f);
    e.target.value = "";
  };

  return (
    <div className="composer" data-screen-label="Composer">
      <div className="composer-box">
        {selectedNode && (
          <div className="composer-ctx">
            <span className="ctx-chip">
              <Icon name="wand" size={11} />
              Editing {selectedNode.name}
              <button className="x" onClick={onClearContext} title="Clear — generate new instead"><Icon name="x" size={11} /></button>
            </span>
          </div>
        )}
        {attachment && (
          <div className="composer-atts">
            <div className="att-thumb">
              <img src={attachment.url} alt={attachment.name} />
              <button className="rm" onClick={() => setAttachment(null)}><Icon name="x" size={9} /></button>
            </div>
          </div>
        )}
        <textarea
          ref={taRef}
          rows={2}
          placeholder={selectedNode
            ? `Describe the change to ${selectedNode.name}…`
            : "Describe what to generate…"}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); send(); }
          }}
        ></textarea>
        <div className="composer-bar">
          <button className="icon-btn" title="Attach image" onClick={() => fileRef.current.click()}>
            <Icon name="paperclip" size={16} />
          </button>
          <input ref={fileRef} type="file" accept="image/*" style={{ display: "none" }} onChange={onFile} />
          <div className="pop-wrap">
            <button className="model-chip" data-open={modelOpen} onClick={() => setModelOpen(!modelOpen)}>
              <span className="dot"></span>
              {prov.name} · {model.effort}
              <Icon name="chevDown" size={12} style={{ color: "var(--muted)" }} />
            </button>
            <ModelPopover open={modelOpen} onClose={() => setModelOpen(false)} model={model} setModel={setModel} />
          </div>
          <button className="send-btn" disabled={busy || !draft.trim()} onClick={send} title="Generate (Enter)">
            <Icon name={busy ? "loader" : "send"} size={16} style={busy ? { animation: "rot 1s linear infinite" } : null} />
          </button>
        </div>
      </div>
      <div className="composer-hint"><b>Enter</b> to send · <b>Shift+Enter</b> for a new line</div>
    </div>
  );
}

function AssistantPanel({ messages, selectedNode, onClearContext, onSend, onSuggest, onRetry, busy, model, setModel, draft, setDraft, width }) {
  const { useRef, useEffect } = React;
  const scrollRef = useRef(null);
  useEffect(() => {
    if (scrollRef.current) scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
  }, [messages]);

  return (
    <aside className="assist" style={width ? { width } : null} data-screen-label="Assistant panel">
      <div className="assist-head">
        <span className="spark"><Icon name="sparkles" size={17} /></span>
        <span className="ttl">Assistant</span>
        <span className="sub">{messages.filter((m) => m.role === "assistant").length > 0 ? `${messages.filter((m) => m.role === "assistant" && m.status === "done").length} generations` : ""}</span>
      </div>
      <AssistantMessages messages={messages} onSuggest={onSuggest} onRetry={onRetry} scrollRef={scrollRef} />
      <Composer
        selectedNode={selectedNode}
        onClearContext={onClearContext}
        onSend={onSend}
        busy={busy}
        model={model}
        setModel={setModel}
        draft={draft}
        setDraft={setDraft}
      />
    </aside>
  );
}

Object.assign(window, { AssistantPanel, ModelPopover, Composer });
