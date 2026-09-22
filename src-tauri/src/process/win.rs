//! Windows process ownership via Job Objects.
//!
//! Enforces `KILL_ON_JOB_CLOSE` so that when ReflexDesk exits or crashes,
//! the Windows kernel automatically terminates all processes in the job tree,
//! including any grandchildren.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::process::Command;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, BOOL, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject, TerminateJobObject,
    JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, ResumeThread, TerminateProcess, PROCESS_INFORMATION, STARTUPINFOW,
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT,
};

/// Encapsulates a Windows Job Object handle with `KILL_ON_JOB_CLOSE`.
#[derive(Debug)]
pub struct PlatformHandle {
    job: HANDLE,
}

unsafe impl Send for PlatformHandle {}
unsafe impl Sync for PlatformHandle {}

impl PlatformHandle {
    /// Create a new Job Object configured with `KILL_ON_JOB_CLOSE`.
    pub fn new() -> Result<Self, String> {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job == 0 as _ || job == INVALID_HANDLE_VALUE {
                return Err(format!("CreateJobObjectW failed (error {})", GetLastError()));
            }

            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

            let success: BOOL = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );

            if success == 0 {
                let err = GetLastError();
                CloseHandle(job);
                return Err(format!("SetInformationJobObject failed (error {err})"));
            }

            Ok(Self { job })
        }
    }

    /// Assign an existing process handle to this Job Object.
    pub fn assign_process(&self, process_handle: HANDLE) -> Result<(), String> {
        unsafe {
            let success: BOOL = AssignProcessToJobObject(self.job, process_handle);
            if success == 0 {
                return Err(format!("AssignProcessToJobObject failed (error {})", GetLastError()));
            }
            Ok(())
        }
    }

    /// Explicitly terminate all processes in this Job Object.
    pub fn terminate(&self) -> Result<(), String> {
        unsafe {
            let success: BOOL = TerminateJobObject(self.job, 1);
            if success == 0 {
                return Err(format!("TerminateJobObject failed (error {})", GetLastError()));
            }
            Ok(())
        }
    }

    /// Raw job handle.
    pub fn raw_handle(&self) -> HANDLE {
        self.job
    }
}

impl Drop for PlatformHandle {
    fn drop(&mut self) {
        unsafe {
            if self.job != 0 as _ && self.job != INVALID_HANDLE_VALUE {
                CloseHandle(self.job);
            }
        }
    }
}

fn to_wide_null<S: AsRef<OsStr>>(s: S) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

fn format_windows_command_line(program: &Path, args: &[String]) -> String {
    let mut line = String::new();
    let prog_str = program.to_string_lossy();
    if prog_str.contains(' ') || prog_str.contains('\t') {
        line.push('"');
        line.push_str(&prog_str);
        line.push('"');
    } else {
        line.push_str(&prog_str);
    }

    for arg in args {
        line.push(' ');
        if arg.contains(' ') || arg.contains('\t') || arg.contains('"') || arg.is_empty() {
            line.push('"');
            for c in arg.chars() {
                if c == '"' {
                    line.push('\\');
                }
                line.push(c);
            }
            line.push('"');
        } else {
            line.push_str(arg);
        }
    }

    line
}

/// Spawns a process suspended, assigns it to a newly created `KILL_ON_JOB_CLOSE` Job Object,
/// and then resumes the main thread.
///
/// This guarantees zero window of execution outside the Job Object.
pub fn spawn_suspended_in_job(
    program: &Path,
    args: &[String],
    cwd: Option<&Path>,
    env_add: &std::collections::HashMap<String, String>,
    env_remove: &[String],
) -> Result<(u32, PlatformHandle), String> {
    let job_handle = PlatformHandle::new()?;

    let cmd_line = format_windows_command_line(program, args);
    let mut cmd_line_wide = to_wide_null(cmd_line);

    let cwd_wide: Option<Vec<u16>> = cwd.map(to_wide_null);
    let cwd_ptr = cwd_wide.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());

    // Build environment block if overrides exist
    let mut env_block_wide: Option<Vec<u16>> = if !env_add.is_empty() || !env_remove.is_empty() {
        let mut vars: std::collections::BTreeMap<String, String> = std::env::vars().collect();
        for k in env_remove {
            vars.remove(k);
        }
        for (k, v) in env_add {
            vars.insert(k.clone(), v.clone());
        }

        let mut block = Vec::new();
        for (k, v) in vars {
            let entry = format!("{k}={v}");
            block.extend(entry.encode_utf16());
            block.push(0);
        }
        block.push(0); // double null termination
        Some(block)
    } else {
        None
    };

    let env_ptr = env_block_wide.as_mut().map_or(std::ptr::null_mut(), |v| v.as_mut_ptr() as *mut _);

    unsafe {
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;

        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();

        let mut flags = CREATE_SUSPENDED | CREATE_NO_WINDOW;
        if env_ptr != std::ptr::null_mut() {
            flags |= CREATE_UNICODE_ENVIRONMENT;
        }

        let created: BOOL = CreateProcessW(
            std::ptr::null(),
            cmd_line_wide.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0, // bInheritHandles = FALSE
            flags,
            env_ptr,
            cwd_ptr,
            &si,
            &mut pi,
        );

        if created == 0 {
            return Err(format!("CreateProcessW failed (error {})", GetLastError()));
        }

        // Assign before resume
        if let Err(e) = job_handle.assign_process(pi.hProcess) {
            let _ = TerminateProcess(pi.hProcess, 1);
            CloseHandle(pi.hThread);
            CloseHandle(pi.hProcess);
            return Err(format!("Failed to assign suspended process to job: {e}"));
        }

        // Resume thread
        let resume_ret = ResumeThread(pi.hThread);
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);

        if resume_ret == u32::MAX {
            let _ = job_handle.terminate();
            return Err(format!("ResumeThread failed (error {})", GetLastError()));
        }

        Ok((pi.dwProcessId, job_handle))
    }
}

/// Spawns a process with std::process::Command and immediately assigns it to the Job Object.
/// Used when stdio piping is required.
pub fn spawn_command_in_job(
    command: &mut Command,
) -> Result<(std::process::Child, PlatformHandle), String> {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;

    let job_handle = PlatformHandle::new()?;

    // Do not show console window
    command.creation_flags(CREATE_NO_WINDOW);

    let mut child = command
        .spawn()
        .map_err(|e| format!("failed to spawn child process: {e}"))?;

    let raw_handle = child.as_raw_handle() as HANDLE;
    if let Err(e) = job_handle.assign_process(raw_handle) {
        // Child spawned but failed to join the job: kill it before returning
        // so a failed assign can never leak an unsupervised process.
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("Failed to assign spawned child to Job Object: {e}"));
    }

    Ok((child, job_handle))
}
