import { useState } from "react";
import type { MarketplacePreview, RiskSeverity } from "../../types/skills";

interface Props {
  /** Canonical project working folder (where the skill installs under .claude/skills/). */
  folderPath: string;
  /** Backend invoker (passed in so the component is unit-testable without Tauri). */
  invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
  /** Called after a successful install so the parent can refresh its lists. */
  onInstalled?: (dest: string) => void;
}

const SEVERITY: Record<RiskSeverity, { label: string; chip: string; dot: string }> = {
  Danger: { label: "Danger", chip: "bg-red-500/15 text-red-300 border-red-500/30", dot: "bg-red-400" },
  Warn: { label: "Warning", chip: "bg-amber-500/15 text-amber-300 border-amber-500/30", dot: "bg-amber-400" },
  Info: { label: "Info", chip: "bg-sky-500/15 text-sky-300 border-sky-500/30", dot: "bg-sky-400" },
};

/**
 * Install an external SKILL.md from a marketplace URL — the OWNER-VETTING surface. The flow never
 * auto-installs: paste a URL → Preview (fetch is SSRF-guarded + the body is risk-scanned) → review
 * the findings → confirm. A Danger finding gates the install behind an explicit acknowledgement.
 */
export function MarketplaceInstall({ folderPath, invoke, onInstalled }: Props) {
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
    if (!preview) return;
    setBusy(true);
    setError(null);
    try {
      const dest = (await invoke("skills_marketplace_install", {
        workingFolderPath: folderPath,
        url: preview.source_url,
        skillName: skillName.trim(),
        expectedSha256: preview.sha256,
        fetchedAt: new Date().toISOString(),
      })) as string;
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
    <section className="rounded-xl border border-white/10 bg-white/[0.02] p-4">
      <h3 className="text-sm font-semibold text-white/90">Install from a marketplace</h3>
      <p className="mt-1 text-xs text-white/50">
        Paste a skill&apos;s <code className="text-white/70">SKILL.md</code> URL. Nothing is installed
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
            if (e.key === "Enter" && url.trim()) doPreview();
          }}
          className="flex-1 rounded-lg border border-white/10 bg-black/30 px-3 py-1.5 text-sm text-white/90 outline-none focus:border-white/30"
        />
        <button
          onClick={doPreview}
          disabled={!url.trim() || busy}
          className="rounded-lg border border-white/15 bg-white/5 px-3 py-1.5 text-sm text-white/90 hover:bg-white/10 disabled:opacity-40"
        >
          {busy && !preview ? "Previewing…" : "Preview"}
        </button>
      </div>

      {error && (
        <p role="alert" className="mt-3 rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-300">
          {error}
        </p>
      )}

      {installed && (
        <p role="status" className="mt-3 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-300">
          Installed to <span className="font-mono">{installed}</span>
        </p>
      )}

      {preview && (
        <div className="mt-4 space-y-3">
          <div>
            <div className="flex items-center gap-2">
              <span className="text-sm font-semibold text-white/90">{preview.name ?? "(unnamed skill)"}</span>
              {preview.worst && (
                <span className={`rounded-full border px-2 py-0.5 text-[11px] ${SEVERITY[preview.worst].chip}`}>
                  worst: {SEVERITY[preview.worst].label}
                </span>
              )}
            </div>
            {preview.description && <p className="mt-0.5 text-xs text-white/60">{preview.description}</p>}
          </div>

          {preview.allowed_tools && (
            <div className="rounded-lg border border-white/10 bg-black/20 px-3 py-2">
              <div className="text-[11px] uppercase tracking-wide text-white/40">Requests these tools</div>
              <div className="mt-1 font-mono text-xs text-white/80">{preview.allowed_tools}</div>
            </div>
          )}

          <div>
            <div className="text-[11px] uppercase tracking-wide text-white/40">
              Risk scan — {preview.findings.length} finding{preview.findings.length === 1 ? "" : "s"}
            </div>
            {preview.findings.length === 0 ? (
              <p className="mt-1 text-xs text-emerald-300/80">No risk patterns detected.</p>
            ) : (
              <ul className="mt-1 space-y-1">
                {preview.findings.map((f, i) => (
                  <li
                    key={`${f.code}-${i}`}
                    className="flex items-start gap-2 rounded-lg border border-white/10 bg-black/20 px-2.5 py-1.5"
                  >
                    <span className={`mt-1.5 h-1.5 w-1.5 shrink-0 rounded-full ${SEVERITY[f.severity].dot}`} />
                    <div className="min-w-0">
                      <div className="text-xs text-white/85">
                        <span className="font-mono text-white/50">{f.code}</span> {f.title}
                      </div>
                      {f.evidence && (
                        <div className="truncate font-mono text-[11px] text-white/45" title={f.evidence}>
                          {f.evidence}
                        </div>
                      )}
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </div>

          <details className="rounded-lg border border-white/10 bg-black/20">
            <summary className="cursor-pointer px-3 py-2 text-xs text-white/60">Preview the skill body</summary>
            <pre className="max-h-48 overflow-auto whitespace-pre-wrap px-3 pb-3 font-mono text-[11px] text-white/70">
              {preview.body_excerpt}
            </pre>
          </details>

          <div className="flex flex-wrap items-center gap-2">
            <label className="text-xs text-white/50">Install as</label>
            <input
              aria-label="Install skill name"
              value={skillName}
              onChange={(e) => setSkillName(e.target.value)}
              placeholder="skill-name"
              className="w-44 rounded-lg border border-white/10 bg-black/30 px-2.5 py-1 font-mono text-xs text-white/90 outline-none focus:border-white/30"
            />
          </div>

          {isDanger && (
            <label className="flex items-start gap-2 rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-200">
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
            className="rounded-lg border border-emerald-500/30 bg-emerald-500/15 px-3 py-1.5 text-sm text-emerald-200 hover:bg-emerald-500/25 disabled:opacity-40"
          >
            {busy ? "Installing…" : "Install skill"}
          </button>
        </div>
      )}
    </section>
  );
}
