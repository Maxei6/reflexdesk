//! Platform-specific process management and ownership boundaries.
//!
//! Windows: Job Objects with `KILL_ON_JOB_CLOSE` + assign-before-resume.
//! POSIX: Dedicated session/process groups via setsid/setpgid, SIGTERM with 2s grace then SIGKILL,
//!        Linux PR_SET_PDEATHSIG, macOS pipe guardian.

#[cfg(target_os = "windows")]
pub mod win;
#[cfg(target_os = "windows")]
pub use win as platform;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod posix;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use posix as platform;

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub mod fallback;
#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub use fallback as platform;
