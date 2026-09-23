use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, sync::Mutex};
use tauri::{AppHandle, Manager};

pub const SETTINGS_SCHEMA_VERSION: u32 = 4;

/// Selectable speech-to-text engines. `openrouter` is the only remote engine.
pub const STT_PROVIDERS: &[&str] = &["nemotron", "moonshine", "openrouter", "manual"];

/// Selectable text-to-speech engines. `local` is the offline OS voice path.
pub const TTS_PROVIDERS: &[&str] = &["local", "openrouter"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub schema_version: u32,
    pub setup_complete: bool,
    pub language: String,
    pub secondary_language: String,
    pub ui_locale: String,
    pub shortcut: String,
    pub start_at_login: bool,
    pub overlay_enabled: bool,
    pub sounds_enabled: bool,
    pub spoken_feedback: bool,
    pub allow_online_ai: bool,
    pub voice_benchmark_ms: Option<u64>,
    pub stt_provider: String,
    pub tts_provider: String,
    pub openrouter_stt_model: String,
    pub openrouter_tts_model: String,
    pub openrouter_tts_voice: String,
    pub openrouter_secret_ref: Option<crate::secrets::SecretRef>,
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
            secondary_language: "none".into(),
            shortcut: "CommandOrControl+Shift+Space".into(),
            ui_locale: "system".into(),
            start_at_login: true,
            overlay_enabled: true,
            sounds_enabled: false,
            spoken_feedback: false,
            allow_online_ai: false,
            voice_benchmark_ms: None,
            stt_provider: "nemotron".into(),
            tts_provider: "local".into(),
            openrouter_stt_model: crate::openrouter::DEFAULT_STT_MODEL.into(),
            openrouter_tts_model: crate::openrouter::DEFAULT_TTS_MODEL.into(),
            openrouter_tts_voice: crate::openrouter::DEFAULT_TTS_VOICE.into(),
            openrouter_secret_ref: None,
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

impl AppSettings {
    /// True when the selected speech-to-text engine is OpenRouter's cloud API.
    pub fn stt_uses_openrouter(&self) -> bool {
        self.stt_provider == "openrouter"
    }

    /// True when the selected text-to-speech engine is OpenRouter's cloud API.
    pub fn tts_uses_openrouter(&self) -> bool {
        self.tts_provider == "openrouter"
    }

    /// Coerces stored values into the supported allowlists.
    ///
    /// Provider strings come from the renderer, so they are normalized rather
    /// than trusted: an unknown engine falls back to the offline default and
    /// can never silently address a remote endpoint. Model/voice identifiers
    /// are validated before they reach a URL or a request body.
    pub fn normalize(&mut self) {
        if !STT_PROVIDERS.contains(&self.stt_provider.as_str()) {
            self.stt_provider = "nemotron".into();
        }
        if !TTS_PROVIDERS.contains(&self.tts_provider.as_str()) {
            self.tts_provider = "local".into();
        }
        if self.planner_secret_ref.as_ref().is_some_and(|r| r.provider != "planner") {
            self.planner_secret_ref = None;
        }
        if self.openrouter_secret_ref.as_ref().is_some_and(|r| r.provider != "openrouter") {
            self.openrouter_secret_ref = None;
        }

        self.openrouter_stt_model =
            crate::openrouter::validate_model_id(&self.openrouter_stt_model)
                .unwrap_or_else(|_| crate::openrouter::DEFAULT_STT_MODEL.to_string());
        self.openrouter_tts_model =
            crate::openrouter::validate_model_id(&self.openrouter_tts_model)
                .unwrap_or_else(|_| crate::openrouter::DEFAULT_TTS_MODEL.to_string());
        self.openrouter_tts_voice =
            crate::openrouter::validate_voice_id(&self.openrouter_tts_voice)
                .unwrap_or_else(|_| crate::openrouter::DEFAULT_TTS_VOICE.to_string());
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

    let mut settings = serde_json::from_value::<AppSettings>(raw_json)
        .map_err(|e| format!("failed to deserialize AppSettings: {e}"))?;

    // Missing fields are filled by serde(default), preserving existing users'
    // language and audio preferences while adding the v3/v4 voice controls.
    settings.normalize();

    if settings.schema_version < SETTINGS_SCHEMA_VERSION {
        settings.schema_version = SETTINGS_SCHEMA_VERSION;
        if settings.ui_locale.is_empty() {
            settings.ui_locale = "system".into();
        }
        let _ = save(app, &settings);
    }

    Ok(settings)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_default_and_migration() {
        let default_settings = AppSettings::default();
        assert_eq!(default_settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(default_settings.ui_locale, "system");
        assert_eq!(default_settings.secondary_language, "none");
        assert!(!default_settings.spoken_feedback);
        assert_eq!(default_settings.tts_provider, "local");
        assert!(!default_settings.tts_uses_openrouter());
        assert!(!default_settings.stt_uses_openrouter());

        // Older settings retain their primary language and gain safe voice defaults.
        let v1_json = serde_json::json!({
            "schema_version": 1,
            "setup_complete": true,
            "language": "en"
        });

        let mut migrated: AppSettings = serde_json::from_value(v1_json).expect("deserialize v1");
        assert_eq!(migrated.schema_version, 1);
        assert_eq!(migrated.secondary_language, "none");
        assert!(!migrated.spoken_feedback);

        if migrated.schema_version < SETTINGS_SCHEMA_VERSION {
            migrated.schema_version = SETTINGS_SCHEMA_VERSION;
            if migrated.ui_locale.is_empty() {
                migrated.ui_locale = "system".into();
            }
        }
        assert_eq!(migrated.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(migrated.ui_locale, "system");
    }

    #[test]
    fn normalize_never_trusts_renderer_engine_or_model_strings() {
        let mut settings = AppSettings {
            // A renderer could send any of these; unknown values must fall back
            // to the offline defaults instead of reaching a remote endpoint.
            stt_provider: "openrouter; rm -rf /".into(),
            tts_provider: "OPENROUTER".into(),
            openrouter_stt_model: "evil model/../../etc".into(),
            openrouter_tts_model: "".into(),
            openrouter_tts_voice: "voice\ninjection".into(),
            planner_secret_ref: Some(crate::secrets::SecretRef {
                provider: "openrouter".into(),
                id: "planner-key".into(),
            }),
            openrouter_secret_ref: Some(crate::secrets::SecretRef {
                provider: "planner".into(),
                id: "planner-key".into(),
            }),
            ..AppSettings::default()
        };
        settings.normalize();

        assert_eq!(settings.stt_provider, "nemotron");
        assert_eq!(settings.tts_provider, "local");
        assert!(settings.planner_secret_ref.is_none());
        assert!(settings.openrouter_secret_ref.is_none());
        assert_eq!(
            settings.openrouter_stt_model,
            crate::openrouter::DEFAULT_STT_MODEL
        );
        assert_eq!(
            settings.openrouter_tts_model,
            crate::openrouter::DEFAULT_TTS_MODEL
        );
        assert_eq!(
            settings.openrouter_tts_voice,
            crate::openrouter::DEFAULT_TTS_VOICE
        );

        let mut valid = AppSettings {
            stt_provider: "openrouter".into(),
            tts_provider: "openrouter".into(),
            openrouter_stt_model: "openai/whisper-large-v3".into(),
            openrouter_tts_voice: "alloy".into(),
            ..AppSettings::default()
        };
        valid.normalize();
        assert!(valid.stt_uses_openrouter());
        assert!(valid.tts_uses_openrouter());
        assert_eq!(valid.openrouter_stt_model, "openai/whisper-large-v3");
        assert_eq!(valid.openrouter_tts_voice, "alloy");
    }
}
