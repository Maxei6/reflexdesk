mod harness;
mod lifecycle;
mod process_supervisor;
mod settings;
mod stt;
mod tools;
mod tray;

use lifecycle::{Phase, RuntimeSnapshot, RuntimeState};
use process_supervisor::ProcessSupervisor;
use serde::Serialize;
use settings::{AppSettings, SettingsState};
use std::{
    process::Command,
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};
use tauri::{menu::MenuId, AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as AutostartManagerExt};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

#[derive(Serialize)]
struct SystemProfile {
    os: &'static str,
    arch: &'static str,
    logical_cpus: usize,
    acceleration_hint: String,
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn update_runtime(
    app: &AppHandle,
    phase: Phase,
    error: Option<String>,
) -> Result<RuntimeSnapshot, String> {
    let runtime = app.state::<RuntimeState>();
    let snapshot = runtime.transition(phase, error)?;
    tray::update(app, &snapshot);
    let _ = app.emit("reflexdesk://state", &snapshot);
    Ok(snapshot)
}

fn register_shortcut(app: &AppHandle, preferred: &str) -> Result<String, String> {
    let candidates = [
        preferred.to_string(),
        "CommandOrControl+Alt+Space".to_string(),
        "CommandOrControl+Shift+Period".to_string(),
    ];

    for shortcut in candidates {
        if app.global_shortcut().register(shortcut.as_str()).is_ok() {
            return Ok(shortcut);
        }
    }

    Err("No global ReflexDesk shortcut could be registered. Change conflicting app shortcuts and restart ReflexDesk.".into())
}

fn apply_autostart(app: &AppHandle, enabled: bool) -> Result<bool, String> {
    let manager = app.autolaunch();
    if enabled {
        manager.enable().map_err(|e| e.to_string())?;
    } else {
        manager.disable().map_err(|e| e.to_string())?;
    }
    manager.is_enabled().map_err(|e| e.to_string())
}

fn set_listening_internal(app: &AppHandle, next: bool) -> Result<RuntimeSnapshot, String> {
    let runtime = app.state::<RuntimeState>();
    let current = runtime.snapshot();

    if next {
        if !current.ready {
            show_main(app);
            let _ = app.emit(
                "reflexdesk://attention",
                serde_json::json!({
                    "kind": "not_ready",
                    "message": "ReflexDesk is still preparing its local engine."
                }),
            );
            return Err("ReflexDesk is not ready yet".into());
        }

        let stt_state = app.state::<stt::SttState>();
        if !stt::status(app, &stt_state).ready {
            let _ = update_runtime(
                app,
                Phase::Degraded,
                Some("Local speech engine stopped unexpectedly".into()),
            );
            start_engine_background(app.clone());
            show_main(app);
            return Err("Local speech engine is recovering".into());
        }

        let snapshot = runtime.set_listening(true)?;
        tray::update(app, &snapshot);
        let _ = app.emit("reflexdesk://state", &snapshot);
        app.emit("reflexdesk://active", true)
            .map_err(|e| e.to_string())?;
        return Ok(snapshot);
    }

    app.emit("reflexdesk://active", false)
        .map_err(|e| e.to_string())?;
    let snapshot = runtime.set_listening(false)?;
    tray::update(app, &snapshot);
    let _ = app.emit("reflexdesk://state", &snapshot);
    Ok(snapshot)
}

fn start_engine_background(app: AppHandle) {
    let phase = app.state::<RuntimeState>().snapshot().phase;
    if matches!(
        phase,
        Phase::Preparing | Phase::Ready | Phase::Listening | Phase::ShuttingDown
    ) {
        return;
    }

    let _ = update_runtime(&app, Phase::Preparing, None);

    thread::spawn(move || {
        let state = app.state::<stt::SttState>();

        if let Err(error) = stt::start(&app, &state) {
            let _ = update_runtime(&app, Phase::Error, Some(error));
            show_main(&app);
            return;
        }

        let started = Instant::now();
        let timeout = Duration::from_secs(600);

        loop {
            if app.state::<RuntimeState>().snapshot().phase == Phase::ShuttingDown {
                return;
            }

            let status = stt::status(&app, &state);
            if status.ready {
                let _ = update_runtime(&app, Phase::Ready, None);
                let _ = app.emit("reflexdesk://engine-ready", status);
                return;
            }

            if !status.running && started.elapsed() > Duration::from_secs(2) {
                let _ = update_runtime(
                    &app,
                    Phase::Error,
                    Some("The local speech engine exited during startup.".into()),
                );
                show_main(&app);
                return;
            }

            if started.elapsed() >= timeout {
                let _ = update_runtime(
                    &app,
                    Phase::Error,
                    Some("Local speech setup timed out. Check your connection and retry.".into()),
                );
                show_main(&app);
                return;
            }

            thread::sleep(Duration::from_millis(500));
        }
    });
}

fn start_watchdog(app: AppHandle) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(8));

        let runtime = app.state::<RuntimeState>().snapshot();
        if runtime.phase == Phase::ShuttingDown {
            break;
        }

        app.state::<ProcessSupervisor>().cleanup_finished();

        let settings = app.state::<SettingsState>().snapshot();
        if !settings.setup_complete {
            continue;
        }

        if matches!(runtime.phase, Phase::Ready | Phase::Listening) {
            let stt_state = app.state::<stt::SttState>();
            if !stt::status(&app, &stt_state).ready {
                if runtime.listening {
                    let _ = app.emit("reflexdesk://active", false);
                }
                let _ = update_runtime(
                    &app,
                    Phase::Degraded,
                    Some("Speech engine stopped; ReflexDesk is restarting it.".into()),
                );
                start_engine_background(app.clone());
            }
        }
    });
}

fn shutdown_and_exit(app: &AppHandle) {
    let _ = update_runtime(app, Phase::ShuttingDown, None);
    let _ = app.emit("reflexdesk://active", false);
    let stt_state = app.state::<stt::SttState>();
    let _ = stt::shutdown(&stt_state);
    app.state::<ProcessSupervisor>().terminate_all();
    app.exit(0);
}

pub(crate) fn handle_tray_action(app: &AppHandle, id: &MenuId) {
    if tray::is_action(id, "tray_settings") {
        show_main(app);
        return;
    }

    if tray::is_action(id, "tray_toggle") {
        let current = app.state::<RuntimeState>().snapshot();
        let _ = set_listening_internal(app, !current.listening);
        return;
    }

    if tray::is_action(id, "tray_quit") {
        shutdown_and_exit(app);
    }
}

#[tauri::command]
fn get_runtime_status(state: State<'_, RuntimeState>) -> RuntimeSnapshot {
    state.snapshot()
}

#[tauri::command]
fn get_app_settings(state: State<'_, SettingsState>) -> AppSettings {
    state.snapshot()
}

#[tauri::command]
fn save_app_settings(
    app: AppHandle,
    state: State<'_, SettingsState>,
    mut settings: AppSettings,
) -> Result<AppSettings, String> {
    settings.schema_version = settings::SETTINGS_SCHEMA_VERSION;
    settings::save(&app, &settings)?;
    state.replace(settings.clone())?;

    if settings.setup_complete {
        apply_autostart(&app, settings.start_at_login)?;
    }

    let _ = app.emit("reflexdesk://settings", &settings);
    Ok(settings)
}

#[tauri::command]
fn get_autostart(app: AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
fn prepare_engine(app: AppHandle) -> RuntimeSnapshot {
    start_engine_background(app.clone());
    app.state::<RuntimeState>().snapshot()
}

#[tauri::command]
fn complete_setup(
    app: AppHandle,
    state: State<'_, SettingsState>,
    benchmark_ms: u64,
) -> Result<AppSettings, String> {
    let stt_state = app.state::<stt::SttState>();
    if !stt::status(&app, &stt_state).ready {
        return Err("The local speech engine is not ready yet.".into());
    }

    let mut settings = state.snapshot();
    settings.setup_complete = true;
    settings.voice_benchmark_ms = Some(benchmark_ms);
    settings.schema_version = settings::SETTINGS_SCHEMA_VERSION;
    settings::save(&app, &settings)?;
    state.replace(settings.clone())?;
    let _ = apply_autostart(&app, settings.start_at_login);
    let _ = update_runtime(&app, Phase::Ready, None);

    Ok(settings)
}

#[tauri::command]
fn reset_setup(
    app: AppHandle,
    state: State<'_, SettingsState>,
) -> Result<AppSettings, String> {
    let mut settings = state.snapshot();
    settings.setup_complete = false;
    settings.voice_benchmark_ms = None;
    settings::save(&app, &settings)?;
    state.replace(settings.clone())?;
    let _ = set_listening_internal(&app, false);
    let _ = update_runtime(&app, Phase::SetupRequired, None);
    show_main(&app);
    Ok(settings)
}

#[tauri::command]
fn get_system_profile() -> SystemProfile {
    let acceleration_hint = if cfg!(target_os = "macos") {
        "Metal-capable native runtime".to_string()
    } else if Command::new("nvidia-smi")
        .arg("--help")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
    {
        "NVIDIA GPU detected; portable runtime active in this build".to_string()
    } else {
        "Portable CPU runtime".to_string()
    };

    SystemProfile {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        logical_cpus: std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(1),
        acceleration_hint,
    }
}

#[tauri::command]
fn set_listening(app: AppHandle, active: bool) -> Result<RuntimeSnapshot, String> {
    set_listening_internal(&app, active)
}

#[tauri::command]
fn toggle_listening(app: AppHandle) -> Result<RuntimeSnapshot, String> {
    let current = app.state::<RuntimeState>().snapshot();
    set_listening_internal(&app, !current.listening)
}

#[tauri::command]
fn execute_tool(
    app: AppHandle,
    runtime: State<'_, RuntimeState>,
    supervisor: State<'_, ProcessSupervisor>,
    name: String,
    args: serde_json::Value,
) -> Result<tools::ToolResult, String> {
    let was_listening = runtime.snapshot().listening;
    let _ = update_runtime(&app, Phase::Executing, None);
    let _ = app.emit("reflexdesk://visual-state", "executing");

    let result = tools::execute(&name, &args, &supervisor);

    match &result {
        Ok(_) => {
            let _ = app.emit("reflexdesk://visual-state", "success");
        }
        Err(error) => {
            let _ = app.emit(
                "reflexdesk://visual-state",
                serde_json::json!({ "state": "error", "message": error }),
            );
        }
    }

    let next = if was_listening {
        Phase::Listening
    } else {
        Phase::Ready
    };
    let _ = update_runtime(&app, next, None);
    result
}

#[tauri::command]
fn laya_route(
    app: AppHandle,
    runtime: State<'_, RuntimeState>,
    endpoint: String,
    text: String,
) -> Result<serde_json::Value, String> {
    if !(endpoint.starts_with("http://127.0.0.1") || endpoint.starts_with("http://localhost")) {
        return Err("Laya endpoint must be localhost".into());
    }

    let was_listening = runtime.snapshot().listening;
    let _ = update_runtime(&app, Phase::Routing, None);
    let _ = app.emit("reflexdesk://visual-state", "thinking");

    let url = format!("{}/route", endpoint.trim_end_matches('/'));
    let result = reqwest::blocking::Client::new()
        .post(url)
        .json(&serde_json::json!({
            "text": text,
            "context": { "source": "reflexdesk" }
        }))
        .send()
        .map_err(|e| e.to_string())
        .and_then(|response| {
            response
                .json::<serde_json::Value>()
                .map_err(|e| e.to_string())
        });

    let _ = update_runtime(
        &app,
        if was_listening { Phase::Listening } else { Phase::Ready },
        None,
    );
    result
}

#[tauri::command]
fn planner_route(
    app: AppHandle,
    runtime: State<'_, RuntimeState>,
    endpoint: String,
    model: String,
    text: String,
    allow_remote: bool,
) -> Result<serde_json::Value, String> {
    let local = endpoint.starts_with("http://127.0.0.1")
        || endpoint.starts_with("http://localhost");

    if !local && !allow_remote {
        return Err("remote planner blocked in offline mode".into());
    }

    let was_listening = runtime.snapshot().listening;
    let _ = update_runtime(&app, Phase::Routing, None);
    let _ = app.emit("reflexdesk://visual-state", "thinking");

    let client = reqwest::blocking::Client::new();
    let mut resolved_model = model;

    if resolved_model.trim().is_empty() || resolved_model == "auto" {
        let models_url = if endpoint.ends_with("/chat/completions") {
            format!("{}/models", endpoint.trim_end_matches("/chat/completions"))
        } else {
            format!("{}/models", endpoint.trim_end_matches('/'))
        };

        let mut request = client.get(models_url);
        if let Ok(key) = std::env::var("REFLEXDESK_PLANNER_API_KEY") {
            if !key.trim().is_empty() {
                request = request.bearer_auth(key);
            }
        }

        let models = request
            .send()
            .map_err(|e| format!("could not discover models: {e}"))?
            .json::<serde_json::Value>()
            .map_err(|e| format!("invalid /models response: {e}"))?;

        resolved_model = models
            .pointer("/data/0/id")
            .and_then(|value| value.as_str())
            .ok_or("no model found; start an OpenAI-compatible model server or choose a model")?
            .to_string();
    }

    let instruction = "Return ONLY compact JSON with keys action,args. Allowed actions: app.open, browser.open, browser.search, harness.start, unknown. Never invent tools. app.open args={app:string}; browser.open args={url:http/https}; browser.search args={query:string}; harness.start args={harness:string,prompt:string,cwd?:string}. If unsure return {\"action\":\"unknown\",\"args\":{}}.";

    let body = serde_json::json!({
        "model": resolved_model,
        "stream": false,
        "temperature": 0,
        "messages": [
            { "role": "system", "content": instruction },
            { "role": "user", "content": text }
        ]
    });

    let mut request = client.post(endpoint).json(&body);
    if let Ok(key) = std::env::var("REFLEXDESK_PLANNER_API_KEY") {
        if !key.trim().is_empty() {
            request = request.bearer_auth(key);
        }
    }

    let result = (|| -> Result<serde_json::Value, String> {
        let response = request.send().map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(format!("planner returned HTTP {}", response.status()));
        }

        let value = response
            .json::<serde_json::Value>()
            .map_err(|e| e.to_string())?;

        let content = value
            .pointer("/choices/0/message/content")
            .and_then(|value| value.as_str())
            .ok_or("planner returned no content")?;

        serde_json::from_str(content.trim())
            .map_err(|e| format!("invalid planner JSON: {e}"))
    })();

    let _ = update_runtime(
        &app,
        if was_listening { Phase::Listening } else { Phase::Ready },
        None,
    );
    result
}

#[tauri::command]
fn detect_harnesses() -> Vec<harness::HarnessStatus> {
    harness::detect_all()
}

#[tauri::command]
fn stt_status(
    app: AppHandle,
    state: State<'_, stt::SttState>,
) -> stt::SttStatus {
    stt::status(&app, &state)
}

#[tauri::command]
fn stt_transcribe(
    app: AppHandle,
    runtime: State<'_, RuntimeState>,
    state: State<'_, stt::SttState>,
    samples: Vec<i16>,
    sample_rate: u32,
    language: String,
) -> Result<stt::Transcript, String> {
    let was_listening = runtime.snapshot().listening;
    let _ = update_runtime(&app, Phase::Transcribing, None);
    let _ = app.emit("reflexdesk://visual-state", "transcribing");

    let result = stt::transcribe(&app, &state, samples, sample_rate, language);

    let _ = update_runtime(
        &app,
        if was_listening { Phase::Listening } else { Phase::Ready },
        None,
    );
    result
}

#[tauri::command]
fn stt_shutdown(state: State<'_, stt::SttState>) -> Result<(), String> {
    stt::shutdown(&state)
}

#[tauri::command]
fn open_settings(app: AppHandle) {
    show_main(&app);
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    shutdown_and_exit(&app);
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(RuntimeState::default())
        .manage(stt::SttState::default())
        .manage(ProcessSupervisor::default())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state != ShortcutState::Pressed {
                        return;
                    }
                    let current = app.state::<RuntimeState>().snapshot();
                    let _ = set_listening_internal(app, !current.listening);
                })
                .build(),
        )
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(|app| {
            let loaded_settings = settings::load(app.handle());
            app.manage(SettingsState(Mutex::new(loaded_settings.clone())));

            tray::setup(app)?;

            let registered_shortcut = register_shortcut(app.handle(), &loaded_settings.shortcut);
            let mut loaded_settings = loaded_settings;

            match registered_shortcut {
                Ok(shortcut) => {
                    if loaded_settings.shortcut != shortcut {
                        loaded_settings.shortcut = shortcut;
                        let _ = settings::save(app.handle(), &loaded_settings);
                        let state = app.state::<SettingsState>();
                        let _ = state.replace(loaded_settings.clone());
                    }
                }
                Err(error) => {
                    let _ = update_runtime(app.handle(), Phase::Error, Some(error));
                    show_main(app.handle());
                }
            }

            if let Some(main) = app.get_webview_window("main") {
                let _ = main.hide();
            }
            if let Some(overlay) = app.get_webview_window("overlay") {
                let _ = overlay.hide();
            }

            if loaded_settings.setup_complete {
                let _ = apply_autostart(app.handle(), loaded_settings.start_at_login);
                start_engine_background(app.handle().clone());
            } else {
                let _ = update_runtime(app.handle(), Phase::SetupRequired, None);
                show_main(app.handle());
            }

            start_watchdog(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_runtime_status,
            get_app_settings,
            save_app_settings,
            get_autostart,
            prepare_engine,
            complete_setup,
            reset_setup,
            get_system_profile,
            set_listening,
            toggle_listening,
            execute_tool,
            laya_route,
            planner_route,
            detect_harnesses,
            stt_status,
            stt_transcribe,
            stt_shutdown,
            open_settings,
            quit_app
        ])
        .run(tauri::generate_context!())
        .expect("error while running ReflexDesk");
}
