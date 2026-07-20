//! Exclusive flock on `.aspis-agents.json.lock` (same 100×50ms spin as agents.rs).

use super::paths::agents_lock_path;
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::path::Path;
use std::thread;
use std::time::Duration;

#[derive(Debug)]
pub struct AgentStateError(pub String);

impl std::fmt::Display for AgentStateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for AgentStateError {}

struct AgentStateFileLock {
    _file: File,
}

fn acquire_lock(projects_dir: &Path) -> Result<AgentStateFileLock, AgentStateError> {
    fs::create_dir_all(projects_dir).map_err(|e| {
        AgentStateError(format!("Could not create projects folder: {e}"))
    })?;
    let lock_path = agents_lock_path(projects_dir);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
        .map_err(|e| {
            AgentStateError(format!(
                "Could not open agent state lock {}: {e}",
                lock_path.display()
            ))
        })?;
    for _ in 0..100 {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(AgentStateFileLock { _file: file }),
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
    Err(AgentStateError(format!(
        "Could not acquire agent state lock: {}",
        lock_path.display()
    )))
}

/// Hold exclusive flock for the duration of `f`.
pub fn with_agents_lock<T, E, F>(projects_dir: &Path, f: F) -> Result<T, E>
where
    F: FnOnce() -> Result<T, E>,
    E: From<AgentStateError>,
{
    let _guard = acquire_lock(projects_dir)?;
    f()
}
