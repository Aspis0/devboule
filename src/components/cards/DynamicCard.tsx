import {
  Cloud,
  Server,
  Brain,
  Github,
  CloudCog,
  Globe,
  BookOpen,
  ExternalLink,
  type LucideIcon,
} from "lucide-react";
import { useState } from "react";
import { safeOpenExternal } from "../../utils/safeOpenExternal";

const iconMap: Record<string, LucideIcon> = {
  Cloud,
  Server,
  Brain,
  Github,
  CloudCog,
  Globe,
  BookOpen,
};

interface DynamicCardProps {
  title: string;
  description: string;
  icon: string;
  status?: "active" | "inactive" | "error";
  url: string;
  variant?: "provider" | "bookmark";
}

const statusStyles: Record<string, string> = {
  active: "bg-sage/10 text-sage-dark",
  inactive: "bg-cream-200/60 text-cream-500",
  error: "bg-coral/10 text-coral-dark",
};

export function DynamicCard({
  title,
  description,
  icon,
  status,
  url,
  variant = "provider",
}: DynamicCardProps) {
  const Icon = iconMap[icon] || Globe;
  const [externalError, setExternalError] = useState<string | null>(null);

  const openExternal = async () => {
    setExternalError(null);
    try {
      await safeOpenExternal(url);
    } catch (e) {
      setExternalError(e instanceof Error ? e.message : "External link failed.");
    }
  };

  return (
    <div className="space-y-2">
      <button
        type="button"
        onClick={() => void openExternal()}
        data-help-title={`${title} opens an external ${variant === "bookmark" ? "bookmark" : "provider"} link.`}
        data-help-lines="External links leave Devboule and open a browser or provider console.|For Aspis Bio, use them to verify provider state, docs, billing, permissions, or repo context directly at the source.|Opening a link does not change cloud resources by itself.|For repeatable provider operations, prefer a guarded action inside the app."
        className="group block w-full text-left bg-white rounded-3xl border border-cream-200 p-5
                   hover:shadow-soft-md hover:border-cream-300 hover:-translate-y-0.5
                   focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-terracotta/40 focus-visible:ring-offset-2
                   transition-all duration-300 cursor-pointer"
      >
        <div className="flex items-start justify-between mb-3">
          <div
            className="w-10 h-10 rounded-2xl bg-cream-50 flex items-center justify-center
                        group-hover:bg-terracotta-50 transition-colors duration-300"
          >
            <Icon className="w-5 h-5 text-cream-600 group-hover:text-terracotta transition-colors duration-300" />
          </div>
          <ExternalLink className="w-3.5 h-3.5 text-cream-300 opacity-0 group-hover:opacity-100 transition-opacity duration-300 mt-1" />
        </div>

        <h3 className="text-[14px] font-semibold text-cream-800 mb-0.5">
          {title}
        </h3>
        {description && (
          <p className="text-[12px] text-cream-500 mb-3 leading-relaxed">
            {description}
          </p>
        )}

        {status && (
          <span
            className={`inline-flex items-center gap-1.5 px-2.5 py-0.5 rounded-full text-[11px] font-medium ${statusStyles[status]}`}
          >
            <span
              className={`w-1.5 h-1.5 rounded-full ${
                status === "active"
                  ? "bg-sage"
                  : status === "error"
                    ? "bg-coral"
                    : "bg-cream-400"
              }`}
            />
            {status.charAt(0).toUpperCase() + status.slice(1)}
          </span>
        )}

        {variant === "bookmark" && (
          <span className="inline-flex items-center gap-1.5 px-2.5 py-0.5 rounded-full text-[11px] font-medium bg-teal/10 text-teal-dark">
            <span className="w-1.5 h-1.5 rounded-full bg-teal" />
            Bookmark
          </span>
        )}
      </button>
      {externalError && (
        <p className="rounded-xl border border-coral/20 bg-coral/[0.04] px-3 py-2 text-[11px] font-medium text-coral-dark">
          {externalError}
        </p>
      )}
    </div>
  );
}
