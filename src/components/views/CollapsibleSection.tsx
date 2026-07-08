import { useState, type ReactNode } from "react";
import { ChevronRight } from "lucide-react";

interface Props {
  title: string;
  badge?: string | number;
  /** Initial open state when not force-opened. */
  defaultOpen?: boolean;
  /** When true, force the section open regardless of the toggle (e.g. an active search). */
  forceOpen?: boolean;
  /** Fires when the section transitions from collapsed → expanded. */
  onExpand?: () => void;
  children: ReactNode;
}

/**
 * A lightweight collapsible section: a clickable header (rotating chevron + title + optional badge)
 * that shows/hides its children. It is NOT itself a bordered card — children keep their own cards —
 * so the page reads as a scannable list of section toggles. `forceOpen` overrides the toggle (used
 * to reveal matches while a search is active).
 */
export function CollapsibleSection({
  title,
  badge,
  defaultOpen = false,
  forceOpen = false,
  onExpand,
  children,
}: Props) {
  const [open, setOpen] = useState(defaultOpen);
  const expanded = forceOpen || open;
  return (
    <div>
      <button
        type="button"
        aria-expanded={expanded}
        onClick={() => {
          setOpen((o) => {
            const next = !o;
            if (next) onExpand?.();
            return next;
          });
        }}
        className="flex w-full items-center gap-2 rounded-2xl px-2 py-2 text-left transition-colors hover:bg-cream-50"
      >
        <ChevronRight
          className={`h-4 w-4 shrink-0 text-cream-400 transition-transform ${expanded ? "rotate-90" : ""}`}
        />
        <span className="text-[13px] font-semibold text-cream-800">{title}</span>
        {badge != null && (
          <span className="ml-auto rounded-full border border-cream-200 px-2 py-0.5 text-[11px] text-cream-500">
            {badge}
          </span>
        )}
      </button>
      {expanded && <div className="mt-1">{children}</div>}
    </div>
  );
}
