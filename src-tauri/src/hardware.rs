//! Hardware detection and profiling for ReflexDesk.
//!
//! Provides empirical hardware capability detection across CPU, RAM, GPU,
//! and compute acceleration runtimes (CUDA, Metal, Vulkan), avoiding
//! hardware-brand heuristics.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Information about a detected GPU device.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GpuInfo {
    pub vendor: String,
    pub device: String,
    pub vram_mb: Option<u64>,
}

/// Comprehensive hardware capability profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HardwareProfile {
    pub os: String,
    pub arch: String,
    pub cpu_features: Vec<String>,
    pub physical_cores: usize,
    pub logical_cores: usize,
    pub ram_mb: u64,
    pub gpu: Option<GpuInfo>,
    pub cuda: bool,
    pub metal: bool,
    pub vulkan: bool,
    pub supported_runtimes: Vec<String>,
}

/// Detects the hardware profile of the current machine empirically.
pub fn detect_hardware_profile() -> HardwareProfile {
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();
    let cpu_features = detect_cpu_features();
    let (physical_cores, logical_cores) = detect_cores();
    let ram_mb = detect_ram_mb();
    let gpu = detect_gpu();
    let cuda = detect_cuda(&gpu);
    let metal = detect_metal();
    let vulkan = detect_vulkan();

    let mut supported_runtimes = Vec::new();

    // CPU-based runtime capabilities
    #[cfg(target_arch = "x86_64")]
    {
        let has_avx2 = cpu_features.iter().any(|f| f == "avx2");
        let has_fma = cpu_features.iter().any(|f| f == "fma");
        if has_avx2 && has_fma {
            supported_runtimes.push("crispasr-cpu-avx2".to_string());
        }
        supported_runtimes.push("crispasr-cpu-legacy".to_string());
    }

    #[cfg(target_arch = "aarch64")]
    {
        supported_runtimes.push("crispasr-cpu-neon".to_string());
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        supported_runtimes.push("crispasr-cpu-generic".to_string());
    }

    // Hardware acceleration runtimes
    if cuda {
        supported_runtimes.push("crispasr-cuda".to_string());
    }
    if metal {
        supported_runtimes.push("crispasr-metal".to_string());
    }
    if vulkan {
        supported_runtimes.push("crispasr-vulkan".to_string());
    }

    // Universal HTTP inference endpoint
    supported_runtimes.push("localhost-http".to_string());

    HardwareProfile {
        os,
        arch,
        cpu_features,
        physical_cores,
        logical_cores,
        ram_mb,
        gpu,
        cuda,
        metal,
        vulkan,
        supported_runtimes,
    }
}

/// Computes a deterministic SHA256 fingerprint for a HardwareProfile.
pub fn hardware_fingerprint(profile: &HardwareProfile) -> String {
    use crate::model_manager::sha256_digest_hex;

    let mut features_sorted = profile.cpu_features.clone();
    features_sorted.sort();

    let mut runtimes_sorted = profile.supported_runtimes.clone();
    runtimes_sorted.sort();

    let gpu_str = match &profile.gpu {
        Some(g) => format!("{}:{}:{:?}", g.vendor, g.device, g.vram_mb),
        None => "none".to_string(),
    };

    let serialized = format!(
        "os={};arch={};cores={}/{};ram={};cpu_features={};gpu={};cuda={};metal={};vulkan={};runtimes={}",
        profile.os,
        profile.arch,
        profile.physical_cores,
        profile.logical_cores,
        profile.ram_mb,
        features_sorted.join(","),
        gpu_str,
        profile.cuda,
        profile.metal,
        profile.vulkan,
        runtimes_sorted.join(","),
    );

    sha256_digest_hex(serialized.as_bytes())
}

/// Deterministic health check for the hardware subsystem.
pub fn health() -> bool {
    let profile = detect_hardware_profile();
    !profile.os.is_empty() && !profile.arch.is_empty() && profile.logical_cores > 0
}

// --- Platform-specific detection helpers ---

fn detect_cpu_features() -> Vec<String> {
    let mut features = Vec::new();

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            features.push("avx2".to_string());
        }
        if is_x86_feature_detected!("fma") {
            features.push("fma".to_string());
        }
        if is_x86_feature_detected!("avx512f") {
            features.push("avx512f".to_string());
        }
        if is_x86_feature_detected!("sse4.2") {
            features.push("sse4.2".to_string());
        }
        if is_x86_feature_detected!("f16c") {
            features.push("f16c".to_string());
        }
        if is_x86_feature_detected!("bmi2") {
            features.push("bmi2".to_string());
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            features.push("neon".to_string());
        }
        if std::arch::is_aarch64_feature_detected!("fp") {
            features.push("fp".to_string());
        }
        if std::arch::is_aarch64_feature_detected!("asimd") {
            features.push("asimd".to_string());
        }
    }

    features
}

fn detect_cores() -> (usize, usize) {
    let logical = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    #[cfg(target_os = "windows")]
    {
        let physical = detect_windows_physical_cores().unwrap_or(logical);
        (physical.max(1), logical)
    }

    #[cfg(target_os = "linux")]
    {
        let physical = detect_linux_physical_cores().unwrap_or(logical);
        (physical.max(1), logical)
    }

    #[cfg(target_os = "macos")]
    {
        let physical = detect_macos_physical_cores().unwrap_or(logical);
        (physical.max(1), logical)
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        (logical, logical)
    }
}

#[cfg(target_os = "windows")]
fn detect_windows_physical_cores() -> Option<usize> {
    #[repr(C)]
    struct SYSTEM_LOGICAL_PROCESSOR_INFORMATION {
        processor_mask: usize,
        relationship: u32,
        payload: [u8; 16],
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLogicalProcessorInformation(
            buffer: *mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION,
            returned_length: *mut u32,
        ) -> i32;
    }

    let mut len: u32 = 0;
    unsafe {
        let _ = GetLogicalProcessorInformation(std::ptr::null_mut(), &mut len);
    }
    if len == 0 {
        return None;
    }

    let item_size = std::mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>() as u32;
    let count = (len / item_size) as usize;
    let mut buffer: Vec<SYSTEM_LOGICAL_PROCESSOR_INFORMATION> = Vec::with_capacity(count);

    let ret = unsafe { GetLogicalProcessorInformation(buffer.as_mut_ptr(), &mut len) };
    if ret == 0 {
        return None;
    }
    unsafe {
        buffer.set_len((len / item_size) as usize);
    }

    let physical = buffer.iter().filter(|info| info.relationship == 0).count();
    if physical > 0 {
        Some(physical)
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
fn detect_linux_physical_cores() -> Option<usize> {
    if let Ok(content) = std::fs::read_to_string("/proc/cpuinfo") {
        let mut core_ids = std::collections::HashSet::new();
        let mut current_phys_id = String::new();

        for line in content.lines() {
            let parts: Vec<&str> = line.split(':').map(|s| s.trim()).collect();
            if parts.len() == 2 {
                if parts[0] == "physical id" {
                    current_phys_id = parts[1].to_string();
                } else if parts[0] == "core id" {
                    core_ids.insert(format!("{}:{}", current_phys_id, parts[1]));
                }
            }
        }
        if !core_ids.is_empty() {
            return Some(core_ids.len());
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn detect_macos_physical_cores() -> Option<usize> {
    let output = std::process::Command::new("sysctl")
        .arg("-n")
        .arg("hw.physicalcpu")
        .output()
        .ok()?;
    if output.status.success() {
        let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
        s.parse::<usize>().ok()
    } else {
        None
    }
}

fn detect_ram_mb() -> u64 {
    #[cfg(target_os = "windows")]
    {
        #[repr(C)]
        struct MemoryStatusEx {
            dw_length: u32,
            dw_memory_load: u32,
            ull_total_phys: u64,
            ull_avail_phys: u64,
            ull_total_page_file: u64,
            ull_avail_page_file: u64,
            ull_total_virtual: u64,
            ull_avail_virtual: u64,
            ull_avail_extended_virtual: u64,
        }

        #[link(name = "kernel32")]
        extern "system" {
            fn GlobalMemoryStatusEx(lpBuffer: *mut MemoryStatusEx) -> i32;
        }

        let mut status = MemoryStatusEx {
            dw_length: std::mem::size_of::<MemoryStatusEx>() as u32,
            dw_memory_load: 0,
            ull_total_phys: 0,
            ull_avail_phys: 0,
            ull_total_page_file: 0,
            ull_avail_page_file: 0,
            ull_total_virtual: 0,
            ull_avail_virtual: 0,
            ull_avail_extended_virtual: 0,
        };

        let ret = unsafe { GlobalMemoryStatusEx(&mut status) };
        if ret != 0 {
            status.ull_total_phys / (1024 * 1024)
        } else {
            0
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
            for line in content.lines() {
                if line.starts_with("MemTotal:") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let Ok(kb) = parts[1].parse::<u64>() {
                            return kb / 1024;
                        }
                    }
                }
            }
        }
        0
    }

    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("sysctl")
            .arg("-n")
            .arg("hw.memsize")
            .output()
            .ok();
        if let Some(out) = output {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if let Ok(bytes) = s.parse::<u64>() {
                    return bytes / (1024 * 1024);
                }
            }
        }
        0
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        0
    }
}

fn detect_gpu() -> Option<GpuInfo> {
    #[cfg(target_os = "windows")]
    {
        detect_windows_gpu()
    }

    #[cfg(target_os = "macos")]
    {
        detect_macos_gpu()
    }

    #[cfg(target_os = "linux")]
    {
        detect_linux_gpu()
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[cfg(target_os = "windows")]
fn detect_windows_gpu() -> Option<GpuInfo> {
    // Probe display devices from the registry under the display class GUID
    for i in 0..16 {
        let subkey = format!("000{i:x}");
        if let Some(info) = query_windows_gpu_subkey(&subkey) {
            // Ignore generic virtual/remote drivers
            if !info.device.contains("Basic Display") && !info.device.contains("Remote Display") {
                return Some(info);
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn query_windows_gpu_subkey(subkey: &str) -> Option<GpuInfo> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            hKey: usize,
            lpSubKey: *const u16,
            ulOptions: u32,
            samDesired: u32,
            phkResult: *mut usize,
        ) -> i32;
        fn RegQueryValueExW(
            hKey: usize,
            lpValueName: *const u16,
            lpReserved: *mut u32,
            lpType: *mut u32,
            lpData: *mut u8,
            lpcbData: *mut u32,
        ) -> i32;
        fn RegCloseKey(hKey: usize) -> i32;
    }

    const HKEY_LOCAL_MACHINE: usize = 0x80000002;
    const KEY_READ: u32 = 0x20019;

    let path = format!(
        "SYSTEM\\CurrentControlSet\\Control\\Class\\{{4d36e968-e325-11ce-bfc1-08002be10318}}\\{subkey}\0"
    );
    let wide_path: Vec<u16> = std::ffi::OsStr::new(&path).encode_wide().collect();

    let mut hkey: usize = 0;
    let res = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_path.as_ptr(),
            0,
            KEY_READ,
            &mut hkey,
        )
    };
    if res != 0 || hkey == 0 {
        return None;
    }

    let read_string = |val_name: &str| -> Option<String> {
        let wide_val: Vec<u16> = std::ffi::OsStr::new(&format!("{val_name}\0"))
            .encode_wide()
            .collect();
        let mut data_len: u32 = 0;
        unsafe {
            let _ = RegQueryValueExW(
                hkey,
                wide_val.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut data_len,
            );
        }
        if data_len == 0 {
            return None;
        }

        let mut buf = vec![0u8; data_len as usize];
        let ret = unsafe {
            RegQueryValueExW(
                hkey,
                wide_val.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                buf.as_mut_ptr(),
                &mut data_len,
            )
        };
        if ret != 0 {
            return None;
        }

        let u16_slice: &[u16] =
            unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u16, (data_len / 2) as usize) };
        let s = String::from_utf16_lossy(u16_slice);
        Some(s.trim_matches('\0').trim().to_string())
    };

    let read_u64_mem = || -> Option<u64> {
        // Check qwMemorySize
        let wide_val: Vec<u16> = std::ffi::OsStr::new("HardwareInformation.qwMemorySize\0")
            .encode_wide()
            .collect();
        let mut data = 0u64;
        let mut data_len = 8u32;
        let ret = unsafe {
            RegQueryValueExW(
                hkey,
                wide_val.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut data as *mut u64 as *mut u8,
                &mut data_len,
            )
        };
        if ret == 0 && data > 0 {
            return Some(data / (1024 * 1024));
        }

        // Fallback to 32-bit MemorySize
        let wide_val32: Vec<u16> = std::ffi::OsStr::new("HardwareInformation.MemorySize\0")
            .encode_wide()
            .collect();
        let mut data32 = 0u32;
        let mut data_len32 = 4u32;
        let ret32 = unsafe {
            RegQueryValueExW(
                hkey,
                wide_val32.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut data32 as *mut u32 as *mut u8,
                &mut data_len32,
            )
        };
        if ret32 == 0 && data32 > 0 {
            return Some((data32 as u64) / (1024 * 1024));
        }

        None
    };

    let device = read_string("DriverDesc")?;
    let vendor = read_string("ProviderName").unwrap_or_else(|| "Unknown".to_string());
    let vram_mb = read_u64_mem();

    unsafe {
        RegCloseKey(hkey);
    }

    Some(GpuInfo {
        vendor,
        device,
        vram_mb,
    })
}

#[cfg(target_os = "macos")]
fn detect_macos_gpu() -> Option<GpuInfo> {
    // Apple Silicon unified GPU or discrete GPU
    if std::env::consts::ARCH == "aarch64" {
        return Some(GpuInfo {
            vendor: "Apple".to_string(),
            device: "Apple Silicon GPU".to_string(),
            vram_mb: None, // Unified memory
        });
    }

    // Intel Mac
    let output = std::process::Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-detailLevel", "mini"])
        .output()
        .ok()?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with("Chipset Model:") {
                let model = line.trim_start_matches("Chipset Model:").trim();
                let vendor = if model.to_lowercase().contains("intel") {
                    "Intel"
                } else if model.to_lowercase().contains("amd") {
                    "AMD"
                } else if model.to_lowercase().contains("nvidia") {
                    "NVIDIA"
                } else {
                    "Unknown"
                };
                return Some(GpuInfo {
                    vendor: vendor.to_string(),
                    device: model.to_string(),
                    vram_mb: None,
                });
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn detect_linux_gpu() -> Option<GpuInfo> {
    // Check for NVIDIA driver details
    if let Ok(content) = std::fs::read_to_string("/proc/driver/nvidia/gpus/0/information") {
        for line in content.lines() {
            if line.starts_with("Model:") {
                let model = line.trim_start_matches("Model:").trim();
                return Some(GpuInfo {
                    vendor: "NVIDIA".to_string(),
                    device: model.to_string(),
                    vram_mb: None,
                });
            }
        }
    }

    // Check lspci fallback
    if let Ok(output) = std::process::Command::new("lspci").output() {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if line.contains("VGA compatible controller") || line.contains("3D controller") {
                    let vendor = if line.contains("NVIDIA") {
                        "NVIDIA"
                    } else if line.contains("Intel") {
                        "Intel"
                    } else if line.contains("Advanced Micro Devices") || line.contains("AMD") {
                        "AMD"
                    } else {
                        "Unknown"
                    };
                    return Some(GpuInfo {
                        vendor: vendor.to_string(),
                        device: line.to_string(),
                        vram_mb: None,
                    });
                }
            }
        }
    }

    None
}

fn detect_cuda(gpu: &Option<GpuInfo>) -> bool {
    #[cfg(target_os = "windows")]
    {
        // 1. Check if NVIDIA driver / CUDA DLL exists in System32
        if Path::new(r"C:\Windows\System32\nvcuda.dll").exists() {
            return true;
        }
        // 2. Check if GPU is NVIDIA and nvidia-smi can run
        if let Some(g) = gpu {
            if g.vendor.to_lowercase().contains("nvidia") {
                if let Ok(out) = std::process::Command::new("nvidia-smi")
                    .arg("--help")
                    .output()
                {
                    return out.status.success();
                }
            }
        }
        false
    }

    #[cfg(target_os = "linux")]
    {
        if Path::new("/usr/local/cuda").exists()
            || Path::new("/usr/lib/x86_64-linux-gnu/libcuda.so.1").exists()
            || Path::new("/usr/lib64/libcuda.so.1").exists()
        {
            return true;
        }
        if let Ok(out) = std::process::Command::new("nvidia-smi")
            .arg("--help")
            .output()
        {
            return out.status.success();
        }
        false
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        false
    }
}

fn detect_metal() -> bool {
    #[cfg(target_os = "macos")]
    {
        // All supported modern macOS targets support Metal
        true
    }

    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

fn detect_vulkan() -> bool {
    #[cfg(target_os = "windows")]
    {
        Path::new(r"C:\Windows\System32\vulkan-1.dll").exists()
    }

    #[cfg(target_os = "linux")]
    {
        Path::new("/usr/lib/x86_64-linux-gnu/libvulkan.so.1").exists()
            || Path::new("/usr/lib64/libvulkan.so.1").exists()
            || Path::new("/usr/lib/libvulkan.so.1").exists()
    }

    #[cfg(target_os = "macos")]
    {
        // MoltenVK presence
        Path::new("/usr/local/lib/libMoltenVK.dylib").exists()
            || Path::new("/opt/homebrew/lib/libMoltenVK.dylib").exists()
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_hardware_profile_non_empty() {
        let profile = detect_hardware_profile();
        assert!(!profile.os.is_empty());
        assert!(!profile.arch.is_empty());
        assert!(profile.logical_cores >= 1);
        assert!(profile.physical_cores >= 1);
        assert!(!profile.supported_runtimes.is_empty());
    }

    #[test]
    fn test_hardware_fingerprint_deterministic() {
        let profile1 = detect_hardware_profile();
        let profile2 = detect_hardware_profile();
        assert_eq!(
            hardware_fingerprint(&profile1),
            hardware_fingerprint(&profile2)
        );
    }

    #[test]
    fn test_hardware_health() {
        assert!(health());
    }
}
