# Task for scout

Inventory all **macOS-specific code and configuration** in the project at C:/Users/gualt/Desktop/devboule.

Distinguish and report these categories separately so we can reason about each:

**Category A — Compile-time macOS-only (truly macOS-only files):**
Files guarded by `#[cfg(target_os = "macos")]` (with no `not(target_os = "macos")` sibling) or whole-file wrappers like `#[cfg(any(target_os = "macos", target_os = "..."))]` where macOS is one of the only targets. List file path + 1-line purpose.

**Category B — Cross-platform files with macOS branches:**
Files that build on all platforms but contain `#[cfg(target_os = "macos")]`, `cfg!(target_os = "macos")`, `#[cfg_attr(target_os = "macos", ...)]`, runtime `if cfg!(target_os = "macos")` checks, or runtime OS checks (`std::env::consts::OS == "macos"`, `whoami::Platform::Mac`, etc.). For each branch: file:line range + what the macOS path does.

**Category C — macOS framework / crate dependencies:**
Anything in `Cargo.toml` files (root, src-tauri/, oracle-core/, devboule-mcp/) that conditionally pulls in macOS-only crates (e.g. `cocoa`, `objc`, `core-foundation`, `security-framework`, `apple-bundles`, `mac-notification-sys`, etc.). Note which crate is added and which `target_os = "macos"` guard gates it.

**Category D — macOS build & signing config:**
- `src-tauri/tauri.conf.json` macOS bundle settings (minimum system version, signing identity, entitlements, hardened runtime, category, file associations, URL schemes, framework targets)
- Entitlements files (*.entitlements) anywhere in the repo
- Any plist files (Info.plist, *.plist)
- notarization / stapling scripts under `scripts/` or `tools/`
- `.macos` / `Makefile` / shell scripts that target macOS specifically
- Github Actions / CI workflows that have macOS-only jobs (run-on: macos)

**Category E — shell scripts and tooling that target macOS only:**
Bash / sh / zsh scripts under `scripts/`, `tools/`, `*.sh` at any depth that contain `uname -Darwin` checks, `defaults write/read`, `osascript`, `xattr`, `codesign`, `xcrun`, `ditto`, hardcoded `.app` paths, etc. Note which are mac-only vs portable scripts with mac branches.

**Category F — Capacities / SecurityPolicy that apply differently on macOS:**
Anything in `src-tauri/capabilities/`, the `tauri.conf.json` `security` block, sandbox policy files (`.sb` ext / seatbelt profiles), or the seatbelt module under `src-tauri/src/backend/sandbox/` — call out what specifically differs on macOS vs other platforms.

**Category G — macOS-only test fixtures / mock files:**
Test files or `#[cfg(test)]` modules that only compile/run on macOS, or test data fixtures that exist solely for macOS testing.

**How to find these:**
- `rg -n 'target_os\s*=\s*"macos"' --type-add 'rust:*.rs' --type rust` (Rust cfg attributes)
- `rg -n 'cfg!\(.*target_os.*macos'`
- `rg -n 'cfg_attr\(target_os\s*=\s*"macos"'`
- `find . -name '*.entitlements' -o -name '*.plist' -o -name '*.sb'`
- `rg -n '\.app|/Library/|\.frameworks/|xattr|codesign|osascript|defaults write|launchctl|/usr/bin/osascript'`
- `rg -n 'uname -D|Darwin' scripts/ tools/'
- `rg -n 'cocoa|objc|core-foundation|security-framework|icrate' Cargo.toml src-tauri/Cargo.toml oracle-core/Cargo.toml devboule-mcp/Cargo.toml`
- `find src-tauri/capabilities -type f`
- `cat src-tauri/tauri.conf.json | jq '.bundle.macOS, .security'`

**Output format:**

For each category, a markdown subsection with:
- Numbered list of findings
- For each finding: **file path**, **what it does**, **why it's macOS-specific** (1 sentence)
- A short verdict per file (true-mac-only vs cross-platform-with-mac-branch)
- Use absolute paths or repo-relative paths consistently

End with a **summary table**:
| File | Category | Why macOS-specific | Verdict (mac-only / cross-with-branch) |

And a **top-level answer** to: "Which files would a Windows or Linux developer never need to touch?" — give the minimal list.

Be concrete. file:line refs. No code edits. Read-only. Save to your output artifact.

---
**Output:**
Write your findings to exactly this path: C:\Users\gualt\Desktop\devboule\.pi-subagents\artifacts\outputs\a0e431f5-f53a-4edc-b7fc-b0deb9b4e0ca\context.md
This path is authoritative for this run.
Ignore any other output filename or output path mentioned elsewhere, including output destinations in the base agent prompt, system prompt, or task instructions.

## Acceptance Contract
Acceptance level: attested
Completion is not accepted from prose alone. End with a structured acceptance report.

Criteria:
- criterion-1: Return concrete findings with file paths and severity when applicable

Required evidence: review-findings, residual-risks

Finish with a fenced JSON block tagged `acceptance-report` in this shape:
Use empty arrays when no items apply; array fields contain strings unless object entries are shown.
`criteriaSatisfied[].status` must be exactly one of: satisfied, not-satisfied, not-applicable.
`commandsRun[].result` must be exactly one of: passed, failed, not-run.
`manualNotes` and `notes` are optional strings; an empty string means no note and does not satisfy `manual-notes` evidence.
```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "specific proof"
    }
  ],
  "changedFiles": [
    "src/file.ts"
  ],
  "testsAddedOrUpdated": [
    "test/file.test.ts"
  ],
  "commandsRun": [
    {
      "command": "command",
      "result": "passed",
      "summary": "short result"
    }
  ],
  "validationOutput": [
    "validation output or concise summary"
  ],
  "residualRisks": [
    "none"
  ],
  "noStagedFiles": true,
  "diffSummary": "short description of the diff",
  "reviewFindings": [
    "blocker: file.ts:12 - issue found, or no blockers"
  ],
  "manualNotes": "anything else the parent should know"
}
```