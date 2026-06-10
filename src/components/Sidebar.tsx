import {
  LayoutDashboard,
  Boxes,
  Cloud,
  KeyRound,
  Cpu,
  Wallet,
  Shield,
  Network,
  BrainCircuit,
  Bot,
  FolderKanban,
  HardDrive,
  MonitorSmartphone,
  Castle,
  Palette,
  Settings,
  type LucideIcon,
} from "lucide-react";
import type { NavItem } from "../types/config";
import { useAppActions, useAppContext } from "../context/AppContext";
import { navIdsForRole } from "../utils/roles";

const iconMap: Record<string, LucideIcon> = {
  LayoutDashboard,
  Boxes,
  Cloud,
  KeyRound,
  Cpu,
  Wallet,
  Network,
  BrainCircuit,
  FolderKanban,
  Bot,
  HardDrive,
  MonitorSmartphone,
  Castle,
  Palette,
};

// The Polis isometric map nav entry. Injected by the Sidebar so it appears
// regardless of whether the backend config knows about it yet.
const POLIS_NAV: NavItem = { id: "polis", label: "Polis", icon: "Castle" };

// The generative-design module nav entry. Injected the same way as Polis so it
// appears regardless of whether the backend config lists it yet.
const DESIGN_NAV: NavItem = { id: "design", label: "Design", icon: "Palette" };

export function Sidebar() {
  const { config, activeView, roleStatus } = useAppContext();
  const { setActiveView } = useAppActions();
  const { project } = config;
  // Ensure the Polis map and Design entries are always present, even if the
  // backend config does not list them yet (injected the same way).
  const withPolis = config.navigation.some((n) => n.id === POLIS_NAV.id)
    ? config.navigation
    : [...config.navigation, POLIS_NAV];
  const baseNavigation = withPolis.some((n) => n.id === DESIGN_NAV.id)
    ? withPolis
    : [...withPolis, DESIGN_NAV];
  // Filter by role (cosmetic — the backend enforces privileged commands). A
  // null/loading role defaults to the restricted collaborator set.
  const allowedIds = new Set(
    navIdsForRole(
      roleStatus?.role ?? null,
      baseNavigation.map((n) => n.id),
    ),
  );
  const navigation = baseNavigation.filter((n) => allowedIds.has(n.id));

  return (
    <aside className="flex max-h-44 shrink-0 flex-col border-b border-cream-200 bg-cream-50 md:h-screen md:max-h-none md:w-60 md:border-b-0 md:border-r">
      <div className="flex items-center gap-3 p-4 pb-2 md:p-6 md:pb-4">
        <div className="flex h-9 w-9 items-center justify-center rounded-2xl bg-terracotta">
          <Shield className="w-5 h-5 text-white" />
        </div>
        <div className="min-w-0">
          <h1 className="truncate text-sm font-semibold leading-tight text-cream-800">
            {project.name}
          </h1>
          <p className="text-[11px] text-cream-400 tracking-wide uppercase">
            Management
          </p>
        </div>
      </div>

      <nav className="mt-1 flex-1 overflow-x-auto px-3 pb-2 md:mt-2 md:overflow-x-visible md:pb-0">
        <p className="mb-2 hidden px-4 text-[10px] font-semibold uppercase tracking-widest text-cream-400 md:block">
          Menu
        </p>
        <div className="flex gap-1 md:block">
          {navigation.map((item) => {
            const Icon = iconMap[item.icon] || LayoutDashboard;
            const isActive = activeView === item.id;

            return (
              <button
                key={item.id}
                onClick={() => setActiveView(item.id)}
                data-help-title={`This opens the ${item.label} page.`}
                data-help-lines="The sidebar only changes what you see in the app.|It does not call Cloudflare, Scaleway, Oracle, or agents by itself.|Use Projects for the work board; open a project to enter its Work mode with agent terminals, and use provider pages for live cloud operations.|If a page looks empty, run its sync or refresh action after opening it."
                className={`
                flex shrink-0 items-center gap-2 rounded-2xl px-3 py-2.5 md:mb-0.5 md:w-full md:gap-3 md:px-4
                text-[13px] font-medium transition-all duration-200 cursor-pointer
                focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-terracotta/30
                ${
                  isActive
                    ? "bg-white text-cream-800 shadow-soft-sm"
                    : "text-cream-500 hover:text-cream-700 hover:bg-cream-100/60"
                }
              `}
              >
                <Icon
                  className={`w-[18px] h-[18px] ${
                    isActive ? "text-terracotta" : ""
                  }`}
                />
                <span className="whitespace-nowrap">{item.label}</span>
              </button>
            );
          })}
        </div>
      </nav>

      <div className="hidden border-t border-cream-200 p-3 md:block">
        <button
          type="button"
          onClick={() => setActiveView("settings")}
          data-help-title="This opens Settings."
          data-help-lines="Settings holds Secrets, Workspace, your account, and Oracle answer settings.|Device roster management is here too, for admins only.|Opening Settings does not change cloud resources by itself.|Use the Lock button in Settings or the header to lock the app."
          className={`flex w-full items-center gap-3 rounded-2xl px-2 py-2 text-left transition-colors ${
            activeView === "settings"
              ? "bg-white shadow-soft-sm"
              : "hover:bg-cream-100/60"
          }`}
        >
          <div className="w-8 h-8 rounded-full bg-terracotta-100 flex items-center justify-center">
            <span className="text-[11px] font-semibold text-terracotta-500">
              MG
            </span>
          </div>
          <div className="min-w-0">
            <p className="truncate text-[13px] font-medium text-cream-700">
              {roleStatus?.isAdmin ? "Administrator" : "Collaborator"}
            </p>
            <p className="truncate text-[11px] text-cream-400">
              {roleStatus?.provisioned === false ? "Onboarding" : "Settings"}
            </p>
          </div>
          <Settings
            className={`ml-auto h-4 w-4 shrink-0 ${
              activeView === "settings" ? "text-terracotta" : "text-cream-400"
            }`}
          />
        </button>
      </div>
    </aside>
  );
}
