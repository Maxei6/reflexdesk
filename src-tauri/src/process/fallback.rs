//! Fallback process platform for unsupported operating systems.

#[derive(Debug)]
pub struct PlatformHandle;

impl PlatformHandle {
    pub fn terminate(&self) -> Result<(), String> {
        Err("Process group termination is unsupported on this platform".to_string())
    }
}
