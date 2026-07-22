# Audit coverage — honest map

**Updated:** 2026-07-20 (second pass, after challenge that the first pass was too thin)

This file exists so nobody confuses **inventory + sampled static review** with a
full-time security engagement.

## What “audited” means here

| Level | Meaning |
|-------|---------|
| **L0 Inventory** | Counted APIs, spawns, configs; no line-by-line threat work |
| **L1 Sample static** | Read critical samples; gate heuristics; some helpers verified |
| **L2 Hostile static** | Trace attack paths with code evidence; still no exploit PoC / runtime |
| **L3 Runtime / e2e** | Locked invoke, live agents, packaged PATH — **mostly NOT done** |
| **L4 Formal / pen-test** | External red team — **not done** |

## Coverage by area

| Area | File | Depth | Notes |
|------|------|-------|-------|
| Inventory / surface | `00-inventory.md` | **L0→L1** | 303 commands, spawn list, CSP, vault map |
| Auth / vault / roles | `01-auth-vault-roles.md` | **L1** | Session APIs, keyring, capability model; no crypto formal review of grants |
| Command surface | `02-command-surface.md` | **L1→L2** | Ungated mutate list refined; not every of 303 bodies read end-to-end |
| Cloud CF/SCW | `03-cloud-providers.md` | **L1** | Sampled rotation, SCW action, D1, R2, create instance — **not full write matrix** |
| Agents / sandbox | `04-agents-sandbox-consent.md` | **L1→L2** | mini path apply, run parser, seatbelt scope, duplex env samples; **not full tool surface** |
| Oracle / MCP / design | `05-oracle-mcp-design.md` | **L1→L2** | secret basename skip, MCP allowlist rules, iframe/postMessage; **aspis_mcp role matrix incomplete** |
| Frontend | `06-frontend-trust.md` | **L1→L2** | lock UI, CSP, localStorage grep, design sinks; **not every view** |
| Functional bugs | `07-functional-bugs.md` | **L0+import** | e2e ledger import; **not re-repro’d live this session** |
| Supply chain | `08-supply-chain-build.md` | **L0→L1** | capabilities/CSP; **`npm audit` / `cargo audit` not executed** |
| Bootstrap crypto | (under 03/08/workspace) | **L2 sample** | signature-before-decrypt, safe_tar, fresh import dir |

## Explicitly NOT done (gaps)

1. **Runtime invoke-while-locked** for F-02-* (needs running app + DevTools or harness).  
2. **Full Scaleway/Cloudflare write-command matrix** (every create/delete/resize).  
3. **Full agentic tool matrix** (every MCP tool + agentic_tools method).  
4. **aspis_mcp.py role_rules × tool** complete matrix.  
5. **Live pi-sidecar / planner e2e** on this machine.  
6. **Dependency CVE scan** (`cargo audit`, `npm audit`).  
7. **Windows-only paths** (Hello, conhost, ACL) — macOS-biased reading.  
8. **Polis** beyond stubs/debug log — not a security focus this pass.  
9. **Formal crypto review** of role grants / bootstrap X25519+AES-GCM (design appears serious; not proven).  
10. **Concurrent multi-agent races** beyond notes.

## What *was* verified with code evidence (second pass)

- Ungated **mutating** command shortlist (refined heuristic + body read).  
- `mini_edit_apply` PASS1 root confinement + PASS2 write path residual TOCTOU.  
- Bootstrap `safe_unpack_tar` + fresh `output_dir` + signature-before-decrypt.  
- `parse_run_command` meta-character denylist.  
- User MCP freeform command + global allowlist exemption.  
- Oracle `basename_is_secret` / `.env` skip.  
- Artifact `postMessage` trust anchor = `event.source` (not origin).  
- pi-sidecar spawn via bare `"node"` + optional Seatbelt wrap.  
- FE `localStorage` prefs-only (no vault tokens).  
- Cloud mutation samples use `sensitive_session_id` + pin guards.

## How to read severity

Findings marked **needs-runtime** or **uncertainty** are **not** confirmed exploits.
**S1 open** items are “this code path lacks a control that the product claims elsewhere” —
high priority for fix or accepted-risk sign-off, not automatic “CVE today”.

---

## Pass 3 (2026-07-20) — expanded

| Deliverable | Depth |
|-------------|-------|
| [matrix-commands.md](./matrix-commands.md) | **L2** full 303 command gate matrix |
| [matrix-cloud.md](./matrix-cloud.md) | **L2** CF/SCW write/read matrix |
| [matrix-role-tools.md](./matrix-role-tools.md) | **L2** role×tool SSOT |
| [matrix-agents-mcp.md](./matrix-agents-mcp.md) | **L1–L2** agentic + mcp notes |
| [matrix-pi-sidecar.md](./matrix-pi-sidecar.md) | **L2** sidecar/pigeon static |
| [supply-chain/](./supply-chain/) | **L2** cargo+npm audit **executed** |

Still **not** done: locked-invoke runtime, live planner e2e, Windows-only Hello paths, formal crypto proof.

---

## Pass 4 (2026-07-20)

| Item | Depth | Status |
|------|-------|--------|
| Locked-invoke (call-graph 3-hop + BackendState contract) | **L2+** | [harness/locked_invoke_report.md](./harness/locked_invoke_report.md) — 16 NO_GATE confirmed; 7 controls GATE |
| Live Tauri `invoke` while locked | L3 | Still not automated (GUI) |
| MCP rotate + SCW action end-to-end trace | **L2** | [trace-mcp-cloud.md](./trace-mcp-cloud.md) — dual stack documented |
| CVE prod vs dev reachability | **L2** | [supply-chain/REACHABILITY.md](./supply-chain/REACHABILITY.md) |

---

## Pass 5 — Cross-file

| Item | Status |
|------|--------|
| Shared artifact multi-writer map | Done — [CROSS-FILE.md](./CROSS-FILE.md) |
| lock_app ↔ agent/Oracle lifecycle | Done — F-XF-001/008/009 |
| Dual cloud stacks | Done — F-XF-002 |
| Co-writer constant parity check | Done — match today; F-XF-004 process risk |
| Role semantic multi-SSOT | Done — F-XF-003/010 |
| Live race stress / full schema field diff | **Not done** |

---

## Pass 6 — Truth-check

| Item | Result |
|------|--------|
| Re-verify S1 claims vs source | Mostly **CONFIRMED** |
| False positives scrubbed | UNGATED_MUTATE over-tag, stale 303, F-08-002, overstated “incomplete” |
| Full report | [VERIFICATION.md](./VERIFICATION.md) |
