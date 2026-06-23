---
name: tdd-strict
description: Make a handed-in FAILING test pass without modifying or weakening it. Use when the task ships with a red test and the goal is to turn it green (TDD-strict mode).
metadata:
  author: devboule
  version: "1.0"
---
# TDD-strict — make the failing test green

You have been handed a test that currently FAILS (red). Your job is to change the
implementation so the test PASSES (green). The enforcement below is checked in code, not
on trust — gaming it fails the gate even if the test ends up green.

## Rules
- **Do not modify the test.** The test file is immutable and is excluded from your file scope; an
  edit targeting it is rejected outright.
- **Do not disable, skip, focus away from, or weaken the test** — in any language. No
  ignore/skip/xfail/disable annotations, no focusing a different test so this one stops running, no
  always-true assertions, no commenting out the checks. Enforcement is automatic and such attempts
  fail the gate regardless of the technique.
- **Do not tamper with test configuration or build scripts** to neuter the test from outside it.
- **Fix the cause.** Implement the behavior the test specifies; do not hardcode the expected
  output just to satisfy this one case.

## How to work
1. Read the failing test to understand the exact contract (inputs, expected outputs, edge cases).
2. Make the smallest implementation change that genuinely satisfies that contract.
3. Stay inside your allowed file scope; reuse what the project already imports.
4. The gate passes only when the test went red → green with no gaming detected.
