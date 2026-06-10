#[cfg(target_os = "windows")]
use std::{
    panic::{catch_unwind, UnwindSafe},
    thread,
};

#[cfg(all(target_os = "windows", not(test)))]
use std::{
    env,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[cfg(all(target_os = "windows", not(test)))]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
use windows::{
    core::HSTRING,
    Security::Credentials::UI::{
        UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability,
    },
    Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
    },
};

#[cfg(target_os = "windows")]
const HELPER_ARG_AVAILABILITY: &str = "--aspis-windows-hello-availability";
#[cfg(target_os = "windows")]
const HELPER_ARG_VERIFY: &str = "--aspis-windows-hello-verify";
#[cfg(all(target_os = "windows", not(test)))]
const HELPER_TIMEOUT_AVAILABILITY: Duration = Duration::from_secs(12);
#[cfg(all(target_os = "windows", not(test)))]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(all(target_os = "windows", not(test)))]
pub fn hello_available() -> bool {
    match run_hello_helper_bool(
        HELPER_ARG_AVAILABILITY,
        None,
        "available",
        HELPER_TIMEOUT_AVAILABILITY,
    ) {
        Ok(available) => available,
        Err(e) => {
            eprintln!("{e}");
            false
        }
    }
}

#[cfg(all(target_os = "windows", test))]
pub fn hello_available() -> bool {
    match run_hello_call("availability check", check_hello_available) {
        Ok(available) => available,
        Err(e) => {
            eprintln!("{e}");
            false
        }
    }
}

#[cfg(target_os = "windows")]
fn check_hello_available() -> Result<bool, String> {
    let _winrt = WinRtGuard::initialize()?;
    match UserConsentVerifier::CheckAvailabilityAsync().and_then(|op| op.get()) {
        Ok(UserConsentVerifierAvailability::Available) => Ok(true),
        Ok(_) => Ok(false),
        Err(e) => Err(format!("Windows Hello availability check failed: {e}")),
    }
}

// --- macOS Touch ID / device-owner authentication ---------------------------
//
// UNVERIFIED: written without a Mac to compile or run it. The objc2 method names
// (`canEvaluatePolicy_error`, `evaluatePolicy_localizedReason_reply`) and the
// `LAPolicy` variant match the objc2-local-authentication 0.2 API; if a different
// crate version is resolved at build time, only these calls need adjusting. The
// Windows build is unaffected (this whole block is target-gated).
#[cfg(target_os = "macos")]
pub fn hello_available() -> bool {
    use objc2_local_authentication::{LAContext, LAPolicy};
    let context = unsafe { LAContext::new() };
    // DeviceOwnerAuthentication allows Touch ID with a password fallback, so a Mac
    // without a fingerprint sensor still authenticates via the login password.
    unsafe { context.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthentication, None) }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn hello_available() -> bool {
    false
}

#[cfg(all(target_os = "windows", not(test)))]
pub fn verify_user(message: &str) -> Result<bool, String> {
    let message = message.to_string();
    run_hello_thread("verification", move || verify_user_inner(&message))
}

#[cfg(all(target_os = "windows", test))]
pub fn verify_user(message: &str) -> Result<bool, String> {
    let message = message.to_string();
    run_hello_thread("verification", move || verify_user_inner(&message))
}

#[cfg(target_os = "windows")]
pub(crate) fn run_helper_from_args<I>(args: I) -> Option<i32>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let _exe = args.next();
    let Some(mode) = args.next() else {
        return None;
    };

    match mode.as_str() {
        HELPER_ARG_AVAILABILITY => Some(write_helper_bool(
            run_hello_call("helper availability check", check_hello_available),
            "available",
            "unavailable",
        )),
        HELPER_ARG_VERIFY => {
            let Some(message) = args.next() else {
                eprintln!("Windows Hello helper missing verification message.");
                return Some(2);
            };
            Some(write_helper_bool(
                run_hello_call("helper verification", || verify_user_inner(&message)),
                "verified",
                "not_verified",
            ))
        }
        _ => None,
    }
}

#[cfg(target_os = "windows")]
fn verify_user_inner(message: &str) -> Result<bool, String> {
    let _winrt = WinRtGuard::initialize()?;
    let availability = UserConsentVerifier::CheckAvailabilityAsync()
        .and_then(|op| op.get())
        .map_err(|e| format!("Windows Hello availability check failed: {e}"))?;

    if availability != UserConsentVerifierAvailability::Available {
        return Err("Windows Hello is unavailable for the current Windows user.".into());
    }

    let result = UserConsentVerifier::RequestVerificationAsync(&HSTRING::from(message))
        .and_then(|op| op.get())
        .map_err(|e| format!("Windows Hello verification failed: {e}"))?;

    if result == UserConsentVerificationResult::Verified {
        return Ok(true);
    }
    if result == UserConsentVerificationResult::Canceled {
        return Ok(false);
    }

    Err(format!(
        "Windows Hello verification failed: {}",
        verification_result_label(result)
    ))
}

#[cfg(all(target_os = "windows", not(test)))]
fn run_hello_helper_bool(
    mode_arg: &str,
    message: Option<&str>,
    true_token: &str,
    timeout: Duration,
) -> Result<bool, String> {
    let exe = env::current_exe()
        .map_err(|e| format!("Windows Hello helper executable lookup failed: {e}"))?;
    let mut command = Command::new(exe);
    command
        .arg(mode_arg)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    if let Some(message) = message {
        command.arg(message);
    }

    let mut child = command
        .spawn()
        .map_err(|e| format!("Windows Hello helper failed to start: {e}"))?;
    let started_at = Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|e| format!("Windows Hello helper wait failed: {e}"))?
            .is_some()
        {
            break;
        }
        if started_at.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "Windows Hello helper timed out after {} seconds.",
                timeout.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Windows Hello helper output failed: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(helper_failure_message(&stdout, &stderr));
    }
    parse_helper_bool_output(&stdout, true_token)
}

#[cfg(target_os = "windows")]
fn write_helper_bool(result: Result<bool, String>, true_token: &str, false_token: &str) -> i32 {
    match result {
        Ok(true) => {
            println!("{true_token}");
            0
        }
        Ok(false) => {
            println!("{false_token}");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            2
        }
    }
}

#[cfg(target_os = "windows")]
fn parse_helper_bool_output(stdout: &str, true_token: &str) -> Result<bool, String> {
    let response = stdout.trim();
    if response == true_token {
        return Ok(true);
    }

    let false_token = match true_token {
        "available" => "unavailable",
        "verified" => "not_verified",
        _ => "",
    };
    if response == false_token {
        return Ok(false);
    }

    Err(format!(
        "Unexpected Windows Hello helper response: {}",
        helper_first_line(response)
    ))
}

#[cfg(target_os = "windows")]
fn helper_failure_message(stdout: &str, stderr: &str) -> String {
    let message = helper_first_line(stderr);
    if !message.is_empty() {
        return message;
    }
    let message = helper_first_line(stdout);
    if !message.is_empty() {
        return message;
    }
    "Windows Hello helper failed without diagnostic output.".into()
}

#[cfg(target_os = "windows")]
fn helper_first_line(value: &str) -> String {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .chars()
        .take(240)
        .collect()
}

#[cfg(target_os = "windows")]
fn run_hello_thread<T, F>(label: &'static str, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + UnwindSafe + 'static,
{
    thread::Builder::new()
        .name(format!("aspis-windows-hello-{label}"))
        .spawn(move || run_hello_call(label, f))
        .map_err(|e| format!("Windows Hello {label} thread failed to start: {e}"))?
        .join()
        .map_err(|_| format!("Windows Hello {label} thread crashed."))?
}

#[cfg(target_os = "windows")]
fn run_hello_call<T, F>(label: &str, f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + UnwindSafe,
{
    catch_unwind(f).map_err(|_| format!("Windows Hello {label} crashed before returning."))?
}

#[cfg(target_os = "windows")]
fn verification_result_label(result: UserConsentVerificationResult) -> &'static str {
    if result == UserConsentVerificationResult::DeviceNotPresent {
        "device not present"
    } else if result == UserConsentVerificationResult::NotConfiguredForUser {
        "not configured for this Windows user"
    } else if result == UserConsentVerificationResult::DisabledByPolicy {
        "disabled by Windows policy"
    } else if result == UserConsentVerificationResult::DeviceBusy {
        "camera or biometric device busy"
    } else if result == UserConsentVerificationResult::RetriesExhausted {
        "biometric retries exhausted"
    } else {
        "unknown Windows Hello result"
    }
}

#[cfg(target_os = "windows")]
struct WinRtGuard {
    initialized: bool,
}

#[cfg(target_os = "windows")]
impl WinRtGuard {
    fn initialize() -> Result<Self, String> {
        match unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            Ok(()) => Ok(Self { initialized: true }),
            Err(e) if e.code() == RPC_E_CHANGED_MODE => Ok(Self { initialized: false }),
            Err(e) => Err(format!("Windows Hello WinRT initialization failed: {e}")),
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for WinRtGuard {
    fn drop(&mut self) {
        if self.initialized {
            unsafe {
                RoUninitialize();
            }
        }
    }
}

/// macOS device-owner authentication (Touch ID, with password fallback).
///
/// Runs the policy evaluation on a dedicated thread and blocks on a channel for
/// the async reply, mirroring the Windows thread model so the caller never
/// deadlocks the UI. UNVERIFIED on hardware — see the note above `hello_available`.
#[cfg(target_os = "macos")]
pub fn verify_user(message: &str) -> Result<bool, String> {
    use std::sync::mpsc;

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};

    let message = message.to_string();
    let handle = std::thread::Builder::new()
        .name("aspis-macos-localauth".into())
        .spawn(move || -> Result<bool, String> {
            let context = unsafe { LAContext::new() };
            if !unsafe {
                context.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthentication, None)
            } {
                return Err("Touch ID / device authentication is unavailable.".into());
            }

            let reason = NSString::from_str(&message);
            let (tx, rx) = mpsc::channel::<bool>();
            let reply = RcBlock::new(move |success: Bool, _error: *mut NSError| {
                let _ = tx.send(success.as_bool());
            });
            unsafe {
                context.evaluatePolicy_localizedReason_reply(
                    LAPolicy::DeviceOwnerAuthentication,
                    &reason,
                    &reply,
                );
            }
            // Fail CLOSED on timeout rather than blocking the unlock command
            // forever: evaluatePolicy calls back on an internal queue, so if the
            // reply never arrives we must not hang. The user can simply retry.
            match rx.recv_timeout(std::time::Duration::from_secs(60)) {
                Ok(success) => Ok(success),
                Err(_) => Ok(false),
            }
        })
        .map_err(|e| format!("macOS authentication thread failed to start: {e}"))?;

    handle
        .join()
        .map_err(|_| "macOS authentication thread crashed.".to_string())?
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn verify_user(_message: &str) -> Result<bool, String> {
    Err("Biometric unlock is only available on Windows and macOS.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_call_guard_converts_panics_to_errors() {
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let err = run_hello_call("unit test", || -> Result<bool, String> {
            panic!("simulated Windows Hello failure");
        })
        .unwrap_err();
        std::panic::set_hook(previous_hook);

        assert!(err.contains("Windows Hello unit test crashed"));
    }

    #[test]
    fn hello_thread_returns_inner_errors_without_panicking() {
        let err = run_hello_thread("unit", || Err::<bool, _>("inner failure".into())).unwrap_err();

        assert_eq!(err, "inner failure");
    }

    #[test]
    fn helper_bool_output_parser_accepts_success_tokens_only() {
        assert!(parse_helper_bool_output("verified\r\n", "verified").unwrap());
        assert!(!parse_helper_bool_output("not_verified\n", "verified").unwrap());

        let err = parse_helper_bool_output("unexpected", "verified").unwrap_err();
        assert!(err.contains("Unexpected Windows Hello helper response"));
    }

    #[test]
    fn helper_failure_message_prefers_stderr_without_leaking_raw_noise() {
        let message = helper_failure_message("ignored stdout", " Windows Hello failed \r\n ");

        assert_eq!(message, "Windows Hello failed");
    }

    #[test]
    fn verification_result_labels_biometric_device_busy() {
        assert_eq!(
            verification_result_label(UserConsentVerificationResult::DeviceBusy),
            "camera or biometric device busy"
        );
        assert_eq!(
            verification_result_label(UserConsentVerificationResult::RetriesExhausted),
            "biometric retries exhausted"
        );
    }
}
