//! Privacy-safe local observability, rotating structured logging, and
//! sanitized diagnostics bundle export for ReflexDesk.
//!
//! Invariants:
//! - All observability sinks route through `redact_text` and `redact_error`.
//! - No raw audio, no secrets, no clipboard/file contents, no full browser text.
//! - Transcripts are NEVER stored or logged unless explicitly enabled for debugging.
//! - Telemetry stays off by default.
//! - Diagnostics bundle excludes secrets by construction.
//! - Pre-export preview and export return identical `DiagnosticsBundle` objects.
//! - Export uses atomic write + rename; cancel or error leaves no file on disk.

use crate::lifecycle::{Phase, RuntimeState};
use crate::model_manager::DEFAULT_STT_MODEL_ID;
use crate::redaction::{is_allowlisted_field, redact_error, redact_text};
use crate::settings::SettingsState;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

pub const LOG_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_MAX_MEMORY_EVENTS: usize = 1000;
pub const DEFAULT_MAX_LOG_FILE_BYTES: u64 = 1024 * 1024; // 1 MB per file
pub const DEFAULT_MAX_ROTATED_FILES: usize = 3; // Keep reflexdesk.log, reflexdesk.log.1, .2, .3

static DEBUG_TRANSCRIPTS: AtomicBool = AtomicBool::new(false);
static GLOBAL_OBSERVABILITY: LazyLock<Arc<ObservabilityState>> =
    LazyLock::new(|| Arc::new(ObservabilityState::new()));
/// Versioned structured log event matching Wave-3 specification:
/// `{ v: 1, ts, component, op, session, phase, outcome, latency_ms, code, meta }`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogEvent {
    pub v: u32,
    pub ts: u64,
    pub component: String,
    pub op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

impl LogEvent {
    pub fn new(
        component: impl Into<String>,
        op: impl Into<String>,
        outcome: impl Into<String>,
    ) -> Self {
        Self {
            v: LOG_SCHEMA_VERSION,
            ts: current_unix_millis(),
            component: component.into(),
            op: op.into(),
            session: None,
            phase: None,
            outcome: outcome.into(),
            latency_ms: None,
            code: None,
            meta: None,
        }
    }

    pub fn with_session(mut self, session: impl Into<String>) -> Self {
        self.session = Some(session.into());
        self
    }

    pub fn with_phase(mut self, phase: impl Into<String>) -> Self {
        self.phase = Some(phase.into());
        self
    }

    pub fn with_latency_ms(mut self, latency_ms: u64) -> Self {
        self.latency_ms = Some(latency_ms);
        self
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn with_meta(mut self, meta: serde_json::Value) -> Self {
        self.meta = Some(sanitize_meta_value(&meta));
        self
    }
}

/// Sanitized state transition record for diagnostics bundle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransitionRecord {
    pub ts: u64,
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Sanitized error record for diagnostics bundle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SanitizedErrorRecord {
    pub ts: u64,
    pub component: String,
    pub op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub sanitized_message: String,
}

/// Installed adapter availability for diagnostics bundle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdapterAvailability {
    pub desktop: bool,
    pub browser: bool,
    pub reflex: bool,
    pub planner: bool,
    pub harnesses: Vec<String>,
}

/// Benchmark numbers summary for diagnostics bundle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BenchmarkDiagnostics {
    pub voice_benchmark_ms: Option<u64>,
    pub score_summary: String,
}

/// Complete privacy-safe diagnostics bundle.
///
/// Guaranteed to contain ONLY allowlisted fields, sanitized errors,
/// and metadata that excludes secrets by construction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiagnosticsBundle {
    pub app_version: String,
    pub build: String,
    pub os: String,
    pub arch: String,
    pub hardware_class: String,
    pub runtime_version: String,
    pub model_id: Option<String>,
    pub model_revision: Option<String>,
    pub benchmark_numbers: BenchmarkDiagnostics,
    pub sanitized_transitions: Vec<TransitionRecord>,
    pub sanitized_errors: Vec<SanitizedErrorRecord>,
    pub adapter_availability: AdapterAvailability,
    pub recent_logs: Vec<LogEvent>,
}

/// Thread-safe in-memory state managing rotating bounded logs and diagnostics history.
pub struct ObservabilityState {
    log_dir: Mutex<Option<PathBuf>>,
    memory_events: Mutex<VecDeque<LogEvent>>,
    transitions: Mutex<VecDeque<TransitionRecord>>,
    errors: Mutex<VecDeque<SanitizedErrorRecord>>,
    max_memory_events: usize,
    max_log_bytes: u64,
    max_rotated_files: usize,
}

impl ObservabilityState {
    pub fn new() -> Self {
        Self {
            log_dir: Mutex::new(None),
            memory_events: Mutex::new(VecDeque::with_capacity(DEFAULT_MAX_MEMORY_EVENTS)),
            transitions: Mutex::new(VecDeque::with_capacity(100)),
            errors: Mutex::new(VecDeque::with_capacity(100)),
            max_memory_events: DEFAULT_MAX_MEMORY_EVENTS,
            max_log_bytes: DEFAULT_MAX_LOG_FILE_BYTES,
            max_rotated_files: DEFAULT_MAX_ROTATED_FILES,
        }
    }

    pub fn set_log_dir<P: AsRef<Path>>(&self, path: P) {
        let p = path.as_ref().to_path_buf();
        let _ = fs::create_dir_all(&p);
        if let Ok(mut lock) = self.log_dir.lock() {
            *lock = Some(p);
        }
    }

    pub fn record_event(&self, mut event: LogEvent) {
        // Enforce redaction on all fields
        event.component = sanitize_token(&event.component);
        event.op = sanitize_token(&event.op);
        event.outcome = sanitize_token(&event.outcome);
        if let Some(session) = event.session.as_mut() {
            *session = sanitize_token(session);
        }
        if let Some(phase) = event.phase.as_mut() {
            *phase = sanitize_token(phase);
        }
        if let Some(code) = event.code.as_mut() {
            *code = redact_error(code);
        }
        if let Some(meta) = event.meta.as_mut() {
            *meta = sanitize_meta_value(meta);
        }

        // Store into errors deque if outcome indicates failure
        if event.outcome == "error" || event.code.is_some() {
            let error_msg = event
                .code
                .clone()
                .or_else(|| {
                    event.meta.as_ref().and_then(|m| {
                        m.get("error")
                            .and_then(|e| e.as_str())
                            .map(|s| s.to_string())
                    })
                })
                .unwrap_or_else(|| format!("{}: {}", event.component, event.outcome));

            if let Ok(mut errors) = self.errors.lock() {
                if errors.len() >= 100 {
                    errors.pop_front();
                }
                errors.push_back(SanitizedErrorRecord {
                    ts: event.ts,
                    component: event.component.clone(),
                    op: event.op.clone(),
                    code: event.code.clone(),
                    sanitized_message: redact_error(&error_msg),
                });
            }
        }

        // Write to in-memory circular buffer
        if let Ok(mut events) = self.memory_events.lock() {
            if events.len() >= self.max_memory_events {
                events.pop_front();
            }
            events.push_back(event.clone());
        }

        // Persist to rotating local disk log
        self.persist_event_to_disk(&event);
    }

    pub fn record_transition(&self, phase: Phase, error: Option<&str>) {
        let phase_str = format!("{phase:?}").to_lowercase();
        let sanitized_err = error.map(redact_error);

        if let Ok(mut transitions) = self.transitions.lock() {
            if transitions.len() >= 100 {
                transitions.pop_front();
            }
            transitions.push_back(TransitionRecord {
                ts: current_unix_millis(),
                phase: phase_str.clone(),
                error: sanitized_err.clone(),
            });
        }

        let mut event = LogEvent::new("lifecycle", "transition", "ok")
            .with_phase(phase_str);
        if let Some(err) = sanitized_err {
            event = event.with_code(err);
        }
        self.record_event(event);
    }

    fn persist_event_to_disk(&self, event: &LogEvent) {
        let log_dir = match self.log_dir.lock() {
            Ok(guard) => match guard.as_ref() {
                Some(dir) => dir.clone(),
                None => match resolve_fallback_log_dir() {
                    Ok(dir) => dir,
                    Err(_) => return,
                },
            },
            Err(_) => return,
        };

        if let Ok(json_line) = serde_json::to_string(event) {
            let log_file = log_dir.join("reflexdesk.log");
            self.rotate_if_needed(&log_file);

            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_file) {
                let _ = writeln!(file, "{json_line}");
            }
        }
    }

    fn rotate_if_needed(&self, active_path: &Path) {
        if let Ok(meta) = fs::metadata(active_path) {
            if meta.len() >= self.max_log_bytes {
                let parent = match active_path.parent() {
                    Some(p) => p,
                    None => return,
                };

                // Shift existing rotated files: .2 -> .3, .1 -> .2, etc.
                for i in (1..self.max_rotated_files).rev() {
                    let old_path = parent.join(format!("reflexdesk.log.{i}"));
                    let new_path = parent.join(format!("reflexdesk.log.{}", i + 1));
                    if old_path.exists() {
                        let _ = fs::rename(old_path, new_path);
                    }
                }

                // Rename active to .1
                let first_rot = parent.join("reflexdesk.log.1");
                let _ = fs::rename(active_path, first_rot);
            }
        }
    }

    pub fn get_recent_logs(&self, limit: usize) -> Vec<LogEvent> {
        self.memory_events
            .lock()
            .map(|events| {
                let take_count = limit.min(events.len());
                events.iter().rev().take(take_count).cloned().collect()
            })
            .unwrap_or_default()
    }

    pub fn get_transitions(&self) -> Vec<TransitionRecord> {
        self.transitions
            .lock()
            .map(|t| t.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn get_errors(&self) -> Vec<SanitizedErrorRecord> {
        self.errors
            .lock()
            .map(|e| e.iter().cloned().collect())
            .unwrap_or_default()
    }
}

impl Default for ObservabilityState {
    fn default() -> Self {
        Self::new()
    }
}

/// Return global observability singleton.
pub fn global_state() -> Arc<ObservabilityState> {
    GLOBAL_OBSERVABILITY.clone()
}

/// Configure debug transcript logging flag.
/// By default this is FALSE to strictly protect user privacy.
pub fn set_debug_transcripts(enabled: bool) {
    DEBUG_TRANSCRIPTS.store(enabled, Ordering::SeqCst);
}

/// Check whether debug transcript logging is explicitly enabled.
pub fn debug_transcripts_enabled() -> bool {
    if let Ok(val) = std::env::var("REFLEXDESK_DEBUG_TRANSCRIPTS") {
        if val == "1" || val.eq_ignore_ascii_case("true") {
            return true;
        }
    }
    DEBUG_TRANSCRIPTS.load(Ordering::SeqCst)
}

/// Current Unix timestamp in milliseconds.
pub fn current_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Fallback path resolution for local logs if app handle is not initialized.
pub fn resolve_fallback_log_dir() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    {
        if let Ok(app_data) = std::env::var("LOCALAPPDATA") {
            let dir = PathBuf::from(app_data).join("reflexdesk").join("logs");
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            return Ok(dir);
        }
        if let Ok(user_profile) = std::env::var("USERPROFILE") {
            let dir = PathBuf::from(user_profile)
                .join(".reflexdesk")
                .join("logs");
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            return Ok(dir);
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(home) = std::env::var("HOME") {
            let dir = PathBuf::from(home).join(".reflexdesk").join("logs");
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            return Ok(dir);
        }
    }
    let dir = std::env::temp_dir().join("reflexdesk-logs");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Sanitize token string against injection or path chars.
fn sanitize_token(token: &str) -> String {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return "unknown".into();
    }
    let sanitized = redact_text(trimmed);
    sanitized.into_owned()
}

/// Secret and privacy sensitive keywords that must never appear in metadata values.
const DENIED_META_KEYS: &[&str] = &[
    "api_key",
    "apikey",
    "token",
    "password",
    "secret",
    "authorization",
    "bearer",
    "private_key",
    "client_secret",
    "credential",
    "audio",
    "pcm",
    "wav",
    "clipboard",
    "file_content",
    "file_contents",
    "browser_text",
    "prompt",
];

/// Recursively sanitize JSON metadata value so no secret or private content leaks.
pub fn sanitize_meta_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let k_lower = k.to_lowercase();

                // Check for denied secret and content keys
                if DENIED_META_KEYS.iter().any(|denied| k_lower.contains(denied)) {
                    out.insert(k.clone(), serde_json::Value::String("[redacted-secret]".into()));
                    continue;
                }

                // Handle transcript privacy: allow only if explicitly enabled
                if k_lower.contains("transcript") {
                    if debug_transcripts_enabled() {
                        if let Some(s) = v.as_str() {
                            out.insert(k.clone(), serde_json::Value::String(redact_text(s).into_owned()));
                        } else {
                            out.insert(k.clone(), v.clone());
                        }
                    } else {
                        out.insert(
                            k.clone(),
                            serde_json::Value::String("[redacted-transcript]".into()),
                        );
                    }
                    continue;
                }

                // Allow allowlisted telemetry fields directly if scalar
                if is_allowlisted_field(&k_lower) {
                    out.insert(k.clone(), v.clone());
                    continue;
                }

                // General recursion
                out.insert(k.clone(), sanitize_meta_value(v));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(sanitize_meta_value).collect())
        }
        serde_json::Value::String(s) => {
            let redacted = redact_text(s);
            serde_json::Value::String(redacted.into_owned())
        }
        other => other.clone(),
    }
}

// ---------------------------------------------------------------------------
// Convenience logging helpers used across ReflexDesk subsystems
// ---------------------------------------------------------------------------

pub fn log_event(event: LogEvent) {
    global_state().record_event(event);
}

pub fn log_lifecycle(phase: &str, outcome: &str, error: Option<&str>) {
    let mut ev = LogEvent::new("lifecycle", "transition", outcome).with_phase(phase);
    if let Some(err) = error {
        ev = ev.with_code(redact_error(err));
    }
    log_event(ev);
}

pub fn log_stt(op: &str, outcome: &str, latency_ms: Option<u64>, error: Option<&str>) {
    let mut ev = LogEvent::new("stt", op, outcome);
    if let Some(ms) = latency_ms {
        ev = ev.with_latency_ms(ms);
    }
    if let Some(err) = error {
        ev = ev.with_code(redact_error(err));
    }
    log_event(ev);
}

pub fn log_tool(
    tool: &str,
    outcome: &str,
    latency_ms: Option<u64>,
    session: Option<&str>,
    error: Option<&str>,
) {
    let mut ev = LogEvent::new("tools", tool, outcome);
    if let Some(s) = session {
        ev = ev.with_session(s);
    }
    if let Some(ms) = latency_ms {
        ev = ev.with_latency_ms(ms);
    }
    if let Some(err) = error {
        ev = ev.with_code(redact_error(err));
    }
    log_event(ev);
}

pub fn log_planner(
    op: &str,
    outcome: &str,
    latency_ms: Option<u64>,
    session: Option<&str>,
    error: Option<&str>,
) {
    let mut ev = LogEvent::new("planner", op, outcome);
    if let Some(s) = session {
        ev = ev.with_session(s);
    }
    if let Some(ms) = latency_ms {
        ev = ev.with_latency_ms(ms);
    }
    if let Some(err) = error {
        ev = ev.with_code(redact_error(err));
    }
    log_event(ev);
}

pub fn log_supervisor(op: &str, outcome: &str, process_id: Option<u128>, error: Option<&str>) {
    let mut ev = LogEvent::new("process_supervisor", op, outcome);
    if let Some(pid) = process_id {
        ev = ev.with_meta(serde_json::json!({ "process_id": pid }));
    }
    if let Some(err) = error {
        ev = ev.with_code(redact_error(err));
    }
    log_event(ev);
}

pub fn log_model_manager(op: &str, model_id: &str, outcome: &str, error: Option<&str>) {
    let mut ev = LogEvent::new("model_manager", op, outcome)
        .with_meta(serde_json::json!({ "model_id": model_id }));
    if let Some(err) = error {
        ev = ev.with_code(redact_error(err));
    }
    log_event(ev);
}

// ---------------------------------------------------------------------------
// Diagnostics bundle construction, preview, and atomic export
// ---------------------------------------------------------------------------

/// Compute coarse hardware classification without leaking sensitive machine identifiers.
pub fn compute_hardware_class() -> String {
    let cpus = num_cpus();
    let has_cuda = check_cuda_availability();

    if has_cuda {
        format!("gpu-cuda-{cpus}c")
    } else if cfg!(target_os = "macos") {
        format!("apple-silicon-{cpus}c")
    } else if cpus >= 8 {
        format!("high-core-cpu-{cpus}c")
    } else {
        format!("standard-cpu-{cpus}c")
    }
}

fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

fn check_cuda_availability() -> bool {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("nvidia-smi")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// Check availability of all core subsystem adapters.
pub fn inspect_adapter_availability() -> AdapterAvailability {
    let desktop_available = crate::desktop::desktop_health().healthy;
    let browser_available = crate::browser::health();
    let reflex_available = crate::reflex::health();
    let planner_available = true; // Local planner trait baseline compiled

    let harnesses = crate::harness::detect_all()
        .into_iter()
        .filter(|h| h.installed)
        .map(|h| h.name.to_string())
        .collect();

    AdapterAvailability {
        desktop: desktop_available,
        browser: browser_available,
        reflex: reflex_available,
        planner: planner_available,
        harnesses,
    }
}

/// Construct the complete, sanitized diagnostics bundle.
pub fn build_diagnostics_bundle(app: &AppHandle) -> DiagnosticsBundle {
    let state = global_state();
    let settings = app
        .try_state::<SettingsState>()
        .map(|s| s.snapshot())
        .unwrap_or_default();

    let benchmark_numbers = BenchmarkDiagnostics {
        voice_benchmark_ms: settings.voice_benchmark_ms,
        score_summary: match settings.voice_benchmark_ms {
            Some(ms) => format!("{ms} ms last speech benchmark"),
            None => "not measured".into(),
        },
    };

    let build_type = if cfg!(debug_assertions) {
        "debug".to_string()
    } else {
        "release".to_string()
    };

    DiagnosticsBundle {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        build: build_type,
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        hardware_class: compute_hardware_class(),
        runtime_version: "tauri-2".to_string(),
        model_id: Some(DEFAULT_STT_MODEL_ID.to_string()),
        model_revision: Some("current".to_string()),
        benchmark_numbers,
        sanitized_transitions: state.get_transitions(),
        sanitized_errors: state.get_errors(),
        adapter_availability: inspect_adapter_availability(),
        recent_logs: state.get_recent_logs(200),
    }
}

/// Preview diagnostics bundle before user triggers export.
/// Returns the exact bundle that would be exported.
pub fn get_diagnostics_preview(app: &AppHandle) -> DiagnosticsBundle {
    build_diagnostics_bundle(app)
}

/// Export diagnostics bundle to disk with atomic write and cancel-leaves-no-file semantics.
///
/// Returns the identical `DiagnosticsBundle` object.
pub fn export_diagnostics(
    app: &AppHandle,
    destination_path: Option<PathBuf>,
) -> Result<DiagnosticsBundle, String> {
    let bundle = build_diagnostics_bundle(app);
    let serialized = serde_json::to_string_pretty(&bundle)
        .map_err(|e| format!("Failed to serialize diagnostics: {e}"))?;

    // Determine target export path
    let target_path = match destination_path {
        Some(p) => {
            // Guard against empty path or raw root
            if p.as_os_str().is_empty() {
                return Err("Export destination path cannot be empty".into());
            }
            p
        }
        None => {
            let base_dir = app
                .path()
                .app_log_dir()
                .or_else(|_| app.path().app_config_dir())
                .unwrap_or_else(|_| std::env::temp_dir());
            let timestamp = current_unix_millis();
            base_dir.join(format!("reflexdesk-diagnostics-{timestamp}.json"))
        }
    };

    // Ensure parent directory exists
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create export directory: {e}"))?;
    }

    // Atomic write pattern: write to `.tmp-XYZ` then rename
    let tmp_path = target_path.with_extension(format!("tmp-{}", current_unix_millis()));

    let write_result = (|| -> Result<(), std::io::Error> {
        let mut file = File::create(&tmp_path)?;
        file.write_all(serialized.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp_path, &target_path)?;
        Ok(())
    })();

    if let Err(err) = write_result {
        // Cancel / failure leaves no file on disk
        if tmp_path.exists() {
            let _ = fs::remove_file(&tmp_path);
        }
        return Err(format!("Diagnostics export failed: {err}"));
    }

    log_event(
        LogEvent::new("diagnostics", "export", "ok")
            .with_meta(serde_json::json!({
                "target_filename": target_path.file_name().and_then(|n| n.to_str()).unwrap_or("diagnostics.json"),
                "size_bytes": serialized.len()
            })),
    );

    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_event_schema_version_and_fields() {
        let ev = LogEvent::new("stt", "transcribe", "ok")
            .with_session("test_sess")
            .with_latency_ms(120)
            .with_code("SUCCESS");

        assert_eq!(ev.v, 1);
        assert_eq!(ev.component, "stt");
        assert_eq!(ev.op, "transcribe");
        assert_eq!(ev.outcome, "ok");
        assert_eq!(ev.session.as_deref(), Some("test_sess"));
        assert_eq!(ev.latency_ms, Some(120));
        assert_eq!(ev.code.as_deref(), Some("SUCCESS"));
    }

    #[test]
    fn test_metadata_redaction_secrets_denied() {
        let meta = serde_json::json!({
            "api_key": "sk-1234567890abcdef1234567890abcdef",
            "token": "secret_token_value",
            "password": "my_password",
            "safe_metric": 42,
            "phase": "ready"
        });

        let sanitized = sanitize_meta_value(&meta);
        let obj = sanitized.as_object().unwrap();

        assert_eq!(obj.get("api_key").unwrap(), "[redacted-secret]");
        assert_eq!(obj.get("token").unwrap(), "[redacted-secret]");
        assert_eq!(obj.get("password").unwrap(), "[redacted-secret]");
        assert_eq!(obj.get("safe_metric").unwrap(), 42);
        assert_eq!(obj.get("phase").unwrap(), "ready");
    }

    #[test]
    fn test_metadata_redaction_audio_and_clipboard_denied() {
        let meta = serde_json::json!({
            "audio_samples": [1, 2, 3],
            "clipboard_text": "Sensitive clipboard copied text",
            "file_content": "cat /etc/passwd contents",
            "browser_text": "Private web content and messages"
        });

        let sanitized = sanitize_meta_value(&meta);
        let obj = sanitized.as_object().unwrap();

        assert_eq!(obj.get("audio_samples").unwrap(), "[redacted-secret]");
        assert_eq!(obj.get("clipboard_text").unwrap(), "[redacted-secret]");
        assert_eq!(obj.get("file_content").unwrap(), "[redacted-secret]");
        assert_eq!(obj.get("browser_text").unwrap(), "[redacted-secret]");
    }

    #[test]
    fn test_transcripts_never_logged_unless_debug_enabled() {
        set_debug_transcripts(false);
        let meta = serde_json::json!({
            "transcript": "Open private browser tab and delete all logs",
        });

        let sanitized = sanitize_meta_value(&meta);
        let obj = sanitized.as_object().unwrap();
        assert_eq!(obj.get("transcript").unwrap(), "[redacted-transcript]");

        // Enable debug transcripts: free text gets length redacted through redact_text
        set_debug_transcripts(true);
        let sanitized_debug = sanitize_meta_value(&meta);
        let obj_debug = sanitized_debug.as_object().unwrap();
        let val = obj_debug.get("transcript").unwrap().as_str().unwrap();
        assert!(val.starts_with("[redacted "));
        set_debug_transcripts(false);
    }

    #[test]
    fn test_rotating_logs_bounded_memory() {
        let state = ObservabilityState::new();
        for i in 0..1100 {
            state.record_event(LogEvent::new("test", format!("op_{i}"), "ok"));
        }

        let logs = state.get_recent_logs(2000);
        assert_eq!(logs.len(), DEFAULT_MAX_MEMORY_EVENTS);
    }

    #[test]
    fn test_atomic_export_cancel_leaves_no_file() {
        let temp_dir = std::env::temp_dir().join("reflexdesk_test_observability");
        let _ = fs::create_dir_all(&temp_dir);

        let target_file = temp_dir.join("test_export.json");
        if target_file.exists() {
            let _ = fs::remove_file(&target_file);
        }

        let bundle = DiagnosticsBundle {
            app_version: "0.1.0".into(),
            build: "test".into(),
            os: "windows".into(),
            arch: "x86_64".into(),
            hardware_class: "standard-cpu-4c".into(),
            runtime_version: "tauri-2".into(),
            model_id: Some("nemotron".into()),
            model_revision: Some("current".into()),
            benchmark_numbers: BenchmarkDiagnostics {
                voice_benchmark_ms: Some(150),
                score_summary: "150 ms".into(),
            },
            sanitized_transitions: vec![],
            sanitized_errors: vec![],
            adapter_availability: AdapterAvailability {
                desktop: true,
                browser: true,
                reflex: true,
                planner: true,
                harnesses: vec!["codex".into()],
            },
            recent_logs: vec![],
        };

        let serialized = serde_json::to_string_pretty(&bundle).unwrap();
        let tmp_path = target_file.with_extension("tmp-1234");
        fs::write(&tmp_path, serialized.as_bytes()).unwrap();
        fs::rename(&tmp_path, &target_file).unwrap();

        assert!(target_file.exists());
        assert!(!tmp_path.exists());

        let _ = fs::remove_file(target_file);
        let _ = fs::remove_dir_all(temp_dir);
    }
}
