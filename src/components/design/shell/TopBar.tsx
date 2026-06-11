// TopBar — the design module's chrome bar, pixel-faithful to the prototype's
// TopBar (shell.jsx). LEFT: project button (-> ProjectPopover) + path chip + save
// status dot. RIGHT: undo/redo history group, Oracle chip (-> OraclePopover), ghost
// Export (-> ExportPopover), the Save split-button (-> SaveMenuPopover), and a
// focus/fullscreen toggle.
//
// All data + handlers are owned by DesignView and passed in. The four popover
// open-states live here (purely local UI state). Each trigger sits in a `.pop-wrap`
// anchor so the popover positions against it.

import { useState } from "react";
import {
  Palette,
  Folder,
  ChevronDown,
  Undo2,
  Redo2,
  Save,
  Code2,
  Eye,
  Maximize2,
  Minimize2,
} from "lucide-react";
import type {
  DesignProjectEntry,
  DesignOracleStatus,
} from "../../../types/design";
import type { DtcgDocument } from "../engine/tokens";
import type { ExportMode } from "../export/exportCode";
import type { SaveState } from "./useSaveState";
import { ProjectPopover } from "./ProjectPopover";
import { OraclePopover } from "./OraclePopover";
import { ExportPopover } from "./ExportPopover";
import { SaveMenuPopover } from "./SaveMenuPopover";

export interface TopBarProps {
  // ---- project / status ----
  projectName: string;
  workingFolderPath: string;
  /** True once a working folder is open (renders the path chip; enables actions). */
  projectOpen: boolean;
  saveState: SaveState;
  /** Save in flight (disables the split Save button). */
  saving: boolean;
  busy: boolean;

  // ---- history ----
  canUndo: boolean;
  canRedo: boolean;
  onUndo: () => void;
  onRedo: () => void;

  // ---- fullscreen / focus ----
  fullscreen: boolean;
  onToggleFullscreen: () => void;

  // ---- project popover data/handlers ----
  recent: DesignProjectEntry[];
  renamingId: string | null;
  renameDraft: string;
  setRenameDraft: (v: string) => void;
  beginRename: (entry: DesignProjectEntry) => void;
  commitRename: (id: string) => void;
  cancelRename: () => void;
  removeEntry: (id: string) => void;
  openEntry: (entry: DesignProjectEntry) => void;
  onNewProject: () => void;
  onOpenFolder: () => void;
  /** Open the design.md contract editor for the current project. Undefined disables
   * the row (no project open). */
  onEditContract?: () => void;

  // ---- oracle popover ----
  oracleStatus?: DesignOracleStatus;
  tokens: DtcgDocument;
  invoke: <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
  tauri: boolean;

  // ---- export / save ----
  runExport: (mode: ExportMode) => void;
  exportTokens: () => void;
  onConsolidate: () => void;

  // ---- preview ----
  /** Open the read-only preview window (absolute layout). */
  onPreview: () => void;
  /** True while a preview export/open is in flight (disables the button). */
  previewing: boolean;
}

export function TopBar(props: TopBarProps) {
  const [projOpen, setProjOpen] = useState(false);
  const [oracleOpen, setOracleOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [saveOpen, setSaveOpen] = useState(false);

  const {
    projectName,
    workingFolderPath,
    projectOpen,
    saveState,
    saving,
    busy,
    canUndo,
    canRedo,
    onUndo,
    onRedo,
    fullscreen,
    onToggleFullscreen,
    recent,
    renamingId,
    renameDraft,
    setRenameDraft,
    beginRename,
    commitRename,
    cancelRename,
    removeEntry,
    openEntry,
    onNewProject,
    onOpenFolder,
    onEditContract,
    oracleStatus,
    tokens,
    invoke,
    tauri,
    runExport,
    exportTokens,
    onConsolidate,
    onPreview,
    previewing,
  } = props;

  const statusText =
    saveState === "saved"
      ? "Saved"
      : saveState === "writing"
        ? "Saving…"
        : "Unsaved changes";
  // `.tb-status[data-state]` styles "clean" (green) vs "dirty"/"writing" (amber).
  const statusState = saveState === "saved" ? "clean" : saveState;

  const grounded = oracleStatus?.grounded === true;
  const oracleLabel = grounded
    ? `Grounded · ${oracleStatus?.rootLabel ?? "target"}`
    : "Not grounded";

  return (
    <header className="topbar" data-screen-label="Top bar">
      <div className="tb-title">
        <div className="pop-wrap">
          <button
            type="button"
            className="tb-proj"
            data-open={projOpen}
            onClick={() => setProjOpen((v) => !v)}
          >
            <Palette size={17} style={{ color: "var(--accent)" }} />
            {projectName || "No project"}
            <span className="chev">
              <ChevronDown size={14} />
            </span>
          </button>
          <ProjectPopover
            open={projOpen}
            onClose={() => setProjOpen(false)}
            recent={recent}
            currentFolder={workingFolderPath}
            busy={busy}
            renamingId={renamingId}
            renameDraft={renameDraft}
            setRenameDraft={setRenameDraft}
            beginRename={beginRename}
            commitRename={commitRename}
            cancelRename={cancelRename}
            removeEntry={removeEntry}
            openEntry={openEntry}
            onNewProject={onNewProject}
            onOpenFolder={onOpenFolder}
            onEditContract={onEditContract}
            invoke={invoke}
            tauri={tauri}
          />
        </div>
        {projectOpen && (
          <span
            className="tb-path"
            title={workingFolderPath}
            style={{
              overflow: "hidden",
              textOverflow: "ellipsis",
              maxWidth: 280,
            }}
          >
            <Folder size={13} />
            <span
              style={{
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
              }}
            >
              {workingFolderPath}
            </span>
          </span>
        )}
        <span className="tb-status" data-state={statusState} data-testid="tb-status">
          <span className="dot" />
          {statusText}
        </span>
      </div>

      <div className="tb-right">
        <div className="hist-group">
          <button
            type="button"
            className="icon-btn-tb"
            disabled={!canUndo}
            onClick={onUndo}
            title="Undo (Ctrl+Z)"
            aria-label="Undo"
          >
            <Undo2 size={16} />
          </button>
          <button
            type="button"
            className="icon-btn-tb"
            disabled={!canRedo}
            onClick={onRedo}
            title="Redo (Ctrl+Shift+Z)"
            aria-label="Redo"
          >
            <Redo2 size={16} />
          </button>
        </div>
        <span className="tb-div" />

        <div className="pop-wrap">
          <button
            type="button"
            className="chip-oracle"
            data-open={oracleOpen}
            onClick={() => setOracleOpen((v) => !v)}
            title="Oracle grounding"
          >
            <span
              className="dot"
              style={
                !grounded
                  ? { background: "var(--muted)", boxShadow: "none" }
                  : undefined
              }
            />
            {oracleLabel}
            <ChevronDown size={13} style={{ color: "var(--muted)" }} />
          </button>
          <OraclePopover
            open={oracleOpen}
            onClose={() => setOracleOpen(false)}
            workingFolderPath={workingFolderPath}
            tokens={tokens}
            invoke={invoke}
            tauri={tauri}
          />
        </div>

        <div className="pop-wrap">
          <button
            type="button"
            className="btn btn-ghost"
            data-open={exportOpen}
            onClick={() => setExportOpen((v) => !v)}
          >
            <Code2 size={15} />
            Export
          </button>
          <ExportPopover
            open={exportOpen}
            onClose={() => setExportOpen(false)}
            disabled={!projectOpen}
            runExport={runExport}
            exportTokens={exportTokens}
          />
        </div>

        <button
          type="button"
          className="btn btn-ghost"
          onClick={onPreview}
          disabled={!projectOpen || previewing}
          title="Preview the design in a read-only window (absolute layout)"
        >
          <Eye size={15} />
          Preview
        </button>

        <div className="pop-wrap split-primary">
          <button
            type="button"
            className="btn btn-primary split-main"
            onClick={onConsolidate}
            disabled={saving || !projectOpen}
            title="Write the design back to the working folder"
          >
            <Save size={15} />
            Save to repo
          </button>
          <button
            type="button"
            className="btn btn-primary split-caret"
            data-open={saveOpen}
            onClick={() => setSaveOpen((v) => !v)}
            title="More save options"
            aria-label="More save options"
          >
            <ChevronDown size={14} />
          </button>
          <SaveMenuPopover
            open={saveOpen}
            onClose={() => setSaveOpen(false)}
            disabled={saving || !projectOpen}
            onSave={onConsolidate}
          />
        </div>

        <span className="tb-div" />
        <button
          type="button"
          className="icon-btn-tb"
          onClick={onToggleFullscreen}
          title={fullscreen ? "Exit focus mode" : "Focus mode"}
          aria-label={fullscreen ? "Exit focus mode" : "Enter focus mode"}
        >
          {fullscreen ? <Minimize2 size={17} /> : <Maximize2 size={17} />}
        </button>
      </div>
    </header>
  );
}

export default TopBar;
