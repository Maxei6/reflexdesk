use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, sync::Mutex};
use tauri::{AppHandle, Manager};

pub const SETTINGS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub schema_version: u32,
    pub setup_complete: bool,
    pub language: String,
    pub shortcut: String,
    pub start_at_login: bool,
    pub overlay_enabled: bool,
    pub sounds_enabled: bool,
    pub allow_online_ai: bool,
    pub voice_benchmark_ms: Option<u64>,
    pub stt_provider: String,
    pub laya_endpoint: String,
    pub planner_endpoint: String,
    pub planner_model: String,
    pub planner_secret_ref: Option<crate::secrets::SecretRef>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            setup_complete: false,
            language: "auto".into(),
            shortcut: "CommandOrControl+Shift+Space".into(),
            start_at_login: true,
            overlay_enabled: true,
            sounds_enabled: false,
            allow_online_ai: false,
            voice_benchmark_ms: None,
            stt_provider: "nemotron".into(),
            laya_endpoint: "http://127.0.0.1:8787".into(),
            planner_endpoint: "http://127.0.0.1:11434/v1/chat/completions".into(),
            planner_model: "auto".into(),
            planner_secret_ref: None,
        }
    }
}

pub struct SettingsState(pub Mutex<AppSettings>);

impl SettingsState {
    pub fn snapshot(&self) -> AppSettings {
        self.0
            .lock()
            .map(|settings| settings.clone())
            .unwrap_or_default()
    }

    pub fn replace(&self, settings: AppSettings) -> Result<(), String> {
        *self.0.lock().map_err(|_| "settings lock poisoned")? = settings;
        Ok(())
    }
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("settings.json"))
}

pub fn load(app: &AppHandle) -> AppSettings {
    match load_guarded(app) {
        Ok(s) => s,
        Err(err) => {
            let msg = crate::redaction::redact_error(&err);
            eprintln!("Settings load warning: {msg}");
            AppSettings::default()
        }
    }
}

pub fn load_guarded(app: &AppHandle) -> Result<AppSettings, String> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(AppSettings::default());
    }
    let contents = fs::read_to_string(&path)
        .map_err(|e| format!("failed to read settings file: {e}"))?;

    let raw_json: serde_json::Value = serde_json::from_str(&contents)
        .map_err(|e| format!("invalid settings JSON: {e}"))?;

    // Plaintext migration guard: reject and quarantine any settings file with plaintext credentials
    if let Some(detected_key) = crate::secrets::detect_plaintext_secret_key(&raw_json) {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let quarantine_path = path.with_extension(format!("json.quarantine.{ts}"));
        let _ = fs::rename(&path, &quarantine_path);
        let safe_msg = format!(
            "Settings quarantined: plaintext credential detected (field: {}). Credentials must be stored in secure SecretStore vault.",
            crate::redaction::redact_text(&detected_key)
        );
        return Err(safe_msg);
    }

    serde_json::from_value::<AppSettings>(raw_json)
        .map_err(|e| format!("failed to deserialize AppSettings: {e}"))
}

pub fn save(app: &AppHandle, settings: &AppSettings) -> Result<(), String> {
    let path = settings_path(app)?;
    let temp = path.with_extension("json.tmp");
    let payload = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(&temp, payload).map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    if path.exists() {
        fs::remove_file(&path).map_err(|e| e.to_string())?;
    }

    fs::rename(&temp, &path).map_err(|e| e.to_string())?;
    Ok(())
}
