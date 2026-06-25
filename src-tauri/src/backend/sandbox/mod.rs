pub mod seatbelt;

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
            writable_paths: Vec::new(),
            net: NetPolicy::None,
            rlimits: ResourceLimits::default(),
        }
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
        // TODO(rlimits): apply policy.rlimits via nix::setrlimit pre_exec (next slice).
        macos_sandbox_exec_argv(&profile, program, args)
    }
    #[cfg(not(target_os = "macos"))]
    {
        // TODO(windows: phase 3 — Restricted Token + WFP + Job Object) / (linux: landlock stub).
        let _ = policy;
        eprintln!(
            "[sandbox] wrap: NO OS confinement on this platform — child '{program}' runs UNRESTRICTED \
             (Windows/Linux sandbox not yet implemented). Auto-mode must refuse unattended use here."
        );
        SandboxedCommand {
            program: program.to_string(),
            args: args.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn default_rlimits_are_conservative() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.cpu_secs, 600);
        assert_eq!(limits.addr_space_bytes, None);
        assert_eq!(limits.max_procs, 256);
    }
}
