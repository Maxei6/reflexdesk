//! Crash-proof process ownership supervisor.
//!
//! Enforces:
//! - Explicit ownership classes (`Internal`, `OwnedSession`, `Attached`, `UserApp`).
//! - Owned processes (`Internal`, `OwnedSession`) terminate on supervisor exit or crash.
//! - External processes (`UserApp`, `Attached`) NEVER auto-terminate.
//! - IDs are supervisor-assigned `u128` values, never exposing raw OS PIDs to the frontend.
//! - Platform-specific protections: Windows Job Objects with `KILL_ON_JOB_CLOSE`, POSIX process groups with parent-death guards.

use crate::process::platform::{self, PlatformHandle};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Child, ChildStderr, ChildStdout, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Process ownership classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipClass {
    /// STT, reflex, planner internal helpers. Always terminate on quit/crash.
    Internal,
    /// Agent or browser automation instance launched by ReflexDesk.
    OwnedSession,
    /// Pre-existing external service or session. Detach only; never kill.
    Attached,
    /// User applications (Chrome, Spotify, VS Code). Never auto-kill.
    UserApp,
}

impl OwnershipClass {
    /// Returns true if ReflexDesk owns this process lifecycle and must terminate it on shutdown.
    #[must_use]
    pub fn is_owned(self) -> bool {
        matches!(self, OwnershipClass::Internal | OwnershipClass::OwnedSession)
    }
}

/// Opaque supervisor process identifier (u128). Never exposes raw OS PIDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProcessId(pub u128);

impl ProcessId {
    /// Creates a new `ProcessId` from a monotonic counter and random suffix.
    #[must_use]
    pub fn new(counter: u64, random_suffix: u64) -> Self {
        let id = ((counter as u128) << 64) | (random_suffix as u128);
        Self(id)
    }

    /// Access the raw u128 representation.
    #[must_use]
    pub fn raw(self) -> u128 {
        self.0
    }
}

impl std::fmt::Display for ProcessId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "proc_{:032x}", self.0)
    }
}

/// Specification for spawning a supervised process.
#[derive(Debug, Clone)]
pub struct ProcessSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env_add: HashMap<String, String>,
    pub env_remove: Vec<String>,
    pub ownership: OwnershipClass,
    pub piped_stdio: bool,
}

impl ProcessSpec {
    /// Create a new process specification. Accepts any path-like (`&str`,
    /// `&Path`, `PathBuf`, …) so callers never need to clone to satisfy bounds.
    pub fn new<P: AsRef<Path>>(program: P, ownership: OwnershipClass) -> Self {
        Self {
            program: program.as_ref().to_path_buf(),
            args: Vec::new(),
            cwd: None,
            env_add: HashMap::new(),
            env_remove: Vec::new(),
            ownership,
            piped_stdio: false,
        }
    }
    pub fn arg<S: Into<String>>(mut self, arg: S) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for a in args {
            self.args.push(a.into());
        }
        self
    }

    pub fn cwd<P: AsRef<Path>>(mut self, cwd: P) -> Self {
        self.cwd = Some(cwd.as_ref().to_path_buf());
        self
    }

    pub fn env<K: Into<String>, V: Into<String>>(mut self, key: K, val: V) -> Self {
        self.env_add.insert(key.into(), val.into());
        self
    }

    pub fn env_remove<K: Into<String>>(mut self, key: K) -> Self {
        self.env_remove.push(key.into());
        self
    }

    pub fn with_piped_stdio(mut self, piped: bool) -> Self {
        self.piped_stdio = piped;
        self
    }
}

/// Process identity tracking PID, start identity, and group/job identifiers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_identity: u64,
    pub group_or_job: Option<String>,
}

/// Handle representing a supervised process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessHandle {
    pub id: ProcessId,
    pub ownership: OwnershipClass,
}

/// Diagnostic snapshot of a supervised process without sensitive environment or arguments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessSnapshot {
    pub id: String,
    pub pid: Option<u32>,
    pub ownership: OwnershipClass,
    pub start_time_ms: u64,
    pub alive: bool,
    pub program: String,
}

struct TrackedChild {
    identity: ProcessIdentity,
    ownership: OwnershipClass,
    program: String,
    start_time: Instant,
    start_time_ms: u64,
    child: Option<Child>,
    platform_handle: Option<PlatformHandle>,
}

struct SupervisorInner {
    children: HashMap<u128, TrackedChild>,
}

impl SupervisorInner {
    fn new() -> Self {
        Self {
            children: HashMap::new(),
        }
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(1);
static GLOBAL_SUPERVISORS: Mutex<Vec<Arc<Mutex<SupervisorInner>>>> = Mutex::new(Vec::new());

fn next_process_id() -> ProcessId {
    use rand::RngCore;
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut rnd_bytes = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut rnd_bytes);
    let rnd = u64::from_le_bytes(rnd_bytes);
    ProcessId::new(count, rnd)
}

/// Process supervisor managing child process lifecycles and ownership boundaries.
pub struct ProcessSupervisor {
    inner: Arc<Mutex<SupervisorInner>>,
}

impl Default for ProcessSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ProcessSupervisor {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl ProcessSupervisor {
    /// Create a new supervisor and register it in the global registry.
    pub fn new() -> Self {
        let inner = Arc::new(Mutex::new(SupervisorInner::new()));
        if let Ok(mut registry) = GLOBAL_SUPERVISORS.lock() {
            registry.push(Arc::clone(&inner));
        }
        Self { inner }
    }

    /// Spawns a new process according to `ProcessSpec`, enforcing ownership and platform rules.
    pub fn spawn(&self, spec: ProcessSpec) -> Result<ProcessId, String> {
        let id = next_process_id();
        let now = Instant::now();
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let program_name = spec
            .program
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| spec.program.to_string_lossy().to_string());

        let (pid, child_opt, platform_opt) = match spec.ownership {
            // USER_APP and ATTACHED are spawned normally without Job Objects or PR_SET_PDEATHSIG
            OwnershipClass::UserApp | OwnershipClass::Attached => {
                let mut cmd = Command::new(&spec.program);
                cmd.args(&spec.args);
                if let Some(dir) = &spec.cwd {
                    cmd.current_dir(dir);
                }
                for (k, v) in &spec.env_add {
                    cmd.env(k, v);
                }
                for k in &spec.env_remove {
                    cmd.env_remove(k);
                }

                #[cfg(target_os = "windows")]
                {
                    use std::os::windows::process::CommandExt;
                    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                    cmd.creation_flags(CREATE_NO_WINDOW);
                }

                let child = cmd
                    .spawn()
                    .map_err(|e| format!("failed to spawn {program_name}: {e}"))?;
                let pid = child.id();
                (pid, Some(child), None)
            }

            // INTERNAL and OWNED_SESSION are bound to platform ownership mechanisms
            OwnershipClass::Internal | OwnershipClass::OwnedSession => {
                #[cfg(target_os = "windows")]
                {
                    if spec.piped_stdio {
                        let mut cmd = Command::new(&spec.program);
                        cmd.args(&spec.args);
                        if let Some(dir) = &spec.cwd {
                            cmd.current_dir(dir);
                        }
                        for (k, v) in &spec.env_add {
                            cmd.env(k, v);
                        }
                        for k in &spec.env_remove {
                            cmd.env_remove(k);
                        }
                        cmd.stdin(Stdio::null())
                            .stdout(Stdio::piped())
                            .stderr(Stdio::piped());

                        let (child, job_handle) =
                            platform::spawn_command_in_job(&mut cmd)?;
                        let pid = child.id();
                        (pid, Some(child), Some(job_handle))
                    } else {
                        let (pid, job_handle) = platform::spawn_suspended_in_job(
                            &spec.program,
                            &spec.args,
                            spec.cwd.as_deref(),
                            &spec.env_add,
                            &spec.env_remove,
                        )?;
                        (pid, None, Some(job_handle))
                    }
                }

                #[cfg(any(target_os = "linux", target_os = "macos"))]
                {
                    let mut cmd = Command::new(&spec.program);
                    cmd.args(&spec.args);
                    if let Some(dir) = &spec.cwd {
                        cmd.current_dir(dir);
                    }
                    for (k, v) in &spec.env_add {
                        cmd.env(k, v);
                    }
                    for k in &spec.env_remove {
                        cmd.env_remove(k);
                    }

                    if spec.piped_stdio {
                        cmd.stdin(Stdio::null())
                            .stdout(Stdio::piped())
                            .stderr(Stdio::piped());
                    }

                    let maybe_platform = platform::configure_posix_command(&mut cmd)?;
                    let child = cmd
                        .spawn()
                        .map_err(|e| format!("failed to spawn {program_name}: {e}"))?;
                    let pid = child.id();
                    let pgid = pid as i32;

                    let mut plat = maybe_platform
                        .unwrap_or_else(|| platform::PlatformHandle::new(pgid));
                    plat.pgid = pgid;

                    (pid, Some(child), Some(plat))
                }

                #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
                {
                    let mut cmd = Command::new(&spec.program);
                    cmd.args(&spec.args);
                    if let Some(dir) = &spec.cwd {
                        cmd.current_dir(dir);
                    }
                    for (k, v) in &spec.env_add {
                        cmd.env(k, v);
                    }
                    for k in &spec.env_remove {
                        cmd.env_remove(k);
                    }
                    if spec.piped_stdio {
                        cmd.stdin(Stdio::null())
                            .stdout(Stdio::piped())
                            .stderr(Stdio::piped());
                    }
                    let child = cmd
                        .spawn()
                        .map_err(|e| format!("failed to spawn {program_name}: {e}"))?;
                    let pid = child.id();
                    (pid, Some(child), None)
                }
            }
        };

        let group_or_job = match spec.ownership {
            OwnershipClass::Internal | OwnershipClass::OwnedSession => {
                Some(format!("{}-pid-{}", spec.ownership.is_owned(), pid))
            }
            _ => None,
        };

        let identity = ProcessIdentity {
            pid,
            start_identity: now_ms,
            group_or_job,
        };

        let tracked = TrackedChild {
            identity,
            ownership: spec.ownership,
            program: program_name,
            start_time: now,
            start_time_ms: now_ms,
            child: child_opt,
            platform_handle: platform_opt,
        };

        let mut inner = self.inner.lock().map_err(|_| "supervisor lock poisoned")?;
        inner.children.insert(id.raw(), tracked);

        Ok(id)
    }

    /// Terminates an owned process (`Internal` or `OwnedSession`).
    /// Returns false if the process does not exist or is `UserApp` / `Attached`.
    pub fn terminate_owned(&self, id: ProcessId) -> bool {
        let mut inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(_) => return false,
        };

        if let Some(tracked) = inner.children.get(&id.raw()) {
            if !tracked.ownership.is_owned() {
                // Safeguard: Never kill UserApp or Attached via terminate_owned!
                return false;
            }
        } else {
            return false;
        }

        if let Some(mut tracked) = inner.children.remove(&id.raw()) {
            if let Some(plat) = tracked.platform_handle.take() {
                let _ = plat.terminate();
            }
            if let Some(mut child) = tracked.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            true
        } else {
            false
        }
    }

    /// Terminates all owned processes (`Internal` and `OwnedSession`).
    /// `UserApp` and `Attached` processes are NEVER terminated.
    pub fn terminate_all_owned(&self) -> usize {
        let mut inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(_) => return 0,
        };

        let mut to_kill = Vec::new();
        let keys: Vec<u128> = inner.children.keys().copied().collect();

        for key in keys {
            if let Some(child) = inner.children.get(&key) {
                if child.ownership.is_owned() {
                    if let Some(tracked) = inner.children.remove(&key) {
                        to_kill.push(tracked);
                    }
                }
            }
        }

        let mut killed = 0;
        for mut tracked in to_kill {
            if let Some(plat) = tracked.platform_handle.take() {
                let _ = plat.terminate();
            }
            if let Some(mut child) = tracked.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            killed += 1;
        }

        killed
    }

    /// Backward-compatible alias for `terminate_all_owned`.
    pub fn terminate_all(&self) {
        let _ = self.terminate_all_owned();
    }

    /// Detaches an `Attached` or tracked process, removing it from supervisor tracking without termination.
    pub fn detach(&self, id: ProcessId) -> bool {
        let mut inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(_) => return false,
        };
        inner.children.remove(&id.raw()).is_some()
    }

    /// Takes the stdout and stderr handles for a supervised process (e.g. for STT log forwarding).
    pub fn take_stdio(&self, id: ProcessId) -> Option<(Option<ChildStdout>, Option<ChildStderr>)> {
        let mut inner = self.inner.lock().ok()?;
        let tracked = inner.children.get_mut(&id.raw())?;
        let child = tracked.child.as_mut()?;
        Some((child.stdout.take(), child.stderr.take()))
    }

    /// Check if a supervised process is still alive.
    pub fn is_alive(&self, id: ProcessId) -> bool {
        let mut inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(_) => return false,
        };
        if let Some(tracked) = inner.children.get_mut(&id.raw()) {
            if let Some(child) = tracked.child.as_mut() {
                matches!(child.try_wait(), Ok(None))
            } else {
                true
            }
        } else {
            false
        }
    }

    /// Reaps finished child processes.
    pub fn cleanup_finished(&self) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.children.retain(|_, tracked| {
            if let Some(child) = tracked.child.as_mut() {
                matches!(child.try_wait(), Ok(None))
            } else {
                true
            }
        });
    }

    /// Diagnostic snapshot of all tracked processes (no command arguments or secrets).
    pub fn snapshot(&self) -> Vec<ProcessSnapshot> {
        let Ok(mut inner) = self.inner.lock() else {
            return Vec::new();
        };

        inner
            .children
            .iter_mut()
            .map(|(raw_id, tracked)| {
                let alive = if let Some(child) = tracked.child.as_mut() {
                    matches!(child.try_wait(), Ok(None))
                } else {
                    true
                };
                ProcessSnapshot {
                    id: ProcessId(*raw_id).to_string(),
                    pid: Some(tracked.identity.pid),
                    ownership: tracked.ownership,
                    start_time_ms: tracked.start_time_ms,
                    alive,
                    program: tracked.program.clone(),
                }
            })
            .collect()
    }

    /// Legacy tracking method for backward compatibility.
    pub fn track(&self, label: String, child: Child) -> Result<ProcessId, String> {
        let id = next_process_id();
        let pid = child.id();
        let now = Instant::now();
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let identity = ProcessIdentity {
            pid,
            start_identity: now_ms,
            group_or_job: Some(label),
        };

        let tracked = TrackedChild {
            identity,
            ownership: OwnershipClass::OwnedSession,
            program: format!("pid-{pid}"),
            start_time: now,
            start_time_ms: now_ms,
            child: Some(child),
            platform_handle: None,
        };

        let mut inner = self.inner.lock().map_err(|_| "supervisor lock poisoned")?;
        inner.children.insert(id.raw(), tracked);
        Ok(id)
    }

    /// Terminates an owned process across all supervisor instances.
    pub fn terminate_process_global(id: ProcessId) -> bool {
        if let Ok(registry) = GLOBAL_SUPERVISORS.lock() {
            for inner_arc in registry.iter() {
                if let Ok(mut inner) = inner_arc.lock() {
                    if let Some(tracked) = inner.children.get(&id.raw()) {
                        if !tracked.ownership.is_owned() {
                            return false;
                        }
                    }
                    if let Some(mut tracked) = inner.children.remove(&id.raw()) {
                        if let Some(plat) = tracked.platform_handle.take() {
                            let _ = plat.terminate();
                        }
                        if let Some(mut child) = tracked.child.take() {
                            let _ = child.kill();
                            let _ = child.wait();
                        }
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Checks if a process is alive across all supervisor instances.
    pub fn is_alive_global(id: ProcessId) -> bool {
        if let Ok(registry) = GLOBAL_SUPERVISORS.lock() {
            for inner_arc in registry.iter() {
                if let Ok(mut inner) = inner_arc.lock() {
                    if let Some(tracked) = inner.children.get_mut(&id.raw()) {
                        if let Some(child) = tracked.child.as_mut() {
                            return matches!(child.try_wait(), Ok(None));
                        }
                        return true;
                    }
                }
            }
        }
        false
    }
}

impl Drop for ProcessSupervisor {
    fn drop(&mut self) {
        // If this is the last reference to the inner supervisor, terminate owned processes
        if Arc::strong_count(&self.inner) <= 2 {
            let _ = self.terminate_all_owned();
        }
    }
}
