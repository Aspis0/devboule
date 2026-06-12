import { ChevronDown } from "lucide-react";
import { useState, type ReactNode } from "react";

// Simple accessible expand/collapse section used by the project detail panel.
// Presentational only: it owns its open/closed UI state and renders children
// lazily-visible (kept mounted, just hidden) so heavy panels do not remount.
export function CollapsibleSection({
  icon: Icon,
  title,
  summary,
  purpose,
  defaultOpen = false,
  helpTitle,
  helpLines,
  onToggle,
  children,
}: {
  icon: typeof ChevronDown;
  title: string;
  summary?: ReactNode;
  // Optional one-line explanation of what the section is FOR, rendered as a
  // small muted subtitle under the title so the purpose is self-evident at a
  // glance without expanding the section.
  purpose?: string;
  defaultOpen?: boolean;
  helpTitle?: string;
  helpLines?: string;
  onToggle?: (open: boolean) => void;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const toggle = () => {
    setOpen((value) => {
      const next = !value;
      onToggle?.(next);
      return next;
    });
  };
  return (
    <section
      className="overflow-hidden rounded-lg border border-cream-200 bg-white"
      data-help-title={helpTitle}
      data-help-lines={helpLines}
    >
      <button
        type="button"
        onClick={toggle}
        aria-expanded={open}
        className="flex w-full items-center justify-between gap-3 px-4 py-3 text-left transition-colors hover:bg-cream-50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-terracotta/40"
      >
        <span className="flex min-w-0 items-start gap-2">
          <Icon className="mt-0.5 h-4 w-4 shrink-0 text-cream-500" />
          <span className="min-w-0">
            <span className="flex min-w-0 flex-wrap items-center gap-x-2">
              <span className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
                {title}
              </span>
              {summary !== undefined && summary !== null && (
                <span className="min-w-0 truncate text-[11px] text-cream-400">
                  {summary}
                </span>
              )}
            </span>
            {purpose && (
              <span className="mt-0.5 block min-w-0 truncate text-[11px] text-cream-500">
                {purpose}
              </span>
            )}
          </span>
        </span>
        <ChevronDown
          className={`h-4 w-4 shrink-0 text-cream-400 transition-transform ${
            open ? "rotate-180" : ""
          }`}
        />
      </button>
      {open && <div className="border-t border-cream-200 p-4">{children}</div>}
    </section>
  );
}
