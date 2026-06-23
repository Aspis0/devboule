---
name: commit-messages
description: Generate Conventional Commits from diffs. Use when creating commit messages to ensure clear, structured history.
metadata:
  author: devboule
  version: "1.0"
---
- **Format**: `type(scope): subject` (subject <=72 chars, imperative mood, no period).
- **Types**: feat, fix, docs, style, refactor, perf, test, build, ci, chore, revert.
- **Body**: Explain WHY the change was made, not WHAT changed. Context is key.
- **Footers**: Include `BREAKING CHANGE:` for major changes, or reference issues (e.g., `Closes #123`).
- **Scope**: Use a meaningful scope (module, component, or file group) if applicable.
- **Check**: Ensure the message is skimmable and provides full context for future readers.
