// ─── C6: ConPTY master + portable_pty trait impls ─────────────────────────────
//
// The interactive agent terminal (agent_pty.rs) needs a real console inside the
// AppContainer. portable_pty's own ConPTY spawn cannot be pointed at our broker,
// so we create the pseudoconsole here and expose it through portable_pty's
// traits: SandboxedChild becomes a portable_pty::Child, and WindowsConPtyMaster
// implements MasterPty over a caller-created HPCON + the two pipes.

/// Create a ConPTY (HPCON) plus the two pipes that form its host side.
/// Returns (master, hpc):
/// - master: read/write/resize endpoints for the app (agent_pty uses them).
/// - hpc: the pseudoconsole handle to pass to [`spawn_sandboxed_pty`]; the
///   caller must keep it alive until the child is reaped (it is referenced by
///   the child's stdio, not copied), then close it with ClosePseudoConsole.
pub fn create_conpty(
    rows: u16,
    cols: u16,
) -> Result<(WindowsConPtyMaster, HANDLE), String> {
    use windows::Win32::System::Console::{
        CreatePseudoConsole, COORD, HPCON,
    };
    // Same pipe wiring as portable-pty's PsuedoCon::new: hInput = read end of
    // the host→child pipe, hOutput = write end of the child→host pipe.
    let (input_read, input_write) = create_pipe()?;
    let (output_read, output_write) = create_pipe()?;
    let mut hpc: HPCON = HPCON::default();
    unsafe {
        CreatePseudoConsole(
            COORD { X: cols as i16, Y: rows as i16 },
            input_read,
            output_write,
            0,
            &mut hpc,
        )
        .map_err(|e| format!("CreatePseudoConsole failed: {e}"))?;
    }
    let master = WindowsConPtyMaster {
        hpc,
        input_write,
        output_read,
        // The read end of the host pipe and write end of the child pipe are
        // owned by the OS now; close our copies on drop.
        _input_read: input_read,
        _output_write: output_write,
        rows,
        cols,
    };
    Ok((master, hpc))
}

/// Host side of a ConPTY: writing to `input_write` feeds the child's stdin,
/// reading from `output_read` consumes the child's stdout/stderr, and resize
/// resizes the pseudoconsole. Implements portable_pty::MasterPty so
/// agent_pty.rs can keep its trait-based plumbing.
pub struct WindowsConPtyMaster {
    hpc: HANDLE,
    input_write: HANDLE,
    output_read: HANDLE,
    _input_read: HANDLE,
    _output_write: HANDLE,
    rows: u16,
    cols: u16,
}

impl Drop for WindowsConPtyMaster {
    fn drop(&mut self) {
        unsafe {
            // ClosePseudoConsole is async: it signals the conhost to exit but
            // does not block. portable-pty calls it in PsuedoCon::drop too.
            let _ = windows::Win32::System::Console::ClosePseudoConsole(self.hpc);
            let _ = CloseHandle(self.input_write);
            let _ = CloseHandle(self.output_read);
            let _ = CloseHandle(self._input_read);
            let _ = CloseHandle(self._output_write);
        }
    }
}

impl WindowsConPtyMaster {
    /// Duplicate `output_read` for the reader thread (each reader gets its own
    /// handle; the master keeps the original for its lifetime).
    pub fn duplicate_reader(&self) -> Result<HANDLE, String> {
        unsafe {
            let mut dup = HANDLE::default();
            windows::Win32::System::Threading::DuplicateHandle(
                windows::Win32::System::Threading::GetCurrentProcess(),
                self.output_read,
                windows::Win32::System::Threading::GetCurrentProcess(),
                &mut dup,
                0,
                false,
                windows::Win32::System::Threading::DUPLICATE_SAME_ACCESS,
            )
            .map_err(|e| format!("DuplicateHandle(reader) failed: {e}"))?;
            Ok(dup)
        }
    }

    /// Duplicate `input_write` for the writer (the master keeps the original).
    pub fn duplicate_writer(&self) -> Result<HANDLE, String> {
        unsafe {
            let mut dup = HANDLE::default();
            windows::Win32::System::Threading::DuplicateHandle(
                windows::Win32::System::Threading::GetCurrentProcess(),
                self.input_write,
                windows::Win32::System::Threading::GetCurrentProcess(),
                &mut dup,
                0,
                false,
                windows::Win32::System::Threading::DUPLICATE_SAME_ACCESS,
            )
            .map_err(|e| format!("DuplicateHandle(writer) failed: {e}"))?;
            Ok(dup)
        }
    }
}

impl std::fmt::Debug for WindowsConPtyMaster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsConPtyMaster")
            .field("hpc", &self.hpc.0)
            .field("rows", &self.rows)
            .field("cols", &self.cols)
            .finish()
    }
}

impl downcast_rs::Downcast for WindowsConPtyMaster {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }
    fn into_any_rc(self: std::rc::Rc<Self>) -> std::rc::Rc<dyn std::any::Any> {
        self
    }
}

impl portable_pty::MasterPty for WindowsConPtyMaster {
    fn resize(&self, size: portable_pty::PtySize) -> Result<(), portable_pty::Error> {
        use windows::Win32::System::Console::ResizePseudoConsole;
        unsafe {
            ResizePseudoConsole(
                self.hpc,
                windows::Win32::System::Console::COORD {
                    X: size.cols as i16,
                    Y: size.rows as i16,
                },
            )
            .map_err(|e| {
                portable_pty::Error::IoError(std::io::Error::other(format!(
                    "ResizePseudoConsole failed: {e}"
                )))
            })
        }
    }

    fn get_size(&self) -> Result<portable_pty::PtySize, portable_pty::Error> {
        Ok(portable_pty::PtySize {
            rows: self.rows,
            cols: self.cols,
            pixel_width: 0,
            pixel_height: 0,
        })
    }

    fn try_clone_reader(
        &self,
    ) -> Result<Box<dyn std::io::Read + Send>, portable_pty::Error> {
        let dup = self
            .duplicate_reader()
            .map_err(|e| portable_pty::Error::IoError(std::io::Error::other(e)))?;
        // Safety: dup is a fresh handle owned by us.
        let file = unsafe { std::fs::File::from_raw_handle(dup.0 as _) };
        Ok(Box::new(file))
    }

    fn take_writer(&self) -> Result<Box<dyn std::io::Write + Send>, portable_pty::Error> {
        let dup = self
            .duplicate_writer()
            .map_err(|e| portable_pty::Error::IoError(std::io::Error::other(e)))?;
        // Safety: dup is a fresh handle owned by us.
        let file = unsafe { std::fs::File::from_raw_handle(dup.0 as _) };
        Ok(Box::new(file))
    }
}

/// SandboxedChild as a portable_pty::Child. `wait` maps to wait_and_restore
/// (child reaped + ACLs/AppContainer profile restored); `kill` terminates the
/// whole job (descendants included).
impl downcast_rs::Downcast for SandboxedChild {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }
    fn into_any_rc(self: std::rc::Rc<Self>) -> std::rc::Rc<dyn std::any::Any> {
        self
    }
}

impl std::fmt::Debug for SandboxedChild {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxedChild")
            .field("pid", &self.pid)
            .field("restored", &self.restored)
            .finish()
    }
}

impl portable_pty::ChildKiller for SandboxedChild {
    fn kill(&mut self) -> std::io::Result<()> {
        SandboxedChild::kill(self).map_err(std::io::Error::other)
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(SandboxedChildKiller {
            pid: self.pid,
            process_handle: self.process_handle,
            job: self.job,
        })
    }
}

impl portable_pty::Child for SandboxedChild {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        SandboxedChild::try_wait(self)
            .map(|opt| opt.map(|code| portable_pty::ExitStatus::with_exit_code(code as u32)))
            .map_err(std::io::Error::other)
    }

    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        // wait_and_restore reaps the child AND restores ACLs + AppContainer
        // profile — the PTY teardown path relies on this.
        let code = self.wait_and_restore().map_err(std::io::Error::other)?;
        Ok(portable_pty::ExitStatus::with_exit_code(code as u32))
    }

    fn process_id(&self) -> Option<u32> {
        Some(self.pid)
    }

    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        Some(self.process_handle.0 as _)
    }
}

/// Standalone killer for a sandboxed child (what clone_killer returns): holds
/// the raw handles WITHOUT the ACL/profile restore responsibility, so it can
/// be shared across threads (Send+Sync) and used after the child object moved
/// into the reap path. Terminating the job kills all descendants.
#[derive(Clone)]
struct SandboxedChildKiller {
    pid: u32,
    process_handle: HANDLE,
    job: HANDLE,
}

impl std::fmt::Debug for SandboxedChildKiller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxedChildKiller")
            .field("pid", &self.pid)
            .finish()
    }
}

impl downcast_rs::Downcast for SandboxedChildKiller {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }
    fn into_any_rc(self: std::rc::Rc<Self>) -> std::rc::Rc<dyn std::any::Any> {
        self
    }
}

impl portable_pty::ChildKiller for SandboxedChildKiller {
    fn kill(&mut self) -> std::io::Result<()> {
        unsafe {
            if !self.job.0.is_null() {
                TerminateJobObject(self.job, 1).map_err(std::io::Error::other)?;
            } else if !self.process_handle.0.is_null() {
                windows::Win32::System::Threading::TerminateProcess(self.process_handle, 1)
                    .map_err(std::io::Error::other)?;
            }
        }
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(self.clone())
    }
}
