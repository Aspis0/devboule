# Devboule

**Alpha** — local desktop control plane for development workspaces: projects board, AI agents, secrets in the OS vault, Oracle code memory.

Built with **Tauri + React + Rust**.

> Pre-production. APIs and UX will change. Do not treat this as a finished product.

## AI disclosure

Devboule was built with heavy AI assistance. Claude (Anthropic) wrote and hostile-reviewed large
parts of this codebase, while a human led the ideas, the architecture decisions, the testing, and
the debugging. We say this plainly because it genuinely shaped how the project was built, not only
how quickly. If shipping and running AI-written code is something you're not comfortable with, this
project isn't for you.

And none of it would exist without the open-source work it stands on — above all Tauri, React, and
the wider Rust ecosystem, much of it written by hand by the people who came before. The full
inventory and attributions are in [THIRD_PARTY_NOTICES.md](./THIRD_PARTY_NOTICES.md).

## Requirements

- Node.js 20+
- Rust (stable), for Tauri

### Platforms

- **macOS** — primary target, actively developed and tested.
- **Windows** — supported; being tested shortly.
- **Linux** — should build on the desktop WebView, but is not tested yet (no near-term plans).

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

Devboule is licensed under Apache-2.0 — see [LICENSE](./LICENSE). All third-party open source
(adapted/vendored code plus the full npm + Rust dependency inventory) is attributed in
[THIRD_PARTY_NOTICES.md](./THIRD_PARTY_NOTICES.md).

## Status

Public alpha preparation. Internal process docs and design handoffs are not shipped in this tree.
