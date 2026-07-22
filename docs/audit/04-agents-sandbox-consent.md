# Phase 4 — Agents, sandbox, consent, git/plan

**Status:** partial (static)  
**Date:** 2026-07-20  
**Depends on:** Phase 0–2  

---

## 1. Scope

| Area | Paths |
|------|-------|
| Launch / roles | `projects.rs`, `agent_role.rs`, `agent_spawn.rs` |
| PTY | `agent_pty.rs` |
| Cloud duplex | `cloud_duplex.rs`, `cloud_claude_config.rs` |
| pi-sidecar | `pi_sidecar.rs`, `pi-sidecar/sidecar.mjs` |
| Mini coder | `mini_coder_executor.rs`, `mini_command_build.rs`, `mini_edit_apply.rs` |
| Agentic tools | `agentic_tools.rs` |
| Sandbox | `sandbox/`, Seatbelt P5 |
| Consent | `consent_hook.rs`, `consent_bridge.rs` |
| Git / plan | `project_git.rs`, `plan_approval.rs` |
| Skills | `project_skill.rs`, `skill_vet.rs`, `global_skills.rs` |

---

## 2. Controls verified (samples)

### Agent launch

- `prepare_or_launch_project_agent`: **`ensure_unlocked` first**  
- Role canonicalization via `effective_launch_role` / `canonicalize_launch_role`  
- Design handoff path validated against project root before launch (comments: symlink-escape rejection)

### Agent PTY write

- `agent_pty_write`: **`ensure_unlocked`**, `validate_agent_id`, max write size  
- No cross-user multi-tenant model (single desktop user)

### Path confinement (`agentic_tools::safe_rel_path`)

- Rejects absolute paths, drive letters (`:`), `..` components  
- Normalizes `\` → `/`  
- Heavy symlink/canonicalize usage elsewhere in file (`symlink` mentions ~53)

### Mini Seatbelt (P5)

- Scope: **local-loopback** oMLX/ollama/AppleFm only  
- Codex / non-loopback / cloud: **no sandbox-exec** (documented intentional)  
- Profile: deny default; broad read; write only tmp/scratch; network outbound localhost only  
- Real `sandbox-exec` acceptance tests exist in mini tests

### Consent hook

- Fail-closed deny on error/timeout  
- Claude PreToolUse hook JSON shape tested  

### Cloud Claude settings

- Net disabled → `permissions.deny` includes `Bash(curl *)`, `WebFetch`, etc.  
- Missing helper → deny rules still emitted  

### Plan / git push approve

- `decide_plan_request` / git approve paths use **`ensure_unlocked`**  
- Double-decide protected under state lock (plan)

---

## 3. Threat cases

| # | Attack | Static verdict |
|---|--------|----------------|
| T1 | Agent `read` `../../.ssh/id_rsa` via rel path | `safe_rel_path` rejects `..` |
| T2 | Symlink inside project → outside | Needs `canonicalize` + strip_prefix on resolve — present in places; **full tool surface audit incomplete** |
| T3 | `run` tool shell chaining | Language allowlist + parse_run_command claims no shell chaining — needs adversarial suite |
| T4 | Consent timeout | Fail-closed deny documented + code path |
| T5 | Unattended sandbox mode | Claude `bypassPermissions` but hook still deny-without-prompt — **policy-sensitive** |
| T6 | Prompt injection “approve yourself” | Human gates file-based; agent should not forge approvals without UI — needs protocol review |
| T7 | Steer while locked | **Fails control** — F-02-001/002/003 |
| T8 | Skill marketplace malicious skill | `skill_vet` exists; depth not fully audited |
| T9 | Cloud duplex env leaks vault tokens | Env map injected into child — **expected for cloud CLIs**; ensure only intended keys |

---

## 4. Findings

### F-04-001 — Seatbelt does not cover Claude/Codex/pi main agents

- **Severity:** S2 (accepted design) / residual risk  
- **Status:** accepted-risk (documented in P5 spec)  
- **Evidence:** P5 applies only local-loopback mini backends; tests assert codex spawns plain `/bin/sh`.  
- **Impact:** Main coding agents run with user privileges; confinement is policy/hooks/settings, not OS sandbox.  
- **Mitigations in product:** consent hooks, deny lists, project root, plan mode defaults for orchestrator duplex.

### F-04-002 — Agent control commands missing unlock (cross-ref)

- **Severity:** S1  
- **Status:** open  
- **See:** F-02-001, F-02-002, F-02-003  

### F-04-003 — `run` tool allows full project build toolchains

- **Severity:** S2 (by design)  
- **Status:** accepted-risk with monitoring  
- **Location:** `agentic_tools.rs` `RUN_PROGRAMS` (cargo, npm, make, gradle, …)  
- **Impact:** Agent can execute project code (intended). Malicious `package.json` scripts = code exec when agent runs tests.  
- **Mitigation:** user-owned repo trust model; optional future: stricter allowlist per project.

### F-04-004 — Unattended + bypassPermissions is high trust

- **Severity:** S2  
- **Status:** open (product policy)  
- **Location:** `claude_permission_mode` → Unattended → `bypassPermissions`  
- **Evidence:** comments: hook still answers deny without prompting for some classes; net deny rules separate.  
- **Impact:** Misconfigured project may auto-accept dangerous tool classes if hook coverage incomplete.  
- **Next:** map exact PreToolUse coverage vs bypassPermissions behavior.

### F-04-005 — Consent fail-closed (positive)

- **Severity:** n/a  
- **Status:** noted  
- **Location:** `consent_hook.rs`  

### F-04-006 — Git push / plan human gates exist and are unlock-gated (positive)

- **Severity:** n/a  
- **Status:** noted  
- **Next:** protocol spoof (agent writing approval files directly) residual review.

### F-04-007 — Pi extension install = remote code into agent toolchain

- **Severity:** S1  
- **Status:** open  
- **See:** F-02-005  
- **Additional:** even with unlock, installing arbitrary npm/GitHub extension is supply chain RCE in agent context.

### F-04-008 — Packaged-app PATH for node/sidecar

- **Severity:** B1 / S2 reliability + security-adjacent  
- **Status:** open (seed from e2e)  
- **Evidence:** historical e2e: GUI apps lack Homebrew PATH → sidecar spawn issues.  
- **Impact:** Failed spawns, silent degradation, or unexpected binary resolution if PATH is polluted.  
- **Next:** confirm explicit path resolution for `node` / sidecar (provider_detect patterns).

### F-04-009 — Mini edit apply path safety (SUPERSEDED)

- **Severity:** n/a (superseded)  
- **Status:** **superseded by F-04-011** (pass 6 truth-check)  
- **Location:** `mini_edit_apply.rs` (canonicalize/symlink mentions present)  
- **Correction:** PASS1 already canonicalize+allowlist+escape-checks; residual is PASS2 TOCTOU only (F-04-011 **CONFIRMED**).

### F-04-010 — Cloud duplex inherits explicit env map

- **Severity:** S2  
- **Status:** needs-inventory  
- **Location:** `cloud_duplex` spawn `for (k,v) in envs { cmd.env }`  
- **Impact:** If env map ever includes broader secrets than intended for role, child process leaks via agent tools.  
- **Next:** list exact keys per role (orchestrator vs coder).

---

## 5. Phase 4 checklist

- [x] Launch unlock  
- [x] PTY write unlock  
- [x] safe_rel_path semantics  
- [x] Seatbelt scope  
- [x] Consent fail-closed sample  
- [ ] Full agentic tool matrix (read/write/edit/bash/run/MCP)  
- [ ] Approval file protocol anti-spoof  
- [ ] skill_vet depth  
- [ ] Env key inventory for all spawns  
- [ ] Runtime path escape tests  

---

## 6. Priority

1. Close F-02 agent unlock gaps  
2. Env key inventory (F-04-010)  
3. mini_edit_apply + symlink proof (F-04-009)  
4. Unattended mode policy review (F-04-004)

---

## 7. Second-pass deep findings (2026-07-20)

### F-04-011 — mini_edit PASS2 write does not re-canonicalize (TOCTOU residual)

- **Severity:** S2  
- **Status:** open (residual)  
- **Location:** `backend/mini_edit_apply.rs` PASS1 ~419–448 vs PASS2 ~476–479  
- **Evidence:** PASS1 `canonicalize`s targets and enforces `starts_with(canon_root)`. PASS2 does `std::fs::write(canon_root.join(rel), …)` **without** re-checking symlink resolution.  
- **Impact:** Local race: if an allowlisted path is swapped for a symlink pointing outside the project between passes, write may follow the symlink. Requires concurrent FS control on the project tree (compromised agent/tool or multi-process). Not a remote unauth bug.  
- **Mitigation direction:** `OpenOptions` + `O_NOFOLLOW` / re-canonicalize after open, or write via fd opened in PASS1.

### F-04-012 — pi-sidecar executes bare `node` from PATH; sandbox optional

- **Severity:** S1 (reliability) / S2 (security)  
- **Status:** open  
- **Location:** `backend/pi_sidecar.rs` spawn (~`Command::new(&program)` with `program = "node"` when unsandboxed)  
- **Evidence:** Non-macOS or env override path logs `sandbox: disabled` and runs `("node", [sidecar.mjs])`. Packaged macOS GUI PATH often lacks Homebrew → wrong/missing binary (e2e seed). When PATH is polluted, **unexpected `node`** may run.  
- **Impact:** Wrong interpreter execution; failed launch; supply-chain if PATH points to malicious node.  
- **Mitigation direction:** resolve absolute node path at build/config time (same as provider_detect pattern); never bare PATH in release.

### F-04-013 — `parse_run_command` is strong against shell chaining (positive)

- **Severity:** n/a  
- **Status:** noted  
- **Location:** `backend/agentic_tools.rs::parse_run_command`  
- **Evidence:** Rejects `;|&$\`<>(){}[]…`, require program ∈ `RUN_PROGRAMS`, reject `..` segments and absolute paths in args.  
- **Impact:** Good control for the agentic `run` tool; residual is allowlisted tools executing project scripts (by design).

### F-04-014 — Agent provider env injects scoped tokens into child processes (by design)

- **Severity:** S2 residual  
- **Status:** accepted-risk (design)  
- **Location:** `projects.rs` (~4872+) `vault::read_cloudflare_agent_token_profile_*`; `EXA_API_KEY` / `DEVBOULE_CLOUD_API_KEY` injection paths  
- **Evidence:** Role-scoped CF profile tokens and optional Exa/cloud LLM keys placed in child env. Comments claim never logged / not on argv for some keys.  
- **Impact:** Any tool that can `env`/`printenv` or crash-dump in the child sees those secrets. Expected for cloud CLIs; keep role profiles least-privilege.

---

## 8. Agentic + role tools (pass 3)

See **[matrix-agents-mcp.md](./matrix-agents-mcp.md)**, **[matrix-role-tools.md](./matrix-role-tools.md)**, **[matrix-pi-sidecar.md](./matrix-pi-sidecar.md)**.

### F-04-020 — Orchestrator may rotate CF worker secrets via MCP tools

- **Severity:** S1  
- **Status:** open (policy vs product rules)  
- **Location:** `oracle/server/role_rules.json` — roles `coder` **and** `orchestrator` include `cloudflare_rotate_worker_secret` and `scaleway_resource_action`  
- **Evidence:** Product non-negotiable (README): orchestrators/verifiers should be read/status oriented; coders mutate. Orchestrator currently has secret rotation + SCW resource action in allowedTools.  
- **Impact:** Compromised/prompt-injected orchestrator can rotate worker secrets or act on Scaleway resources (still subject to Rust cloud guards + scoped tokens).

### F-04-021 — Verifier lacks rotate/cloud mutate tools (positive)

- **Severity:** n/a  
- **Status:** noted  
- **Evidence:** verifier allowedTools = 22; no `cloudflare_rotate_worker_secret`, no `scaleway_resource_action`, no `request_git_push`.

### F-04-022 — Mini role is read-only Oracle/structure (positive)

- **Severity:** n/a  
- **Status:** noted  
- **Evidence:** mini has only `agent_register`, `oracle_context`, `project_structure`, `get_neighborhood`, `find_imports`.

### F-04-023 — aspis_mcp enforces allowedTools set membership

- **Severity:** n/a  
- **Status:** noted  
- **Evidence:** `aspis_mcp.py` builds `ALLOWED_TOOLS` from ROLE_RULES; code paths with `not in allowed`.

### F-04-024 — pi_sidecar default ON (opt-out); pigeon classify absent in Rust

- **Severity:** S2 / B0  
- **Status:** open  
- **Location:** `pi_sidecar.rs::pi_sidecar_enabled` default-on; `pigeon_service.rs` has **no** `classify`  
- **Evidence:** matrix-pi-sidecar.md; aligns with e2e diagnosis of classification timeout/null path in sidecar.mjs.

---

## Truth-check (pass 6)

F-04-011 TOCTOU **CONFIRMED**. F-04-012 bare node **CONFIRMED**. F-04-020 orchestrator rotate **CONFIRMED**. F-04-009 **superseded**. See [VERIFICATION.md](./VERIFICATION.md).
