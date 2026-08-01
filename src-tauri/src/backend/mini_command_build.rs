//! Mini-coder COMMAND BUILD — the one-shot PTY command line per backend/OS
//! (ollama/api/omlx/apple-fm x windows/macos/other), the stdout->result wrappers,
//! the loopback gates, the ulimit/rlimit cage consts and the macOS Seatbelt
//! profile for the sandboxed one-shot spawn. Extracted VERBATIM from
//! `mini_coder_executor.rs` (role-untangle Phase 2, pure move). The dense test
//! battery (incl. the real-kernel-parser Seatbelt regression test) stays in
//! `mini_coder_executor.rs` via its wildcard re-export.

use std::path::{Path, PathBuf};

use portable_pty::CommandBuilder;

use super::mini_coder::{MiniCoderBackend, MiniCoderBackendKind, DEFAULT_WALL_CLOCK_CAP_SECS};
use super::mini_coder_executor::{
    omlx_http_timeout_secs, McpRoots, FORBIDDEN_USER_MCP_ENV, MINI_SCRATCH_DIR,
    OMLX_TIMEOUT_ENV,
};
#[cfg(windows)]
use super::projects::ps_single_quote;

/// LOCAL-MODEL LATENCY FIX 2 — hard generation budget (tokens) for the oMLX path.
/// This is the ACTUAL runaway guard: the mini POSTs `stream:false` to the
/// mlx-lm/oMLX OpenAI-compatible server, which runs its OWN decode loop and does
/// NOT stop on EOS by default — a reasoning model with the known repetition bug
/// otherwise runs to the server's default max (minutes, or effectively forever).
/// There is no Rust-side token loop to add an EOS-break to, so the cap must ride
/// IN the request body. mlx_lm.server reads it as `max_tokens` (its fallback for
/// `max_completion_tokens`).
///
/// This budget INCLUDES thinking tokens. On the FIX pass thinking is ON, so the
/// budget must hold the `<think>` CoT PLUS the emit-edits JSON answer that follows
/// it. 6144 is a deliberate mid-point of the 4096–8192 range: 4096 risks
/// truncating a legitimate think-then-answer on a non-trivial file (FIX 2 must NOT
/// break correct outputs), while 8192 leaves the runaway window large. 6144 keeps
/// thinking + a moderate JSON answer roomy while bounding the worst-case
/// repetition runaway to a few minutes (vs. unbounded) on both the ~60 tok/s MoE
/// and the ~14.5 tok/s dense model. A constant default (not a settings knob) per
/// the master-plan scope rule.
pub(crate) const OMLX_MAX_TOKENS_DEFAULT: u32 = 6144;

/// LOCAL-MODEL LATENCY FIX 2 — repetition penalty for the oMLX path. The Gemma4
/// repetition bug is the proximate cause of the decode runaway; a mild penalty
/// damps the degenerate loop directly (in addition to the `max_tokens` backstop).
/// Confirmed accepted by mlx_lm.server as the body field `repetition_penalty`
/// (`self.body.get("repetition_penalty", 0.0)`). 1.1 is the conventional safe
/// value: 1.0 is off, >1.2 starts degrading quality. Sent on BOTH passes.
pub(crate) const OMLX_REPETITION_PENALTY: &str = "1.1";

/// P5 (macOS sandbox) — POSIX `ulimit` rlimits applied in the `/bin/sh` preamble ON THE
/// SANDBOXED LOCAL-LOOPBACK PATH ONLY (oMLX/ollama/AppleFm on a loopback endpoint). They
/// are belt-and-suspenders alongside the Seatbelt profile: a defense-in-depth resource
/// cage on the python-urllib TIGHT path (the child does HTTP + prints JSON; Rust applies
/// edits per P4). Each is emitted as `ulimit -X N 2>/dev/null || true` so a kernel-rejected
/// limit (already lower, or unsupported) never aborts the script under `set -e`.
///
/// `ulimit -t` is a CPU-TIME cap (RLIMIT_CPU — seconds the process spends ON-CPU), NOT a
/// wall-clock cap: a child blocked in the HTTP wait accrues ~no CPU time, so `ulimit -t`
/// would never fire on a stalled-network hang. The WALL-CLOCK enforcer is the out-of-band
/// PTY kill (the executor kills the PTY after [`DEFAULT_WALL_CLOCK_CAP_SECS`]); `ulimit -t`
/// only bounds a CPU-BOUND runaway (a busy-loop) as defense-in-depth. We REUSE the same
/// [`DEFAULT_WALL_CLOCK_CAP_SECS`] value for the CPU cap so the in-shell CPU budget and the
/// PTY wall-clock kill derive from ONE source and never silently diverge — but the two cap
/// DIFFERENT things (on-CPU seconds vs. real elapsed time).
///
/// Address-space cap ~= 4 GiB (in KiB, the `ulimit -v` unit). python's stdlib urllib POST
/// + JSON parse is a few MiB; 4 GiB is generous headroom that still bounds a runaway
/// allocation. Open risk B: if a future writable-local backend needs more, make it a param.
pub(crate) const MINI_RLIMIT_ADDRESS_SPACE_KIB: u64 = 4 * 1024 * 1024;
/// Max user processes — a fork-bomb guard for the sandboxed child. 256 is ample for
/// `sh` + `python3` (+ any short-lived helper) while bounding a runaway fork loop.
pub(crate) const MINI_RLIMIT_MAX_PROCS: u64 = 256;

/// Build the per-kind one-shot command. Returns a [`MiniCommandBuild`] carrying the
/// `CommandBuilder` AND the restricted temp-file paths the caller must clean up if the
/// SPAWN itself fails (the in-script wrapper/trap never ran to delete them):
///   - `prompt_file`: the 0600 temp file holding the prompt (delivered over STDIN, never
///     argv); the wrapper reads it, DELETES it, then pipes it to the backend.
///
/// Per kind:
///   - codex: `codex exec` (prompt over stdin, `-m <model>` if set). The mini
///     WRITES its result JSON to `resultPath` itself. MINOR 9: a mini gets NO MCP
///     grant (see below) — it works from the front-loaded prompt context only.
///   - ollama: `ollama run <model>` (prompt over stdin; text-only, no tools). The
///     wrapper captures the model's stdout and normalizes it into `resultPath`.
///   - api: the configured CLI `command` (prompt over stdin). Same stdout->file
///     wrapper as ollama. The API key comes from the CLI's own ENV, never argv.
///
/// MINOR 9 → P3 (security scope): the read-only scope now EXISTS. A codex mini
/// whose directive granted the oracle gets the SAME `-c mcp_servers.*` tokens as
/// full coders (shared builder, no drift), and the narrowing is SERVER-side: it
/// can only register as role "mini" (launch-token-bound), whose allowed tools
/// are {agent_register, oracle_context} — mutation tools are rejected at the
/// MCP role gate. No grant, or any text-only backend ⇒ NO flags, front-loaded
/// prompt context only (the MINOR 9 status quo, byte-identical).
/// The result of [`build_mini_command`]: the launch command plus the restricted temp
/// files the SPAWN caller must clean up on a spawn failure (the in-script cleanup never
/// ran). `prompt_file` is always present once built; `profile_file` is `Option` because
/// only the sandboxed macOS local-loopback path produces a `.sb` temp.
pub(crate) struct MiniCommandBuild {
    pub(crate) prompt_file: Option<PathBuf>,
    /// P5: the per-launch Seatbelt `.sb` profile temp, present ONLY on the sandboxed
    /// local-loopback macOS path. The in-script EXIT trap removes it on success/abort; the
    /// SPAWN caller removes it (via `remove_mini_temp_files`) on a spawn failure (where the
    /// script never ran). `None` on every non-Seatbelt path (codex/api/
    /// non-loopback; Windows is broker-sandboxed at spawn time, not via a
    /// profile — is_enforced() == true since C6).
    pub(crate) profile_file: Option<PathBuf>,
    /// C6: inspectable launch components (the PTY path on Windows spawns through
    /// the AppContainer broker, which cannot read a portable_pty CommandBuilder).
    pub(crate) command: crate::backend::agent_pty::PtyCommand,
}

pub(crate) fn build_mini_command(
    backend: &MiniCoderBackend,
    project_root: &Path,
    result_target: &Path,
    prompt: &str,
    mcp_roots: Option<&McpRoots>,
    // P6: thinking ON for fix passes (attempt > 0), OFF for initial writes.
    fix_pass_thinking: bool,
) -> Result<MiniCommandBuild, String> {
    // The prompt goes to a restricted temp file (0600). It is NOT a secret, but
    // keeping it off argv matches the agent-launch contract and avoids argv-length
    // / quoting issues with large multi-file prompts.
    let prompt_file = super::projects::write_restricted_prompt_file(prompt)?;
    // MINOR 9 → P3: the roots now flow through. Only the codex arms consume them
    // (ollama/api/omlx are text-only and ignore the parameter), and the caller
    // only resolves roots for `allow_oracle` codex directives, so a text-only or
    // no-grant mini still builds a byte-identical command.
    let cmd = build_mini_command_impl(
        backend,
        project_root,
        result_target,
        &prompt_file,
        mcp_roots,
        fix_pass_thinking,
    );
    match cmd {
        Ok((command, profile_file)) => {
            let mut pty_cmd = crate::backend::agent_pty::PtyCommand::from_command_builder(&command);
            // C6: the mini script reads the prompt from a user-only %TEMP% dir
            // and may run git (session gitconfig + real config includes) — the
            // AppContainer child needs all of these as read roots (Windows).
            // macOS seatbelt allows broad reads, so this is a no-op there.
            #[cfg(target_os = "windows")]
            for root in super::agent_spawn::agent_sandbox_read_roots(Some(&prompt_file)) {
                pty_cmd = pty_cmd.read_root(root);
            }
            Ok(MiniCommandBuild {
                prompt_file: Some(prompt_file),
                profile_file,
                command: pty_cmd,
            })
        }
        Err(e) => {
            super::projects::remove_restricted_temp_file(&prompt_file);
            Err(e)
        }
    }
}


#[cfg(windows)]
pub(crate) fn build_mini_command_impl(
    backend: &MiniCoderBackend,
    project_root: &Path,
    result_target: &Path,
    prompt_file: &Path,
    mcp_roots: Option<&McpRoots>,
    fix_pass_thinking: bool,
) -> Result<(CommandBuilder, Option<PathBuf>), String> {
    let prompt_path = ps_single_quote(&prompt_file.to_string_lossy());
    let result_path = ps_single_quote(&result_target.to_string_lossy());
    // WARNING 7: a sibling temp file for the backend's RAW stdout, so we never hold
    // the (potentially huge) output in a PowerShell string. It lives next to the
    // result file inside the same scratch dir and is removed in the `finally`. The
    // `.raw` suffix is on the directive's result path so it stays under the scratch
    // root (the result path was traversal-validated by claim_and_launch).
    let raw_path = ps_single_quote(&format!("{}.raw", result_target.to_string_lossy()));

    // FIX 1 (source-content leak): define the prompt file / its restricted parent dir
    // / the raw capture path BEFORE the try, then read the prompt INSIDE the try so a
    // failing `Get-Content` (ErrorActionPreference=Stop) can no longer skip cleanup.
    // The `finally` ALWAYS removes the source-bearing prompt dir AND the `.raw`
    // capture, on success OR on any error in the body. NEVER `Write-Host $prompt`
    // (B1: no prompt on the PTY stream).
    let preamble = format!(
        "$ErrorActionPreference='Stop'\n\
$promptFile = {prompt_path}\n\
$promptDir = [System.IO.Path]::GetDirectoryName($promptFile)\n\
$rawFile = {raw_path}\n\
try {{\n\
$prompt = Get-Content -Raw -LiteralPath $promptFile\n"
    );

    let body = match backend.kind {
        MiniCoderBackendKind::Codex => {
            // codex exec: prompt piped over stdin (read from `-`), -m if set. The mini
            // WRITES the result file itself. P3: with the oracle grant the shared
            // `-c mcp_servers.*` tokens ride along (server-side "mini" role narrowing);
            // no grant ⇒ no flags ⇒ byte-identical to the MINOR 9 status quo.
            let mut args: Vec<String> = vec!["exec".to_string()];
            if let Some(roots) = mcp_roots {
                let app_bin = super::projects::resolve_app_binary();
                let app_bin = app_bin.as_ref().map(|p| p.to_string_lossy().into_owned());
                args.extend(super::projects::codex_mcp_config_args(
                    &crate::oracle::oracle_setup::resolve_oracle_python(),
                    &roots.management_root,
                    &roots.projects_dir,
                    app_bin.as_deref(),
                    // MINI-EXCLUSION (design §6, HARD): the mini NEVER receives user MCP
                    // servers. It reuses this shared Oracle-only builder and passes an
                    // EMPTY slice, so its codex `-c` flags stay Oracle-only (narrowed
                    // server-side by role "mini"). This bare empty literal names no
                    // user-MCP type, so this file keeps ZERO references to the user-MCP
                    // config code.
                    &[],
                )?);
            }
            if let Some(model) = backend.model.as_deref() {
                if !model.trim().is_empty() {
                    args.push("-m".to_string());
                    args.push(model.trim().to_string());
                }
            }
            let arg_list = args
                .iter()
                .map(|a| ps_single_quote(a))
                .collect::<Vec<_>>()
                .join(", ");
            // `$prompt | & codex @codexArgs`: prompt on STDIN, never argv.
            format!("$codexArgs = @({arg_list})\n$prompt | & codex @codexArgs\n")
        }
        MiniCoderBackendKind::Ollama => {
            let model = backend
                .model
                .as_deref()
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .ok_or_else(|| "ollama backend requires a model tag".to_string())?;
            let model_q = ps_single_quote(model);
            // `$prompt | & ollama run <model>`: prompt on STDIN. `& ollama run` uses
            // the call operator because the executable + args are OUR fixed tokens
            // (no operator-supplied command line to tokenize). Capture stdout into the
            // result file via the shared wrapper.
            let run = format!("$prompt | & ollama run {model_q}");
            windows_stdout_to_result_wrapper(&run, &result_path, &raw_path)
        }
        MiniCoderBackendKind::Api => {
            let command = backend
                .command
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .ok_or_else(|| "api backend requires a command line".to_string())?;
            // BLOCKER 1 / WARNING 5: the `command` is a TRUSTED, operator-configured
            // shell command LINE — the same trust model as a `customAgentClients`
            // command (see projects::build_windows_agent_script's custom branch). We
            // therefore interpolate it VERBATIM as a pipeline target WITHOUT the `&`
            // call operator, so PowerShell tokenizes the whole line itself
            // (`mycli chat --json` runs `mycli` with args `chat --json`). Using
            // `& {command}` would treat the entire multi-word string as a single
            // executable NAME and fail. The prompt is piped over stdin; the API key
            // comes from the CLI's OWN env — never injected by us, never on argv.
            let run = format!("$prompt | {command}");
            windows_stdout_to_result_wrapper(&run, &result_path, &raw_path)
        }
        MiniCoderBackendKind::Omlx => {
            // oMLX-P2: the one-shot script POSTs an OpenAI chat-completion to the
            // loopback oMLX server ITSELF (Invoke-RestMethod), emits the model's text
            // on stdout, and the EXISTING wrapper extracts the MiniCoderResult JSON —
            // exactly as for ollama/api (Option A: keep the PTY). model + base_url are
            // REQUIRED (validated in oMLX-P1; re-checked here so a hand-edited config
            // fails cleanly instead of building a bad request).
            let model = backend
                .model
                .as_deref()
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .ok_or_else(|| "omlx backend requires a model".to_string())?;
            let base_url = backend
                .base_url
                .as_deref()
                .map(str::trim)
                .filter(|b| !b.is_empty())
                .ok_or_else(|| "omlx backend requires a base URL".to_string())?;
            let run = build_omlx_run_windows(base_url, model, fix_pass_thinking);
            windows_stdout_to_result_wrapper(&run, &result_path, &raw_path)
        }
        MiniCoderBackendKind::AppleFm => {
            return Err("Apple on-device requires macOS 27+.".to_string());
        }
        MiniCoderBackendKind::Openai => {
            return Err(
                "OpenAI backend runs via the api/cli bridge, not the directive executor"
                    .to_string(),
            );
        }
        MiniCoderBackendKind::Cloud => {
            // Cloud is a pi-engine backend (HTTPS remote provider); the directive
            // executor's one-shot PTY/script path cannot drive it today. Fail
            // LOUDLY here rather than silently downgrading (so a misconfigured
            // spawn is diagnosed, not hidden behind a `done` from the wrong
            // backend). The pi sidecar's `map_mini_coder_backend_to_sidecar_env`
            // handles Cloud correctly.
            return Err(
                "cloud backend runs via the pi engine; the directive executor does not \
                 support it yet"
                    .to_string(),
            );
        }
    };

    // FIX 1: close the try opened in the preamble and ALWAYS run cleanup in the
    // `finally` — the source-bearing prompt dir AND the `.raw` capture are removed on
    // success OR any error (so a failed Get-Content / backend can no longer leak the
    // restricted prompt file on disk). SilentlyContinue: an already-removed file is fine.
    //
    // F5: codex does NOT use `windows_stdout_to_result_wrapper`, so it never writes the
    // `.raw` file; guard the removal with `Test-Path` so the cleanup targets a file that
    // actually exists (the wrapper backends still get their raw capture removed).
    let finally = format!(
        "}}\n\
finally {{\n\
  Remove-Item -LiteralPath $promptDir -Recurse -Force -ErrorAction SilentlyContinue\n\
  if (Test-Path -LiteralPath $rawFile) {{ Remove-Item -LiteralPath $rawFile -Force -ErrorAction SilentlyContinue }}\n\
}}\n"
    );
    let script = format!("{preamble}{body}{finally}exit 0");
    let mut cmd = CommandBuilder::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]);
    cmd.cwd(project_root);
    // MINI-EXCLUSION (design §6): scrub the orchestrator-only user-MCP env var so the mini
    // child can NEVER inherit it from the host process env (CommandBuilder snapshots it).
    cmd.env_remove(FORBIDDEN_USER_MCP_ENV);
    // P5 (updated C6): on macOS a `.sb` Seatbelt profile wraps the command; on
    // Windows the AppContainer broker (spawn_sandboxed) provides the sandbox
    // at spawn time (is_enforced()==true since C6), so this builder emits no
    // profile — the script/argv are byte-for-byte unchanged.
    Ok((cmd, None))
}

/// oMLX-P2 (Windows): build the `$run` pipeline that POSTs an OpenAI chat-completion
/// to the loopback oMLX server and writes the model's answer to stdout, which the
/// EXISTING `windows_stdout_to_result_wrapper` then extracts into a `MiniCoderResult`.
///
/// INJECTION-SAFETY (critical): the prompt is the `$prompt` PowerShell variable read
/// from the restricted file in the preamble; it is passed as a VALUE into a hashtable
/// and JSON-encoded by `ConvertTo-Json`. It is NEVER string-concatenated into the JSON
/// body, so no prompt content can break out of the JSON string and forge fields.
///
/// `model` and `base_url` are OUR tokens (validated in oMLX-P1: model is a bare tag,
/// base_url is a normalized loopback origin), embedded single-quoted via
/// `ps_single_quote`. `base_url` already has any trailing slash stripped (P1), so
/// `<base>/chat/completions` never double-slashes.
///
/// FAILURE = SILENCE: the whole request is wrapped in `try { … } catch { }` so ANY
/// connection/HTTP/parse error writes NOTHING to stdout. The wrapper then finds no
/// valid JSON and writes the clean `{"status":"failed",...}` fallback — a non-2xx
/// response (Invoke-RestMethod throws) yields the SAME clean fallback, never partial
/// garbage in the result file.
///
#[cfg(windows)]
pub(crate) fn build_omlx_run_windows(
    base_url: &str,
    model: &str,
    fix_pass_thinking: bool,
) -> String {
    // P6: $true on fix passes, $false on initial writes (Qwen-only, gated below).
    let thinking_ps = if fix_pass_thinking { "$true" } else { "$false" };
    // FIX 2: bound the decode — a hard token budget (includes thinking) plus a mild
    // repetition penalty, the only runaway guards on this stream:false path. Both ride
    // the body via ConvertTo-Json (never string-concatenated). PowerShell numeric
    // literals: an integer for max_tokens, a decimal for the penalty.
    let max_tokens = OMLX_MAX_TOKENS_DEFAULT;
    let rep_penalty = OMLX_REPETITION_PENALTY;
    let model_q = ps_single_quote(model);
    let uri_q = ps_single_quote(&format!("{base_url}/chat/completions"));
    // F3: cap the HTTP request so a stalled oMLX server fails fast (Invoke-RestMethod
    // throws on timeout -> the try/catch swallows it -> clean `failed` fallback) instead
    // of holding the PTY until the wall-clock kill. Derived from the SAME constant as the
    // macOS python timeout (wall-clock cap minus a margin).
    let http_timeout = omlx_http_timeout_secs();
    // The prompt rides as a VALUE (`content = $prompt`) — ConvertTo-Json encodes it;
    // NEVER `'\"content\":\"' + $prompt`. -Compress keeps the body one line.
    //
    // The whole try/catch is wrapped in a `& { … }` script block so that the shared
    // `windows_stdout_to_result_wrapper`'s `{run} > $rawFile` redirects the ENTIRE
    // block's output stream (the `Write-Output $content`) — not just the last
    // statement. This keeps the wrapper UNCHANGED (same idiom as the single-pipeline
    // ollama/api `$run`).
    format!(
        "& {{\n\
try {{\n\
$headers = @{{}}\n\
$bodyMap = @{{ model = {model_q}; messages = @(@{{ role = 'user'; content = $prompt }}); stream = $false; temperature = 0.1; max_tokens = {max_tokens}; repetition_penalty = {rep_penalty} }}\n\
if ({model_q} -match 'qwen') {{ $bodyMap['chat_template_kwargs'] = @{{ enable_thinking = {thinking_ps} }} }}\n\
$body = $bodyMap | ConvertTo-Json -Depth 6 -Compress\n\
$resp = Invoke-RestMethod -Method Post -Uri {uri_q} -ContentType 'application/json' -Headers $headers -Body $body -TimeoutSec {http_timeout}\n\
if ($resp.choices[0].finish_reason -eq 'length') {{\n\
  # FIX B: max_tokens truncated the decode -> the content is a cut-off, unparseable\n\
  # JSON. Emit a DISTINCT failed result so truncation is observable in logs and to\n\
  # the parent coder, instead of falling through to the generic `failed` fallback\n\
  # (which is indistinguishable from a genuine model failure).\n\
  Write-Output '{{\"status\":\"failed\",\"output\":\"generation truncated at max_tokens ({max_tokens}) — increase budget or reduce scope\"}}'\n\
}} else {{\n\
  $content = $resp.choices[0].message.content\n\
  if ($null -ne $content) {{ Write-Output $content }}\n\
}}\n\
}} catch {{ }}\n\
}}"
    )
}

/// Windows wrapper: run `$run` (a pipeline that writes the backend's answer to
/// stdout), redirect that stdout to a bounded RAW temp file (WARNING 7 — never hold
/// all output in a PS string / cap memory), then normalize it into a
/// `MiniCoderResult` JSON at `$result_path`.
///
/// BLOCKER 2 + MINOR 10: extraction is a BALANCED-BRACE walk, not first-`{` /
/// last-`}`. We strip ANSI CSI **and** OSC/DCS/APC/PM/SOS escape payloads (ollama
/// spinners can carry `{`/`}` inside an OSC), then for EACH `{` we attempt to parse
/// the balanced `{...}` candidate starting there (honoring JSON string literals so
/// a `}` inside `"output":"foo() {bar}"` does not end the object early). The FIRST
/// candidate that parses AND has a valid `status` wins; none -> best-effort
/// `failed`. This stops trailing prose `}` from downgrading a valid `done`.
///
/// B1: nothing sensitive is on argv; the prompt was on stdin.
#[cfg(windows)]
pub(crate) fn windows_stdout_to_result_wrapper(
    run: &str,
    result_path: &str,
    raw_path: &str,
) -> String {
    // WARNING 7: read the RAW file with a bounded byte cap so a runaway backend
    // cannot OOM us. Mirrors mini_coder::MAX_RESULT_BYTES (1 MiB).
    // NOTE: `$rawFile` is (re)declared here so the wrapper is self-contained — it is
    // also invoked standalone (e.g. by the balanced-walk behavioral test) WITHOUT the
    // build_mini_command_impl preamble, so it must not assume an externally-set var.
    let max_bytes = super::mini_coder::MAX_RESULT_BYTES;
    format!(
        "$rawFile = {raw_path}\n\
# WARNING 7: redirect the backend's stdout to a temp FILE (not a PS string).\n\
{run} > $rawFile 2>$null\n\
$out = $null\n\
try {{\n\
  # Read with a BOM-detecting StreamReader: Windows PowerShell's `>` writes UTF-16\n\
  # LE+BOM, while an external CLI's bytes are decoded via the console encoding — a\n\
  # detecting reader handles both. Bounded to MAX_RESULT_BYTES chars so a runaway\n\
  # backend cannot OOM us; loop because a single Read may return fewer chars.\n\
  $sr = New-Object System.IO.StreamReader($rawFile, $true)\n\
  try {{\n\
    $cap = {max_bytes}\n\
    $cbuf = New-Object char[] $cap\n\
    $total = 0\n\
    while ($total -lt $cap) {{\n\
      $n = $sr.Read($cbuf, $total, $cap - $total)\n\
      if ($n -le 0) {{ break }}\n\
      $total += $n\n\
    }}\n\
  }} finally {{ $sr.Close() }}\n\
  $raw = New-Object string($cbuf, 0, $total)\n\
  # FIX2: capture the FIRST self-reported `failed` object (the oMLX truncation emitter\n\
  # writes {{\"status\":\"failed\",\"output\":\"generation truncated at max_tokens ...\"}}) so\n\
  # its DISTINCT message reaches the parent coder verbatim instead of the generic\n\
  # fallback. A terminal status (done/needs_clarification) still WINS over it.\n\
  $failedOut = $null\n\
  # MINOR 10: strip OSC/DCS/APC/PM/SOS payloads, then CSI escapes.\n\
  $clean = [regex]::Replace($raw, \"\\x1b\\][^\\x07\\x1b]*(\\x07|\\x1b\\\\)\", '')\n\
  $clean = [regex]::Replace($clean, \"\\x1b[P_^X][^\\x1b]*\\x1b\\\\\", '')\n\
  $clean = [regex]::Replace($clean, \"\\x1b\\[[0-9;?]*[A-Za-z]\", '')\n\
  # BLOCKER 2: balanced-brace walk. For each '{{' try the balanced object there.\n\
  for ($i = 0; $i -lt $clean.Length -and $null -eq $out; $i++) {{\n\
    if ($clean[$i] -ne '{{') {{ continue }}\n\
    $depth = 0; $inStr = $false; $esc = $false; $end = -1\n\
    for ($j = $i; $j -lt $clean.Length; $j++) {{\n\
      $ch = $clean[$j]\n\
      if ($inStr) {{\n\
        if ($esc) {{ $esc = $false }}\n\
        elseif ($ch -eq '\\') {{ $esc = $true }}\n\
        elseif ($ch -eq '\"') {{ $inStr = $false }}\n\
      }} else {{\n\
        if ($ch -eq '\"') {{ $inStr = $true }}\n\
        elseif ($ch -eq '{{') {{ $depth++ }}\n\
        elseif ($ch -eq '}}') {{ $depth--; if ($depth -eq 0) {{ $end = $j; break }} }}\n\
      }}\n\
    }}\n\
    if ($end -lt 0) {{ continue }}\n\
    $candidate = $clean.Substring($i, $end - $i + 1)\n\
    try {{\n\
      $parsed = $candidate | ConvertFrom-Json\n\
      if ($parsed.status -eq 'done' -or $parsed.status -eq 'needs_clarification') {{\n\
        $out = $candidate\n\
      }} elseif ($parsed.status -eq 'failed' -and $null -eq $failedOut -and $parsed.output -is [string]) {{\n\
        $failedOut = $candidate\n\
      }}\n\
    }} catch {{ }}\n\
  }}\n\
}} catch {{ $out = $null }}\n\
Remove-Item -LiteralPath $rawFile -Force -ErrorAction SilentlyContinue\n\
if ($null -eq $out) {{ $out = $failedOut }}\n\
if ($null -eq $out) {{\n\
  $out = '{{\"status\":\"failed\",\"output\":\"mini backend produced no valid JSON result\"}}'\n\
}}\n\
[System.IO.File]::WriteAllText({result_path}, $out, (New-Object System.Text.UTF8Encoding $false))\n"
    )
}

/// PURE, platform-agnostic builder for the macOS `/bin/sh` cleanup preamble. Kept
/// uncfg'd (no `target_os` gate) so it is unit-testable on the Windows dev host.
///
/// FIX (BLOCKER): the paths are ASSIGNED to shell variables FIRST, then referenced
/// DOUBLE-QUOTED inside the trap's single-quoted body. The arguments `prompt_dir_q`
/// and `raw_path_q` are already `sh_single_quote_local`-wrapped (they expand to
/// `'…'`), so the assignment RHS is correctly quoted for ANY path (spaces, quotes).
/// Putting the wrapped paths directly inside the trap's own single-quoted string
/// (the previous code) terminated the outer trap delimiter on the first embedded
/// `'`, making the trap a shell syntax error for any space/quote-containing path —
/// so the `EXIT` trap never armed and the source-bearing prompt dir + `.raw` capture
/// leaked on disk. With the variable indirection only `$_MINI_*` expansion happens
/// at EXIT time, inside the still-intact single-quoted trap body.
///
/// The trap is armed BEFORE `set -e` so it fires even if a later command aborts under
/// `set -e`. `_MINI_RAW_FILE` is always set (a non-existent file in `rm -rf` is a
/// no-op, so the codex backend — which writes no `.raw` — is unaffected).
///
/// P5: `profile_dir_q` is the OPTIONAL restricted parent dir of the `.sb` Seatbelt
/// profile (present ONLY on the sandboxed local-loopback path). When `Some`, the trap
/// ALSO removes it on every exit path — a leaked `.sb` per launch is a bug — guarded on
/// a non-empty value. sandbox-exec reads the profile at parse
/// time (in the PARENT process, before the sandbox is applied), so removing it from the
/// in-sandbox trap is safe. P5: `sandboxed` gates the `ulimit` rlimit lines, which are
/// emitted BETWEEN the trap and `set -e` (trap first so cleanup is always armed; the
/// rlimits before `set -e`, each with `|| true` so a kernel-rejected limit can never
/// abort the script). The codex/api/non-loopback path passes `sandboxed=false`, leaving
/// the preamble byte-identical to the pre-P5 status quo.
//
// Used by the macOS `build_mini_command_impl` arm and by the platform-agnostic test;
// on a non-test, non-macOS build it is unreferenced, hence the conditional allow.
#[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]
pub(crate) fn build_macos_trap_preamble(
    prompt_dir_q: &str,
    raw_path_q: &str,
    profile_dir_q: Option<&str>,
    sandboxed: bool,
) -> String {
    // P5: BYTE-FOR-BYTE-UNCHANGED guarantee — the codex/api/non-loopback (NON-sandboxed)
    // path must emit the EXACT pre-P5 preamble. So the `.sb` profile machinery (its var
    // assignment, its trap removal clause) AND the rlimit lines are emitted ONLY when
    // sandboxed (`profile_dir_q.is_some()` is true iff sandboxed). When not sandboxed they
    // collapse to empty strings, so the produced script is identical to pre-P5.
    let profile_assign = match profile_dir_q {
        Some(q) => format!("_MINI_PROFILE_DIR={q}\n"),
        None => String::new(),
    };
    // The `.sb` removal is appended to the trap body ONLY on the sandboxed path, mirroring
    // the (guarded) key-dir clause. A leaked `.sb` per launch is a bug.
    let profile_trap_clause = if profile_dir_q.is_some() {
        "; [ -n \"$_MINI_PROFILE_DIR\" ] && rm -rf \"$_MINI_PROFILE_DIR\" 2>/dev/null || true"
    } else {
        ""
    };
    // P5: rlimit cage on the sandboxed path ONLY, BETWEEN the trap and `set -e`. The CPU
    // cap reuses the wall-clock kill cap so the two never diverge; each line `|| true`.
    let rlimits = if sandboxed {
        format!(
            "ulimit -t {} 2>/dev/null || true\n\
ulimit -v {} 2>/dev/null || true\n\
ulimit -u {} 2>/dev/null || true\n",
            DEFAULT_WALL_CLOCK_CAP_SECS, MINI_RLIMIT_ADDRESS_SPACE_KIB, MINI_RLIMIT_MAX_PROCS,
        )
    } else {
        String::new()
    };
    format!(
        "_MINI_PROMPT_DIR={prompt_dir_q}\n\
_MINI_RAW_FILE={raw_path_q}\n\
{profile_assign}\
trap 'rm -rf \"$_MINI_PROMPT_DIR\" \"$_MINI_RAW_FILE\" 2>/dev/null || true{profile_trap_clause}' EXIT\n\
{rlimits}\
set -e\n"
    )
}

/// PURE, platform-agnostic single-quote for embedding a value inside the macOS
/// `/bin/sh -c` script. Mirrors `sh_single_quote_local` but is uncfg'd so the oMLX
/// macOS-script builder (and its test) work on the Windows dev host.
#[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]
pub(crate) fn sh_single_quote_portable(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// oMLX-P2 (macOS): build the `$run` block that POSTs an OpenAI chat-completion to the
/// loopback oMLX server via a `python3` + stdlib `urllib.request` heredoc and prints
/// `choices[0].message.content` on stdout. Its stdout is captured by the EXISTING
/// `macos_stdout_to_result_wrapper`, which extracts the MiniCoderResult JSON.
///
/// INJECTION-SAFETY: the prompt is read from the file at `$MINI_PROMPT_FILE` (path via
/// ENV, never argv) and the request body is built with `json.dumps`, so prompt content
/// is JSON-encoded by the encoder and can never break out of the JSON string. The base
/// URL and prompt path ride in ENV vars — nothing on argv.
///
/// NOTE: an earlier revision also plumbed an OMLX_KEY_FILE env-var and a bearer-token
/// reader block; that key plumbing has been removed. The generated script is
/// functionally identical to the pre-cleanup version for the (now-only) no-key path.
///
/// FAILURE = SILENCE: any exception (connection refused, non-2xx → `HTTPError`, missing
/// field, non-JSON body) prints NOTHING and exits, so the wrapper finds no valid JSON
/// and writes the clean `{"status":"failed",...}` fallback (no partial garbage).
///
/// Kept uncfg'd (platform-agnostic) so it is unit-testable on the Windows dev host,
/// like `build_macos_trap_preamble`. The inner heredoc uses the `OMLXEOF` delimiter so
/// it never collides with the wrapper's own `PYEOF` heredoc.
/// Python payload for [`build_omlx_run_macos`]'s heredoc, kept as a module-scope
/// RAW string so its indentation survives verbatim. Inside a `format!` literal
/// the `\n\` line continuations strip each following line's leading whitespace,
/// which silently flattens the Python block structure and makes the script die
/// with `IndentationError` at runtime (found on the first real macOS run).
/// `@OMLX_TIMEOUT_ENV@` / `@OMLX_TIMEOUT_DEFAULT@` are
/// substituted by the builder.
pub(crate) const OMLX_RUN_MACOS_PY: &str = r#"import os, json
import urllib.request, urllib.error
try:
    with open(os.environ['MINI_PROMPT_FILE'], 'r', encoding='utf-8') as f:
        prompt = f.read()
    model = os.environ['OMLX_MODEL']
    body_dict = {
        'model': model,
        'messages': [{'role': 'user', 'content': prompt}],
        'stream': False,
        'temperature': 0.1,
        'max_tokens': @OMLX_MAX_TOKENS@,
        'repetition_penalty': @OMLX_REP_PENALTY@,
    }
    if 'qwen' in model.lower():
        body_dict['chat_template_kwargs'] = {'enable_thinking': @OMLX_THINKING@}
    body = json.dumps(body_dict).encode('utf-8')
    req = urllib.request.Request(os.environ['OMLX_URL'], data=body, method='POST')
    req.add_header('Content-Type', 'application/json')
    timeout = int(os.environ.get('@OMLX_TIMEOUT_ENV@', '@OMLX_TIMEOUT_DEFAULT@'))
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        data = json.loads(resp.read().decode('utf-8', 'replace'))
    import sys
    if data['choices'][0].get('finish_reason') == 'length':
        # FIX B: max_tokens truncated the decode -> the content is a cut-off,
        # unparseable JSON. Emit a DISTINCT failed result so truncation is
        # observable in logs and to the parent coder, instead of falling through to
        # the generic `failed` fallback (indistinguishable from a model failure).
        sys.stdout.write('{"status":"failed","output":"generation truncated at max_tokens (@OMLX_MAX_TOKENS@) — increase budget or reduce scope"}')
    else:
        content = data['choices'][0]['message']['content']
        if content is not None:
            sys.stdout.write(content)
except Exception:
    pass
"#;

#[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]
pub(crate) fn build_omlx_run_macos(
    base_url: &str,
    model: &str,
    prompt_path_q: &str,
    fix_pass_thinking: bool,
) -> String {
    let url_q = sh_single_quote_portable(&format!("{base_url}/chat/completions"));
    let model_q = sh_single_quote_portable(model);
    // F2: the HTTP timeout (seconds) is derived from the SAME wall-clock cap as the PTY
    // kill (minus a margin) and rides a non-secret env var, so a stalled request aborts
    // JUST UNDER the cap with a clean `failed` fallback. The python default mirrors the
    // derived value so the two never silently diverge.
    let http_timeout = omlx_http_timeout_secs();
    // Export the base URL, model and prompt path for python (all via env, never argv).
    // `OMLX_MODEL` carries OUR validated bare tag; still passed via env for symmetry and
    // to keep argv empty.
    let py = OMLX_RUN_MACOS_PY
        .replace("@OMLX_TIMEOUT_ENV@", OMLX_TIMEOUT_ENV)
        .replace("@OMLX_TIMEOUT_DEFAULT@", &http_timeout.to_string())
        // FIX 2: bound the decode — a hard token budget (includes thinking) plus a
        // mild repetition penalty, the only runaway guards on this stream:false path.
        .replace("@OMLX_MAX_TOKENS@", &OMLX_MAX_TOKENS_DEFAULT.to_string())
        .replace("@OMLX_REP_PENALTY@", OMLX_REPETITION_PENALTY)
        // P6: True on fix passes, False on initial writes (Qwen-only, gated above).
        .replace(
            "@OMLX_THINKING@",
            if fix_pass_thinking { "True" } else { "False" },
        );
    format!(
        "OMLX_URL={url_q}\nexport OMLX_URL\n\
OMLX_MODEL={model_q}\nexport OMLX_MODEL\n\
MINI_PROMPT_FILE={prompt_path_q}\nexport MINI_PROMPT_FILE\n\
{OMLX_TIMEOUT_ENV}={http_timeout}\nexport {OMLX_TIMEOUT_ENV}\n\
python3 - <<'OMLXEOF'\n{py}OMLXEOF\n"
    )
}

#[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]
pub(crate) fn build_apple_fm_run_macos(
    prompt_pipe: &str,
    fm_path: &str,
    model: Option<&str>,
) -> String {
    let mut parts = vec![sh_single_quote_portable(fm_path), "respond".to_string()];
    if let Some(model) = model.map(str::trim).filter(|m| !m.is_empty()) {
        parts.push("--model".to_string());
        parts.push(sh_single_quote_portable(model));
    }
    format!("{prompt_pipe} | {}", parts.join(" "))
}

/// P5: is the (resolved) backend base URL a LOOPBACK endpoint? `true` for an EMPTY URL
/// (ollama/AppleFm carry no base_url — ollama talks to its own loopback daemon, AppleFm
/// is on-device, so neither has a remote endpoint to confine away from) and for any
/// `http://` URL whose host is `localhost` / `127.0.0.0/8` / `[::1]`. `false` for a
/// NON-loopback URL (e.g. a hand-edited oMLX config pointing off-box). Reuses the SINGLE
/// loopback-host rule shared across this machine ([`crate::backend::censor::gemma::
/// is_loopback_base`], via `authority_is_loopback`) so the sandbox-scope gate can never
/// drift from the privacy validators — the same `@`-userinfo / `127.0.0.1.evil.com`
/// suffix tricks are rejected. Port-agnostic on purpose (the scope gate only cares about
/// the HOST; oMLX's own `:port` validation lives in `validate_omlx_base_url`).
///
/// Kept uncfg'd (platform-agnostic) so it is unit-testable on the Windows dev host, like
/// [`build_macos_trap_preamble`].
#[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]
pub(crate) fn base_url_host_is_loopback(base_url: &str) -> bool {
    let trimmed = base_url.trim();
    trimmed.is_empty() || crate::backend::censor::gemma::is_loopback_base(trimmed)
}

/// AUDIT CRITICAL (use-time base_url revalidation): a LOCAL-kind backend (oMLX/Ollama/AppleFm)
/// MUST point at a loopback endpoint. A hand-edited NON-loopback base_url would ship the prompt +
/// project source to a REMOTE host — no sandbox confines an HTTP egress to a config-controlled URL.
/// `true` = REJECT the agentic spawn (caller declines → escalates, never leaks). Codex/Api are
/// cloud backends (remote by design) and are not rejected here. Pure → unit-testable.
pub(crate) fn agentic_local_base_url_rejected(kind: MiniCoderBackendKind, base_url: &str) -> bool {
    matches!(
        kind,
        MiniCoderBackendKind::Omlx | MiniCoderBackendKind::Ollama | MiniCoderBackendKind::AppleFm
    ) && !base_url_host_is_loopback(base_url)
}

// sbpl_escape + canonical_sandbox_path now live in `sandbox::seatbelt` as the single source of
// truth (unified per review finding F3 — the duplicate copies here have been removed). They are
// used by `build_seatbelt_profile` below.
use crate::backend::sandbox::seatbelt::{canonical_sandbox_path, sbpl_escape};

// TODO(P5-followup): (a) the mini does NOT yet execute the project test suite (P4 noted
// "tests run in the sandbox" but nothing runs them today), and (b) the Censor static-
// analysis runners spawn OUTSIDE this sh sandbox (they go through Rust `Command::new`, not
// `/bin/sh` under sandbox-exec). Both are separate future phases; this profile only confines
// the one-shot local-loopback mini launch.
/// P5: build the TIGHT loopback Seatbelt/SBPL profile for a sandboxed LOCAL-LOOPBACK mini
/// (oMLX/ollama/AppleFm on a loopback endpoint). This is the ONLY profile kind needed this
/// phase: the child does HTTP + prints its JSON result on stdout; Rust (not the child)
/// applies the emitted edits per P4, so the child needs NO project-file WRITE access.
///
/// Boundary model: file-READS are broad (a tight `file-read*` breaks python3/dyld at load
/// time), so the security boundary lives on the WRITES (deny-by-default; only the
/// parameterized scratch/temp set) and on the NETWORK (deny-all, loopback-only). The base
/// URL host:port is user-configurable, so the net rule is loopback-only (`remote tcp/udp
/// "localhost:*"`, which the kernel matches for both 127.0.0.1 and ::1) and NEVER
/// hardcodes a port. `writable_paths` are each canonicalized and emitted as one
/// `(subpath …)`; the project root is read-only (present under `file-read*`, ABSENT under
/// `file-write*`). All interpolated paths are SBPL-escaped.
///
/// Kept uncfg'd (platform-agnostic) so it is unit-testable on the Windows dev host, like
/// [`build_macos_trap_preamble`].
#[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]
pub(crate) fn build_seatbelt_profile(project_root: &Path, writable_paths: &[PathBuf]) -> String {
    let project_root_q = sbpl_escape(&canonical_sandbox_path(project_root).to_string_lossy());
    // TMPDIR — the child's scratch/temp area (python tempfiles, etc). Canonicalize so the
    // rule matches the real inode (`/var/folders/...` is a symlink to `/private/var/...`).
    let tmpdir = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let tmpdir_q = sbpl_escape(&canonical_sandbox_path(&tmpdir).to_string_lossy());
    // One `(subpath "<canonical abs>")` per writable path, canonicalized + SBPL-escaped.
    let writable_subpaths = writable_paths
        .iter()
        .filter_map(|p| {
            // review F2: skip non-absolute writable paths — a relative `(subpath "..")` has
            // dangerous CWD-relative SBPL semantics. Mirrors the guard in `seatbelt::build_profile`.
            if !p.is_absolute() {
                eprintln!("[sandbox] mini one-shot: skipping non-absolute writable_path {p:?}");
                return None;
            }
            Some(format!(
                "    (subpath \"{}\")",
                sbpl_escape(&canonical_sandbox_path(p).to_string_lossy())
            ))
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "(version 1)\n\
(deny default)\n\
\n\
; reads broad — a tight file-read* breaks python3/dyld at load: the dyld SHARED CACHE lives on\n\
; a separate Preboot/Cryptexes APFS volume that `(subpath \"/System\")` does NOT traverse, so a\n\
; subpath-filtered file-read* makes /bin/sh abort (SIGABRT) before exec. The security boundary\n\
; lives on the WRITES (deny-by-default) and the NETWORK (loopback-only), NOT on reads.\n\
; (project_root {project_root_q} is readable here AND absent from file-write* => read-only.)\n\
(allow file-read*)\n\
(allow file-read-metadata)\n\
(allow sysctl-read)\n\
(allow mach-lookup)\n\
\n\
; writes deny-by-default; ONLY the parameterized scratch/temp set (NO project files on the emit-edits path)\n\
(allow file-write*\n\
    (literal \"/dev/null\")\n\
    (subpath \"{tmpdir_q}\")\n\
{writable_subpaths})\n\
\n\
; exec: sh + python3. Allow the standard interpreter dirs so PATH-resolved python3 matches\n\
; (robust to /usr/bin vs /opt/homebrew/bin vs venv — exec of read-only system bins is not the boundary)\n\
(allow process-exec\n\
    (literal \"/bin/sh\")\n\
    (subpath \"/usr/bin\") (subpath \"/bin\")\n\
    (subpath \"/opt/homebrew\") (subpath \"/usr/local/bin\"))\n\
(allow process-fork)\n\
\n\
; network: deny all, allow loopback only (base_url host:port is user-configurable -> NEVER hardcode a port)\n\
; (remote tcp \"localhost:*\") covers 127.0.0.1 AND ::1 at the kernel level; an external IP stays denied\n\
(deny network*)\n\
(allow network-outbound\n\
    (remote tcp \"localhost:*\")\n\
    (remote udp \"localhost:*\"))\n"
    )
}

#[cfg(target_os = "macos")]
pub(crate) fn build_mini_command_impl(
    backend: &MiniCoderBackend,
    project_root: &Path,
    result_target: &Path,
    prompt_file: &Path,
    mcp_roots: Option<&McpRoots>,
    fix_pass_thinking: bool,
) -> Result<(CommandBuilder, Option<PathBuf>), String> {
    // P5 SCOPE GATE: the sandbox-exec wrap + rlimits apply ONLY to a LOCAL-LOOPBACK
    // backend (oMLX/ollama/AppleFm) whose resolved base_url host is loopback. codex/api
    // (remote-API egress) and a local-kind backend pointed off-box keep the spawn path
    // BYTE-FOR-BYTE unchanged — codex confinement is a separate future net-proxy phase.
    let sandboxed = matches!(
        backend.kind,
        MiniCoderBackendKind::Omlx | MiniCoderBackendKind::Ollama | MiniCoderBackendKind::AppleFm
    ) && base_url_host_is_loopback(backend.base_url.as_deref().unwrap_or(""));
    // WARNING 6: use `/bin/sh` UNCONDITIONALLY (do not read the unvalidated $SHELL).
    let prompt_path = sh_single_quote_local(&prompt_file.to_string_lossy());
    let result_path = sh_single_quote_local(&result_target.to_string_lossy());
    // The prompt's per-launch restricted PARENT dir (WARNING 8: cleaned by the trap,
    // not leaked on disk). Removing the dir recursively also removes the prompt file.
    let prompt_dir = sh_single_quote_local(
        &prompt_file
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    // WARNING 7: a sibling RAW stdout file (next to the result file, under the
    // scratch root) so we never capture the backend's stdout into a shell variable
    // (which truncates silently at ARG_MAX ~128KB and is unbounded in memory).
    let raw_path = sh_single_quote_local(&format!("{}.raw", result_target.to_string_lossy()));

    // FIX 1: deliver the prompt by piping the restricted FILE directly into the
    // backend (`cat {prompt_path} | ...`) so the bytes are preserved VERBATIM — the
    // old `PROMPT="$(cat ...)"` capture silently stripped trailing newlines and
    // mutated the prompt. `cat` keeps the prompt off argv (B1: never on the PTY
    // stream / process args).
    let prompt_pipe = format!("cat {prompt_path}");

    // FIX 1 (source-content leak): a `trap ... EXIT` as the FIRST line guarantees the
    // restricted prompt dir (which front-loads SOURCE CODE) AND the `.raw` stdout
    // capture are ALWAYS removed on ANY exit — success, `set -e` abort, a missing
    // `cat`, or a missing `python3`. The old code deleted the prompt AFTER the read,
    // so a `set -e` abort before that line leaked the source-bearing file on disk.
    //
    // BLOCKER FIX: the trap body references the paths via DOUBLE-QUOTED shell
    // variables assigned before the trap, so a path containing whitespace/quotes (e.g.
    // `/Users/the owner/My Project/`) no longer terminates the trap's own single-quoted
    // delimiter and break the trap. See `build_macos_trap_preamble`.
    // P5: on the sandboxed local-loopback path, generate the TIGHT Seatbelt profile and
    // write it to a per-launch 0600 `.sb` temp (same restricted-dir mechanism as the
    // prompt files). The child does HTTP + prints JSON; Rust applies the edits per
    // P4, so the WRITABLE set is scratch/temp ONLY (NO project-file writes). Every path
    // the in-sandbox trap removes (prompt dir, `.raw` parent, the `.sb` dir
    // itself) MUST be writable or the trap's `rm -rf` would be denied inside the sandbox.
    // The returned `profile_path` (and its restricted parent dir) are cleaned up on BOTH
    // the EXIT trap (success/abort) AND the spawn-failure path (see `remove_mini_temp_files`).
    let profile_path: Option<PathBuf> = if sandboxed {
        let scratch_root = project_root.join(MINI_SCRATCH_DIR);
        let mut writable_paths: Vec<PathBuf> = vec![scratch_root];
        if let Some(p) = prompt_file.parent() {
            writable_paths.push(p.to_path_buf());
        }
        // The `.raw` capture sits next to the result file (same parent as result_target).
        if let Some(p) = result_target.parent() {
            writable_paths.push(p.to_path_buf());
        }

        let profile = build_seatbelt_profile(project_root, &writable_paths);
        let path = super::projects::write_restricted_prompt_file(&profile)?;
        Some(path)
    } else {
        None
    };
    // The `.sb`'s restricted parent dir is added to the trap (removed on every exit).
    // Single-quoted for safe embedding in the trap variable.
    let profile_dir = profile_path.as_ref().map(|p| {
        sh_single_quote_local(
            &p.parent()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
    });
    let preamble = build_macos_trap_preamble(
        &prompt_dir,
        &raw_path,
        profile_dir.as_deref(),
        sandboxed,
    );

    // The body match can fail (a hand-edited config missing a required model/base_url, or
    // an unresolved `fm` binary). On the SANDBOXED path the `.sb` profile is already on
    // disk by now, so capture the body in a Result and, on Err, remove the profile temp
    // before propagating — otherwise a body error would leak the `.sb` (the in-script trap
    // never ran). The non-Seatbelt path has `profile_path == None` (incl.
    // Windows, broker-sandboxed at spawn), so this is a no-op there.
    let body_result: Result<String, String> = (|| -> Result<String, String> {
        Ok(match backend.kind {
            MiniCoderBackendKind::Codex => {
                // P3: with the read-only oracle grant the mini's codex gets the SAME
                // devboule server as full coders via the shared token builder
                // (no drift); narrowing is SERVER-side (role "mini"). No grant ⇒ no
                // `-c` flags ⇒ byte-identical to the MINOR 9 status quo.
                let mut args: Vec<String> = vec!["exec".to_string()];
                if let Some(roots) = mcp_roots {
                    let app_bin = super::projects::resolve_app_binary();
                    let app_bin = app_bin.as_ref().map(|p| p.to_string_lossy().into_owned());
                    args.extend(super::projects::codex_mcp_config_args(
                        &crate::oracle::oracle_setup::resolve_oracle_python(),
                        &roots.management_root,
                        &roots.projects_dir,
                        app_bin.as_deref(),
                        // MINI-EXCLUSION (design §6, HARD): the mini NEVER receives user MCP
                        // servers. It reuses this shared Oracle-only builder and passes an
                        // EMPTY slice, so its codex `-c` flags stay Oracle-only (narrowed
                        // server-side by role "mini"). This bare empty literal names no
                        // user-MCP type, so this file keeps ZERO references to the user-MCP
                        // config code.
                        &[],
                    )?);
                }
                if let Some(model) = backend.model.as_deref() {
                    if !model.trim().is_empty() {
                        args.push("-m".to_string());
                        args.push(model.trim().to_string());
                    }
                }
                let arg_line = args
                    .iter()
                    .map(|a| sh_single_quote_local(a))
                    .collect::<Vec<_>>()
                    .join(" ");
                // prompt on STDIN (piped from the file), never argv. `arg_line` already
                // leads with `exec` (so this is `codex exec [-m model]`).
                format!("{prompt_pipe} | codex {arg_line}\n")
            }
            MiniCoderBackendKind::Ollama => {
                let model = backend
                    .model
                    .as_deref()
                    .map(str::trim)
                    .filter(|m| !m.is_empty())
                    .ok_or_else(|| "ollama backend requires a model tag".to_string())?;
                // ollama run <tag>: our fixed tokens (the tag is validated to a bare
                // token), prompt on STDIN (piped from the file). Capture stdout via the
                // shared file wrapper.
                let run = format!(
                    "{prompt_pipe} | ollama run {}",
                    sh_single_quote_local(model)
                );
                macos_stdout_to_result_wrapper(&run, &result_path, &raw_path)
            }
            MiniCoderBackendKind::Api => {
                let command = backend
                    .command
                    .as_deref()
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                    .ok_or_else(|| "api backend requires a command line".to_string())?;
                // BLOCKER 1 / WARNING 5: `command` is a TRUSTED, operator-configured shell
                // command LINE — the same trust model as a `customAgentClients` command
                // (see projects::build_macos_agent_script's custom branch). It is placed
                // VERBATIM as a pipeline target so `/bin/sh` tokenizes the whole line
                // (`mycli chat --json` runs `mycli` with args `chat --json`). The prompt
                // is piped over stdin; the API key comes from the CLI's OWN env — never
                // injected by us, never on argv.
                let run = format!("{prompt_pipe} | {command}");
                macos_stdout_to_result_wrapper(&run, &result_path, &raw_path)
            }
            MiniCoderBackendKind::Omlx => {
                // oMLX-P2 (macOS): the one-shot script POSTs an OpenAI chat-completion to
                // the loopback oMLX server via a `python3`+`urllib` heredoc (stdlib only —
                // NO curl/jq), prints `choices[0].message.content` on stdout, and the
                // EXISTING wrapper extracts the MiniCoderResult JSON (Option A: keep PTY).
                // model + base_url REQUIRED (validated in oMLX-P1; re-checked for a clean
                // failure on a hand-edited config).
                let model = backend
                    .model
                    .as_deref()
                    .map(str::trim)
                    .filter(|m| !m.is_empty())
                    .ok_or_else(|| "omlx backend requires a model".to_string())?;
                let base_url = backend
                    .base_url
                    .as_deref()
                    .map(str::trim)
                    .filter(|b| !b.is_empty())
                    .ok_or_else(|| "omlx backend requires a base URL".to_string())?;
                // The prompt path and base URL ride in ENV vars — NEVER on argv.  (Key
                // plumbing was removed; this is functionally identical to the old no-key path.)
                let run = build_omlx_run_macos(
                    base_url,
                    model,
                    &prompt_path,
                    fix_pass_thinking,
                );
                macos_stdout_to_result_wrapper(&run, &result_path, &raw_path)
            }
            MiniCoderBackendKind::Openai => {
                // OpenAI mini-coder backend: CLI invocation mirroring the Codex arm.
                // The prompt is piped over STDIN (never argv). The API key rides in the
                // user's `OPENAI_API_KEY` env — we never inject it, never put it on argv.
                // The `openai` CLI binary is installed by the user. Like codex, the agent
                // writes its own result file, so we do NOT wrap stdout to the result target.
                // With the read-only oracle grant the mini's openai gets the SAME
                // devboule server as full coders via the shared token builder (no
                // drift); narrowing is SERVER-side (role "mini"). No grant ⇒ no `-c` flags
                // ⇒ plain invocation.
                let mut args: Vec<String> =
                    vec!["exec".to_string(), "--skip-git-repo-check".to_string()];
                // P3 (Openai): same Oracle-only MCP wiring as the Codex arm — the mini
                // reuses the shared builder with an EMPTY user-MCP slice so its `-c` flags
                // stay Oracle-only (narrowed server-side by role "mini").
                if let Some(roots) = mcp_roots {
                    let app_bin = super::projects::resolve_app_binary();
                    let app_bin = app_bin.as_ref().map(|p| p.to_string_lossy().into_owned());
                    args.extend(super::projects::codex_mcp_config_args(
                        &crate::oracle::oracle_setup::resolve_oracle_python(),
                        &roots.management_root,
                        &roots.projects_dir,
                        app_bin.as_deref(),
                        // MINI-EXCLUSION (design §6, HARD): the mini NEVER receives user MCP
                        // servers. It reuses this shared Oracle-only builder and passes an
                        // EMPTY slice, so its openai `-c` flags stay Oracle-only (narrowed
                        // server-side by role "mini"). This bare empty literal names no
                        // user-MCP type, so this file keeps ZERO references to the user-MCP
                        // config code.
                        &[],
                    )?);
                }
                if let Some(model) = backend.model.as_deref() {
                    if !model.trim().is_empty() {
                        args.push("-m".to_string());
                        args.push(model.trim().to_string());
                    }
                }
                let arg_line = args
                    .iter()
                    .map(|a| sh_single_quote_local(a))
                    .collect::<Vec<_>>()
                    .join(" ");
                // prompt on STDIN (piped from the file), never argv. `arg_line` already
                // leads with `exec` (so this is `openai exec [--skip-git-repo-check] [-m model]`).
                format!("{prompt_pipe} | openai {arg_line}\n")
            }
            MiniCoderBackendKind::AppleFm => {
                let fm = crate::backend::provider_detect::resolve_program("fm")
                    .ok_or_else(|| "Apple on-device requires macOS 27+.".to_string())?;
                let fm_path = fm.to_string_lossy();
                let run = build_apple_fm_run_macos(
                    &prompt_pipe,
                    fm_path.as_ref(),
                    backend.model.as_deref(),
                );
                macos_stdout_to_result_wrapper(&run, &result_path, &raw_path)
            }
            MiniCoderBackendKind::Cloud => {
                // Cloud is a pi-engine backend (HTTPS remote provider); the
                // directive executor's one-shot /bin/sh script path cannot
                // drive it today. Fail LOUDLY rather than silently
                // downgrading — a misconfigured spawn is diagnosed at the
                // boundary, not hidden behind a `done` from the wrong
                // backend. The pi sidecar's
                // `map_mini_coder_backend_to_sidecar_env` handles Cloud.
                return Err(
                    "cloud backend runs via the pi engine; the directive executor does \
                     not support it yet"
                        .to_string(),
                );
            }
        })
    })();
    let body = match body_result {
        Ok(body) => body,
        Err(e) => {
            // P5: a body error after the `.sb` was written would leak it (no in-script
            // trap ran) — remove the profile temp (and its restricted dir) before bailing.
            if let Some(path) = profile_path.as_deref() {
                super::projects::remove_restricted_temp_file(path);
            }
            return Err(e);
        }
    };

    let script = format!("{preamble}{body}exit 0");
    // P5: the SANDBOXED local-loopback path wraps the spawn in `/usr/bin/sandbox-exec -f
    // <profile.sb> /bin/sh -c <script>`; every OTHER path (codex/api/non-loopback) keeps
    // the BYTE-FOR-BYTE-unchanged `/bin/sh -c <script>` spawn — no sandbox, no rlimits.
    let mut cmd = match profile_path.as_ref() {
        Some(path) => {
            let profile_arg = path.to_string_lossy().into_owned();
            let mut cmd = CommandBuilder::new("/usr/bin/sandbox-exec");
            cmd.args(["-f", &profile_arg, "/bin/sh", "-c", &script]);
            cmd
        }
        None => {
            let mut cmd = CommandBuilder::new("/bin/sh");
            cmd.args(["-c", &script]);
            cmd
        }
    };
    cmd.cwd(project_root);
    // MINI-EXCLUSION (design §6): scrub the orchestrator-only user-MCP env var so the mini
    // child can NEVER inherit it from the host process env (CommandBuilder snapshots it).
    cmd.env_remove(FORBIDDEN_USER_MCP_ENV);
    Ok((cmd, profile_path))
}

/// macOS wrapper: run `$run`, redirect its stdout to a bounded RAW temp FILE
/// (WARNING 7 — never into a shell var, which truncates at ARG_MAX and is unbounded
/// in memory), then normalize it into a `MiniCoderResult` at `$result_path`.
///
/// BLOCKER 2 + MINOR 10: python3 strips ANSI CSI **and** OSC/DCS/APC/PM/SOS escape
/// payloads, then does a PROGRESSIVE `json.JSONDecoder().raw_decode(clean, i)` at
/// each `{` index — the FIRST candidate that decodes to a dict with a valid `status`
/// wins. This is a true balanced parse (a `}` inside `"output":"foo() {bar}"` is
/// handled by the JSON grammar), so trailing prose `}` cannot downgrade a `done`.
/// python3 ships on macOS dev setups (the Oracle runtime already requires python);
/// the result/raw paths ride in env vars so nothing is on argv.
/// Python payload for [`macos_stdout_to_result_wrapper`]'s heredoc, kept as a
/// module-scope RAW string so its indentation survives verbatim (same
/// `IndentationError` pitfall as [`OMLX_RUN_MACOS_PY`] — `\n\` continuations in
/// a `format!` literal strip the next line's leading whitespace). `@MAX_BYTES@`
/// is substituted by the wrapper.
#[cfg(target_os = "macos")]
pub(crate) const MACOS_RESULT_EXTRACTOR_PY: &str = r#"import os, re, json
out = None
# FIX2: a backend that self-reports a DISTINCT failure (the oMLX finish_reason=='length'
# truncation emitter writes {"status":"failed","output":"generation truncated at
# max_tokens ..."}) must reach the parent coder VERBATIM, not be replaced by the generic
# "no valid JSON" fallback. So we capture the FIRST balanced `failed` object too, but a
# terminal status (done/needs_clarification) always WINS over it.
failed_out = None
try:
    with open(os.environ['MINI_RAW_FILE'], 'rb') as f:
        raw = f.read(@MAX_BYTES@).decode('utf-8', 'replace')
    # MINOR 10: strip OSC/DCS/APC/PM/SOS payloads, then CSI escapes.
    clean = re.sub(r'\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)', '', raw)
    clean = re.sub(r'\x1b[P_^X][^\x1b]*\x1b\\', '', clean)
    clean = re.sub(r'\x1b\[[0-9;?]*[A-Za-z]', '', clean)
    dec = json.JSONDecoder()
    i = 0
    n = len(clean)
    while i < n and out is None:
        if clean[i] != '{':
            i += 1
            continue
        try:
            obj, _end = dec.raw_decode(clean, i)
            if isinstance(obj, dict):
                st = obj.get('status')
                if st in ('done', 'needs_clarification'):
                    out = clean[i:_end]
                elif st == 'failed' and failed_out is None and isinstance(obj.get('output'), str):
                    # Keep the self-reported failure verbatim (distinct message survives).
                    failed_out = clean[i:_end]
        except Exception:
            pass
        i += 1
except Exception:
    out = None
try:
    os.remove(os.environ['MINI_RAW_FILE'])
except Exception:
    pass
if out is None:
    out = failed_out
if out is None:
    out = json.dumps({'status': 'failed', 'output': 'mini backend produced no valid JSON result'})
with open(os.environ['MINI_RESULT'], 'w', encoding='utf-8') as f:
    f.write(out)
"#;

#[cfg(target_os = "macos")]
pub(crate) fn macos_stdout_to_result_wrapper(
    run: &str,
    result_path: &str,
    raw_path: &str,
) -> String {
    let py = MACOS_RESULT_EXTRACTOR_PY.replace(
        "@MAX_BYTES@",
        &super::mini_coder::MAX_RESULT_BYTES.to_string(),
    );
    format!(
        "MINI_RAW_FILE={raw_path}\nexport MINI_RAW_FILE\nMINI_RESULT={result_path}\nexport MINI_RESULT\n\
# WARNING 7: redirect the backend's stdout to a temp FILE (not a shell var).\n\
{{ {run} ; }} > \"$MINI_RAW_FILE\" 2>/dev/null || true\n\
python3 - <<'PYEOF'\n{py}PYEOF\n"
    )
}

/// macOS-only single-quote for embedding inside the `/bin/sh -c` script. Mirrors
/// `projects::sh_single_quote` (kept local so the executor does not depend on a
/// private projects fn): wrap in single quotes, escape embedded quotes via `'\''`.
#[cfg(target_os = "macos")]
pub(crate) fn sh_single_quote_local(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

// TODO: Linux sandbox = bubblewrap/landlock when the Linux mini arm lands (the macOS
// arm uses sandbox-exec + Seatbelt; there is no mini launch path on Linux yet).
#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) fn build_mini_command_impl(
    backend: &MiniCoderBackend,
    _project_root: &Path,
    _result_target: &Path,
    _prompt_file: &Path,
    _mcp_roots: Option<&McpRoots>,
    _fix_pass_thinking: bool,
) -> Result<(CommandBuilder, Option<PathBuf>), String> {
    if backend.kind == MiniCoderBackendKind::AppleFm {
        return Err("Apple on-device requires macOS 27+.".into());
    }
    Err("Mini-coder is supported on Windows and macOS only.".into())
}
