//! Plan 12 — Native CrispASR streaming STT seam and baseline comparison.
//!
//! # Upstream C API / FFI Stability Evaluation
//! Upstream CrispASR releases pre-compiled standalone executables (`crispasr.exe`, `crispasr`)
//! containing embedded GGUF inference runners. Direct shared-library C ABI dynamic linking
//! (`libcrispasr.so` / `crispasr.dll`) requires an external C header interface (`crispasr.h`)
//! and native C/C++ compilation toolchains (CMake, MSVC, Clang) across each platform.
//!
//! # Threat Model & Insecure Listener Invariant
//! Upstream CrispASR includes an experimental realtime WebSocket listener. However, as
//! documented in `docs/P0.md` and `docs/THREAT_MODEL.md`, that listener binds to all local
//! and external network interfaces (`0.0.0.0`) without applying bearer token authentication
//! or origin validation. Enabling that upstream realtime WebSocket listener would violate
//! ReflexDesk's core security invariants (offline-first, no unauthenticated local/network listeners).
//!
//! # Seam Architecture
//! ReflexDesk adheres to the preferred target:
//! ```text
//! AudioWorklet
//!   -> Rust audio bridge
//!   -> direct/native realtime session (seam with loopback HTTP production fallback)
//!   -> partial transcript
//!   -> speculative pre-routing (LLM-free fast path)
//!   -> final transcript
//!   -> execute (strictly on verified final transcript)
//! ```
//! In this phase, `NativeSttBackend` serves as the verified seam:
//! - Reports `native_streaming_supported: false`
//! - Refuses unauthenticated network listeners
//! - Retains authenticated loopback HTTP as the active production backend
//! - Tracks baseline latency and Word Error Rate (WER) metrics for future comparison

use super::{SttBackend, SttStatus, Transcript};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;
use tauri::AppHandle;

/// Hardware and protocol baseline metrics comparing the current HTTP path against
/// the native streaming target.
///
/// Measured fields are `None` until reproducibly measured on this machine
/// (AGENTS.md: never claim benchmark numbers until measured). Only live
/// observations via `record_latency` populate `mean_observed_http_latency_ms`;
/// `native_target_speech_end_latency_ms` is a design target, not a measurement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SttBaselineMetrics {
    pub provider: &'static str,
    pub model: &'static str,
    pub http_p50_latency_ms: Option<u64>,
    pub http_p95_latency_ms: Option<u64>,
    pub http_word_error_rate: Option<f32>,
    pub native_target_speech_end_latency_ms: u64,
    pub native_streaming_supported: bool,
    pub insecure_listener_prevented: bool,
    pub sample_rate_hz: u32,
    pub sample_channels: u16,
    pub total_transcriptions_observed: usize,
    pub mean_observed_http_latency_ms: Option<u64>,
}

/// Dynamic tracker for observing live HTTP latencies and maintaining baseline statistics.
pub struct BaselineTracker {
    count: AtomicUsize,
    total_latency_ms: AtomicU64,
    min_latency_ms: AtomicU64,
    max_latency_ms: AtomicU64,
}

impl Default for BaselineTracker {
    fn default() -> Self {
        Self {
            count: AtomicUsize::new(0),
            total_latency_ms: AtomicU64::new(0),
            min_latency_ms: AtomicU64::new(u64::MAX),
            max_latency_ms: AtomicU64::new(0),
        }
    }
}

impl BaselineTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a measured HTTP transcription latency.
    pub fn record_latency(&self, latency_ms: u64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.total_latency_ms
            .fetch_add(latency_ms, Ordering::Relaxed);

        let _ = self
            .min_latency_ms
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
                Some(cur.min(latency_ms))
            });
        let _ = self
            .max_latency_ms
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
                Some(cur.max(latency_ms))
            });
    }

    /// Export baseline metrics snapshot.
    pub fn snapshot(&self) -> SttBaselineMetrics {
        let count = self.count.load(Ordering::Relaxed);
        let total = self.total_latency_ms.load(Ordering::Relaxed);
        let mean = if count > 0 {
            Some(total / (count as u64))
        } else {
            None
        };

        SttBaselineMetrics {
            provider: "nemotron",
            model: "nvidia/nemotron-3.5-asr-streaming-0.6b",
            http_p50_latency_ms: None,
            http_p95_latency_ms: None,
            http_word_error_rate: None,
            native_target_speech_end_latency_ms: 45,
            native_streaming_supported: false,
            insecure_listener_prevented: true,
            sample_rate_hz: 16000,
            sample_channels: 1,
            total_transcriptions_observed: count,
            mean_observed_http_latency_ms: mean,
        }
    }
}

/// Representation of an active native streaming session state machine.
pub struct NativeSession {
    pub session_id: String,
    pub sample_rate: u32,
    pub pcm_buffer: Vec<i16>,
    pub partial_hypotheses: Vec<String>,
    pub created_at: Instant,
    pub last_chunk_at: Instant,
    pub is_active: bool,
}

impl NativeSession {
    pub fn new(session_id: impl Into<String>, sample_rate: u32) -> Self {
        let now = Instant::now();
        Self {
            session_id: session_id.into(),
            sample_rate,
            pcm_buffer: Vec::with_capacity(sample_rate as usize * 4),
            partial_hypotheses: Vec::new(),
            created_at: now,
            last_chunk_at: now,
            is_active: true,
        }
    }

    /// Ingest a streaming PCM audio chunk into this session.
    pub fn push_chunk(&mut self, chunk: &[i16]) {
        self.pcm_buffer.extend_from_slice(chunk);
        self.last_chunk_at = Instant::now();
    }

    /// Number of accumulated audio samples.
    pub fn sample_count(&self) -> usize {
        self.pcm_buffer.len()
    }

    /// Duration of buffered audio in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 {
            0
        } else {
            (self.pcm_buffer.len() as u64 * 1000) / (self.sample_rate as u64)
        }
    }

    /// Reset buffered state.
    pub fn reset(&mut self) {
        self.pcm_buffer.clear();
        self.partial_hypotheses.clear();
        self.last_chunk_at = Instant::now();
    }
}

/// Native CrispASR backend seam.
///
/// Refuses unauthenticated network listeners and returns deterministic failure/fallback
/// state until an authenticated in-process C ABI binding is linked.
pub struct NativeSttBackend {
    sessions: Mutex<std::collections::HashMap<String, NativeSession>>,
    healthy: AtomicBool,
}

impl Default for NativeSttBackend {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(std::collections::HashMap::new()),
            healthy: AtomicBool::new(false),
        }
    }
}

impl NativeSttBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get or create a session for incremental audio ingestion.
    pub fn get_or_create_session(&self, session_id: &str, sample_rate: u32) -> Result<(), String> {
        let mut guard = self
            .sessions
            .lock()
            .map_err(|_| "Native sessions lock poisoned".to_string())?;
        guard
            .entry(session_id.to_string())
            .or_insert_with(|| NativeSession::new(session_id, sample_rate));
        Ok(())
    }

    /// Feed audio samples into the session.
    pub fn feed_pcm(&self, session_id: &str, chunk: &[i16]) -> Result<u64, String> {
        let mut guard = self
            .sessions
            .lock()
            .map_err(|_| "Native sessions lock poisoned".to_string())?;
        if let Some(session) = guard.get_mut(session_id) {
            session.push_chunk(chunk);
            Ok(session.duration_ms())
        } else {
            Err(format!("Native STT session {session_id} not found"))
        }
    }
}

impl SttBackend for NativeSttBackend {
    fn name(&self) -> &'static str {
        "crispasr-native"
    }

    fn health(&self) -> bool {
        self.healthy.load(Ordering::SeqCst)
    }

    fn is_running(&self) -> bool {
        false
    }

    fn native_streaming_supported(&self) -> bool {
        false
    }

    fn start(&self, _app: &AppHandle) -> Result<SttStatus, String> {
        Err(
            "Native CrispASR C API streaming runtime is not bundled; authenticated loopback HTTP backend is the active production engine."
                .to_string(),
        )
    }

    fn transcribe_final(
        &self,
        _app: &AppHandle,
        _samples: &[i16],
        _sample_rate: u32,
        _language: &str,
    ) -> Result<Transcript, String> {
        Err(
            "Native CrispASR C API streaming runtime is unlinked; forward to HTTP backend fallback."
                .to_string(),
        )
    }

    fn cancel_or_reset(&self, session_id: Option<&str>) -> Result<(), String> {
        let mut guard = self
            .sessions
            .lock()
            .map_err(|_| "Native sessions lock poisoned".to_string())?;
        if let Some(id) = session_id {
            guard.remove(id);
        } else {
            guard.clear();
        }
        Ok(())
    }

    fn shutdown(&self) -> Result<(), String> {
        self.cancel_or_reset(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_backend_reports_streaming_unsupported_as_seam() {
        let backend = NativeSttBackend::new();
        assert_eq!(backend.name(), "crispasr-native");
        assert!(!backend.native_streaming_supported());
        assert!(!backend.health());
        assert!(!backend.is_running());
    }

    #[test]
    fn baseline_metrics_record_insecure_listener_prevention() {
        let tracker = BaselineTracker::new();
        let baseline = tracker.snapshot();

        assert_eq!(baseline.provider, "nemotron");
        assert!(baseline.insecure_listener_prevented);
        assert!(!baseline.native_streaming_supported);
        assert_eq!(baseline.sample_rate_hz, 16000);
        assert_eq!(baseline.native_target_speech_end_latency_ms, 45);
        assert!(baseline.http_p50_latency_ms.is_none());
        assert!(baseline.http_p95_latency_ms.is_none());
        assert!(baseline.http_word_error_rate.is_none());
        assert_eq!(baseline.total_transcriptions_observed, 0);
        assert!(baseline.mean_observed_http_latency_ms.is_none());

        tracker.record_latency(150);
        tracker.record_latency(210);
        let updated = tracker.snapshot();
        assert_eq!(updated.total_transcriptions_observed, 2);
        assert_eq!(updated.mean_observed_http_latency_ms, Some(180));
    }

    #[test]
    fn native_session_accumulates_and_resets_pcm() {
        let mut session = NativeSession::new("test_session_1", 16000);
        assert_eq!(session.sample_count(), 0);
        assert_eq!(session.duration_ms(), 0);

        let chunk = vec![0i16; 1600]; // 100ms
        session.push_chunk(&chunk);
        assert_eq!(session.sample_count(), 1600);
        assert_eq!(session.duration_ms(), 100);

        session.push_chunk(&chunk);
        assert_eq!(session.sample_count(), 3200);
        assert_eq!(session.duration_ms(), 200);

        session.reset();
        assert_eq!(session.sample_count(), 0);
        assert_eq!(session.duration_ms(), 0);
    }
}
