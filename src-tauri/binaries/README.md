# Tauri externalBin — `devboule-mcp`

This directory holds the staged **Devboule app-tools MCP** sidecar for packaging.

## Layout

Tauri expects a **target-triple-suffixed** binary:

```text
binaries/devboule-mcp-<triple>       # e.g. devboule-mcp-aarch64-apple-darwin
binaries/devboule-mcp                # optional local un-suffixed copy (dev resolve)
```

Configured in `tauri.conf.json`:

```json
"externalBin": ["binaries/devboule-mcp"]
```

At package time Tauri copies the matching triple file next to the app executable
as `devboule-mcp` (no suffix). Runtime resolution:

1. `DEVBOULE_MCP_BIN`
2. path recorded by `set_bundled_mcp_bin` / `discover_and_record_bundled_mcp_bin`
3. siblings of the app executable / `Resources/`
4. (debug) cargo target tree + this staged un-suffixed copy
5. `PATH`

## Stage

From repo root (also runs automatically in `beforeBuildCommand`):

```bash
npm run mcp:stage
# or
bash scripts/stage-devboule-mcp.sh
```

## Soak

```bash
npm run mcp:soak
```

Do **not** commit the binary artifacts (see root `.gitignore`).
