# Phase 5 — Oracle, MCP, skills, design / XSS

**Status:** partial (static)  
**Date:** 2026-07-20  

---

## 1. Scope

| Area | Paths |
|------|-------|
| Oracle commands | `oracle/commands.rs` |
| Oracle service / resident server | `backend/oracle_service.rs`, oracle-core |
| MCP app tools | `oracle/server/aspis_mcp.py` |
| User MCP | `backend/user_mcp_config.rs`, FE consent UI |
| Skills | `project_skill.rs`, `skill_marketplace.rs`, `skill_vet.rs`, `global_skills.rs` |
| Design sanitize | `src/components/design/sanitize.ts` |
| Design canvas / interactive | `DesignCanvas.tsx`, `interactivePipeline.ts` |
| Artifacts | `src/components/projects/artifact/*` |

---

## 2. Oracle controls (verified)

### Auth helpers

| Helper | Behavior |
|--------|----------|
| `require_graph_auth` | `ensure_unlocked` |
| `require_graph_auth_and_enabled` | unlock + Oracle enabled flag |
| `require_oracle_auth` | unlock + enabled → `OracleError` |

### Operator vs agent

- `ask_oracle` → HTTP POST `/ask` (operator full corpus) with LLM config  
- Agent path documented as `/ask-bounded` with `allowed_file_ids` fail-closed when absent  
- Index jobs use graph auth + enabled  

### Setup exceptions

- `get_oracle_runtime_setup` / `install_oracle_runtime` intentionally work when Oracle disabled (repair path) — **confirm still require unlock** (helper: setup may differ; inventory marked no gate in body — **needs re-read**)

---

## 3. Design / XSS controls (verified)

### Static mode

- DOMPurify chokepoint in `sanitize.ts` with extra hooks: strip `on*`, URI clamp, CSS neutralize  
- `DesignCanvas` / `NodeContent` use `dangerouslySetInnerHTML` **after** sanitize pipeline  
- Tests: `sanitize.test.ts`

### Interactive / artifact mode

- **No DOMPurify** (scripts required)  
- Boundary: sandboxed iframe  
  - Artifact: `sandbox="allow-scripts"` **without** `allow-same-origin` (opaque origin)  
  - Design preview page: empty `sandbox=""` (most restrictive)  
- Remote URL neutralization in interactive pipeline  
- `frame-src` allows `artifact:` + `http://artifact.localhost`

### Plan markdown

- Explicitly text nodes only — no `dangerouslySetInnerHTML` (`planMarkdown.ts`, `MarkdownRenderer.tsx`)

---

## 4. User MCP

- Commands: list/add/remove/set_enabled  
- `ensure_unlocked` count = 5 on module (commands gated)  
- FE: `UserMcpConsentDialog`  
- **Next:** validate command/args cannot inject shell when spawning MCP servers  

---

## 5. Skills

| Piece | Gate notes |
|-------|------------|
| `global_skills_*` | `ensure_unlocked` present |
| `skills_marketplace_install` | unlock (project_skill) |
| `skill_vet.rs` | pure vet logic, no unlock (library) |
| Catalogs | some read-only ungated (Phase 2) |

---

## 6. Findings

### F-05-001 — Dual design security models are coherent (positive)

- **Severity:** n/a  
- **Status:** noted  
- **Evidence:** static = DOMPurify; interactive = opaque iframe sandbox; preview = empty sandbox.  

### F-05-002 — Interactive artifact runs attacker-influenced JS in sandbox

- **Severity:** S2  
- **Status:** residual risk  
- **Location:** `ArtifactView.tsx` `sandbox="allow-scripts"`  
- **Impact:** JS runs but opaque origin should block parent DOM/cookie access. Bridge/postMessage must not trust guest.  
- **Evidence:** comments warn origin `"null"` is forgeable — handlers must not use origin alone.  
- **Next:** audit all `message` event listeners for artifact/design bridges.

### F-05-003 — `dangerouslySetInnerHTML` remains a single-point failure if sanitize skipped

- **Severity:** S1 if regression  
- **Status:** open (guard with tests)  
- **Location:** `NodeContent.tsx`, `DesignCanvas.tsx`  
- **Evidence:** comments claim parent passes raw and child/chokepoint sanitizes — verify no bypass path writes unsanitized HTML into nodes.  
- **Next:** trace all writers to node HTML fields.

### F-05-004 — Oracle skips known secret basenames; residual content secrets remain

- **Severity:** S2  
- **Status:** open (residual)  
- **Location:** `oracle-core/src/ingest/collect.rs` — `basename_is_secret`, `is_sensitive_relative_path`  
- **Evidence:** `.env` / `.env.*`, `id_rsa*`, `.pem`/`.key`/`.pfx`/`.p12`/`.dev.vars`, and names containing secret-ish words for data extensions are treated sensitive and skipped at collect (`is_sensitive_relative_path` → `return false` keep). Workspace hygiene mirrors similar names in `workspace.rs::is_sensitive_file_name`.  
- **Impact:** Hardcoded secret *files* with non-matching names (e.g. `prod_config.json` with raw API keys, or secrets inside `.rs`/`.ts` source) can still be indexed and returned by operator `/ask`.  
- **Residual:** content-based secret detection is basename-oriented, not full secret scanning of file bodies at index time.

### F-05-005 — Operator `/ask` is full-corpus (by design)

- **Severity:** S2  
- **Status:** accepted-risk  
- **Evidence:** comments explicitly avoid `/ask-bounded` for operator.  
- **Impact:** unlocked operator queries entire index including multi-project data if shared index root.  

### F-05-006 — Oracle setup/install requires unlock (positive)

- **Severity:** n/a  
- **Status:** noted  
- **Location:** `oracle/commands.rs::get_oracle_runtime_setup`, `install_oracle_runtime`  
- **Evidence:** both call `require_graph_auth` → `ensure_unlocked` before blocking setup/install work.  
- **Impact:** Locked webview cannot start Oracle install.  

### F-05-007 — User MCP: freeform command; project allowlist; global allowlist-exempt

- **Severity:** S1  
- **Status:** open  
- **Location:** `backend/user_mcp_config.rs`  
- **Evidence:**  
  - `user_mcp_add` requires `ensure_unlocked` + `validate_server`.  
  - `validate_command` / `validate_args` only reject **control characters / newlines** — not shell metacharacters, absolute paths, or interpreter choice.  
  - Transport limited to `stdio`.  
  - Name charset restricted (alnum/`-`/`_`) against config-key injection.  
  - **Project-scoped** servers filtered by exact-match `allowed_commands` allowlist at merge; empty/malformed allowlist fails closed (reject project servers).  
  - **Global-scoped** servers are **allowlist-exempt** (tests: `global_server_is_exempt_from_allowlist`).  
- **Impact:** Unlocked operator can register a global MCP whose `command` is any executable (e.g. shell) and args arbitrary → agent-context code execution by design of MCP, with weaker friction than project scope.  
- **Suggested fix direction:** require allowlist (or consent fingerprint) for global too; optional binary path allowlist.

### F-05-008 — Skill marketplace install is supply chain

- **Severity:** S2  
- **Status:** open  
- **Location:** marketplace install + `skill_vet`  
- **Impact:** Malicious skill content executed in agent context after install.  
- **Next:** vet rules completeness; whether network install requires confirm.

### F-05-009 — aspis_mcp role enforcement (WEAKENED)

- **Severity:** S3  
- **Status:** weakened (pass 6) — `require_registered_role` + ALLOWED_TOOLS confirmed; residual = every tool body  
- **Location:** `oracle/server/aspis_mcp.py`, `role_rules.json`  
- **Impact:** Verifier/orchestrator might get write tools if role rules wrong.  

---

## 7. Phase 5 checklist

- [x] Oracle auth helpers  
- [x] Design sanitize + iframe model  
- [x] Artifact sandbox attributes  
- [ ] Index ignore list for secrets  
- [ ] postMessage bridge audit  
- [ ] User MCP spawn path  
- [ ] skill_vet rules  
- [ ] aspis_mcp role_rules vs tools  

---

## 8. Priority

1. Index secret ignore + F-05-004  
2. postMessage bridges F-05-002  
3. User MCP F-05-007  
4. MCP role rules F-05-009

---

## 9. Second-pass deep findings (2026-07-20)

### F-05-010 — Artifact postMessage uses frame source identity (positive)

- **Severity:** n/a  
- **Status:** noted  
- **Location:** `src/components/projects/artifact/ArtifactView.tsx` + `artifactProtocol.ts`  
- **Evidence:** Handler rejects unless `isFromFrame(event.source, iframe)`; comments explicitly forbid trusting `event.origin` (`"null"` forgeable). Messages only resize/ready/error — no `invoke`.  
- **Impact:** Solid pattern for sandboxed guest → parent bridge.

### F-05-011 — Bootstrap package unpack: signature first, fresh dir, path components sanitized (positive)

- **Severity:** n/a  
- **Status:** noted  
- **Location:** `workspace.rs::decrypt_bootstrap_package`, `safe_unpack_tar`, `safe_tar_relative_path`  
- **Evidence:** Ed25519 over header before payload decrypt; recipient fingerprint must match device; `output_dir` must not already exist; tar paths reject `..`/absolute; non-files skipped; size caps; manifest hash verify fail-closed with dir delete.  
- **Impact:** Strong bootstrap crypto/path story. Residual: no post-join symlink-escape check inside extract (mitigated by fresh empty `output_dir`).

### F-05-012 — aspis_mcp role×tool matrix (WEAKENED)

- **Severity:** S3  
- **Status:** weakened (pass 6) — full matrix in `matrix-role-tools.md`; residual = per-handler depth  
- **Location:** `oracle/server/aspis_mcp.py`, `role_rules.json`  
- **Evidence:** Not fully traced agent-vs-operator tool allowlists in second pass.  
- **Impact:** Unknown whether verifier can obtain write tools via MCP misconfig.

---

## 10. Role SSOT cross-check (pass 3)

Full tool lists: **[matrix-role-tools.md](./matrix-role-tools.md)**.

### F-05-020 — Single SSOT role_rules.json (positive)

- **Severity:** n/a  
- **Status:** noted  
- **Evidence:** JSON `$comment` states consumed by aspis_mcp.py + agents.rs include_str + projects launchPrompt — no hand-synced copies.

### F-05-021 — DOMPurify advisory moderate (cross-ref F-08-023)

- **Severity:** S2  
- **Status:** open  
- **Location:** npm `dompurify` used by `src/components/design/sanitize.ts`  
- **Evidence:** npm audit moderate — Trusted Types / ALLOWED_ATTR config pollution.

---

## Truth-check (pass 6)

F-05-006 install gated **CONFIRMED**. F-05-007 MCP freeform **CONFIRMED**. F-05-004 .env skip + residual **CONFIRMED**. F-05-009/012 **WEAKENED**. See [VERIFICATION.md](./VERIFICATION.md).
