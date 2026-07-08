//! Terminal lifecycle: raw mode + alternate screen, restored on ALL exit paths.
//!
//! [`TerminalGuard`] is an RAII handle: constructing it enters raw mode and the
//! alternate screen; dropping it (normal return, `?` early-return, OR unwinding
//! panic) leaves the alternate screen and disables raw mode. We additionally
//! chain the panic hook so the terminal is restored BEFORE the panic message is
//! printed — otherwise the backtrace would render into the raw alternate screen
//! and the user would be left with a wrecked terminal.
//!
//! Restoration is IDEMPOTENT: `Drop` and the panic hook both call [`restore`],
//! but a [`RESTORED`] flag makes the second call a no-op, so a panic does not
//! emit the leave/disable escape sequences twice. The panic hook is installed at
//! most once (via [`std::sync::Once`]) so repeated `enter()` calls never stack
//! hooks.

use std::io::{self, Stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;

use ratatui::crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::CrosstermBackend;
use ratatui::Terminal;

/// The concrete terminal type used throughout the app.
pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Tracks whether the terminal is currently in the entered (raw + alt-screen)
/// state. `restore()` only emits the leave/disable sequences when this is
/// `true`, then flips it `false`, making restoration idempotent across `Drop`
/// and the panic hook. `enter()` sets it `true` on success.
static RESTORED: AtomicBool = AtomicBool::new(true);

/// Ensures the panic hook is chained exactly once, no matter how many times
/// `enter()` runs, so hooks never stack.
static PANIC_HOOK: Once = Once::new();

/// RAII guard that owns the raw-mode + alternate-screen state.
///
/// Hold this for the lifetime of the TUI. Its `Drop` is the single source of
/// truth for restoration, so every exit path (including `?` and panics) cleans
/// up exactly once.
pub struct TerminalGuard {
    terminal: Tui,
}

impl TerminalGuard {
    /// Enter raw mode + alternate screen and install the panic hook. Returns a
    /// ready-to-draw terminal wrapped in the guard.
    ///
    /// All-or-nothing: if any step fails, every step already performed is undone
    /// before the error is returned, so the terminal is left EXACTLY as it was
    /// found (raw mode disabled, main screen). A partially-armed terminal is
    /// never observable by the caller.
    pub fn enter() -> io::Result<Self> {
        install_panic_hook();

        enable_raw_mode()?;

        let mut stdout = io::stdout();
        if let Err(e) = execute!(stdout, EnterAlternateScreen) {
            // Raw mode was enabled above; undo it before surfacing the error.
            let _ = disable_raw_mode();
            return Err(e);
        }

        let backend = CrosstermBackend::new(stdout);
        let terminal = match Terminal::new(backend) {
            Ok(t) => t,
            Err(e) => {
                // Alt screen + raw mode are both live; undo both in reverse.
                let mut stdout = io::stdout();
                let _ = execute!(stdout, LeaveAlternateScreen);
                let _ = disable_raw_mode();
                return Err(e);
            }
        };

        // Fully armed: from here, restoration is owned by Drop / the panic hook.
        RESTORED.store(false, Ordering::SeqCst);
        Ok(Self { terminal })
    }

    /// Mutable access to the underlying terminal for drawing.
    pub fn terminal_mut(&mut self) -> &mut Tui {
        &mut self.terminal
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Best-effort restore; nothing useful to do if these fail during
        // teardown. Idempotent: if the panic hook already restored, this is a
        // no-op.
        let _ = restore();
    }
}

/// Atomically claim the right to perform restoration. Returns `true` exactly
/// once per armed cycle (the transition `false -> true`); every later call
/// returns `false`. Splitting this out keeps the idempotence logic pure and
/// unit-testable without touching the real terminal.
fn claim_restore() -> bool {
    !RESTORED.swap(true, Ordering::SeqCst)
}

/// Leave the alternate screen and disable raw mode, exactly once per `enter()`.
///
/// Idempotent: the first call after a successful `enter()` performs the work and
/// marks the terminal restored; any subsequent call (e.g. `Drop` running after
/// the panic hook already restored) returns immediately without re-emitting the
/// escape sequences.
fn restore() -> io::Result<()> {
    if !claim_restore() {
        return Ok(());
    }
    let mut stdout = io::stdout();
    execute!(stdout, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}

/// Chain the existing panic hook so we restore the terminal first, then let the
/// default hook print the panic where the user can actually read it. Installed
/// exactly once via [`Once`]; later `enter()` calls do not re-wrap, so hooks
/// never stack. The hook shares the idempotent [`restore`], so it cooperates
/// with `Drop` (whichever runs first does the work; the other is a no-op).
fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let original = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = restore();
            original(info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The idempotence contract behind the double-restore fix, exercised purely
    /// via `claim_restore` so no raw mode / alternate screen is ever touched.
    ///
    /// A single test (not two) drives the whole sequence: the shared `RESTORED`
    /// atomic is global, so splitting this across tests would race under the
    /// default parallel test runner.
    #[test]
    fn claim_restore_fires_exactly_once_per_armed_cycle() {
        // Armed state (post-`enter()`): RESTORED == false.
        RESTORED.store(false, Ordering::SeqCst);
        // First claim (e.g. the panic hook) wins and does the work...
        assert!(claim_restore(), "first claim after arming must win");
        // ...a second claim (e.g. Drop) is a no-op -> no double escape sequences.
        assert!(!claim_restore(), "second claim must be a no-op");
        assert!(!claim_restore(), "and stays a no-op");

        // Not armed (RESTORED already true): every claim is a no-op, so calling
        // restore() before/without a successful enter() never touches the tty.
        RESTORED.store(true, Ordering::SeqCst);
        assert!(!claim_restore(), "claim without arming is a no-op");

        // Leave the global flag in the safe (restored) state for other tests.
        RESTORED.store(true, Ordering::SeqCst);
    }
}
