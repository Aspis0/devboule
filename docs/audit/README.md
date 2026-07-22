# Devboule security & bug audit

**Mode:** audit only (no product fixes)  
**Latest pass:** 6 (2026-07-20) — + truth-check (anti-hallucination)

## Start here

1. **[COVERAGE.md](./COVERAGE.md)** — what was actually covered (honest depths)  
2. **[FINDINGS.md](./FINDINGS.md)** — master ledger + priority queue  
3. Area reports `0N-*.md`  
4. Machine matrices `matrix-*.md`  
5. Supply-chain raw logs `supply-chain/`

## Area reports

| File | Area |
|------|------|
| [00-plan.md](./00-plan.md) | Plan |
| [00-inventory.md](./00-inventory.md) | Inventory |
| [01-auth-vault-roles.md](./01-auth-vault-roles.md) | Auth / vault / roles |
| [02-command-surface.md](./02-command-surface.md) | Tauri commands |
| [03-cloud-providers.md](./03-cloud-providers.md) | Cloudflare / Scaleway |
| [04-agents-sandbox-consent.md](./04-agents-sandbox-consent.md) | Agents / sandbox / consent |
| [05-oracle-mcp-design.md](./05-oracle-mcp-design.md) | Oracle / MCP / design |
| [06-frontend-trust.md](./06-frontend-trust.md) | Frontend trust |
| [07-functional-bugs.md](./07-functional-bugs.md) | Functional / e2e bugs |
| [08-supply-chain-build.md](./08-supply-chain-build.md) | Supply chain |

## Pass 4 deliverables

| | |
|--|--|
| Locked invoke | [harness/locked_invoke_report.md](./harness/locked_invoke_report.md) |
| MCP cloud | [trace-mcp-cloud.md](./trace-mcp-cloud.md) |
| CVE reachability | [supply-chain/REACHABILITY.md](./supply-chain/REACHABILITY.md) |
| **Cross-file** | **[CROSS-FILE.md](./CROSS-FILE.md)** |
| **Truth-check** | **[VERIFICATION.md](./VERIFICATION.md)** |

## Top S1 (fix now, still audit-only)


1. Ungated agent IPC (`F-02-001`…`014`, `F-02-020`) — add `ensure_unlocked`  
2. Orchestrator MCP secret rotation (`F-04-020`) — policy mismatch  
3. pi-sidecar PATH/`node` (`F-04-012`)  
4. User MCP global freeform command (`F-05-007`)  
5. rmcp / vitest advisories (`F-08-010`, `F-08-020`)  
6. DOMPurify on design path (`F-08-023` / `F-05-021`)
