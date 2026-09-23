use crate::process_supervisor::{OwnershipClass, ProcessSpec, ProcessSupervisor};
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
pub struct ToolResult {
    pub ok: bool,
    pub message: String,
}

fn open_app(app: &str, supervisor: &ProcessSupervisor) -> Result<(), String> {
    let key = app.to_lowercase();

    #[cfg(target_os = "windows")]
    let (program, args): (&str, Vec<String>) = (
        match key.as_str() {
            "spotify" => "spotify.exe",
            "chrome" => "chrome.exe",
            "firefox" => "firefox.exe",
            "vscode" => "code.cmd",
            "terminal" => "wt.exe",
            "notepad" => "notepad.exe",
            _ => return Err(format!("unknown app alias: {app}")),
        },
        Vec::new(),
    );

    #[cfg(target_os = "macos")]
    let (program, args): (&str, Vec<String>) = (
        "open",
        vec![
            "-a".into(),
            match key.as_str() {
                "spotify" => "Spotify",
                "chrome" => "Google Chrome",
                "firefox" => "Firefox",
                "vscode" => "Visual Studio Code",
                "terminal" => "Terminal",
                "notepad" => "TextEdit",
                _ => return Err(format!("unknown app alias: {app}")),
            }
            .into(),
        ],
    );

    #[cfg(target_os = "linux")]
    let (program, args): (&str, Vec<String>) = (
        match key.as_str() {
            "spotify" => "spotify",
            "chrome" => "google-chrome",
            "firefox" => "firefox",
            "vscode" => "code",
            "terminal" => "x-terminal-emulator",
            "notepad" => "gedit",
            _ => return Err(format!("unknown app alias: {app}")),
        },
        Vec::new(),
    );

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    return Err("unsupported platform".into());

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    {
        supervisor
            .spawn(ProcessSpec::new(program, OwnershipClass::UserApp).args(args))
            .map(|_| ())
    }
}

pub fn execute(
    name: &str,
    args: &Value,
    supervisor: &ProcessSupervisor,
    updater: &crate::updater::UpdaterService,
    model_manager: &crate::model_manager::ModelManager,
) -> Result<ToolResult, String> {
    match name {
        "app.open" => {
            let app = args
                .get("app")
                .and_then(Value::as_str)
                .ok_or("missing app")?;
            open_app(app, supervisor)?;
            Ok(ToolResult {
                ok: true,
                message: format!("Opened {app}."),
            })
        }
        "browser.open" => {
            let url = args
                .get("url")
                .and_then(Value::as_str)
                .ok_or("missing url")?;
            super::browser::open(url, false)?;
            Ok(ToolResult {
                ok: true,
                message: "Opened in your browser.".into(),
            })
        }
        "browser.search" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .ok_or("missing query")?;
            let url = format!(
                "https://www.google.com/search?q={}",
                urlencoding::encode(query)
            );
            super::browser::open(&url, false)?;
            Ok(ToolResult {
                ok: true,
                message: format!("Searching for {query}."),
            })
        }
        "browser.tabs" => {
            let tabs = super::browser::tabs()?;
            Ok(ToolResult {
                ok: true,
                message: serde_json::to_string(&tabs).map_err(|e| e.to_string())?,
            })
        }
        "browser.inspect" => {
            let tab_id = args.get("tab_id").and_then(Value::as_u64).map(|n| n as u32);
            let snap = super::browser::inspect(tab_id)?;
            Ok(ToolResult {
                ok: true,
                message: serde_json::to_string(&snap).map_err(|e| e.to_string())?,
            })
        }
        "browser.find" => {
            let tab_id = args.get("tab_id").and_then(Value::as_u64).map(|n| n as u32);
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .ok_or("missing query")?;
            let by = args.get("by").and_then(Value::as_str).unwrap_or("text");
            let elements = super::browser::find(tab_id, query, by)?;
            Ok(ToolResult {
                ok: true,
                message: serde_json::to_string(&elements).map_err(|e| e.to_string())?,
            })
        }
        "browser.click" => {
            let tab_id = args.get("tab_id").and_then(Value::as_u64).map(|n| n as u32);
            let ref_id = args
                .get("ref")
                .and_then(Value::as_str)
                .ok_or("missing ref")?;
            let res = super::browser::click(tab_id, ref_id)?;
            Ok(ToolResult {
                ok: res.success,
                message: format!("Clicked {ref_id}."),
            })
        }
        "browser.type" => {
            let tab_id = args.get("tab_id").and_then(Value::as_u64).map(|n| n as u32);
            let ref_id = args
                .get("ref")
                .and_then(Value::as_str)
                .ok_or("missing ref")?;
            let text = args
                .get("text")
                .and_then(Value::as_str)
                .ok_or("missing text")?;
            let clear = args.get("clear").and_then(Value::as_bool).unwrap_or(false);
            let submit = args.get("submit").and_then(Value::as_bool).unwrap_or(false);
            let res = super::browser::type_text(tab_id, ref_id, text, clear, submit)?;
            Ok(ToolResult {
                ok: res.success,
                message: format!("Typed text into {ref_id}."),
            })
        }
        "browser.select" => {
            let tab_id = args.get("tab_id").and_then(Value::as_u64).map(|n| n as u32);
            let ref_id = args
                .get("ref")
                .and_then(Value::as_str)
                .ok_or("missing ref")?;
            let value = args
                .get("value")
                .and_then(Value::as_str)
                .ok_or("missing value")?;
            let res = super::browser::select(tab_id, ref_id, value)?;
            Ok(ToolResult {
                ok: res.success,
                message: format!("Selected {value} in {ref_id}."),
            })
        }
        "browser.scroll" => {
            let tab_id = args.get("tab_id").and_then(Value::as_u64).map(|n| n as u32);
            let direction = args
                .get("direction")
                .and_then(Value::as_str)
                .unwrap_or("down");
            let amount = args.get("amount").and_then(Value::as_i64).unwrap_or(500) as i32;
            let res = super::browser::scroll(tab_id, direction, amount)?;
            Ok(ToolResult {
                ok: res.success,
                message: format!("Scrolled {direction}."),
            })
        }
        "browser.extract" => {
            let tab_id = args.get("tab_id").and_then(Value::as_u64).map(|n| n as u32);
            let ref_id = args.get("ref").and_then(Value::as_str);
            let format = args.get("format").and_then(Value::as_str).unwrap_or("text");
            let res = super::browser::extract(tab_id, ref_id, format)?;
            Ok(ToolResult {
                ok: res.success,
                message: res.content.unwrap_or_default(),
            })
        }
        "browser.wait" => {
            let tab_id = args.get("tab_id").and_then(Value::as_u64).map(|n| n as u32);
            let selector = args
                .get("selector")
                .and_then(Value::as_str)
                .ok_or("missing selector")?;
            let condition = args
                .get("condition")
                .and_then(Value::as_str)
                .unwrap_or("visible");
            let timeout_ms = args
                .get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(5000);
            let res = super::browser::wait(tab_id, selector, condition, timeout_ms)?;
            Ok(ToolResult {
                ok: res.success,
                message: format!("Waited for {selector} ({condition})."),
            })
        }
        "browser.download" => {
            let url = args
                .get("url")
                .and_then(Value::as_str)
                .ok_or("missing url")?;
            let filename = args.get("filename").and_then(Value::as_str);
            let res = super::browser::download(url, filename)?;
            Ok(ToolResult {
                ok: res.success,
                message: format!("Download initiated: {:?}.", res.filename),
            })
        }
        "browser.verify" => {
            let contract: crate::policy::VerificationContract =
                serde_json::from_value(args.clone()).map_err(|e| {
                    format!("invalid-args: browser.verify requires verification contract: {e}")
                })?;
            let res = super::browser::verify_action(&contract)?;
            Ok(ToolResult {
                ok: res.success,
                message: "Verified.".into(),
            })
        }
        "harness.start" => {
            let harness = args
                .get("harness")
                .and_then(Value::as_str)
                .ok_or("missing harness")?;
            let prompt = args.get("prompt").and_then(Value::as_str).unwrap_or("");
            let cwd = args.get("cwd").and_then(Value::as_str);
            super::harness::launch(harness, prompt, cwd, supervisor)?;
            Ok(ToolResult {
                ok: true,
                message: format!("Started {harness}."),
            })
        }
        "desktop.inspect" => {
            let win = args.get("window").and_then(Value::as_str);
            let sel = args
                .get("selector")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            let snap = crate::desktop::desktop_inspect(win, sel.as_ref())?;
            Ok(ToolResult {
                ok: true,
                message: serde_json::to_string(&snap).map_err(|e| e.to_string())?,
            })
        }
        "desktop.find" => {
            let sel = args.get("selector").ok_or("missing selector")?;
            let selector = serde_json::from_value(sel.clone()).map_err(|e| e.to_string())?;
            let win = args.get("window").and_then(Value::as_str);
            let elements = crate::desktop::desktop_find(&selector, win)?;
            Ok(ToolResult {
                ok: true,
                message: serde_json::to_string(&elements).map_err(|e| e.to_string())?,
            })
        }
        "desktop.focus_window" => {
            let win_id = args.get("window_id").and_then(Value::as_u64);
            let title = args.get("title_or_app").and_then(Value::as_str);
            let win = crate::desktop::desktop_focus_window(win_id, title)?;
            Ok(ToolResult {
                ok: true,
                message: format!("Focused window '{}' ({}).", win.title, win.app),
            })
        }
        "desktop.close_window" => {
            let win_id = args.get("window_id").and_then(Value::as_u64);
            let title = args.get("title_or_app").and_then(Value::as_str);
            crate::desktop::desktop_close_window(win_id, title)?;
            Ok(ToolResult {
                ok: true,
                message: "Closed window successfully.".into(),
            })
        }
        "desktop.invoke" => {
            let el_id = args.get("element_id").and_then(Value::as_u64);
            let sel = args
                .get("selector")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            let action = args.get("action").and_then(Value::as_str);
            let res = crate::desktop::desktop_invoke(el_id, sel.as_ref(), action)?;
            Ok(ToolResult {
                ok: true,
                message: serde_json::to_string(&res).map_err(|e| e.to_string())?,
            })
        }
        "desktop.click" => {
            let el_id = args.get("element_id").and_then(Value::as_u64);
            let sel = args
                .get("selector")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            let res = crate::desktop::desktop_click(el_id, sel.as_ref())?;
            Ok(ToolResult {
                ok: true,
                message: serde_json::to_string(&res).map_err(|e| e.to_string())?,
            })
        }
        "desktop.type" => {
            let el_id = args.get("element_id").and_then(Value::as_u64);
            let sel = args
                .get("selector")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            let text = args
                .get("text")
                .and_then(Value::as_str)
                .ok_or("missing text")?;
            let clear_first = args
                .get("clear_first")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let res = crate::desktop::desktop_type(el_id, sel.as_ref(), text, clear_first)?;
            Ok(ToolResult {
                ok: true,
                message: serde_json::to_string(&res).map_err(|e| e.to_string())?,
            })
        }
        "desktop.press_key" => {
            let key = args
                .get("key")
                .and_then(Value::as_str)
                .ok_or("missing key")?;
            let empty_mods = Vec::new();
            let modifiers: Vec<&str> = args
                .get("modifiers")
                .and_then(Value::as_array)
                .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                .unwrap_or(empty_mods);
            let res = crate::desktop::desktop_press_key(key, &modifiers)?;
            Ok(ToolResult {
                ok: true,
                message: serde_json::to_string(&res).map_err(|e| e.to_string())?,
            })
        }
        "desktop.scroll" => {
            let el_id = args.get("element_id").and_then(Value::as_u64);
            let sel = args
                .get("selector")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            let dir = args
                .get("direction")
                .and_then(Value::as_str)
                .unwrap_or("down");
            let amount = args.get("amount").and_then(Value::as_f64).unwrap_or(1.0);
            let res = crate::desktop::desktop_scroll(el_id, sel.as_ref(), dir, amount)?;
            Ok(ToolResult {
                ok: true,
                message: serde_json::to_string(&res).map_err(|e| e.to_string())?,
            })
        }
        "desktop.read" => {
            let el_id = args.get("element_id").and_then(Value::as_u64);
            let sel = args
                .get("selector")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            let res = crate::desktop::desktop_read(el_id, sel.as_ref())?;
            Ok(ToolResult {
                ok: true,
                message: serde_json::to_string(&res).map_err(|e| e.to_string())?,
            })
        }
        "desktop.verify" => {
            let contract: crate::policy::VerificationContract =
                serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
            let res = crate::desktop::desktop_verify(&contract)?;
            Ok(ToolResult {
                ok: true,
                message: serde_json::to_string(&res).map_err(|e| e.to_string())?,
            })
        }
        "system.update_check" => {
            let channel = crate::updater::UpdateChannel::parse_channel(
                args.get("channel")
                    .and_then(Value::as_str)
                    .unwrap_or("stable"),
            )?;
            updater.set_channel(channel);
            let status = updater.check_now(model_manager)?;
            Ok(ToolResult {
                ok: !matches!(
                    status,
                    crate::updater::UpdateStatus::Blocked { .. }
                        | crate::updater::UpdateStatus::Failed { .. }
                ),
                message: serde_json::to_string(&status).map_err(|e| e.to_string())?,
            })
        }
        "system.update_apply" => {
            let target_version = args
                .get("target_version")
                .and_then(Value::as_str)
                .ok_or("missing target_version")?;
            updater.stage_available(target_version, model_manager)?;
            let activation = updater.activate_staged(target_version, model_manager)?;
            Ok(ToolResult {
                ok: true,
                message: serde_json::to_string(&activation).map_err(|e| e.to_string())?,
            })
        }
        "system.update_rollback" => {
            let rollback = updater.schedule_rollback()?;
            Ok(ToolResult {
                ok: true,
                message: serde_json::to_string(&rollback).map_err(|e| e.to_string())?,
            })
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}
