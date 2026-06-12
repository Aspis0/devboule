import { ChevronDown } from "lucide-react";
import {
  memo,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

export interface MiniMenuItem {
  key: string;
  label: ReactNode;
  onSelect: () => void;
  disabled?: boolean;
  title?: string;
  "aria-label"?: string;
  "data-help-title"?: string;
  "data-help-lines"?: string;
}

// Estimated menu width used for horizontal clamping before the real width is
// measured. Matches the `min-w-[7rem]` floor below (7rem = 112px) with a little
// slack for longer item labels.
const ESTIMATED_MENU_WIDTH = 160;
// Gap between the trigger and the menu, in px.
const MENU_GAP = 4;
// Margin kept from the viewport edges when clamping.
const VIEWPORT_MARGIN = 8;

type MenuPosition = {
  top: number;
  left: number;
  // When flipped above the trigger we anchor by the bottom edge so the menu
  // grows upward and stays glued to the button regardless of its height.
  placement: "below" | "above";
  bottom: number;
};

// Tiny accessible dropdown: a trigger button that toggles a small menu list.
// The open list is rendered in a PORTAL at document.body with FIXED positioning
// computed from the trigger's bounding rect, so it escapes every ancestor
// overflow (the Board's horizontal scroll) and stacking context (the
// collapsible sections below) and always paints on top. Closes on outside
// click or Escape, REPOSITIONS on scroll/resize (and only closes if the trigger
// scrolls fully out of the viewport), or closes after an item is selected.
// Selecting an item just invokes its onSelect — the menu owns no business logic,
// so the move/launch handlers stay identical to the old button rows. Reused for
// both "Move" and "Launch" on the task cards.
function MiniMenuInner({
  label,
  items,
  disabled = false,
  title,
  align = "left",
  triggerClassName,
  "aria-label": ariaLabel,
  "data-help-title": helpTitle,
  "data-help-lines": helpLines,
}: {
  label: ReactNode;
  items: MiniMenuItem[];
  disabled?: boolean;
  title?: string;
  align?: "left" | "right";
  triggerClassName?: string;
  "aria-label"?: string;
  "data-help-title"?: string;
  "data-help-lines"?: string;
}) {
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState<MenuPosition | null>(null);
  // The mounted portal node, tracked in STATE (not just a ref) so a layout
  // effect can fire once it exists and re-measure with the real menu height —
  // refs don't re-run effects, so without this the first-open flip/clamp would
  // be stuck on the zero height of an unpainted node. See computePosition.
  const [menuNode, setMenuNode] = useState<HTMLDivElement | null>(null);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const menuId = useId();

  // Compute the fixed position from the trigger's bounding rect. Prefers below
  // the button; flips above if it would overflow the viewport bottom; clamps
  // horizontally into the viewport.
  const computePosition = useCallback(() => {
    const trigger = triggerRef.current;
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const viewportWidth = window.innerWidth;
    const viewportHeight = window.innerHeight;
    const menuWidth = menuNode?.offsetWidth || ESTIMATED_MENU_WIDTH;
    const menuHeight = menuNode?.offsetHeight ?? 0;

    // Horizontal: align the menu's left edge with the trigger by default, or
    // its right edge with the trigger when align="right", then clamp.
    let left = align === "right" ? rect.right - menuWidth : rect.left;
    const maxLeft = viewportWidth - menuWidth - VIEWPORT_MARGIN;
    left = Math.max(VIEWPORT_MARGIN, Math.min(left, Math.max(VIEWPORT_MARGIN, maxLeft)));

    // Vertical: flip above when there is not enough room below but there is
    // above.
    const spaceBelow = viewportHeight - rect.bottom;
    const spaceAbove = rect.top;
    const wantedHeight = menuHeight || 0;
    const flipAbove =
      wantedHeight > 0 &&
      spaceBelow < wantedHeight + MENU_GAP + VIEWPORT_MARGIN &&
      spaceAbove > spaceBelow;

    setPosition({
      placement: flipAbove ? "above" : "below",
      top: rect.bottom + MENU_GAP,
      bottom: viewportHeight - rect.top + MENU_GAP,
      left,
    });
  }, [align, menuNode]);

  // Recompute on open and re-measure once the portal node mounts. The first
  // pass (menuNode === null) runs with the estimated width and a zero height so
  // the menu can paint; mounting the node updates `menuNode`, which feeds back
  // into `computePosition`, firing this effect a SECOND time with the real
  // measured width/height so the flip-above / horizontal-clamp decision uses the
  // actual menu box. `items.length` re-measures if the item set changes while
  // open. No null-position write on close is needed: the portal is unmounted by
  // the `open && createPortal(...)` guard, so the stale position is dropped with
  // the subtree and reset by the visibility-hidden-until-positioned path on the
  // next open.
  useLayoutEffect(() => {
    if (!open) return;
    computePosition();
  }, [open, computePosition, items.length]);

  useEffect(() => {
    if (!open) return;

    const handlePointer = (event: MouseEvent) => {
      const target = event.target as Node;
      // Treat clicks inside the trigger OR inside the portaled menu as "inside"
      // — the menu lives outside the trigger's DOM subtree, so we must check it
      // explicitly or the outside-click handler would close the menu on its own
      // items before their onClick fires.
      if (triggerRef.current?.contains(target)) return;
      if (menuNode?.contains(target)) return;
      setOpen(false);
    };
    const handleKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };

    // On scroll/resize, REPOSITION the menu to follow the trigger instead of
    // closing it — the old close-on-any-scroll made the menu nearly unusable on
    // a trackpad and on the horizontally-scrolling Kanban board. Only close when
    // the trigger has scrolled ENTIRELY out of the viewport (no longer anchorable
    // / no longer visible to the user). The reposition is throttled to one run
    // per animation frame to avoid layout thrash during momentum scrolling.
    let frame: number | null = null;
    const handleReposition = () => {
      if (frame !== null) return;
      frame = requestAnimationFrame(() => {
        frame = null;
        const trigger = triggerRef.current;
        if (!trigger) return;
        const rect = trigger.getBoundingClientRect();
        const fullyOffscreen =
          rect.bottom <= 0 ||
          rect.top >= window.innerHeight ||
          rect.right <= 0 ||
          rect.left >= window.innerWidth;
        if (fullyOffscreen) {
          setOpen(false);
          return;
        }
        computePosition();
      });
    };

    document.addEventListener("mousedown", handlePointer);
    document.addEventListener("keydown", handleKey);
    // Listen in capture so we catch scrolls on ANY ancestor scroll container
    // (the Board's horizontal scroller), not just the window.
    window.addEventListener("scroll", handleReposition, true);
    window.addEventListener("resize", handleReposition);
    return () => {
      document.removeEventListener("mousedown", handlePointer);
      document.removeEventListener("keydown", handleKey);
      window.removeEventListener("scroll", handleReposition, true);
      window.removeEventListener("resize", handleReposition);
      if (frame !== null) cancelAnimationFrame(frame);
    };
  }, [open, computePosition, menuNode]);

  return (
    <div className="relative">
      <button
        ref={triggerRef}
        type="button"
        onClick={() => setOpen((value) => !value)}
        disabled={disabled}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        aria-label={ariaLabel}
        title={title}
        data-help-title={helpTitle}
        data-help-lines={helpLines}
        className={
          triggerClassName ??
          "inline-flex w-full items-center justify-center gap-1 rounded-md border border-cream-200 px-2 py-1 text-[10px] font-semibold text-cream-500 transition hover:border-terracotta/30 hover:text-terracotta disabled:opacity-60"
        }
      >
        {label}
        <ChevronDown
          className={`h-3 w-3 shrink-0 transition-transform ${open ? "rotate-180" : ""}`}
          aria-hidden
        />
      </button>
      {open &&
        createPortal(
          <div
            ref={setMenuNode}
            id={menuId}
            role="menu"
            // Hidden until positioned to avoid a one-frame flash at (0,0).
            style={{
              position: "fixed",
              left: position ? `${position.left}px` : undefined,
              top:
                position && position.placement === "below"
                  ? `${position.top}px`
                  : undefined,
              bottom:
                position && position.placement === "above"
                  ? `${position.bottom}px`
                  : undefined,
              visibility: position ? "visible" : "hidden",
            }}
            className="z-[100] min-w-[7rem] overflow-hidden rounded-md border border-cream-200 bg-white py-1 shadow-soft"
          >
            {items.map((item) => (
              <button
                key={item.key}
                type="button"
                role="menuitem"
                onClick={() => {
                  if (item.disabled) return;
                  setOpen(false);
                  item.onSelect();
                }}
                disabled={item.disabled}
                title={item.title}
                aria-label={item["aria-label"]}
                data-help-title={item["data-help-title"]}
                data-help-lines={item["data-help-lines"]}
                className="block w-full px-3 py-1.5 text-left text-[11px] font-semibold text-cream-600 transition hover:bg-cream-50 hover:text-terracotta disabled:cursor-not-allowed disabled:opacity-50 disabled:hover:bg-transparent disabled:hover:text-cream-600"
              >
                {item.label}
              </button>
            ))}
          </div>,
          document.body,
        )}
    </div>
  );
}

// Memoized: TaskCard now passes stable (memoized) `items` arrays and the other
// props are primitives/stable, so the board's 5s/10s poll re-renders skip
// re-rendering an open menu and won't churn its positioning effects.
export const MiniMenu = memo(MiniMenuInner);
