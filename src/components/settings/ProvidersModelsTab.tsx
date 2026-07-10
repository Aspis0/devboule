import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
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
// an AI provider/model. Layout:
//   - Detected strip   — one detect_providers call, reusing pure designProviderDetection
//   - Roles table      — <RolesTableCard /> (orchestrator / coder / verifier per-role config)
//   - Models group     — <RecommendedConfigCard />, <ModelRegistryCard />
//   - Gates & helpers  — <CensorLocalAiCard />, <DesignLlmBackendCard />,
//                        <MiniWriteBehaviorCard />, <WebSearchCard />
//   - Extensions group — <BundledExtensionsCard />, <UserMcpServersCard />,
//                        <PiExtensionsCard />
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

// S1: a collapsible group wrapper so the long Providers & Models scroll becomes a few
// expandable sections (progressive disclosure). Generated by the local Qwen model.
function CollapsibleGroup({
  title,
  defaultOpen = true,
  children,
}: {
  title: string;
  defaultOpen?: boolean;
  children: React.ReactNode;
}) {
  const [isOpen, setIsOpen] = useState(defaultOpen);
  // B-F8 (a11y): stable id linking the header button to the content it toggles.
  const contentId = `group-${title.replace(/\s+/g, "-").toLowerCase()}`;
  return (
    <section className="space-y-2">
      <button
        type="button"
        onClick={() => setIsOpen(!isOpen)}
        aria-expanded={isOpen}
        aria-controls={contentId}
        className="flex w-full items-center justify-between gap-2 rounded-2xl border border-cream-200 bg-cream-50/60 px-4 py-3 text-left"
      >
        <span className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          {title}
        </span>
        {isOpen ? (
          <ChevronDown className="h-4 w-4 text-cream-400" />
        ) : (
          <ChevronRight className="h-4 w-4 text-cream-400" />
        )}
      </button>
      {/* B-F4: keep children MOUNTED when collapsed (display-toggle, not unmount) so a
          half-filled form — an Oracle API key, an in-progress MCP consent dialog — is not
          silently destroyed when the group is collapsed. Matches pre-S1 mount behaviour. */}
      <div id={contentId} className={isOpen ? "mt-4 space-y-6" : "hidden"}>
        {children}
      </div>
    </section>
  );
}

export function ProvidersModelsTab() {
  return (
    <div className="max-w-3xl space-y-6">
      <DetectedProvidersStrip />

      <RecommendedConfigCard />

      <RolesTableCard />

      <CollapsibleGroup title="Models">
        <RoleSection
          title="Model registry"
          description="The curated list of local models the coders may choose from per role, each with a tier (agentic / emit-edits) and tuned defaults."
        >
          <ModelRegistryCard />
        </RoleSection>
      </CollapsibleGroup>

      {/* Role untangle (P6b): Censor and the Design LLM are NOT agent roles — Censor is a
          review GATE (no Kanban/claim/write), Design is a rendering helper. They configure a
          backend like the roles, so they live here together, distinct from the Roles table. */}
      <CollapsibleGroup title="Gates & helpers">
        <RoleSection
          title="Censor model"
          description="A review GATE, not a role: where Censor's tier-2 local review runs (Ollama, local oMLX, or Apple on-device) and which model it uses."
        >
          <CensorLocalAiCard />
        </RoleSection>

        <RoleSection
          title="Design LLM"
          description="A rendering helper: the model the generative-design module generates node markup with."
        >
          <DesignLlmBackendCard />
        </RoleSection>

        <RoleSection
          title="Mini write behavior"
          description="The ceiling for how your coders delegate file writes to the local mini."
        >
          <MiniWriteBehaviorCard />
        </RoleSection>
      </CollapsibleGroup>

      <CollapsibleGroup title="Extensions" defaultOpen={false}>
        <WebSearchCard />
        <BundledExtensionsCard />
        <UserMcpServersCard />
        <PiExtensionsCard />
      </CollapsibleGroup>
    </div>
  );
}
