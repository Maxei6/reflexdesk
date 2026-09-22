pub mod native;

use crate::process_supervisor::{OwnershipClass, ProcessId, ProcessSpec, ProcessSupervisor};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    net::TcpListener,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};

const PROVIDER: &str = "nemotron";
const MODEL: &str = "nvidia/nemotron-3.5-asr-streaming-0.6b";

// ---------------------------------------------------------------------------
// Common STT Types
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct LocalEndpoint {
    port: u16,
    token: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SttStatus {
    pub provider: &'static str,
    pub model: &'static str,
    pub runtime_found: bool,
    pub running: bool,
    pub ready: bool,
    pub endpoint: Option<String>,
    pub native_streaming_supported: bool,
    pub active_backend: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Transcript {
    pub text: String,
    pub provider: &'static str,
    pub latency_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunkResult {
    pub session_id: String,
    pub partial_text: Option<String>,
    pub speculative_action: Option<String>,
    pub confidence: Option<f32>,
    pub is_final: bool,
    pub transcript: Option<Transcript>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartialTranscriptEvent {
    pub session_id: String,
    pub text: String,
    pub redacted_text: String,
    pub speculative_action: Option<String>,
    pub confidence: Option<f32>,
    pub is_final: bool,
}

// ---------------------------------------------------------------------------
// SttBackend Trait
// ---------------------------------------------------------------------------

/// Unified speech recognition backend interface.
/// Allows swapping between authenticated HTTP loopback and native C API streaming.
pub trait SttBackend: Send + Sync {
    /// Identifier for this backend (e.g. "http-loopback", "crispasr-native").
    fn name(&self) -> &'static str;

    /// Deterministic health check.
    fn health(&self) -> bool;

    /// Is the backend currently running.
    fn is_running(&self) -> bool;

    /// Does this backend support true in-process native streaming.
    fn native_streaming_supported(&self) -> bool;

    /// Start or prepare the backend engine.
    fn start(&self, app: &AppHandle) -> Result<SttStatus, String>;

    /// Transcribe a final utterance.
    fn transcribe_final(
        &self,
        app: &AppHandle,
        samples: &[i16],
        sample_rate: u32,
        language: &str,
    ) -> Result<Transcript, String>;

    /// Cancel or reset in-flight state.
    fn cancel_or_reset(&self, session_id: Option<&str>) -> Result<(), String>;

    /// Shutdown the backend.
    fn shutdown(&self) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// HTTP Production Backend
// ---------------------------------------------------------------------------

pub struct HttpSttBackend {
    process_id: Mutex<Option<ProcessId>>,
    endpoint: Mutex<Option<LocalEndpoint>>,
    starting: AtomicBool,
    shutting_down: AtomicBool,
}

impl Default for HttpSttBackend {
    fn default() -> Self {
        Self {
            process_id: Mutex::new(None),
            endpoint: Mutex::new(None),
            starting: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
        }
    }
}

impl HttpSttBackend {
    pub fn new() -> Self {
        Self::default()
    }

    fn endpoint_snapshot(&self) -> Option<LocalEndpoint> {
        self.endpoint.lock().ok().and_then(|guard| guard.clone())
    }

    fn child_running(&self) -> bool {
        let Ok(mut guard) = self.process_id.lock() else {
            return false;
        };
        let Some(id) = *guard else {
            return false;
        };

        if ProcessSupervisor::is_alive_global(id) {
            true
        } else {
            *guard = None;
            false
        }
    }

    pub fn is_starting(&self) -> bool {
        self.starting.load(Ordering::SeqCst)
    }
}

impl SttBackend for HttpSttBackend {
    fn name(&self) -> &'static str {
        "http-loopback"
    }

    fn health(&self) -> bool {
        let Some(endpoint) = self.endpoint_snapshot() else {
            return false;
        };

        reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(350))
            .build()
            .ok()
            .and_then(|client| {
                client
                    .get(endpoint_url(&endpoint, "/health"))
                    .send()
                    .ok()
            })
            .filter(|response| response.status().is_success())
            .and_then(|response| response.json::<serde_json::Value>().ok())
            .and_then(|payload| {
                payload
                    .get("backend")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
            })
            .map(|backend| backend == "nemotron")
            .unwrap_or(false)
    }

    fn is_running(&self) -> bool {
        self.child_running()
    }

    fn native_streaming_supported(&self) -> bool {
        false
    }

    fn start(&self, app: &AppHandle) -> Result<SttStatus, String> {
        if self.health() {
            return Ok(status_for_backend(app, self));
        }

        if self.child_running() {
            return Ok(status_for_backend(app, self));
        }
        self.shutdown()?;
        self.shutting_down.store(false, Ordering::SeqCst);
        self.starting.store(true, Ordering::SeqCst);

        let binary = runtime_path(app).ok_or_else(|| {
            self.starting.store(false, Ordering::SeqCst);
            "CrispASR runtime is missing. Reinstall ReflexDesk or repair the installation."
                .to_string()
        })?;

        let endpoint = allocate_endpoint().map_err(|e| {
            self.starting.store(false, Ordering::SeqCst);
            e
        })?;
        let threads = std::thread::available_parallelism()
            .map(|n| n.get().min(8).max(2))
            .unwrap_or(4);

        let runtime_dir = binary
            .parent()
            .ok_or_else(|| {
                self.starting.store(false, Ordering::SeqCst);
                "invalid CrispASR runtime path".to_string()
            })?
            .to_path_buf();

        let port = endpoint.port.to_string();
        let thread_count = threads.to_string();

        // Pre-spawn check: abort if shutdown was initiated
        if self.shutting_down.load(Ordering::SeqCst) {
            self.starting.store(false, Ordering::SeqCst);
            return Err("ReflexDesk STT start cancelled: shutting down".to_string());
        }

        let supervisor = app.state::<ProcessSupervisor>();
        let spec = ProcessSpec::new(&binary, OwnershipClass::Internal)
            .cwd(&runtime_dir)
            .args([
                "--server",
                "--host",
                "127.0.0.1",
                "--port",
                &port,
                "--backend",
                "nemotron",
                "-m",
                "auto",
                "--auto-download",
                "-l",
                "auto",
                "-t",
                &thread_count,
            ])
            .env("CRISPASR_API_KEYS", &endpoint.token)
            .env("CRISPASR_NEMOTRON_CONTEXT_PRESET", "0")
            .env("CRISPASR_NEMOTRON_STREAMING", "1")
            .with_piped_stdio(true);

        let proc_id = match supervisor.spawn(spec) {
            Ok(id) => id,
            Err(e) => {
                self.starting.store(false, Ordering::SeqCst);
                return Err(format!("failed to start local speech engine: {e}"));
            }
        };

        // Post-spawn check: abort if shutdown occurred during spawn
        if self.shutting_down.load(Ordering::SeqCst) {
            self.starting.store(false, Ordering::SeqCst);
            let _ = supervisor.terminate_owned(proc_id);
            return Err("ReflexDesk STT start aborted: shutting down".to_string());
        }

        if let Some((stdout, stderr)) = supervisor.take_stdio(proc_id) {
            if let Some(stdout) = stdout {
                forward_logs(stdout, app.clone(), "stdout");
            }
            if let Some(stderr) = stderr {
                forward_logs(stderr, app.clone(), "stderr");
            }
        }

        *self
            .endpoint
            .lock()
            .map_err(|_| "STT endpoint lock poisoned".to_string())? = Some(endpoint);

        *self
            .process_id
            .lock()
            .map_err(|_| "STT process lock poisoned".to_string())? = Some(proc_id);

        self.starting.store(false, Ordering::SeqCst);
        let _ = app.emit(
            "reflexdesk://stt-status",
            serde_json::json!({
                "stream": "runtime",
                "message": "Preparing NVIDIA Nemotron 3.5 locally. First setup may download the speech model."
            }),
        );

        Ok(status_for_backend(app, self))
    }

    fn transcribe_final(
        &self,
        app: &AppHandle,
        samples: &[i16],
        sample_rate: u32,
        language: &str,
    ) -> Result<Transcript, String> {
        if samples.len() < (sample_rate as usize / 10) {
            return Err("speech segment is too short".into());
        }
        if samples.len() > (sample_rate as usize * 30) {
            return Err("speech segment exceeds the 30 second command limit".into());
        }

        if !self.health() {
            let _ = self.start(app)?;
            if !self.health() {
                return Err("speech engine is still preparing".into());
            }
        }

        let endpoint = self.endpoint_snapshot().ok_or("local speech endpoint is unavailable")?;
        let started = Instant::now();
        let wav = wav_bytes(samples, sample_rate);

        let part = reqwest::blocking::multipart::Part::bytes(wav)
            .file_name("reflexdesk-command.wav")
            .mime_str("audio/wav")
            .map_err(|e| e.to_string())?;

        let mut form = reqwest::blocking::multipart::Form::new()
            .part("file", part)
            .text("response_format", "json");

        let lang = language.trim();
        if !lang.is_empty() && lang != "auto" {
            form = form.text("language", lang.to_string());
        }

        let response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| e.to_string())?
            .post(endpoint_url(&endpoint, "/v1/audio/transcriptions"))
            .bearer_auth(&endpoint.token)
            .multipart(form)
            .send()
            .map_err(|e| format!("local speech request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("local speech engine returned HTTP {}", response.status()));
        }

        let payload = response
            .json::<serde_json::Value>()
            .map_err(|e| format!("invalid local STT response: {e}"))?;

        let text = payload
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        if text.is_empty() {
            return Err("speech engine returned an empty transcript".into());
        }

        Ok(Transcript {
            text,
            provider: PROVIDER,
            latency_ms: started.elapsed().as_millis(),
        })
    }

    fn cancel_or_reset(&self, _session_id: Option<&str>) -> Result<(), String> {
        Ok(())
    }

    fn shutdown(&self) -> Result<(), String> {
        self.shutting_down.store(true, Ordering::SeqCst);

        if let Ok(mut guard) = self.process_id.lock() {
            if let Some(id) = guard.take() {
                ProcessSupervisor::terminate_process_global(id);
            }
        }

        *self
            .endpoint
            .lock()
            .map_err(|_| "STT endpoint lock poisoned".to_string())? = None;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Streaming Session Management
// ---------------------------------------------------------------------------

struct StreamSession {
    session_id: String,
    samples: Vec<i16>,
    sample_rate: u32,
    language: String,
    last_partial: Option<String>,
    created_at: Instant,
    last_activity: Instant,
}

impl StreamSession {
    fn new(session_id: &str, sample_rate: u32, language: &str) -> Self {
        let now = Instant::now();
        Self {
            session_id: session_id.to_string(),
            samples: Vec::with_capacity(sample_rate as usize * 4),
            sample_rate,
            language: language.to_string(),
            last_partial: None,
            created_at: now,
            last_activity: now,
        }
    }
}

// ---------------------------------------------------------------------------
// SttState
// ---------------------------------------------------------------------------

pub struct SttState {
    http: Arc<HttpSttBackend>,
    native: Arc<native::NativeSttBackend>,
    active_backend: Mutex<String>,
    sessions: Mutex<HashMap<String, StreamSession>>,
    tracker: Arc<native::BaselineTracker>,
}

impl Default for SttState {
    fn default() -> Self {
        Self {
            http: Arc::new(HttpSttBackend::new()),
            native: Arc::new(native::NativeSttBackend::new()),
            active_backend: Mutex::new("http".to_string()),
            sessions: Mutex::new(HashMap::new()),
            tracker: Arc::new(native::BaselineTracker::new()),
        }
    }
}

impl Drop for SttState {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

impl SttState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tracker(&self) -> &Arc<native::BaselineTracker> {
        &self.tracker
    }

    pub fn shutdown(&self) -> Result<(), String> {
        let _ = self.http.shutdown();
        let _ = self.native.shutdown();
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.clear();
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn preferred_runtime_dir() -> &'static str {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        if !(std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("fma"))
        {
            return "crispasr-legacy";
        }
        return "crispasr";
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        if !(std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("fma"))
        {
            return "crispasr-legacy";
        }
        return "crispasr";
    }

    #[cfg(not(all(
        any(target_os = "windows", target_os = "linux"),
        target_arch = "x86_64"
    )))]
    {
        "crispasr"
    }
}

fn runtime_binary_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "crispasr.exe"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "crispasr"
    }
}

fn runtime_candidates(app: &AppHandle) -> Vec<PathBuf> {
    let dir = preferred_runtime_dir();
    let name = runtime_binary_name();
    let mut candidates = Vec::new();

    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(resource_dir.join(dir).join(name));
        candidates.push(resource_dir.join("resources").join(dir).join(name));
    }

    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join(dir)
            .join(name),
    );

    candidates
}

fn runtime_path(app: &AppHandle) -> Option<PathBuf> {
    runtime_candidates(app).into_iter().find(|path| path.is_file())
}

fn endpoint_url(endpoint: &LocalEndpoint, path: &str) -> String {
    format!("http://127.0.0.1:{}{}", endpoint.port, path)
}

fn allocate_endpoint() -> Result<LocalEndpoint, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("could not reserve local STT port: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    drop(listener);

    let token = format!(
        "{:032x}{:032x}",
        rand::random::<u128>(),
        rand::random::<u128>()
    );

    Ok(LocalEndpoint { port, token })
}

fn status_for_backend(app: &AppHandle, backend: &HttpSttBackend) -> SttStatus {
    let endpoint = backend.endpoint_snapshot();
    let model_ready = app
        .try_state::<crate::model_manager::ModelManager>()
        .map(|mgr| mgr.is_ready_for_engine(crate::model_manager::DEFAULT_STT_MODEL_ID))
        .unwrap_or(false);

    SttStatus {
        provider: PROVIDER,
        model: MODEL,
        runtime_found: runtime_path(app).is_some(),
        running: backend.child_running(),
        ready: backend.health() && model_ready,
        endpoint: endpoint.map(|value| format!("127.0.0.1:{}", value.port)),
        native_streaming_supported: false,
        active_backend: "http".to_string(),
    }
}

fn forward_logs<R: std::io::Read + Send + 'static>(
    reader: R,
    app: AppHandle,
    stream: &'static str,
) {
    thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            let lower = line.to_lowercase();
            if lower.contains("nemotron")
                || lower.contains("download")
                || lower.contains("model")
                || lower.contains("server")
                || lower.contains("error")
                || lower.contains("listen")
                || lower.contains("cache")
            {
                // Redact line before emitting to frontend log sink (Plan14 requirement)
                let redacted = crate::redaction::redact_error(&line);
                let _ = app.emit(
                    "reflexdesk://stt-status",
                    serde_json::json!({ "stream": stream, "message": redacted }),
                );
            }
        }
    });
}

fn wav_bytes(samples: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);

    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());

    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }

    out
}

// ---------------------------------------------------------------------------
// Public API Functions
// ---------------------------------------------------------------------------

pub fn status(app: &AppHandle, state: &SttState) -> SttStatus {
    status_for_backend(app, &state.http)
}

pub fn start(app: &AppHandle, state: &SttState) -> Result<SttStatus, String> {
    state.http.start(app)
}

pub fn transcribe(
    app: &AppHandle,
    state: &SttState,
    samples: Vec<i16>,
    sample_rate: u32,
    language: String,
) -> Result<Transcript, String> {
    let transcript = state
        .http
        .transcribe_final(app, &samples, sample_rate, &language)?;
    state.tracker.record_latency(transcript.latency_ms as u64);
    Ok(transcript)
}

/// Ingest a streaming PCM audio chunk from AudioWorklet.
///
/// Gates intermediate chunks through `TranscriptGate::is_nonce_valid`.
/// If partial text is provided or extracted, applies `redact_text`, runs
/// speculative pre-routing in `ReflexEngine` (warming the reflex cache),
/// and emits `reflexdesk://stt-partial`.
///
/// If `is_final` is true, executes final transcription through the production
/// backend and returns the complete `Transcript`. Execution is never performed
/// here: the overlay must submit the final transcript via `submit_transcript`.
pub fn stream_chunk(
    app: &AppHandle,
    state: &SttState,
    session_id: String,
    nonce: String,
    samples: Vec<i16>,
    sample_rate: u32,
    language: String,
    partial_hint: Option<String>,
    is_final: bool,
) -> Result<StreamChunkResult, String> {
    // 1. Validate session nonce (fails closed if nonce is missing or expired)
    let gate = app.state::<crate::transcript::TranscriptGate>();
    if !gate.is_nonce_valid(&nonce) {
        return Err("invalid-transcript-nonce".into());
    }

    // 2. Bounds check on input chunk (prevent unbounded memory consumption)
    if samples.len() > (sample_rate as usize * 30) {
        return Err("audio chunk exceeds 30s limit".into());
    }

    // 3. Update session buffer
    let mut accumulated = Vec::new();
    {
        let mut guard = state
            .sessions
            .lock()
            .map_err(|_| "STT sessions lock poisoned".to_string())?;

        let session = guard
            .entry(session_id.clone())
            .or_insert_with(|| StreamSession::new(&session_id, sample_rate, &language));

        session.samples.extend_from_slice(&samples);
        session.last_activity = Instant::now();

        if session.samples.len() > (sample_rate as usize * 30) {
            return Err("total session audio exceeds 30s limit".into());
        }

        if is_final {
            accumulated = std::mem::take(&mut session.samples);
        }
    }

    // 4. Speculative Pre-Routing on Partial Text
    let mut speculative_action = None;
    let mut speculative_confidence = None;

    if let Some(text) = partial_hint.filter(|t| !t.trim().is_empty()) {
        let clean = text.trim();
        if clean.len() <= crate::transcript::MAX_TRANSCRIPT_CHARS && !clean.contains('\0') {
            let redacted = crate::redaction::redact_text(clean).into_owned();

            // Run speculative pre-routing in ReflexEngine (if available) to warm the cache.
            // Fast path is kept LLM-free: Tier-0 deterministic routing only.
            if !crate::policy::is_negated_command(clean) {
                if let Some(engine) = app.try_state::<crate::reflex::ReflexEngine>() {
                    let ctx = crate::reflex::ReflexContext::new(
                        clean,
                        &session_id,
                        None,
                        Some(&language),
                    );
                    let decision = engine.route_command(&ctx, "deterministic");
                    speculative_action = decision.action;
                    speculative_confidence = Some(decision.confidence);
                }
            }

            let _ = app.emit(
                "reflexdesk://stt-partial",
                PartialTranscriptEvent {
                    session_id: session_id.clone(),
                    text: clean.to_string(),
                    redacted_text: redacted,
                    speculative_action: speculative_action.clone(),
                    confidence: speculative_confidence,
                    is_final: false,
                },
            );
        }
    }

    // 5. If final, run final transcription via production backend
    if is_final {
        let samples_to_transcribe = if accumulated.is_empty() {
            samples
        } else {
            accumulated
        };

        let transcript = state
            .http
            .transcribe_final(app, &samples_to_transcribe, sample_rate, &language)?;

        state.tracker.record_latency(transcript.latency_ms as u64);

        // Remove session from map
        if let Ok(mut guard) = state.sessions.lock() {
            guard.remove(&session_id);
        }

        Ok(StreamChunkResult {
            session_id,
            partial_text: None,
            speculative_action,
            confidence: speculative_confidence,
            is_final: true,
            transcript: Some(transcript),
        })
    } else {
        Ok(StreamChunkResult {
            session_id,
            partial_text: None,
            speculative_action,
            confidence: speculative_confidence,
            is_final: false,
            transcript: None,
        })
    }
}

/// Cancel and reset in-flight streaming session state (e.g. on mic disconnect or stop).
pub fn cancel_stream(
    app: &AppHandle,
    state: &SttState,
    session_id: &str,
) -> Result<(), String> {
    if let Ok(mut guard) = state.sessions.lock() {
        guard.remove(session_id);
    }
    let _ = state.native.cancel_or_reset(Some(session_id));
    let _ = app.emit(
        "reflexdesk://stt-cancelled",
        serde_json::json!({ "session_id": session_id }),
    );
    Ok(())
}

pub fn baseline_metrics(state: &SttState) -> native::SttBaselineMetrics {
    state.tracker.snapshot()
}

pub fn shutdown(state: &SttState) -> Result<(), String> {
    state.shutdown()
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_backend_reports_name_and_streaming_status() {
        let backend = HttpSttBackend::new();
        assert_eq!(backend.name(), "http-loopback");
        assert!(!backend.native_streaming_supported());
        assert!(!backend.health());
        assert!(!backend.is_running());
    }

    #[test]
    fn wav_header_format_is_valid() {
        let samples = vec![0i16; 1600]; // 100ms at 16kHz
        let bytes = wav_bytes(&samples, 16000);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(&bytes[36..40], b"data");
        // 44 header bytes + 3200 data bytes = 3244
        assert_eq!(bytes.len(), 44 + 3200);
    }

    #[test]
    fn stt_state_lifecycle() {
        let state = SttState::new();
        assert_eq!(state.tracker().snapshot().provider, "nemotron");
        assert!(state.shutdown().is_ok());
    }
}
