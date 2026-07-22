# Devboule audit — master findings ledger

**Last updated:** 2026-07-20 (pass 6 — truth-check / anti-hallucination)
**Truth-check:** [VERIFICATION.md](./VERIFICATION.md) — re-verified claims vs source  
**Mode:** static audit (no product code changes)  
**Plan:** [00-plan.md](./00-plan.md)  
**Coverage honesty:** [COVERAGE.md](./COVERAGE.md) — this is **not** a full L3/L4 pen-test; depths vary by area.

Severity: **S0–S3** security · **B0–B3** functional · **n/a** positive/control notes  

Status: `open` · `needs-*` · `accepted-risk` · `noted`

---

## Priority queue (actionable)

| Priority | Id | Sev | Title | Phase file |
|----------|-----|-----|-------|------------|
| 1 | F-00-002 | S1 | UI lock is not an IPC firewall | [00](./00-inventory.md) |
| 1 | F-02-001 | S1 | Cloud duplex send/interrupt/compact without unlock | [02](./02-command-surface.md) |
| 1 | F-02-002 | S1 | Orchestrator steer / planner reset without unlock | [02](./02-command-surface.md) |
| 1 | F-02-003 | S1 | Mini-coder steer without unlock | [02](./02-command-surface.md) |
| 1 | F-02-005 | S1 | Pi extension install/remove without unlock | [02](./02-command-surface.md) |
| 1 | F-02-014 | S1 | Cloud duplex send any agent_id, no unlock | [02](./02-command-surface.md) |
| 1 | F-04-012 | S1/S2 | pi-sidecar bare `node` PATH + optional sandbox | [04](./04-agents-sandbox-consent.md) |
| 1 | F-04-020 | S1 | Orchestrator can rotate CF secrets via MCP | [matrix-role-tools.md](./matrix-role-tools.md) |
| 1 | F-08-010 | S1 | rmcp DNS rebinding (cargo) | [08](./08-supply-chain-build.md) |
| 1 | F-08-020 | S1 | vitest critical (dev) | [08](./08-supply-chain-build.md) |
| 1 | F-02-020 | S1 | Full ungated mutate list (16) | [matrix-commands.md](./matrix-commands.md) |
| 2 | F-04-007 | S1 | Pi extension = supply-chain code exec | [04](./04-agents-sandbox-consent.md) |
| 2 | F-04-011 | S2 | mini_edit PASS2 symlink TOCTOU residual | [04](./04-agents-sandbox-consent.md) |
| 2 | F-02-013 | S2 | design_request_complete arbitrary paths | [02](./02-command-surface.md) |
| 2 | F-05-007 | S1 | User MCP freeform global command (allowlist-exempt) | [05](./05-oracle-mcp-design.md) |
| 2 | F-05-003 | S1 | Design innerHTML sanitize regression risk | [05](./05-oracle-mcp-design.md) |
| 2 | F-08-005 | S1 | Marketplace/extension supply chain | [08](./08-supply-chain-build.md) |
| 3 | F-05-004 | S2 | Oracle residual: secrets in non-secret filenames | [05](./05-oracle-mcp-design.md) |
| 3 | F-06-003 | S3 | FE localStorage prefs only (tokens not persisted) | [06](./06-frontend-trust.md) |
| 3 | F-02-004 | S2 | Design-request claim/complete ungated | [02](./02-command-surface.md) |
| 3 | F-02-009 | S2 | No per-command IPC ACL | [02](./02-command-surface.md) |
| 3 | F-03-002 | S2 | Cloud write matrix incomplete | [03](./03-cloud-providers.md) |
| 3 | F-03-003 | S2 | D1 write-detection trust-on-tests | [03](./03-cloud-providers.md) |
| 3 | F-04-004 | S2 | Unattended bypassPermissions trust | [04](./04-agents-sandbox-consent.md) |
| 3 | F-04-009 | S2 | mini_edit_apply path proof incomplete | [04](./04-agents-sandbox-consent.md) |
| 3 | F-04-010 | S2 | Cloud duplex env key inventory | [04](./04-agents-sandbox-consent.md) |
| 3 | F-05-002 | S2 | Interactive artifact JS residual | [05](./05-oracle-mcp-design.md) |
| 3 | F-01-002 | S2 | reset device identity no ManageDevices | [01](./01-auth-vault-roles.md) |
| 3 | F-07-002 | B0 | Planner console reliability | [07](./07-functional-bugs.md) |
| 4 | F-01-001 | S3 | “Windows Hello” string everywhere | [01](./01-auth-vault-roles.md) |
| 4 | F-07-001 | B1 | Idle freeze/minimize | [07](./07-functional-bugs.md) |
| 4 | F-08-002 | S2 | cargo/npm audit not run this pass | [08](./08-supply-chain-build.md) |

---

## Full ledger by phase

### Phase 0 — Inventory

See [00-inventory.md](./00-inventory.md).

| Id | Sev | Status | Title |
|----|-----|--------|-------|
| F-00-001 | S2 | open | Large Tauri command surface (303 handlers) |
| F-00-002 | S1 | open | UI lock is not an IPC firewall |
| F-00-003 | S2 | open | ~148 process spawn sites |
| F-00-004 | n/a | noted | Secrets in OS keyring (positive) |

**Headline metrics:** 303 Tauri commands · ~148 spawn sites · keyring vault · CSP present · minimal capabilities.

---

### Phase 1 — Auth / vault / roles

| Id | Sev | Status | Title |
|----|-----|--------|-------|
| F-01-001 | S3 | open | Lock errors always say Windows Hello |
| F-01-002 | S2 | open | `reset_local_device_identity` no ManageDevices |
| F-01-003 | S3 | accepted-risk | Capability checks not a hard security boundary |
| F-01-004 | S3 | open | Devices snapshot any unlocked role |
| F-01-005 | S3 | open | Token min-length only |
| F-01-006 | n/a | noted | Session expire clears sensitive runtime data |

---

### Phase 2 — Command surface

| Id | Sev | Status | Title |
|----|-----|--------|-------|
| F-02-001 | S1 | open | Cloud duplex control ungated |
| F-02-002 | S1 | open | Local orchestrator steer / planner reset ungated |
| F-02-003 | S1 | open | Mini-coder steer ungated |
| F-02-004 | S2 | open | Design-request claim/complete ungated |
| F-02-005 | S1 | open | Pi extension install/remove ungated |
| F-02-006 | n/a | noted | open_external_url well gated |
| F-02-007 | n/a | noted | Agent launch gated via helper |
| F-02-008 | n/a | noted | Plan approve/deny gated via helper |
| F-02-009 | S2 | accepted-risk | No per-command Tauri ACL |
| F-02-010 | S3 | open | Read-only probes ungated |
| F-02-011 | S3 | open | Cost/budget ungated |
| F-02-012 | S3 | open | polis_debug_log no auth |
| F-02-013 | S2 | open | design_request_complete arbitrary paths |
| F-02-014 | S1 | open | cloud duplex send no unlock/ownership |
| F-02-015 | S1 | open | Ungated mutate shortlist (second pass) |

---

### Phase 3 — Cloud

| Id | Sev | Status | Title |
|----|-----|--------|-------|
| F-03-001 | n/a | noted | Mature guard chains on sampled mutations |
| F-03-002 | S2 | needs-static-completion | Full destructive matrix not done |
| F-03-003 | S3 | open | D1 write-detection CTE/EXPLAIN covered; residual edges |
| F-03-004 | S2 | open | Inventory TOCTOU residual |
| F-03-005 | n/a | noted | Billing session-gated reads |
| F-03-006 | n/a | noted | Scoped CF agent token profiles design |

---

### Phase 4 — Agents / sandbox / consent

| Id | Sev | Status | Title |
|----|-----|--------|-------|
| F-04-001 | S2 | accepted-risk | Seatbelt only mini loopback |
| F-04-002 | S1 | open | Agent control unlock gaps (→ F-02-*) |
| F-04-003 | S2 | accepted-risk | `run` tool full build toolchains |
| F-04-004 | S2 | open | Unattended + bypassPermissions |
| F-04-005 | n/a | noted | Consent fail-closed |
| F-04-006 | n/a | noted | Git/plan human gates unlock-gated |
| F-04-007 | S1 | open | Pi extension supply chain |
| F-04-008 | B1/S2 | open | Packaged GUI PATH |
| F-04-009 | S2 | needs-deeper-read | mini_edit_apply path safety |
| F-04-010 | S2 | needs-inventory | Duplex env keys per role |
| F-04-011 | S2 | open | mini_edit PASS2 TOCTOU symlink residual |
| F-04-012 | S1/S2 | open | pi-sidecar bare node PATH |
| F-04-013 | n/a | noted | parse_run_command strong meta denylist |
| F-04-014 | S2 | accepted-risk | Provider tokens in agent child env |

---

### Phase 5 — Oracle / MCP / design

| Id | Sev | Status | Title |
|----|-----|--------|-------|
| F-05-001 | n/a | noted | Dual design security models coherent |
| F-05-002 | S2 | residual | Artifact JS in sandbox |
| F-05-003 | S1 | open | innerHTML sanitize single point of failure |
| F-05-004 | S2 | open | Oracle residual: secrets in non-secret filenames |
| F-05-005 | S2 | accepted-risk | Operator full-corpus /ask |
| F-05-006 | n/a | noted | Oracle setup/install requires unlock |
| F-05-007 | S1 | open | User MCP freeform global command |
| F-05-008 | S2 | open | Skill marketplace supply chain |
| F-05-009 | S2 | needs-deeper-read | aspis_mcp role rules |
| F-05-010 | n/a | noted | Artifact postMessage source identity |
| F-05-011 | n/a | noted | Bootstrap unpack crypto+path strong |
| F-05-012 | S2 | open | aspis_mcp role matrix coverage gap |

---

### Phase 6 — Frontend

| Id | Sev | Status | Title |
|----|-----|--------|-------|
| F-06-001 | S3 | accepted-risk | FE role/nav not security |
| F-06-002 | S3 | open | Deep links are view#tab only |
| F-06-003 | S3 | open | localStorage prefs only; tokens not persisted |
| F-06-004 | S2 | open | Event listener cleanup |
| F-06-005 | S3 | accepted-risk | style-src unsafe-inline |
| F-06-006 | S2 | needs-check | Error UI leakage |
| F-06-007 | n/a | noted | Debug role release-disabled backend |

---

### Phase 7 — Functional

| Id | Sev | Status | Title |
|----|-----|--------|-------|
| F-07-001 | B1 | open | Idle freeze/minimize |
| F-07-002 | B0 | open | Planner/orchestrator console |
| F-07-003 | B3 | open | Help thin |
| F-07-004 | B1 | open | macOS input/buttons |
| F-07-007 | S3 | open | Hello copy (= F-01-001) |
| F-07-010 | B2 | open | Oracle false “not responding” |
| F-07-011 | B2 | open | Notifications not dismissable |
| F-07-015 | B2 | open | Ungated IPC vs lock UX |
| F-07-016 | B3 | open | Polis SCW stubs |

(Additional e2e rows F-07-005…014 in [07-functional-bugs.md](./07-functional-bugs.md).)

---

### Phase 8 — Supply chain

| Id | Sev | Status | Title |
|----|-----|--------|-------|
| F-08-001 | n/a | noted | Minimal capabilities |
| F-08-002 | S2 | open | npm/cargo audit not run |
| F-08-003 | S3 | open | Dual node_modules trees |
| F-08-004 | S1 | needs-check | Resources secret scan |
| F-08-005 | S1 | open | Extension install supply chain |
| F-08-006 | n/a | noted | License/notices present |

---

## Counts (approx)

| Sev | Open / needs-* | Accepted / noted |
|-----|----------------:|------------------:|
| S0 | 0 | — |
| S1 | ~10 | — |
| S2 | ~15 | several accepted |
| S3 | ~8 | several accepted |
| B0 | 1 | — |
| B1–B3 | many (e2e import) | — |

**No S0 confirmed** in static pass. Highest confidence **S1** cluster: **ungated agent-control + extension install IPC** (F-02-001…003, F-02-005).

---

## Phase completion status

| Phase | File | Status |
|-------|------|--------|
| Plan | [00-plan.md](./00-plan.md) | done |
| 0 Inventory | [00-inventory.md](./00-inventory.md) | **complete (static)** + findings |
| 1 Auth | [01-auth-vault-roles.md](./01-auth-vault-roles.md) | complete (static) |
| 2 Commands | [02-command-surface.md](./02-command-surface.md) | complete (static) |
| 3 Cloud | [03-cloud-providers.md](./03-cloud-providers.md) | complete (static samples) |
| 4 Agents | [04-agents-sandbox-consent.md](./04-agents-sandbox-consent.md) | complete (static) |
| 5 Oracle/MCP/Design | [05-oracle-mcp-design.md](./05-oracle-mcp-design.md) | complete (static) |
| 6 Frontend | [06-frontend-trust.md](./06-frontend-trust.md) | complete (static) |
| 7 Bugs | [07-functional-bugs.md](./07-functional-bugs.md) | complete (ledger + e2e import) |
| 8 Supply chain | [08-supply-chain-build.md](./08-supply-chain-build.md) | complete (static; no npm/cargo audit run) |
| Coverage map | [COVERAGE.md](./COVERAGE.md) | L0–L2 honesty |

---

## Recommended next audit steps (still no product fixes)

1. **Runtime** matrix: invoke F-02-001…005 / F-02-014 while app locked  
2. Complete cloud write-command matrix (F-03-002)  
3. Full **aspis_mcp** role×tool matrix (F-05-012)  
4. Absolute **node** resolution for pi-sidecar (F-04-012)  
5. `cargo audit` + `npm audit` attach to Phase 8  
6. Live planner e2e re-repro (F-07-002)

---

## Pass 3 additions (full matrices + cargo/npm audit)

### New / elevated findings

| Id | Sev | Title | Detail file |
|----|-----|-------|-------------|
| F-02-020 | S1 | Full ungated list (16 cmds; ~10–11 real mutates — see VERIFICATION FP-1) | [matrix-commands.md](./matrix-commands.md) |
| F-03-010 | n/a | Cloud writes session-gated (matrix) | [matrix-cloud.md](./matrix-cloud.md) |
| F-03-011 | S3 | Confirm keywords not on all deletes | [matrix-cloud.md](./matrix-cloud.md) |
| F-04-020 | S1 | Orchestrator MCP can rotate CF secrets | [matrix-role-tools.md](./matrix-role-tools.md) |
| F-04-021 | n/a | Verifier no cloud mutate tools | matrix-role-tools |
| F-04-022 | n/a | Mini read-only tools | matrix-role-tools |
| F-04-023 | n/a | aspis_mcp enforces allowedTools | matrix-agents-mcp |
| F-04-024 | S2/B0 | pi default-on; pigeon classify missing in Rust | [matrix-pi-sidecar.md](./matrix-pi-sidecar.md) |
| F-05-020 | n/a | role_rules.json SSOT | matrix-role-tools |
| F-05-021 | S2 | DOMPurify npm moderate (design path) | [08](./08-supply-chain-build.md) |
| F-08-010 | S1 | rmcp DNS rebinding RUSTSEC-2026-0189 | cargo-audit |
| F-08-011 | S2 | quick-xml DoS (multiple) | cargo-audit |
| F-08-012 | S2 | quinn-proto mem exhaustion | cargo-audit |
| F-08-013 | S2 | crossbeam-epoch | cargo-audit |
| F-08-014 | S3 | 22 unmaintained/unsound warnings | cargo-audit |
| F-08-020 | S1 | vitest critical (dev UI) | npm-audit-root |
| F-08-021 | S2 | vite high (Windows) | npm-audit-root |
| F-08-022 | S2 | undici high | npm-audit-root |
| F-08-023 | S2 | DOMPurify moderate | npm-audit-root |
| F-08-024 | n/a | pi-sidecar npm clean | npm-audit-pi-sidecar |

### Artifact index (pass 3)

| Artifact | Purpose |
|----------|---------|
| [matrix-commands.md](./matrix-commands.md) | All 303 commands × gate |
| [matrix-commands.json](./matrix-commands.json) | Machine-readable |
| [matrix-cloud.md](./matrix-cloud.md) | CF/SCW write/read |
| [matrix-role-tools.md](./matrix-role-tools.md) | role × allowedTools |
| [matrix-agents-mcp.md](./matrix-agents-mcp.md) | agentic + mcp notes |
| [matrix-pi-sidecar.md](./matrix-pi-sidecar.md) | sidecar/pigeon |
| [supply-chain/cargo-audit.txt](./supply-chain/cargo-audit.txt) | Raw cargo audit |
| [supply-chain/npm-audit-root.txt](./supply-chain/npm-audit-root.txt) | Raw npm audit |
| [supply-chain/npm-audit-pi-sidecar.txt](./supply-chain/npm-audit-pi-sidecar.txt) | Raw npm audit sidecar |
| [COVERAGE.md](./COVERAGE.md) | Honest depth map |

### Still out of scope (honest)

- Runtime invoke-while-locked harness (static matrix is the substitute)
- Live planner/oMLX e2e re-run
- Formal crypto proofs
- Auto-bump of vulnerable deps (audit-only)

---

## Pass 4 — locked harness, MCP cloud trace, CVE reachability

| Deliverable | Path |
|-------------|------|
| Locked-invoke report | [harness/locked_invoke_report.md](./harness/locked_invoke_report.md) |
| Call-graph JSON | [harness/locked_invoke_callgraph.json](./harness/locked_invoke_callgraph.json) |
| MCP → cloud trace | [trace-mcp-cloud.md](./trace-mcp-cloud.md) |
| CVE reachability | [supply-chain/REACHABILITY.md](./supply-chain/REACHABILITY.md) |

### New finding IDs

| Id | Sev | Title |
|----|-----|-------|
| F-LCK-001 | S1 | 16 cmds never reach session gate (3-hop) |
| F-LCK-002 | n/a | Control group correctly gated |
| F-LCK-003 | S1 | UI lock ≠ IPC lock reaffirmed |
| F-MCP-001 | S2 | Dual cloud mutation stacks (Rust vs Python) |
| F-MCP-002 | S2 | Orchestrator mutate intentional in code |
| F-MCP-003 | n/a | MCP SCW confirm-by-name positive |
| F-MCP-004 | n/a | MCP CF inventory check positive |
| F-MCP-005 | S2 | MCP mutations independent of Tauri lock |
| F-MCP-006 | S2 | Agent kill-on-lock needs-check |
| F-RCH-001 | S2 | quick-xml PROD-REACH (S3 XML) |
| F-RCH-002 | S2 | DOMPurify PROD-REACH |
| F-RCH-003 | S3 | rmcp HTTP advisory low match (stdio) |
| F-RCH-004 | S1-dev | vitest critical DEV-ONLY |
| F-RCH-005 | S2-dev | vite/undici DEV-ONLY |

### Priority refresh (after pass 4)

1. **Gate the 16** (F-LCK-001 / F-02-020) — highest confidence S1  
2. **Dual cloud path** (F-MCP-001) — keep Rust/Python guards in sync  
3. **Orchestrator policy** (F-MCP-002 / F-04-020) — doc or remove tools  
4. **Lock kills agents?** (F-MCP-006)  
5. **Bump quick-xml + dompurify** (F-RCH-001/002) when leaving audit-only  
6. Dev bumps vitest/vite (F-RCH-004/005)

---

## Pass 5 — Cross-file interactions

**Report:** [CROSS-FILE.md](./CROSS-FILE.md)

| Id | Sev | Title |
|----|-----|-------|
| F-XF-001 | S1 | Lock keeps agents+Oracle alive; only clears caches |
| F-XF-002 | S1 | Dual cloud mutation stacks (Rust UI ≠ MCP Python) |
| F-XF-003 | S2 | Orchestrator mutator vs “tighter planner” narrative |
| F-XF-004 | S2 | Co-writer parity comment-only (constants match today) |
| F-XF-005 | S2 | Shared agents.json multi-process RMW / last-writer |
| F-XF-006 | S2 | Git/plan/consent multi-actor file bridges |
| F-XF-007 | S1 | Three steer channels, inconsistent gates |
| F-XF-008 | S2 | Oracle decoupled from vault lock (documented) |
| F-XF-009 | S2 | FE lock_app does not tear down agents |
| F-XF-010 | S2 | Role SSOT incomplete (tools vs CODER_LIKE vs aliases) |
| F-XF-011 | S2 | Lock timeout / stuck bell under cross-writer load |
| F-XF-012 | S2 | Dual inventory truth (cache vs MCP live list) |

**Top cross-file priority:** F-XF-001 + F-XF-002 + F-XF-007 (lock model, dual cloud, steer gates).

---

## Pass 6 — Truth-check (anti-hallucination)

Full report: **[VERIFICATION.md](./VERIFICATION.md)**

### Corrections (do not ignore)

| Issue | Correction |
|-------|------------|
| “16 UNGATED_MUTATE” | **16 ungated**, but only **~10–11 real mutates**; catalogs/snapshot/list are **reads** (FP) |
| Command count 303 | Now **303** `#[tauri::command]` (stale metric) |
| F-08-002 audits not run | **REFUTED** — cargo/npm audit logs exist |
| F-04-009 path unknown | **Superseded** by F-04-011 (PASS1 solid, PASS2 TOCTOU) |
| F-05-012 / F-03-002 “not audited” | **Weakened** — matrices exist; residual is depth |
| rmcp as prod RCE | **Weakened** — stdio only (F-RCH-003) |

### Still CONFIRMED S1 (after scrub)

- Ungated agent steer/send/reset/install/claim-complete  
- Lock keeps agents+Oracle; no session kill  
- Dual cloud stacks (Tauri ≠ MCP Python)  
- Orchestrator MCP secret rotation allowed  
