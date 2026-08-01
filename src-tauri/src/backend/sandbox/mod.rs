pub mod seatbelt;
pub mod windows;

use std::path::{Path, PathBuf};

/// Network policy for a sandboxed child process.
/// Controls outbound connectivity to prevent data exfiltration or unauthorized API calls.
/// - `None`: deny all network (default — the tightest, used by Censor linters and most build/test commands).
/// - `Loopback`: deny all EXCEPT loopback (127.0.0.1 / ::1 / localhost) — used when the child must reach a
///   local service (e.g. the one-shot mini calling an oMLX server on loopback).
/// - `Enabled`: allow outbound network — used only for roles that legitimately need egress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetPolicy {
    None,
    Loopback,
    Enabled,
}

/// OS resource limits applied to the sandboxed child (rlimit on macOS/Linux, Job Object on Windows).
/// Enforced at the process level to prevent runaway tasks from starving the host or other agents.
/// `addr_space_bytes == None` means "do not cap address space".
///
/// **Platform-specific semantics** (reviewer N2, post-C1):
/// - `cpu_secs`: enforced via `RLIMIT_CPU` on unix; Windows has no CPU-time limit and **silently ignores** this field (relies on AgentScope/orchestrator timeouts instead).
/// - `addr_space_bytes`: on unix maps to `RLIMIT_AS` (virtual address space); on Windows maps to `JOBOBJECT_EXTENDED_LIMIT_INFORMATION.ProcessMemoryLimit` which is "private commit charge" (committed, non-shareable virtual memory), NOT total virtual address space. Both are sufficient for a runaway-task guard (an infinite allocator loop hits either), but `top`/`tasklist` will report different numbers.
/// - `max_procs`: enforced via `RLIMIT_NPROC` on unix-macOS (intentionally NOT set on macOS — see `apply_rlimits`); on Windows **silently ignored** today (would require `JOB_OBJECT_LIMIT_ACTIVE_PROCESS`). Per-process fork-bounding belongs to the Windows Job Object's process-count, which we may set in C4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceLimits {
    pub cpu_secs: u64,
    pub addr_space_bytes: Option<u64>,
    pub max_procs: u64,
}

impl Default for ResourceLimits {
    /// Provides safe, conservative defaults that prevent indefinite host resource consumption
    /// while remaining permissive enough for typical linting, compilation, and test workloads.
    fn default() -> Self {
        Self {
            cpu_secs: 600,
            addr_space_bytes: None,
            max_procs: 256,
        }
    }
}

/// The common sandbox contract: what a wrapped child may read/write/reach.
/// Serves as the single source of truth for permission boundaries before spawning a child process.
/// - `readonly_root`: the project root — readable, NEVER writable. Ensures source integrity.
/// - `writable_paths`: the ONLY paths the child may write to (scratch, tmp, granted working-set dirs).
/// - `net`: network policy.
/// - `rlimits`: resource limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPolicy {
    pub readonly_root: PathBuf,
    /// Additional read-only paths granted to the child (beyond `readonly_root`).
    /// Used by the PTY path to expose per-launch support files (prompt temp dir,
    /// session gitconfig) that live outside the project root — without these,
    /// the deny-by-default AppContainer cannot read them (C6 reviewer finding).
    pub readonly_paths: Vec<PathBuf>,
    pub writable_paths: Vec<PathBuf>,
    pub net: NetPolicy,
    pub rlimits: ResourceLimits,
}

impl Default for SandboxPolicy {
    /// Defaults to a deny-all policy on an empty path. This ensures that any uninitialized
    /// policy safely rejects execution rather than accidentally granting host access.
    fn default() -> Self {
        Self::deny(PathBuf::new())
    }
}

impl SandboxPolicy {
    /// Constructs the DEFAULT-DENY policy: empty `writable_paths`, `net: NetPolicy::None`,
    /// and conservative resource limits. This is the baseline for all sandboxed commands.
    pub fn deny(readonly_root: PathBuf) -> Self {
        Self {
            readonly_root,
            readonly_paths: Vec::new(),
            writable_paths: Vec::new(),
            net: NetPolicy::None,
            rlimits: ResourceLimits::default(),
        }
    }

    /// Adds a path to the read-only whitelist (beyond `readonly_root`). Used for
    /// per-launch support files (prompt file, session gitconfig) the child must
    /// read but that live outside the project root.
    pub fn readonly(mut self, path: PathBuf) -> Self {
        self.readonly_paths.push(path);
        self
    }

    /// Adds a path to the whitelist of directories the child may write to.
    /// Used for scratch spaces, temporary outputs, or explicitly granted working sets.
    pub fn writable(mut self, path: PathBuf) -> Self {
        self.writable_paths.push(path);
        self
    }

    /// Overrides the network policy for this sandbox instance.
    pub fn net(mut self, net: NetPolicy) -> Self {
        self.net = net;
        self
    }

    /// Overrides the resource limits for this sandbox instance.
    pub fn rlimits(mut self, rlimits: ResourceLimits) -> Self {
        self.rlimits = rlimits;
        self
    }
}

/// A command rewritten to run under the sandbox: `program` + `args` ready to hand to a
/// `std::process::Command` / `CommandBuilder`. On macOS this is `/usr/bin/sandbox-exec`
/// wrapping the real program; on unsupported OSes it is the original command unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxedCommand {
    pub program: String,
    pub args: Vec<String>,
}

/// Build the `sandbox-exec -p <profile> -- <program> <args...>` argv. Pure + platform-agnostic
/// (testable on the Windows/Linux dev host). `/usr/bin/sandbox-exec` is hardcoded to defend
/// against a PATH-injected sandbox-exec.
fn macos_sandbox_exec_argv(profile: &str, program: &str, args: &[String]) -> SandboxedCommand {
    let mut a = vec![
        "-p".to_string(),
        profile.to_string(),
        "--".to_string(),
        program.to_string(),
    ];
    a.extend(args.iter().cloned());
    SandboxedCommand {
        program: "/usr/bin/sandbox-exec".to_string(),
        args: a,
    }
}

/// Wrap `program`+`args` so they run confined to `policy`. On macOS: builds the Seatbelt
/// profile from `policy` and prefixes `/usr/bin/sandbox-exec -p <profile> --`. On other OSes:
/// returns the command UNCHANGED (Windows lands in a later phase; Linux is a stub).
/// `_cwd` is reserved for the Windows backend (workspace ACL grant); unused on macOS.
pub fn wrap(policy: &SandboxPolicy, program: &str, args: &[String], _cwd: &Path) -> SandboxedCommand {
    #[cfg(target_os = "macos")]
    {
        let profile = seatbelt::build_profile(policy);
        // rlimits are applied separately by `apply_rlimits()` (the spawner calls it on the built
        // Command) — Seatbelt has no native rlimit, and they must go in a pre_exec on the Command.
        macos_sandbox_exec_argv(&profile, program, args)
    }
    #[cfg(target_os = "windows")]
    {
        crate::backend::sandbox::windows::wrap_policy(policy, program, args, _cwd)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // TODO(linux: landlock stub).
        let _ = policy;
        // review F3: log ONCE (Censor/agentic call wrap dozens of times → stderr spam) and WITHOUT
        // the program name (a user path can be sensitive).
        use std::sync::Once;
        static WARN_ONCE: Once = Once::new();
        WARN_ONCE.call_once(|| {
            eprintln!(
                "[sandbox] wrap: NO OS confinement on this platform — children run UNRESTRICTED \
                 (Linux sandbox not yet implemented). Auto-mode must refuse unattended use here."
            );
        });
        SandboxedCommand {
            program: program.to_string(),
            args: args.to_vec(),
        }
    }
}

/// Enforce `limits` on a command via a `pre_exec` `setrlimit` (unix). Seatbelt has NO native rlimit,
/// so CPU-seconds / address-space / max-procs are set here on the spawned process (sandbox-exec),
/// inherited by its child. Best-effort: a denied limit never aborts the spawn. No-op on non-unix.
/// (RLIMIT_AS is a soft no-op on macOS but harmless to set.) The spawner calls this after building
/// the Command from `wrap`'s `SandboxedCommand`.
#[cfg(unix)]
pub fn apply_rlimits(cmd: &mut std::process::Command, limits: &ResourceLimits) {
    use std::os::unix::process::CommandExt;
    let cpu = limits.cpu_secs;
    let nproc = limits.max_procs;
    let addr = limits.addr_space_bytes;
    // SAFETY: pre_exec runs in the forked child before exec; we only call the async-signal-safe
    // setrlimit syscall (no allocation, no locks).
    // NOTE (review F4): RLIMIT_NPROC is intentionally NOT set — on macOS it caps the whole UID
    // (the Tauri app + every concurrent agent), not just this child, so a fork-bomb here would also
    // starve the app. Per-process fork bounding belongs to the Windows Job Object (phase 3); here
    // CPU + address-space + the timeout/process-group kill are the runaway guard.
    let _ = nproc;
    unsafe {
        cmd.pre_exec(move || {
            set_rlimit(libc::RLIMIT_CPU as libc::c_int, cpu);
            if let Some(bytes) = addr {
                set_rlimit(libc::RLIMIT_AS as libc::c_int, bytes);
            }
            Ok(())
        });
    }
}

#[cfg(unix)]
fn set_rlimit(resource: libc::c_int, value: u64) {
    let lim = libc::rlimit {
        rlim_cur: value as libc::rlim_t,
        rlim_max: value as libc::rlim_t,
    };
    // best-effort; ignore failure so a rejected limit can't abort the spawn under set -e semantics
    unsafe {
        libc::setrlimit(resource as _, &lim);
    }
}

/// Windows arm: delegate to the Job Object backend (C1 — kill-on-close + memory limit).
#[cfg(target_os = "windows")]
pub fn apply_rlimits(cmd: &mut std::process::Command, limits: &ResourceLimits) {
    crate::backend::sandbox::windows::apply_rlimits(cmd, limits)
}

/// No-op elsewhere (Linux/other — landlock stub not yet implemented).
#[cfg(not(any(unix, target_os = "windows")))]
pub fn apply_rlimits(_cmd: &mut std::process::Command, _limits: &ResourceLimits) {}

/// Returns `true` if this platform actually applies OS-level sandbox confinement in [`wrap`].
///
/// This is the SINGLE source of truth for "is the sandbox real here". Callers gate autonomous
/// (Unattended) behaviour on it so that no agent runs unsupervised code without OS isolation
/// (see `broker::effective_sandbox_mode`). Zero-I/O, compile-time only.
///
/// Windows has been go-live since 2026-07-31 (C6): the AppContainer broker
/// (C5, per-spawn profiles + SECURITY_CAPABILITIES + Job Object + net
/// capability) covers every app-hosted spawn path; the external conhost path
/// rejects Unattended (projects.rs `unattended_external_is_rejected`). See
/// the `windows` arm below.
pub fn is_enforced() -> bool {
    #[cfg(target_os = "macos")]
    {
        true
    }
    #[cfg(target_os = "windows")]
    {
        // TRUE since 2026-07-31 (C6): the AppContainer broker (C5) covers every
        // APP-HOSTED spawn path — agentic runs, sidecars, cloud duplex, censor,
        // interactive agent PTY and one-shot mini (ConPTY via
        // PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE + SECURITY_CAPABILITIES + Job
        // Object; verified TokenIsAppContainer=1 + prompt-read roundtrip on a
        // non-elevated host). The legacy EXTERNAL conhost terminal is excluded
        // by design (attended path, parity with macOS Terminal.app). This
        // unlocks Unattended autonomy for app-hosted agents on Windows.
        true
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Linux/other: `wrap` is passthrough (landlock stub) — no OS confinement yet.
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// macOS: rlimits actually take effect — a child sees the RLIMIT_CPU we set (via `ulimit -t`).
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_apply_rlimits_sets_cpu_limit() {
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.arg("-c").arg("ulimit -t");
        let limits = ResourceLimits { cpu_secs: 7, addr_space_bytes: None, max_procs: 64 };
        apply_rlimits(&mut cmd, &limits);
        let out = cmd.output().expect("spawn sh");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(stdout.trim(), "7", "child should see the RLIMIT_CPU we set");
    }

    #[test]
    fn macos_argv_wraps_with_sandbox_exec() {
        let cmd = macos_sandbox_exec_argv("(version 1)(deny default)", "/bin/echo", &["hi".to_string()]);
        assert_eq!(cmd.program, "/usr/bin/sandbox-exec");
        assert_eq!(cmd.args[0], "-p");
        assert_eq!(cmd.args[1], "(version 1)(deny default)");
        assert_eq!(cmd.args[2], "--");
        assert_eq!(cmd.args[3], "/bin/echo");
        assert_eq!(cmd.args[4], "hi");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn wrap_is_passthrough_off_macos() {
        let policy = SandboxPolicy::deny("/proj".into());
        let cmd = wrap(&policy, "/bin/echo", &["hi".to_string()], std::path::Path::new("/proj"));
        assert_eq!(cmd.program, "/bin/echo");
        assert_eq!(cmd.args, vec!["hi".to_string()]);
    }

    #[test]
    fn default_policy_is_deny() {
        let policy = SandboxPolicy::deny("/proj".into());
        assert!(policy.writable_paths.is_empty());
        assert_eq!(policy.net, NetPolicy::None);
    }

    #[test]
    fn builder_adds_writable_and_sets_net() {
        let policy = SandboxPolicy::deny("/proj".into())
            .writable("/proj/scratch".into())
            .net(NetPolicy::Loopback);

        assert_eq!(policy.writable_paths, vec![PathBuf::from("/proj/scratch")]);
        assert_eq!(policy.net, NetPolicy::Loopback);
    }

    #[test]
    fn builder_adds_readonly_paths() {
        let policy = SandboxPolicy::deny("/proj".into())
            .readonly("/tmp/prompt.d".into())
            .readonly("/tmp/gitconfig".into());

        assert_eq!(
            policy.readonly_paths,
            vec![PathBuf::from("/tmp/prompt.d"), PathBuf::from("/tmp/gitconfig")]
        );
        // readonly_paths must NOT grant write access (deny-by-default otherwise).
        assert!(policy.writable_paths.is_empty());
    }

    #[test]
    fn default_rlimits_are_conservative() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.cpu_secs, 600);
        assert_eq!(limits.addr_space_bytes, None);
        assert_eq!(limits.max_procs, 256);
    }

    /// macOS DOES apply real OS confinement (Seatbelt) in `wrap`, so `is_enforced()` is true here.
    /// This is the predicate that gates Unattended autonomy (`broker::effective_sandbox_mode`).
    #[cfg(target_os = "macos")]
    #[test]
    fn is_enforced_true_on_macos() {
        assert!(is_enforced());
    }

    /// Windows: is_enforced() is true since C6 — every unattended spawn path
    /// (agentic runs, sidecars, cloud duplex, censor, interactive agent PTY,
    /// one-shot mini) routes through the AppContainer broker. Verified on a
    /// non-elevated host (TokenIsAppContainer=1, PTY echo roundtrip).
    #[cfg(target_os = "windows")]
    #[test]
    fn is_enforced_true_on_windows() {
        assert!(is_enforced());
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn is_enforced_false_on_linux() {
        assert!(!is_enforced());
    }
}
