use serde::Serialize;
use std::process::Command;
use crate::process_supervisor::ProcessSupervisor;

#[derive(Serialize)]
pub struct HarnessStatus {
    pub name: &'static str,
    pub executable: &'static str,
    pub installed: bool,
}

const HARNESSES: &[(&str, &str)] = &[
    ("OpenCode", "opencode"),
    ("Kilo", "kilo"),
    ("Codex", "codex"),
    ("Claude Code", "claude"),
    ("Gemini", "gemini"),
];

fn command_exists(cmd: &str) -> bool {
    #[cfg(target_os = "windows")]
    let result = Command::new("where").arg(cmd).output();
    #[cfg(not(target_os = "windows"))]
    let result = Command::new("which").arg(cmd).output();
    result.map(|o| o.status.success()).unwrap_or(false)
}

pub fn detect_all() -> Vec<HarnessStatus> {
    HARNESSES.iter()
        .map(|(name, exe)| HarnessStatus { name, executable: exe, installed: command_exists(exe) })
        .collect()
}

pub fn launch(harness: &str, prompt: &str, cwd: Option<&str>, supervisor: &ProcessSupervisor) -> Result<(), String> {
    let exe = match harness.to_lowercase().as_str() {
        "opencode" => "opencode",
        "kilo" => "kilo",
        "codex" => "codex",
        "claude" | "claude code" => "claude",
        "gemini" => "gemini",
        _ => return Err("unsupported harness".into()),
    };
    if !command_exists(exe) {
        return Err(format!("{exe} is not installed or not on PATH"));
    }

    let mut child = Command::new(exe);
    if let Some(dir) = cwd.filter(|v| !v.trim().is_empty()) {
        child.current_dir(dir);
    }

    // Only Codex prompt injection is enabled in v0.1 because its structured
    // CLI contract is verified. Other harnesses launch conservatively until
    // their ACP/SDK adapter is implemented.
    if !prompt.is_empty() && exe == "codex" {
        child.arg("exec").arg(prompt);
    }
    let child = child.spawn().map_err(|e| e.to_string())?;
    let label = format!("harness:{exe}:{}", child.id());
    supervisor.track(label, child)
}
