//! Agent terminal spawning and launch-script building.
//!
//! Extracted from `projects.rs` (S9 Pass 2b) to isolate the OS-specific agent
//! spawn helpers, launch-script builders, and related utilities.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::projects::{
    AgentLaunchEnv, OrchestratorLaunchConfig, command_exists, create_restricted_temp_file,
    mcp_client_config_json, ps_single_quote, remove_restricted_temp_file,
    resolve_app_binary, write_restricted_prompt_file,
};
use super::agents::agent_window_title;
use tauri::Manager;
// `process_creation_time` is only called from the Windows agent-spawn path (a #[cfg(windows)]
// site); importing it unconditionally trips `unused_imports` on non-Windows builds. Keep the
// import gated so the Windows build resolves it and the macOS build stays clean.
#[cfg(windows)]
use super::agents::process_creation_time;
use super::user_mcp_config;

/// F65: true when the caller already injected a vault Claude OAuth token into
/// `provider_env` (single vault read at launch assembly in `projects.rs`). Config-dir
/// sites must use this instead of re-reading the vault (audit A-1).
fn vault_oauth_present_in(provider_env: &[AgentLaunchEnv]) -> bool {
    provider_env
        .iter()
        .any(|e| e.name == "CLAUDE_CODE_OAUTH_TOKEN")
}

/// HE-4: env slice actually injected into a launch.
///
/// `custom_command` is an operator-configured command line interpolated into the
/// launch shell/script **unescaped**. A compromised config is RCE at next launch.
/// Trust model (alpha): operator-trusted, config integrity assumed. Isolate vault /
/// provider secrets from that path so a malicious custom command cannot harvest them
/// from the process environment. App-built launches (codex/claude/orchestrator) keep
/// secrets via the normal `provider_env` channel; argv-safe construction stays there.
fn provider_env_for_launch<'a>(
    custom_command: Option<&str>,
    provider_env: &'a [AgentLaunchEnv],
) -> &'a [AgentLaunchEnv] {
    if custom_command.is_some() {
        &[]
    } else {
        provider_env
    }
}

/// What a successful agent terminal spawn yields. `pid` is the spawned child's id
/// (the conhost child on Windows; the osascript helper on macOS — see the macOS
/// impl). `creation_time` is the Windows process creation FILETIME captured right
/// after spawn (None elsewhere) — the anti-pid-reuse fingerprint stored in the
/// ledger. `prompt_file` is the launch-token-bearing temp file so the app can
/// delete it on stop if the child shell died before its own Remove-Item ran.
pub(crate) struct SpawnedAgent {
    pub(crate) pid: u32,
    pub(crate) creation_time: Option<u64>,
    pub(crate) prompt_file: Option<PathBuf>,
}

/// Force-stop a just-spawned agent when its control record could not be saved.
/// Kills by EXACT window title (pid-reuse-safe). The osascript/macOS path closes
/// the titled Terminal window. This reuses the same primitives as stop_agent.
pub(crate) fn kill_spawned_agent_on_record_failure(window_title: &str, spawned: &SpawnedAgent) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // Try the exact-title kill first.
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/FI", &format!("WINDOWTITLE eq {window_title}")])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        // The window title may not be registered yet in the split-second after
        // spawn; as a recovery, also kill the just-spawned pid tree. This pid was
        // captured microseconds ago from OUR own spawn, so it is not a recycled id.
        let _ = Command::new("taskkill")
            .args(["/PID", &spawned.pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = spawned;
        let needle = window_title.replace('\\', "\\\\").replace('"', "\\\"");
        let close = format!(
            "tell application \"Terminal\" to close (every window whose name is \"{needle}\")"
        );
        let _ = Command::new("osascript").arg("-e").arg(&close).status();
        // HE-2: stop path — sweep stale secrets-bearing launch script dirs left
        // behind when bash never ran (self-delete is inside the script).
        sweep_stale_macos_launch_script_dirs(&std::env::temp_dir(), MACOS_LAUNCH_SCRIPT_STALE_SECS);
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (window_title, spawned);
    }
}

/// Cross-platform entrypoint. The SHARED logic (validating the client and that
/// its CLI exists on PATH, choosing the window-title string) lives here; the
/// actual OS-specific terminal spawn is delegated to a cfg-gated implementation.
/// Returns the spawn details (pid, creation time, prompt-file path).
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_agent_terminal(
    agent_id: &str,
    root_path: &Path,
    client: &str,
    // Some(command) for a configured custom client (the script execs it after the
    // universal prompt delivery); None for a built-in codex/claude/bare client.
    custom_command: Option<&str>,
    prompt: &str,
    management_root: &Path,
    projects_dir: &Path,
    model: Option<&str>,
    provider_env: &[AgentLaunchEnv],
    // L2.4: Some for the local Devboule orchestrator client.
    orchestrator: Option<&OrchestratorLaunchConfig>,
    // Phase A.2: merged, enabled user MCP servers for this launch (main coder only).
    user_servers: &[user_mcp_config::UserMcpServer],
) -> Result<SpawnedAgent, String> {
    // A custom client runs an arbitrary, operator-configured command line, so there
    // is no single executable on PATH to pre-check; the built-ins still are checked.
    // The orchestrator's executable is the resolved binary (already existence-checked
    // by resolve_orchestrator_binary at assembly time), so it stays empty here.
    let executable = if custom_command.is_some() || orchestrator.is_some() {
        ""
    } else {
        match client {
            "codex" => "codex",
            "claude" => "claude",
            _ => "",
        }
    };
    if !executable.is_empty() && !command_exists(executable) {
        return Err(format!("{executable} command not found in PATH."));
    }

    spawn_agent_terminal_impl(
        agent_id,
        root_path,
        client,
        executable,
        custom_command,
        prompt,
        management_root,
        projects_dir,
        model,
        provider_env,
        orchestrator,
        user_servers,
    )
}

/// App-hosted entrypoint: spawn the agent's shell INSIDE the app under a PTY (via
/// `backend::agent_pty`) instead of a detached OS console. Shares the exact same
/// SHARED script builders as the external path (`build_windows_agent_script` /
/// `build_macos_agent_script`), so prompt-file handling and env are identical; the
/// ONLY difference is the program/args (cfg-gated below) and that output is
/// streamed to the frontend rather than to an OS window. The PTY child's cwd is
/// the project root — the same working dir the external path uses.
///
/// There is no OS console pid/title to record, so the ledger entry stamps host
/// "app" and leaves pid/title/creationTime None; `stop_agent` routes by host to
/// `agent_pty_kill`.
/// Returns the launch-token-bearing prompt-file path (so the caller records it in
/// the ledger for stop_agent cleanup if the PTY child dies before its own
/// Remove-Item runs). `None` on platforms with no prompt file.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_agent_terminal_app(
    app: &tauri::AppHandle,
    agent_id: &str,
    root_path: &Path,
    client: &str,
    custom_command: Option<&str>,
    prompt: &str,
    management_root: &Path,
    projects_dir: &Path,
    model: Option<&str>,
    provider_env: &[AgentLaunchEnv],
    // L2.4: Some for the local Devboule orchestrator client.
    orchestrator: Option<&OrchestratorLaunchConfig>,
    // Phase A.2: merged, enabled user MCP servers for this launch (main coder only).
    user_servers: &[user_mcp_config::UserMcpServer],
) -> Result<Option<String>, String> {
    // The orchestrator's executable is the resolved binary (already existence-checked
    // by resolve_orchestrator_binary at assembly time), so it stays empty here.
    let executable = if custom_command.is_some() || orchestrator.is_some() {
        ""
    } else {
        match client {
            "codex" => "codex",
            "claude" => "claude",
            _ => "",
        }
    };
    if !executable.is_empty() && !command_exists(executable) {
        return Err(format!("{executable} command not found in PATH."));
    }
    spawn_agent_terminal_app_impl(
        app,
        agent_id,
        root_path,
        client,
        executable,
        custom_command,
        prompt,
        management_root,
        projects_dir,
        model,
        provider_env,
        orchestrator,
        user_servers,
    )
}

#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
fn spawn_agent_terminal_app_impl(
    app: &tauri::AppHandle,
    agent_id: &str,
    root_path: &Path,
    client: &str,
    executable: &str,
    custom_command: Option<&str>,
    prompt: &str,
    management_root: &Path,
    projects_dir: &Path,
    model: Option<&str>,
    provider_env: &[AgentLaunchEnv],
    orchestrator: Option<&OrchestratorLaunchConfig>,
    user_servers: &[user_mcp_config::UserMcpServer],
) -> Result<Option<String>, String> {
    use crate::backend::agent_pty::PtyCommand;

    let (prompt_file, script) = build_windows_agent_script(
        agent_id,
        root_path,
        client,
        executable,
        custom_command,
        prompt,
        management_root,
        projects_dir,
        model,
        orchestrator,
        user_servers,
        vault_oauth_present_in(provider_env),
    )?;

    // PTY host: run powershell directly (NO conhost — the PTY IS the console). Same
    // -NoExit/-ExecutionPolicy Bypass/-Command script the external path runs.
    // C6: PtyCommand keeps the components inspectable so the Windows broker can
    // spawn the child inside the AppContainer sandbox with a ConPTY.
    let mut cmd = PtyCommand::new(
        "powershell.exe",
        vec!["-NoExit".into(), "-ExecutionPolicy".into(), "Bypass".into(), "-Command".into(), script],
        root_path.to_path_buf(),
        provider_env_for_launch(custom_command, provider_env)
            .iter()
            .map(|e| (e.name.clone(), e.value.clone()))
            .collect(),
    );
    // C6: the AppContainer child cannot read user-only %TEMP% files — grant the
    // prompt dir + session gitconfig dir as read roots (Windows only; on macOS
    // the seatbelt profile allows broad reads so these are no-ops there).
    // Reviewer follow-up (1113782): the session gitconfig [include]s the user's
    // REAL global git config (~/.gitconfig / XDG) — that file must be readable
    // too or every git call in the sandbox dies with "unable to read config
    // file". Grant the real config FILE(S) directly (single-file ACEs, cheap;
    // contains commit identity + safe.directory, no credentials by design).
    #[cfg(target_os = "windows")]
    {
        for root in agent_sandbox_read_roots(Some(&prompt_file)) {
            cmd = cmd.read_root(root);
        }
    }

    let sessions = app
        .try_state::<crate::backend::agent_pty::AgentPtySessions>()
        .ok_or_else(|| "Agent terminal state is unavailable.".to_string())?;
    if let Err(e) = crate::backend::agent_pty::spawn_agent_pty(app, &sessions, agent_id, cmd) {
        // The PTY shell never started, so it cannot delete the temp prompt.
        remove_restricted_temp_file(&prompt_file);
        return Err(e);
    }
    // Surface the prompt-file path so the caller records it for stop_agent cleanup.
    Ok(Some(prompt_file.to_string_lossy().into_owned()))
}

// UNVERIFIED on macOS — needs testing on a real Mac.
//
// macOS app-hosted PTY: run the user's LOGIN SHELL with `-ic <script>` (the same
// shell script the external Terminal.app path builds via build_macos_agent_script),
// under our PTY. `-i` gives an interactive shell so the agent CLI behaves as in a
// real terminal; `-c <script>` runs our setup+launch script. cwd = project root.
#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn spawn_agent_terminal_app_impl(
    app: &tauri::AppHandle,
    agent_id: &str,
    root_path: &Path,
    client: &str,
    executable: &str,
    custom_command: Option<&str>,
    prompt: &str,
    management_root: &Path,
    projects_dir: &Path,
    model: Option<&str>,
    provider_env: &[AgentLaunchEnv],
    orchestrator: Option<&OrchestratorLaunchConfig>,
    user_servers: &[user_mcp_config::UserMcpServer],
) -> Result<Option<String>, String> {
    use crate::backend::agent_pty::PtyCommand;

    let (prompt_file, script) = build_macos_agent_script(
        agent_id,
        root_path,
        client,
        executable,
        custom_command,
        prompt,
        management_root,
        projects_dir,
        model,
        provider_env,
        orchestrator,
        // FIX 2 — PTY path: the script is the `zsh -ic <script>` argument and the
        // provider_env secrets are injected out-of-band via cmd.env below, so the
        // builder must NOT re-export them in-script (that would put them on argv).
        // There is no temp file to self-delete here either. -> false.
        false,
        user_servers,
    )?;

    // Prefer the user's login shell; fall back to /bin/zsh (macOS default), then
    // /bin/bash. The script itself is POSIX-sh compatible.
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let cmd = PtyCommand::new(
        shell,
        vec!["-ic".into(), script],
        root_path.to_path_buf(),
        provider_env_for_launch(custom_command, provider_env)
            .iter()
            .map(|e| (e.name.clone(), e.value.clone()))
            .collect(),
    );

    let sessions = app
        .try_state::<crate::backend::agent_pty::AgentPtySessions>()
        .ok_or_else(|| "Agent terminal state is unavailable.".to_string())?;
    if let Err(e) = crate::backend::agent_pty::spawn_agent_pty(app, &sessions, agent_id, cmd) {
        remove_restricted_temp_file(&prompt_file);
        return Err(e);
    }
    // Surface the prompt-file path so the caller records it for stop_agent cleanup.
    Ok(Some(prompt_file.to_string_lossy().into_owned()))
}

#[cfg(not(any(windows, target_os = "macos")))]
#[allow(clippy::too_many_arguments)]
fn spawn_agent_terminal_app_impl(
    _app: &tauri::AppHandle,
    _agent_id: &str,
    _root_path: &Path,
    _client: &str,
    _executable: &str,
    _custom_command: Option<&str>,
    _prompt: &str,
    _management_root: &Path,
    _projects_dir: &Path,
    _model: Option<&str>,
    _provider_env: &[AgentLaunchEnv],
    _orchestrator: Option<&OrchestratorLaunchConfig>,
    _user_servers: &[user_mcp_config::UserMcpServer],
) -> Result<Option<String>, String> {
    Err("App-hosted agent terminals are supported on Windows and macOS only.".into())
}

/// SHARED Windows launch-script builder used by BOTH the external console path
/// (`spawn_agent_terminal_impl`) and the app-hosted PTY path
/// (`spawn_agent_terminal_app_impl`). Centralising it guarantees identical
/// prompt-file handling (the launch-token-bearing prompt is written to a
/// restricted temp file and read back in-script — NEVER on argv) and identical
/// env assembly (management root, projects dir, PYTHONPATH, profile mode). The
/// ONLY thing the two callers differ on is HOW they run the returned script
/// (conhost+powershell window vs. a powershell child under a PTY).
///
/// Returns the restricted prompt-file path (so the caller can delete it if the
/// spawn itself fails) and the PowerShell script text.
///
/// B1: the prompt embeds the launch token, so it must NOT appear on the child
/// process command line (visible via argv to other processes / EDR / Sysmon).
#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_windows_agent_script(
    agent_id: &str,
    root_path: &Path,
    client: &str,
    executable: &str,
    // Some(command) for a configured custom client; None for a built-in.
    custom_command: Option<&str>,
    prompt: &str,
    management_root: &Path,
    projects_dir: &Path,
    model: Option<&str>,
    // L2.4: Some for the local Devboule orchestrator client (the resolved binary +
    // its non-secret env). None for codex/claude/custom — keeps their command_line
    // byte-identical. Dispatched FIRST so the orchestrator (whose `executable` is
    // empty) is not swallowed by the bare-client branch.
    orchestrator: Option<&OrchestratorLaunchConfig>,
    // Phase A.2: the merged, enabled user MCP servers, injected into the codex/claude
    // config. EMPTY ⇒ command_line byte-identical to before. NEVER reaches the mini
    // (this is the MAIN-coder launch path only — design §6).
    user_servers: &[user_mcp_config::UserMcpServer],
    // F65/A-1: vault setup-token presence from the caller's single vault read
    // (threaded via provider_env), NOT a re-read here.
    vault_token_present: bool,
) -> Result<(PathBuf, String), String> {
    let is_custom = custom_command.is_some();
    let command_line = if let Some(orchestrator) = orchestrator {
        // L2.4 LOCAL DEVBOULE ORCHESTRATOR: set the binary's non-secret env via
        // `$env:` and invoke the resolved binary. No prompt argv (it is autonomous);
        // the launch token + Exa key arrive via the spawning process env
        // (provider_env), so they are never on the binary's argv (B1 invariant).
        orchestrator_launch_script(orchestrator)
    } else if let Some(command) = custom_command {
        // CUSTOM CLIENT: run the operator-configured command line VERBATIM. The
        // prompt is delivered via $env:ASPIS_AGENT_PROMPT_FILE (0600 temp file)
        // only — never on the system clipboard (HE-3). The command is the
        // operator's own (unlock-gated config); we do NOT shell-escape it.
        //
        // HE-4 trust model: operator-trusted config integrity assumed. Provider
        // vault secrets are NOT injected into this launch env (see
        // `provider_env_for_launch`).
        //
        // B1 INVARIANT: the launch token lives ONLY in the restricted prompt file
        // — NEVER on argv, NEVER on the clipboard, and NEVER echoed to the PTY
        // (no `Write-Host $prompt` here), so it cannot leak into the ConPTY snapshot.
        command.to_string()
    } else if executable.is_empty() {
        // Bare/other client: nothing to exec. Prompt stays at ASPIS_AGENT_PROMPT_FILE
        // (HE-3: not on the clipboard; no Write-Host of `$prompt` — that would print
        // the token into the ConPTY ring buffer / snapshot / xterm viewer).
        String::new()
    } else if client == "codex" {
        let app_bin = resolve_app_binary();
        let app_bin = app_bin.as_ref().map(|p| p.to_string_lossy().into_owned());
        codex_launch_script(
            &crate::oracle::oracle_setup::resolve_oracle_python(),
            root_path,
            management_root,
            projects_dir,
            model,
            app_bin.as_deref(),
            user_servers,
        )?
    } else if client == "claude" {
        let app_bin = resolve_app_binary();
        let app_bin = app_bin.as_ref().map(|p| p.to_string_lossy().into_owned());
        claude_launch_script(
            &crate::oracle::oracle_setup::resolve_oracle_python(),
            management_root,
            projects_dir,
            model,
            app_bin.as_deref(),
            user_servers,
        )?
    } else {
        executable.to_string()
    };

    let prompt_file = write_restricted_prompt_file(prompt)?;
    let prompt_path_label = ps_single_quote(&prompt_file.display().to_string());

    let root_label = ps_single_quote(&root_path.display().to_string());
    let management_root_label = ps_single_quote(&management_root.display().to_string());
    let projects_dir_label = ps_single_quote(&projects_dir.display().to_string());
    // Stable, unique window-title marker so focus_agent_terminal can find this
    // exact console window by substring later. Kept in sync via agent_window_title.
    // (Harmless under the app-hosted PTY path, where there is no OS window to find.)
    let window_title_label = ps_single_quote(&agent_window_title(agent_id));

    // Built-in clients + the orchestrator delete the token-bearing prompt file
    // immediately after reading it (built-ins pipe `$prompt` over STDIN;
    // orchestrator is autonomous and never needs it). CUSTOM and bare/other
    // clients instead expose it via $env:ASPIS_AGENT_PROMPT_FILE so the human /
    // arbitrary CLI can read it (macOS parity — HE-3 removed clipboard delivery,
    // so bare had no other channel on Windows). The ledger records the path so
    // stop_agent (and the spawn-failure rollback) still clean it up. The file stays
    // 0600 in its per-launch restricted directory either way.
    // HE-3: the launch token is never placed on the system clipboard.
    let keep_prompt_file = is_custom || (executable.is_empty() && orchestrator.is_none());
    let prompt_file_lifecycle = if keep_prompt_file {
        format!("$env:ASPIS_AGENT_PROMPT_FILE = {prompt_path_label}\n")
    } else {
        "$promptDir = Split-Path -Parent -LiteralPath $promptFile\n\
Remove-Item -LiteralPath $promptFile -Force -ErrorAction SilentlyContinue\n\
Remove-Item -LiteralPath $promptDir -Recurse -Force -ErrorAction SilentlyContinue\n"
            .to_string()
    };
    let copied_hint = if keep_prompt_file {
        "Write-Host 'Devboule agent prompt at' $env:ASPIS_AGENT_PROMPT_FILE '(not on clipboard)'\n"
    } else if orchestrator.is_some() {
        "Write-Host 'Devboule orchestrator is autonomous (no agent prompt file; not on clipboard).'\n"
    } else {
        "Write-Host 'Devboule agent prompt delivered via STDIN (not on clipboard).'\n"
    };
    // B1 (keep-file paths): the verbatim operator command / bare interactive shell
    // runs in THIS PowerShell scope, where `$prompt` still holds the launch token.
    // Built-ins pipe `$prompt` into the CLI and must keep it, but custom and bare
    // receive the prompt via the restricted $env:ASPIS_AGENT_PROMPT_FILE (the file
    // persists), so we wipe the in-scope variable BEFORE the command line so the
    // token is not readable from the running command's session. HE-3: no clipboard
    // copy of `$prompt`.
    let prompt_clear = if keep_prompt_file {
        "Remove-Variable -Name prompt -ErrorAction SilentlyContinue\n$prompt = $null\n"
    } else {
        ""
    };
    // GH-P5 (cooperative push enforcement, NOT a security sandbox). We set, on the
    // SPAWNED agent's environment only, git neutralizers so a CONFUSED cooperative
    // agent that runs a raw `git push` fails fast instead of silently publishing
    // through an ambient credential:
    //   - GIT_TERMINAL_PROMPT=0  → never block on an interactive credential prompt.
    //   - GIT_CONFIG_NOSYSTEM=1  → ignore the system-wide git config (system helper).
    //   - GIT_CONFIG_GLOBAL=<per-session file> → a generated global config that
    //     `[include]`s the user's REAL global config (so user.name/email,
    //     safe.directory, core.* survive → commit still works, no "dubious
    //     ownership") then RESETS `credential.helper` to empty AFTER the include, so
    //     NO ambient helper (Windows GCM, `gh`, ~/.git-credentials, osxkeychain) is
    //     consulted at credential-fill time.
    // F1/F2 (why this replaced the old empty-helper env triple of count/key/value):
    //   - On Windows setting the env VALUE var to '' DELETES the variable (Win32
    //     SetEnvironmentVariable treats empty as delete), so git saw count=1 + a key
    //     but no value → `fatal: unable to parse command-line config` on EVERY git
    //     command. An empty value in a config FILE works fine.
    //   - GIT_CONFIG_NOSYSTEM only strips SYSTEM config; a helper in the user's GLOBAL
    //     ~/.gitconfig was still consulted. GIT_CONFIG_GLOBAL replaces the global file
    //     entirely (our include+reset), closing that gap.
    // RESIDUAL LIMIT (Finding B — DOCUMENTED, by design): BEST-EFFORT cooperative, NOT
    // a sandbox. A determined or compromised agent can still override this (its own
    // `git -c credential.helper=...`, a fresh GIT_CONFIG_GLOBAL it points elsewhere, or
    // `gh auth`), and on a box where AM's PAT is the SOLE configured credential it could
    // find a path to it. This only stops a cooperative agent that misfires a raw push on
    // a box with an ambient helper — publishing is meant to go through request_git_push
    // + human approval. The push-gate (P4) and these neutralizers reinforce, they do not
    // contain.
    let session_gitconfig = write_session_gitconfig()?;
    let session_gitconfig_label = ps_single_quote(&session_gitconfig.display().to_string());
    // F36: isolate product Claude from the operator's personal ~/.claude (CLAUDE.md,
    // skills, allowlists). Same helper as cloud duplex; best-effort if mkdir fails.
    // F65: when vault setup-token is present (caller-threaded, single vault read),
    // drop stale .credentials.json so the injected CLAUDE_CODE_OAUTH_TOKEN is sole auth.
    let claude_config_env = if client == "claude" {
        match crate::backend::cloud_claude_config::ensure_claude_product_config_dir(
            projects_dir,
            agent_id,
            vault_token_present,
        ) {
            Ok(dir) => {
                let label = ps_single_quote(&dir.display().to_string());
                format!("$env:CLAUDE_CONFIG_DIR = {label}\n")
            }
            Err(e) => {
                eprintln!(
                    "agent_spawn windows: CLAUDE_CONFIG_DIR isolation failed ({e}); \
                     Claude may inherit owner ~/.claude (F36 degraded)"
                );
                String::new()
            }
        }
    } else {
        String::new()
    };
    // HE-3: do NOT `Set-Clipboard -Value $prompt` — the prompt embeds the launch
    // token and any local process can read the clipboard. Token stays in the 0600
    // prompt file / `$prompt` for STDIN only.
    let script = format!(
        "$Host.UI.RawUI.WindowTitle = {window_title_label}\n\
$promptFile = {prompt_path_label}\n\
$prompt = Get-Content -Raw -LiteralPath $promptFile\n\
{prompt_file_lifecycle}\
$env:DEVBOULE_ROOT = {management_root_label}\n\
$env:ASPIS_PROJECTS_DIR = {projects_dir_label}\n\
$env:GIT_TERMINAL_PROMPT = '0'\n\
$env:GIT_CONFIG_NOSYSTEM = '1'\n\
$env:GIT_CONFIG_GLOBAL = {session_gitconfig_label}\n\
{claude_config_env}\
if ($env:PYTHONPATH) {{ $env:PYTHONPATH = {management_root_label} + ';' + $env:PYTHONPATH }} else {{ $env:PYTHONPATH = {management_root_label} }}\n\
{copied_hint}\
Write-Host 'Working root:' {root_label}\n\
Write-Host 'MCP root:' {management_root_label}\n\
{prompt_clear}\
{command_line}\n",
    );
    Ok((prompt_file, script))
}

#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
fn spawn_agent_terminal_impl(
    agent_id: &str,
    root_path: &Path,
    client: &str,
    executable: &str,
    custom_command: Option<&str>,
    prompt: &str,
    management_root: &Path,
    projects_dir: &Path,
    model: Option<&str>,
    provider_env: &[AgentLaunchEnv],
    orchestrator: Option<&OrchestratorLaunchConfig>,
    user_servers: &[user_mcp_config::UserMcpServer],
) -> Result<SpawnedAgent, String> {
    let (prompt_file, script) = build_windows_agent_script(
        agent_id,
        root_path,
        client,
        executable,
        custom_command,
        prompt,
        management_root,
        projects_dir,
        model,
        orchestrator,
        user_servers,
        vault_oauth_present_in(provider_env),
    )?;
    // Launch through conhost.exe so the agent always gets its OWN dedicated
    // CLASSIC console window (tagged with the unique title above), not a shared
    // Windows Terminal tab. On Win11 the default terminal may be Windows Terminal,
    // which would group every agent into tabs of one window and break per-agent
    // focus; conhost forces a standalone console we can find and foreground.
    let spawn_result = Command::new("conhost.exe")
        .arg("powershell.exe")
        .arg("-NoExit")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-Command")
        .arg(script)
        .envs(
            provider_env_for_launch(custom_command, provider_env)
                .iter()
                .map(|env| (env.name.as_str(), env.value.as_str())),
        )
        .current_dir(root_path)
        .spawn();

    match spawn_result {
        Ok(child) => {
            let pid = child.id();
            // Capture the process creation time NOW, while we know this pid is the
            // process we just spawned. Stored in the ledger as the anti-pid-reuse
            // fingerprint for the verified-pid stop/focus fallback.
            let creation_time = process_creation_time(pid);
            Ok(SpawnedAgent {
                pid,
                creation_time,
                prompt_file: Some(prompt_file),
            })
        }
        Err(e) => {
            // The launched shell never ran, so it cannot delete the temp prompt.
            // Remove it here so the token does not linger on disk.
            remove_restricted_temp_file(&prompt_file);
            Err(format!("Could not launch agent terminal: {e}"))
        }
    }
}

// UNVERIFIED on macOS — needs testing on a real Mac.
//
// Best-effort macOS terminal launch. There is no `conhost`/per-window console
// model like Windows: we ask Terminal.app (via `osascript`) to open a new window
// running a generated shell script. That script sets the window title to the
// stable `Aspis Agent {id}` marker (so the focus command can find it by name),
// loads the prompt from a 0600 temp file (HE-3: never pbcopy's the token), exports
// the same env vars the Windows path sets, cd's to the working root and finally
// runs the codex/claude CLI (or leaves the prompt file for bare/other clients).
//
// PID caveat: the pid we capture is the `osascript` helper process, NOT the
// Terminal shell that actually runs the agent. We store it for parity, but
// killing it will not stop the agent (see stop_agent's unix branch TODO). HE-1:
// the osascript Child is reaped on a detached thread so it cannot zombie.
/// SHARED macOS launch-script builder used by BOTH the external Terminal.app path
/// (`spawn_agent_terminal_impl`) and the app-hosted PTY path
/// (`spawn_agent_terminal_app_impl`). Mirrors `build_windows_agent_script`: it
/// guarantees identical prompt-file handling (token-bearing prompt read from a
/// 0o600 temp file, never on argv / never on the clipboard — HE-3) and
/// identical env exports (management root, projects dir, PYTHONPATH, profile
/// mode, provider env — HE-4: provider secrets omitted for custom_command).
/// Returns the restricted prompt-file path (so the caller can delete it if the
/// spawn fails) and the shell script text.
#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_macos_agent_script(
    agent_id: &str,
    root_path: &Path,
    client: &str,
    executable: &str,
    // Some(command) for a configured custom client; None for a built-in.
    custom_command: Option<&str>,
    prompt: &str,
    management_root: &Path,
    projects_dir: &Path,
    model: Option<&str>,
    provider_env: &[AgentLaunchEnv],
    // L2.4: Some for the local Devboule orchestrator client (the resolved binary +
    // its non-secret env). None for codex/claude/custom — keeps their cli_line
    // byte-identical. Dispatched FIRST so the orchestrator (whose `executable` is
    // empty) is not swallowed by the bare-client branch.
    orchestrator: Option<&OrchestratorLaunchConfig>,
    // FIX 2 — how the caller RUNS the returned script, which decides where secrets go:
    //   * `true`  (external Terminal.app path): the script is written to a 0600 temp
    //     file and run as `bash <file>`. There is NO out-of-band env channel (osascript
    //     spawns Terminal), so `provider_env` MUST be exported INSIDE the script, and
    //     the script SELF-DELETES (`rm -f "$0"`) the moment bash starts so the secrets
    //     file does not linger after a successful launch.
    //   * `false` (in-app PTY path): the script is passed as `zsh -ic <script>` and the
    //     caller ALSO injects every `provider_env` entry via `cmd.env(...)`. Exporting
    //     the secrets a SECOND time inside the script would put them on the `-ic` argv
    //     (visible via `ps`/argv to other processes) — the exact B1 leak. So we SKIP the
    //     in-script `provider_env` export here; there is no temp file to self-delete.
    runs_from_temp_file: bool,
    // Phase A.2: the merged, enabled user MCP servers injected into the codex/claude
    // config. EMPTY ⇒ cli_line byte-identical to before. NEVER reaches the mini (design §6).
    user_servers: &[user_mcp_config::UserMcpServer],
) -> Result<(PathBuf, String), String> {
    // Same temp-file delivery contract as Windows: keep the launch-token-bearing
    // prompt off the child argv. The generated shell script reads it from a 0600
    // file (HE-3: never pbcopy), then deletes it (built-ins that consume STDIN) or
    // exposes it via $ASPIS_AGENT_PROMPT_FILE (custom / bare). The file is locked
    // to 0o600 (see the unix branch in write_restricted_prompt_file).
    let is_custom = custom_command.is_some();
    let prompt_file = write_restricted_prompt_file(prompt)?;

    let cli_line = if let Some(orchestrator) = orchestrator {
        // L2.4 LOCAL DEVBOULE ORCHESTRATOR: run the resolved binary with its
        // non-secret env set inline. The binary takes no prompt argv (it is
        // autonomous); the launch token + Exa key arrive via provider_env (env only).
        macos_orchestrator_launch_line(orchestrator)
    } else if let Some(command) = custom_command {
        // CUSTOM CLIENT: run the operator-configured command verbatim. The prompt is
        // delivered via $ASPIS_AGENT_PROMPT_FILE only (HE-3: no clipboard token).
        // HE-4 trust model: operator-trusted config integrity assumed; vault secrets
        // are NOT exported into this launch (see `provider_env_for_launch` / the
        // `!is_custom` guard on the in-script export block below).
        // B1: the launch token is never on argv and never echoed to the PTY.
        command.to_string()
    } else if executable.is_empty() {
        // Bare/other client: nothing to exec; prompt stays at ASPIS_AGENT_PROMPT_FILE
        // (HE-3: not on the clipboard).
        String::new()
    } else if client == "codex" {
        let app_bin = resolve_app_binary();
        let app_bin = app_bin.as_ref().map(|p| p.to_string_lossy().into_owned());
        macos_codex_launch_line(
            &crate::oracle::oracle_setup::resolve_oracle_python(),
            root_path,
            management_root,
            projects_dir,
            model,
            app_bin.as_deref(),
            user_servers,
        )?
    } else if client == "claude" {
        let app_bin = resolve_app_binary();
        let app_bin = app_bin.as_ref().map(|p| p.to_string_lossy().into_owned());
        macos_claude_launch_line(
            &crate::oracle::oracle_setup::resolve_oracle_python(),
            management_root,
            projects_dir,
            model,
            app_bin.as_deref(),
            user_servers,
        )?
    } else {
        sh_single_quote(executable)
    };

    let window_title = agent_window_title(agent_id);
    let mut script = String::new();
    // FIX 2(b) — external Terminal.app path only: this script is a 0600 temp file that
    // carries the provider_env secrets (launch token, Exa key, etc.) in its
    // `export` block below, because Terminal.app gives no out-of-band env channel. Make
    // it SELF-DELETE the instant bash starts (`$0` is the script path under `bash
    // <file>`), so the secrets file is gone immediately on a SUCCESSFUL launch instead
    // of lingering until reboot. The contents stay valid in this already-running shell
    // (the file is read once at exec). MUST be the FIRST executable line. The in-app PTY
    // path runs `zsh -ic <script>` where `$0` is the shell, not a file, and injects the
    // secrets via cmd.env rather than the in-script export block — so it neither needs
    // nor wants this line (it does not pass `runs_from_temp_file`).
    if runs_from_temp_file {
        script.push_str("rm -f \"$0\" 2>/dev/null || true\n");
    }
    // Set the Terminal window/tab title via the OSC-0 escape so the focus command
    // can match it by name later (mirrors the Windows RawUI.WindowTitle marker).
    script.push_str(&format!(
        "printf '\\033]0;%s\\007' {}\n",
        sh_single_quote(&window_title)
    ));
    // Read the prompt from the restricted temp file into $PROMPT for STDIN piping.
    // HE-3: do NOT `pbcopy` the token-bearing prompt — any local process can read
    // the system clipboard. Token stays in the 0600 file / $PROMPT only.
    script.push_str(&format!(
        "ASPIS_PROMPT_FILE={}\n",
        sh_single_quote(&prompt_file.display().to_string())
    ));
    script.push_str("PROMPT=\"$(cat \"$ASPIS_PROMPT_FILE\")\"\n");
    // Keep the 0600 prompt file for custom (CLI reads ASPIS_AGENT_PROMPT_FILE) and
    // bare/other (no STDIN consumer). Built-ins + orchestrator delete after load.
    let keep_prompt_file = is_custom || (executable.is_empty() && orchestrator.is_none());
    if keep_prompt_file {
        script.push_str("export ASPIS_AGENT_PROMPT_FILE=\"$ASPIS_PROMPT_FILE\"\n");
    } else {
        // FIX 2: the prompt file lives inside a per-launch restricted directory;
        // remove the whole directory so nothing (and no empty restricted dir)
        // lingers once a built-in CLI has the prompt over STDIN.
        script.push_str("rm -rf \"$(dirname \"$ASPIS_PROMPT_FILE\")\" 2>/dev/null || true\n");
    }
    // Export the same env vars the Windows path sets.
    script.push_str(&format!(
        "export DEVBOULE_ROOT={}\n",
        sh_single_quote(&management_root.display().to_string())
    ));
    script.push_str(&format!(
        "export ASPIS_PROJECTS_DIR={}\n",
        sh_single_quote(&projects_dir.display().to_string())
    ));
    // GH-P5 (cooperative push enforcement, NOT a security sandbox) — mirror of the
    // Windows builder's git neutralizers, exported on the SPAWNED agent's environment
    // so a CONFUSED cooperative agent's raw `git push` fails fast instead of
    // publishing through an ambient credential:
    //   - GIT_TERMINAL_PROMPT=0  → never block on an interactive credential prompt.
    //   - GIT_CONFIG_NOSYSTEM=1  → ignore the system-wide git config.
    //   - GIT_CONFIG_GLOBAL=<per-session file> → a generated global config that
    //     `[include]`s the user's REAL global config (so user.name/email,
    //     safe.directory, core.* survive → commit works, no "dubious ownership")
    //     then RESETS `credential.helper` to empty AFTER the include, so NO inherited
    //     helper (osxkeychain / `gh` / ~/.git-credentials) is consulted at fill time.
    // F1/F2: this replaced the old empty-helper env triple (count/key/value), which
    // was broken (an empty env var is deleted on Windows → `fatal: unable to parse
    // command-line config`) and left a GLOBAL ~/.gitconfig helper consulted. An empty
    // value in a config FILE works; GIT_CONFIG_GLOBAL replaces the whole global file.
    // RESIDUAL LIMIT (Finding B — DOCUMENTED, by design): BEST-EFFORT cooperative, NOT
    // a sandbox. A determined/compromised agent can override this (its own
    // `git -c credential.helper=...`, a fresh GIT_CONFIG_GLOBAL, or `gh auth`); on a box
    // where AM's PAT is the sole credential it could still reach it. This only stops a
    // cooperative misfire on a box with an ambient helper — publishing goes through
    // request_git_push + human approval (P4). See the Windows builder for the rationale.
    let session_gitconfig = write_session_gitconfig()?;
    script.push_str("export GIT_TERMINAL_PROMPT='0'\n");
    script.push_str("export GIT_CONFIG_NOSYSTEM='1'\n");
    script.push_str(&format!(
        "export GIT_CONFIG_GLOBAL={}\n",
        sh_single_quote(&session_gitconfig.display().to_string())
    ));
    // F36: isolate product Claude from the operator's personal ~/.claude.
    // Set in-script on both PTY and external Terminal paths (non-secret path).
    // F65: vault presence from caller-threaded provider_env (single vault read at
    // launch assembly) — do not re-read the vault here (audit A-1).
    if client == "claude" {
        let vault_token_present = vault_oauth_present_in(provider_env);
        match crate::backend::cloud_claude_config::ensure_claude_product_config_dir(
            projects_dir,
            agent_id,
            vault_token_present,
        ) {
            Ok(dir) => {
                script.push_str(&format!(
                    "export CLAUDE_CONFIG_DIR={}\n",
                    sh_single_quote(&dir.display().to_string())
                ));
            }
            Err(e) => {
                eprintln!(
                    "agent_spawn macos: CLAUDE_CONFIG_DIR isolation failed ({e}); \
                     Claude may inherit owner ~/.claude (F36 degraded)"
                );
            }
        }
    }
    script.push_str(&format!(
        "if [ -n \"$PYTHONPATH\" ]; then export PYTHONPATH={mr}:\"$PYTHONPATH\"; else export PYTHONPATH={mr}; fi\n",
        mr = sh_single_quote(&management_root.display().to_string())
    ));
    // FIX 2(a) — provider env vars (launch token, Exa key, the orchestrator's
    // launch token + Exa key, etc.) are SECRETS. Export them IN-SCRIPT ONLY on the
    // external Terminal.app path (`runs_from_temp_file`), where there is no other env
    // channel and the script file is 0600 + self-deleting. On the in-app PTY path the
    // caller injects every one of these via `cmd.env(...)`, so re-exporting them here
    // would also place them on the `zsh -ic <script>` ARGV (readable via `ps`/argv) —
    // the B1 leak. So SKIP the in-script export there; cmd.env is the sole channel.
    // (The non-secret GIT neutralizers + PYTHONPATH above stay in-script on BOTH paths
    // because the PTY caller does NOT set those via cmd.env.)
    //
    // HE-4: also skip when `custom_command` is set — that path interpolates an
    // operator-trusted command unescaped (RCE if config is compromised). Isolating
    // vault secrets from the custom launch env is the alpha mitigation; config
    // integrity is assumed.
    if runs_from_temp_file && !is_custom {
        for env in provider_env_for_launch(custom_command, provider_env) {
            script.push_str(&format!(
                "export {}={}\n",
                shell_env_name(&env.name),
                sh_single_quote(&env.value)
            ));
        }
    }
    script.push_str(&format!(
        "cd {} || true\n",
        sh_single_quote(&root_path.display().to_string())
    ));
    if keep_prompt_file {
        script.push_str(
            "echo \"Devboule agent prompt at $ASPIS_AGENT_PROMPT_FILE (not on clipboard)\"\n",
        );
    } else if orchestrator.is_some() {
        script.push_str(
            "echo 'Devboule orchestrator is autonomous (no agent prompt file; not on clipboard).'\n",
        );
    } else {
        script.push_str("echo 'Devboule agent prompt delivered via STDIN (not on clipboard).'\n");
    }
    // FIX 2(c) [corrected by the max-recall adversarial pass] — clear `$PROMPT` before the
    // command line for every client EXCEPT the codex/claude built-ins. `$PROMPT` holds the
    // full launch prompt, which embeds the app-issued LAUNCH TOKEN; under `zsh -ic`/an
    // interactive bash it is the prompt-string variable, so a token-bearing `$PROMPT` would
    // otherwise LINGER in the interactive PTY shell. For custom (prompt delivered via
    // $ASPIS_AGENT_PROMPT_FILE), the orchestrator (reads config from env, not the prompt),
    // and bare/other clients, nothing downstream needs it → clear it. BUT the codex/claude
    // `cli_line` is `printf '%s' "$PROMPT" | codex …` — it MUST keep `$PROMPT` set or the
    // CLI receives an empty task (the regression the adversarial verify caught). So unset
    // it UNLESS the built cli_line is the codex/claude branch that consumes it.
    let cli_consumes_prompt = orchestrator.is_none()
        && !is_custom
        && !executable.is_empty()
        && matches!(client, "codex" | "claude");
    if !cli_consumes_prompt {
        script.push_str("unset PROMPT\n");
    }
    if !cli_line.is_empty() {
        script.push_str(&cli_line);
        script.push('\n');
    }

    Ok((prompt_file, script))
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn spawn_agent_terminal_impl(
    agent_id: &str,
    root_path: &Path,
    client: &str,
    executable: &str,
    custom_command: Option<&str>,
    prompt: &str,
    management_root: &Path,
    projects_dir: &Path,
    model: Option<&str>,
    provider_env: &[AgentLaunchEnv],
    orchestrator: Option<&OrchestratorLaunchConfig>,
    user_servers: &[user_mcp_config::UserMcpServer],
) -> Result<SpawnedAgent, String> {
    // HE-2: start-path sweep of stale secrets-bearing launch script dirs left
    // when a prior launch never reached bash's self-delete (`rm -f "$0"`).
    sweep_stale_macos_launch_script_dirs(&std::env::temp_dir(), MACOS_LAUNCH_SCRIPT_STALE_SECS);

    let (prompt_file, script) = build_macos_agent_script(
        agent_id,
        root_path,
        client,
        executable,
        custom_command,
        prompt,
        management_root,
        projects_dir,
        model,
        // Pass the UNFILTERED provider env so vault_oauth_present_in (F65/CLAUDE
        // isolation) still sees real vault presence. HE-4 secret stripping for the
        // custom_command path is applied inside the builder (export block skips
        // custom; provider_env_for_launch) — do not pre-filter here or the OAuth
        // hint collapses to "absent".
        provider_env,
        orchestrator,
        // FIX 2 — external Terminal.app path: osascript runs `bash <script_file>`, so
        // there is no cmd.env channel and the provider_env secrets MUST be exported
        // in-script (except HE-4 custom). The script is written to a 0600 temp file
        // just below, so the builder also injects the `rm -f "$0"` self-delete (first
        // line) to remove that secrets file the moment bash starts. -> true.
        true,
        user_servers,
    )?;

    // Write the generated script to its own restricted temp file and have Terminal
    // run it. Embedding a multi-line script directly inside an AppleScript string
    // is brittle (quoting/escaping); a file path is robust. HE-2: this file may
    // carry provider_env secrets — cleaned on spawn failure / osascript non-zero
    // / stale sweep if bash never self-deletes it.
    let script_file = write_restricted_script_file(&script)?;
    let script_path = script_file.display().to_string();

    // AppleScript: open a NEW Terminal window running our script via `bash`, then
    // bring Terminal to the foreground. `osascript -e <line> -e <line>` runs the
    // statements in order.
    let applescript_do = format!(
        "tell application \"Terminal\" to do script {}",
        applescript_quote(&format!("bash {}", sh_single_quote(&script_path)))
    );

    let spawn_result = Command::new("osascript")
        .arg("-e")
        .arg(&applescript_do)
        .arg("-e")
        .arg("tell application \"Terminal\" to activate")
        .spawn();

    match spawn_result {
        // NOTE: this is the osascript pid, not the Terminal shell pid. stop_agent
        // on macOS closes the Terminal window by its EXACT title instead of killing
        // this pid, so the pid is stored only for parity. creation_time is None on
        // macOS (the verified-pid fallback is Windows-only).
        // HE-1: reap osascript on a detached thread so the Child is not dropped
        // without wait (zombie accumulation). HE-2: if osascript fails, bash never
        // ran the self-delete — remove the secrets-bearing script file AND the
        // token-bearing 0600 prompt file (both live in restricted temp dirs).
        Ok(mut child) => {
            let pid = child.id();
            let script_file_for_reap = script_file;
            let prompt_file_for_reap = prompt_file.clone();
            std::thread::spawn(move || match child.wait() {
                Ok(status) if status.success() => {}
                _ => {
                    remove_restricted_temp_file(&script_file_for_reap);
                    remove_restricted_temp_file(&prompt_file_for_reap);
                }
            });
            Ok(SpawnedAgent {
                pid,
                creation_time: None,
                prompt_file: Some(prompt_file),
            })
        }
        Err(e) => {
            // The Terminal script never ran, so it cannot delete the temp files.
            remove_restricted_temp_file(&prompt_file);
            remove_restricted_temp_file(&script_file);
            Err(format!("Could not launch agent terminal: {e}"))
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
#[allow(clippy::too_many_arguments)]
fn spawn_agent_terminal_impl(
    _agent_id: &str,
    _root_path: &Path,
    _client: &str,
    _executable: &str,
    _custom_command: Option<&str>,
    _prompt: &str,
    _management_root: &Path,
    _projects_dir: &Path,
    _model: Option<&str>,
    _provider_env: &[AgentLaunchEnv],
    _orchestrator: Option<&OrchestratorLaunchConfig>,
    _user_servers: &[user_mcp_config::UserMcpServer],
) -> Result<SpawnedAgent, String> {
    Err("Agent terminal launch is supported on Windows and macOS only.".into())
}

/// GH-P5 (F1/F2): convert an absolute filesystem path to the forward-slash form
/// git expects inside a config `[include] path = ...` line. git on Windows treats a
/// backslash as an escape inside config values, so `C:\Users\...` would be mangled;
/// forward slashes (`C:/Users/...`) are accepted on every platform and are what git
/// itself emits. Empirically (git 2.54, Windows): a backslash include path silently
/// fails to resolve (user.name/email come back empty), a forward-slash absolute path
/// resolves correctly.
fn gitconfig_include_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// C6: compute the read roots an agent child needs beyond its project cwd:
/// the per-launch prompt dir (if given), the session gitconfig dir, and the
/// REAL global gitconfig file(s) the session config [include]s (level-1 only;
/// includeIf targets are NOT covered — documented limitation). On Windows the
/// deny-by-default AppContainer cannot read user-only %TEMP% or home files
/// without these grants; every sandboxed agent spawn builder (app-hosted PTY,
/// one-shot mini, cloud duplex) must apply them.
///
/// NOTE: the real gitconfig may contain http.extraHeader/credential sections
/// (users store PATs there) — granting it is a deliberate, documented widening
/// vs the C5 no-home policy; copying only identity keys is a follow-up.
#[cfg(target_os = "windows")]
pub(crate) fn agent_sandbox_read_roots(prompt_file: Option<&Path>) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(prompt_file) = prompt_file {
        if let Some(parent) = prompt_file.parent() {
            roots.push(parent.to_path_buf());
        }
    }
    if let Ok(gitconfig) = write_session_gitconfig() {
        if let Some(parent) = gitconfig.parent() {
            roots.push(parent.to_path_buf());
        }
    }
    for real in real_global_gitconfig_paths() {
        roots.push(real);
    }
    roots
}

/// GH-P5 (F1/F2): the absolute paths of the user's REAL global git config(s) that
/// our per-session config should `[include]` so commit identity (user.name/email),
/// safe.directory and core.* survive while we reset ONLY the credential helper.
///
/// Returns every candidate that EXISTS (git would ignore a missing include path
/// anyway, but only including real files keeps the generated config tidy and the
/// behaviour obvious). Order matches git's own global-config precedence: the
/// XDG location (`$XDG_CONFIG_HOME/git/config` or `~/.config/git/config`) is read
/// BEFORE `~/.gitconfig`, so we list it first.
///
/// cfg-gated because the home/profile resolution differs per platform.
fn real_global_gitconfig_paths() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();

    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME").map(PathBuf::from);

    // XDG location first (git reads it before ~/.gitconfig).
    let xdg_git_config = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(xdg) if !xdg.is_empty() => Some(PathBuf::from(xdg).join("git").join("config")),
        _ => home
            .as_ref()
            .map(|h| h.join(".config").join("git").join("config")),
    };
    if let Some(path) = xdg_git_config {
        if path.is_file() {
            out.push(path);
        }
    }

    // The classic ~/.gitconfig.
    if let Some(home) = home {
        let dot_gitconfig = home.join(".gitconfig");
        if dot_gitconfig.is_file() {
            out.push(dot_gitconfig);
        }
    }

    out
}

/// GH-P5 (F1/F2): write a per-session git GLOBAL config file that NEUTRALIZES any
/// inherited credential helper at credential-FILL time while PRESERVING the user's
/// commit identity, safe.directory and core.* settings. The agent launch scripts
/// point `GIT_CONFIG_GLOBAL` at this file.
///
/// Contents:
/// ```text
/// [include]
///     path = <abs path to the user's real global gitconfig>   ; (each that exists)
/// [credential]
///     helper =
/// ```
/// git reads the `[include]`d real config FIRST, then our `[credential] helper =`
/// (empty value) which RESETS the inherited helper list to empty — so no helper is
/// consulted when git fills a credential, while user.name / user.email /
/// safe.directory / core.* from the real config remain visible (commit + no
/// "dubious ownership"). An empty value in a config FILE works on every platform
/// (unlike `$env:GIT_CONFIG_VALUE_0 = ''`, which Win32 SetEnvironmentVariable treats
/// as a DELETE — the reason the old GIT_CONFIG_* env triple was broken on Windows).
///
/// EMPIRICALLY VERIFIED (git 2.54, Windows): with this file as GIT_CONFIG_GLOBAL
/// (+ GIT_CONFIG_NOSYSTEM=1), `git credential fill` does NOT invoke the stored
/// helper and falls through to a (suppressed) terminal prompt; `git config user.name`
/// / `user.email` are still readable.
///
/// The file holds NO secret (an include + an empty helper), so it is written to a
/// stable, app-controlled scratch directory under the OS temp root
/// (`aspis-agent-gitconfig/`) and OVERWRITTEN on every spawn (regenerated so it
/// always reflects the current real global path). Cleanup is non-critical.
///
/// RESIDUAL LIMIT (by design): BEST-EFFORT cooperative, NOT a sandbox. A determined
/// agent can still set its own `git -c credential.helper=...`, override
/// GIT_CONFIG_GLOBAL, or run `gh auth`; the real gate is request_git_push + human
/// approval (P4).
pub(crate) fn write_session_gitconfig() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("aspis-agent-gitconfig");
    fs::create_dir_all(&dir)
        .map_err(|e| format!("Could not create the per-session gitconfig directory: {e}"))?;
    let path = dir.join("session.gitconfig");

    let mut contents = String::from("[include]\n");
    for real in real_global_gitconfig_paths() {
        contents.push_str(&format!("\tpath = {}\n", gitconfig_include_path(&real)));
    }
    // The empty value RESETS the inherited credential.helper list to empty. It must
    // come AFTER the include so it wins over the real global's helper.
    contents.push_str("[credential]\n\thelper =\n");

    fs::write(&path, contents)
        .map_err(|e| format!("Could not write the per-session gitconfig: {e}"))?;
    Ok(path)
}

/// HE-2: age threshold for sweeping abandoned macOS launch-script dirs under the
/// OS temp root. Successful launches self-delete the `.sh` on bash start; failed
/// pre-exec paths can leave the secrets-bearing tree until this sweep.
#[cfg(target_os = "macos")]
const MACOS_LAUNCH_SCRIPT_STALE_SECS: u64 = 300;

/// HE-2: remove stale `aspis-agent-launch-*.d` directories under `temp_root` whose
/// mtime is at least `max_age_secs` old. Concurrent launches younger than the
/// threshold are preserved. Best-effort; errors are ignored.
#[cfg(target_os = "macos")]
fn sweep_stale_macos_launch_script_dirs(temp_root: &Path, max_age_secs: u64) {
    let Ok(entries) = fs::read_dir(temp_root) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("aspis-agent-launch-") && name.ends_with(".d")) {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let stale = match now.duration_since(modified) {
            Ok(age) => age.as_secs() >= max_age_secs,
            // Clock skew / future mtime: treat as stale so secrets cannot linger.
            Err(_) => true,
        };
        if stale {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

/// macOS-only: write a generated shell script to a restricted (0o600) temp file
/// so Terminal can `bash` it. May carry provider_env secrets on the external
/// path — cleaned on failure (HE-2) and self-deleted by bash on success.
// UNVERIFIED on macOS — needs testing on a real Mac.
#[cfg(target_os = "macos")]
fn write_restricted_script_file(script: &str) -> Result<PathBuf, String> {
    // Same owner-only-before-write contract as the prompt file (O_EXCL + 0o600).
    create_restricted_temp_file(script, "aspis-agent-launch-", ".sh")
}

/// macOS-only: single-quote a value for embedding inside a POSIX `sh`/`bash`
/// command line. Wraps in single quotes and escapes embedded single quotes via
/// the standard `'\''` idiom.
// UNVERIFIED on macOS — needs testing on a real Mac.
#[cfg(target_os = "macos")]
fn sh_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// macOS-only: quote a value for embedding inside an AppleScript string literal
/// (double-quoted; backslashes and double quotes escaped).
// UNVERIFIED on macOS — needs testing on a real Mac.
#[cfg(target_os = "macos")]
fn applescript_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// macOS-only: sanitize an env var name for `export NAME=value`. Env var names
/// from the vault are already simple ASCII identifiers, but guard against
/// injection by keeping only `[A-Za-z0-9_]` and prefixing a leading digit.
// UNVERIFIED on macOS — needs testing on a real Mac.
#[cfg(target_os = "macos")]
fn shell_env_name(name: &str) -> String {
    let mut cleaned: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        cleaned.insert(0, '_');
    }
    cleaned
}

/// macOS-only: build the codex CLI invocation line for the launch script. Mirrors
/// `codex_launch_script` (the Windows/PowerShell variant) but emits a single
/// POSIX-shell line that pipes the prompt via STDIN (keeping the launch token off
/// the argv) and passes the same `-c mcp_servers.devboule.*` config.
///
/// Honors `DEVBOULE_MCP_BACKEND` via the shared entry builder. Fail-closed: `Err`
/// when the backend cannot be resolved (e.g. rust selected but bin missing).
// UNVERIFIED on macOS — needs testing on a real Mac.
#[cfg(target_os = "macos")]
pub(crate) fn macos_codex_launch_line(
    python: &str,
    root_path: &Path,
    management_root: &Path,
    projects_dir: &Path,
    model: Option<&str>,
    app_bin: Option<&str>,
    // User-declared MCP servers (design Phase A.2). EMPTY ⇒ byte-identical to before.
    user_servers: &[user_mcp_config::UserMcpServer],
) -> Result<String, String> {
    let root_s = root_path.to_string_lossy().into_owned();
    let entry = crate::backend::mcp_backend::build_devboule_mcp_server_entry(
        crate::backend::mcp_backend::McpBackend::from_env(),
        python,
        management_root,
        projects_dir,
        app_bin,
    )?;
    let mut config_args = codex_devboule_settings_from_entry(&entry, management_root)?;
    // User servers AFTER the Oracle config args (design §5.1). EMPTY ⇒ no change.
    for server in user_servers {
        config_args.extend(codex_user_server_config_settings(server));
    }
    let mut line = String::from("printf '%s' \"$PROMPT\" | codex --cd ");
    line.push_str(&sh_single_quote(&root_s));
    if let Some(model) = model {
        line.push_str(" -m ");
        line.push_str(&sh_single_quote(model));
    }
    for config in &config_args {
        line.push_str(" -c ");
        line.push_str(&sh_single_quote(config));
    }
    Ok(line)
}

pub(crate) fn orchestrator_env_pairs(config: &OrchestratorLaunchConfig) -> Vec<(&'static str, String)> {
    let mut pairs: Vec<(&'static str, String)> = vec![
        ("DEVBOULE_OMLX_BASE_URL", config.omlx_base_url.to_string()),
        ("DEVBOULE_OMLX_MODEL", config.omlx_model.to_string()),
        ("DEVBOULE_CONTEXT_WINDOW", config.context_window.to_string()),
        ("DEVBOULE_MCP_PYTHON", config.mcp_python.to_string()),
        (
            "DEVBOULE_MCP_ROOT",
            config.mcp_root.to_string_lossy().into_owned(),
        ),
        (
            "DEVBOULE_MCP_PROJECTS_DIR",
            config.mcp_projects_dir.to_string_lossy().into_owned(),
        ),
        ("DEVBOULE_AGENT_ID", config.agent_id.to_string()),
        (
            "DEVBOULE_PROJECT_ROOT",
            config.project_root.to_string_lossy().into_owned(),
        ),
    ];
    // CLOUD (opt-in) NON-SECRET vars, appended ONLY when the configured kind is `cloud`
    // (both are empty for the local kinds, so a Local-mode launch stays byte-identical to a
    // pre-cloud one). The cloud API KEY is NEVER here — it rides via `provider_env`
    // (DEVBOULE_CLOUD_API_KEY), off argv (B1 invariant).
    if !config.cloud_base_url.trim().is_empty() {
        pairs.push(("DEVBOULE_CLOUD_BASE_URL", config.cloud_base_url.clone()));
        pairs.push(("DEVBOULE_CLOUD_MODEL", config.cloud_model.clone()));
    }
    if !config.app_bin.trim().is_empty() {
        pairs.push(("DEVBOULE_APP_BIN", config.app_bin.clone()));
    }
    // The activity-file bridge path, appended LAST and ONLY when present (so the
    // bridge-disabled case stays byte-identical to the prior output). Non-secret.
    if !config.activity_file.trim().is_empty() {
        pairs.push(("DEVBOULE_ACTIVITY_FILE", config.activity_file.clone()));
    }
    // The reverse-bridge steer inbox path, appended ONLY when present (steer-disabled
    // case stays byte-identical). Non-secret — it's just a per-agent file path.
    if !config.steer_file.trim().is_empty() {
        pairs.push(("DEVBOULE_STEER_FILE", config.steer_file.clone()));
    }
    // 3c — the Oracle-side project key for the planner's `plan_submit` (so the plan
    // surfaces under THIS project in the per-project Plans tab). Set ONLY when present;
    // an empty id is omitted (the binary's config reader treats absent == empty and the
    // planner escalates rather than mis-submitting). Non-secret.
    if !config.project_id.trim().is_empty() {
        pairs.push(("DEVBOULE_PROJECT_ID", config.project_id.clone()));
    }
    // v6 Phase 4 (resume): a STABLE-per-project session file so a re-spawned orchestrator
    // resumes its cumulative conversation instead of starting fresh. Placed in the steer
    // file's directory (the per-agent-state area), keyed by project id. SAFE by design: if
    // the path is not actually stable across re-spawns, resume simply never triggers and
    // the orchestrator starts fresh (today's behavior) — never a regression.
    if !config.project_id.trim().is_empty() && !config.steer_file.trim().is_empty() {
        if let Some(dir) = std::path::Path::new(&config.steer_file).parent() {
            let safe_id: String = config
                .project_id
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            let session_path = dir.join(format!("devboule-session-{safe_id}.txt"));
            pairs.push((
                "DEVBOULE_SESSION_FILE",
                session_path.to_string_lossy().into_owned(),
            ));
        }
    }
    // 3b — plan-first bias. Appended ONLY when set ("1"); when the toggle was OFF the
    // field is empty and the pair is omitted entirely, so a non-plan-first launch is
    // byte-identical to a pre-3b one. Non-secret.
    if !config.plan_first.trim().is_empty() {
        pairs.push(("DEVBOULE_PLAN_FIRST", config.plan_first.clone()));
    }
    // Phase B — the user MCP servers JSON array (DEVBOULE_USER_MCP_SERVERS), appended
    // ONLY when non-empty (no user servers ⇒ the field is "" ⇒ the pair is omitted, so
    // the launch is byte-identical to a pre-B one). This is the ORCHESTRATOR launch
    // (the local MAIN coder); the mini launch NEVER carries this var (design §6).
    if !config.user_mcp_servers_json.trim().is_empty() {
        pairs.push((
            "DEVBOULE_USER_MCP_SERVERS",
            config.user_mcp_servers_json.clone(),
        ));
    }
    // Phase 5 — the (orchestrator × language) persona block (DEVBOULE_LANG_SKILL), appended ONLY
    // when non-empty (no language ⇒ "" ⇒ the pair is omitted, byte-identical launch). The binary
    // threads it to whichever backend (oMLX/Ollama/Cloud) — backend-agnostic.
    if !config.lang_skill.trim().is_empty() {
        pairs.push(("DEVBOULE_LANG_SKILL", config.lang_skill.clone()));
    }
    // The project-context block (AGENTS.md/CLAUDE.md), already fenced + neutralized by the host —
    // appended ONLY when present (absent ⇒ the pair is omitted, byte-identical launch).
    if !config.project_context.trim().is_empty() {
        pairs.push(("DEVBOULE_PROJECT_CONTEXT", config.project_context.clone()));
    }
    // Orchestrator composer: the typed goal (the binary runs headless on it, plan-first) and the
    // auto-create toggle ("0" ⇒ don't create tasks on approval). Each appended ONLY when set, so an
    // interactive launch with neither is byte-identical to a pre-feature launch.
    if !config.initial_goal.trim().is_empty() {
        pairs.push(("DEVBOULE_GOAL", config.initial_goal.clone()));
    }
    if !config.auto_create.trim().is_empty() {
        pairs.push(("DEVBOULE_AUTO_CREATE", config.auto_create.clone()));
    }
    pairs
}

/// macOS-only: build the local Devboule orchestrator invocation LINE for the launch
/// script. Mirrors `macos_codex_launch_line`: a single POSIX-shell line that sets
/// the binary's NON-SECRET env (oMLX base/model, MCP python/root/projects-dir,
/// agent id, project root) and execs the resolved binary. UNLIKE codex there is no
/// `-c mcp_servers.*` config and NO prompt piped over STDIN: the binary is
/// autonomous (it spawns its own MCP server from the env and drives its own loop),
/// so it takes no prompt argv at all.
///
/// SECRETS (the launch token + Exa key) are deliberately ABSENT here — they are
/// injected via `provider_env` (the parent shell's already-`export`ed environment),
/// so they never appear on this line / the binary's argv (B1 invariant). The
/// env-vars set here are all non-secret config.
// UNVERIFIED on macOS — needs testing on a real Mac.
#[cfg(target_os = "macos")]
pub(crate) fn macos_orchestrator_launch_line(config: &OrchestratorLaunchConfig) -> String {
    // Each pair is emitted as an inline shell assignment `NAME=<sh-quoted value> ` that
    // prefixes the exec line (NOT `export` — a temporary, per-command assignment, which
    // the exec'd binary still inherits like env(1)). Only NON-SECRET config is set this
    // way; the loopback-only base URL is validated upstream (read_local_coder_backend).
    let pairs = orchestrator_env_pairs(config);
    let mut line = String::new();
    for (name, value) in &pairs {
        line.push_str(name);
        line.push('=');
        line.push_str(&sh_single_quote(value));
        line.push(' ');
    }
    // Exec the resolved binary (no argv prompt; it is autonomous).
    line.push_str(&sh_single_quote(&config.binary.to_string_lossy()));
    line
}

/// Windows/PowerShell variant: build the local Devboule orchestrator launch script
/// line. Mirrors `codex_launch_script`'s PowerShell shape but sets the binary's
/// NON-SECRET env via `$env:NAME = '<value>'` and invokes the resolved binary with
/// no argv prompt (the binary is autonomous). The two SECRETS (launch token + Exa
/// key) are injected via `provider_env` (the spawning process env), so they are
/// NEVER on this script line / the binary's argv (B1 invariant).
pub(crate) fn orchestrator_launch_script(config: &OrchestratorLaunchConfig) -> String {
    let pairs = orchestrator_env_pairs(config);
    let mut script = String::new();
    for (name, value) in &pairs {
        script.push_str(&format!("$env:{name} = {}\n", ps_single_quote(value)));
    }
    // Invoke the resolved binary by absolute path (no argv prompt; it is autonomous).
    script.push_str(&format!(
        "& {}",
        ps_single_quote(&config.binary.to_string_lossy())
    ));
    script
}

/// macOS-only: build the claude CLI invocation line for the launch script.
/// Mirrors `claude_launch_script`: passes the same MCP client config JSON via
/// `--mcp-config` and pipes the prompt over STDIN.
/// Fail-closed when the MCP entry cannot be built (rust bin missing, etc.).
// UNVERIFIED on macOS — needs testing on a real Mac.
#[cfg(target_os = "macos")]
pub(crate) fn macos_claude_launch_line(
    python: &str,
    management_root: &Path,
    projects_dir: &Path,
    model: Option<&str>,
    app_bin: Option<&str>,
    user_servers: &[user_mcp_config::UserMcpServer],
) -> Result<String, String> {
    let config =
        mcp_client_config_json(python, management_root, projects_dir, app_bin, user_servers)?;
    let model_flag = match model {
        Some(model) => format!("--model {} ", sh_single_quote(model)),
        None => String::new(),
    };
    Ok(format!(
        "printf '%s' \"$PROMPT\" | claude {}--mcp-config {}",
        model_flag,
        sh_single_quote(&config)
    ))
}

// MINOR 9 → P3: the old full-server mini grant stayed removed; the read-only,
// oracle_context-only scope now exists. The mini wires the SAME server via
// `codex_mcp_config_args` above, and the narrowing is SERVER-side: the mini
// registers as role "mini" (launch-token-bound), whose ROLE_ALLOWED_TOOLS is
// {agent_register, oracle_context} — project-mutation / spawn_mini_coder /
// censor_dispose are rejected at the MCP role gate, not hidden by config.

/// P3: the codex `-c mcp_servers.devboule.*` config tokens, UNQUOTED —
/// each caller applies its own shell quoting (PowerShell vs `/bin/sh`). Shared
/// by the FULL coder launch (`codex_launch_script`) and the read-only mini
/// grant (mini_coder_executor): both wire the SAME server; the mini's scope is
/// narrowed SERVER-side by its "mini" role (oracle_context only), never by the
/// client config. Extracted so the two call sites cannot drift.
///
/// Built from [`crate::backend::mcp_backend::build_devboule_mcp_server_entry`] so
/// Codex honors `DEVBOULE_MCP_BACKEND`. Fail-closed on resolution failure.
pub(crate) fn codex_mcp_config_args(
    python: &str,
    management_root: &Path,
    projects_dir: &Path,
    app_bin: Option<&str>,
    // User-declared MCP servers (design Phase A.2): emitted as `-c mcp_servers.<name>.*`
    // tokens AFTER the Oracle tokens. EMPTY ⇒ byte-identical to the pre-A.2 token list.
    // The mini wires the SAME server but passes an empty slice (mini-exclusion §6).
    user_servers: &[user_mcp_config::UserMcpServer],
) -> Result<Vec<String>, String> {
    let entry = crate::backend::mcp_backend::build_devboule_mcp_server_entry(
        crate::backend::mcp_backend::McpBackend::from_env(),
        python,
        management_root,
        projects_dir,
        app_bin,
    )?;
    let mut out = Vec::new();
    for setting in codex_devboule_settings_from_entry(&entry, management_root)? {
        out.push("-c".to_string());
        out.push(setting);
    }
    // User servers AFTER the Oracle tokens (design §5.1: Oracle first). Each emits a
    // `-c mcp_servers.<name>.*` block. With NO user servers this loop adds nothing, so the
    // token list is byte-identical to before A.2 (regression guard).
    for server in user_servers {
        out.extend(codex_user_server_config_tokens(server));
    }
    Ok(out)
}

/// Convert a shared MCP server entry JSON into Codex `mcp_servers.devboule.*`
/// KEY=VALUE settings (no `-c` prefix). Command, optional non-empty args, cwd,
/// and every string env key from the entry.
fn codex_devboule_settings_from_entry(
    entry: &serde_json::Value,
    management_root: &Path,
) -> Result<Vec<String>, String> {
    let command = entry
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "devboule MCP entry missing command".to_string())?;
    let mut settings = vec![format!(
        "mcp_servers.devboule.command={}",
        toml_string(command)
    )];

    if let Some(args) = entry.get("args").and_then(|v| v.as_array()) {
        let arg_refs: Vec<&str> = args.iter().filter_map(|a| a.as_str()).collect();
        // Match user-server / empty-args convention: omit `args=[]` (codex defaults
        // missing args to no arguments). Python backend always has non-empty args.
        if !arg_refs.is_empty() {
            settings.push(format!(
                "mcp_servers.devboule.args={}",
                toml_array(&arg_refs)
            ));
        }
    }

    let management_root_s = management_root.to_string_lossy();
    settings.push(format!(
        "mcp_servers.devboule.cwd={}",
        toml_string(management_root_s.as_ref())
    ));

    if let Some(env) = entry.get("env").and_then(|v| v.as_object()) {
        for (key, value) in env {
            if let Some(s) = value.as_str() {
                settings.push(format!(
                    "mcp_servers.devboule.env.{key}={}",
                    toml_string(s)
                ));
            }
        }
    }
    Ok(settings)
}

/// Build the codex config KEY=VALUE strings for ONE user server (no `-c` prefix; the
/// caller interleaves `-c`). Shared by the Windows `codex_mcp_config_args` (which wraps
/// each in a `-c` pair) and the macOS `macos_codex_launch_line` (which shell-quotes each
/// and prefixes `-c`), so the two codex paths cannot drift. Reuses the SAME
/// `toml_string`/`toml_array` helpers as the Oracle tokens. `name` is guarded
/// (no reserved prefix, never `devboule`) before it reaches here.
fn codex_user_server_config_settings(server: &user_mcp_config::UserMcpServer) -> Vec<String> {
    let name = &server.name;
    // `command` is always emitted. `args` is emitted ONLY when non-empty — matching the
    // Oracle tokens (which never emit an empty `args=[]`) and keeping the launch line
    // smaller; codex defaults a missing `args` to no arguments, same as `args=[]`.
    let mut settings = vec![format!(
        "mcp_servers.{name}.command={}",
        toml_string(&server.command)
    )];
    if !server.args.is_empty() {
        let arg_refs: Vec<&str> = server.args.iter().map(|s| s.as_str()).collect();
        settings.push(format!("mcp_servers.{name}.args={}", toml_array(&arg_refs)));
    }
    // env keys come from the (deterministically-ordered) BTreeMap so the token order is stable.
    for (key, value) in &server.env {
        settings.push(format!(
            "mcp_servers.{name}.env.{key}={}",
            toml_string(value)
        ));
    }
    settings
}

/// The Windows `codex_mcp_config_args` form: each setting wrapped as a `-c <setting>` pair
/// (the caller quotes the whole arg vector via `ps_single_quote` later).
fn codex_user_server_config_tokens(server: &user_mcp_config::UserMcpServer) -> Vec<String> {
    let mut out = Vec::new();
    for setting in codex_user_server_config_settings(server) {
        out.push("-c".to_string());
        out.push(setting);
    }
    out
}

pub(crate) fn codex_launch_script(
    python: &str,
    root_path: &Path,
    management_root: &Path,
    projects_dir: &Path,
    model: Option<&str>,
    app_bin: Option<&str>,
    user_servers: &[user_mcp_config::UserMcpServer],
) -> Result<String, String> {
    let root_s = root_path.to_string_lossy().into_owned();
    let mut args = vec!["--cd".to_string(), root_s];
    if let Some(model) = model {
        args.push("-m".to_string());
        args.push(model.to_string());
    }
    args.extend(codex_mcp_config_args(
        python,
        management_root,
        projects_dir,
        app_bin,
        user_servers,
    )?);
    let args = args
        .iter()
        .map(|value| ps_single_quote(value))
        .collect::<Vec<_>>()
        .join(", ");
    // Deliver the prompt via STDIN, not as a trailing native argv. Passing the
    // multi-line prompt as `$prompt` argv makes PowerShell word-split it and
    // mangle `<`/`>` (codex/claude then clap-error on "model>, message=..."). It
    // also keeps the embedded launch token off the codex command line.
    //
    // B1 + HE-3: the prompt/launch token must NEVER be written to the PTY stream
    // and must NEVER be placed on the system clipboard. Delivered to the CLI over
    // STDIN only — no `Write-Host $prompt`/`echo $prompt`/`Set-Clipboard`.
    Ok(format!(
        "$codexArgs = @({args})\n$prompt | & codex @codexArgs"
    ))
}

pub(crate) fn claude_launch_script(
    python: &str,
    management_root: &Path,
    projects_dir: &Path,
    model: Option<&str>,
    app_bin: Option<&str>,
    user_servers: &[user_mcp_config::UserMcpServer],
) -> Result<String, String> {
    let config =
        mcp_client_config_json(python, management_root, projects_dir, app_bin, user_servers)?
            .replace("'@", "' @");
    let model_flag = match model {
        Some(model) => format!("--model {} ", ps_single_quote(model)),
        None => String::new(),
    };
    // Deliver the prompt via STDIN, not as a trailing native argv (see
    // codex_launch_script for the full rationale): avoids PowerShell word-splitting
    // and `<`/`>` mangling, and keeps the embedded launch token off claude's
    // command line.
    //
    // B1 + HE-3: same as codex — the prompt/launch token is delivered over STDIN
    // only; never written to the PTY stream and never placed on the clipboard.
    Ok(format!(
        "$mcpConfig = @'\n{config}\n'@\n$prompt | & claude {model_flag}--mcp-config $mcpConfig"
    ))
}


fn toml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn toml_array(values: &[&str]) -> String {
    let values = values
        .iter()
        .map(|value| toml_string(value))
        .collect::<Vec<_>>()
        .join(",");
    format!("[{values}]")
}

#[cfg(test)]
mod he_security_tests {
    use super::*;
    use crate::backend::projects::AgentLaunchEnv;
    #[cfg(windows)]
    use std::path::PathBuf;

    fn secret_env_fixture() -> Vec<AgentLaunchEnv> {
        vec![
            AgentLaunchEnv {
                name: "DEVBOULE_MCP_LAUNCH_TOKEN".into(),
                value: "tok-secret-launch-he4-test".into(),
            },
            AgentLaunchEnv {
                name: "EXA_API_KEY".into(),
                value: "exa-secret-he4-test".into(),
            },
        ]
    }

    /// HE-4: custom_command path must not receive vault/provider secrets.
    #[test]
    fn custom_command_launch_env_omits_vault_secrets() {
        let envs = secret_env_fixture();
        assert!(
            provider_env_for_launch(Some("evil-cli --flag"), &envs).is_empty(),
            "custom_command must isolate provider secrets"
        );
        assert_eq!(
            provider_env_for_launch(None, &envs).len(),
            envs.len(),
            "built-in path keeps provider_env"
        );
    }

    /// HE-2: simulated launch-failure cleanup removes the restricted temp tree.
    #[cfg(target_os = "macos")]
    #[test]
    fn temp_script_removed_on_simulated_launch_failure() {
        let script_file =
            write_restricted_script_file("export SECRET='should-not-linger'\n").expect("write");
        assert!(script_file.is_file());
        let parent = script_file.parent().map(|p| p.to_path_buf());
        // Same cleanup the external spawn Err / osascript non-zero path runs.
        remove_restricted_temp_file(&script_file);
        assert!(
            !script_file.exists(),
            "secrets-bearing script must be gone after failure cleanup"
        );
        if let Some(parent) = parent {
            assert!(
                !parent.exists(),
                "restricted parent dir must be removed: {parent:?}"
            );
        }
    }

    /// HE-2: age-based sweep removes stale launch dirs, keeps fresh ones.
    #[cfg(target_os = "macos")]
    #[test]
    fn sweep_stale_launch_script_dirs_respects_age() {
        let base = std::env::temp_dir().join(format!(
            "devboule-he2-sweep-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();

        let stale = base.join("aspis-agent-launch-stale.d");
        let fresh = base.join("aspis-agent-launch-fresh.d");
        fs::create_dir_all(&stale).unwrap();
        fs::create_dir_all(&fresh).unwrap();
        fs::write(stale.join("aspis-agent-launch-stale.sh"), b"secret").unwrap();
        fs::write(fresh.join("aspis-agent-launch-fresh.sh"), b"secret").unwrap();

        // max_age = u64::MAX → nothing is old enough.
        sweep_stale_macos_launch_script_dirs(&base, u64::MAX);
        assert!(stale.is_dir());
        assert!(fresh.is_dir());

        // max_age = 0 → every existing dir is stale.
        sweep_stale_macos_launch_script_dirs(&base, 0);
        assert!(!stale.exists(), "stale dir must be swept");
        assert!(!fresh.exists(), "zero-age threshold sweeps all matching dirs");

        let _ = fs::remove_dir_all(&base);
    }

    /// HE-3: macOS launch script must not put the token-bearing prompt on pbcopy.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_script_clipboard_payload_has_no_token() {
        let base = std::env::temp_dir().join(format!(
            "devboule-he3-mac-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let root = base.join("root");
        fs::create_dir_all(&root).unwrap();
        let projects = base.join("projects");
        fs::create_dir_all(&projects).unwrap();

        let token_prompt = "launch-token-HE3-should-not-reach-clipboard";
        let result = build_macos_agent_script(
            "coder-he3",
            &root,
            "deepseek",
            "",
            Some("deepseek chat"),
            token_prompt,
            &root,
            &projects,
            None,
            &secret_env_fixture(),
            None,
            true,
            &[],
        );
        let (prompt_file, script) = result.expect("custom script builds without MCP");
        assert!(
            !script.contains("pbcopy"),
            "HE-3: script must not pbcopy the token-bearing prompt: {script}"
        );
        assert!(
            !script.contains(token_prompt),
            "token must not be embedded in the script body"
        );
        assert!(
            script.contains("export ASPIS_AGENT_PROMPT_FILE="),
            "token delivery is the 0600 prompt file only"
        );
        // HE-4: custom + external must not export vault secrets into the script.
        assert!(
            !script.contains("export DEVBOULE_MCP_LAUNCH_TOKEN="),
            "HE-4: custom path must not export launch token: {script}"
        );
        assert!(
            !script.contains("tok-secret-launch-he4-test"),
            "HE-4: secret value must not appear in custom launch script"
        );
        assert!(
            !script.contains("exa-secret-he4-test"),
            "HE-4: Exa key must not appear in custom launch script"
        );
        remove_restricted_temp_file(&prompt_file);
        let _ = fs::remove_dir_all(&base);
    }

    /// HE-3: Windows launch script must not Set-Clipboard the token-bearing prompt.
    #[cfg(windows)]
    #[test]
    fn windows_script_clipboard_payload_has_no_token() {
        let root = PathBuf::from("C:\\Devboule");
        let projects = root.join("projects");
        let token_prompt = "launch-token-HE3-should-not-reach-clipboard";
        let (prompt_file, script) = build_windows_agent_script(
            "coder-he3",
            &root,
            "deepseek",
            "",
            Some("deepseek chat"),
            token_prompt,
            &root,
            &projects,
            None,
            None,
            &[],
            false,
        )
        .expect("script builds");
        assert!(
            !script.contains("Set-Clipboard"),
            "HE-3: script must not put the prompt on the clipboard: {script}"
        );
        assert!(
            !script.contains(token_prompt),
            "token must not be embedded in the script body"
        );
        assert!(
            script.contains("$env:ASPIS_AGENT_PROMPT_FILE = "),
            "token delivery is the 0600 prompt file only"
        );
        remove_restricted_temp_file(&prompt_file);
    }

    /// M-1: Windows bare/other client delivers the prompt via the 0600 file + env
    /// (macOS parity). HE-3 removed clipboard delivery; bare must still have a channel.
    #[cfg(windows)]
    #[test]
    fn windows_bare_prompt_delivered_via_env_file_not_clipboard() {
        let root = PathBuf::from("C:\\Devboule");
        let projects = root.join("projects");
        let token_prompt = "launch-token-bare-windows-m1";
        let (prompt_file, script) = build_windows_agent_script(
            "coder-bare-m1",
            &root,
            "other",
            "", // empty executable = bare/other
            None,
            token_prompt,
            &root,
            &projects,
            None,
            None, // no orchestrator
            &[],
            false,
        )
        .expect("bare script builds");
        assert!(
            !script.contains("Set-Clipboard"),
            "HE-3: bare path must not put the prompt on the clipboard: {script}"
        );
        assert!(
            !script.contains(token_prompt),
            "token must not be embedded in the script body"
        );
        assert!(
            script.contains("$env:ASPIS_AGENT_PROMPT_FILE = "),
            "bare path must export ASPIS_AGENT_PROMPT_FILE like macOS: {script}"
        );
        assert!(
            !script.contains("Remove-Item -LiteralPath $promptFile"),
            "bare path must keep the 0600 prompt file (not delete after load): {script}"
        );
        assert!(
            script.contains(
                "Write-Host 'Devboule agent prompt at' $env:ASPIS_AGENT_PROMPT_FILE '(not on clipboard)'"
            ),
            "bare hint must point at the env file path: {script}"
        );
        assert!(
            prompt_file.is_file(),
            "builder must leave the restricted prompt file on disk for bare"
        );
        remove_restricted_temp_file(&prompt_file);
    }

    /// M-3: orchestrator launch hint must not claim STDIN delivery.
    #[cfg(windows)]
    #[test]
    fn windows_orchestrator_hint_is_autonomous_not_stdin() {
        let root = PathBuf::from("C:\\Devboule");
        let projects = root.join("projects");
        let orch = OrchestratorLaunchConfig {
            binary: PathBuf::from("C:\\Devboule\\devboule-coder.exe"),
            omlx_base_url: String::new(),
            omlx_model: String::new(),
            context_window: 8192,
            cloud_base_url: String::new(),
            cloud_model: String::new(),
            mcp_python: "python".into(),
            mcp_root: root.clone(),
            mcp_projects_dir: projects.clone(),
            agent_id: "orch-m3".into(),
            project_root: root.clone(),
            app_bin: String::new(),
            activity_file: String::new(),
            steer_file: String::new(),
            project_id: String::new(),
            plan_first: String::new(),
            user_mcp_servers_json: String::new(),
            lang_skill: String::new(),
            project_context: String::new(),
            initial_goal: String::new(),
            auto_create: String::new(),
        };
        let (prompt_file, script) = build_windows_agent_script(
            "orch-m3",
            &root,
            "devboule",
            "",
            None,
            "orchestrator-prompt-unused",
            &root,
            &projects,
            None,
            Some(&orch),
            &[],
            false,
        )
        .expect("orchestrator script builds");
        assert!(
            script.contains(
                "Write-Host 'Devboule orchestrator is autonomous (no agent prompt file; not on clipboard).'"
            ),
            "orchestrator hint must be accurate: {script}"
        );
        assert!(
            !script.contains("delivered via STDIN"),
            "orchestrator must not claim STDIN delivery: {script}"
        );
        assert!(
            !script.contains("$env:ASPIS_AGENT_PROMPT_FILE = "),
            "orchestrator must not keep the prompt file: {script}"
        );
        remove_restricted_temp_file(&prompt_file);
    }

    /// H-1: OAuth-present hint must be computed on the unfiltered provider env.
    /// HE-4 still strips secrets from the launch injection slice for custom.
    #[test]
    fn oauth_present_hint_uses_unfiltered_provider_env_for_custom() {
        let mut envs = secret_env_fixture();
        envs.push(AgentLaunchEnv {
            name: "CLAUDE_CODE_OAUTH_TOKEN".into(),
            value: "oauth-tok-h1-must-not-leak".into(),
        });
        // Launch injection stays empty under custom (HE-4).
        assert!(
            provider_env_for_launch(Some("custom-cli --flag"), &envs).is_empty(),
            "custom must still isolate provider secrets from the launch env"
        );
        // Hint / CLAUDE isolation must see the real vault presence on the unfiltered slice.
        assert!(
            vault_oauth_present_in(&envs),
            "unfiltered provider_env must report OAuth present"
        );
        assert!(
            !vault_oauth_present_in(provider_env_for_launch(Some("custom-cli --flag"), &envs)),
            "filtered slice is empty — double-filtering would collapse the hint"
        );
    }

    /// H-1 integrated (macOS): custom + claude + oauth token → CLAUDE_CONFIG_DIR path
    /// treats vault as present (drops stale .credentials.json) while HE-4 still
    /// omits secrets from the script export block.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_custom_claude_oauth_hint_from_unfiltered_env() {
        let base = std::env::temp_dir().join(format!(
            "devboule-h1-oauth-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let root = base.join("root");
        fs::create_dir_all(&root).unwrap();
        let projects = base.join("projects");
        fs::create_dir_all(&projects).unwrap();

        let agent_id = "agent-h1-oauth";
        let config_dir =
            crate::backend::cloud_claude_config::claude_product_config_dir(&projects, agent_id);
        fs::create_dir_all(&config_dir).unwrap();
        let creds = config_dir.join(".credentials.json");
        fs::write(&creds, b"stale-credentials").unwrap();

        let mut envs = secret_env_fixture();
        envs.push(AgentLaunchEnv {
            name: "CLAUDE_CODE_OAUTH_TOKEN".into(),
            value: "oauth-tok-h1-must-not-leak".into(),
        });

        let result = build_macos_agent_script(
            agent_id,
            &root,
            "claude",
            "",
            Some("claude --custom-wrapper"),
            "custom-claude-prompt",
            &root,
            &projects,
            None,
            &envs, // unfiltered — matches post-H-1 spawn path
            None,
            true,
            &[],
        );
        let (prompt_file, script) = result.expect("custom claude script builds");
        assert!(
            !creds.exists(),
            "vault OAuth present on unfiltered env must drop stale .credentials.json"
        );
        assert!(
            !script.contains("export CLAUDE_CODE_OAUTH_TOKEN="),
            "HE-4: custom must not export OAuth token: {script}"
        );
        assert!(
            !script.contains("oauth-tok-h1-must-not-leak"),
            "HE-4: OAuth secret value must not appear in custom launch script"
        );
        assert!(
            !script.contains("export DEVBOULE_MCP_LAUNCH_TOKEN="),
            "HE-4: custom must not export launch token"
        );
        remove_restricted_temp_file(&prompt_file);
        let _ = fs::remove_dir_all(&base);
    }

    /// H-2: osascript failure cleanup must remove both the secrets-bearing script
    /// file and the token-bearing 0600 prompt file (and their restricted parents).
    #[cfg(target_os = "macos")]
    #[test]
    fn prompt_and_script_removed_on_simulated_osascript_failure() {
        let prompt_file =
            write_restricted_prompt_file("launch-token-h2-should-not-linger").expect("prompt");
        let script_file =
            write_restricted_script_file("export SECRET='should-not-linger'\n").expect("script");
        assert!(prompt_file.is_file());
        assert!(script_file.is_file());
        let prompt_parent = prompt_file.parent().map(|p| p.to_path_buf());
        let script_parent = script_file.parent().map(|p| p.to_path_buf());
        // Same cleanup the osascript Err / non-success wait path runs (H-2).
        remove_restricted_temp_file(&script_file);
        remove_restricted_temp_file(&prompt_file);
        assert!(
            !script_file.exists(),
            "secrets-bearing script must be gone after failure cleanup"
        );
        assert!(
            !prompt_file.exists(),
            "token-bearing prompt file must be gone after failure cleanup"
        );
        if let Some(parent) = script_parent {
            assert!(
                !parent.exists(),
                "script restricted parent dir must be removed: {parent:?}"
            );
        }
        if let Some(parent) = prompt_parent {
            assert!(
                !parent.exists(),
                "prompt restricted parent dir must be removed: {parent:?}"
            );
        }
    }

    /// M-3 (macOS): orchestrator launch hint must not claim STDIN delivery.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_orchestrator_hint_is_autonomous_not_stdin() {
        let base = std::env::temp_dir().join(format!(
            "devboule-m3-orch-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let root = base.join("root");
        fs::create_dir_all(&root).unwrap();
        let projects = base.join("projects");
        fs::create_dir_all(&projects).unwrap();

        let orch = OrchestratorLaunchConfig {
            binary: base.join("devboule-coder"),
            omlx_base_url: String::new(),
            omlx_model: String::new(),
            context_window: 8192,
            cloud_base_url: String::new(),
            cloud_model: String::new(),
            mcp_python: "python".into(),
            mcp_root: root.clone(),
            mcp_projects_dir: projects.clone(),
            agent_id: "orch-m3".into(),
            project_root: root.clone(),
            app_bin: String::new(),
            activity_file: String::new(),
            steer_file: String::new(),
            project_id: String::new(),
            plan_first: String::new(),
            user_mcp_servers_json: String::new(),
            lang_skill: String::new(),
            project_context: String::new(),
            initial_goal: String::new(),
            auto_create: String::new(),
        };
        let (prompt_file, script) = build_macos_agent_script(
            "orch-m3",
            &root,
            "devboule",
            "",
            None,
            "orchestrator-prompt-unused",
            &root,
            &projects,
            None,
            &[],
            Some(&orch),
            true,
            &[],
        )
        .expect("orchestrator script builds");
        assert!(
            script.contains(
                "echo 'Devboule orchestrator is autonomous (no agent prompt file; not on clipboard).'"
            ),
            "orchestrator hint must be accurate: {script}"
        );
        assert!(
            !script.contains("delivered via STDIN"),
            "orchestrator must not claim STDIN delivery: {script}"
        );
        remove_restricted_temp_file(&prompt_file);
        let _ = fs::remove_dir_all(&base);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod f36_isolation_tests {
    use super::*;

    #[test]
    fn macos_claude_script_exports_claude_config_dir() {
        let base = std::env::temp_dir().join(format!(
            "devboule-f36-spawn-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let root = base.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let mgmt = base.join("mgmt");
        std::fs::create_dir_all(&mgmt).unwrap();
        let projects = base.join("projects");
        std::fs::create_dir_all(&projects).unwrap();

        // MCP config may fail if rust bin missing — still want CLAUDE_CONFIG_DIR
        // emission before the launch line. If MCP fails, the whole builder errs;
        // in that case fall back to asserting the pure dir helper used by the path.
        let result = build_macos_agent_script(
            "agent-f36-test",
            &root,
            "claude",
            "claude",
            None,
            "hello prompt",
            &mgmt,
            &projects,
            None,
            &[],
            None,
            false,
            &[],
        );
        match result {
            Ok((_pf, script)) => {
                assert!(
                    script.contains("export CLAUDE_CONFIG_DIR="),
                    "F36 isolation missing from macos claude script:\n{script}"
                );
                assert!(
                    script.contains("claude-agent-config"),
                    "config dir should be under app-owned claude-agent-config"
                );
                assert!(
                    !script.contains("export CLAUDE_CONFIG_DIR='$HOME/.claude'")
                        && !script.contains("export CLAUDE_CONFIG_DIR=\"$HOME/.claude\""),
                    "must not point at home .claude"
                );
            }
            Err(e) => {
                // MCP binary may be absent in CI; still prove the helper path used by F36.
                eprintln!("build_macos_agent_script err (MCP?): {e}");
                let dir = crate::backend::cloud_claude_config::claude_product_config_dir(
                    &projects,
                    "agent-f36-test",
                );
                assert!(dir.starts_with(&projects));
                assert!(dir.to_string_lossy().contains("claude-agent-config"));
            }
        }
        let _ = std::fs::remove_dir_all(&base);
    }
}
