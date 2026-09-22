use serde::Serialize;
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};

const PORT: u16 = 8789;
const PROVIDER: &str = "nemotron";
const MODEL: &str = "nvidia/nemotron-3.5-asr-streaming-0.6b";

pub struct SttState {
    child: Mutex<Option<Child>>,
}

impl Default for SttState {
    fn default() -> Self {
        Self {
            child: Mutex::new(None),
        }
    }
}

impl Drop for SttState {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

#[derive(Serialize)]
pub struct SttStatus {
    pub provider: &'static str,
    pub model: &'static str,
    pub runtime_found: bool,
    pub running: bool,
    pub ready: bool,
    pub endpoint: String,
}

#[derive(Serialize)]
pub struct Transcript {
    pub text: String,
    pub provider: &'static str,
    pub latency_ms: u128,
}

fn endpoint(path: &str) -> String {
    format!("http://127.0.0.1:{PORT}{path}")
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

fn health() -> bool {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(250))
        .build()
        .ok()
        .and_then(|client| client.get(endpoint("/health")).send().ok())
        .filter(|response| response.status().is_success())
        .and_then(|response| response.json::<serde_json::Value>().ok())
        .and_then(|payload| payload.get("backend").and_then(|value| value.as_str()).map(str::to_owned))
        .map(|backend| backend == "nemotron")
        .unwrap_or(false)
}

fn child_running(state: &SttState) -> bool {
    let Ok(mut guard) = state.child.lock() else {
        return false;
    };
    let Some(child) = guard.as_mut() else {
        return false;
    };

    match child.try_wait() {
        Ok(None) => true,
        Ok(Some(_)) | Err(_) => {
            *guard = None;
            false
        }
    }
}

pub fn status(app: &AppHandle, state: &SttState) -> SttStatus {
    SttStatus {
        provider: PROVIDER,
        model: MODEL,
        runtime_found: runtime_path(app).is_some(),
        running: child_running(state) || health(),
        ready: health(),
        endpoint: endpoint(""),
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
    if health() {
        return Ok(status(app, state));
    }

    if child_running(state) {
        return Ok(status(app, state));
    }

    let binary = runtime_path(app).ok_or_else(|| {
        "CrispASR runtime is missing. Run npm run prepare:stt or reinstall ReflexDesk."
            .to_string()
    })?;

    let threads = std::thread::available_parallelism()
        .map(|n| n.get().min(8).max(2))
        .unwrap_or(4);

    let port = PORT.to_string();
    let thread_count = threads.to_string();

    let runtime_dir = binary
        .parent()
        .ok_or_else(|| "invalid CrispASR runtime path".to_string())?
        .to_path_buf();

    let mut command = Command::new(&binary);
    command
        .current_dir(&runtime_dir)
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
        .env("CRISPASR_NEMOTRON_CONTEXT_PRESET", "0")
        .env("CRISPASR_NEMOTRON_STREAMING", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command
        .spawn()
        .map_err(|e| format!("failed to start CrispASR: {e}"))?;

    if let Some(stdout) = child.stdout.take() {
        forward_logs(stdout, app.clone(), "stdout");
    }
    if let Some(stderr) = child.stderr.take() {
        forward_logs(stderr, app.clone(), "stderr");
    }

    *state
        .child
        .lock()
        .map_err(|_| "STT process lock poisoned".to_string())? = Some(child);

    let _ = app.emit(
        "reflexdesk://stt-status",
        serde_json::json!({
            "stream": "runtime",
            "message": "Starting NVIDIA Nemotron 3.5 locally. First run may download the approximately 458 MB Q4_K model."
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

    if !health() {
        let _ = start(app, state)?;
        if !health() {
            return Err("Nemotron is still starting or downloading its model".into());
        }
    }

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
        .post(endpoint("/v1/audio/transcriptions"))
        .multipart(form)
        .send()
        .map_err(|e| format!("local Nemotron request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("local Nemotron returned HTTP {}", response.status()));
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
        return Err("Nemotron returned an empty transcript".into());
    }

    Ok(Transcript {
        text,
        provider: PROVIDER,
        latency_ms: started.elapsed().as_millis(),
    })
}

pub fn shutdown(state: &SttState) -> Result<(), String> {
    let mut guard = state
        .child
        .lock()
        .map_err(|_| "STT process lock poisoned".to_string())?;
    if let Some(mut child) = guard.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    Ok(())
}
