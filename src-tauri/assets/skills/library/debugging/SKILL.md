---
name: debugging
description: Root-cause analysis for bugs, crashes, or failing tests. Use when an unexpected behavior is observed to isolate and fix the cause.
metadata:
  author: devboule
  version: "1.0"
---
- **Reproduce**: Create a reliable, minimal reproduction case. If it doesn't happen consistently, stop.
- **Minimize**: Strip away unrelated code/context to isolate the smallest failing unit.
- **Hypothesize**: Form a falsifiable hypothesis about the cause.
- **Bisect**: Use binary search (git bisect or code commenting) to pinpoint the exact change or condition.
- **Fix Cause**: Address the root cause, not just the symptom. Avoid patches that hide the issue.
- **Test**: Write a regression test that fails before the fix and passes after.
- **Verify**: Confirm the fix resolves the original reproduction case without side effects.
