# Devboule

**Alpha** — local desktop control plane for development workspaces: projects board, AI agents, secrets in the OS vault, Oracle code memory.

Built with **Tauri + React + Rust**.

> Pre-production. APIs and UX will change. Do not treat this as a finished product.

## Requirements

- Node.js 20+
- Rust (stable), for Tauri
- macOS / Windows / Linux (desktop WebView)

## Develop

```bash
npm install
npm run dev          # frontend on http://127.0.0.1:1420
# other terminal, from src-tauri:
cargo run
# or: npm run tauri dev   # if configured
```

Secrets stay in the OS keyring. Do not put API keys in project Markdown, prompts, or logs.

## License

Devboule is licensed under Apache-2.0 — see [LICENSE](./LICENSE). Copied/adapted source
snippets are attributed in [THIRD_PARTY_NOTICES.md](./THIRD_PARTY_NOTICES.md); the full
open-source dependency inventory (npm + Rust crates) is in
[THIRD_PARTY_LICENSES.md](./THIRD_PARTY_LICENSES.md).

## Status

Public alpha preparation. Internal process docs and design handoffs are not shipped in this tree.
