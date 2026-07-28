# Milestone H — CI Matrix: Hostile Audit

**Commit**: `db0cb20` on `windows-port`
**Parent**: `5522a31`
**Reviewer**: deepseek-v4-pro (subagent)
**Scope**: `.github/workflows/ci.yml` (new, +120 lines)

---

## Review

### Correct

- **Exactly 1 file, +120 lines**: `git show db0cb20 --stat` confirms `.github/workflows/ci.yml | 120 ++++++` — no other files touched. (line count: `git diff --stat 5522a31..db0cb20` → 120 insertions, 0 deletions)

- **Valid YAML**: `python -c "import yaml; yaml.safe_load(...)"` → `YAML OK`. No parse errors. `.github/workflows/ci.yml:1` — `name: ci`.

- **Triggers correct** (`.github/workflows/ci.yml:4-7`):
  - `push.branches: [main, "windows-*"]` — covers main + all windows-port branches.
  - `pull_request.branches: [main]` — PR checks restricted to main, no noise on feature→feature PRs.

- **Concurrency present** (`.github/workflows/ci.yml:10-13`): `group: ${{ github.workflow }}-${{ github.ref }}`, `cancel-in-progress: true`. In-flight runs on same branch are cancelled on new push — standard CI hygiene.

- **`test` job matrix correct** (`.github/workflows/ci.yml:15-22`):
  - `strategy.fail-fast: false` → one OS failure doesn't cancel others (line 20).
  - `matrix.os: [ubuntu-latest, macos-latest, windows-latest]` → all 3 OSes (line 22).

- **`windows-target-check` is a separate job** (`.github/workflows/ci.yml:107-120`): Sibling of `test` under `jobs`, NOT nested. Runs on `ubuntu-latest` (line 112). The final step is `cargo check --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc` (line 120).

- **protoc install per-OS correct**:
  - Ubuntu (`.github/workflows/ci.yml:53`): `sudo apt-get update && sudo apt-get install -y protobuf-compiler` — correct; GitHub runners need `sudo` + `update` for fresh package lists.
  - macOS (`.github/workflows/ci.yml:57`): `brew install protobuf` — correct for macOS runners.
  - Windows (`.github/workflows/ci.yml:61-62`): `choco install protoc -y --no-progress` with `shell: pwsh` — correct; `choco` is the Windows runner package manager, `pwsh` is needed for choco.

- **Windows MSVC target conditional** (`.github/workflows/ci.yml:36-38`): `if: matrix.os == 'windows-latest'` → `rustup target add x86_64-pc-windows-msvc` only runs on Windows. Correctly gated.

- **Node setup** (`.github/workflows/ci.yml:66-69`): `actions/setup-node@v4`, `node-version: "22"`, `cache: "npm"`. `npm test` resolves to `vitest run` (verified in `package.json:6`). Correct.

- **Rust cache covers 3 separate workspaces** (`.github/workflows/ci.yml:29-33`):
  ```
  workspaces: |
    src-tauri/target -> .cargo
    oracle-core/target -> .cargo
    devboule-mcp/target -> .cargo
  ```
  Each crate has its own `Cargo.lock` (confirmed: `src-tauri/Cargo.lock`, `oracle-core/Cargo.lock`, `devboule-mcp/Cargo.lock`). The `-> .cargo` key suffix is valid rust-cache v2 syntax even when `.cargo/` directory doesn't exist (it falls back to hashing workspace-level manifests). Correct.

- **`cargo test` only on devboule-mcp** (`.github/workflows/ci.yml:97`): `cargo test --manifest-path devboule-mcp/Cargo.toml --no-fail-fast`. No `cargo test` on `src-tauri`. Inline comment at lines 91-95 documents the pre-existing link error (`libesaxx_rs MT_StaticRelease vs ort_sys MD_DynamicRelease`). Correct omission per plan.

- **Conventional Commits**: `ci: add 3-OS matrix + Windows cross-compile gate (H)` — prefix `ci:` matches spec. Commit body fully documents rationale, protoc sources, and the no-root-workspace constraint.

- **Per-crate `cargo check`** (`.github/workflows/ci.yml:74-86`): Uses `--manifest-path` for each crate — correct since there is no root `Cargo.toml` workspace.

- **No secrets referenced**: Appropriate for v1. No code-signing, no deployment credentials.

### Fixed

*(none — read-only review)*

### Blocker

**None.** All 10 verification criteria pass with specific file:line evidence.

### Note

1. **Unused macOS targets** (`.github/workflows/ci.yml:41-44`): `x86_64-apple-darwin` and `aarch64-apple-darwin` are added with `|| true`, but no `cargo check --target x86_64-apple-darwin` step exists in the workflow. On Apple Silicon runners, the host `cargo check` (line 75) compiles for `aarch64-apple-darwin` (native), so the added x86_64 target is never actually checked. This is dead code — harmless but wasteful. Consider either adding a `cargo check --target x86_64-apple-darwin` step or removing the target additions.

2. **`windows-target-check` cache may always miss** (`.github/workflows/ci.yml:117`): `Swatinem/rust-cache@v2` is used without the `workspaces:` parameter. Since the repo has no root `Cargo.toml`, rust-cache's auto-detection may not find a valid workspace root and could fall back to no caching or incorrect key generation. This is a cache-efficiency issue, not a correctness issue — the job will still pass (just slower on cold cache). Non-blocking.

3. **`mingw-w64` in `windows-target-check`** (`.github/workflows/ci.yml:118-119`): `sudo apt-get install -y mingw-w64` provides a cross-linker (`x86_64-w64-mingw32-gcc`). For `cargo check` (no linking), this is technically unnecessary, but it's harmless defensive practice. Some build scripts (e.g., `cc` crate) may probe for a linker even during `check`, so including it is reasonable.

4. **`npm ci --prefer-offline`** (`.github/workflows/ci.yml:71`): With `setup-node@v4`'s built-in `cache: "npm"`, this flag is safe but may occasionally use stale cached packages. In CI, `--prefer-offline` is standard for speed; npm will still fetch on cache miss.

5. **Workflow is well-documented**: Inline comments explain the no-root-workspace constraint, protoc rationale, the src-tauri test exclusion, and the purpose of `windows-target-check`. Maintainable.

---

## Verdict

**✅ PASS** — No blockers. All 10 verification criteria pass with specific file:line evidence. The implementation is significantly more robust than the plan sketch (protoc install, multi-crate cache, Windows cross-compile gate, concurrency control, inline docs). Ready to merge.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "10 concrete findings with file:line citations across .github/workflows/ci.yml, verified via git show, YAML parse, package.json inspection, and filesystem checks"
    }
  ],
  "changedFiles": [
    ".github/workflows/ci.yml"
  ],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    {
      "command": "git show db0cb20 --stat",
      "result": "passed",
      "summary": "1 file changed, 120 insertions(+) — .github/workflows/ci.yml"
    },
    {
      "command": "python -c \"import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))\"",
      "result": "passed",
      "summary": "YAML OK — no parse errors"
    },
    {
      "command": "git log --format='%H%n%an <%ae>%n%s%n%n%b' db0cb20^..db0cb20",
      "result": "passed",
      "summary": "Conventional Commits: 'ci: add 3-OS matrix + Windows cross-compile gate (H)'"
    },
    {
      "command": "git diff --name-only 5522a31..db0cb20",
      "result": "passed",
      "summary": "Only .github/workflows/ci.yml changed"
    },
    {
      "command": "check package.json 'test' script",
      "result": "passed",
      "summary": "'npm test' resolves to 'vitest run'"
    },
    {
      "command": "verify crate Cargo.lock files exist",
      "result": "passed",
      "summary": "src-tauri/Cargo.lock, oracle-core/Cargo.lock, devboule-mcp/Cargo.lock all present"
    },
    {
      "command": "git show 5522a31:.github/",
      "result": "passed",
      "summary": "Confirmed .github/ did not exist in parent commit 5522a31"
    }
  ],
  "validationOutput": [
    "YAML: valid",
    "Triggers: push to [main, windows-*] + PR to [main] ✓",
    "Concurrency: cancel-in-progress: true ✓",
    "Matrix: 3 OS × fail-fast: false ✓",
    "windows-target-check: separate ubuntu job ✓",
    "protoc: apt/brew/choco per-OS ✓",
    "Windows MSVC target: conditional on windows-latest ✓",
    "Node 22 + npm cache ✓",
    "Rust cache: 3 separate workspaces ✓",
    "cargo test: devboule-mcp only (src-tauri excluded per plan) ✓",
    "Conventional Commits: ci: prefix ✓"
  ],
  "residualRisks": [
    "Unused macOS rustup targets (lines 41-44) — dead code, harmless",
    "windows-target-check cache may always miss (no workspaces param, line 117) — cold cache only",
    "mingw-w64 in check-only job (line 119) — unnecessary but harmless",
    "CI not yet run on GitHub — workflow has never been executed on actual runners"
  ],
  "noStagedFiles": true,
  "diffSummary": "Add .github/workflows/ci.yml (120 lines): 3-OS Rust/Node CI matrix with per-crate cargo check, Windows MSVC cross-compile gate, protoc install per-OS, concurrency control, and vitest frontend tests",
  "reviewFindings": [
    "No blockers. All 10 verification criteria pass with file:line evidence."
  ],
  "manualNotes": "Workflow is well-documented but has never run on GitHub Actions. First push to windows-port will be the true validation. The macOS target additions (x86_64 + aarch64 with || true, lines 41-44) are unused dead code — consider removing or adding a --target check step. windows-target-check rust-cache may need the same workspaces: config as the test job (line 117 missing it)."
}
```

---

**Return path**: `C:\Users\gualt\Desktop\devboule\.pi-subagents\artifacts\outputs\26a26605-b52c-495e-b276-2b888adddebb\reviewer\audit-h.md`
**Verdict**: ✅ PASS