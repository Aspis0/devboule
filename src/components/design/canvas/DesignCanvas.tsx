// Direct-DOM design canvas (replaces the Path-B srcDoc iframe). Renders every
// manifest node as an absolutely-positioned `.node-host` directly in the parent
// (app) DOM under the `.dsgn` wrapper. Pan/zoom live here; a non-passive wheel
// listener on the viewport pans (plain wheel) or cursor-anchored-zooms (ctrl/cmd
// wheel). Drag/resize mutate ONLY the dragged host's inline style via a ref during
// the gesture (no React state per move), then commit ONCE on pointer-up through
// the pure engine (snap + smartGuides + manifestOps), firing `onBeginChange`
// before applying so the parent can snapshot history.
//
// SECURITY: every node's inner markup is routed through `sanitizeNodeMarkup`
// before it reaches `dangerouslySetInnerHTML` (the single chokepoint). N3: that
// chokepoint now lives INSIDE `NodeContent` (memoized + useMemo on the raw markup)
// so it runs only when a node's markup changes, not on every canvas re-render — but
// it remains the LAST step before innerHTML. The canvas passes RAW markup. The app
// DOM has no `allow-scripts` sandbox, so the sanitizer IS the boundary here.

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type {
  DesignManifest,
  DesignNodePlacement,
  DesignProject,
} from "../../../types/design";
import {
  bringToFront,
  moveBackward,
  moveForward,
} from "../engine/manifestOps";
import { smartGuides, snapToGrid } from "../engine/snap";
import type { NodeRect } from "../../../types/design";
import {
  computeDragCommit,
  otherRects,
  previewPlacement,
  type DragMode,
} from "./dragCommit";
import {
  fitToBounds,
  nodesBounds,
  clampZoom,
  wheelZoom,
  zoomAtPoint,
  type Pan,
} from "./viewportMath";
import { NodeContent } from "./NodeContent";
import { LayersPanel } from "./LayersPanel";
import { ZoomPill } from "./ZoomPill";
import { ToolPill } from "./ToolPill";
import { ContentToolbar, type SwatchTarget } from "./ContentToolbar";
import { SpotEdit, type SpotEditHandle } from "./SpotEdit";
import { cleanSerialize, elHasText, startInlineTextEdit } from "./contentEdit";
import { colorTokens, type DtcgDocument } from "../engine/tokens";
import { sanitizeNodeMarkup } from "../sanitize";
import type { Point } from "../../../types/design";
import { Inspector } from "../Inspector";
import { Sparkles, Wand2 } from "lucide-react";

export interface DesignCanvasProps {
  project: DesignProject;
  /** Commit a placement-only manifest change (drag/resize/z-order/hidden/transform). */
  onManifestChange: (next: DesignManifest) => void;
  /**
   * Commit a STRUCTURAL project change (delete removes the component too; duplicate
   * adds one). Optional so the existing test mock (manifest-only) keeps working; the
   * canvas falls back to a manifest-only commit when it is absent.
   */
  onProjectChange?: (next: DesignProject) => void;
  /** Selection changed (id or null on empty-canvas click). */
  onSelect?: (id: string | null) => void;
  /** The currently selected node id (controlled by the parent). */
  selectedId?: string | null;
  /** Called BEFORE any committed mutation so the parent can push history. */
  onBeginChange?: () => void;
  /** Ids whose markup is currently being (re)generated — shows a shimmer/skeleton. */
  generatingIds?: Set<string>;
  /** Optional action for the empty-state button (wired in a later phase). */
  onEmptyAction?: () => void;
  /** The project's DTCG tokens — color leaves feed the CE toolbar swatches. */
  tokens?: DtcgDocument;
  /**
   * CONTENT-EDIT COMMIT. Fired when CE mode exits with a real change to a node's
   * inner markup: the canvas passes the RAW serialized markup (helper classes/attrs
   * stripped, but NOT sanitized). The parent MUST re-sanitize via `sanitizeNodeMarkup`
   * and run the node-persist path. Absent -> CE mode is disabled (double-click is a
   * no-op), so the existing manifest-only test mock keeps working unchanged.
   */
  onNodeMarkupCommit?: (nodeId: string, rawSerialized: string) => void;
  /**
   * SPOT EDIT. Fired when the user runs Analyze on a drawn region: the canvas passes
   * the polygon (WORLD coords) + the prompt. The parent picks hit nodes and runs the
   * edit chain. Absent -> the Spot Edit tool is hidden.
   */
  onRegionAnalyze?: (polygonWorldPts: Point[], prompt: string) => void;
  /** True while a Spot Edit analyze chain runs — keeps the region shimmer on. */
  spotBusy?: boolean;
  /**
   * Seed the assistant composer with an edit context (the "Ask AI" button in the CE
   * toolbar). Optional — when absent the button just exits CE.
   */
  onSeedComposer?: (ctx: { nodeId: string; tag: string }) => void;
}

/** The grid for snapping, from the project meta (px). */
function gridOf(project: DesignProject): number {
  return project.meta.canvas.grid;
}

/** A guide line in WORLD coordinates produced during a live drag. */
interface DragGuide {
  orientation: "vertical" | "horizontal";
  position: number;
  /** The span (other coordinate) so the line has a sensible length. */
  from: number;
  to: number;
}

export function DesignCanvas({
  project,
  onManifestChange,
  onProjectChange,
  onSelect,
  selectedId = null,
  onBeginChange,
  generatingIds,
  onEmptyAction,
  tokens,
  onNodeMarkupCommit,
  onRegionAnalyze,
  spotBusy = false,
  onSeedComposer,
}: DesignCanvasProps) {
  const [zoom, setZoom] = useState(0.85);
  const [pan, setPan] = useState<Pan>({ x: 40, y: 24 });
  const [guides, setGuides] = useState<DragGuide[]>([]);

  // Tool: "move" (select/drag) or "ai" (Spot Edit region). Spot Edit is only
  // available when the parent wired `onRegionAnalyze`.
  const [tool, setTool] = useState<"move" | "ai">("move");
  const ceEnabled = !!onNodeMarkupCommit;
  const spotEnabled = !!onRegionAnalyze;

  // --- content-edit (CE) mode state -----------------------------------------
  // `ceNodeId`: the node currently in CE (dashed ring, drag disabled). `ceEl`: the
  // selected inner element. `ceVer`: bumped to reposition the toolbar (select / pan /
  // zoom / reorder / recolor / inline-edit end).
  const [ceNodeId, setCeNodeId] = useState<string | null>(null);
  const [ceEl, setCeEl] = useState<HTMLElement | null>(null);
  const [ceVer, setCeVer] = useState(0);
  // The live CE content root (`.node-content`) so commit can serialize it.
  const ceContentRef = useRef<HTMLDivElement | null>(null);
  // Live mirror of the CE element so the keydown listener (Delete) reads it without
  // re-subscribing on every selection.
  const ceElRef = useRef<HTMLElement | null>(null);
  ceElRef.current = ceEl;
  const ceNodeIdRef = useRef<string | null>(null);
  ceNodeIdRef.current = ceNodeId;
  // Teardown handle for an in-flight inline text edit (so we can force-end it on exit).
  const inlineEditCancelRef = useRef<(() => void) | null>(null);
  // The stored markup of the CE node AT THE MOMENT CE was entered. If the stored markup
  // changes underneath CE (a generation/edit/undo replaced the node's content WITHOUT
  // removing the node), the external update wins and CE exits SILENTLY — committing the
  // now-stale serialized DOM would clobber the fresh content. null when not in CE.
  const ceEntryMarkupRef = useRef<string | null>(null);

  const wrapRef = useRef<HTMLDivElement>(null);
  const viewportRef = useRef<HTMLDivElement>(null);
  const worldRef = useRef<HTMLDivElement>(null);
  const spotRef = useRef<SpotEditHandle>(null);

  // Live refs so the long-lived wheel/pointer/keyboard listeners never capture a
  // stale closure (zoom/pan/project/selection change between renders).
  const zoomRef = useRef(zoom);
  zoomRef.current = zoom;
  const panRef = useRef(pan);
  panRef.current = pan;
  const projectRef = useRef(project);
  projectRef.current = project;
  const selectedRef = useRef(selectedId);
  selectedRef.current = selectedId;

  // Measured rendered heights per node id (for snap math + auto-height nodes).
  const measuredH = useRef<Record<string, number>>({});

  // Active drag/resize state (ref, NOT state — a gesture must not re-render).
  const dragRef = useRef<{
    id: string;
    mode: DragMode;
    startClientX: number;
    startClientY: number;
    origX: number;
    origY: number;
    origW: number;
    moved: boolean;
    host: HTMLElement;
  } | null>(null);

  // --- commit helpers --------------------------------------------------------

  const commitManifest = useCallback(
    (next: DesignManifest) => {
      if (next === projectRef.current.manifest) return; // no-op (reference)
      onBeginChange?.();
      onManifestChange(next);
    },
    [onManifestChange, onBeginChange],
  );

  // A structural commit (changes components / nodeOrder). Falls back to a
  // manifest-only commit when the parent didn't supply onProjectChange.
  const commitProject = useCallback(
    (next: DesignProject) => {
      onBeginChange?.();
      if (onProjectChange) onProjectChange(next);
      else onManifestChange(next.manifest);
    },
    [onProjectChange, onManifestChange, onBeginChange],
  );

  const selectNode = useCallback(
    (id: string | null) => {
      onSelect?.(id);
    },
    [onSelect],
  );

  // --- content-edit (CE) mode -----------------------------------------------

  // Exit CE mode. Force-ends any in-flight inline edit, serializes the content root,
  // and — when the markup actually changed — hands the RAW serialized markup UP for
  // re-sanitization + persistence. Always clears CE state. `silent` skips the commit
  // (used when the node vanished out from under CE — nothing valid to persist).
  const exitCe = useCallback(
    (silent = false) => {
      // End any open inline text edit first so its contenteditable attrs are gone
      // before we serialize (cleanSerialize also strips them, but this fires onDone).
      if (inlineEditCancelRef.current) {
        inlineEditCancelRef.current();
        inlineEditCancelRef.current = null;
      }
      const nodeId = ceNodeIdRef.current;
      const root = ceContentRef.current;
      if (!silent && nodeId && root && onNodeMarkupCommit) {
        // Only commit when this node still exists AND the serialized markup differs
        // from what is on file (avoid a churn commit + history push for a pure click).
        const live = projectRef.current.manifest.nodes[nodeId];
        if (live) {
          const raw = cleanSerialize(root);
          const current = projectRef.current.components[nodeId] ?? "";
          if (raw !== current) onNodeMarkupCommit(nodeId, raw);
        }
      }
      if (ceElRef.current) ceElRef.current.classList.remove("ce-sel");
      // Sweep any lingering `ce-hover` from descendants: a pointer can leave the content
      // root WITHOUT firing mouseout (e.g. CE exits while hovered, or the toolbar steals
      // focus), leaving a stale hover ring on the next CE entry / the persisted DOM.
      if (root) {
        root
          .querySelectorAll<HTMLElement>(".ce-hover")
          .forEach((el) => el.classList.remove("ce-hover"));
      }
      ceContentRef.current = null;
      ceEntryMarkupRef.current = null;
      setCeEl(null);
      setCeNodeId(null);
    },
    [onNodeMarkupCommit],
  );

  // Select an inner element within the CE content root (the prototype's ceSelect).
  const ceSelect = useCallback((target: HTMLElement, container: HTMLElement) => {
    if (target === container) return; // clicking the bare root selects nothing
    if (ceElRef.current && ceElRef.current !== target) {
      ceElRef.current.classList.remove("ce-sel");
    }
    target.classList.add("ce-sel");
    setCeEl(target);
    setCeVer((v) => v + 1);
  }, []);

  // Begin inline text editing on a CE element (double-click text / toolbar button).
  const ceStartText = useCallback((el: HTMLElement) => {
    if (!elHasText(el)) return;
    // Cancel any prior inline edit before starting a new one.
    if (inlineEditCancelRef.current) inlineEditCancelRef.current();
    inlineEditCancelRef.current = startInlineTextEdit(el, () => {
      inlineEditCancelRef.current = null;
      setCeVer((v) => v + 1);
    });
  }, []);

  // Enter CE mode for a node (double-click a node in Move tool).
  const enterCe = useCallback(
    (id: string) => {
      if (!ceEnabled) return;
      selectNode(id);
      // Snapshot the stored markup so the abort guard can detect an external content
      // change (generation/undo) landing on THIS node while it's being edited.
      ceEntryMarkupRef.current = projectRef.current.components[id] ?? "";
      setCeNodeId(id);
      setCeEl(null);
      setCeVer((v) => v + 1);
    },
    [ceEnabled, selectNode],
  );

  // Color swatch applied to the selected CE element (text color or background fill).
  // This mutates the LIVE DOM directly; the change is serialized + committed on exit.
  const onCeColor = useCallback((value: string, target: SwatchTarget) => {
    const el = ceElRef.current;
    if (!el) return;
    if (target === "fill") el.style.background = value;
    else el.style.color = value;
    setCeVer((v) => v + 1);
  }, []);

  // Reorder the selected CE element among its siblings (live DOM move).
  const onCeMove = useCallback((dir: "up" | "down") => {
    const el = ceElRef.current;
    if (!el) return;
    const parent = el.parentElement;
    if (!parent) return;
    if (dir === "up" && el.previousElementSibling) {
      parent.insertBefore(el, el.previousElementSibling);
    } else if (dir === "down" && el.nextElementSibling) {
      parent.insertBefore(el.nextElementSibling, el);
    }
    setCeVer((v) => v + 1);
  }, []);

  // Remove the selected CE element (toolbar trash / Delete key).
  const onCeRemove = useCallback(() => {
    const el = ceElRef.current;
    if (!el) return;
    el.remove();
    setCeEl(null);
    setCeVer((v) => v + 1);
  }, []);

  // "Ask AI": seed the composer with this element's context, then exit CE (committing).
  const onCeAskAi = useCallback(() => {
    const el = ceElRef.current;
    const nodeId = ceNodeIdRef.current;
    const tag = el ? el.tagName.toLowerCase() : "";
    exitCe();
    if (nodeId) onSeedComposer?.({ nodeId, tag });
  }, [exitCe, onSeedComposer]);

  // --- structural ops (delete / duplicate / hidden) --------------------------

  const deleteNode = useCallback(
    (id: string) => {
      const p = projectRef.current;
      if (!p.manifest.nodes[id]) return;
      const nodes = { ...p.manifest.nodes };
      delete nodes[id];
      const components = { ...p.components };
      delete components[id];
      const next: DesignProject = {
        ...p,
        manifest: { ...p.manifest, nodes },
        components,
        meta: {
          ...p.meta,
          nodeOrder: p.meta.nodeOrder.filter((n) => n !== id),
        },
      };
      commitProject(next);
      if (selectedRef.current === id) selectNode(null);
    },
    [commitProject, selectNode],
  );

  const toggleHidden = useCallback(
    (id: string) => {
      const p = projectRef.current;
      const node = p.manifest.nodes[id];
      if (!node) return;
      const next: DesignManifest = {
        ...p.manifest,
        nodes: { ...p.manifest.nodes, [id]: { ...node, hidden: !node.hidden } },
      };
      commitManifest(next);
    },
    [commitManifest],
  );

  // --- viewport: wheel pan / cursor-anchored zoom (non-passive) --------------
  useEffect(() => {
    const vp = viewportRef.current;
    if (!vp) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      if (e.ctrlKey || e.metaKey) {
        const r = vp.getBoundingClientRect();
        const cx = e.clientX - r.left;
        const cy = e.clientY - r.top;
        const { zoom: nz, pan: np } = wheelZoom(
          zoomRef.current,
          panRef.current,
          e.deltaY,
          cx,
          cy,
        );
        setZoom(nz);
        setPan(np);
      } else {
        setPan((prev) => ({ x: prev.x - e.deltaX, y: prev.y - e.deltaY }));
      }
    };
    vp.addEventListener("wheel", onWheel, { passive: false });
    return () => vp.removeEventListener("wheel", onWheel);
  }, []);

  // --- measure rendered heights after every render ---------------------------
  // V3: scope the measurement to the TOP-LEVEL `.node-host` divs only (the DIRECT
  // children of `world` that carry `data-node-id`). A plain descendant
  // `querySelectorAll("[data-node-id]")` is RECURSIVE: untrusted node markup that embeds a
  // `data-node-id` of a SIBLING (or any id) would be matched too and clobber that id's real
  // measuredH with the inner element's height. The `:scope > [data-node-id]` selector
  // matches only the host divs (rendered directly under `world` with the attribute), so a
  // nested attribute inside a node's content can never corrupt another node's measurement.
  useEffect(() => {
    const world = worldRef.current;
    if (!world) return;
    world
      .querySelectorAll<HTMLElement>(":scope > [data-node-id]")
      .forEach((el) => {
        const id = el.getAttribute("data-node-id");
        if (id) measuredH.current[id] = el.offsetHeight;
      });
  });

  // --- keyboard: delete / z-order / esc --------------------------------------
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const tag = (target?.tagName ?? "").toLowerCase();
      if (
        tag === "input" ||
        tag === "textarea" ||
        target?.isContentEditable
      ) {
        return;
      }
      // CE mode owns the keyboard: Esc commits+exits, Delete removes the selected
      // inner element (prototype parity), and z-order/node-delete are suppressed.
      if (ceNodeIdRef.current) {
        if (e.key === "Escape") {
          exitCe();
        } else if (e.key === "Delete" || e.key === "Backspace") {
          if (ceElRef.current) onCeRemove();
        }
        return;
      }
      const sel = selectedRef.current;
      if (e.key === "Escape") {
        // Spot Edit: Esc cancels the region / leaves the tool before clearing selection.
        if (spotRef.current?.hasRegion() || tool === "ai") {
          spotRef.current?.cancel();
          setTool("move");
          return;
        }
        selectNode(null);
        return;
      }
      if (!sel) return;
      if (e.key === "[" || e.key === "]") {
        const m = projectRef.current.manifest;
        const next = e.key === "]" ? moveForward(m, sel) : moveBackward(m, sel);
        commitManifest(next);
      } else if (e.key === "Delete" || e.key === "Backspace") {
        deleteNode(sel);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selectNode, commitManifest, deleteNode, exitCe, onCeRemove, tool]);

  // CE ABORT: if the node in CE mode vanishes from the manifest (a generation, an
  // undo/redo, or a self-repair replaced it), exit CE SILENTLY — there is nothing
  // valid left to serialize/commit, and the stale `.node-content` ref would point at
  // a detached DOM tree. (A normal exit would try to commit against a missing node.)
  //
  // ALSO abort silently if the node still exists but its STORED markup changed out from
  // under CE (a generation/edit/undo landed on THIS same node): the external content is
  // already re-rendered into the live `.node-content`, so the user's in-flight DOM edits
  // are gone — committing the serialized DOM now would either re-clobber the fresh
  // content with stale edits or persist a mix. The external update wins; CE just exits.
  useEffect(() => {
    if (!ceNodeId) return;
    if (!project.manifest.nodes[ceNodeId]) {
      exitCe(true);
      return;
    }
    const entry = ceEntryMarkupRef.current;
    if (entry !== null && (project.components[ceNodeId] ?? "") !== entry) {
      exitCe(true);
    }
  }, [project.manifest.nodes, project.components, ceNodeId, exitCe]);

  // --- drag / resize ---------------------------------------------------------

  const onPointerMove = useCallback((e: PointerEvent) => {
    const d = dragRef.current;
    if (!d) return;
    // The node can vanish mid-drag if a generation/self-repair commit lands under
    // the pointer (it replaces the manifest). Abort the live preview rather than
    // dereference an absent node; pointer-up already re-checks before committing.
    const liveNode = projectRef.current.manifest.nodes[d.id];
    if (!liveNode) return;
    const z = zoomRef.current;
    const dx = (e.clientX - d.startClientX) / z;
    const dy = (e.clientY - d.startClientY) / z;
    if (!d.moved && Math.abs(dx) + Math.abs(dy) > 1) d.moved = true;

    const base: DesignNodePlacement = {
      ...liveNode,
      x: d.origX,
      y: d.origY,
      w: d.origW,
    };
    const preview = previewPlacement(base, d.mode, dx, dy);

    if (d.mode === "move") {
      // Compute snap + smart guides live so the user sees alignment lines, and
      // apply the snapped position to the DOM (commit re-derives it identically).
      const grid = gridOf(projectRef.current);
      const movingRect: NodeRect = {
        id: d.id,
        x: snapToGrid(preview.x, grid),
        y: snapToGrid(preview.y, grid),
        w: d.origW,
        h: measuredH.current[d.id] ?? 0,
        z: base.z,
      };
      const others = otherRects(
        projectRef.current.manifest,
        d.id,
        measuredH.current,
      );
      const g = smartGuides(movingRect, others);
      const finalX = movingRect.x + g.dx;
      const finalY = movingRect.y + g.dy;
      d.host.style.left = `${finalX}px`;
      d.host.style.top = `${finalY}px`;
      setGuides(
        g.guides.map((gl) =>
          gl.orientation === "vertical"
            ? {
                orientation: "vertical",
                position: gl.position,
                from: Math.min(finalY, movingRect.y) - 24,
                to: Math.max(finalY + movingRect.h, movingRect.y) + 24,
              }
            : {
                orientation: "horizontal",
                position: gl.position,
                from: Math.min(finalX, movingRect.x) - 24,
                to: Math.max(finalX + movingRect.w, movingRect.x) + 24,
              },
        ),
      );
    } else {
      d.host.style.width = `${Math.max(1, preview.w)}px`;
      if (typeof preview.h === "number") d.host.style.height = `${preview.h}px`;
    }
  }, []);

  const onPointerUp = useCallback(
    (e: PointerEvent) => {
      const d = dragRef.current;
      if (!d) return;
      dragRef.current = null;
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", onPointerUp);
      window.removeEventListener("pointercancel", onPointerUp);
      try {
        d.host.releasePointerCapture?.(e.pointerId);
      } catch {
        // capture may already be gone — ignore.
      }
      setGuides([]);
      if (!d.moved) return; // a click, not a drag — no commit

      const z = zoomRef.current;
      const dx = (e.clientX - d.startClientX) / z;
      const dy = (e.clientY - d.startClientY) / z;
      const live = projectRef.current.manifest;
      const node = live.nodes[d.id];
      if (!node) return; // node vanished mid-drag (generation/self-repair) — abort
      const others = otherRects(live, d.id, measuredH.current);
      let next = computeDragCommit({
        manifest: live,
        id: d.id,
        mode: d.mode,
        dx,
        dy,
        grid: gridOf(projectRef.current),
        others,
      });
      // Bring-to-front on a completed drag (prototype parity) — fold into one commit.
      next = bringToFront(next, d.id);
      commitManifest(next);
    },
    [onPointerMove, commitManifest],
  );

  const beginDrag = useCallback(
    (id: string, mode: DragMode, e: React.PointerEvent) => {
      const node = projectRef.current.manifest.nodes[id];
      if (!node) return;
      const host = (e.currentTarget as HTMLElement).closest<HTMLElement>(
        "[data-node-id]",
      );
      if (!host) return;
      e.preventDefault();
      e.stopPropagation();
      selectNode(id);
      dragRef.current = {
        id,
        mode,
        startClientX: e.clientX,
        startClientY: e.clientY,
        origX: node.x,
        origY: node.y,
        origW: node.w,
        moved: false,
        host,
      };
      try {
        host.setPointerCapture?.(e.pointerId);
      } catch {
        // non-fatal: fall back to window listeners below.
      }
      window.addEventListener("pointermove", onPointerMove);
      window.addEventListener("pointerup", onPointerUp);
      window.addEventListener("pointercancel", onPointerUp);
    },
    [selectNode, onPointerMove, onPointerUp],
  );

  // Teardown: drop any in-flight gesture listeners on unmount.
  useEffect(() => {
    return () => {
      if (dragRef.current) {
        dragRef.current = null;
        window.removeEventListener("pointermove", onPointerMove);
        window.removeEventListener("pointerup", onPointerUp);
        window.removeEventListener("pointercancel", onPointerUp);
      }
    };
  }, [onPointerMove, onPointerUp]);

  // --- zoom pill actions -----------------------------------------------------

  // Set an exact zoom level, anchored at the viewport center (so − / + / reset all
  // keep the middle of the canvas stable rather than the world origin).
  const setZoomAnchoredCenter = useCallback((nz: number) => {
    const z = zoomRef.current;
    const vp = viewportRef.current;
    if (vp) {
      const r = vp.getBoundingClientRect();
      const cx = r.width / 2;
      const cy = r.height / 2;
      setPan(zoomAtPoint(z, panRef.current, nz, cx, cy));
    }
    setZoom(nz);
  }, []);

  const zoomBy = useCallback(
    (delta: number) => setZoomAnchoredCenter(clampZoom(zoomRef.current + delta)),
    [setZoomAnchoredCenter],
  );

  const resetZoom = useCallback(
    () => setZoomAnchoredCenter(1),
    [setZoomAnchoredCenter],
  );

  const fit = useCallback(() => {
    const vp = viewportRef.current;
    if (!vp) return;
    const r = vp.getBoundingClientRect();
    const rects: NodeRect[] = Object.entries(projectRef.current.manifest.nodes)
      .filter(([, p]) => !p.hidden)
      .map(([id, p]) => ({
        id,
        x: p.x,
        y: p.y,
        w: p.w,
        h: typeof p.h === "number" ? p.h : measuredH.current[id] ?? 200,
        z: p.z,
      }));
    const { zoom: nz, pan: np } = fitToBounds(nodesBounds(rects), r.width, r.height);
    setZoom(nz);
    setPan(np);
  }, []);

  // --- render ----------------------------------------------------------------

  // Visible nodes sorted by z asc (paint order); hidden ones are skipped.
  const visibleNodes = useMemo(() => {
    return Object.entries(project.manifest.nodes)
      .filter(([, p]) => !p.hidden)
      .sort((a, b) =>
        a[1].z !== b[1].z ? a[1].z - b[1].z : a[0].localeCompare(b[0]),
      );
  }, [project.manifest.nodes]);

  const totalNodeCount = Object.keys(project.manifest.nodes).length;
  const dotSize = 22 * zoom;

  // Color-token swatches for the CE toolbar (recomputed only when tokens change).
  const ceSwatches = useMemo(() => colorTokens(tokens ?? {}), [tokens]);

  // Tool switches. Move clears any in-progress region; Spot Edit drops node selection
  // and commits/exits any CE session (prototype parity).
  const onSelectMove = useCallback(() => {
    setTool("move");
    spotRef.current?.cancel();
  }, []);
  const onSelectAi = useCallback(() => {
    setTool("ai");
    selectNode(null);
    if (ceNodeIdRef.current) exitCe();
  }, [selectNode, exitCe]);

  // Run analysis: hand the polygon (world coords) + prompt UP, then leave the tool
  // (the parent starts the sequential edit chain). The region is cleared here so the
  // chain runs without a lingering overlay (prototype keeps it shimmering via busy;
  // we clear immediately because the canvas has no per-run completion signal — the
  // parent owns `spotBusy` over its OWN nodes' generating shimmer instead).
  const onSpotAnalyze = useCallback(
    (polygonWorldPts: Point[], prompt: string) => {
      onRegionAnalyze?.(polygonWorldPts, prompt);
      spotRef.current?.cancel();
      setTool("move");
    },
    [onRegionAnalyze],
  );

  return (
    <div className="canvas-wrap" ref={wrapRef}>
      <div
        ref={viewportRef}
        className="canvas-viewport"
        data-tool={tool}
        style={{
          backgroundImage:
            "radial-gradient(circle, #DDD1BE 1px, transparent 1px)",
          backgroundSize: `${dotSize}px ${dotSize}px`,
          backgroundPosition: `${pan.x}px ${pan.y}px`,
        }}
        onPointerDown={(e) => {
          // Spot Edit: an empty-viewport press starts drawing a region (over nodes too).
          if (tool === "ai") {
            spotRef.current?.startDraw(e.clientX, e.clientY);
            return;
          }
          // Click outside any node commits + exits CE before clearing selection.
          if (ceNodeIdRef.current) exitCe();
          selectNode(null);
        }}
      >
        <div
          ref={worldRef}
          className="canvas-world"
          style={{
            transform: `translate(${pan.x}px, ${pan.y}px) scale(${zoom})`,
          }}
        >
          {visibleNodes.map(([id, node]) => {
            const radius = node.radius == null ? 14 : node.radius;
            const generating = generatingIds?.has(id) ?? false;
            // N3: pass the RAW markup. NodeContent (React.memo'd on this string)
            // runs the sanitizeNodeMarkup chokepoint inside a useMemo keyed on it,
            // so sanitization happens only when a node's markup actually changes —
            // not on every pan/zoom/sibling-drag re-render of the canvas.
            const rawMarkup = project.components[id] ?? "";
            const heightStyle =
              typeof node.h === "number" ? { height: node.h } : {};
            const inCe = id === ceNodeId;
            return (
              <div
                key={id}
                data-node-id={id}
                className={
                  "node-host" +
                  (id === selectedId && tool !== "ai" ? " sel" : "") +
                  (inCe ? " content" : "")
                }
                style={{
                  left: node.x,
                  top: node.y,
                  width: node.w,
                  zIndex: node.z,
                  cursor: inCe ? "default" : "grab",
                  ...heightStyle,
                }}
                onPointerDown={(e) => {
                  // Spot Edit: let the press bubble to the viewport so the region can
                  // be drawn OVER nodes (prototype parity).
                  if (tool === "ai") return;
                  // In CE: swallow the press so it neither starts a node drag nor
                  // bubbles to the viewport's commit-and-exit handler.
                  if (inCe) {
                    e.stopPropagation();
                    return;
                  }
                  // A press on a DIFFERENT node while in CE commits + exits first.
                  if (ceNodeIdRef.current) exitCe();
                  beginDrag(id, "move", e);
                }}
                onDoubleClick={(e) => {
                  if (tool === "ai") return;
                  if (!inCe && ceEnabled) {
                    e.stopPropagation();
                    enterCe(id);
                  }
                }}
              >
                <div className="node-tag">
                  {inCe
                    ? `${node.name ?? id} · editing content — Esc to finish`
                    : node.name ?? id}
                  {!inCe && <span className="kind">{node.kind.toUpperCase()}</span>}
                </div>
                <div
                  className="node-card"
                  style={{
                    borderRadius: radius,
                    boxShadow: node.flat ? "none" : undefined,
                  }}
                >
                  {inCe ? (
                    // CE: render sanitized markup into a content root we OWN (ref'd for
                    // serialize-on-commit) and attach the element select/hover/inline
                    // handlers. We deliberately re-run sanitize here (NodeContent's
                    // chokepoint) — CE only edits the LIVE DOM, never the stored string.
                    <div
                      className="node-content"
                      ref={(el) => {
                        ceContentRef.current = el;
                      }}
                      dangerouslySetInnerHTML={{
                        __html: sanitizeNodeMarkup(rawMarkup),
                      }}
                      onMouseOver={(e) => {
                        const t = e.target as HTMLElement;
                        if (
                          t !== e.currentTarget &&
                          !t.isContentEditable
                        ) {
                          t.classList.add("ce-hover");
                        }
                      }}
                      onMouseOut={(e) => {
                        (e.target as HTMLElement).classList.remove("ce-hover");
                      }}
                      onClick={(e) => {
                        e.stopPropagation();
                        ceSelect(
                          e.target as HTMLElement,
                          e.currentTarget as HTMLElement,
                        );
                      }}
                      onDoubleClick={(e) => {
                        e.stopPropagation();
                        const t = e.target as HTMLElement;
                        ceSelect(t, e.currentTarget as HTMLElement);
                        ceStartText(t);
                      }}
                    />
                  ) : (
                    <NodeContent markup={rawMarkup} generating={generating} />
                  )}
                </div>
                {generating && (
                  <div
                    className="node-shimmer"
                    style={{ borderRadius: radius }}
                  />
                )}
                <div className="node-ring" style={{ borderRadius: radius + 3 }} />
                <div
                  className="node-handle"
                  onPointerDown={(e) => {
                    if (tool === "ai" || inCe) return;
                    beginDrag(id, "resize", e);
                  }}
                />
              </div>
            );
          })}

          {guides.map((g, i) =>
            g.orientation === "vertical" ? (
              <div
                key={i}
                className="guide v"
                style={{ left: g.position, top: g.from, height: g.to - g.from }}
              />
            ) : (
              <div
                key={i}
                className="guide h"
                style={{ top: g.position, left: g.from, width: g.to - g.from }}
              />
            ),
          )}
        </div>
      </div>

      {totalNodeCount === 0 && (
        <div className="canvas-empty">
          <div className="ce-card">
            <span className="ce-ic">
              <Sparkles size={22} />
            </span>
            <b>No sections yet</b>
            <p>
              Describe a section in the assistant — it's generated grounded on
              your codebase, with your real components, tokens and copy.
            </p>
            <button
              type="button"
              className="btn btn-primary"
              onClick={onEmptyAction}
            >
              <Wand2 size={15} />
              Generate a section
            </button>
          </div>
        </div>
      )}

      <ToolPill
        tool={tool}
        onSelectMove={onSelectMove}
        onSelectAi={spotEnabled ? onSelectAi : onSelectMove}
      />

      {/* Spot Edit overlay — present only when the tool is active or a region exists.
          It owns its region state; the canvas calls startDraw via spotRef. */}
      {spotEnabled && (
        <SpotEdit
          ref={spotRef}
          pan={pan}
          zoom={zoom}
          viewportRef={viewportRef}
          worldRef={worldRef}
          wrapRef={wrapRef}
          busy={spotBusy}
          onAnalyze={onSpotAnalyze}
        />
      )}

      {/* Content-edit toolbar — floats over the selected inner element in CE mode. */}
      {ceEl && ceNodeId && (
        <ContentToolbar
          el={ceEl}
          wrapEl={wrapRef.current}
          version={ceVer}
          swatches={ceSwatches}
          onEditText={() => ceStartText(ceEl)}
          onColor={onCeColor}
          onMove={onCeMove}
          onRemove={onCeRemove}
          onAskAi={onCeAskAi}
        />
      )}

      <Inspector
        project={project}
        selectedId={tool === "ai" ? null : selectedId}
        onManifestChange={commitManifest}
        onProjectChange={commitProject}
        onSelect={selectNode}
        measuredHeight={selectedId ? measuredH.current[selectedId] : undefined}
      />

      <LayersPanel
        manifest={project.manifest}
        selectedId={selectedId}
        onSelect={selectNode}
        onToggleHidden={toggleHidden}
        onDelete={deleteNode}
      />

      <ZoomPill
        zoom={zoom}
        onZoomBy={zoomBy}
        onReset={resetZoom}
        onFit={fit}
      />

      <div className="canvas-hint">
        <span>
          <b>Drag</b> to move · corner to resize · <b>[ ]</b> to reorder
        </span>
        <span>
          <b>Scroll</b> to pan · <b>⌘ scroll</b> to zoom · <b>Esc</b> deselect
        </span>
      </div>
    </div>
  );
}

export default DesignCanvas;
