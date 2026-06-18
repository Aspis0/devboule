import { Search, Bell, Lock, AlertTriangle, AlertCircle, Info, UserCircle2 } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  invokeBackendCommand,
  useAppActions,
  useAppContext,
} from "../context/AppContext";
import { attentionBellTarget, parseViewTarget } from "../utils/deepLink";
import type {
  AgentSession,
  LocalRoleStatus,
  RiskFlag,
  Role,
} from "../types/backend";
import { useAgentAttentionStore } from "../store/agentAttentionStore";
import { attentionSessions } from "./agents/agentFleet";
import { stripSpoofChars } from "./agents/attentionNotifier";
import { combineBadgeCount } from "./headerBadge";
import { useNow } from "../hooks/useNow";

const viewTitles: Record<string, string> = {
  providers: "Providers & Cloud",
  projects: "Projects & Agents",
  settings: "Settings",
  polis: "Polis",
  oracle: "Oracle",
  design: "Design",
  // Re-homed views still resolve a title for any lingering deep-link.
  secrets: "Secrets & API Keys",
  compute: "Infrastructure & Compute",
  cloudflare: "Cloudflare",
  budget: "Budget & Consumption",
  devices: "Devices",
  workspace: "Workspace",

};

// Jump-search targets after the sidebar was compressed. Includes the re-homed
// pages as "view#tab" deep-links so search still reaches Secrets, Cloudflare,
// Compute, Budget, Devices and Workspace. The standalone Agents page was
// dissolved (Phase G): agents now live inside a project's Work mode, reached by
// opening a project from the Board — so there is no separate Agents jump target.
const JUMP_TARGETS: { label: string; target: string }[] = [
  { label: "Projects", target: "projects" },
  // The standalone Oracle page (search + info + admin) is the primary target.
  { label: "Oracle", target: "oracle" },
  // The Polis parchment ask panel is an additional way to ask Oracle from the map.
  { label: "Oracle (Ask)", target: "polis" },
  { label: "Providers", target: "providers" },
  { label: "Cloudflare", target: "providers#cloudflare" },
  { label: "Scaleway / Compute", target: "providers#scaleway" },
  { label: "Budget", target: "providers#budget" },
  { label: "Polis", target: "polis" },
  { label: "Settings", target: "settings" },
  { label: "Secrets", target: "settings#secrets" },
  { label: "Workspace", target: "settings#workspace" },
  { label: "Devices", target: "settings#devices" },
];

const riskIconConfig = {
  high: { icon: AlertTriangle, text: "text-coral-dark", bg: "bg-coral/10" },
  medium: { icon: AlertCircle, text: "text-amber-dark", bg: "bg-amber/10" },
  low: { icon: Info, text: "text-teal", bg: "bg-teal/10" },
};

// Map a risk flag to a deep-link "view#tab" target. The re-homed pages now live
// under Settings (secrets) and Providers (cloudflare/scaleway/budget).
function viewForRisk(flag: RiskFlag): string {
  const source = `${flag.source} ${flag.title} ${flag.description}`.toLowerCase();
  if (source.includes("object storage") || source.includes("access key")) {
    return "settings#secrets";
  }
  if (source.includes("secret") || source.includes("token") || source.includes("rotation")) {
    return "settings#secrets";
  }
  if (source.includes("scaleway") || source.includes("gpu") || source.includes("cpu")) {
    return "providers#scaleway";
  }
  if (source.includes("budget") || source.includes("cost")) {
    return "providers#budget";
  }
  if (source.includes("cloudflare") || source.includes("worker")) {
    return "providers#cloudflare";
  }
  return "providers";
}

// Compact "how long ago" label for an agent's needsUser.since timestamp. Returns
// "" for a missing/unparsable value so the caller can omit the age.
function formatSinceAge(since: string | null | undefined, nowMs: number): string {
  if (!since) return "";
  const t = Date.parse(since);
  if (Number.isNaN(t)) return "";
  const seconds = Math.max(0, Math.floor((nowMs - t) / 1000));
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ago`;
}

export function Header() {
  const { activeView, cloudSnapshot, roleStatus } = useAppContext();
  const { requestView, lock, refreshRole } = useAppActions();

  // DEV-only role impersonation. The backend `set_debug_role` is compiled to a
  // no-op/error in release, and this control is hidden outside dev, so it cannot
  // be used in production.
  const setDebugRole = async (role: Role | null) => {
    try {
      await invokeBackendCommand<LocalRoleStatus>("set_debug_role", { role });
      await refreshRole();
    } catch {
      // dev-only; ignore
    }
  };
  const [query, setQuery] = useState("");
  const [notificationsOpen, setNotificationsOpen] = useState(false);
  const notificationsRef = useRef<HTMLDivElement | null>(null);
  const cleanQuery = query.trim().toLowerCase();
  const risks = cloudSnapshot?.risks ?? [];
  const riskCount = risks.length;

  // Agents needing the human. Fed by the existing agent-live-state pollers via
  // the attention store (no new poller). A live clock keeps stale/lost health
  // current between polls; attentionSessions is the SINGLE attention predicate.
  const attentionStoreSessions = useAgentAttentionStore((s) => s.sessions);
  const now = useNow();
  const attention = useMemo(
    () => attentionSessions(attentionStoreSessions, now),
    [attentionStoreSessions, now],
  );
  const attentionCount = attention.length;
  const badgeCount = combineBadgeCount(riskCount, attentionCount);
  const matches = useMemo(() => {
    if (!cleanQuery) return [];
    return JUMP_TARGETS.filter((item) =>
      `${item.target} ${item.label}`.toLowerCase().includes(cleanQuery),
    ).slice(0, 5);
  }, [cleanQuery]);

  // Open a "view" or "view#tab" deep-link target: route the view and stage the
  // requested sub-tab for the destination to consume.
  const openView = (target: string) => {
    const { view, tab } = parseViewTarget(target);
    requestView(view, tab);
    setQuery("");
    setNotificationsOpen(false);
  };

  // Open the needs-you agent in its project's Work mode (Phase G: the standalone
  // Agents page is gone). The session carries the agent's current project id, so
  // deep-link straight into that project's full-screen workspace via
  // `projects#work:<projectId>` (ProjectsView consumes the work:<id> tab and
  // enters Work mode for it). When the agent has NO resolvable project (a
  // project-less session), attentionBellTarget falls back to the Projects Board
  // (no tab) so the click never produces a dead `work:` token or crashes.
  const openAgent = (session: AgentSession) => {
    const { view, tab } = attentionBellTarget(session.currentProjectId);
    requestView(view, tab);
    setNotificationsOpen(false);
  };

  useEffect(() => {
    if (!notificationsOpen) return;

    const closeOnOutsideClick = (event: MouseEvent) => {
      if (
        notificationsRef.current &&
        !notificationsRef.current.contains(event.target as Node)
      ) {
        setNotificationsOpen(false);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setNotificationsOpen(false);
    };

    document.addEventListener("mousedown", closeOnOutsideClick);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("mousedown", closeOnOutsideClick);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [notificationsOpen]);

  return (
    <header className="relative z-40 flex min-h-16 shrink-0 flex-wrap items-center justify-between gap-3 border-b border-cream-200 bg-cream-50/80 px-4 py-3 backdrop-blur-sm md:h-16 md:flex-nowrap md:px-8 md:py-0">
      <h2 className="min-w-0 truncate text-base font-semibold text-cream-800">
        {viewTitles[activeView] || "Projects"}
      </h2>

      <div className="flex min-w-0 flex-1 items-center justify-end gap-2 md:flex-none md:gap-3">
        <div className="relative min-w-0 flex-1 md:flex-none">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-cream-400" />
          <input
            type="text"
            value={query}
            data-help-title="This search jumps to another page."
            data-help-lines="Type part of a page name, then press Enter to open the first match.|It only changes navigation and does not run provider commands.|Use Escape to clear the search.|If you cannot find an action, open the matching provider or project page first."
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && matches[0]) {
                openView(matches[0].target);
              }
              if (event.key === "Escape") {
                setQuery("");
              }
            }}
            placeholder="Jump to view..."
            className="w-full min-w-0 rounded-2xl border border-cream-200 bg-cream-100/60 py-2 pl-9 pr-4 md:w-56
                       text-[13px] text-cream-700 placeholder:text-cream-400
                       focus:outline-none focus:border-terracotta-200 focus:ring-2 focus:ring-terracotta-100
                       transition-all duration-200"
          />
          {matches.length > 0 && (
            <div className="absolute right-0 top-[calc(100%+8px)] z-40 w-full min-w-56 overflow-hidden rounded-xl border border-cream-200 bg-white shadow-soft md:w-56">
              {matches.map((item) => (
                <button
                  key={item.target}
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={() => openView(item.target)}
                  className="block w-full px-3 py-2 text-left text-[12px] font-medium text-cream-700 hover:bg-cream-50"
                >
                  {item.label}
                </button>
              ))}
            </div>
          )}
        </div>

        <div ref={notificationsRef} className="relative">
          <button
            type="button"
            onClick={() => setNotificationsOpen((open) => !open)}
            data-help-title="This opens risk flags and agents that need you."
            data-help-lines="A risk flag is a warning produced by the last provider sync; click it to jump to the page that can fix it.|An 'agent needs you' entry means a launched agent is waiting on a human answer (a question, an allow/deny, or a block).|Click an agent to open its project's Work mode where its terminal lives.|The count combines risk flags and agents waiting on you."
            className="relative p-2.5 rounded-2xl hover:bg-cream-100 transition-colors"
            title={badgeCount > 0 ? `${badgeCount} notification${badgeCount === 1 ? "" : "s"}` : "No notifications"}
            aria-label="Notifications"
            aria-expanded={notificationsOpen}
            aria-haspopup="menu"
          >
            <Bell className="w-[18px] h-[18px] text-cream-500" />
            {badgeCount > 0 && (
              <span className="absolute top-1.5 right-1.5 min-w-3 rounded-full bg-coral px-1 text-[8px] font-semibold leading-3 text-white">
                {badgeCount > 9 ? "9+" : badgeCount}
              </span>
            )}
          </button>

          {notificationsOpen && (
            <div className="absolute right-0 top-[calc(100%+8px)] z-[80] w-[min(20rem,calc(100vw-2rem))] overflow-hidden rounded-xl border border-cream-200 bg-white shadow-soft-lg">
              {attentionCount > 0 && (
                <div className="border-b border-cream-100">
                  <div className="flex items-center justify-between bg-terracotta/[0.06] px-4 py-3">
                    <p className="text-[11px] font-semibold uppercase tracking-widest text-terracotta">
                      Agents need you
                    </p>
                    <span className="text-[11px] font-medium text-cream-400">
                      {attentionCount}
                    </span>
                  </div>
                  <div className="max-h-60 overflow-y-auto py-1">
                    {attention.map((session) => {
                      // Strip bidi/zero-width spoofing chars from the untrusted
                      // agent-supplied id + message before rendering them raw.
                      const agentId = stripSpoofChars(session.agentId);
                      const rawMessage =
                        session.needsUser?.message ??
                        session.message ??
                        "Waiting on a response.";
                      const message =
                        stripSpoofChars(rawMessage) || "Waiting on a response.";
                      const age = formatSinceAge(session.needsUser?.since, now);
                      return (
                        <button
                          key={session.agentId}
                          onClick={() => openAgent(session)}
                          className="flex w-full items-start gap-3 px-4 py-3 text-left hover:bg-cream-50"
                        >
                          <span className="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-terracotta/10">
                            <UserCircle2 className="h-3.5 w-3.5 text-terracotta" />
                          </span>
                          <span className="min-w-0 flex-1">
                            <span className="block truncate text-[13px] font-semibold text-cream-800">
                              {agentId}
                            </span>
                            <span className="mt-0.5 line-clamp-2 block text-[11px] leading-relaxed text-cream-500">
                              {message}
                            </span>
                            {age && (
                              <span className="mt-1 block text-[10px] text-cream-400">
                                {session.role} · {age}
                              </span>
                            )}
                          </span>
                        </button>
                      );
                    })}
                  </div>
                </div>
              )}
              <div className="flex items-center justify-between border-b border-cream-100 px-4 py-3">
                <p className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
                  Risk Flags
                </p>
                <span className="text-[11px] font-medium text-cream-400">
                  {riskCount}
                </span>
              </div>
              <div className="max-h-80 overflow-y-auto py-1">
                {risks.map((risk) => {
                  const cfg =
                    riskIconConfig[risk.severity as keyof typeof riskIconConfig] ??
                    riskIconConfig.low;
                  const Icon = cfg.icon;

                  return (
                    <button
                      key={risk.id}
                      onClick={() => openView(viewForRisk(risk))}
                      className="flex w-full items-start gap-3 px-4 py-3 text-left hover:bg-cream-50"
                    >
                      <span
                        className={`mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-lg ${cfg.bg}`}
                      >
                        <Icon className={`h-3.5 w-3.5 ${cfg.text}`} />
                      </span>
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-[13px] font-semibold text-cream-800">
                          {risk.title}
                        </span>
                        <span className="mt-0.5 line-clamp-2 block text-[11px] leading-relaxed text-cream-500">
                          {risk.description}
                        </span>
                        <span className="mt-1 block text-[10px] text-cream-400">
                          {risk.source} · {risk.timestamp}
                        </span>
                      </span>
                    </button>
                  );
                })}
                {riskCount === 0 && (
                  <p className="px-4 py-3 text-[12px] text-cream-400">
                    No provider risks reported.
                  </p>
                )}
              </div>
            </div>
          )}
        </div>

        {Boolean(
          (import.meta as ImportMeta & { env?: { DEV?: boolean } }).env?.DEV,
        ) && (
          <select
            value={roleStatus?.role ?? ""}
            onChange={(event) =>
              void setDebugRole((event.target.value || null) as Role | null)
            }
            title="DEV only: impersonate a role (compiled out of release)"
            className="rounded-2xl border border-amber/40 bg-amber/10 px-2 py-1.5 text-[11px] font-semibold text-amber-dark outline-none"
          >
            <option value="admin">role: admin</option>
            <option value="collaborator">role: collaborator</option>
            <option value="">role: reset</option>
          </select>
        )}

        <button
          onClick={lock}
          data-help-title="This locks the app again."
          data-help-lines="Locking hides the dashboard behind device authentication (Windows Hello or Touch ID).|It does not stop background provider data already loaded in memory.|Use it before leaving the computer unattended.|Unlock again with PIN, face, or fingerprint depending on your system setup."
          className="p-2.5 rounded-2xl hover:bg-cream-100 transition-colors"
          title="Lock app"
        >
          <Lock className="w-[18px] h-[18px] text-cream-500" />
        </button>
      </div>
    </header>
  );
}
