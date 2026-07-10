import {
  AlertTriangle,
  CheckCircle2,
  Cpu,
  RefreshCw,
  Terminal,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invokeBackendCommand } from "../../context/AppContext";
import { DESIGN_BACKEND_KINDS } from "../design/designLlmBackend";
import {
  buildProviderStatusMap,
  selectorLabel,
  type ProviderStatusMap,
} from "../design/designProviderDetection";
import type { DetectedProvider } from "../../types/config";
import { RolesTableCard } from "./RolesTableCard";
import { RecommendedConfigCard } from "./RecommendedConfigCard";
import { detectApplePlatform } from "../../lib/platform";
import { ModelRegistryCard } from "./ModelRegistryCard";
import { MiniWriteBehaviorCard } from "./MiniWriteBehaviorCard";
import { WebSearchCard } from "./WebSearchCard";
import { DesignLlmBackendCard } from "./DesignLlmBackendCard";
import { CensorLocalAiCard } from "../views/WorkspaceView";
import { UserMcpServersCard } from "./UserMcpServersCard";
import { BundledExtensionsCard } from "./BundledExtensionsCard";
import { PiExtensionsCard } from "../views/PiExtensionsCard";

// Phase 5 — the "Providers & Models" tab: a single home for everything that picks
// an AI provider/model. Layout is split into SEMANTIC SUB-PAGES (an internal pill
// bar) so the long scroll becomes a few focused sub-tabs:
//   - Detected strip   — one detect_providers call, reusing pure designProviderDetection
//                        (shared, mounted once at the top of the tab)
//   - Models            — <RecommendedConfigCard />, <RolesTableCard />, <ModelRegistryCard />
//   - Gates & helpers   — <CensorLocalAiCard />, <MiniWriteBehaviorCard />, <WebSearchCard />
//   - Extensions        — <BundledExtensionsCard />, <UserMcpServersCard />, <PiExtensionsCard />
//   - Design            — <DesignLlmBackendCard />
//
// NOTE: Oracle LLM config and LocalCoderBackendCard are NOT here — Oracle LLM lives
// inside OracleAdminPanel on the Oracle page, and LocalCoderBackendCard was replaced
// by the RolesTableCard's per-role config.
//
// PRIVACY (W2): the detection strip shows availability + live model COUNT only. The
// resolved CLI path is deliberately NOT shown — the engine never sends it over IPC
// (it would leak the filesystem layout); DetectedProvider has no `path` field.

// The shared "Detected on this machine" strip. Self-contained: one detect_providers
// call with an out-of-order guard, loading/empty/error states. Mirrors the design
// card's detection block but read-only (it does not configure anything itself).
function DetectedProvidersStrip() {
  const [detected, setDetected] = useState<DetectedProvider[] | null>(null);
  const [detecting, setDetecting] = useState(false);
  const [detectError, setDetectError] = useState<string | null>(null);
  const mountedRef = useRef(true);
  const detectId = useRef(0);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const runDetect = useCallback(async () => {
    const id = detectId.current + 1;
    detectId.current = id;
    setDetecting(true);
    setDetectError(null);
    try {
      const result =
        await invokeBackendCommand<DetectedProvider[]>("detect_providers");
      if (mountedRef.current && detectId.current === id) {
        setDetected(Array.isArray(result) ? result : []);
      }
    } catch (e) {
      if (mountedRef.current && detectId.current === id) {
        setDetectError(
          e instanceof Error ? e.message : "Provider detection failed.",
        );
        // Keep any prior result on a transient failure.
      }
    } finally {
      if (mountedRef.current && detectId.current === id) {
        setDetecting(false);
      }
    }
  }, []);

  useEffect(() => {
    void runDetect();
  }, [runDetect]);

  const statusMap: ProviderStatusMap = useMemo(
    () => buildProviderStatusMap(detected),
    [detected],
  );

  // Empty == detection ran and found NO available CLI/HTTP provider (api is always
  // "configurable" and excluded from this judgement).
  const noneAvailable = useMemo(
    () =>
      detected !== null &&
      DESIGN_BACKEND_KINDS.every((k) => k === "api" || !statusMap[k].available),
    [detected, statusMap],
  );

  const detectedAppleFm = useMemo(() => {
    if (!Array.isArray(detected)) return null;
    const entry = detected.find((candidate) => candidate?.kind === "appleFm");
    if (!entry) return null;
    if (typeof entry !== "object") return null;
    return entry as DetectedProvider;
  }, [detected]);

  const isAppleHost = detectApplePlatform();

  const appleFmStatusText = useMemo(() => {
    if (!detectedAppleFm) return null;
    const available = detectedAppleFm.available === true && isAppleHost !== false;
    const details = String(detectedAppleFm.detail ?? "").trim();
    const models = Array.isArray(detectedAppleFm.models)
      ? detectedAppleFm.models.filter((m): m is string => typeof m === "string")
      : [];
    if (available) {
      if (models.length > 0) {
        return `running (${models.length} model${models.length === 1 ? "" : "s"})`;
      }
      return details || "configured";
    }
    if (isAppleHost === false) return "not available on this OS";
    return "not found";
  }, [detectedAppleFm, isAppleHost]);

  return (
    <section className="rounded-2xl border border-cream-200 bg-cream-50/60 p-4">
      <div className="mb-3 flex items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <Cpu className="h-3.5 w-3.5 text-teal" />
          <span className="text-[10px] font-semibold uppercase tracking-widest text-cream-500">
            Detected on this machine
          </span>
        </div>
        <button
          type="button"
          onClick={() => void runDetect()}
          disabled={detecting}
          className="inline-flex items-center gap-2 rounded-md border border-cream-200 bg-white px-3 py-1.5 text-[11px] font-semibold text-cream-600 hover:text-terracotta disabled:opacity-60"
        >
          <RefreshCw className={`h-3.5 w-3.5 ${detecting ? "animate-spin" : ""}`} />
          {detecting ? "Detecting..." : "Re-detect"}
        </button>
      </div>

      {detecting && detected === null ? (
        <p className="text-[11px] text-cream-400">Detecting providers...</p>
      ) : (
        <>
          <ul className="grid gap-1.5 sm:grid-cols-2">
            {DESIGN_BACKEND_KINDS.map((k) => {
              const s = statusMap[k];
              const good = k === "api" ? false : s.available;
              const bad = k !== "api" && !s.available;
              return (
                <li
                  key={k}
                  className="flex items-center justify-between gap-2 rounded-md bg-white px-2.5 py-1.5"
                >
                  <span className="flex items-center gap-1.5 text-[11px] text-cream-700">
                    {good ? (
                      <CheckCircle2 className="h-3.5 w-3.5 shrink-0 text-sage-dark" />
                    ) : bad ? (
                      <AlertTriangle className="h-3.5 w-3.5 shrink-0 text-cream-400" />
                    ) : (
                      <Terminal className="h-3.5 w-3.5 shrink-0 text-cream-400" />
                    )}
                    <span>{selectorLabel(s)}</span>
                  </span>
                  {/* W2: resolved CLI path intentionally NOT shown — never sent
                      over IPC (filesystem-layout leak). Availability + model count
                      (in selectorLabel) is all the UI needs. */}
                </li>
              );
            })}
            {detectedAppleFm ? (
              <li
                key="appleFm"
                className="flex items-center justify-between gap-2 rounded-md bg-white px-2.5 py-1.5"
              >
                <span className="flex items-center gap-1.5 text-[11px] text-cream-700">
                  {appleFmStatusText?.startsWith("running") ||
                  appleFmStatusText === "configured" ? (
                    <CheckCircle2 className="h-3.5 w-3.5 shrink-0 text-sage-dark" />
                  ) : (
                    <AlertTriangle className="h-3.5 w-3.5 shrink-0 text-cream-400" />
                  )}
                  <span>
                    Apple on-device (local model) — {appleFmStatusText}
                  </span>
                </span>
              </li>
            ) : null}
          </ul>
          {noneAvailable ? (
            <p className="mt-2 text-[10px] text-cream-400">
              No local CLI or HTTP provider detected. You can still configure an
              API command or a remote provider below.
            </p>
          ) : null}
        </>
      )}

      {detectError ? (
        <p className="mt-2 text-[10px] text-amber-dark">
          Detection failed ({detectError}). You can still configure a provider
          manually below.
        </p>
      ) : null}
    </section>
  );
}

// One labelled section wrapper so the four per-role blocks read consistently.
function RoleSection({
  title,
  description,
  children,
}: {
  title: string;
  description: string;
  children: React.ReactNode;
}) {
  return (
    <section className="space-y-2">
      <div>
        <h2 className="text-[12px] font-semibold text-cream-800">{title}</h2>
        <p className="text-[11px] leading-4 text-cream-400">{description}</p>
      </div>
      {children}
    </section>
  );
}

// S1: the long Providers & Models scroll is split into SEMANTIC SUB-PAGES
// (sub-tabs) instead of one long page. The shared detection strip stays mounted
// at the top (its detection state — isAppleHost, detect_providers — is shared
// across every sub-tab), and only the active sub-page's cards render below the
// pill bar. Generated by the local Qwen model.
type SubTabId = "models" | "gates" | "extensions" | "design";

const SUB_TABS: { id: SubTabId; label: string }[] = [
  { id: "models", label: "Models" },
  { id: "gates", label: "Gates & helpers" },
  { id: "extensions", label: "Extensions" },
  { id: "design", label: "Design" },
];

export function ProvidersModelsTab() {
  const [activeSubTab, setActiveSubTab] = useState<SubTabId>("models");

  return (
    <div className="max-w-3xl space-y-6">
      {/* Shared detection strip — mounted once at the top so its detection state
          (isAppleHost, detect_providers) is shared across every sub-tab. */}
      <DetectedProvidersStrip />

      {/* Internal sub-tab bar — mirrors the parent SettingsView pill/segmented
          control for visual consistency. Keyboard-navigable with tablist/tab ARIA. */}
      <div
        role="tablist"
        aria-label="Providers & Models sections"
        className="flex w-fit flex-wrap gap-1 rounded-2xl border border-cream-200 bg-white p-1"
      >
        {SUB_TABS.map((tab) => {
          const isActive = activeSubTab === tab.id;
          return (
            <button
              key={tab.id}
              type="button"
              role="tab"
              id={`subtab-${tab.id}`}
              aria-selected={isActive}
              aria-controls={`subtab-panel-${tab.id}`}
              onClick={() => setActiveSubTab(tab.id)}
              className={`flex items-center gap-1.5 rounded-xl px-3 py-1.5 text-[12px] font-semibold transition-colors ${
                isActive
                  ? "bg-terracotta text-white"
                  : "text-cream-500 hover:bg-cream-50 hover:text-cream-700"
              }`}
            >
              {tab.label}
            </button>
          );
        })}
      </div>

      {/* All FOUR sub-panels stay MOUNTED at all times — only the active one is
          shown. The HTML `hidden` attribute keeps each inactive subtree in the
          DOM (preserving half-filled forms / MCP consent state) while removing
          it from the visual layout and the a11y tree. This also guarantees every
          `aria-controls="subtab-panel-${id}"` on the tab buttons resolves to a
          real element. Generated by the local Qwen model. */}
      <div
        role="tabpanel"
        id="subtab-panel-models"
        aria-labelledby="subtab-models"
        hidden={activeSubTab !== "models"}
        className="space-y-6"
      >
        <RecommendedConfigCard />

        <RolesTableCard />

        <RoleSection
          title="Model registry"
          description="The curated list of local models the coders may choose from per role, each with a tier (agentic / emit-edits) and tuned defaults."
        >
          <ModelRegistryCard />
        </RoleSection>
      </div>

      {/* Role untangle (P6b): Censor and the Design LLM are NOT agent roles — Censor is a
          review GATE (no Kanban/claim/write), Design is a rendering helper. They configure a
          backend like the roles, so they live here together, distinct from the Roles table. */}
      <div
        role="tabpanel"
        id="subtab-panel-gates"
        aria-labelledby="subtab-gates"
        hidden={activeSubTab !== "gates"}
        className="space-y-6"
      >
        <RoleSection
          title="Censor model"
          description="A review GATE, not a role: where Censor's tier-2 local review runs (Ollama, local oMLX, or Apple on-device) and which model it uses."
        >
          <CensorLocalAiCard />
        </RoleSection>

        <RoleSection
          title="Mini write behavior"
          description="The ceiling for how your coders delegate file writes to the local mini."
        >
          <MiniWriteBehaviorCard />
        </RoleSection>

        <WebSearchCard />
      </div>

      <div
        role="tabpanel"
        id="subtab-panel-extensions"
        aria-labelledby="subtab-extensions"
        hidden={activeSubTab !== "extensions"}
        className="space-y-6"
      >
        <BundledExtensionsCard />
        <UserMcpServersCard />
        <PiExtensionsCard />
      </div>

      <div
        role="tabpanel"
        id="subtab-panel-design"
        aria-labelledby="subtab-design"
        hidden={activeSubTab !== "design"}
        className="space-y-6"
      >
        <RoleSection
          title="Design LLM"
          description="A rendering helper: the model the generative-design module generates node markup with."
        >
          <DesignLlmBackendCard />
        </RoleSection>
      </div>
    </div>
  );
}
