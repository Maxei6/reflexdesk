//! POSIX process supervision using process groups and parent-liveness guards.
//!
//! Enforces:
//! - Dedicated process group / session via `setsid` / `setpgid` in `pre_exec`.
//! - SIGTERM to entire process group (`-pgid`), bounded grace period (2 seconds), then SIGKILL.
//! - Linux: `PR_SET_PDEATHSIG` with post-set parent liveness re-check to eliminate fork/exec race.
//! - macOS: Parent-liveness pipe guardian that monitors parent process survival.

use std::os::unix::process::CommandExt;
use std::process::Command;
use std::time::{Duration, Instant};

/// Encapsulates a POSIX process group and optional parent-liveness pipe handles.
#[derive(Debug)]
pub struct PlatformHandle {
    pub pgid: i32,
    #[cfg(target_os = "macos")]
    pub pipe_write: Option<std::os::unix::io::RawFd>,
}

impl PlatformHandle {
    pub fn new(pgid: i32) -> Self {
        Self {
            pgid,
            #[cfg(target_os = "macos")]
            pipe_write: None,
        }
    }

    /// Terminate the process group with SIGTERM, 2-second grace, then SIGKILL.
    pub fn terminate(&self) -> Result<(), String> {
        terminate_process_group(self.pgid);
        Ok(())
    }

    pub fn identity_label(&self) -> String {
        format!("posix-pgid-{}", self.pgid)
    }
}

impl Drop for PlatformHandle {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(fd) = self.pipe_write.take() {
            unsafe {
                libc::close(fd);
            }
        }
    }
}

pub fn process_start_identity(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after_comm = stat.rsplit_once(") ")?.1;
        return after_comm.split_whitespace().nth(19)?.parse().ok();
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// Send SIGTERM to the process group (-pgid), wait up to 2 seconds,
/// and send SIGKILL if any process in the group remains alive.
pub fn terminate_process_group(pgid: i32) {
    if pgid <= 1 {
        // Safety: Never signal process group 0 (all processes in current group) or 1 (init)!
        return;
    }

    unsafe {
        // 1. Send SIGTERM to process group
        libc::kill(-pgid, libc::SIGTERM);
    }

    // 2. Bounded grace period of 2 seconds
    let start = Instant::now();
    let grace = Duration::from_secs(2);
    while start.elapsed() < grace {
        std::thread::sleep(Duration::from_millis(50));
        let alive = unsafe { libc::kill(-pgid, 0) == 0 };
        if !alive {
            return;
        }
    }

    // 3. SIGKILL to process group if any processes survived the grace period
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
}

/// Configures a `std::process::Command` with Unix pre-exec hooks for process ownership.
pub fn configure_posix_command(command: &mut Command) -> Result<Option<PlatformHandle>, String> {
    #[cfg(target_os = "linux")]
    {
        let parent_pid = unsafe { libc::getpid() };

        unsafe {
            command.pre_exec(move || {
                // 1. Dedicated session / process group
                if libc::setsid() == -1 {
                    libc::setpgid(0, 0);
                }

                // 2. Linux: Ask kernel to deliver SIGKILL to child if parent dies
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    libc::_exit(1);
                }

                // 3. Post-set parent liveness re-check:
                // If parent died between fork and prctl, child would be reparented to init.
                if libc::getppid() != parent_pid {
                    libc::_exit(1);
                }

                Ok(())
            });
        }

        Ok(None)
    }

    #[cfg(target_os = "macos")]
    {
        // macOS lacks PR_SET_PDEATHSIG. We create an anonymous pipe where the parent
        // holds the write end and child holds the read end.
        let mut fds = [0i32; 2];
        let pipe_res = unsafe { libc::pipe(fds.as_mut_ptr()) };
        if pipe_res != 0 {
            return Err("failed to create liveness pipe for macOS".to_string());
        }

        let read_fd = fds[0];
        let write_fd = fds[1];

        // Ensure parent write_fd is closed on exec in case child spawns others
        unsafe {
            let flags = libc::fcntl(write_fd, libc::F_GETFD);
            if flags != -1 {
                libc::fcntl(write_fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }

        unsafe {
            command.pre_exec(move || {
                // Dedicated session/process group. The target PID becomes the PGID.
                if libc::setsid() == -1 {
                    libc::setpgid(0, 0);
                }
                libc::close(write_fd);

                // macOS has no PDEATHSIG. Fork a minimal guardian before exec.
                // It owns only the liveness read end; EOF means the ReflexDesk
                // parent closed/died, so it kills the entire owned process group.
                let target_pgid = libc::getpid();
                let guardian = libc::fork();
                if guardian == -1 {
                    libc::close(read_fd);
                    return Err(std::io::Error::last_os_error());
                }
                if guardian == 0 {
                    let mut byte = [0u8; 1];
                    loop {
                        let n = libc::read(read_fd, byte.as_mut_ptr().cast(), 1);
                        if n == 0 {
                            libc::kill(-target_pgid, libc::SIGKILL);
                            libc::_exit(0);
                        }
                        if n < 0 {
                            let error = *libc::__error();
                            if error == libc::EINTR {
                                continue;
                            }
                            libc::kill(-target_pgid, libc::SIGKILL);
                            libc::_exit(1);
                        }
                    }
                }

                // Only the guardian watches the pipe. The target proceeds to exec.
                libc::close(read_fd);
                Ok(())
            });
        }

        Ok(Some(PlatformHandle {
            pgid: 0, // Set after child is spawned
            pipe_write: Some(write_fd),
        }))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    libc::setpgid(0, 0);
                }
                Ok(())
            });
        }
        Ok(None)
    }
}
