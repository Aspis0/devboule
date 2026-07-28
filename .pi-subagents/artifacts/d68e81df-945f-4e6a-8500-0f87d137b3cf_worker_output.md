Done. All verification passed. Commit `5510752`.

**Commit hash:** `55107529ff903372ad714eb8e7709e95428a7793` (short `5510752`)

**cargo check --tests result (post-commit, last lines):**
```
warning: `devboule` (lib) generated 168 warnings (run `cargo fix --lib -p devboule` to apply 31 suggestions)
warning: `devboule` (lib test) generated 101 warnings (52 duplicates) (run `cargo fix --lib -p devboule --tests` to apply 25 suggestions)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.76s
```
(0 errors. The 168/101 warnings are pre-existing across the crate and unrelated to C1.)

**git show HEAD --stat:**
```
5510752 feat(sandbox): add Windows Job Object wrapper (C1, kill-on-close + mem limit)

 src-tauri/src/backend/agentic_tools.rs   |  11 +++
 src-tauri/src/backend/sandbox/mod.rs     |  21 ++++-
 src-tauri/src/backend/sandbox/windows.rs | 151 +++++++++++++++++++++++++++++++
 3 files changed, 178 insertions(+), 5 deletions(-)
```

**Scope confirmation:** Only the 3 listed files were modified/created (`windows.rs` new, `mod.rs` + `agentic_tools.rs` modified). `Cargo.toml`, `seatbelt.rs`, `is_enforced()`, and all test files are untouched. The untracked `oracle/decision-c1.md`, `advisor/`, and `.pi-subagents/` dirs were deliberately left out of the commit. No other tracked file changed (`git diff --name-only HEAD~1 HEAD` lists exactly those 3).

**Notes on implementation vs spec:** Two spec inaccuracies were corrected during compilation: (1) `AssignProcessToJobObject` lives in `Win32::System::JobObjects`, not `Threading` — fixed the import; (2) the spec's `super::windows::` path from inside `mod.rs` is wrong (`super` from the `sandbox` module resolves to `backend`, not `sandbox`) — changed to `crate::backend::sandbox::windows::`. Both are mechanical path fixes; the API shape, behavior, and failure modes match the spec exactly. `is_enforced()` remains `false` on Windows as required.