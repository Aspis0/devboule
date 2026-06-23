---
name: code-review
description: Systematic diff review for correctness, ripple effects, and architecture altitude. Use when reviewing a PR/diff before merge to ensure quality and safety.
metadata:
  author: devboule
  version: "1.0"
---
- **Scope**: Analyze the diff line-by-line, focusing on logic errors, security, and performance.
- **Check Removals**: Verify removed code is truly unused or replaced; check for dangling references.
- **Ripple Effects**: Identify cross-file impacts. Does this change break consumers, APIs, or shared utilities?
- **Simplification**: Suggest refactors for DRY principles, reduced complexity, or better efficiency.
- **Altitude**: Assess if the change is at the right abstraction layer (e.g., business logic vs. UI).
- **Verdicts**: Tag findings as `CONFIRMED` (bug), `PLAUSIBLE` (risk), or `REFUTED` (false alarm) with `file:line`.
- **Output**: Provide a structured list of findings with clear actionable recommendations.
