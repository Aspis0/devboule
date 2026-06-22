# Open-source we use (credits + license compliance)

Comprehensive inventory of the third-party open-source projects, standards, libraries, tools, models,
papers, and services the **Aspis / Devboule** product builds on — what we use, the license, whether we
**adapted code** / took an **idea** / link a **dependency** / **invoke** a tool / follow a **standard**,
and where it's referenced.

**Honest summary.** Everything we **bundle or adapt as code** is **permissive** (MIT / Apache-2.0 /
BSD-3 / ISC / Zlib / MPL-file-level). We copied **no copyleft (GPL/AGPL) source**. Copyleft/GPL tools are
**invoked as subprocesses only** (never bundled → no copyleft obligation) and flagged in §10. Where a
copyleft project was useful as a concept (Smithery, AGPL) we took the **idea/API-shape only**.

> Last swept 2026-06-22 (whole-repo explorer). For a public release, generate a machine `THIRD_PARTY_
> NOTICES` (e.g. `cargo-about` + `license-checker`) to enumerate transitive crate/npm licenses.

---

## 1. Standards & protocols (specs, not code)

| Standard | Owner | License | How we use it |
|---|---|---|---|
| **SKILL.md** | agentskills.io / Linux Foundation **AAIF** | Apache-2.0 spec | Skill format (`skill_format.rs`) + the local client (`devboule-coder/src/skills.rs`); import/export-compatible |
| **AGENTS.md** | OpenAI-origin (Aug 2025) → Linux Foundation AAIF (Dec 2025) | Open convention | Always-on project context (CLAUDE.md twin); injected as the fixed prefix; recognized by goose/codex/OpenHands |
| **MCP (Model Context Protocol)** | Anthropic | Apache-2.0 spec | The Tools rail (tool servers) via `rmcp` + the user-MCP allowlist |
| **DTCG** (W3C Design Token CG, 2025.10) | W3C | Open | Design-token format the design scorer targets (P11) |
| **WCAG** | W3C | Open | a11y gate via `@axe-core/playwright` (P11) |
| **OpenAI chat-completions API** | OpenAI (de-facto) | Public | `/v1/chat/completions` interface for oMLX / Ollama / cloud backends |
| **AWS S3 Signature V4** | AWS | Open | Scaleway Object Storage signing (`hmac` + `sha2`) |

---

## 2. OSS CODE we ADAPTED (idea + parts of source — license-permitting)

| Project | Repo | License | What we took | Lives in |
|---|---|---|---|---|
| **OpenAI Codex** | `openai/codex` | Apache-2.0 | `project_doc.rs`: AGENTS/CLAUDE precedence + blank-doc skip (NOT the multi-level git-root walk) | `project_skill.rs::read_project_context` |
| **Block goose** | `block/goose` | Apache-2.0 | `skills/client.rs`+`mod.rs`: `load_skill`, `"name/rel/path"` access, the `canonical.starts_with` traversal guard, fuzzy not-found, catalog format | `devboule-coder/src/skills.rs` |
| **SkillGate** | `charliechenye/SkillGate` | MIT | `rules/script_rules.py`+`markdown_rules.py`: the **real risk-pattern ruleset** (adapted Python `re` lookaround → Rust `\b`) | `skill_vet.rs` |
| **Roo Code** | `RooCodeInc/Roo-Code` | Apache-2.0 | Marketplace UX: `RemoteConfigLoader`/`SimpleInstaller`/`MarketplaceManager` + the `type:"mode"\|"mcp"` discriminator | `MarketplaceInstall.tsx`, Discover/Tools routing |
| **Cline** | `cline/cline` | Apache-2.0 | DESIGN INFLUENCE (not verbatim): `McpHub.ts` install-confirm + command-allowlist + `{server}__…__{tool}` namespacing + 3-tier policy; "Cline-style" board arrows (Phase 17) | user-MCP allowlist design; Phase 17 plan |
| **Anthropic Skills / agentskills** | `anthropics/skills`, `agentskills/agentskills` | Apache-2.0 | SKILL.md frontmatter schema + progressive-disclosure contract + the `<available_skills>` catalog idea | `skill_format.rs`, the catalog block |

---

## 3. IDEA / INSPIRATION only (no code taken)

| Project / paper | What it is | License | What we drew | Where |
|---|---|---|---|---|
| **Smithery CLI** | MCP registry CLI | **AGPL-3.0** ⚠️ | public registry-API shape only — NO source | credits §10 |
| `gray_matter` / `gray-matter` | frontmatter parsers | MIT | decided to hand-roll a dep-free parser (offline-build safety) | — |
| **terax-ai** | agent-terminal PTY project | Apache-2.0 | reference for the `portable-pty` (ConPTY/openpty) stack | `Cargo.toml` comment |
| **Aider** | Python AI coder | Apache-2.0 | the `similar`-crate fuzzy splice = Aider's `SequenceMatcher.ratio()`; Aider-polyglot bench | `Cargo.toml` (`similar`), P15 |
| **goose** (beyond the skills code) | Rust+TS agent harness (AAIF) | Apache-2.0 | structural reference for the in-process Rust agent loop; preferred local-main-coder harness | `docs/local-main-coder-harness-design-*` |
| **OpenHands / gptme / Plandex / Continue** | agent harnesses | MIT / MIT / MIT / Apache-2.0 | evaluated as harness candidates, deprioritized (Docker/Win/no-MCP) | harness-design doc |
| **Crush** | agent harness | **FSL-1.1** ⚠️ not-OSI | explicitly EXCLUDED (license invariant) | harness-design doc |
| **`@cline/sdk`** | embeddable agent runtime | Apache-2.0 | "embed" option documented, not picked | harness-design doc |
| **macOS `sandbox-exec` (Seatbelt/SBPL)** | OS sandbox | Apple system | chosen for P5 sandboxing; hand-authored SBPL profile (over Anthropic `srt`) | `docs/p5-sandbox-impl-spec-*` |
| **Microsoft SkillOpt** + **Darwin Gödel Machine** | self-evolving skills + open-ended evolution | SkillOpt: ⚠️ license-check pending; DGM: paper | the Lab's first experiment (P18) | master-plan P18 |
| **ANTLR / CodeQL** | parser-gen / static-analysis | BSD-3 / GitHub-proprietary | considered + REJECTED (redundant / license forbids commercial) | master-plan "Deterministic Sandwich" |

---

## 4. Rust dependencies — `src-tauri/Cargo.toml`

| Crate | License | Purpose |
|---|---|---|
| `tauri`, `tauri-build`, `tauri-plugin-{dialog,notification}` | MIT/Apache-2.0 | desktop shell, IPC, webview, native dialogs/notifications |
| `serde`, `serde_json` | MIT/Apache-2.0 | serialization (boundary) |
| `reqwest` (0.12, rustls) | MIT/Apache-2.0 | HTTP (oMLX, Exa, Scaleway, Cloudflare, marketplace SSRF fetch) |
| `tokio` | MIT | async runtime |
| `thiserror` | MIT/Apache-2.0 | error types |
| `regex` | MIT/Apache-2.0 | linear-time (ReDoS-immune) scanner patterns |
| `chrono` | MIT/Apache-2.0 | timestamps |
| `keyring` (3.6) | MIT/Apache-2.0 | OS keystore (Keychain / Win Credential Store) |
| `open`, `urlencoding` | MIT | shell-open, URL encode |
| `sha2`, `hmac`, `hex`, `getrandom` | MIT/Apache-2.0 | S3 Sig V4 + install provenance hashes |
| `x25519-dalek`, `ed25519-dalek`, `subtle` | BSD-3 | E2E key-exchange / signing / constant-time compare (device pairing) |
| `aes-gcm`, `hkdf`, `zeroize`, `rand_core` | MIT/Apache-2.0 | authenticated encryption, KDF, secret-zeroing |
| `quick-xml`, `tar`, `uuid`, `fs2`, `futures-util`, `libc` | MIT/Apache-2.0 | XML, archives, UUIDs, file-locking, streams, process-group kill |
| `notify` (6) | MIT/Apache-2.0 | FS watcher (Oracle re-index trigger) |
| `sysinfo` (0.33) | MIT | RAM/CPU query (resource-aware orchestration) |
| `portable-pty` (0.9) | MIT | cross-platform PTY for in-app agent terminals (ref: terax-ai) |
| `ignore` (0.4) | MIT | ripgrep gitignore engine (file scanner / Oracle indexer) |
| `tree-sitter` (0.25) + grammars `-rust/-typescript/-python/-go/-cpp/-html/-kotlin-ng` | MIT | multi-language AST (Censor deterministic sandwich); single-ABI discipline; `kotlin-ng` chosen for ABI safety |
| `similar` (2) | MIT/Apache-2.0 | Aider-style fuzzy-match edit fallback (`TextDiff::ratio()`) |
| `windows` (0.58), `webview2-com`, `windows`@0.61.3 (renamed) | MIT/Apache-2.0 | Win32/WinRT + WebView2 screenshot (Windows, cfg-gated) |
| `objc2`, `objc2-foundation`, `objc2-local-authentication`, `block2` | MIT | macOS Foundation + Touch ID (cfg-gated) |

MPL-2.0 (file-level weak copyleft, commercial-safe if unmodified): `cssparser`, `selectors`, `dtoa-short`, `option-ext` (transitive).

---

## 5. Rust dependencies — `devboule-coder/Cargo.toml`

| Crate | License | Purpose |
|---|---|---|
| `ratatui` (0.29), `crossterm` (0.28) | MIT | TUI + terminal backend for the local main-coder harness |
| `tui-textarea`, `tui-markdown` (0.3.6 pin), `throbber-widgets-tui` | MIT / MIT-Apache / Zlib | REPL input, markdown render, spinner |
| `rmcp` (1) | Apache-2.0 | **official MCP Rust SDK** (client + child-process transport) |
| `reqwest`, `tokio`, `futures`, `serde`/`serde_json`, `regex`, `async-trait` | MIT/Apache-2.0 | HTTP, async, action protocol, traits |
| `ignore`, `globset` | MIT | gitignore-aware FS walk + glob for the `read/grep/glob` tools |
| `tempfile` (dev) | MIT/Apache-2.0 | FS-confinement tests |

---

## 6. Frontend dependencies — `package.json`

| Package | License | Purpose |
|---|---|---|
| `react`, `react-dom` | MIT | UI framework |
| `@tauri-apps/api` + `plugin-{dialog,notification}` | MIT/Apache-2.0 | IPC + native dialogs/notifications |
| `@xterm/xterm` + `addon-fit` | MIT | in-app terminal display |
| `pixi.js` + `pixi-filters` + `pixi-viewport` | MIT | WebGL renderer (Polis map + Phase-17 Kanban) |
| `d3-force` | ISC | force-layout (Phase-17 dependency arrows) |
| `perfect-arrows` | MIT | Cline-style connector geometry (Kanban) |
| `gsap` | **GreenSock Standard** ⚠️ not-OSI | animations (verify for commercial) |
| `dompurify` | Apache-2.0/MIT | HTML sanitization (XSS) |
| `lucide-react` | ISC | icons |
| `zustand` | MIT | state management |
| `@fontsource-variable/{instrument-sans,source-serif-4}` | OFL-1.1 | UI fonts |
| **dev:** `vite`, `vitest`, `typescript`, `@vitejs/plugin-react`, `tailwindcss`, `postcss`, `autoprefixer`, `esbuild`, `jsdom` | MIT (typescript: Apache-2.0) | build, test, types, styling |

---

## 7. Gate tools (INVOKED as subprocesses — never bundled)

Per the master-plan **licensing invariant**: invoked via subprocess ⇒ no copyleft obligation; **GPL/LGPL tools MUST NEVER be bundled** in the installer. ⚠️ = copyleft.

| Tool | License | Used for |
|---|---|---|
| `oxlint`, `eslint-plugin-tailwindcss`, `prettier`/`oxfmt`, `stylelint`, `knip`, `actionlint` | MIT | JS/TS/CSS lint + format + dead-code + CI |
| `pyright`, `ruff`, `vulture` | MIT | Python types / lint / dead-code |
| `pip-audit`, `bandit` | Apache-2.0 | Python vuln / security |
| `cargo fmt`/`clippy`, `cargo-deny`, `cargo-mutants`, `cargo-fuzz` | MIT/Apache-2.0 | Rust format/lint/dep-audit/mutation/fuzz |
| `gofmt`/`go vet` | BSD-3 | Go format/vet |
| `ktlint` | MIT | Kotlin lint |
| `sqlfluff`, `zizmor`, `schemathesis` | MIT | SQL lint / GH-Actions audit / API property-test |
| `Joern (CPG)`, `osv-scanner`/`trivy`, `checkov`/`tfsec`, `Playwright`(`@axe-core/playwright`) | Apache-2.0 | interprocedural taint / vuln+SBOM / IaC / E2E+a11y |
| `gitleaks` | MIT (binary) | secret scanning |
| `npm audit` | Artistic-2.0 | JS dep vuln |
| `semgrep` CLI | **LGPL-2.1** ⚠️ | multi-lang pattern analysis (invoke-fine) |
| `shellcheck`, `hadolint`, `yamllint`, `cppcheck` | **GPL-3.0** ⚠️ | shell / Dockerfile / YAML / C++ lint — INVOKE ONLY, never bundle |
| `tidy` (html-tidy) | W3C/HTMLTIDY ⚠️ not-OSI | HTML validation |

Rejected by license: **CodeQL** (GitHub-proprietary, forbids private/commercial → use Joern instead).

---

## 8. ML — models, methods, datasets, benchmarks (ideas; weights NOT redistributed)

**Inference models** (run locally / via API, not shipped): Qwen3-Embedding-0.6B, Qwen3.6-35B-A3B (MoE), Qwen3.6-27B, Qwen3-8B, Qwen3.5-9B — *Apache-2.0*; Gemma 4 12B/27B — *Gemma Terms*; Seed-Coder-8B, DeepSeek-R1-Distill-Qwen-7B — *MIT*; DeepSeek-R1 (API, distill-permitted); Nemotron-3-Nano-4B — *NVIDIA Community*; Devstral-Small-2-24B — *Mistral*; Granite-4.0 H-Tiny — *Apache-2.0*.

**Training methods studied/applied** (research papers, idea-level): ORPO (P13 nightly), GRPO (DeepSeek-R1 2501.12948), RLVR, QLoRA, LoRA/QAT/RSLoRA/CURLoRA, **WiSE-FT** (2109.01903) + LM-Cocktail, **LP-FT** (Kumar 2202.10054), MoLE (2506.18923), s1 budget-forcing (2501.19393), LIMO (2502.03387), DLCoT (2503.16385), Light-R1 (2503.10460), Hermes-4 (2508.18255), OpenThoughts3 (2506.04178), Reasoning-Trace-Collapse (2605.21127), REDI (2505.24850), Learning-from-Mistakes (2601.04992), DFR (2204.02937), Temperature-scaling (1706.04599), Semantic-Entropy-Probes (2406.15927), SAPLMA (2304.13734), Orgad "LLMs Know More" (2410.02707), AutoProbe (2510.02934), Geometry-of-Truth (2310.06824), SelectiveNet (1901.09192), AWI survey (2312.00324), ShortGPT (referenced, EXCLUDED from P12). Recipes: Open-R1 / Bespoke-Stratos / Sky-T1 / OpenThinker / VibeThinker-3B.

**Probing repos referenced** (MIT): representation-engineering, honest_llama, TransformerLens, baukit, semantic-entropy-probes, netcal.

**Datasets** (training): `coseal/CodeUltraFeedback_binarized` (Apache-2.0), CVEfixes-Rust (CC-BY-4.0), Microsoft CodeReviewer (Apache-2.0), RustSec advisories. **Benchmarks** (eval refs): HumanEval (MIT), Aider-polyglot (MIT), LiveCodeBench, SWE-bench Verified (MIT), Design2Code (Stanford SALT), DesignBench/UI-Bench/WebGen-Bench, ProdCodeBench (2604.01527, inspired our prodbench).

---

## 9. External services integrated

| Service | Provider | Use |
|---|---|---|
| **Scaleway** | Scaleway SAS (FR) | primary cloud (compute / object storage / serverless / AI); own Rust client + S3 Sig V4 |
| **Cloudflare** | Cloudflare | secondary cloud (Workers/R2/DNS/KV/D1) |
| **Exa** | Exa AI | web search for the agent loop |
| **Infomaniak** | Infomaniak SA (CH) | ZDR-compliant AI inference alternative |
| **HuggingFace Hub** | HuggingFace | model download/cache |
| **RunPod** | RunPod | cloud GPU (H100) for training runs |
| **OpenRouter** | OpenRouter | training-data generation (teacher models) |

---

## 10. License-compliance & copyleft flags (action items for a commercial release)

| # | Item | License | Action |
|---|---|---|---|
| 1 | `shellcheck`, `hadolint`, `yamllint`, `cppcheck` | **GPL-3.0** | invoke-only; **MUST NOT bundle** in the installer |
| 2 | `semgrep` CLI | **LGPL-2.1** | invoke fine; do not relink/bundle |
| 3 | **Smithery CLI** | **AGPL-3.0** | idea/API-shape only — ZERO source copied (keep it that way) |
| 4 | `gsap` | GreenSock Standard | NOT OSI — verify terms for commercial SaaS/OEM |
| 5 | `html-tidy` | W3C/HTMLTIDY | permissive but NOT OSI — flag if strict-OSI bar required |
| 6 | **Crush** | FSL-1.1 | NOT OSI — deliberately excluded from the product |
| 7 | **CodeQL** | GitHub-proprietary | forbids private/commercial — excluded (use Joern) |
| 8 | **microsoft/SkillOpt** | unknown | verify license BEFORE adapting any code (P18 Lab) |
| 9 | MPL-2.0 transitive crates (`cssparser`, `selectors`, `dtoa-short`, `option-ext`) | MPL-2.0 | file-level weak copyleft; safe if those files are unmodified |
| 10 | Gemma / Nemotron / Mistral model terms | vendor | not redistributed (inference only); check ToS for any hosted commercial use |

Everything **bundled or compiled into** the app is MIT / Apache-2.0 / BSD-3 / ISC / Zlib / OFL — no copyleft is linked. For a shipped build, run a license enumerator (`cargo-about`, `license-checker`) and pin this list to the generated report.
