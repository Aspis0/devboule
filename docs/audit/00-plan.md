# Devboule — Full App Audit Plan (bugs + security)

**Status:** active  
**Started:** 2026-07-20  
**Mode:** read-only audit (no product code changes in audit phases)  
**Owner:** security / quality pass over Tauri + Rust + React + pi-sidecar + Oracle + MCP + cloud

---

## 1. Scope

| Layer | Paths | Risk |
|--------|--------|------|
| Tauri shell | `src-tauri/` | Invoke surface, CSP, process spawn |
| Vault / unlock | `backend/vault.rs`, `auth.rs`, `state.rs` | Secrets, sensitive sessions |
| Cloud mutations | `backend/commands.rs`, CF / Scaleway | Destroy, secret rotation, billing |
| Agent runtime | `pi_sidecar`, `mini_coder*`, `agent_pty`, `cloud_*` | Shell, FS write, tool abuse |
| Consent / git | `consent_*`, `project_git`, `plan_approval` | Human-gate bypass |
| Devices / roles | `devices.rs`, `roles.rs` | Admin escalation |
| Oracle / MCP | `oracle/`, `oracle-core`, `aspis_mcp.py` | Code/secret leak via RAG |
| Design preview | `design*`, iframe / artifact protocol | XSS → invoke |
| Frontend | `src/` | UI-only guards, deep links, state |
| Sidecar | `pi-sidecar/` | Prompt injection, model/tool chain |
| Pigeon | `pigeon/` | Prompt classification routing |
| Polis | `polis/` | Secondary (path/I/O, cloud stubs) |

**Out of scope unless leakage:** `node_modules/`, `src-tauri/target/`, `oracle-data/venv/`, bench/prodbench artifacts, Polis art binaries.

---

## 2. Threat model

1. Malicious collaborator or unlocked stolen device  
2. Local malware / other process probing vault, IPC, state files  
3. Prompt-injected or compromised AI agent (coder / orchestrator / mini / skill / user MCP)  
4. Untrusted bootstrap package / marketplace skill / pi extension  
5. Over-scoped cloud tokens (provider blast radius)  
6. XSS in frontend or design/artifact iframe leading to Tauri `invoke`

**Product non-negotiables (must be verified, not only documented):**

- Secrets only in OS vault — never Markdown / Oracle / logs / prompts  
- Cloud mutations: project-scope HARD-FAIL  
- Dangerous actions: confirm-by-name + **Rust** enforce  
- “If the UI claims a dangerous capability, the backend must enforce the same rule”

---

## 3. Severity

| Sev | Meaning |
|-----|---------|
| **S0 Critical** | RCE, secret exfil, out-of-scope cloud destroy, unlock bypass |
| **S1 High** | Agent path escape, role escalation, consent bypass, XSS→invoke |
| **S2 Medium** | Info leak, wallet DoS, race, incomplete defense-in-depth |
| **S3 Low** | Hardening gap, misleading security copy, polish |
| **B0–B3** | Functional bugs (blocker → polish) |

---

## 4. Phase files (one MD per phase)

| File | Phase | Focus |
|------|-------|--------|
| [00-plan.md](./00-plan.md) | Plan | This document |
| [00-inventory.md](./00-inventory.md) | **0** | Commands, gates, spawns, network, secrets map |
| [01-auth-vault-roles.md](./01-auth-vault-roles.md) | **1** | Unlock, vault, devices, role grants |
| [02-command-surface.md](./02-command-surface.md) | **2** | Per-command validation, ungated IPC |
| [03-cloud-providers.md](./03-cloud-providers.md) | **3** | Cloudflare + Scaleway mutations |
| [04-agents-sandbox-consent.md](./04-agents-sandbox-consent.md) | **4** | Agents, PTY, seatbelt, consent, git/plan |
| [05-oracle-mcp-design.md](./05-oracle-mcp-design.md) | **5** | Oracle, MCP, skills, design/XSS |
| [06-frontend-trust.md](./06-frontend-trust.md) | **6** | FE trust boundary, deep links, CSP |
| [07-functional-bugs.md](./07-functional-bugs.md) | **7** | Reliability / e2e bug track |
| [08-supply-chain-build.md](./08-supply-chain-build.md) | **8** | Deps, capabilities, build flags |
| [FINDINGS.md](./FINDINGS.md) | Index | Master ledger of all open findings |

---

## 5. Method (every phase)

1. Inventory APIs / paths  
2. Threat cases (concrete attacks)  
3. Static trace: UI → invoke → guard → side effect  
4. Note tests that exist / missing adversarial tests  
5. Finding: id, severity, evidence, repro idea, residual risk  
6. No product code edits in audit-only mode  

---

## 6. Execution order

1. Phase 0 inventory (baseline)  
2. Phase 1–2 session gates & command surface (P0)  
3. Phase 3 cloud (P0 blast radius)  
4. Phase 4 agents / sandbox (P0/P1)  
5. Phase 5–6 Oracle/MCP/FE (P1)  
6. Phase 7 functional bugs (parallel)  
7. Phase 8 supply chain  
8. Update `FINDINGS.md` continuously  

---

## 7. Known seed issues (imported, not re-discovered)

From `docs/e2e-bugs-2026-07-09.md` and architecture docs:

- Planner / orchestrator console perceived dead (latency + thinking UX + Pigeon)  
- Packaged-app PATH vs shell PATH for `node` / tools  
- Seatbelt only on mini local-loopback backends (by design)  
- macOS still shows “Windows Hello” copy in some lock errors  

---

## 8. Conventions for findings

```
### F-XX-NNN — short title
- **Severity:** S0|S1|S2|S3|B0–B3
- **Status:** open | needs-runtime | accepted-risk | fixed (out of audit scope)
- **Phase:** N
- **Location:** path:symbol
- **Evidence:** …
- **Impact:** …
- **Repro idea:** …
- **Suggested fix direction:** … (no implementation in audit mode)
```

---

## Later artifacts

- [VERIFICATION.md](./VERIFICATION.md) — anti-hallucination pass
- [CROSS-FILE.md](./CROSS-FILE.md) — interaction audit
- [README.md](./README.md) — index
