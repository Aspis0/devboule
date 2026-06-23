---
name: webapp-testing
description: Write robust E2E/UI tests for web apps. Use when creating or fixing browser-based tests to ensure reliability and maintainability.
metadata:
  author: devboule
  version: "1.0"
---
- **Selectors**: Use stable selectors (roles, test-ids, text) over brittle CSS classes.
- **User Flows**: Test real user journeys, not internal implementation details.
- **Assertions**: Assert on observable state (DOM, network, visual) rather than internal variables.
- **Async Control**: Use explicit waits for elements/actions; never use fixed sleeps.
- **Isolation**: Ensure tests are independent and can run in any order.
- **Headless**: Ensure tests run reliably in headless browsers (CI environments).
- **Cleanup**: Reset state between tests to avoid side effects.
