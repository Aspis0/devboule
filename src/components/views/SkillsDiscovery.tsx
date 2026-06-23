import { useEffect, useState } from "react";
import type { LibraryCatalogEntry, FeaturedMarketplace } from "../../types/skills";

interface Props {
  folderPath: string;
  invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
  onInstalled?: (dest: string) => void;
  /** Optional filter applied to the bundled-skill + featured lists (matches name/description). */
  query?: string;
}

/**
 * Skills discovery surface: the bundled (in-binary) library skills the app can install in one click,
 * plus the featured open-source marketplaces to browse. `invoke` is a prop so the component is
 * unit-testable without Tauri (mirrors MarketplaceInstall).
 */
export function SkillsDiscovery({ folderPath, invoke, onInstalled, query = "" }: Props) {
  const [library, setLibrary] = useState<LibraryCatalogEntry[]>([]);
  const [featured, setFeatured] = useState<FeaturedMarketplace[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [installed, setInstalled] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const [catalog, markets] = await Promise.all([
          invoke("skills_library_catalog"),
          invoke("skills_featured_marketplaces"),
        ]);
        if (!alive) return;
        setLibrary(Array.isArray(catalog) ? (catalog as LibraryCatalogEntry[]) : []);
        setFeatured(Array.isArray(markets) ? (markets as FeaturedMarketplace[]) : []);
      } catch (e) {
        if (alive) setError(String(e));
      }
    })();
    return () => {
      alive = false;
    };
  }, [invoke]);

  // Clear the last-install / error banners when the project changes, so a prior project's status
  // never lingers on screen for a different folder.
  useEffect(() => {
    setInstalled(null);
    setError(null);
  }, [folderPath]);

  async function install(name: string) {
    if (busy) return;
    setBusy(name);
    setError(null);
    setInstalled(null);
    try {
      const dest = (await invoke("skills_install_bundled_library", {
        workingFolderPath: folderPath,
        skillName: name,
      })) as string;
      setInstalled(dest);
      onInstalled?.(dest);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  const q = query.trim().toLowerCase();
  const shownLibrary = q
    ? library.filter(
        (s) => s.name.toLowerCase().includes(q) || s.description.toLowerCase().includes(q),
      )
    : library;
  const shownFeatured = q
    ? featured.filter(
        (m) => m.name.toLowerCase().includes(q) || m.description.toLowerCase().includes(q),
      )
    : featured;

  return (
    <section className="rounded-2xl border border-cream-200 bg-white p-4 space-y-4">
      <div>
        <h3 className="text-[13px] font-semibold text-cream-800">Bundled skills</h3>
        <p className="mt-0.5 text-[12px] text-cream-500">
          Ready-to-use, agentskills.io-conformant skills shipped with devboule. One click installs into
          this project&apos;s <code className="text-cream-700">.claude/skills/</code>.
        </p>
      </div>

      {error && (
        <div role="alert" className="rounded-2xl border border-red-300 bg-red-50 px-3 py-2 text-[12px] text-red-700">
          {error}
        </div>
      )}

      {installed && (
        <div role="status" className="rounded-2xl border border-teal/30 bg-teal/10 px-3 py-2 text-[12px] text-teal">
          Installed to <span className="font-mono">{installed}</span>
        </div>
      )}

      <ul className="space-y-2">
        {shownLibrary.map((s) => (
          <li
            key={s.name}
            className="flex items-center justify-between rounded-2xl border border-cream-200 bg-cream-50 px-3 py-2"
          >
            <div className="min-w-0 flex-1">
              <div className="font-mono font-semibold text-cream-800">{s.name}</div>
              <div className="mt-0.5 text-[12px] text-cream-500">{s.description}</div>
            </div>
            <button
              data-skill={s.name}
              disabled={busy !== null}
              onClick={() => install(s.name)}
              className="ml-3 flex-shrink-0 rounded-2xl border border-teal/30 bg-teal/10 px-3 py-1 text-[12px] font-semibold text-teal hover:bg-teal/20 disabled:opacity-40"
            >
              {busy === s.name ? "Installing…" : "Install"}
            </button>
          </li>
        ))}
      </ul>

      <hr className="border-cream-200" />

      <div>
        <h4 className="text-[13px] font-semibold text-cream-800">Featured open-source marketplaces</h4>
        <ul className="mt-2 space-y-2">
          {shownFeatured.map((m) => (
            <li
              key={m.name}
              className="flex items-start justify-between gap-3 rounded-2xl border border-cream-200 bg-cream-50 px-3 py-2"
            >
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-semibold text-cream-800">{m.name}</span>
                  <span className="rounded-full border border-cream-200 px-2 py-0.5 text-[11px] text-cream-600">
                    {m.license}
                  </span>
                </div>
                <div className="mt-0.5 text-[12px] text-cream-500">{m.description}</div>
              </div>
              <span className="flex-shrink-0 font-mono text-[11px] text-cream-400">{m.url}</span>
            </li>
          ))}
        </ul>
      </div>
    </section>
  );
}
