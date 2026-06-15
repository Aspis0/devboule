# P5 — Sandbox the mini (Seatbelt + rlimits) [macOS] — IMPLEMENTATION SPEC

> Drafted 2026-06-14 (architect pass + owner decision). Scope for `veteran-coder`. Pairs with the master plan P5 (`docs/master-plan-2026-06-self-improving-mini-design.md`). All code edits in `src-tauri/src/backend/mini_coder_executor.rs` unless noted.

## Decisions (locked — do not relitigate)
- **MECHANISM = `/usr/bin/sandbox-exec`** (built-in, present) with a Seatbelt/SBPL profile we generate, written to a per-launch restricted temp `.sb` file and passed as `-f <file>`. NOT Anthropic `srt` (not installed; it's just an npm wrapper around `sandbox-exec` + a net proxy we don't need; requiring it would violate PRODUCT GENERALITY). rlimits = POSIX `ulimit` in the sh preamble (the `night_run.py:170` pattern), not a Seatbelt feature.
- **SCOPE = LOCAL-LOOPBACK BACKENDS ONLY (owner decision 2026-06-14).** Apply the sandbox-exec wrap + rlimits **only** when the backend kind ∈ {oMLX, ollama, AppleFm} **AND** the resolved base_url host is loopback (`127.0.0.1` / `localhost` / `::1`). In **every other case** — `codex`, `api`, or a local-kind backend pointed at a NON-loopback URL — the macOS spawn path stays **byte-for-byte unchanged** (no sandbox, no rlimit change). Codex confinement (it needs remote-API egress) is a SEPARATE future phase (net-proxy). This keeps the change low-risk and the codex path identical to today.
- **`enable_thinking` / cache:** unaffected by P5.
- **GPU CONSTRAINT:** another session is training on the GPU (100% saturated — see memory `concurrent-training-gpu-rule`). Do all verification GPU-free: `cargo test --lib` (CPU) + the manual Seatbelt-mechanic checks below use a DUMMY command (no oMLX/Metal). The live "oMLX mini writes through the sandbox" e2e is **DEFERRED until the GPU is free**.

## Code ground truth (architect-verified; plan line numbers were stale)
| Concern | Location in `mini_coder_executor.rs` |
|---|---|
| macOS script assembled | `build_mini_command_impl` macOS arm `~:3272-3435` |
| Trap + `set -e` preamble | `build_macos_trap_preamble` `~:3115-3132` |
| oMLX python heredoc | `OMLX_RUN_MACOS_PY` `~:3169` + `build_omlx_run_macos` `~:3213` |
| stdout→result wrapper | `macos_stdout_to_result_wrapper` `~:3490` |
| **THE single spawn site** | `~:3424-3434` — `CommandBuilder::new("/bin/sh"); cmd.args(["-c", &script]); cmd.cwd(project_root)` |
| temp-file helper to reuse | `projects::write_restricted_prompt_file` / `remove_restricted_temp_file` (used `~:2680/2688`) |
| spawn-failure cleanup | `remove_mini_temp_files` `~:2373`; `MiniCommandBuild` struct `~:2662` |
| Windows arm (DO NOT TOUCH) | `~:2739` (`#[cfg(windows)]`) |
| Linux/other arm | `~:3509` returns `Err` (add TODO comment only) |

## Edits

### 1. New `build_seatbelt_profile` (pure, uncfg'd + `#[cfg_attr(all(not(target_os="macos"),not(test)), allow(dead_code))]` like `build_macos_trap_preamble`)
```
fn build_seatbelt_profile(project_root: &Path, writable_paths: &[PathBuf]) -> String
```
Emits the TIGHT loopback profile (only kind needed this phase — child does HTTP + prints JSON; Rust applies edits per P4, so it needs NO project-file write access). **Validated against the real `/usr/bin/sandbox-exec` parser on macOS 26.5.1** (test `seatbelt_profile_accepted_by_real_sandbox_exec`) — a string-contains test cannot catch a profile the kernel rejects. SBPL:
```scheme
(version 1)
(deny default)

; reads broad — a SUBPATH-FILTERED file-read* makes /bin/sh SIGABRT before exec: the dyld
; SHARED CACHE lives on a separate Preboot/Cryptexes APFS volume that `(subpath "/System")`
; does NOT traverse (empirically verified vs sandbox-exec on macOS 26.5.1). So reads are FULLY
; broad; the security boundary is the WRITES + the NETWORK, NOT the reads. @PROJECT_ROOT@ is
; readable here and ABSENT from file-write* => read-only.
(allow file-read*)
(allow file-read-metadata)
(allow sysctl-read)
(allow mach-lookup)

; writes deny-by-default; ONLY the parameterized scratch/temp set (NO project files on the emit-edits path)
; NOTE: /private/var/folders is readable via the broad rule — NOT writable (would grant other sessions' caches)
(allow file-write*
    (literal "/dev/null")
    (subpath "@TMPDIR@")
    @WRITABLE_SUBPATHS@)

; exec: sh + python3. Allow the standard interpreter dirs so PATH-resolved python3 matches
; /opt/homebrew (NOT /opt/homebrew/bin): Seatbelt checks the SYMLINK-RESOLVED real binary
; (/opt/homebrew/Cellar/python@3.x/.../python3.x), so the grant must cover the whole prefix.
(allow process-exec
    (literal "/bin/sh")
    (subpath "/usr/bin") (subpath "/bin")
    (subpath "/opt/homebrew") (subpath "/usr/local/bin"))
(allow process-fork)

; network: deny all, allow loopback only (oMLX base_url host:port is user-configurable -> NEVER hardcode :8000)
; `remote tcp/udp "localhost:*"` is the ONLY valid loopback SBPL — `remote ip "127.0.0.1:*"` /
; `local ip ...` are REJECTED by sandbox-exec ("host must be * or localhost"). The kernel matches
; "localhost" for both 127.0.0.1 and ::1; an external IP (e.g. 8.8.8.8) stays denied. No bind (outbound only).
(deny network*)
(allow network-outbound
    (remote tcp "localhost:*")
    (remote udp "localhost:*"))
```
- `@WRITABLE_SUBPATHS@` = one `(subpath "<canonical abs>")` per `writable_paths`. Canonicalize each (reuse the P4 canonicalize logic `~:1727-1809`); escape `"` and `\` for SBPL string literals.
- `@PROJECT_ROOT@` / `@TMPDIR@` substituted with canonical abs paths.

### 2. `build_mini_command_impl` macOS arm (`~:3272-3435`)
- Add a guard: `let sandboxed = matches!(kind, oMLX|ollama|AppleFm) && base_url_host_is_loopback(&base_url);` (write a small `base_url_host_is_loopback(&str) -> bool` helper: parse host, true for `127.0.0.1`/`localhost`/`::1`/`[::1]`). If NOT `sandboxed` → leave the existing spawn path **exactly as today** (early-return the unchanged build).
- If `sandboxed`: compute `writable_paths` = `[ scratch_root (.aspis-mini), prompt_dir, raw_path parent, key_dir (if Some), the .sb profile's dir, TMPDIR ]` (emit-edits → NO `directive.files`). Call `build_seatbelt_profile(project_root, &writable_paths)`; write it via `write_restricted_prompt_file` to a `.sb` temp; change the spawn at `~:3425-3426` to:
  `CommandBuilder::new("/usr/bin/sandbox-exec"); cmd.args(["-f", &profile_path, "/bin/sh", "-c", &script]); cmd.cwd(project_root)`.
- Return the profile path for cleanup (thread through).

### 3. rlimits in `build_macos_trap_preamble` — ONLY on the sandboxed path
Between `trap … EXIT` and `set -e` (trap first so cleanup always runs):
```sh
ulimit -t 600 2>/dev/null || true     # CPU seconds (reuse the wall-clock cap const)
ulimit -v 4194304 2>/dev/null || true # ~4 GiB addr space (python urllib is tiny; safe on this TIGHT path)
ulimit -u 256 2>/dev/null || true     # max procs — fork-bomb guard
```
- Gate these on `sandboxed` (param the preamble, or emit a second `build_macos_rlimit_preamble()` concatenated only when sandboxed) so the codex/api/non-loopback path stays byte-identical.
- Keep the existing invariant `trap_idx < set_e_idx`; add `trap_idx < ulimit_idx < set_e_idx`.
- Each `|| true` so a rejected limit doesn't abort under `set -e`.

### 4. Cleanup plumbing
- `MiniCommandBuild` (`~:2662`): add `profile_file: Option<PathBuf>`.
- EXIT trap (`build_macos_trap_preamble`): also `rm -f` the `.sb` (read once at sandbox-exec parse time before the trap fires — safe). Mirror `key_dir` handling.
- `remove_mini_temp_files` (`~:2373`) + the `spawn_one_shot_mini` failure arms (`~:2353/2361`): also remove the profile file (spawn-failure path).

### 5. cfg invariants
- All new logic inside `#[cfg(target_os="macos")]`. The `#[cfg(windows)]` arm untouched (assert via existing Windows string tests staying green).
- Linux arm (`~:3509`) unchanged; add `// TODO: Linux sandbox = bubblewrap/landlock when the Linux mini arm lands`.

## Tests (GPU-free `cargo test --lib`)
1. `seatbelt_profile_version1_deny_default` — starts `(version 1)`, contains `(deny default)`.
2. `seatbelt_profile_writes_only_parameterized_paths` — each writable path present under `file-write*`; an unrelated path absent; reads are BROAD (`(allow file-read*)` — a filtered read aborts /bin/sh via dyld) so the project root is readable, and it is ABSENT under `file-write*` (emit-edits → no project write); `/private/var/folders` ABSENT from `file-write*`.
3. `seatbelt_profile_loopback_only_no_hardcoded_8000` — contains `(remote tcp "localhost:*")`; asserts `!profile.contains("remote ip")` (invalid SBPL) and `!profile.contains(":8000")`; net is loopback-only (`(deny network*)` present, no `(allow network*)`).
4. `seatbelt_profile_exec_allows_sh_and_python_dirs` — exec dirs include `/opt/homebrew` (NOT the narrow `/opt/homebrew/bin`).
5. `macos_local_backend_wraps_with_sandbox_exec` — oMLX + loopback base_url ⇒ argv[0] == `/usr/bin/sandbox-exec`, contains `-f`, a `.sb` path, then `/bin/sh -c`. (Adapt the macOS-arm tests `~:5877`.)
6. `macos_codex_path_unchanged_no_sandbox` — codex kind ⇒ spawn is `/bin/sh` (NO `sandbox-exec`), byte-identical to today.
7. `macos_local_backend_nonloopback_url_not_sandboxed` — oMLX with a remote base_url ⇒ NOT wrapped (unchanged path).
8. `rlimit_preamble_order_when_sandboxed` — `trap` < `ulimit -u` < `set -e`; the 3 ulimit lines present with `|| true`; and ABSENT on the non-sandboxed path.
9. `windows_mini_command_unchanged` — Windows script still spawns powershell, contains NO `sandbox-exec` (extend existing).
10. `seatbelt_profile_accepted_by_real_sandbox_exec` (macOS, GPU-free) — feeds the generated profile to the REAL `/usr/bin/sandbox-exec` and asserts: (a) `echo ok` runs (kernel ACCEPTS the profile — catches an invalid SBPL token that would abort exit 65); (b) `python3 -c 'print(1)'` runs if python3 is resolvable, else SKIP; (c) a write into the read-only project_root is DENIED + the file does not exist; (d) a write into the granted scratch dir SUCCEEDS. This is the regression guard a string-contains test cannot provide.

## Manual GPU-free Seatbelt-mechanic checks (do when convenient; DUMMY command, no oMLX)
Generate a profile from `build_seatbelt_profile` and run `sandbox-exec -f <gen.sb> /bin/sh -c '<dummy>'`:
1. out-of-scope write (`echo x > ~/outside.txt`) → BLOCKED (`Operation not permitted`).
2. in-scope write (into `.aspis-mini`) → OK.
3. external net (`python3 -c "urllib...urlopen('http://1.1.1.1',timeout=3)"`) → BLOCKED.
4. loopback (`python3 -m http.server 12345` outside; curl `127.0.0.1:12345` inside) → OK.
5. fork-bomb under `ulimit -u 64` → bounded (`Resource temporarily unavailable`), host survives.

## Open risks (carried, not blocking)
- **A (deferred by owner):** codex/api NOT sandboxed this phase — net-proxy confinement is a future phase.
- **B:** `ulimit -v 4 GiB` safe on the python-urllib TIGHT path; if a future writable-local backend needs more, make it a param.
- **C:** "tests run in the sandbox" (P4 note) is UNBUILT — nothing executes the project test suite today; Censor static-analysis runners run OUTSIDE the sh sandbox (Rust `Command::new`). Both are separate phases. Leave a `// TODO(P5-followup)` noting (a) test execution unimplemented, (b) Censor runners unsandboxed.
- **D:** SBPL is Apple-undocumented/deprecated but is exactly what `srt`/`sandbox-exec` consume — acceptable, documented.
