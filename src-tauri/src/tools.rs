use serde::Serialize;
use serde_json::Value;
use std::process::Command;
use crate::process_supervisor::ProcessSupervisor;

#[derive(Serialize)]
pub struct ToolResult {
    pub ok: bool,
    pub message: String,
}

fn spawn_detached(program: &str, args: &[&str]) -> Result<(), String> {
    Command::new(program).args(args).spawn().map(|_| ()).map_err(|e| e.to_string())
}

fn open_external(target: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    { return spawn_detached("cmd", &["/C", "start", "", target]); }
    #[cfg(target_os = "macos")]
    { return spawn_detached("open", &[target]); }
    #[cfg(target_os = "linux")]
    { return spawn_detached("xdg-open", &[target]); }
    #[allow(unreachable_code)]
    Err("unsupported platform".into())
}

fn open_app(app: &str) -> Result<(), String> {
    let key = app.to_lowercase();

    #[cfg(target_os = "windows")]
    {
        let command = match key.as_str() {
            "spotify" => "spotify.exe",
            "chrome" => "chrome.exe",
            "firefox" => "firefox.exe",
            "vscode" => "code.cmd",
            "terminal" => "wt.exe",
            "notepad" => "notepad.exe",
            _ => return Err(format!("unknown app alias: {app}")),
        };
        return spawn_detached(command, &[]);
    }

    #[cfg(target_os = "macos")]
    {
        let bundle = match key.as_str() {
            "spotify" => "Spotify",
            "chrome" => "Google Chrome",
            "firefox" => "Firefox",
            "vscode" => "Visual Studio Code",
            "terminal" => "Terminal",
            "notepad" => "TextEdit",
            _ => return Err(format!("unknown app alias: {app}")),
        };
        return spawn_detached("open", &["-a", bundle]);
    }

    #[cfg(target_os = "linux")]
    {
        let command = match key.as_str() {
            "spotify" => "spotify",
            "chrome" => "google-chrome",
            "firefox" => "firefox",
            "vscode" => "code",
            "terminal" => "x-terminal-emulator",
            "notepad" => "gedit",
            _ => return Err(format!("unknown app alias: {app}")),
        };
        return spawn_detached(command, &[]);
    }

    #[allow(unreachable_code)]
    Err("unsupported platform".into())
}

pub fn execute(name: &str, args: &Value, supervisor: &ProcessSupervisor) -> Result<ToolResult, String> {
    match name {
        "app.open" => {
            let app = args.get("app").and_then(Value::as_str).ok_or("missing app")?;
            open_app(app)?;
            Ok(ToolResult { ok: true, message: format!("Opened {app}.") })
        }
        "browser.open" => {
            let url = args.get("url").and_then(Value::as_str).ok_or("missing url")?;
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err("only http/https URLs are allowed".into());
            }
            open_external(url)?;
            Ok(ToolResult { ok: true, message: "Opened in your default browser.".into() })
        }
        "browser.search" => {
            let query = args.get("query").and_then(Value::as_str).ok_or("missing query")?;
            let url = format!("https://www.google.com/search?q={}", urlencoding::encode(query));
            open_external(&url)?;
            Ok(ToolResult { ok: true, message: format!("Searching for {query}.") })
        }
        "harness.start" => {
            let harness = args.get("harness").and_then(Value::as_str).ok_or("missing harness")?;
            let prompt = args.get("prompt").and_then(Value::as_str).unwrap_or("");
            let cwd = args.get("cwd").and_then(Value::as_str);
            super::harness::launch(harness, prompt, cwd, supervisor)?;
            Ok(ToolResult { ok: true, message: format!("Started {harness}.") })
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}
