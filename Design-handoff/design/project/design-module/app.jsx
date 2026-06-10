// app.jsx — root: state, generation simulation, tweaks, mount

const TWEAK_DEFAULTS = /*EDITMODE-BEGIN*/{
  "accent": "#C14B1B",
  "panelSide": "right",
  "density": "comfortable"
}/*EDITMODE-END*/;

function App() {
  const { useState, useRef, useCallback, useEffect } = React;
  const [t, setTweak] = useTweaks(TWEAK_DEFAULTS);

  const [nodes, setNodes] = useState(INITIAL_NODES);
  const [selectedId, setSelectedId] = useState(null);
  const [messages, setMessages] = useState([]);
  const [busy, setBusy] = useState(false);
  const [model, setModel] = useState({ provider: "claude", effort: "High", timeout: 180 });
  const [draft, setDraft] = useState("");
  const [saveState, setSaveState] = useState("saved"); // saved | dirty | writing
  const [toastMsg, setToastMsg] = useState(null);
  const [fullscreen, setFullscreen] = useState(false);
  const [panelW, setPanelW] = useState(350);
  const [handoff, setHandoff] = useState(null);
  const genCount = useRef(0);
  const saveTimer = useRef(null);
  const toastTimer = useRef(null);
  const handoffTimers = useRef([]);

  /* ---- undo / redo history ---- */
  const nodesRef = useRef(nodes);
  nodesRef.current = nodes; // mirror latest committed nodes every render
  const histRef = useRef({ past: [], future: [] });
  const [, bumpHist] = useState(0);
  const pushHistory = useCallback((snap) => {
    const h = histRef.current;
    h.past.push(snap || nodesRef.current);
    if (h.past.length > 60) h.past.shift();
    h.future = [];
    bumpHist((v) => v + 1);
  }, []);
  const undo = useCallback(() => {
    const h = histRef.current;
    if (!h.past.length) return;
    h.future.push(nodesRef.current);
    const prev = h.past.pop();
    setSelectedId(null);
    setNodes(prev);
    bumpHist((v) => v + 1);
    onDirty();
  }, []);
  const redo = useCallback(() => {
    const h = histRef.current;
    if (!h.future.length) return;
    h.past.push(nodesRef.current);
    const next = h.future.pop();
    setSelectedId(null);
    setNodes(next);
    bumpHist((v) => v + 1);
    onDirty();
  }, []);

  /* keyboard: undo / redo */
  useEffect(() => {
    const onKey = (e) => {
      const tag = (e.target.tagName || "").toLowerCase();
      if (tag === "input" || tag === "textarea" || e.target.isContentEditable) return;
      const meta = e.metaKey || e.ctrlKey;
      if (meta && e.key.toLowerCase() === "z") {
        e.preventDefault();
        if (e.shiftKey) redo(); else undo();
      } else if (meta && e.key.toLowerCase() === "y") {
        e.preventDefault();
        redo();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [undo, redo]);

  const selectedNode = nodes.find((n) => n.id === selectedId) || null;

  const toast = (msg) => {
    setToastMsg(msg);
    clearTimeout(toastTimer.current);
    toastTimer.current = setTimeout(() => setToastMsg(null), 2400);
  };

  /* drag commit → throttled manifest write */
  const onDirty = useCallback(() => {
    setSaveState("dirty");
    clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(() => {
      setSaveState("writing");
      setTimeout(() => setSaveState("saved"), 650);
    }, 900);
  }, []);

  const onConsolidate = () => {
    setSaveState("writing");
    setTimeout(() => {
      setSaveState("saved");
      toast("Saved to working folder");
    }, 700);
  };

  /* ---- hand off to local agents: package → dispatch → agents wire up backend ---- */
  const onHandoff = () => {
    handoffTimers.current.forEach(clearTimeout);
    const base = AGENT_TASKS.map((t) => ({ ...t, status: "pending" }));
    setSaveState("writing");
    setHandoff({ phase: "packaging", tasks: base });
    const timers = [];
    let acc = 950;
    timers.push(setTimeout(() => {
      setSaveState("saved");
      setHandoff((h) => h && { ...h, phase: "dispatch" });
    }, acc));
    AGENT_TASKS.forEach((t, i) => {
      acc += 560;
      timers.push(setTimeout(() => setHandoff((h) => h && {
        ...h, tasks: h.tasks.map((x, j) => (j === i ? { ...x, status: "running" } : x)),
      }), acc));
      acc += 720;
      timers.push(setTimeout(() => setHandoff((h) => h && {
        ...h, tasks: h.tasks.map((x, j) => (j === i ? { ...x, status: "done" } : x)),
      }), acc));
    });
    acc += 360;
    timers.push(setTimeout(() => setHandoff((h) => h && { ...h, phase: "done" }), acc));
    handoffTimers.current = timers;
  };
  const closeHandoff = () => {
    handoffTimers.current.forEach(clearTimeout);
    handoffTimers.current = [];
    setHandoff(null);
  };

  const updateMsg = (idx, patch) =>
    setMessages((ms) => ms.map((m, i) => (i === idx ? { ...m, ...patch } : m)));

  /* assistant panel resize */
  const startPanelResize = (e) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = panelW;
    const side = t.panelSide;
    const onMove = (ev) => {
      const dx = ev.clientX - startX;
      const w = side === "right" ? startW - dx : startW + dx;
      setPanelW(Math.max(290, Math.min(540, w)));
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };

  /* ---- Spot Edit: region analysis simulation ---- */
  const onRegionAnalyze = (region, prompt, hits, doneCb) => {
    setBusy(true);
    const before = nodesRef.current;
    const names = hits.map((h) => h.name).join(", ") || "an empty canvas area";
    setMessages((ms) => [
      ...ms,
      { role: "user", text: prompt || "Auto-detect issues in the selected area", context: "Spot Edit" },
      { role: "assistant", status: "working", title: "Analyzing region…", desc: `Vision pass over ${names}.` },
    ]);
    const aiIdx = messages.length + 1;
    setTimeout(() => updateMsg(aiIdx, { sources: ORACLE_SOURCES.slice(0, 2) }), 900);
    setTimeout(() => {
      const hitCta = hits.some((h) => h.id === "cta");
      if (hitCta) {
        pushHistory(before);
        setNodes((ns) => ns.map((n) => (n.id === "cta" ? { ...n, html: NODE_HTML.ctaAlt } : n)));
      }
      updateMsg(aiIdx, {
        status: "done",
        title: hitCta ? "Spot fix applied to CTA" : "Analysis complete",
        desc: hitCta
          ? "Detected an off-token button fill in the region → swapped to color.accent. Placement untouched."
          : `Inspected ${names} — tokens, contrast and spacing are coherent. Nothing to fix.`,
        nodeName: hitCta ? "CTA" : null,
        onLocate: hitCta ? () => setSelectedId("cta") : null,
      });
      setBusy(false);
      onDirty();
      doneCb();
    }, 2600);
  };

  /* ---- generation simulation ---- */
  const onSend = (text, attachment) => {
    if (busy) return;
    const editing = selectedNode;
    const before = nodesRef.current;
    const willFail = /\b(fail|error|timeout|crash|break|broken)\b/i.test(text);
    setBusy(true);

    setMessages((ms) => [
      ...ms,
      { role: "user", text, context: editing ? `Editing ${editing.name}` : null, image: attachment ? attachment.url : null },
    ]);

    if (editing) {
      /* --- edit round-trip: only this node's markup is sent --- */
      const nodeId = editing.id;
      const aiIdx = messages.length + 1;
      setMessages((ms) => [...ms, {
        role: "assistant", status: "working",
        title: `Updating ${editing.name}…`,
        desc: "Sending only this node's markup. Placement stays untouched.",
      }]);
      setNodes((ns) => ns.map((n) => (n.id === nodeId ? { ...n, generating: true } : n)));

      setTimeout(() => updateMsg(aiIdx, { sources: ORACLE_SOURCES.slice(0, 2) }), 800);
      setTimeout(() => {
        setNodes((ns) => ns.map((n) => (n.id === nodeId ? { ...n, generating: false } : n)));
        if (willFail) {
          updateMsg(aiIdx, {
            status: "error",
            title: `Couldn't update ${editing.name}`,
            desc: `Provider timed out after ${model.timeout}s — the node was left untouched. Grounding context is preserved.`,
            retry: text,
          });
          setBusy(false);
          return;
        }
        pushHistory(before);
        setNodes((ns) => ns.map((n) =>
          n.id === nodeId
            ? { ...n, generating: false, html: nodeId === "cta" ? NODE_HTML.ctaAlt : n.html }
            : n
        ));
        updateMsg(aiIdx, {
          status: "done",
          title: `Updated ${editing.name}`,
          desc: "Markup swapped, manifest re-applied — node stayed put.",
          nodeName: editing.name,
          onLocate: () => setSelectedId(nodeId),
        });
        setBusy(false);
        onDirty();
      }, 2300);
    } else {
      /* --- new generation: ghost node streams in --- */
      const tmpl = GEN_TEMPLATES.find((g) => g.match.test(text)) || GEN_TEMPLATES[GEN_TEMPLATES.length - 1];
      genCount.current += 1;
      const id = "gen-" + genCount.current;
      const name = tmpl.name;

      setNodes((ns) => {
        const maxRight = Math.max(...ns.map((n) => n.x + n.w), 0);
        const maxZ = Math.max(...ns.map((n) => n.z), 0);
        const nx = ns.length ? maxRight + 56 : 96;
        return [...ns, { id, name, kind: "html", x: nx, y: 96, z: maxZ + 1, w: tmpl.w, html: "", generating: true }];
      });

      const aiIdx = messages.length + 1;
      setMessages((ms) => [...ms, {
        role: "assistant", status: "working",
        title: `Generating ${name}…`,
        desc: "Grounding on devboule via Oracle…",
      }]);

      setTimeout(() => updateMsg(aiIdx, {
        desc: "Streaming markup — placement is assigned deterministically.",
        sources: ORACLE_SOURCES,
      }), 900);

      setTimeout(() => {
        if (willFail) {
          setNodes((ns) => ns.filter((n) => n.id !== id));
          updateMsg(aiIdx, {
            status: "error",
            title: `Generation failed`,
            desc: `Provider timed out after ${model.timeout}s — no node was added. Retry or lower the effort.`,
            retry: text,
          });
          setBusy(false);
          return;
        }
        pushHistory(before);
        setNodes((ns) => ns.map((n) => (n.id === id ? { ...n, html: tmpl.html, generating: false } : n)));
        updateMsg(aiIdx, {
          status: "done",
          title: `Added ${name}`,
          desc: "1 node · grounded on 3 files · logged to generations.jsonl",
          nodeName: name,
          onLocate: () => setSelectedId(id),
        });
        setBusy(false);
        setSelectedId(id);
        onDirty();
      }, 2600);
    }
  };

  const canUndo = histRef.current.past.length > 0;
  const canRedo = histRef.current.future.length > 0;

  return (
    <div className="app" data-density={t.density} data-fullscreen={fullscreen} style={{ "--accent": t.accent }}>
      {!fullscreen && <AppSidebar />}
      <div className="main">
        <TopBar saveState={saveState} onConsolidate={onConsolidate} onHandoff={onHandoff} toast={toast} fullscreen={fullscreen} onToggleFullscreen={() => setFullscreen(!fullscreen)} onUndo={undo} onRedo={redo} canUndo={canUndo} canRedo={canRedo} />
        <div className="work" data-side={t.panelSide} data-screen-label="Design workspace">
          <DesignCanvas
            nodes={nodes}
            setNodes={setNodes}
            selectedId={selectedId}
            setSelectedId={setSelectedId}
            onDirty={onDirty}
            onBeginChange={pushHistory}
            onRegionAnalyze={onRegionAnalyze}
          />
          <div className="panel-resizer" onPointerDown={startPanelResize} title="Drag to resize"></div>
          <AssistantPanel
            width={panelW}
            messages={messages}
            selectedNode={selectedNode}
            onClearContext={() => setSelectedId(null)}
            onSend={onSend}
            onRetry={(txt) => onSend(txt, null)}
            onSuggest={(s) => setDraft(s)}
            busy={busy}
            model={model}
            setModel={setModel}
            draft={draft}
            setDraft={setDraft}
          />
        </div>
      </div>
      <Toast msg={toastMsg} />
      <HandoffModal handoff={handoff} onClose={closeHandoff} />
      <TweaksPanel>
        <TweakSection label="Brand" />
        <TweakColor
          label="Accent intensity"
          value={t.accent}
          options={["#D97757", "#C14B1B", "#9E3A0C"]}
          onChange={(v) => setTweak("accent", v)}
        />
        <TweakSection label="Layout" />
        <TweakRadio
          label="Assistant panel"
          value={t.panelSide}
          options={["left", "right"]}
          onChange={(v) => setTweak("panelSide", v)}
        />
        <TweakRadio
          label="Density"
          value={t.density}
          options={["comfortable", "compact"]}
          onChange={(v) => setTweak("density", v)}
        />
      </TweaksPanel>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("root")).render(<App />);
