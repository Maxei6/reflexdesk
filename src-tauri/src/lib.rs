pub mod benchmark;
pub mod browser;
pub mod desktop;
pub mod hardware;
mod harness;
mod lifecycle;
mod model_manager;
pub mod observability;
pub mod planner;
mod policy;
mod process;
mod process_supervisor;
mod redaction;
pub mod reflex;
pub mod secrets;
mod security;
mod settings;
pub mod skills;
mod stt;
mod tools;
mod transcript;
mod tray;
pub mod updater;

use lifecycle::{Phase, RuntimeSnapshot, RuntimeState};
use model_manager::{ModelManager, ModelProgress, ModelStatus, DEFAULT_STT_MODEL_ID};
use process_supervisor::ProcessSupervisor;
use serde::Serialize;
use settings::{AppSettings, SettingsState};
use std::{
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
    let sanitized_error = error.map(|e| redaction::redact_error(&e));
    let runtime = app.state::<RuntimeState>();
    let snapshot = runtime.transition(phase, sanitized_error.clone())?;
    tray::update(app, &snapshot);
    let _ = app.emit("reflexdesk://state", &snapshot);
    observability::log_lifecycle(
        &format!("{:?}", phase).to_lowercase(),
        if sanitized_error.is_some() {
            "error"
        } else {
            "ok"
        },
        sanitized_error.as_deref(),
    );
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
    Ok(enabled)
}

fn set_listening_internal(app: &AppHandle, next: bool) -> Result<RuntimeSnapshot, String> {
    let runtime = app.state::<RuntimeState>();
    let current = runtime.snapshot();

    if !next {
        // Deterministic global cancel: stops capture, aborts the current
        // action, and marks owned harness sessions interrupted.
        policy::cancel_now("listening-stopped");
    }
    if next {
        let app_settings = app.state::<SettingsState>().snapshot();
        let requires_native_stt = app_settings.stt_provider == "nemotron";

        if !current.ready && (!app_settings.setup_complete || requires_native_stt) {
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
        if requires_native_stt && !stt::status(app, &stt_state).ready {
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
        let model_mgr = app.state::<ModelManager>();

        // Model acquisition first: Ready requires a manager-verified Active
        // artifact; STT health alone never yields Ready.
        if !model_mgr.is_ready_for_engine(DEFAULT_STT_MODEL_ID) {
            let app_handle = app.clone();
            let res = model_mgr.download_and_activate(
                DEFAULT_STT_MODEL_ID,
                Some(move |progress: ModelProgress| {
                    model_manager::emit_model_progress(&app_handle, &progress);
                }),
            );
            if let Err(err) = res {
                let _ = update_runtime(
                    &app,
                    Phase::Error,
                    Some(format!("Model acquisition failed: {err}")),
                );
                show_main(&app);
                return;
            }
        }

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
            // Ready requires BOTH STT process health AND manager-verified Active.
            if status.ready && model_mgr.is_ready_for_engine(DEFAULT_STT_MODEL_ID) {
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

        if settings.stt_provider == "nemotron"
            && matches!(runtime.phase, Phase::Ready | Phase::Listening)
        {
            let stt_state = app.state::<stt::SttState>();
            let model_mgr = app.state::<ModelManager>();
            let engine_healthy = stt::status(&app, &stt_state).ready;
            let model_active = model_mgr.is_ready_for_engine(DEFAULT_STT_MODEL_ID);

            if !engine_healthy || !model_active {
                if runtime.listening {
                    let _ = app.emit("reflexdesk://active", false);
                }
                let message = if !model_active {
                    "Model artifact missing or corrupted; repairing...".into()
                } else {
                    "Speech engine stopped; ReflexDesk is restarting it.".into()
                };
                let _ = update_runtime(&app, Phase::Degraded, Some(message));
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
    app.state::<ProcessSupervisor>().terminate_all_owned();
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
    let previous = state.snapshot();
    settings::save(&app, &settings)?;
    state.replace(settings.clone())?;

    if settings.setup_complete {
        apply_autostart(&app, settings.start_at_login)?;

        if previous.stt_provider != settings.stt_provider {
            if settings.stt_provider == "nemotron" {
                let _ = update_runtime(&app, Phase::Booting, None);
                start_engine_background(app.clone());
            } else {
                let stt_state = app.state::<stt::SttState>();
                let _ = stt::shutdown(&stt_state);
                let _ = update_runtime(&app, Phase::Ready, None);
            }
        }
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
    let model_mgr = app.state::<ModelManager>();
    if !stt::status(&app, &stt_state).ready || !model_mgr.is_ready_for_engine(DEFAULT_STT_MODEL_ID)
    {
        return Err("The local speech engine and model are not ready yet.".into());
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
fn reset_setup(app: AppHandle, state: State<'_, SettingsState>) -> Result<AppSettings, String> {
    let mut settings = state.snapshot();
    settings.setup_complete = false;
    settings.voice_benchmark_ms = None;
    settings::save(&app, &settings)?;
    state.replace(settings.clone())?;
    let stt_state = app.state::<stt::SttState>();
    let _ = stt::shutdown(&stt_state);
    let _ = set_listening_internal(&app, false);
    let _ = update_runtime(&app, Phase::SetupRequired, None);
    show_main(&app);
    Ok(settings)
}

#[tauri::command]
fn get_model_status(model_mgr: State<'_, ModelManager>, model_id: Option<String>) -> ModelStatus {
    let id = model_id.unwrap_or_else(|| DEFAULT_STT_MODEL_ID.to_string());
    model_mgr.get_status(&id)
}

#[tauri::command]
fn get_model_cache_size(model_mgr: State<'_, ModelManager>) -> u64 {
    model_mgr.total_cache_bytes()
}

#[tauri::command]
fn repair_model(app: AppHandle, model_mgr: State<'_, ModelManager>) -> Result<String, String> {
    let app_handle = app.clone();
    let path = model_mgr.repair(
        DEFAULT_STT_MODEL_ID,
        Some(move |progress: ModelProgress| {
            model_manager::emit_model_progress(&app_handle, &progress);
        }),
    )?;
    start_engine_background(app);
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
fn get_system_profile() -> SystemProfile {
    let profile = hardware::detect_hardware_profile();
    let cached = benchmark::get_cached_benchmark();

    let acceleration_hint = if let Some(report) = cached {
        if let Some(selected) = &report.selected_candidate_id {
            format!("Empirical benchmark active: {selected}")
        } else {
            "Benchmarked (no active candidate)".to_string()
        }
    } else if profile.metal {
        "Metal-capable native runtime detected".to_string()
    } else if profile.cuda {
        "NVIDIA CUDA GPU detected; empirical benchmark recommended".to_string()
    } else if profile.vulkan {
        "Vulkan compute detected; empirical benchmark recommended".to_string()
    } else if profile.cpu_features.iter().any(|f| f == "avx2") {
        "AVX2 vector-accelerated CPU runtime".to_string()
    } else {
        "Portable legacy CPU runtime".to_string()
    };

    SystemProfile {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        logical_cpus: profile.logical_cores,
        acceleration_hint,
    }
}

#[tauri::command]
fn get_hardware_profile() -> hardware::HardwareProfile {
    hardware::detect_hardware_profile()
}

#[tauri::command]
fn get_benchmark_report() -> Option<benchmark::BenchmarkReport> {
    benchmark::get_cached_benchmark()
}

#[tauri::command]
fn run_hardware_benchmark(timeout_secs: Option<u64>) -> Result<benchmark::BenchmarkReport, String> {
    benchmark::run_benchmark(timeout_secs)
}

#[tauri::command]
fn set_backend_override(candidate_id: Option<String>) -> Result<Option<String>, String> {
    benchmark::set_backend_override(candidate_id)
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

/// Single internal execution path. Every action flows through
/// validate -> authorize -> pre-cancel -> execute -> verify -> post-cancel.
/// There is no other route to `tools::execute`.
fn execute_verified(
    app: &AppHandle,
    runtime: &RuntimeState,
    supervisor: &ProcessSupervisor,
    settings: &AppSettings,
    envelope: &policy::ActionEnvelope,
    raw_text: Option<&str>,
) -> policy::ActionExecutionResult {
    let denied = |reason: String| {
        let redacted_reason = redaction::redact_error(&reason);
        observability::log_tool(
            &envelope.tool,
            "deny",
            None,
            Some(&envelope.session_id),
            Some(&redacted_reason),
        );
        policy::ActionExecutionResult {
            status: "deny".into(),
            output: None,
            verification: None,
            confirmation_id: None,
            tool: Some(envelope.tool.clone()),
            risk: Some(envelope.risk),
            args_summary: None,
            reason: Some(redacted_reason),
        }
    };

    // 1. Policy decision: validation, negation, authorization, pre-cancel.
    let sanitized_args =
        match policy::decide_and_prepare_with_settings(envelope, raw_text, settings) {
            policy::PolicyDecision::Denied { reason } => return denied(reason),
            policy::PolicyDecision::NeedConfirm {
                confirmation_id,
                tool,
                risk,
                args_summary,
            } => {
                observability::log_tool(
                    &envelope.tool,
                    "confirm",
                    None,
                    Some(&envelope.session_id),
                    None,
                );
                return policy::ActionExecutionResult {
                    status: "confirm".into(),
                    output: None,
                    verification: None,
                    confirmation_id: Some(confirmation_id),
                    tool: Some(tool),
                    risk: Some(risk),
                    args_summary: Some(args_summary),
                    reason: None,
                };
            }
            policy::PolicyDecision::ExecuteNow { sanitized_args } => sanitized_args,
        };

    // 2. Pre-execution cancellation check.
    if policy::is_cancelled(&envelope.session_id) {
        let mut res = denied("action-cancelled".into());
        res.status = "cancelled".into();
        return res;
    }

    let was_listening = runtime.snapshot().listening;
    let _ = update_runtime(app, Phase::Executing, None);
    let _ = app.emit("reflexdesk://visual-state", "executing");
    let settle = |app: &AppHandle, was_listening: bool| {
        let next = if was_listening {
            Phase::Listening
        } else {
            Phase::Ready
        };
        let _ = update_runtime(app, next, None);
    };

    // 3. Execution via the tool registry surface.
    let updater = app.state::<updater::UpdaterService>();
    let model_manager = app.state::<ModelManager>();
    let tool_res = match tools::execute(
        &envelope.tool,
        &sanitized_args,
        supervisor,
        &updater,
        &model_manager,
    ) {
        Ok(res) => res,
        Err(err) => {
            let redacted_err = redaction::redact_error(&err);
            let _ = app.emit(
                "reflexdesk://visual-state",
                serde_json::json!({ "state": "error", "message": &redacted_err }),
            );
            settle(app, was_listening);
            observability::log_tool(
                &envelope.tool,
                "error",
                None,
                Some(&envelope.session_id),
                Some(&redacted_err),
            );
            let mut res = denied(redacted_err);
            res.status = "error".into();
            return res;
        }
    };

    // 4. Post-action verification re-inspection.
    if let Err(verify_err) = policy::verify_stub(&envelope.verification) {
        let _ = app.emit(
            "reflexdesk://visual-state",
            serde_json::json!({ "state": "error", "message": &verify_err }),
        );
        settle(app, was_listening);
        let mut res = denied(verify_err);
        res.status = "error".into();
        return res;
    }

    // 5. Delayed-completion re-check: never report success for cancelled work.
    if policy::is_cancelled(&envelope.session_id) {
        settle(app, was_listening);
        let mut res = denied("action-cancelled".into());
        res.status = "cancelled".into();
        return res;
    }

    let _ = app.emit("reflexdesk://visual-state", "success");
    observability::log_tool(
        &envelope.tool,
        "success",
        None,
        Some(&envelope.session_id),
        None,
    );
    settle(app, was_listening);
    let res = policy::ActionExecutionResult {
        status: "success".into(),
        output: serde_json::to_value(&tool_res).ok(),
        verification: Some(serde_json::json!({ "ok": true })),
        confirmation_id: None,
        tool: Some(envelope.tool.clone()),
        risk: Some(envelope.risk),
        args_summary: None,
        reason: None,
    };
    skills::record_verified_action(envelope, &res);
    res
}

#[tauri::command]
fn request_action(
    app: AppHandle,
    runtime: State<'_, RuntimeState>,
    supervisor: State<'_, ProcessSupervisor>,
    settings_state: State<'_, SettingsState>,
    envelope: policy::ActionEnvelope,
    raw_text: Option<String>,
) -> Result<policy::ActionExecutionResult, String> {
    let settings = settings_state.snapshot();
    Ok(execute_verified(
        &app,
        &runtime,
        &supervisor,
        &settings,
        &envelope,
        raw_text.as_deref(),
    ))
}

#[tauri::command]
fn confirm_action(
    app: AppHandle,
    runtime: State<'_, RuntimeState>,
    supervisor: State<'_, ProcessSupervisor>,
    settings_state: State<'_, SettingsState>,
    confirmation_id: String,
    approve: bool,
) -> Result<policy::ActionExecutionResult, String> {
    if !policy::is_confirmation_id(&confirmation_id) {
        return Err("invalid-args: malformed confirmation id".into());
    }
    let settings = settings_state.snapshot();
    match policy::resolve_confirmation(&confirmation_id, approve) {
        Some(envelope) => Ok(execute_verified(
            &app,
            &runtime,
            &supervisor,
            &settings,
            &envelope,
            None,
        )),
        None => Ok(policy::ActionExecutionResult {
            status: "deny".into(),
            output: None,
            verification: None,
            confirmation_id: Some(confirmation_id),
            tool: None,
            risk: None,
            args_summary: None,
            reason: Some(if approve {
                "confirmation-expired-or-not-found".into()
            } else {
                "user-denied".into()
            }),
        }),
    }
}

#[tauri::command]
fn cancel_session(session_id: String) -> Result<(), String> {
    policy::cancel_session(&session_id);
    Ok(())
}

#[tauri::command]
fn get_desktop_health() -> desktop::DesktopHealth {
    desktop::desktop_health()
}
#[tauri::command]
fn get_browser_status() -> browser::BrowserStatus {
    browser::get_status()
}

#[tauri::command]
fn get_browser_pairing_secret() -> String {
    browser::get_pairing_secret()
}

#[tauri::command]
fn laya_route(
    app: AppHandle,
    runtime: State<'_, RuntimeState>,
    endpoint: String,
    text: String,
) -> Result<serde_json::Value, String> {
    if !security::is_loopback_url(&endpoint) {
        return Err("laya endpoint must be a local loopback URL".into());
    }

    let was_listening = runtime.snapshot().listening;
    let _ = update_runtime(&app, Phase::Routing, None);
    let _ = app.emit("reflexdesk://visual-state", "thinking");

    let url = format!("{}/route", endpoint.trim_end_matches('/'));
    let mut client_builder =
        reqwest::blocking::Client::builder().timeout(Duration::from_millis(500));
    let client = client_builder.build().map_err(|e| e.to_string())?;

    let mut req = client.post(&url).json(&serde_json::json!({
        "text": text,
        "context": { "source": "reflexdesk" }
    }));

    // Inject ephemeral bearer token if supervised Laya endpoint matches
    if let Some(engine) = app.try_state::<reflex::ReflexEngine>() {
        if let Some(ep) = engine.supervisor().endpoint_snapshot() {
            if endpoint.contains(&format!(":{}", ep.port)) {
                req = req.header("Authorization", format!("Bearer {}", ep.token));
            }
        }
    }

    let result = req
        .send()
        .map_err(|e| redaction::redact_error(&e.to_string()))
        .and_then(|response| {
            response
                .json::<serde_json::Value>()
                .map_err(|e| redaction::redact_error(&e.to_string()))
        });

    let _ = update_runtime(
        &app,
        if was_listening {
            Phase::Listening
        } else {
            Phase::Ready
        },
        None,
    );
    result
}

#[tauri::command]
fn reflex_route(
    app: AppHandle,
    _runtime: State<'_, RuntimeState>,
    text: String,
    session_id: Option<String>,
) -> Result<reflex::ReflexDecision, String> {
    let settings = app.state::<SettingsState>().snapshot();
    let session = session_id.unwrap_or_else(|| "reflex_session".into());
    let ctx = reflex::ReflexContext::new(&text, &session, None, Some(&settings.language));
    if let Some(engine) = app.try_state::<reflex::ReflexEngine>() {
        Ok(engine.route_command(&ctx, &settings.stt_provider))
    } else {
        let engine = reflex::ReflexEngine::new();
        Ok(engine.route_command(&ctx, "deterministic"))
    }
}

#[tauri::command]
fn get_reflex_health(app: AppHandle) -> serde_json::Value {
    if let Some(engine) = app.try_state::<reflex::ReflexEngine>() {
        let laya_health = engine.supervisor().health();
        let laya_running = engine.supervisor().child_running();
        serde_json::json!({
            "deterministic_health": true,
            "compact_health": true,
            "laya_health": laya_health,
            "laya_running": laya_running,
            "ready": true,
        })
    } else {
        serde_json::json!({
            "deterministic_health": true,
            "compact_health": true,
            "laya_health": false,
            "laya_running": false,
            "ready": true,
        })
    }
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
    let persisted_allow_online = app.state::<SettingsState>().snapshot().allow_online_ai;
    let was_listening = runtime.snapshot().listening;
    let _ = update_runtime(&app, Phase::Routing, None);
    let _ = app.emit("reflexdesk://visual-state", "thinking");

    let secret_bytes = app
        .state::<SettingsState>()
        .snapshot()
        .planner_secret_ref
        .as_ref()
        .and_then(|sref| {
            app.state::<secrets::AppSecretStore>()
                .0
                .get(sref)
                .ok()
                .map(std::sync::Arc::new)
        });

    let mut planner_adapter =
        planner::OpenAiCompatibleLocalPlanner::new(endpoint, model, persisted_allow_online);
    if let Some(key) = secret_bytes {
        planner_adapter = planner_adapter.with_api_key(Some(key));
    }
    let ctx = planner::PlannerContext::default();
    let req = planner::PlannerRequest::new("planner_route", text, allow_remote);

    let result = (|| -> Result<serde_json::Value, String> {
        use planner::LocalPlanner;
        let envelopes = planner_adapter.plan(&ctx, &req)?;
        if let Some(env) = envelopes.first() {
            Ok(serde_json::json!({
                "action": env.tool,
                "args": env.args,
            }))
        } else {
            Ok(serde_json::json!({
                "action": "unknown",
                "args": {},
            }))
        }
    })();

    let _ = update_runtime(
        &app,
        if was_listening {
            Phase::Listening
        } else {
            Phase::Ready
        },
        None,
    );
    result
}

#[tauri::command]
fn planner_health(app: AppHandle) -> bool {
    let settings = app.state::<SettingsState>().snapshot();
    let secret_bytes = settings.planner_secret_ref.as_ref().and_then(|sref| {
        app.state::<secrets::AppSecretStore>()
            .0
            .get(sref)
            .ok()
            .map(std::sync::Arc::new)
    });
    let service = planner::PlannerService::new_with_secret(&settings, secret_bytes);
    service.health()
}

#[tauri::command]
fn planner_status(app: AppHandle) -> planner::PlannerStatus {
    let settings = app.state::<SettingsState>().snapshot();
    let service = planner::PlannerService::new(&settings);
    service.status(&settings)
}

#[tauri::command]
fn detect_harnesses() -> Vec<harness::HarnessStatus> {
    harness::detect_all()
}

#[tauri::command]
fn connect_provider(
    app: AppHandle,
    provider: String,
    api_key: String,
) -> Result<secrets::ProviderConnectionStatus, String> {
    if api_key.trim().is_empty() {
        return Err("API key cannot be empty".into());
    }

    let secret_store = app.state::<secrets::AppSecretStore>();
    let sref = secret_store.0.set(&provider, api_key.as_bytes())?;

    let settings_state = app.state::<SettingsState>();
    let mut current = settings_state.snapshot();
    current.planner_secret_ref = Some(sref.clone());
    settings::save(&app, &current)?;
    settings_state.replace(current.clone())?;

    let _ = app.emit("reflexdesk://settings", &current);

    let secret_bytes = secret_store.0.get(&sref)?;
    let status = secrets::test_provider_connection(
        &current.planner_endpoint,
        &secret_bytes,
        current.allow_online_ai,
    );

    Ok(secrets::ProviderConnectionStatus {
        provider,
        connected: status.last_status != "auth-invalid",
        secret_id: Some(sref.id),
        updated_at_ts: Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        ),
        last_status: status.last_status,
        message: status.message,
    })
}

#[tauri::command]
fn test_provider(
    app: AppHandle,
    provider: String,
) -> Result<secrets::ProviderConnectionStatus, String> {
    let settings = app.state::<SettingsState>().snapshot();
    let sref = match &settings.planner_secret_ref {
        Some(r) => r.clone(),
        None => {
            return Ok(secrets::ProviderConnectionStatus {
                provider,
                connected: false,
                secret_id: None,
                updated_at_ts: None,
                last_status: "disconnected".into(),
                message: "No provider credentials configured in SecretStore".into(),
            });
        }
    };

    let secret_store = app.state::<secrets::AppSecretStore>();
    let secret_bytes = secret_store.0.get(&sref)?;
    let status = secrets::test_provider_connection(
        &settings.planner_endpoint,
        &secret_bytes,
        settings.allow_online_ai,
    );

    Ok(secrets::ProviderConnectionStatus {
        provider,
        connected: status.last_status != "auth-invalid",
        secret_id: Some(sref.id),
        updated_at_ts: Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        ),
        last_status: status.last_status,
        message: status.message,
    })
}

#[tauri::command]
fn disconnect_provider(
    app: AppHandle,
    provider: String,
) -> Result<secrets::ProviderConnectionStatus, String> {
    let settings_state = app.state::<SettingsState>();
    let mut current = settings_state.snapshot();

    if let Some(sref) = current.planner_secret_ref.take() {
        let secret_store = app.state::<secrets::AppSecretStore>();
        let _ = secret_store.0.delete(&sref);
        settings::save(&app, &current)?;
        settings_state.replace(current.clone())?;
        let _ = app.emit("reflexdesk://settings", &current);
    }

    Ok(secrets::ProviderConnectionStatus {
        provider,
        connected: false,
        secret_id: None,
        updated_at_ts: None,
        last_status: "disconnected".into(),
        message: "Provider disconnected and secret deleted from vault".into(),
    })
}

#[tauri::command]
fn get_provider_status(
    app: AppHandle,
    provider: String,
) -> Result<secrets::ProviderConnectionStatus, String> {
    let settings = app.state::<SettingsState>().snapshot();
    let secret_id = settings.planner_secret_ref.as_ref().map(|r| r.id.clone());
    let connected = secret_id.is_some();

    Ok(secrets::ProviderConnectionStatus {
        provider,
        connected,
        secret_id,
        updated_at_ts: None,
        last_status: if connected {
            "connected".into()
        } else {
            "disconnected".into()
        },
        message: if connected {
            "Provider credentials configured in vault".into()
        } else {
            "No credentials stored".into()
        },
    })
}

#[tauri::command]
fn list_secret_metadata(app: AppHandle) -> Result<Vec<secrets::SecretMetadata>, String> {
    let secret_store = app.state::<secrets::AppSecretStore>();
    secret_store.0.list_metadata()
}
#[tauri::command]
fn stt_status(app: AppHandle, state: State<'_, stt::SttState>) -> stt::SttStatus {
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
    // IPC payload cap + 16 kHz allowlist + 100 ms–30 s bounds + language enum,
    // enforced before the sample buffer is processed further.
    security::check_transcript_bounds(samples.len() * 2, sample_rate, &language)?;
    let was_listening = runtime.snapshot().listening;
    let _ = update_runtime(&app, Phase::Transcribing, None);
    let _ = app.emit("reflexdesk://visual-state", "transcribing");
    let result = stt::transcribe(&app, &state, samples, sample_rate, language);

    let _ = update_runtime(
        &app,
        if was_listening {
            Phase::Listening
        } else {
            Phase::Ready
        },
        None,
    );
    result
}

#[tauri::command]
fn stt_shutdown(state: State<'_, stt::SttState>) -> Result<(), String> {
    stt::shutdown(&state)
}

#[tauri::command]
fn stt_stream_chunk(
    app: AppHandle,
    state: State<'_, stt::SttState>,
    session_id: String,
    nonce: String,
    samples: Vec<i16>,
    sample_rate: u32,
    language: String,
    partial_hint: Option<String>,
    is_final: bool,
) -> Result<stt::StreamChunkResult, String> {
    stt::stream_chunk(
        &app,
        &state,
        session_id,
        nonce,
        samples,
        sample_rate,
        language,
        partial_hint,
        is_final,
    )
}

#[tauri::command]
fn stt_cancel_stream(
    app: AppHandle,
    state: State<'_, stt::SttState>,
    session_id: String,
) -> Result<(), String> {
    stt::cancel_stream(&app, &state, &session_id)
}

#[tauri::command]
fn stt_baseline_metrics(state: State<'_, stt::SttState>) -> stt::native::SttBaselineMetrics {
    stt::baseline_metrics(&state)
}

#[tauri::command]
fn transcript_nonce(gate: State<'_, transcript::TranscriptGate>) -> String {
    gate.issue_nonce()
}

#[tauri::command]
fn submit_transcript(
    app: AppHandle,
    gate: State<'_, transcript::TranscriptGate>,
    text: String,
    nonce: String,
    session_id: String,
    stt_latency_ms: Option<u64>,
) -> Result<transcript::VerifiedTranscript, String> {
    // Bounded input checked before further work (fail closed on oversize).
    if text.len() > transcript::MAX_TRANSCRIPT_CHARS * 4 {
        return Err("invalid-args: transcript payload too large".into());
    }
    gate.submit(&app, &text, &nonce, &session_id, stt_latency_ms)
}
#[tauri::command]
fn open_settings(app: AppHandle) {
    show_main(&app);
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    shutdown_and_exit(&app);
}
#[tauri::command]
fn get_diagnostics_preview(app: AppHandle) -> Result<observability::DiagnosticsBundle, String> {
    Ok(observability::get_diagnostics_preview(&app))
}

#[tauri::command]
fn export_diagnostics(
    app: AppHandle,
    path: Option<String>,
) -> Result<observability::DiagnosticsBundle, String> {
    observability::export_diagnostics(&app, path.map(std::path::PathBuf::from))
}

// ---------------------------------------------------------------------------
// Skills Automation Commands (Plan 18)
// ---------------------------------------------------------------------------

#[tauri::command]
fn list_skills() -> Result<Vec<skills::SkillSummary>, String> {
    Ok(skills::global_skill_store()
        .list()
        .iter()
        .map(skills::SkillSummary::from)
        .collect())
}

#[tauri::command]
fn get_skill(id: String) -> Result<skills::SkillDefinition, String> {
    skills::global_skill_store()
        .get(&id)
        .ok_or_else(|| format!("skill '{id}' not found"))
}

#[tauri::command]
fn save_skill(skill: serde_json::Value) -> Result<skills::SkillDefinition, String> {
    let validated = skills::validate_skill_definition(&skill)?;
    skills::global_skill_store().save(validated.clone())?;
    Ok(validated)
}

#[tauri::command]
fn delete_skill(id: String) -> Result<bool, String> {
    skills::global_skill_store().delete(&id)
}

#[tauri::command]
fn execute_skill(
    app: AppHandle,
    runtime: State<'_, RuntimeState>,
    supervisor: State<'_, ProcessSupervisor>,
    settings_state: State<'_, SettingsState>,
    id: String,
    inputs: Option<std::collections::HashMap<String, serde_json::Value>>,
    session_id: Option<String>,
) -> Result<skills::SkillExecutionResult, String> {
    let skill = skills::global_skill_store()
        .get(&id)
        .ok_or_else(|| format!("skill '{id}' not found"))?;
    let sess =
        session_id.unwrap_or_else(|| format!("skill_exec_{}", policy::new_confirmation_id()));
    let empty_inputs = std::collections::HashMap::new();
    let user_inputs = inputs.as_ref().unwrap_or(&empty_inputs);
    let settings = settings_state.snapshot();

    let runner = |envelope: &policy::ActionEnvelope| -> policy::ActionExecutionResult {
        execute_verified(&app, &runtime, &supervisor, &settings, envelope, None)
    };

    Ok(skills::execute_skill(&skill, user_inputs, &sess, &runner))
}

#[tauri::command]
fn set_skill_enabled(id: String, enabled: bool) -> Result<(), String> {
    skills::global_skill_store().set_enabled(&id, enabled)
}

#[tauri::command]
fn import_skill(json_str: String, trusted: bool) -> Result<skills::SkillDefinition, String> {
    skills::import_skill_bundle(&json_str, trusted)
}

#[tauri::command]
fn export_skill(id: String) -> Result<String, String> {
    skills::export_skill_bundle(&id)
}

#[tauri::command]
fn start_skill_recording() -> Result<(), String> {
    skills::global_recorder().start();
    Ok(())
}

#[tauri::command]
fn stop_skill_recording() -> Result<usize, String> {
    let trace = skills::global_recorder().stop();
    Ok(trace.len())
}

#[tauri::command]
fn compile_skill_draft(id: String, name: String) -> Result<skills::SkillDefinition, String> {
    skills::global_recorder().compile_draft(&id, &name)
}

#[tauri::command]
fn get_skill_recording_status() -> Result<bool, String> {
    Ok(skills::global_recorder().is_recording())
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
        .manage(transcript::TranscriptGate::default())
        .manage(reflex::ReflexEngine::new())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state != ShortcutState::Pressed {
                        return;
                    }
                    let current = app.state::<RuntimeState>().snapshot();
                    if current.listening {
                        policy::cancel_now("hotkey-cancelled");
                    }
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
            let model_mgr = ModelManager::default_manager()
                .map_err(|e| format!("failed to initialize ModelManager: {e}"))?;
            app.manage(model_mgr);

            let vault_dir = app
                .path()
                .app_config_dir()
                .map(|p| p.join("vault"))
                .unwrap_or_else(|_| std::path::PathBuf::from("vault"));
            let vault_store = secrets::OsVaultSecretStore::new(vault_dir)
                .map_err(|e| format!("failed to initialize OsVaultSecretStore: {e}"))?;
            app.manage(secrets::AppSecretStore::new(std::sync::Arc::new(
                vault_store,
            )));
            if let Err(error) = browser::initialize_pairing_secret() {
                eprintln!(
                    "Browser pairing disabled: {}",
                    redaction::redact_error(&error)
                );
                let _ = browser::start_bridge();
            } else if let Err(error) = browser::start_bridge() {
                eprintln!(
                    "Browser bridge disabled: {}",
                    redaction::redact_error(&error)
                );
            }
            let app_data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let updater_service = updater::UpdaterService::new(app_data_dir);
            app.manage(updater_service);

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
                Err(_) => {
                    loaded_settings.shortcut = "Unavailable (shortcut conflict)".into();
                    let _ = settings::save(app.handle(), &loaded_settings);
                    let state = app.state::<SettingsState>();
                    let _ = state.replace(loaded_settings.clone());
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
                if loaded_settings.stt_provider == "nemotron" {
                    start_engine_background(app.handle().clone());
                } else {
                    let _ = update_runtime(app.handle(), Phase::Ready, None);
                }
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
            get_model_status,
            get_model_cache_size,
            repair_model,
            get_system_profile,
            set_listening,
            toggle_listening,
            request_action,
            confirm_action,
            cancel_session,
            get_desktop_health,
            get_browser_status,
            get_browser_pairing_secret,
            laya_route,
            reflex_route,
            get_reflex_health,
            planner_route,
            planner_health,
            planner_status,
            detect_harnesses,
            stt_status,
            stt_transcribe,
            stt_shutdown,
            transcript_nonce,
            submit_transcript,
            open_settings,
            stt_stream_chunk,
            stt_cancel_stream,
            stt_baseline_metrics,
            get_diagnostics_preview,
            export_diagnostics,
            quit_app,
            get_hardware_profile,
            get_benchmark_report,
            run_hardware_benchmark,
            set_backend_override,
            connect_provider,
            test_provider,
            disconnect_provider,
            get_provider_status,
            list_secret_metadata,
            execute_skill,
            list_skills,
            get_skill,
            save_skill,
            delete_skill,
            import_skill,
            export_skill,
            set_skill_enabled,
            start_skill_recording,
            stop_skill_recording,
            compile_skill_draft,
            get_skill_recording_status,
            updater::get_update_status,
            updater::set_update_channel,
            updater::check_for_updates,
        ])
        .run(tauri::generate_context!())
        .expect("error while running ReflexDesk");
}
