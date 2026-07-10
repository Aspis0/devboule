import { Suspense, lazy, useEffect, useRef, useState } from "react";
import { Sidebar } from "./components/Sidebar";
import { Header } from "./components/Header";
import { CloudflareView } from "./components/views/CloudflareView";
import { ProvidersView } from "./components/views/ProvidersView";
import { ProjectsView } from "./components/views/ProjectsView";
import { SecretsView } from "./components/views/SecretsView";
import { ComputeView } from "./components/views/ComputeView";
import { BudgetView } from "./components/views/BudgetView";
import { WorkspaceView } from "./components/views/WorkspaceView";
import { DevicesView } from "./components/views/DevicesView";
import { OracleView } from "./components/views/OracleView";
import { SettingsView } from "./components/views/SettingsView";
import { LabsView } from "./components/views/LabsView";
import { LockedScreen } from "./components/auth/LockedScreen";
import { OnboardingWizard } from "./components/onboarding/OnboardingWizard";
import { HelpModeOverlay } from "./components/help/HelpModeOverlay";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { AppProvider, useAppContext } from "./context/AppContext";
import { isViewAllowedForRole } from "./utils/roles";
import { useAgentAttentionStore } from "./store/agentAttentionStore";
import { useDesignVisible } from "./store/labsSettings";
import { startAttentionWatcher } from "./components/agents/attentionNotifier";
import { startAttentionPoller } from "./components/agents/attentionPoller";
import { invokeBackendCommand } from "./context/AppContext";
import type { AgentLiveState } from "./types/backend";

// Lazy-load the Polis isometric map: it pulls in PixiJS + pixi-viewport, so it
// must be its own async chunk and not bloat the initial bundle.
const PolisView = lazy(() =>
  import("./components/polis/PolisView").then((m) => ({ default: m.PolisView })),
);

// Lazy-load the generative-design module: it pulls in DOMPurify + the canvas
// engine, so it gets its own async chunk and never bloats the initial bundle.
const DesignView = lazy(() =>
  import("./components/design/DesignView").then((m) => ({
    default: m.DesignView,
  })),
);

// Lazy-load the per-project Skills view (the SKILL.md editor + toggle + template
// installer). It is its own async chunk to match the Design/Polis pattern and
// keep it off the initial bundle.
const SkillsView = lazy(() =>
  import("./components/views/SkillsView").then((m) => ({
    default: m.SkillsView,
  })),
);

// Lazy-load the Help (getting-started / how-it-works) view. Kept its own async
// chunk to match the Skills/Design/Polis pattern and keep it off the initial bundle.
const HelpView = lazy(() =>
  import("./components/views/HelpView").then((m) => ({
    default: m.HelpView,
  })),
);

function ViewFallback() {
  return (
    <div className="flex items-center gap-2 text-[12px] font-medium text-cream-500">
      <div className="h-3.5 w-3.5 rounded-full border-2 border-cream-300 border-t-terracotta animate-spin" />
      Loading...
    </div>
  );
}

function AppShell() {
  const {
    config,
    activeView,
    isLoading,
    unlockRetryBlocked,
    error,
    isDesktopRuntime,
    isLocked,
    authState,
    roleStatus,
    unlock,
  } = useAppContext();

  // Whether the Labs "Design" nav entry is visible. Used here to reconcile an
  // already-open Design view if the user flips the toggle OFF (the Sidebar drops
  // the nav entry but activeView would still be "design").
  const designVisible = useDesignVisible();

  // Escape hatch so a collaborator can never get wedged in onboarding (audit H1):
  // if their grant keeps failing, they can continue into the app with the default
  // least-privilege Collaborator role. The backend still enforces capabilities
  // fresh, so entering unprovisioned is safe.
  const [onboardingDismissed, setOnboardingDismissed] = useState(false);

  // Mount the OS-notification attention watcher ONCE while the app is unlocked.
  // It subscribes to the agentAttentionStore (fed by the active attention feeder)
  // and fires an OS notification on each needsUser transition. Tearing it down on
  // lock stops notifications while locked and avoids a stranded subscription; it
  // remounts on the next unlock.
  useEffect(() => {
    if (isLocked) return;
    const unsubscribe = startAttentionWatcher(useAgentAttentionStore);
    return unsubscribe;
  }, [isLocked]);

  // Live activeView for the global attention poller. The poller is long-lived
  // (mounted only on lock changes), so it must READ the current view each tick
  // rather than capture it — a ref updated every render is that live getter.
  const activeViewRef = useRef(activeView);
  activeViewRef.current = activeView;

  // Live lock state for the global attention poller. The mount effect is gated on
  // isLocked, but a strict-mode/race re-mount could briefly outlive a lock change,
  // so the poller's tick reads this ref each tick and never fetches/feeds while
  // locked (BLOCKER: deriving "unlocked" from the teardown flag alone is unsafe).
  const isLockedRef = useRef(isLocked);
  isLockedRef.current = isLocked;

  // GLOBAL attention poller (Phase G): keeps the Header bell + OS notifications
  // fed EVERYWHERE, not only on the Projects view. It SKIPS its tick while the
  // Projects view is active (ProjectsView's own poll feeds the store there), so
  // there is exactly ONE attention feeder — never a double get_agent_live_state.
  // Visibility-gated + in-flight-guarded inside startAttentionPoller; feeds ONLY
  // the attention store (never board/project state). Torn down on lock.
  useEffect(() => {
    if (isLocked) return;
    const unsubscribe = startAttentionPoller({
      getActiveView: () => activeViewRef.current,
      isUnlocked: () => !isLockedRef.current,
      fetchLiveState: () =>
        invokeBackendCommand<AgentLiveState>("get_agent_live_state"),
      feed: (state) =>
        useAgentAttentionStore.getState().setFromLiveState(state),
    });
    return unsubscribe;
  }, [isLocked]);

  if (isLocked) {
    return (
      <>
        <LockedScreen
          onUnlock={unlock}
          isLoading={isLoading}
          unlockRetryBlocked={unlockRetryBlocked}
          error={error}
          desktopRuntimeAvailable={isDesktopRuntime}
          helloAvailable={authState?.helloAvailable ?? null}
        />
        <HelpModeOverlay />
      </>
    );
  }

  // Fresh collaborator (has no verified grant yet, and is not the admin): run the
  // onboarding wizard before the app shell. The admin is provisioned by the trust
  // anchor and never sees this.
  if (
    roleStatus &&
    !roleStatus.provisioned &&
    !roleStatus.isAdmin &&
    !onboardingDismissed
  ) {
    return (
      <>
        <OnboardingWizard onSkip={() => setOnboardingDismissed(true)} />
        <HelpModeOverlay />
      </>
    );
  }

  const renderView = () => {
    // Role gate (cosmetic — the backend enforces privileged commands). Only block
    // once the role is KNOWN, so the admin never sees a "not authorized" flash
    // while the role resolves.
    if (roleStatus !== null && !isViewAllowedForRole(roleStatus.role, activeView)) {
      return (
        <div className="rounded-lg border border-cream-200 bg-white p-8 text-center">
          <p className="text-[13px] font-semibold text-cream-800">
            Not available for your role
          </p>
          <p className="mt-1 text-[12px] text-cream-400">
            This page is restricted. Use the menu to open one you have access to.
          </p>
        </div>
      );
    }
    switch (activeView) {
      case "providers":
        return <ProvidersView config={config} />;
      case "cloudflare":
        return <CloudflareView />;
      case "projects":
        return <ProjectsView />;
      case "devices":
        return <DevicesView />;
      case "workspace":
        return <WorkspaceView />;
      case "secrets":
        return <SecretsView />;
      case "compute":
        return <ComputeView />;
      case "budget":
        return <BudgetView />;
      case "oracle":
        return <OracleView />;
      case "labs":
        return <LabsView />;
      case "settings":
        return <SettingsView />;
      case "polis":
        // Wrap the lazy Polis view in an error boundary so a runtime throw
        // inside PixiJS / the renderer shows a contained fallback instead of
        // blanking the whole app. (A synchronous freeze isn't an error and is
        // handled by the renderer's non-blocking chunked build, not here.)
        return (
          <ErrorBoundary label="Polis">
            <Suspense fallback={<ViewFallback />}>
              <PolisView />
            </Suspense>
          </ErrorBoundary>
        );
      case "design":
        // If the Labs "Design" toggle is OFF, the nav entry is gone but the
        // user may still have `activeView === "design"`. Don't keep rendering
        // the (now orphaned) Design module — fall back to the default view so
        // the screen stays reachable and consistent with the sidebar.
        if (!designVisible) return <ProjectsView />;
        return (
          <ErrorBoundary label="Design">
            <Suspense fallback={<ViewFallback />}>
              <DesignView />
            </Suspense>
          </ErrorBoundary>
        );
      case "skills":
        // Lazy Skills view in its own error boundary + Suspense (mirrors Design).
        return (
          <ErrorBoundary label="Skills">
            <Suspense fallback={<ViewFallback />}>
              <SkillsView />
            </Suspense>
          </ErrorBoundary>
        );
      case "help":
        // Lazy Help view in its own error boundary + Suspense (mirrors Skills).
        return (
          <ErrorBoundary label="Help">
            <Suspense fallback={<ViewFallback />}>
              <HelpView />
            </Suspense>
          </ErrorBoundary>
        );
      default:
        return <ProjectsView />;
    }
  };

  return (
    <div className="flex h-screen flex-col bg-cream-100 font-sans md:flex-row">
      <Sidebar />
      <main className="flex min-h-0 flex-1 flex-col overflow-hidden">
        <Header />
        <div className="flex-1 overflow-y-auto p-4 md:p-8">
          {isLoading && (
            <div className="mb-4 flex items-center gap-2 rounded-2xl border border-cream-200 bg-white px-4 py-2 text-[12px] font-medium text-cream-500">
              <div className="h-3.5 w-3.5 rounded-full border-2 border-cream-300 border-t-terracotta animate-spin" />
              Updating...
            </div>
          )}
          {error && (
            <div className="mb-4 px-4 py-3 rounded-2xl bg-coral/10 border border-coral/20 text-coral-dark text-[13px]">
              {error}
            </div>
          )}
          {renderView()}
        </div>
      </main>
      <HelpModeOverlay />
    </div>
  );
}

function App() {
  return (
    <AppProvider>
      <ErrorBoundary label="App">
        <AppShell />
      </ErrorBoundary>
    </AppProvider>
  );
}

export default App;
