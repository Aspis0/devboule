// content-edit.jsx — inside-node direct editing: element select, inline text, token colors, reorder

const TOKEN_COLORS = [
  { name: "color.accent", v: "#C14B1B" },
  { name: "color.ink", v: "#37291A" },
  { name: "color.muted", v: "#7A6B56" },
  { name: "color.cream", v: "#F3E3CB" },
  { name: "color.paper", v: "#FFFFFF" },
];

/* serialize edited DOM back to clean node markup (strip helper classes/attrs) */
function cleanSerialize(container) {
  const clone = container.cloneNode(true);
  clone.querySelectorAll("*").forEach((el) => {
    el.removeAttribute("class");
    el.removeAttribute("contenteditable");
    el.removeAttribute("spellcheck");
  });
  return clone.innerHTML;
}

function elHasText(el) {
  return el && Array.from(el.childNodes).some((n) => n.nodeType === 3 && n.textContent.trim());
}

/* make an element inline-editable until blur / Enter / Esc */
function startInlineTextEdit(el, onDone) {
  if (!el) return;
  el.setAttribute("contenteditable", "true");
  el.setAttribute("spellcheck", "false");
  el.focus();
  const r = document.createRange();
  r.selectNodeContents(el);
  r.collapse(false);
  const s = window.getSelection();
  s.removeAllRanges();
  s.addRange(r);
  const kd = (e) => {
    if ((e.key === "Enter" && !e.shiftKey) || e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      el.blur();
    }
  };
  const end = () => {
    el.removeAttribute("contenteditable");
    el.removeAttribute("spellcheck");
    el.removeEventListener("blur", end);
    el.removeEventListener("keydown", kd);
    if (onDone) onDone();
  };
  el.addEventListener("blur", end);
  el.addEventListener("keydown", kd);
}

function ContentToolbar({ el, wrapRef, version, pan, zoom, onEditText, onColor, onMove, onRemove, onAskAi }) {
  const { useState, useLayoutEffect } = React;
  const [pos, setPos] = useState(null);
  const [mode, setMode] = useState("text");

  useLayoutEffect(() => {
    if (!el || !el.isConnected || !wrapRef.current) { setPos(null); return; }
    const er = el.getBoundingClientRect();
    const wr = wrapRef.current.getBoundingClientRect();
    let top = er.top - wr.top - 46;
    if (top < 8) top = er.bottom - wr.top + 10;
    let left = er.left - wr.left;
    left = Math.max(8, Math.min(left, wr.width - 360));
    setPos({ top, left });
  }, [el, version, pan, zoom]);

  if (!el || !pos) return null;
  const canText = elHasText(el);

  return (
    <div className="ce-toolbar" style={{ top: pos.top, left: pos.left }} onPointerDown={(e) => e.stopPropagation()}>
      <span className="ce-tag">{el.tagName.toLowerCase()}</span>
      <button className="tb" disabled={!canText} title={canText ? "Edit text (or double-click it)" : "No direct text here"} onClick={onEditText}>
        <Icon name="type" size={14} />
      </button>
      <span className="sep"></span>
      <button className={"tb" + (mode === "text" ? " sel" : "")} title="Apply swatches to text color" onClick={() => setMode("text")}>
        <span style={{ fontWeight: 700, fontSize: 12.5 }}>A</span>
      </button>
      <button className={"tb" + (mode === "fill" ? " sel" : "")} title="Apply swatches to fill" onClick={() => setMode("fill")}>
        <span style={{ width: 11, height: 11, borderRadius: 3, background: "currentColor", display: "block", opacity: .65 }}></span>
      </button>
      {TOKEN_COLORS.map((c) => (
        <button
          key={c.name}
          className="ce-sw"
          title={`${c.name} → ${mode === "fill" ? "fill" : "text"}`}
          style={{ background: c.v }}
          onClick={() => onColor(c.v, mode)}
        ></button>
      ))}
      <span className="sep"></span>
      <button className="tb" title="Move earlier in layout" onClick={() => onMove("up")}><Icon name="up" size={13} /></button>
      <button className="tb" title="Move later in layout" onClick={() => onMove("down")}><Icon name="down" size={13} /></button>
      <button className="tb" title="Remove element (Del)" onClick={onRemove}><Icon name="trash" size={13} /></button>
      <span className="sep"></span>
      <button className="tb" title="Ask the AI to change this element" onClick={onAskAi} style={{ color: "var(--accent)" }}>
        <Icon name="sparkles" size={14} />
      </button>
    </div>
  );
}

Object.assign(window, { TOKEN_COLORS, cleanSerialize, elHasText, startInlineTextEdit, ContentToolbar });
