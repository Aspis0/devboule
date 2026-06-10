import { useState } from "react";
import { KeyRound, RotateCw, Eye, EyeOff } from "lucide-react";
import type { Secret } from "../../types/config";

interface SecretsCardProps {
  secrets: Secret[];
}

export function SecretsCard({ secrets }: SecretsCardProps) {
  // Reveal state is local to this card: toggling a single secret must not
  // re-render the whole app.
  const [revealedSecrets, setRevealedSecrets] = useState<
    Record<string, boolean>
  >({});
  const toggleSecret = (id: string) =>
    setRevealedSecrets((prev) => ({ ...prev, [id]: !prev[id] }));

  return (
    <div className="bg-white rounded-3xl border border-cream-200 p-6">
      <h3 className="text-[11px] font-semibold text-cream-500 uppercase tracking-widest mb-3">
        API Keys
      </h3>
      <div className="divide-y divide-cream-100">
        {secrets.map((secret) => (
          <div
            key={secret.id}
            className="flex items-center justify-between py-3"
          >
            <div className="flex items-center gap-3 min-w-0">
              <div className="w-8 h-8 rounded-xl bg-cream-50 flex items-center justify-center shrink-0">
                <KeyRound className="w-4 h-4 text-cream-500" />
              </div>
              <div className="min-w-0">
                <p className="text-[13px] font-medium text-cream-700 truncate">
                  {secret.name}
                </p>
                <p className="text-[11px] text-cream-400 font-mono">
                  {revealedSecrets[secret.id]
                    ? "sk_live_••••••••"
                    : "••••••••••••"}
                </p>
              </div>
            </div>
            <div className="flex items-center gap-1 shrink-0">
              <button
                onClick={() => toggleSecret(secret.id)}
                data-help-title={`This reveals or hides ${secret.name}.`}
                data-help-lines="Reveal is only for local inspection of a masked credential.|For Aspis Bio, raw keys should stay out of screenshots, project notes, Oracle, and agent prompts.|If you need to rotate or replace a provider token, use the full Secrets page.|Hide the value again before sharing the screen."
                className="p-1.5 rounded-xl hover:bg-cream-50 transition-colors"
              >
                {revealedSecrets[secret.id] ? (
                  <EyeOff className="w-3.5 h-3.5 text-cream-400" />
                ) : (
                  <Eye className="w-3.5 h-3.5 text-cream-400" />
                )}
              </button>
              <button
                className="p-1.5 rounded-xl hover:bg-cream-50 transition-colors group"
                data-help-title={`Rotation shortcut for ${secret.name}.`}
                data-help-lines="This card-level shortcut is visual only unless wired to a backend rotation flow.|For Aspis Bio, real token rotation should happen in Secrets or the provider console with scope audit after saving.|Temporary tokens expire, so keep expiry dates in task notes without storing raw token values.|Do not assume this icon rotated anything unless the page reports success."
              >
                <RotateCw className="w-3.5 h-3.5 text-cream-400 group-hover:text-terracotta transition-colors" />
              </button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
