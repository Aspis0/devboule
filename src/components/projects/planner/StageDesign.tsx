import { useState, useRef, useEffect, type CSSProperties } from 'react';
import { ArrowRight, Check, ImageOff, Loader, Play, Maximize2 } from 'lucide-react';
import { generateAndRegisterDesign } from '../../design/generation/generateAndRegister';
import { ArtifactView } from '../artifact/ArtifactView';
import { ArtifactFrame } from '../artifact/ArtifactFrame';
import { inferFrameKind } from '../artifact/frameHeuristic';
import type { ArtifactKind, ArtifactFrameKind } from '../../../types/design';

interface StageDesignProps {
  design: {
    name: string;
    version: string | null;
    ago: string | null;
    thumbnailUri: string | null;
    /** Registry entry id — required for the task-link command. */
    id?: string;
    /** Phase 3: present when the registry entry is an interactive artifact. */
    kind?: ArtifactKind;
    /** Phase 3: the registry entry id — used to build the artifact:// URL. */
    artifactId?: string;
    /** Phase 4: device-frame skin stored on the registry entry. Absent ⇒ inferred. */
    frame?: ArtifactFrameKind;
  } | null;
  linkedTask: number | null;
  onOpenInDesign: () => void;
  /** Phase 3: absolute project root path — used to construct working folder paths. */
  projectRoot: string | null;
  /** Phase 3: called after a design is generated so the parent can refresh its list. */
  onGenerated: () => void;
  /**
   * Phase 5: fired when an INTERACTIVE artifact's display state changes.
   * Receives `true` when the artifact is being shown, `false` when it is closed
   * or when the component unmounts (so the parent's hold can never get stuck).
   * Only fires for `kind === 'interactive'`; static previews do not affect rotation.
   */
  onArtifactActiveChange?: (active: boolean) => void;
  /**
   * OPTIONAL task list for the "Attach to task" selector. When non-empty AND the
   * design has a registry id, a small select is shown near the linkedTask badge.
   */
  tasks?: { n: number; title: string }[];
  /**
   * Called when the user selects a task (n = 1-based number) or picks "— none —"
   * (n = null). The parent invokes the backend command and triggers a reload.
   * Returns a Promise so the caller can disable the select while the command is
   * in-flight (prevents concurrent-change races) and surface errors on failure.
   */
  onLinkTask?: (n: number | null) => void | Promise<void>;
}

const FRAME_OPTIONS: { value: ArtifactFrameKind | ''; label: string }[] = [
  { value: '', label: 'Default' },
  { value: 'android', label: 'Android' },
  { value: 'ios', label: 'iOS' },
  { value: 'web', label: 'Browser' },
  { value: 'component', label: 'Component' },
];

export const StageDesign: React.FC<StageDesignProps> = ({
  design,
  linkedTask,
  onOpenInDesign,
  projectRoot,
  onGenerated,
  onArtifactActiveChange,
  tasks,
  onLinkTask,
}) => {
  // Always-current ref for onArtifactActiveChange — mirrors ArtifactView's onReadyRef
  // pattern. This lets the unmount cleanup (Effect 2) call the LATEST prop value without
  // capturing a stale closure, and removes the need for an eslint-disable comment.
  const onArtifactActiveChangeRef = useRef(onArtifactActiveChange);
  useEffect(() => {
    onArtifactActiveChangeRef.current = onArtifactActiveChange;
  });

  // --- user-trigger form state ---
  const [promptInput, setPromptInput] = useState('');
  const [modeInput, setModeInput] = useState<'interactive' | 'static'>('interactive');
  const [frameInput, setFrameInput] = useState<ArtifactFrameKind | ''>('');
  const [isGenerating, setIsGenerating] = useState(false);
  const [genError, setGenError] = useState<string | null>(null);
  // Task-link in-flight guard: disables the select while a command is pending so rapid
  // consecutive changes cannot race (last click may not win).
  const [isLinking, setIsLinking] = useState(false);
  // Inline error shown near the task-link select when the backend command fails.
  const [linkError, setLinkError] = useState<string | null>(null);
  // The registry id of the most-recently generated artifact (shows the Open button inline).
  const [localArtifactId, setLocalArtifactId] = useState<string | null>(null);
  // Whether to show ArtifactView inline in the left panel.
  const [showArtifact, setShowArtifact] = useState(false);
  const promptRef = useRef<HTMLInputElement>(null);
  // Fix 5: synchronous reentrancy lock so two rapid clicks/Enter presses cannot both
  // pass the isGenerating guard before React re-renders the disabled button.
  const generatingRef = useRef(false);

  // Resolved artifact id: prefer the just-generated local one, fall back to the
  // registry-loaded one (surfaced via the `design` prop after a nonce refresh).
  const artifactId =
    localArtifactId ??
    (design?.kind === 'interactive' ? design.artifactId : undefined) ??
    null;

  // Effective frame kind for ArtifactFrame:
  //   1. Explicit user pick from the frame selector dropdown (frameInput !== '')
  //   2. Stored on the registry entry (design.frame — set at generation time)
  //   3. Heuristic inference from the prompt (last resort default)
  // The user override ALWAYS wins; the heuristic only pre-selects when no explicit
  // choice is present (per plan requirement: "user's explicit frame selector ALWAYS WINS").
  const effectiveFrameKind: ArtifactFrameKind =
    (frameInput !== '' ? frameInput : null) ??
    design?.frame ??
    inferFrameKind(promptInput);

  // Phase 5 — hold-rotation signal ----------------------------------------
  // True iff an interactive artifact is currently displayed inline.
  // `artifactId` is non-null ONLY for interactive artifacts: `localArtifactId`
  // is set exclusively when `modeInput === 'interactive'` (see handleGenerate),
  // and the design-prop fallback already gates on `design?.kind === 'interactive'`.
  // Static generations leave both paths null, so this invariant is always
  // satisfied — no static design can cause isInteractiveAndOpen to be true.
  const isInteractiveAndOpen = showArtifact && artifactId !== null;

  // Effect 1: emit the current active state whenever it changes.
  // No cleanup return here — we intentionally avoid the cleanup-then-re-run
  // pattern that would spuriously emit false before emitting true when the
  // artifact is opened (no flicker for the parent's hold state).
  useEffect(() => {
    onArtifactActiveChange?.(isInteractiveAndOpen);
  }, [isInteractiveAndOpen, onArtifactActiveChange]);

  // Effect 2: unmount-only guard.
  // If the Design stage is rotated away while an interactive artifact is live,
  // this component unmounts without Effect 1 re-running. The cleanup here
  // ensures the parent's `artifactActive` state is reset to false so the
  // rotation hold is never stuck. Using the always-current ref (updated by the
  // preceding effect on every render) avoids the stale-closure risk when a
  // future caller passes the optional prop conditionally.
  useEffect(() => {
    return () => {
      onArtifactActiveChangeRef.current?.(false);
    };
  }, []);

  const handleGenerate = async () => {
    const prompt = promptInput.trim();
    // Fix 5: check the ref synchronously BEFORE the async state check so two rapid clicks
    // both arriving in the same render cycle cannot both slip through. The ref is the
    // reentrancy lock; isGenerating remains the UI disabled/spinner signal.
    if (generatingRef.current || !prompt || !projectRoot || isGenerating) return;
    generatingRef.current = true;
    setIsGenerating(true);
    setGenError(null);
    setLocalArtifactId(null);
    setShowArtifact(false);
    try {
      // Unique working folder per generation (timestamp-based; no UUID dep required).
      const workingFolderPath = `${projectRoot}/.aspis-design/design-${Date.now()}`;
      const id = await generateAndRegisterDesign({
        mode: modeInput,
        // Persist the RESOLVED frame, not the raw dropdown value. `effectiveFrameKind` is
        // `dropdown ?? design.frame ?? inferFrameKind(promptInput)`, so a heuristic-inferred
        // skin (e.g. "iOS dashboard" → ios with no explicit pick) is stored on the registry
        // entry and survives remount — passing `frameInput` alone would store `undefined`,
        // and after a reload `inferFrameKind('')` (promptInput is reset) falls back to
        // `component`, rendering the artifact bare. The explicit dropdown still wins because
        // it is first in the `effectiveFrameKind` chain. `effectiveFrameKind` is a derived
        // value recomputed every render, and `handleGenerate` is recreated each render too,
        // so this closure always reads the current value (no stale-closure risk).
        frame: effectiveFrameKind,
        prompt,
        workingFolderPath,
        designName: prompt.slice(0, 60),
      });
      // Only interactive generations produce an artifact:// entry — static designs
      // open via the existing onOpenInDesign / Design-canvas flow, never via the
      // artifact iframe. Setting localArtifactId for static would wrongly show
      // the Maximize/Open-artifact buttons and freeze the stage rotation hold.
      if (modeInput === 'interactive') {
        setLocalArtifactId(id);
        setShowArtifact(true);
      }
      // Refresh the parent's design list so the Stage shows the new entry.
      onGenerated();
    } catch (e) {
      setGenError(e instanceof Error ? e.message : String(e));
    } finally {
      generatingRef.current = false;
      setIsGenerating(false);
    }
  };

  // -----------------------------------------------------------------------
  // Styles
  // -----------------------------------------------------------------------

  const designName = design?.name ?? 'No design yet';
  const initial = design?.name?.charAt(0).toUpperCase() ?? 'A';

  const rootStyle: CSSProperties = {
    flex: 1,
    display: 'flex',
    flexDirection: 'column',
    gap: 10,
    minHeight: 0,
  };

  const generateRowStyle: CSSProperties = {
    display: 'flex',
    alignItems: 'center',
    gap: 7,
    padding: '7px 10px',
    background: '#FBF3E8',
    border: '1px solid #EFE7DA',
    borderRadius: 9,
    flexShrink: 0,
  };

  const inputStyle: CSSProperties = {
    flex: 1,
    minWidth: 0,
    height: 28,
    border: '1px solid #E4DDD0',
    borderRadius: 7,
    padding: '0 9px',
    fontSize: 12,
    color: '#2A2621',
    background: '#fff',
    outline: 'none',
  };

  const selectStyle: CSSProperties = {
    height: 28,
    border: '1px solid #E4DDD0',
    borderRadius: 7,
    padding: '0 6px',
    fontSize: 11,
    color: '#2A2621',
    background: '#fff',
    cursor: 'pointer',
  };

  const modeToggleStyle = (active: boolean): CSSProperties => ({
    height: 28,
    padding: '0 10px',
    border: `1px solid ${active ? '#C0894F' : '#E4DDD0'}`,
    borderRadius: 7,
    background: active ? '#F1E4D2' : '#fff',
    color: active ? '#9A6A2E' : '#9c9488',
    fontSize: 11,
    fontWeight: active ? 700 : 400,
    cursor: 'pointer',
  });

  const generateBtnStyle: CSSProperties = {
    height: 28,
    padding: '0 11px',
    border: 'none',
    borderRadius: 7,
    background: isGenerating
      ? '#CFC6B6'
      : 'linear-gradient(135deg,#C8945C,#B07D43)',
    color: '#FBF6EF',
    fontSize: 11.5,
    fontWeight: 700,
    cursor: isGenerating ? 'default' : 'pointer',
    display: 'flex',
    alignItems: 'center',
    gap: 5,
    flexShrink: 0,
  };

  const contentRowStyle: CSSProperties = {
    flex: 1,
    display: 'flex',
    gap: 14,
    minHeight: 0,
  };

  const leftPanelStyle: CSSProperties = {
    flex: 1,
    minWidth: 0,
    background: '#fff',
    border: '1px solid #E4DDD0',
    borderRadius: 10,
    overflow: 'hidden',
    boxShadow: '0 12px 30px -18px rgba(0,0,0,.25)',
    display: 'flex',
    flexDirection: 'column',
  };

  const topBarStyle: CSSProperties = {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    padding: '9px 12px',
    borderBottom: '1px solid #EFE7DA',
    background: '#FBF3E8',
  };

  const badgeStyle: CSSProperties = {
    width: 18,
    height: 18,
    background: '#C0894F',
    color: '#fff',
    borderRadius: 5,
    fontSize: 9,
    fontWeight: 700,
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    flexShrink: 0,
  };

  const titleStyle: CSSProperties = {
    fontSize: 12,
    fontWeight: 700,
    color: '#2A2621',
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    flex: 1,
  };

  const bodyStyle: CSSProperties = {
    flex: 1,
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    padding: 12,
    overflow: 'hidden',
  };

  const imgStyle: CSSProperties = {
    maxWidth: '100%',
    maxHeight: '100%',
    objectFit: 'contain',
    borderRadius: 6,
  };

  const rightPanelStyle: CSSProperties = {
    flex: 'none',
    width: 178,
    display: 'flex',
    flexDirection: 'column',
  };

  const monoLabelStyle: CSSProperties = {
    fontSize: 9.5,
    letterSpacing: '.14em',
    color: '#A89F90',
  };

  const nameStyle: CSSProperties = {
    fontSize: 13.5,
    fontWeight: 700,
    color: '#2A2621',
    marginTop: 9,
  };

  const metaStyle: CSSProperties = {
    fontSize: 10,
    color: '#9c9488',
    marginTop: 3,
  };

  const badgeContainerStyle: CSSProperties = {
    marginTop: 10,
    alignSelf: 'flex-start',
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    fontSize: 10,
    fontWeight: 600,
    color: '#9A6A2E',
    background: '#F1E4D2',
    border: '1px solid #E6D3BB',
    padding: '4px 9px',
    borderRadius: 7,
  };

  const descriptionStyle: CSSProperties = {
    fontSize: 11.5,
    color: '#7c766b',
    lineHeight: 1.5,
    marginTop: 12,
  };

  const buttonStyle: CSSProperties = {
    marginTop: 'auto',
    height: 38,
    border: 'none',
    background: 'linear-gradient(150deg,#C8945C,#B07D43)',
    borderRadius: 10,
    color: '#FBF6EF',
    fontSize: 12.5,
    fontWeight: 700,
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 7,
    cursor: design ? 'pointer' : 'default',
    opacity: design ? 1 : 0.5,
  };

  const openArtifactBtnStyle: CSSProperties = {
    marginTop: 10,
    height: 32,
    border: '1px solid #C0894F',
    borderRadius: 9,
    background: showArtifact ? '#F1E4D2' : '#fff',
    color: '#9A6A2E',
    fontSize: 11.5,
    fontWeight: 600,
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 6,
    cursor: 'pointer',
  };

  const emptyStateStyle: CSSProperties = {
    display: 'flex',
    flexDirection: 'column',
    alignItems: 'center',
    justifyContent: 'center',
    textAlign: 'center',
  };

  const errorStyle: CSSProperties = {
    padding: '6px 10px',
    background: '#fdecec',
    border: '1px solid #f0c9c2',
    borderRadius: 7,
    color: '#8a2a1d',
    fontSize: 11,
    marginTop: 4,
    wordBreak: 'break-word',
  };

  return (
    <div className="pp-view-enter" style={rootStyle}>
      {/* TOP ROW — Generate affordance (Phase 3) */}
      <div style={generateRowStyle}>
        <input
          ref={promptRef}
          style={inputStyle}
          type="text"
          placeholder="Describe a UI screen or component…"
          value={promptInput}
          onChange={(e) => setPromptInput(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter') void handleGenerate(); }}
          disabled={isGenerating || !projectRoot}
          aria-label="Design prompt"
        />

        {/* Mode toggle: interactive / static */}
        <button
          type="button"
          style={modeToggleStyle(modeInput === 'interactive')}
          onClick={() => setModeInput('interactive')}
          aria-pressed={modeInput === 'interactive'}
          title="Generate an interactive artifact (real JS)"
        >
          interactive
        </button>
        <button
          type="button"
          style={modeToggleStyle(modeInput === 'static')}
          onClick={() => setModeInput('static')}
          aria-pressed={modeInput === 'static'}
          title="Generate a static mockup (DOMPurify-sanitized)"
        >
          static
        </button>

        {/* Frame selector */}
        <select
          style={selectStyle}
          value={frameInput}
          onChange={(e) => setFrameInput(e.target.value as ArtifactFrameKind | '')}
          disabled={isGenerating || modeInput === 'static'}
          aria-label="Frame"
          title={modeInput === 'static' ? 'Frames apply to interactive mode only' : 'Device frame'}
        >
          {FRAME_OPTIONS.map((opt) => (
            <option key={opt.value} value={opt.value}>{opt.label}</option>
          ))}
        </select>

        <button
          type="button"
          style={generateBtnStyle}
          onClick={() => void handleGenerate()}
          disabled={isGenerating || !promptInput.trim() || !projectRoot}
          aria-label={isGenerating ? 'Generating…' : 'Generate'}
        >
          {isGenerating ? (
            <Loader size={12} style={{ animation: 'spin 1s linear infinite' }} />
          ) : (
            <Play size={12} />
          )}
          {isGenerating ? 'Generating…' : 'Generate'}
        </button>
      </div>

      {genError && <div style={errorStyle} role="alert">{genError}</div>}

      {/* CONTENT ROW — left panel (preview / artifact) + right meta panel */}
      <div style={contentRowStyle}>
        {/* LEFT PANEL */}
        <div style={leftPanelStyle}>
          {/* TOP BAR */}
          <div style={topBarStyle}>
            <div style={badgeStyle}>
              {initial}
            </div>
            <div style={titleStyle} title={designName}>
              {designName}
            </div>
            {artifactId && (
              <button
                type="button"
                title={showArtifact ? 'Hide artifact' : 'Open interactive artifact'}
                onClick={() => setShowArtifact((v) => !v)}
                style={{
                  background: 'none',
                  border: 'none',
                  cursor: 'pointer',
                  color: showArtifact ? '#C0894F' : '#9c9488',
                  display: 'flex',
                  alignItems: 'center',
                  padding: 0,
                  flexShrink: 0,
                }}
                aria-pressed={showArtifact}
              >
                <Maximize2 size={13} />
              </button>
            )}
          </div>

          {/* BODY */}
          <div style={bodyStyle}>
            {showArtifact && artifactId ? (
              <div style={{ width: '100%', height: '100%', overflow: 'auto' }}>
                {/* Phase 4: wrap in the correct device-frame skin. Fixed-dimension skins
                    (android/ios/web) get autoResize=false so the iframe fills and scrolls
                    internally without bursting the bezel. The bare `component` skin gets
                    autoResize=true (content-height auto-grow, the original behaviour). */}
                <ArtifactFrame kind={effectiveFrameKind} viewport="mobile">
                  <ArtifactView
                    artifactId={artifactId}
                    title={designName}
                    minHeight={180}
                    autoResize={effectiveFrameKind === 'component'}
                  />
                </ArtifactFrame>
              </div>
            ) : design && design.thumbnailUri ? (
              <img
                src={design.thumbnailUri}
                alt={design.name}
                style={imgStyle}
              />
            ) : (
              <div style={emptyStateStyle}>
                <ImageOff size={28} color="#C9BEA9" />
                <p style={{ fontSize: 11.5, color: '#9c8d77', marginTop: 8 }}>
                  {projectRoot
                    ? 'Enter a prompt above and press Generate.'
                    : 'Open a project to generate a design.'}
                </p>
              </div>
            )}
          </div>
        </div>

        {/* RIGHT PANEL */}
        <div style={rightPanelStyle}>
          <div className="pp-mono" style={monoLabelStyle}>
            FROM DESIGN
          </div>
          <div style={nameStyle}>
            {design?.name ?? '—'}
          </div>
          <div className="pp-mono" style={metaStyle}>
            {design
              ? `Design · ${design.version ?? 'v1'} · ${design.ago ?? 'recent'}`
              : 'no design'}
          </div>

          {linkedTask != null && (
            <div style={badgeContainerStyle}>
              <Check size={11} />
              <span>linked to task {linkedTask}</span>
            </div>
          )}

          {/* Task-link selector: shown only when there are plan tasks AND the design
              has a registry id. Detach = "— none —" (calls onLinkTask(null)).
              The select is disabled while a link command is in-flight (isLinking) to
              prevent concurrent-change races: only one change can be pending at a time. */}
          {tasks && tasks.length > 0 && design?.id && onLinkTask && (
            <div style={{ marginTop: 8 }}>
              <div className="pp-mono" style={{ ...monoLabelStyle, marginBottom: 4 }}>
                ATTACH TO TASK
              </div>
              <select
                style={selectStyle}
                value={linkedTask ?? ''}
                disabled={isLinking}
                onChange={async (e) => {
                  if (isLinking) return;
                  const val = e.target.value;
                  const parsed = val === '' ? null : parseInt(val, 10);
                  setIsLinking(true);
                  setLinkError(null);
                  try {
                    await onLinkTask(parsed);
                  } catch {
                    setLinkError('Could not update task link — try again.');
                  } finally {
                    setIsLinking(false);
                  }
                }}
                aria-label="Attach design to task"
              >
                <option value="">— none —</option>
                {tasks.map((t) => (
                  <option key={t.n} value={t.n}>
                    #{t.n} {t.title.length > 32 ? t.title.slice(0, 32) + '…' : t.title}
                  </option>
                ))}
              </select>
              {linkError && (
                <div
                  role="alert"
                  style={{ marginTop: 4, fontSize: 11, color: '#c0392b' }}
                >
                  {linkError}
                </div>
              )}
            </div>
          )}

          {design != null && (
            <div style={descriptionStyle}>
              {design.kind === 'interactive'
                ? 'Interactive artifact — click the expand icon to preview it live.'
                : 'The Orchestrator pulled this screen so the matching task ships pixel-matched.'}
            </div>
          )}

          {/* Open artifact inline (Phase 3 — full Stage integration is Phase 5) */}
          {artifactId && (
            <button
              type="button"
              style={openArtifactBtnStyle}
              onClick={() => setShowArtifact((v) => !v)}
            >
              <Maximize2 size={12} />
              {showArtifact ? 'Hide artifact' : 'Open artifact'}
            </button>
          )}

          <button
            type="button"
            style={buttonStyle}
            onClick={onOpenInDesign}
            disabled={!design}
          >
            Open in Design
            <ArrowRight size={14} />
          </button>
        </div>
      </div>
    </div>
  );
};
