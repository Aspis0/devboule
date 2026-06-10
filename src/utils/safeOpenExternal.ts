import { invokeBackendCommand, isTauriRuntime } from "../context/AppContext";

const allowedHosts = new Set([
  "aspis-bio.com",
  "console.nebius.ai",
  "console.scaleway.com",
  "dash.cloudflare.com",
  "developers.cloudflare.com",
  "docs.aspis-bio.com",
  "github.com",
  "manager.infomaniak.com",
  "www.scaleway.com",
]);

export async function safeOpenExternal(url: string): Promise<void> {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    throw new Error("External link is invalid.");
  }
  if (
    parsed.protocol !== "https:" ||
    parsed.username ||
    parsed.password ||
    !allowedHosts.has(parsed.hostname)
  ) {
    throw new Error("External link is not in the allowlist.");
  }

  try {
    await invokeBackendCommand<void>("open_external_url", { url: parsed.toString() });
  } catch {
    if (isTauriRuntime()) throw new Error("External link was blocked by the desktop app.");
    window.open(parsed.toString(), "_blank", "noopener,noreferrer");
  }
}
