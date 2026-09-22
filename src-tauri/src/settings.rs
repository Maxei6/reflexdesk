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
    pub start_at_login: bool,
    pub overlay_enabled: bool,
    pub sounds_enabled: bool,
    pub allow_online_ai: bool,
    pub voice_benchmark_ms: Option<u64>,
    pub stt_provider: String,
    pub laya_endpoint: String,
    pub planner_endpoint: String,
    pub planner_model: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            setup_complete: false,
            language: "auto".into(),
            start_at_login: true,
            overlay_enabled: true,
            sounds_enabled: false,
            allow_online_ai: false,
            voice_benchmark_ms: None,
            stt_provider: "nemotron".into(),
            laya_endpoint: "http://127.0.0.1:8787".into(),
            planner_endpoint: "http://127.0.0.1:11434/v1/chat/completions".into(),
            planner_model: "auto".into(),
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
    let Ok(path) = settings_path(app) else {
        return AppSettings::default();
    };
    let Ok(contents) = fs::read_to_string(path) else {
        return AppSettings::default();
    };
    serde_json::from_str::<AppSettings>(&contents).unwrap_or_default()
}

pub fn save(app: &AppHandle, settings: &AppSettings) -> Result<(), String> {
    let path = settings_path(app)?;
    let temp = path.with_extension("json.tmp");
    let payload = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(&temp, payload).map_err(|e| e.to_string())?;
    fs::rename(&temp, &path).map_err(|e| e.to_string())?;
    Ok(())
}
