# Audit harnesses (pass 4)

| Artifact | Purpose |
|----------|---------|
| [locked_invoke_report.md](./locked_invoke_report.md) | Locked-session / ungated IPC proof |
| [locked_invoke_callgraph.json](./locked_invoke_callgraph.json) | Machine output of 3-hop gate reachability |
| [locked_invoke_static.py](./locked_invoke_static.py) | Re-runnable call-graph scanner |
| [locked_state_probe.rs.txt](./locked_state_probe.rs.txt) | Copy-paste unit probe for `BackendState` (see report) |

Parent findings: `docs/audit/FINDINGS.md`, `docs/audit/02-command-surface.md`.

Truth-check of NO_GATE results: [../VERIFICATION.md](../VERIFICATION.md) (FP-1 on mutate labels).
