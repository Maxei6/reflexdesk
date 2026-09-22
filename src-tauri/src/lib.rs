mod harness;
mod stt;
mod tools;

use serde::Serialize;
use std::sync::Mutex;
use tauri::{Emitter, Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

#[derive(Default)]
struct RuntimeState {
    active: Mutex<bool>,
}

#[derive(Serialize)]
struct Status {
    active: bool,
    mode: &'static str,
}

fn apply_listening(app: &tauri::AppHandle, state: &RuntimeState, next: bool) -> Result<Status, String> {
    *state.active.lock().map_err(|_| "state lock poisoned")? = next;

    if let Some(overlay) = app.get_webview_window("overlay") {
        if next {
            overlay.show().map_err(|e| e.to_string())?;
        } else {
            overlay.hide().map_err(|e| e.to_string())?;
        }
    }

    app.emit("reflexdesk://active", next).map_err(|e| e.to_string())?;
    Ok(Status { active: next, mode: "offline" })
}

#[tauri::command]
fn get_status(state: State<'_, RuntimeState>) -> Result<Status, String> {
    let active = *state.active.lock().map_err(|_| "state lock poisoned")?;
    Ok(Status { active, mode: "offline" })
}

#[tauri::command]
fn set_listening(
    app: tauri::AppHandle,
    state: State<'_, RuntimeState>,
    active: bool,
) -> Result<Status, String> {
    apply_listening(&app, &state, active)
}

#[tauri::command]
fn toggle_listening(
    app: tauri::AppHandle,
    state: State<'_, RuntimeState>,
) -> Result<Status, String> {
    let next = !*state.active.lock().map_err(|_| "state lock poisoned")?;
    apply_listening(&app, &state, next)
}

#[tauri::command]
fn execute_tool(
    app: tauri::AppHandle,
    name: String,
    args: serde_json::Value,
) -> Result<tools::ToolResult, String> {
    app.emit("reflexdesk://working", true).ok();
    let result = tools::execute(&name, &args);
    app.emit("reflexdesk://working", false).ok();
    result
}

#[tauri::command]
fn laya_route(endpoint: String, text: String) -> Result<serde_json::Value, String> {
    if !(endpoint.starts_with("http://127.0.0.1") || endpoint.starts_with("http://localhost")) {
        return Err("Laya endpoint must be localhost".into());
    }

    let url = format!("{}/route", endpoint.trim_end_matches('/'));
    let response = reqwest::blocking::Client::new()
        .post(url)
        .json(&serde_json::json!({
            "text": text,
            "context": { "source": "reflexdesk" }
        }))
        .send()
        .map_err(|e| e.to_string())?;

    response
        .json::<serde_json::Value>()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn planner_route(
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
            .and_then(|v| v.as_str())
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

    let response = request.send().map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("planner returned HTTP {}", response.status()));
    }

    let value = response
        .json::<serde_json::Value>()
        .map_err(|e| e.to_string())?;

    let content = value
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .ok_or("planner returned no content")?;

    let cleaned = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    serde_json::from_str(cleaned)
        .map_err(|e| format!("invalid planner JSON: {e}"))
}

#[tauri::command]
fn detect_harnesses() -> Vec<harness::HarnessStatus> {
    harness::detect_all()
}

#[tauri::command]
fn stt_status(
    app: tauri::AppHandle,
    state: State<'_, stt::SttState>,
) -> stt::SttStatus {
    stt::status(&app, &state)
}

#[tauri::command]
fn stt_start(
    app: tauri::AppHandle,
    state: State<'_, stt::SttState>,
) -> Result<stt::SttStatus, String> {
    stt::start(&app, &state)
}

#[tauri::command]
fn stt_transcribe(
    app: tauri::AppHandle,
    state: State<'_, stt::SttState>,
    samples: Vec<i16>,
    sample_rate: u32,
    language: String,
) -> Result<stt::Transcript, String> {
    stt::transcribe(&app, &state, samples, sample_rate, language)
}

#[tauri::command]
fn stt_shutdown(state: State<'_, stt::SttState>) -> Result<(), String> {
    stt::shutdown(&state)
}

pub fn run() {
    tauri::Builder::default()
        .manage(RuntimeState::default())
        .manage(stt::SttState::default())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state != ShortcutState::Pressed {
                        return;
                    }
                    let state = app.state::<RuntimeState>();
                    let current = match state.active.lock() {
                        Ok(guard) => *guard,
                        Err(_) => return,
                    };
                    let _ = apply_listening(app, &state, !current);
                })
                .build(),
        )
        .setup(|app| {
            app.global_shortcut()
                .register("CommandOrControl+Shift+Space")?;
            if let Some(overlay) = app.get_webview_window("overlay") {
                let _ = overlay.hide();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            set_listening,
            toggle_listening,
            execute_tool,
            laya_route,
            planner_route,
            detect_harnesses,
            stt_status,
            stt_start,
            stt_transcribe,
            stt_shutdown
        ])
        .run(tauri::generate_context!())
        .expect("error while running ReflexDesk");
}
