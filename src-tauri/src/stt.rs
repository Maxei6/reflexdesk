use crate::process_supervisor::{OwnershipClass, ProcessId, ProcessSpec, ProcessSupervisor};
use serde::Serialize;
use std::{
    io::{BufRead, BufReader},
    net::TcpListener,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};

const PROVIDER: &str = "nemotron";
const MODEL: &str = "nvidia/nemotron-3.5-asr-streaming-0.6b";

#[derive(Clone)]
struct LocalEndpoint {
    port: u16,
    token: String,
}

pub struct SttState {
    process_id: Mutex<Option<ProcessId>>,
    endpoint: Mutex<Option<LocalEndpoint>>,
    starting: AtomicBool,
    shutting_down: AtomicBool,
}

impl Default for SttState {
    fn default() -> Self {
        Self {
            process_id: Mutex::new(None),
            endpoint: Mutex::new(None),
            starting: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
        }
    }
}

impl Drop for SttState {
    fn drop(&mut self) {
        let _ = shutdown(self);
    }
}

#[derive(Clone, Serialize)]
pub struct SttStatus {
    pub provider: &'static str,
    pub model: &'static str,
    pub runtime_found: bool,
    pub running: bool,
    pub ready: bool,
    pub endpoint: Option<String>,
}

#[derive(Serialize)]
pub struct Transcript {
    pub text: String,
    pub provider: &'static str,
    pub latency_ms: u128,
}

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

fn endpoint_snapshot(state: &SttState) -> Option<LocalEndpoint> {
    state.endpoint.lock().ok().and_then(|guard| guard.clone())
}

fn health(state: &SttState) -> bool {
    let Some(endpoint) = endpoint_snapshot(state) else {
        return false;
    };

    reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(350))
        .build()
        .ok()
        .and_then(|client| client.get(endpoint_url(&endpoint, "/health")).send().ok())
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

fn child_running(state: &SttState) -> bool {
    let Ok(mut guard) = state.process_id.lock() else {
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

pub fn status(app: &AppHandle, state: &SttState) -> SttStatus {
    let endpoint = endpoint_snapshot(state);
    // Ready requires manager-verified Active artifact; process health alone
    // never yields Ready. Missing manager state fails closed (not ready).
    let model_ready = app
        .try_state::<crate::model_manager::ModelManager>()
        .map(|mgr| mgr.is_ready_for_engine(crate::model_manager::DEFAULT_STT_MODEL_ID))
        .unwrap_or(false);
    SttStatus {
        provider: PROVIDER,
        model: MODEL,
        runtime_found: runtime_path(app).is_some(),
        running: child_running(state),
        ready: health(state) && model_ready,
        endpoint: endpoint.map(|value| format!("127.0.0.1:{}", value.port)),
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
                let _ = app.emit(
                    "reflexdesk://stt-status",
                    serde_json::json!({ "stream": stream, "message": line }),
                );
            }
        }
    });
}

pub fn start(app: &AppHandle, state: &SttState) -> Result<SttStatus, String> {
    if health(state) {
        return Ok(status(app, state));
    }

    if child_running(state) {
        return Ok(status(app, state));
    }
    shutdown(state)?;
    state.shutting_down.store(false, Ordering::SeqCst);
    state.starting.store(true, Ordering::SeqCst);

    let binary = runtime_path(app).ok_or_else(|| {
        state.starting.store(false, Ordering::SeqCst);
        "CrispASR runtime is missing. Reinstall ReflexDesk or repair the installation."
            .to_string()
    })?;

    let endpoint = allocate_endpoint().map_err(|e| {
        state.starting.store(false, Ordering::SeqCst);
        e
    })?;
    let threads = std::thread::available_parallelism()
        .map(|n| n.get().min(8).max(2))
        .unwrap_or(4);

    let runtime_dir = binary
        .parent()
        .ok_or_else(|| {
            state.starting.store(false, Ordering::SeqCst);
            "invalid CrispASR runtime path".to_string()
        })?
        .to_path_buf();

    let port = endpoint.port.to_string();
    let thread_count = threads.to_string();

    // Pre-spawn check: abort if shutdown was initiated
    if state.shutting_down.load(Ordering::SeqCst) {
        state.starting.store(false, Ordering::SeqCst);
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
            state.starting.store(false, Ordering::SeqCst);
            return Err(format!("failed to start local speech engine: {e}"));
        }
    };

    // Post-spawn check: abort if shutdown occurred during spawn
    if state.shutting_down.load(Ordering::SeqCst) {
        state.starting.store(false, Ordering::SeqCst);
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

    *state
        .endpoint
        .lock()
        .map_err(|_| "STT endpoint lock poisoned".to_string())? = Some(endpoint);

    *state
        .process_id
        .lock()
        .map_err(|_| "STT process lock poisoned".to_string())? = Some(proc_id);

    state.starting.store(false, Ordering::SeqCst);
    let _ = app.emit(
        "reflexdesk://stt-status",
        serde_json::json!({
            "stream": "runtime",
            "message": "Preparing NVIDIA Nemotron 3.5 locally. First setup may download the speech model."
        }),
    );

    Ok(status(app, state))
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

pub fn transcribe(
    app: &AppHandle,
    state: &SttState,
    samples: Vec<i16>,
    sample_rate: u32,
    language: String,
) -> Result<Transcript, String> {
    if samples.len() < (sample_rate as usize / 10) {
        return Err("speech segment is too short".into());
    }
    if samples.len() > (sample_rate as usize * 30) {
        return Err("speech segment exceeds the 30 second command limit".into());
    }

    if !health(state) {
        let _ = start(app, state)?;
        if !health(state) {
            return Err("speech engine is still preparing".into());
        }
    }

    let endpoint = endpoint_snapshot(state).ok_or("local speech endpoint is unavailable")?;
    let started = Instant::now();
    let wav = wav_bytes(&samples, sample_rate);

    let part = reqwest::blocking::multipart::Part::bytes(wav)
        .file_name("reflexdesk-command.wav")
        .mime_str("audio/wav")
        .map_err(|e| e.to_string())?;

    let mut form = reqwest::blocking::multipart::Form::new()
        .part("file", part)
        .text("response_format", "json");

    let language = language.trim();
    if !language.is_empty() && language != "auto" {
        form = form.text("language", language.to_string());
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

pub fn shutdown(state: &SttState) -> Result<(), String> {
    state.shutting_down.store(true, Ordering::SeqCst);

    if let Ok(mut guard) = state.process_id.lock() {
        if let Some(id) = guard.take() {
            ProcessSupervisor::terminate_process_global(id);
        }
    }

    *state
        .endpoint
        .lock()
        .map_err(|_| "STT endpoint lock poisoned".to_string())? = None;

    Ok(())
}
