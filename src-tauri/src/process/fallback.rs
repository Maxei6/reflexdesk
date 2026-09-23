//! Fallback process platform for unsupported operating systems.

#[derive(Debug)]
pub struct PlatformHandle;

impl PlatformHandle {
    pub fn terminate(&self) -> Result<(), String> {
        Err("Process group termination is unsupported on this platform".to_string())
    }

    pub fn identity_label(&self) -> String {
        "unsupported-platform-group".to_string()
    }
}

pub fn process_start_identity(_pid: u32) -> Option<u64> {
    None
}
