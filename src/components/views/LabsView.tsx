import { useEffect, useRef, useState } from "react";
import { FlaskConical, Bird, BrainCircuit } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { invokeBackendCommand } from "../../context/AppContext";

interface FeatureToggleCardProps {
  title: string;
  subtitle: string;
  description: string;
  switchLabel: string;
  getCommand: string;
  setCommand: string;
  defaultEnabled: boolean;
  Icon: LucideIcon;
}

function FeatureToggleCard({
  title,
  subtitle,
  description,
  switchLabel,
  getCommand,
  setCommand,
  defaultEnabled,
  Icon,
}: FeatureToggleCardProps) {
  const [enabled, setEnabled] = useState(defaultEnabled);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const mountedRef = useRef(true);

  useEffect(() => () => {
    mountedRef.current = false;
  }, []);

  useEffect(() => {
    let alive = true;
    invokeBackendCommand<boolean>(getCommand)
      .then((v) => {
        if (alive) setEnabled(Boolean(v));
      })
      .catch(() => {})
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [getCommand]);

  const onToggle = () => {
    if (busy || loading) return;
    const next = !enabled;
    setEnabled(next);
    setBusy(true);
    invokeBackendCommand<boolean>(setCommand, { enabled: next })
      .catch(() => {
        if (mountedRef.current) setEnabled(!next);
      })
      .finally(() => {
        if (mountedRef.current) setBusy(false);
      });
  };

  return (
    <div className="rounded-2xl border border-cream-200 bg-white p-5">
      <div className="flex items-center justify-between">
        <div className="flex flex-col">
          <div className="flex h-9 w-9 items-center justify-center rounded-xl bg-amber/10">
            <Icon className="h-4 w-4 text-amber-dark" />
          </div>
          <div className="mt-3 flex flex-col">
            <span className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
              {title}
            </span>
            <span className="text-[11px] text-cream-400">{subtitle}</span>
          </div>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={enabled}
          aria-label={switchLabel}
          disabled={busy || loading}
          onClick={() => void onToggle()}
          className={`relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors disabled:opacity-50 ${
            enabled ? "bg-teal" : "bg-cream-300"
          }`}
        >
          <span
            aria-hidden="true"
            className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform ${
              enabled ? "translate-x-4" : "translate-x-0.5"
            }`}
          />
        </button>
      </div>
      <p className="mt-4 text-[12px] leading-5 text-cream-600">{description}</p>
      <p className="mt-2 text-[11px] text-cream-400">Applies on app restart.</p>
    </div>
  );
}

export function LabsView() {
  return (
    <div className="mx-auto max-w-3xl px-6 py-8">
      <div className="mb-6 flex items-center gap-3">
        <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-amber/10">
          <FlaskConical className="h-4 w-4 text-amber-dark" />
        </div>
        <div>
          <h1 className="text-lg font-semibold text-cream-900">Labs</h1>
          <p className="text-sm text-cream-500">
            Experimental features. Toggle on/off — changes apply on restart.
          </p>
        </div>
      </div>
      <div className="grid gap-4">
        <FeatureToggleCard
          title="Pigeon"
          subtitle="Async agent mailbox"
          description="Persistent mailbox that lets agents hand off tasks across model load/unload. Optional; off by default."
          switchLabel="Toggle Pigeon"
          getCommand="get_pigeon_enabled"
          setCommand="set_pigeon_enabled"
          defaultEnabled={false}
          Icon={Bird}
        />
        <FeatureToggleCard
          title="Oracle"
          subtitle="RAG retrieval server"
          description="The resident embeddings/RAG server agents query for project context. On by default."
          switchLabel="Toggle Oracle"
          getCommand="get_oracle_enabled"
          setCommand="set_oracle_enabled"
          defaultEnabled={true}
          Icon={BrainCircuit}
        />
      </div>
    </div>
  );
}

export default LabsView;
