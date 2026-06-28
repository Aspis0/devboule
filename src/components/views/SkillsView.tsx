import { useEffect, useState } from "react";
import { invokeBackendCommand } from "../../context/AppContext";
import type { FeaturedMarketplace } from "../../types/skills";
import { GlobalLibraryPanel } from "./GlobalLibraryPanel";
import { MarketplaceInstall } from "./MarketplaceInstall";
import { UserMcpServersCard } from "../settings/UserMcpServersCard";

// The sidebar "Skills" view is GLOBAL-only: a personal skill Library + your MCP Tools, shared
// across every project. Per-project, per-role skills (and language personas) live in the Work
// Console's "Skills & Tools" modal — there is intentionally no project folder picker here.
export function SkillsView() {
  const [view, setView] = useState<"library" | "tools">("library");
  // Bumped after a URL install so the global library list refetches.
  const [reloadToken, setReloadToken] = useState(0);
  const [featured, setFeatured] = useState<FeaturedMarketplace[]>([]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const r = await invokeBackendCommand("skills_featured_marketplaces");
        if (!cancelled && Array.isArray(r)) {
          setFeatured(r as FeaturedMarketplace[]);
        }
      } catch {
        // best-effort: the discovery list simply doesn't render.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div data-testid="skills-view" className="flex flex-col gap-4 p-4">
      <div className="rounded-2xl border border-cream-200 bg-cream-50 px-4 py-3 text-[12px] text-cream-600">
        Your global skill <strong>Library</strong> and MCP <strong>Tools</strong>, shared across
        every project. Per-project, per-role skills (and language personas) are managed in the Work
        Console&apos;s Skills &amp; Tools panel. <em>Skills are manuals; Tools are MCP machines.</em>
      </div>

      <div role="tablist" aria-label="Skills view" className="flex gap-2">
        {(["library", "tools"] as const).map((v) => (
          <button
            key={v}
            type="button"
            role="tab"
            id={`skills-view-tab-${v}`}
            data-testid={`skills-view-tab-${v}`}
            aria-selected={view === v}
            aria-controls={`skills-view-panel-${v}`}
            onClick={() => setView(v)}
            className={`rounded-2xl px-3 py-1.5 text-[12px] font-semibold transition-colors ${
              view === v
                ? "bg-teal/10 text-teal"
                : "text-cream-500 hover:text-cream-800"
            }`}
          >
            {v.charAt(0).toUpperCase() + v.slice(1)}
          </button>
        ))}
      </div>

      {view === "library" && (
        <div
          role="tabpanel"
          id="skills-view-panel-library"
          aria-labelledby="skills-view-tab-library"
        >
          <GlobalLibraryPanel reloadToken={reloadToken} />
          <div className="mb-2 mt-5 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
            Install from a URL
          </div>
          <MarketplaceInstall
            scope="global"
            invoke={invokeBackendCommand}
            onInstalled={() => setReloadToken((t) => t + 1)}
          />
          {featured.length > 0 && (
            <>
              <div className="mb-2 mt-5 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
                Featured marketplaces
              </div>
              <div className="flex flex-col gap-2">
                {featured.map((f) => (
                  <a
                    key={f.url}
                    href={f.url}
                    target="_blank"
                    rel="noreferrer"
                    data-testid={`featured-${f.name}`}
                    className="block rounded-lg border border-cream-200 bg-white p-2 text-[12px] hover:border-cream-300"
                  >
                    <div className="font-medium text-cream-800">
                      {f.name}{" "}
                      <span className="text-[10px] text-cream-500">{f.license}</span>
                    </div>
                    <div className="text-[11px] text-cream-500">{f.description}</div>
                  </a>
                ))}
              </div>
            </>
          )}
        </div>
      )}

      {view === "tools" && (
        <div
          role="tabpanel"
          id="skills-view-panel-tools"
          aria-labelledby="skills-view-tab-tools"
        >
          <UserMcpServersCard />
        </div>
      )}
    </div>
  );
}
