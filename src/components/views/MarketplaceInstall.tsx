import { useState } from "react";
import type { MarketplacePreview, RiskSeverity } from "../../types/skills";

interface Props {
  /** Canonical project working folder (where a PROJECT-scope skill installs under .claude/skills/).
   *  Optional: unused (and not required) for scope="global". */
  folderPath?: string;
  /** Backend invoker (passed in so the component is unit-testable without Tauri). */
  invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
  /** Called after a successful install so the parent can refresh its lists. */
  onInstalled?: (dest: string) => void;
  /** Install target: "project" (default, into the working folder) or "global" (the user library). */
  scope?: "project" | "global";
}

const SEVERITY: Record<RiskSeverity, { label: string; chip: string; dot: string }> = {
  Danger: { label: "Danger", chip: "border-red-300 bg-red-50 text-red-700", dot: "bg-red-500" },
  Warn: { label: "Warning", chip: "border-amber-300 bg-amber-50 text-amber-700", dot: "bg-amber-500" },
  Info: { label: "Info", chip: "border-sky-300 bg-sky-50 text-sky-700", dot: "bg-sky-500" },
};

/**
 * Install an external SKILL.md from a marketplace URL — the OWNER-VETTING surface. The flow never
 * auto-installs: paste a URL → Preview (fetch is SSRF-guarded + the body is risk-scanned) → review
 * the findings → confirm. A Danger finding gates the install behind an explicit acknowledgement.
 */
export function MarketplaceInstall({ folderPath, invoke, onInstalled, scope = "project" }: Props) {
  const [url, setUrl] = useState("");
  const [preview, setPreview] = useState<MarketplacePreview | null>(null);
  const [skillName, setSkillName] = useState("");
  const [ackRisk, setAckRisk] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [installed, setInstalled] = useState<string | null>(null);

  const isDanger = preview?.worst === "Danger";
  const canInstall = !!preview && skillName.trim().length > 0 && (!isDanger || ackRisk) && !busy;

  async function doPreview() {
    if (busy) return; // defense-in-depth: the button is disabled + Enter is guarded, but be robust.
    setBusy(true);
    setError(null);
    setPreview(null);
    setInstalled(null);
    setAckRisk(false);
    try {
      const p = (await invoke("skills_marketplace_preview", { url: url.trim() })) as MarketplacePreview;
      setPreview(p);
      setSkillName(p.name ?? "");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function doInstall() {
    if (!preview || busy) return;
    if (scope !== "global" && !folderPath) {
      setError("No project folder selected for a project-scope install.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const dest = (await invoke(
        scope === "global"
          ? "global_skills_marketplace_install"
          : "skills_marketplace_install",
        scope === "global"
          ? {
              url: preview.source_url,
              skillName: skillName.trim(),
              expectedSha256: preview.sha256,
              fetchedAt: new Date().toISOString(),
            }
          : {
              workingFolderPath: folderPath!,
              url: preview.source_url,
              skillName: skillName.trim(),
              expectedSha256: preview.sha256,
              fetchedAt: new Date().toISOString(),
            },
      )) as string;
      setInstalled(dest);
      setPreview(null);
      setUrl("");
      onInstalled?.(dest);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="rounded-2xl border border-cream-200 bg-white p-4">
      <h3 className="text-[13px] font-semibold text-cream-800">Install from a marketplace</h3>
      <p className="mt-1 text-[12px] text-cream-500">
        Paste a skill&apos;s <code className="text-cream-700">SKILL.md</code> URL. Nothing is installed
        until you preview the risks and confirm — the fetch is sandboxed and the content is scanned.
      </p>

      <div className="mt-3 flex gap-2">
        <input
          aria-label="Marketplace skill URL"
          type="url"
          placeholder="https://…/SKILL.md"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && url.trim() && !busy) doPreview();
          }}
          className="flex-1 rounded-2xl border border-cream-200 bg-white px-3 py-1.5 text-[12px] text-cream-800 focus:border-teal/40 focus:outline-none"
        />
        <button
          onClick={doPreview}
          disabled={!url.trim() || busy}
          className="rounded-2xl border border-cream-200 bg-cream-50 px-3 py-1.5 text-[12px] font-semibold text-cream-800 hover:bg-cream-100 disabled:opacity-40"
        >
          {busy && !preview ? "Previewing…" : "Preview"}
        </button>
      </div>

      {error && (
        <p role="alert" className="mt-3 rounded-2xl border border-red-300 bg-red-50 px-3 py-2 text-[12px] text-red-700">
          {error}
        </p>
      )}

      {installed && (
        <p role="status" className="mt-3 rounded-2xl border border-teal/30 bg-teal/10 px-3 py-2 text-[12px] text-teal">
          Installed to <span className="font-mono">{installed}</span>
        </p>
      )}

      {preview && (
        <div className="mt-4 space-y-3">
          <div>
            <div className="flex items-center gap-2">
              <span className="text-[13px] font-semibold text-cream-800">{preview.name ?? "(unnamed skill)"}</span>
              {preview.worst && (
                <span className={`rounded-full border px-2 py-0.5 text-[11px] ${SEVERITY[preview.worst].chip}`}>
                  worst: {SEVERITY[preview.worst].label}
                </span>
              )}
            </div>
            {preview.description && <p className="mt-0.5 text-[12px] text-cream-500">{preview.description}</p>}
          </div>

          {preview.allowed_tools && (
            <div className="rounded-2xl border border-cream-200 bg-cream-50 px-3 py-2">
              <div className="text-[11px] uppercase tracking-wide text-cream-400">Requests these tools</div>
              <div className="mt-1 font-mono text-[12px] text-cream-800">{preview.allowed_tools}</div>
            </div>
          )}

          <div>
            <div className="text-[11px] uppercase tracking-wide text-cream-400">
              Risk scan — {preview.findings.length} finding{preview.findings.length === 1 ? "" : "s"}
            </div>
            {preview.findings.length === 0 ? (
              <p className="mt-1 text-[12px] text-teal">No risk patterns detected.</p>
            ) : (
              <ul className="mt-1 space-y-1">
                {preview.findings.map((f, i) => (
                  <li
                    key={`${f.code}-${i}`}
                    className="flex items-start gap-2 rounded-2xl border border-cream-200 bg-cream-50 px-2.5 py-1.5"
                  >
                    <span className={`mt-1.5 h-1.5 w-1.5 shrink-0 rounded-full ${SEVERITY[f.severity].dot}`} />
                    <div className="min-w-0">
                      <div className="text-[12px] text-cream-800">
                        <span className="font-mono text-cream-400">{f.code}</span> {f.title}
                      </div>
                      {f.evidence && (
                        <div className="truncate font-mono text-[11px] text-cream-400" title={f.evidence}>
                          {f.evidence}
                        </div>
                      )}
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </div>

          <div>
            <div className="text-[11px] uppercase tracking-wide text-cream-400">
              agentskills.io conformance
            </div>
            {preview.conformant ? (
              <p className="mt-1 text-[12px] text-teal">Spec-conformant.</p>
            ) : preview.conformance_warnings.length === 0 ? (
              <p className="mt-1 text-[12px] text-amber-700">Not spec-conformant.</p>
            ) : (
              <ul className="mt-1 space-y-1">
                {preview.conformance_warnings.map((w, i) => (
                  <li
                    key={i}
                    className="flex items-start gap-2 rounded-2xl border border-amber-300 bg-amber-50 px-2.5 py-1.5"
                  >
                    <span className="mt-1.5 h-1.5 w-1.5 shrink-0 rounded-full bg-amber-400" />
                    <div className="min-w-0">
                      <div className="text-[12px] text-amber-700">
                        <code className="font-mono text-[11px]">{w}</code>
                      </div>
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </div>

          <details className="rounded-2xl border border-cream-200 bg-cream-50">
            <summary className="cursor-pointer px-3 py-2 text-[12px] text-cream-500">Preview the skill body</summary>
            <pre className="max-h-48 overflow-auto whitespace-pre-wrap px-3 pb-3 font-mono text-[11px] text-cream-700">
              {preview.body_excerpt}
            </pre>
          </details>

          <div className="flex flex-wrap items-center gap-2">
            <label className="text-[12px] text-cream-500">Install as</label>
            <input
              aria-label="Install skill name"
              value={skillName}
              onChange={(e) => setSkillName(e.target.value)}
              placeholder="skill-name"
              className="w-44 rounded-2xl border border-cream-200 bg-white px-2.5 py-1 font-mono text-[12px] text-cream-800 focus:border-teal/40 focus:outline-none"
            />
          </div>
          {preview.name && skillName.trim().length > 0 && skillName.trim() !== preview.name && (
            <p className="mt-1 rounded-2xl border border-amber-300 bg-amber-50 px-3 py-2 text-[12px] text-amber-700">
              The install name does not match the skill&apos;s declared name ({'"'}
              {preview.name}
              {'"'}); agentskills.io expects them to match.
            </p>
          )}

          {isDanger && (
            <label className="flex items-start gap-2 rounded-2xl border border-red-300 bg-red-50 px-3 py-2 text-[12px] text-red-700">
              <input type="checkbox" checked={ackRisk} onChange={(e) => setAckRisk(e.target.checked)} className="mt-0.5" />
              <span>
                This skill triggered a <strong>Danger</strong> finding. I&apos;ve reviewed it and want to
                install it anyway.
              </span>
            </label>
          )}

          <button
            onClick={doInstall}
            disabled={!canInstall}
            className="rounded-2xl border border-teal/30 bg-teal/10 px-3 py-1.5 text-[12px] font-semibold text-teal hover:bg-teal/20 disabled:opacity-40"
          >
            {busy ? "Installing…" : "Install skill"}
          </button>
        </div>
      )}
    </section>
  );
}
