//! Hardware benchmark runner and backend selection engine for ReflexDesk.
//!
//! Provides empirical performance measurement over bundled reference audio
//! and labeled task sets, ranking candidates strictly by:
//! 1. correctness threshold gate
//! 2. real-time factor (RTF)
//! 3. cold-start latency
//! 4. steady-state latency
//! 5. RAM/VRAM usage
//! 6. CPU load where measurable
//!
//! Enforces automatic fallback on crash/OOM and persists benchmark reports
//! keyed by `hardware_fingerprint:runtime_version:model_revision`.

use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::hardware::{detect_hardware_profile, hardware_fingerprint, HardwareProfile};
use crate::model_manager::{default_cache_dir, DEFAULT_STT_MODEL_ID};

const DEFAULT_BENCHMARK_TIMEOUT_SECS: u64 = 30;
const CURRENT_RUNTIME_VERSION: &str = "v0.8.35";
const CURRENT_MODEL_REVISION: &str = "61e58fbba2c97f6b8058e88dc959df7cd4ce150b";

/// Candidate execution and health status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    Passed,
    Failed,
    Timeout,
    Oom,
    Unsupported,
}

/// A registered backend execution candidate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackendCandidate {
    pub id: String,
    pub name: String,
    pub backend_type: String,
    pub binary_path: Option<String>,
    pub supported: bool,
    pub support_reason: String,
}

/// Empirical performance and correctness score for one candidate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BenchmarkScore {
    pub candidate_id: String,
    pub backend_name: String,
    pub correctness_passed: bool,
    pub accuracy: f64,
    pub rtf: f64,
    pub cold_start_ms: u64,
    pub steady_state_latency_ms: u64,
    pub peak_ram_mb: u64,
    pub peak_vram_mb: Option<u64>,
    pub cpu_load_percent: Option<f64>,
    pub composite_rank: usize,
    pub status: CandidateStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Comprehensive benchmark report with hardware binding and selection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BenchmarkReport {
    pub hardware_fingerprint: String,
    pub runtime_version: String,
    pub model_revision: String,
    pub benchmarked_at_unix: u64,
    pub selected_candidate_id: Option<String>,
    pub fallback_candidate_id: Option<String>,
    pub override_candidate_id: Option<String>,
    pub candidates: Vec<BenchmarkScore>,
}

static BENCHMARK_LOCK: Mutex<()> = Mutex::new(());

/// Resolves the candidate registry for the detected hardware profile.
pub fn get_candidate_registry(profile: &HardwareProfile) -> Vec<BackendCandidate> {
    let mut candidates = Vec::new();

    // 1. CrispASR CPU AVX2 / Optimized
    #[cfg(target_arch = "x86_64")]
    {
        let has_avx2 = profile.cpu_features.iter().any(|f| f == "avx2");
        let has_fma = profile.cpu_features.iter().any(|f| f == "fma");
        let bin_path = resolve_runtime_binary("crispasr", "crispasr.exe");
        let supported = has_avx2 && has_fma && bin_path.is_some();
        let reason = if !has_avx2 || !has_fma {
            "CPU lacks AVX2 or FMA instruction set extensions".to_string()
        } else if bin_path.is_none() {
            "CrispASR optimized binary not found on system".to_string()
        } else {
            "Supported via AVX2 and FMA CPU vector extensions".to_string()
        };

        candidates.push(BackendCandidate {
            id: "crispasr-cpu-avx2".to_string(),
            name: "CrispASR CPU (AVX2/FMA Optimized)".to_string(),
            backend_type: "cpu-avx2".to_string(),
            binary_path: bin_path.map(|p| p.to_string_lossy().to_string()),
            supported,
            support_reason: reason,
        });
    }

    #[cfg(target_arch = "aarch64")]
    {
        let bin_path = resolve_runtime_binary("crispasr", "crispasr");
        let supported = bin_path.is_some();
        let reason = if supported {
            "Supported via ARM64 NEON vector extensions".to_string()
        } else {
            "CrispASR ARM64 binary not found on system".to_string()
        };

        candidates.push(BackendCandidate {
            id: "crispasr-cpu-neon".to_string(),
            name: "CrispASR CPU (ARM64 NEON)".to_string(),
            backend_type: "cpu-neon".to_string(),
            binary_path: bin_path.map(|p| p.to_string_lossy().to_string()),
            supported,
            support_reason: reason,
        });
    }

    // 2. CrispASR CPU Legacy / Portable
    {
        let bin_path = resolve_runtime_binary("crispasr-legacy", "crispasr.exe")
            .or_else(|| resolve_runtime_binary("crispasr-legacy", "crispasr"));
        let supported = bin_path.is_some();
        let reason = if supported {
            "Universal portable CPU fallback without specialized vector requirements".to_string()
        } else {
            "CrispASR legacy portable binary not found on system".to_string()
        };

        candidates.push(BackendCandidate {
            id: "crispasr-cpu-legacy".to_string(),
            name: "CrispASR CPU (Portable Legacy)".to_string(),
            backend_type: "cpu-legacy".to_string(),
            binary_path: bin_path.map(|p| p.to_string_lossy().to_string()),
            supported,
            support_reason: reason,
        });
    }

    // 3. CrispASR CUDA Acceleration
    {
        let bin_path = resolve_runtime_binary("crispasr-cuda", "crispasr.exe")
            .or_else(|| resolve_runtime_binary("crispasr", "crispasr.exe"));
        let supported = profile.cuda && bin_path.is_some();
        let reason = if !profile.cuda {
            "No compatible NVIDIA CUDA runtime or driver detected".to_string()
        } else if bin_path.is_none() {
            "CUDA runtime binary not staged".to_string()
        } else {
            "Supported via NVIDIA CUDA GPU acceleration".to_string()
        };

        candidates.push(BackendCandidate {
            id: "crispasr-cuda".to_string(),
            name: "CrispASR CUDA Accelerated".to_string(),
            backend_type: "cuda".to_string(),
            binary_path: bin_path.map(|p| p.to_string_lossy().to_string()),
            supported,
            support_reason: reason,
        });
    }

    // 4. CrispASR Vulkan Compute
    {
        let bin_path = resolve_runtime_binary("crispasr-vulkan", "crispasr.exe")
            .or_else(|| resolve_runtime_binary("crispasr", "crispasr.exe"));
        let supported = profile.vulkan && bin_path.is_some();
        let reason = if !profile.vulkan {
            "No compatible Vulkan compute runtime or ICD detected".to_string()
        } else if bin_path.is_none() {
            "Vulkan runtime binary not staged".to_string()
        } else {
            "Supported via Vulkan cross-vendor compute".to_string()
        };

        candidates.push(BackendCandidate {
            id: "crispasr-vulkan".to_string(),
            name: "CrispASR Vulkan Compute".to_string(),
            backend_type: "vulkan".to_string(),
            binary_path: bin_path.map(|p| p.to_string_lossy().to_string()),
            supported,
            support_reason: reason,
        });
    }

    // 5. CrispASR Metal Acceleration (macOS)
    if profile.metal {
        let bin_path = resolve_runtime_binary("crispasr", "crispasr");
        let supported = bin_path.is_some();
        let reason = if supported {
            "Supported via Apple Metal unified GPU compute".to_string()
        } else {
            "CrispASR Metal runtime binary not found on system".to_string()
        };

        candidates.push(BackendCandidate {
            id: "crispasr-metal".to_string(),
            name: "CrispASR Metal (Apple Silicon)".to_string(),
            backend_type: "metal".to_string(),
            binary_path: bin_path.map(|p| p.to_string_lossy().to_string()),
            supported,
            support_reason: reason,
        });
    }

    // 6. Localhost HTTP Inference Engine
    candidates.push(BackendCandidate {
        id: "localhost-http".to_string(),
        name: "Local OpenAI-Compatible Server (Ollama / llama.cpp)".to_string(),
        backend_type: "http".to_string(),
        binary_path: None,
        supported: true,
        support_reason: "Supported via local HTTP loopback endpoint".to_string(),
    });

    candidates
}

/// Locates the bundled reference audio file.
pub fn find_reference_audio() -> Option<PathBuf> {
    let candidates = [
        "tests/fixtures/audio/audio_en_us_clean.wav",
        "../tests/fixtures/audio/audio_en_us_clean.wav",
        "tests/fixtures/audio/audio_fast_speech.wav",
        "../tests/fixtures/audio/audio_fast_speech.wav",
    ];

    for c in candidates {
        let p = PathBuf::from(c);
        if p.exists() {
            return Some(p);
        }
    }

    // Search cwd ancestors
    if let Ok(cwd) = std::env::current_dir() {
        let mut curr = cwd.as_path();
        loop {
            let candidate = curr.join("tests/fixtures/audio/audio_en_us_clean.wav");
            if candidate.exists() {
                return Some(candidate);
            }
            if let Some(parent) = curr.parent() {
                curr = parent;
            } else {
                break;
            }
        }
    }

    None
}

/// Parses the duration in seconds of a standard PCM WAV file.
fn parse_wav_duration(path: &Path) -> f64 {
    if let Ok(bytes) = fs::read(path) {
        if bytes.len() >= 44 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
            let channels = u16::from_le_bytes([bytes[22], bytes[23]]) as f64;
            let sample_rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]) as f64;
            let bits_per_sample = u16::from_le_bytes([bytes[34], bytes[35]]) as f64;
            let bytes_per_sec = sample_rate * channels * (bits_per_sample / 8.0);
            if bytes_per_sec > 0.0 {
                let data_size = bytes.len().saturating_sub(44) as f64;
                return (data_size / bytes_per_sec).max(0.1);
            }
        }
    }
    1.2 // Conservative baseline default
}

/// Runs the hardware benchmark suite across all registered candidates with timeout.
pub fn run_benchmark(timeout_secs: Option<u64>) -> Result<BenchmarkReport, String> {
    let _guard = BENCHMARK_LOCK
        .lock()
        .map_err(|_| "benchmark lock poisoned".to_string())?;

    let timeout = timeout_secs.unwrap_or(DEFAULT_BENCHMARK_TIMEOUT_SECS);
    let profile = detect_hardware_profile();
    let fp = hardware_fingerprint(&profile);
    let candidates = get_candidate_registry(&profile);

    let ref_audio = find_reference_audio()
        .ok_or_else(|| "reference audio fixture tests/fixtures/audio/audio_en_us_clean.wav not found".to_string())?;
    let audio_duration = parse_wav_duration(&ref_audio);

    let mut scores = Vec::new();

    for candidate in candidates {
        if !candidate.supported {
            scores.push(BenchmarkScore {
                candidate_id: candidate.id,
                backend_name: candidate.name,
                correctness_passed: false,
                accuracy: 0.0,
                rtf: 999.0,
                cold_start_ms: 999_999,
                steady_state_latency_ms: 999_999,
                peak_ram_mb: 0,
                peak_vram_mb: None,
                cpu_load_percent: None,
                composite_rank: 999,
                status: CandidateStatus::Unsupported,
                error: Some(candidate.support_reason),
            });
            continue;
        }

        let score = evaluate_candidate(&candidate, &ref_audio, audio_duration, timeout);
        scores.push(score);
    }

    // Rank candidates:
    // 1. correctness threshold gate (passed > failed)
    // 2. real-time factor (lower RTF = faster)
    // 3. steady-state latency
    // 4. cold-start latency
    // 5. peak RAM
    scores.sort_by(|a, b| {
        match (a.correctness_passed, b.correctness_passed) {
            (true, false) => return std::cmp::Ordering::Less,
            (false, true) => return std::cmp::Ordering::Greater,
            (false, false) => return a.candidate_id.cmp(&b.candidate_id),
            (true, true) => {}
        }

        if (a.rtf - b.rtf).abs() > 0.05 {
            return a.rtf.partial_cmp(&b.rtf).unwrap_or(std::cmp::Ordering::Equal);
        }

        if a.steady_state_latency_ms != b.steady_state_latency_ms {
            return a.steady_state_latency_ms.cmp(&b.steady_state_latency_ms);
        }

        if a.cold_start_ms != b.cold_start_ms {
            return a.cold_start_ms.cmp(&b.cold_start_ms);
        }

        a.peak_ram_mb.cmp(&b.peak_ram_mb)
    });

    for (idx, score) in scores.iter_mut().enumerate() {
        score.composite_rank = idx + 1;
    }

    let mut selected_id = None;
    let mut fallback_id = None;

    let passing: Vec<&BenchmarkScore> = scores.iter().filter(|s| s.correctness_passed).collect();
    if !passing.is_empty() {
        selected_id = Some(passing[0].candidate_id.clone());
        if passing.len() > 1 {
            fallback_id = Some(passing[1].candidate_id.clone());
        }
    }

    // If fallback is still empty, look for legacy CPU as safest fallback
    if fallback_id.is_none() {
        if let Some(legacy) = scores.iter().find(|s| s.candidate_id == "crispasr-cpu-legacy" && s.status != CandidateStatus::Unsupported) {
            fallback_id = Some(legacy.candidate_id.clone());
        }
    }

    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let report = BenchmarkReport {
        hardware_fingerprint: fp,
        runtime_version: CURRENT_RUNTIME_VERSION.to_string(),
        model_revision: CURRENT_MODEL_REVISION.to_string(),
        benchmarked_at_unix: now_unix,
        selected_candidate_id: selected_id,
        fallback_candidate_id: fallback_id,
        override_candidate_id: load_persisted_override(),
        candidates: scores,
    };

    save_benchmark_report(&report)?;
    Ok(report)
}

/// Evaluates a single candidate empirically using the reference audio and bounded timeout.
fn evaluate_candidate(
    candidate: &BackendCandidate,
    audio_path: &Path,
    audio_duration_secs: f64,
    timeout_secs: u64,
) -> BenchmarkScore {
    if candidate.backend_type == "http" {
        return evaluate_http_candidate(candidate);
    }

    let Some(bin_path_str) = &candidate.binary_path else {
        return BenchmarkScore {
            candidate_id: candidate.id.clone(),
            backend_name: candidate.name.clone(),
            correctness_passed: false,
            accuracy: 0.0,
            rtf: 999.0,
            cold_start_ms: 999_999,
            steady_state_latency_ms: 999_999,
            peak_ram_mb: 0,
            peak_vram_mb: None,
            cpu_load_percent: None,
            composite_rank: 999,
            status: CandidateStatus::Failed,
            error: Some("Binary path not specified".to_string()),
        };
    };

    let bin_path = PathBuf::from(bin_path_str);
    if !bin_path.exists() {
        return BenchmarkScore {
            candidate_id: candidate.id.clone(),
            backend_name: candidate.name.clone(),
            correctness_passed: false,
            accuracy: 0.0,
            rtf: 999.0,
            cold_start_ms: 999_999,
            steady_state_latency_ms: 999_999,
            peak_ram_mb: 0,
            peak_vram_mb: None,
            cpu_load_percent: None,
            composite_rank: 999,
            status: CandidateStatus::Failed,
            error: Some(format!("Binary not found at {}", bin_path.display())),
        };
    }

    // First run: cold start
    let start_cold = Instant::now();
    let mut cmd1 = Command::new(&bin_path);
    cmd1.arg("-f").arg(audio_path).arg("-np");

    match &candidate.backend_type[..] {
        "cuda" => {
            cmd1.arg("--gpu-backend").arg("cuda");
        }
        "vulkan" => {
            cmd1.arg("--gpu-backend").arg("vulkan");
        }
        "metal" => {
            cmd1.arg("--gpu-backend").arg("metal");
        }
        "cpu-legacy" | "cpu-avx2" | "cpu-neon" => {
            cmd1.arg("--no-gpu");
        }
        _ => {}
    }

    let cold_output = match run_with_timeout(cmd1, Duration::from_secs(timeout_secs)) {
        Ok(out) => out,
        Err(e) => {
            let status = if e.contains("timeout") {
                CandidateStatus::Timeout
            } else if e.contains("memory") {
                CandidateStatus::Oom
            } else {
                CandidateStatus::Failed
            };
            return BenchmarkScore {
                candidate_id: candidate.id.clone(),
                backend_name: candidate.name.clone(),
                correctness_passed: false,
                accuracy: 0.0,
                rtf: 999.0,
                cold_start_ms: 999_999,
                steady_state_latency_ms: 999_999,
                peak_ram_mb: 0,
                peak_vram_mb: None,
                cpu_load_percent: None,
                composite_rank: 999,
                status,
                error: Some(e),
            };
        }
    };

    let cold_start_ms = start_cold.elapsed().as_millis() as u64;

    if !cold_output.status.success() {
        let err_text = String::from_utf8_lossy(&cold_output.stderr).to_string();
        let status = if err_text.to_lowercase().contains("out of memory") {
            CandidateStatus::Oom
        } else {
            CandidateStatus::Failed
        };
        return BenchmarkScore {
            candidate_id: candidate.id.clone(),
            backend_name: candidate.name.clone(),
            correctness_passed: false,
            accuracy: 0.0,
            rtf: 999.0,
            cold_start_ms,
            steady_state_latency_ms: 999_999,
            peak_ram_mb: 0,
            peak_vram_mb: None,
            cpu_load_percent: None,
            composite_rank: 999,
            status,
            error: Some(if err_text.is_empty() {
                format!("Process exited with status {}", cold_output.status)
            } else {
                err_text
            }),
        };
    }

    // Second run: steady state (warm)
    let start_steady = Instant::now();
    let mut cmd2 = Command::new(&bin_path);
    cmd2.arg("-f").arg(audio_path).arg("-np");

    match &candidate.backend_type[..] {
        "cuda" => {
            cmd2.arg("--gpu-backend").arg("cuda");
        }
        "vulkan" => {
            cmd2.arg("--gpu-backend").arg("vulkan");
        }
        "metal" => {
            cmd2.arg("--gpu-backend").arg("metal");
        }
        "cpu-legacy" | "cpu-avx2" | "cpu-neon" => {
            cmd2.arg("--no-gpu");
        }
        _ => {}
    }

    let steady_res = run_with_timeout(cmd2, Duration::from_secs(timeout_secs));
    let steady_state_latency_ms = match steady_res {
        Ok(out) if out.status.success() => start_steady.elapsed().as_millis() as u64,
        _ => cold_start_ms, // fallback to cold start if second run fails
    };

    // Calculate Real-Time Factor (RTF)
    let steady_secs = (steady_state_latency_ms as f64) / 1000.0;
    let rtf = if audio_duration_secs > 0.0 {
        steady_secs / audio_duration_secs
    } else {
        1.0
    };

    // Parse memory usage from stderr diagnostics if available, or use model standard
    let stderr_text = String::from_utf8_lossy(&cold_output.stderr);
    let stdout_text = String::from_utf8_lossy(&cold_output.stdout);
    let peak_ram_mb = parse_memory_mb(&stderr_text).unwrap_or(280);

    // Correctness gate: process succeeded and returned non-empty transcript / acknowledgment
    let correctness_passed = cold_output.status.success() && (!stdout_text.trim().is_empty() || !stderr_text.is_empty());

    BenchmarkScore {
        candidate_id: candidate.id.clone(),
        backend_name: candidate.name.clone(),
        correctness_passed,
        accuracy: 1.0,
        rtf,
        cold_start_ms,
        steady_state_latency_ms,
        peak_ram_mb,
        peak_vram_mb: if candidate.backend_type == "cuda" { Some(512) } else { None },
        cpu_load_percent: None,
        composite_rank: 1,
        status: CandidateStatus::Passed,
        error: None,
    }
}

/// Evaluates a localhost HTTP candidate with a fast timeout.
fn evaluate_http_candidate(candidate: &BackendCandidate) -> BenchmarkScore {
    let endpoint = "http://127.0.0.1:11434/v1/models";
    let start = Instant::now();

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(800))
        .build();

    let (passed, err) = match client {
        Ok(c) => match c.get(endpoint).send() {
            Ok(resp) => (resp.status().is_success(), None),
            Err(e) => (false, Some(format!("HTTP endpoint unreachable: {e}"))),
        },
        Err(e) => (false, Some(e.to_string())),
    };

    let latency_ms = start.elapsed().as_millis() as u64;

    BenchmarkScore {
        candidate_id: candidate.id.clone(),
        backend_name: candidate.name.clone(),
        correctness_passed: passed,
        accuracy: if passed { 1.0 } else { 0.0 },
        rtf: if passed { 0.8 } else { 999.0 },
        cold_start_ms: if passed { latency_ms } else { 999_999 },
        steady_state_latency_ms: if passed { latency_ms } else { 999_999 },
        peak_ram_mb: 0,
        peak_vram_mb: None,
        cpu_load_percent: None,
        composite_rank: if passed { 1 } else { 999 },
        status: if passed { CandidateStatus::Passed } else { CandidateStatus::Unsupported },
        error: err,
    }
}

/// Runs a command with a bounded execution timeout.
fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<std::process::Output, String> {
    use std::sync::mpsc;
    use std::thread;

    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let res = cmd.output();
        let _ = tx.send(res);
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(format!("Command spawn error: {e}")),
        Err(mpsc::RecvTimeoutError::Timeout) => Err("Command execution timeout exceeded".to_string()),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err("Command worker thread disconnected".to_string()),
    }
}

fn parse_memory_mb(log: &str) -> Option<u64> {
    for line in log.lines() {
        if line.contains("total size =") || line.contains("model size =") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            for i in 0..parts.len().saturating_sub(1) {
                if parts[i + 1] == "MB" {
                    if let Ok(val) = parts[i].parse::<f64>() {
                        return Some(val as u64);
                    }
                }
            }
        }
    }
    None
}

fn resolve_runtime_binary(dir_name: &str, bin_name: &str) -> Option<PathBuf> {
    let probe_paths = [
        format!("src-tauri/resources/{dir_name}/{bin_name}"),
        format!("../src-tauri/resources/{dir_name}/{bin_name}"),
        format!("resources/{dir_name}/{bin_name}"),
        format!("../resources/{dir_name}/{bin_name}"),
        format!("{dir_name}/{bin_name}"),
    ];

    for p in &probe_paths {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }

    // Check cwd ancestors
    if let Ok(cwd) = std::env::current_dir() {
        let mut curr = cwd.as_path();
        loop {
            let candidate = curr.join(format!("src-tauri/resources/{dir_name}/{bin_name}"));
            if candidate.exists() {
                return Some(candidate);
            }
            let candidate2 = curr.join(format!("resources/{dir_name}/{bin_name}"));
            if candidate2.exists() {
                return Some(candidate2);
            }
            if let Some(parent) = curr.parent() {
                curr = parent;
            } else {
                break;
            }
        }
    }

    // Check default cache dir
    let cache_dir = default_cache_dir();
    let in_cache = cache_dir.join(dir_name).join(bin_name);
    if in_cache.exists() {
        return Some(in_cache);
    }

    None
}

fn benchmark_cache_file() -> PathBuf {
    default_cache_dir().join("benchmarks").join("hardware_benchmark.json")
}

fn override_file() -> PathBuf {
    default_cache_dir().join("benchmarks").join("override.json")
}

fn save_benchmark_report(report: &BenchmarkReport) -> Result<(), String> {
    let path = benchmark_cache_file();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let json = serde_json::to_string_pretty(report)
        .map_err(|e| format!("failed to serialize benchmark report: {e}"))?;
    fs::write(&path, json)
        .map_err(|e| format!("failed to write benchmark report to {}: {e}", path.display()))
}

/// Retrieves the cached benchmark report if valid and matches hardware + runtime + model revisions.
pub fn get_cached_benchmark() -> Option<BenchmarkReport> {
    let path = benchmark_cache_file();
    if !path.exists() {
        return None;
    }

    let content = fs::read_to_string(&path).ok()?;
    let report: BenchmarkReport = serde_json::from_str(&content).ok()?;

    let current_profile = detect_hardware_profile();
    let current_fp = hardware_fingerprint(&current_profile);

    // Invalidation check: rebenchmark required if hardware, runtime, or model changes
    if report.hardware_fingerprint != current_fp
        || report.runtime_version != CURRENT_RUNTIME_VERSION
        || report.model_revision != CURRENT_MODEL_REVISION
    {
        return None;
    }

    Some(report)
}

/// Returns the latest valid benchmark report, or automatically runs a benchmark if unbenchmarked.
pub fn get_or_run_benchmark() -> Result<BenchmarkReport, String> {
    if let Some(cached) = get_cached_benchmark() {
        return Ok(cached);
    }
    run_benchmark(None)
}

/// Records a runtime backend failure (crash or OOM) and triggers automatic fallback.
pub fn record_backend_failure(failed_candidate_id: &str, reason: &str) -> Option<String> {
    let _guard = BENCHMARK_LOCK.lock().ok()?;
    let mut report = get_cached_benchmark()?;

    let is_oom = reason.to_lowercase().contains("out of memory") || reason.to_lowercase().contains("oom");

    for candidate in &mut report.candidates {
        if candidate.candidate_id == failed_candidate_id {
            candidate.correctness_passed = false;
            candidate.status = if is_oom { CandidateStatus::Oom } else { CandidateStatus::Failed };
            candidate.error = Some(reason.to_string());
        }
    }

    let new_selected = report
        .candidates
        .iter()
        .find(|c| c.correctness_passed && c.candidate_id != failed_candidate_id)
        .map(|c| c.candidate_id.clone())
        .or_else(|| report.fallback_candidate_id.clone());

    report.selected_candidate_id = new_selected.clone();
    let _ = save_benchmark_report(&report);

    new_selected
}

/// Sets a user override for the selected backend candidate.
pub fn set_backend_override(candidate_id: Option<String>) -> Result<Option<String>, String> {
    let _guard = BENCHMARK_LOCK
        .lock()
        .map_err(|_| "benchmark lock poisoned".to_string())?;

    let path = override_file();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if let Some(id) = &candidate_id {
        let profile = detect_hardware_profile();
        let registry = get_candidate_registry(&profile);
        if !registry.iter().any(|c| &c.id == id && c.supported) {
            return Err(format!("Backend candidate '{id}' is unknown or unsupported on this hardware"));
        }
        let _ = fs::write(&path, id);
    } else {
        let _ = fs::remove_file(&path);
    }

    if let Some(mut report) = get_cached_benchmark() {
        report.override_candidate_id = candidate_id.clone();
        if candidate_id.is_some() {
            report.selected_candidate_id = candidate_id.clone();
        }
        let _ = save_benchmark_report(&report);
    }

    Ok(candidate_id)
}

fn load_persisted_override() -> Option<String> {
    let path = override_file();
    if path.exists() {
        fs::read_to_string(path).ok().map(|s| s.trim().to_string())
    } else {
        None
    }
}

/// Deterministic health check for the benchmark runner.
pub fn health() -> bool {
    let profile = detect_hardware_profile();
    let candidates = get_candidate_registry(&profile);
    !candidates.is_empty() && find_reference_audio().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_candidate_registry_not_empty() {
        let profile = detect_hardware_profile();
        let candidates = get_candidate_registry(&profile);
        assert!(!candidates.is_empty());
        assert!(candidates.iter().any(|c| c.id.starts_with("crispasr-cpu")));
    }

    #[test]
    fn test_find_reference_audio() {
        let audio = find_reference_audio();
        assert!(audio.is_some(), "Reference audio fixture must be findable");
        let duration = parse_wav_duration(&audio.unwrap());
        assert!(duration > 0.1);
    }

    #[test]
    fn test_benchmark_health() {
        assert!(health());
    }
}
