//! Wave-0 frozen contracts: policy gate, tool registry surface, cancel token,
//! confirmation correlation IDs. Field names and command names in
//! `local://contracts.md` are frozen; later waves consume verbatim, never rename.
//!
//! Wave-1 owns the full registry validators + request_action/confirm flow.
//! This module provides the types plus a compiling baseline implementation.

use crate::settings::AppSettings;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
// ---------------------------------------------------------------------------
// Frozen enums (serde renames are part of the contract)
// ---------------------------------------------------------------------------

/// Risk classification. `EXTERNAL_COMMIT` from plan 01 maps to
/// `Destructive` + `external:true` in `schemas/tool.schema.json`; in Rust it
/// is the explicit `ExternalCommit` variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    Safe,
    Sensitive,
    Destructive,
    ExternalCommit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyOutcome {
    Allow,
    Deny,
    Confirm,
    RequireUnlock,
}

impl PolicyOutcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            PolicyOutcome::Allow => "allow",
            PolicyOutcome::Deny => "deny",
            PolicyOutcome::Confirm => "confirm",
            PolicyOutcome::RequireUnlock => "require-unlock",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionSource {
    Reflex,
    Planner,
    Harness,
    User,
}

// ---------------------------------------------------------------------------
// Frozen structs (field names frozen by contracts.md)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerificationContract {
    pub kind: String,
    pub selector: Option<String>,
    pub expect: serde_json::Value,
    pub timeout_ms: u64,
}

impl Default for VerificationContract {
    fn default() -> Self {
        Self {
            kind: "none".into(),
            selector: None,
            expect: serde_json::Value::Null,
            timeout_ms: 2_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionEnvelope {
    pub tool: String,
    pub args: serde_json::Value,
    pub source: ActionSource,
    pub session_id: String,
    pub risk: RiskClass,
    pub capability: String,
    pub verification: VerificationContract,
}

// ---------------------------------------------------------------------------
// Tool registry surface (Wave-1 fills validators; shape frozen here)
// ---------------------------------------------------------------------------

/// Per-tool metadata. `risk` mirrors `schemas/tool.schema.json`
/// (`safe|sensitive|destructive` plus `external:true` → `ExternalCommit`).
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: &'static str,
    pub risk: RiskClass,
    pub capability: &'static str,
    pub needs_confirm: bool,
    pub external: bool,
}

/// Central registry. Wave-1 adds JSON-schema-equivalent validators per tool;
/// the four entries below match the current `tools::execute` surface.
pub fn tool_registry() -> HashMap<&'static str, ToolDef> {
    let mut map = HashMap::new();
    for def in [
        ToolDef {
            name: "app.open",
            risk: RiskClass::Sensitive,
            capability: "desktop.launch",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "browser.open",
            risk: RiskClass::Sensitive,
            capability: "browser.open",
            needs_confirm: false,
            external: true,
        },
        ToolDef {
            name: "browser.search",
            risk: RiskClass::Safe,
            capability: "browser.open",
            needs_confirm: false,
            external: true,
        },
        ToolDef {
            name: "browser.tabs",
            risk: RiskClass::Safe,
            capability: "browser.read",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "browser.inspect",
            risk: RiskClass::Safe,
            capability: "browser.read",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "browser.find",
            risk: RiskClass::Safe,
            capability: "browser.read",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "browser.click",
            risk: RiskClass::Sensitive,
            capability: "browser.control",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "browser.type",
            risk: RiskClass::Sensitive,
            capability: "browser.control",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "browser.select",
            risk: RiskClass::Sensitive,
            capability: "browser.control",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "browser.scroll",
            risk: RiskClass::Safe,
            capability: "browser.control",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "browser.extract",
            risk: RiskClass::Safe,
            capability: "browser.read",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "browser.wait",
            risk: RiskClass::Safe,
            capability: "browser.read",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "browser.download",
            risk: RiskClass::Sensitive,
            capability: "browser.control",
            needs_confirm: false,
            external: true,
        },
        ToolDef {
            name: "browser.verify",
            risk: RiskClass::Safe,
            capability: "browser.read",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "harness.start",
            risk: RiskClass::ExternalCommit,
            capability: "harness.start",
            needs_confirm: true,
            external: true,
        },
        ToolDef {
            name: "desktop.inspect",
            risk: RiskClass::Safe,
            capability: "desktop.inspect",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "desktop.find",
            risk: RiskClass::Safe,
            capability: "desktop.inspect",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "desktop.read",
            risk: RiskClass::Safe,
            capability: "desktop.inspect",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "desktop.verify",
            risk: RiskClass::Safe,
            capability: "desktop.inspect",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "desktop.focus_window",
            risk: RiskClass::Safe,
            capability: "desktop.control",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "desktop.scroll",
            risk: RiskClass::Safe,
            capability: "desktop.control",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "desktop.click",
            risk: RiskClass::Sensitive,
            capability: "desktop.control",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "desktop.invoke",
            risk: RiskClass::Sensitive,
            capability: "desktop.control",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "desktop.type",
            risk: RiskClass::Sensitive,
            capability: "desktop.control",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "desktop.press_key",
            risk: RiskClass::Sensitive,
            capability: "desktop.control",
            needs_confirm: false,
            external: false,
        },
        ToolDef {
            name: "desktop.close_window",
            risk: RiskClass::Destructive,
            capability: "desktop.control",
            needs_confirm: true,
            external: false,
        },
        ToolDef {
            name: "system.update_check",
            risk: RiskClass::Safe,
            capability: "system.update",
            needs_confirm: false,
            external: true,
        },
        ToolDef {
            name: "system.update_apply",
            risk: RiskClass::Destructive,
            capability: "system.update",
            needs_confirm: true,
            external: true,
        },
    ] {
        map.insert(def.name, def);
    }
    map
}

// ---------------------------------------------------------------------------
// authorize: deterministic policy gate baseline
// ---------------------------------------------------------------------------

/// Baseline gate. Wave-1 extends with settings-gated remote policy and
/// confirmation bookkeeping; the outcome ordering is frozen:
///
/// - unknown tool → `Deny` (fail closed, `unknown-tool`)
/// - negated commands are denied by the caller via [`is_negated_command`]
///   (envelope carries no raw text by design)
/// - `ExternalCommit` / `Destructive` → `Confirm` (`RequireUnlock` when the
///   tool def demands it and settings require unlock)
/// - `Sensitive` → `Confirm` unless the capability is explicitly granted for
///   this session (Wave-1 session grants; Wave-0 always confirms sensitive)
/// - `Safe` → `Allow`
pub fn authorize(env: &ActionEnvelope, _settings: &AppSettings) -> PolicyOutcome {
    let registry = tool_registry();
    let def = match registry.get(env.tool.as_str()) {
        Some(def) => def,
        None => return PolicyOutcome::Deny,
    };
    match def.risk {
        RiskClass::ExternalCommit | RiskClass::Destructive => {
            if def.needs_confirm {
                PolicyOutcome::Confirm
            } else {
                PolicyOutcome::RequireUnlock
            }
        }
        RiskClass::Sensitive => PolicyOutcome::Confirm,
        RiskClass::Safe => PolicyOutcome::Allow,
    }
}

// ---------------------------------------------------------------------------
// validate_args: fail-closed typed validators (baseline; Wave-1 extends)
// ---------------------------------------------------------------------------

/// Validate tool args, returning sanitized args on success.
/// Unknown tools → `Err("unknown-tool: ...")`; malformed → `Err("invalid-args: ...")`.
pub fn validate_args(tool: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {
    match tool {
        "app.open" => {
            let app = args
                .get("app")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "invalid-args: app.open requires {app: string}".to_string())?;
            let app = app.trim();
            if app.is_empty() || app.len() > 64 {
                return Err("invalid-args: app.open app must be 1..64 chars".into());
            }
            if app.contains(['/', '\\', ':', '\0']) {
                return Err("invalid-args: app.open app must be a bare alias".into());
            }
            Ok(serde_json::json!({ "app": app.to_lowercase() }))
        }
        "browser.open" => {
            let url = args
                .get("url")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "invalid-args: browser.open requires {url: string}".to_string())?;
            let url = url.trim();
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err("invalid-args: browser.open only allows http/https URLs".into());
            }
            if url.len() > 2048 || url.contains([' ', '\0', '<', '>', '"', '\'']) {
                return Err("invalid-args: browser.open url is malformed".into());
            }
            Ok(serde_json::json!({ "url": url }))
        }
        "browser.search" => {
            let query = args
                .get("query")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    "invalid-args: browser.search requires {query: string}".to_string()
                })?;
            let query = query.trim();
            if query.is_empty() || query.len() > 500 {
                return Err("invalid-args: browser.search query must be 1..500 chars".into());
            }
            Ok(serde_json::json!({ "query": query }))
        }
        "browser.tabs" => {
            let mut out = serde_json::json!({});
            if let Some(cw) = args
                .get("current_window_only")
                .and_then(serde_json::Value::as_bool)
            {
                out["current_window_only"] = serde_json::Value::Bool(cw);
            }
            Ok(out)
        }
        "browser.inspect" => {
            let mut out = serde_json::json!({});
            if let Some(tab_id) = args.get("tab_id").and_then(serde_json::Value::as_u64) {
                out["tab_id"] = serde_json::json!(tab_id);
            }
            if let Some(depth) = args.get("max_depth").and_then(serde_json::Value::as_u64) {
                out["max_depth"] = serde_json::json!(depth);
            }
            Ok(out)
        }
        "browser.find" => {
            let query = args
                .get("query")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "invalid-args: browser.find requires {query: string}".to_string())?;
            let query = query.trim();
            if query.is_empty() || query.len() > 500 {
                return Err("invalid-args: browser.find query must be 1..500 chars".into());
            }
            let mut out = serde_json::json!({ "query": query });
            if let Some(tab_id) = args.get("tab_id").and_then(serde_json::Value::as_u64) {
                out["tab_id"] = serde_json::json!(tab_id);
            }
            if let Some(by) = args.get("by").and_then(serde_json::Value::as_str) {
                let by_norm = by.to_lowercase();
                if !["selector", "text", "role", "label"].contains(&by_norm.as_str()) {
                    return Err(
                        "invalid-args: browser.find by must be selector, text, role, or label"
                            .into(),
                    );
                }
                out["by"] = serde_json::Value::String(by_norm);
            }
            Ok(out)
        }
        "browser.click" => {
            let ref_id = args
                .get("ref")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "invalid-args: browser.click requires {ref: string}".to_string())?;
            let ref_id = ref_id.trim();
            if ref_id.is_empty() || ref_id.len() > 64 {
                return Err("invalid-args: browser.click ref must be 1..64 chars".into());
            }
            let mut out = serde_json::json!({ "ref": ref_id });
            if let Some(tab_id) = args.get("tab_id").and_then(serde_json::Value::as_u64) {
                out["tab_id"] = serde_json::json!(tab_id);
            }
            if let Some(btn) = args.get("button").and_then(serde_json::Value::as_str) {
                out["button"] = serde_json::Value::String(btn.to_lowercase());
            }
            Ok(out)
        }
        "browser.type" => {
            let ref_id = args
                .get("ref")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "invalid-args: browser.type requires {ref: string}".to_string())?;
            let text = args
                .get("text")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "invalid-args: browser.type requires {text: string}".to_string())?;
            if text.len() > 8000 {
                return Err("invalid-args: browser.type text exceeds 8000 chars".into());
            }
            let mut out = serde_json::json!({ "ref": ref_id.trim(), "text": text });
            if let Some(tab_id) = args.get("tab_id").and_then(serde_json::Value::as_u64) {
                out["tab_id"] = serde_json::json!(tab_id);
            }
            if let Some(clear) = args.get("clear").and_then(serde_json::Value::as_bool) {
                out["clear"] = serde_json::Value::Bool(clear);
            }
            if let Some(submit) = args.get("submit").and_then(serde_json::Value::as_bool) {
                out["submit"] = serde_json::Value::Bool(submit);
            }
            Ok(out)
        }
        "browser.select" => {
            let ref_id = args
                .get("ref")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "invalid-args: browser.select requires {ref: string}".to_string())?;
            let value = args
                .get("value")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    "invalid-args: browser.select requires {value: string}".to_string()
                })?;
            let mut out = serde_json::json!({ "ref": ref_id.trim(), "value": value });
            if let Some(tab_id) = args.get("tab_id").and_then(serde_json::Value::as_u64) {
                out["tab_id"] = serde_json::json!(tab_id);
            }
            Ok(out)
        }
        "browser.scroll" => {
            let mut out = serde_json::json!({});
            if let Some(tab_id) = args.get("tab_id").and_then(serde_json::Value::as_u64) {
                out["tab_id"] = serde_json::json!(tab_id);
            }
            if let Some(dir) = args.get("direction").and_then(serde_json::Value::as_str) {
                let dir_norm = dir.to_lowercase();
                if !["up", "down", "top", "bottom"].contains(&dir_norm.as_str()) {
                    return Err(
                        "invalid-args: browser.scroll direction must be up, down, top, or bottom"
                            .into(),
                    );
                }
                out["direction"] = serde_json::Value::String(dir_norm);
            }
            if let Some(amt) = args.get("amount").and_then(serde_json::Value::as_i64) {
                out["amount"] = serde_json::json!(amt);
            }
            Ok(out)
        }
        "browser.extract" => {
            let mut out = serde_json::json!({});
            if let Some(tab_id) = args.get("tab_id").and_then(serde_json::Value::as_u64) {
                out["tab_id"] = serde_json::json!(tab_id);
            }
            if let Some(r) = args.get("ref").and_then(serde_json::Value::as_str) {
                out["ref"] = serde_json::Value::String(r.trim().to_string());
            }
            if let Some(fmt) = args.get("format").and_then(serde_json::Value::as_str) {
                out["format"] = serde_json::Value::String(fmt.to_lowercase());
            }
            Ok(out)
        }
        "browser.wait" => {
            let selector = args
                .get("selector")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    "invalid-args: browser.wait requires {selector: string}".to_string()
                })?;
            let mut out = serde_json::json!({ "selector": selector.trim() });
            if let Some(tab_id) = args.get("tab_id").and_then(serde_json::Value::as_u64) {
                out["tab_id"] = serde_json::json!(tab_id);
            }
            if let Some(cond) = args.get("condition").and_then(serde_json::Value::as_str) {
                out["condition"] = serde_json::Value::String(cond.to_lowercase());
            }
            if let Some(timeout) = args.get("timeout_ms").and_then(serde_json::Value::as_u64) {
                out["timeout_ms"] = serde_json::json!(timeout);
            }
            Ok(out)
        }
        "browser.download" => {
            let url = args
                .get("url")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    "invalid-args: browser.download requires {url: string}".to_string()
                })?;
            let sanitized = crate::security::sanitize_open_external(url)?;
            let mut out = serde_json::json!({ "url": sanitized });
            if let Some(fname) = args.get("filename").and_then(serde_json::Value::as_str) {
                out["filename"] = serde_json::Value::String(fname.to_string());
            }
            Ok(out)
        }
        "browser.verify" => {
            let kind = args
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("browser.element_present");
            let mut out = serde_json::json!({ "kind": kind });
            if let Some(sel) = args.get("selector").and_then(serde_json::Value::as_str) {
                out["selector"] = serde_json::Value::String(sel.to_string());
            }
            if let Some(exp) = args.get("expect") {
                out["expect"] = exp.clone();
            }
            if let Some(timeout) = args.get("timeout_ms").and_then(serde_json::Value::as_u64) {
                out["timeout_ms"] = serde_json::json!(timeout);
            }
            Ok(out)
        }
        "harness.start" => {
            let harness = args
                .get("harness")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    "invalid-args: harness.start requires {harness: string}".to_string()
                })?;
            let key = harness.trim().to_lowercase();
            match key.as_str() {
                "opencode" | "kilo" | "codex" | "claude" | "claude code" | "gemini" => {}
                _ => return Err("invalid-args: harness.start harness is not supported".into()),
            }
            let prompt = args
                .get("prompt")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if prompt.len() > 8_000 {
                return Err("invalid-args: harness.start prompt exceeds 8000 chars".into());
            }
            let mut out = serde_json::json!({ "harness": key, "prompt": prompt });
            if let Some(cwd) = args.get("cwd").and_then(serde_json::Value::as_str) {
                if !cwd.trim().is_empty() {
                    if cwd.len() > 1024 || cwd.contains('\0') {
                        return Err("invalid-args: harness.start cwd is malformed".into());
                    }
                    out["cwd"] = serde_json::Value::String(cwd.to_string());
                }
            }
            Ok(out)
        }
        "desktop.inspect" => {
            let mut out = serde_json::json!({});
            if let Some(win) = args.get("window").and_then(serde_json::Value::as_str) {
                if win.len() > 256 {
                    return Err("invalid-args: desktop.inspect window exceeds 256 chars".into());
                }
                out["window"] = serde_json::Value::String(win.to_string());
            }
            if let Some(sel) = args.get("selector") {
                out["selector"] = sel.clone();
            }
            Ok(out)
        }
        "desktop.find" => {
            let sel = args.get("selector").ok_or_else(|| {
                "invalid-args: desktop.find requires {selector: object}".to_string()
            })?;
            let mut out = serde_json::json!({ "selector": sel });
            if let Some(win) = args.get("window").and_then(serde_json::Value::as_str) {
                if win.len() > 256 {
                    return Err("invalid-args: desktop.find window exceeds 256 chars".into());
                }
                out["window"] = serde_json::Value::String(win.to_string());
            }
            Ok(out)
        }
        "desktop.focus_window" => {
            let win_id = args.get("window_id").and_then(serde_json::Value::as_u64);
            let title = args.get("title_or_app").and_then(serde_json::Value::as_str);
            if win_id.is_none() && title.is_none() {
                return Err(
                    "invalid-args: desktop.focus_window requires window_id or title_or_app".into(),
                );
            }
            let mut out = serde_json::json!({});
            if let Some(id) = win_id {
                out["window_id"] = serde_json::Value::from(id);
            }
            if let Some(t) = title {
                if t.trim().is_empty() || t.len() > 256 {
                    return Err(
                        "invalid-args: desktop.focus_window title_or_app must be 1..256 chars"
                            .into(),
                    );
                }
                out["title_or_app"] = serde_json::Value::String(t.to_string());
            }
            Ok(out)
        }
        "desktop.close_window" => {
            let win_id = args.get("window_id").and_then(serde_json::Value::as_u64);
            let title = args.get("title_or_app").and_then(serde_json::Value::as_str);
            if win_id.is_none() && title.is_none() {
                return Err(
                    "invalid-args: desktop.close_window requires window_id or title_or_app".into(),
                );
            }
            let mut out = serde_json::json!({});
            if let Some(id) = win_id {
                out["window_id"] = serde_json::Value::from(id);
            }
            if let Some(t) = title {
                if t.trim().is_empty() || t.len() > 256 {
                    return Err(
                        "invalid-args: desktop.close_window title_or_app must be 1..256 chars"
                            .into(),
                    );
                }
                out["title_or_app"] = serde_json::Value::String(t.to_string());
            }
            Ok(out)
        }
        "desktop.click" => {
            let el_id = args.get("element_id").and_then(serde_json::Value::as_u64);
            let sel = args.get("selector");
            if el_id.is_none() && sel.is_none() {
                return Err("invalid-args: desktop.click requires element_id or selector".into());
            }
            let mut out = serde_json::json!({});
            if let Some(id) = el_id {
                out["element_id"] = serde_json::Value::from(id);
            }
            if let Some(s) = sel {
                out["selector"] = s.clone();
            }
            Ok(out)
        }
        "desktop.invoke" => {
            let el_id = args.get("element_id").and_then(serde_json::Value::as_u64);
            let sel = args.get("selector");
            if el_id.is_none() && sel.is_none() {
                return Err("invalid-args: desktop.invoke requires element_id or selector".into());
            }
            let mut out = serde_json::json!({});
            if let Some(id) = el_id {
                out["element_id"] = serde_json::Value::from(id);
            }
            if let Some(s) = sel {
                out["selector"] = s.clone();
            }
            if let Some(act) = args.get("action").and_then(serde_json::Value::as_str) {
                if act.len() > 64 {
                    return Err("invalid-args: desktop.invoke action exceeds 64 chars".into());
                }
                out["action"] = serde_json::Value::String(act.to_string());
            }
            Ok(out)
        }
        "desktop.type" => {
            let el_id = args.get("element_id").and_then(serde_json::Value::as_u64);
            let sel = args.get("selector");
            if el_id.is_none() && sel.is_none() {
                return Err("invalid-args: desktop.type requires element_id or selector".into());
            }
            let text = args
                .get("text")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "invalid-args: desktop.type requires {text: string}".to_string())?;
            if text.len() > 8000 {
                return Err("invalid-args: desktop.type text exceeds 8000 chars".into());
            }
            let mut out = serde_json::json!({ "text": text });
            if let Some(id) = el_id {
                out["element_id"] = serde_json::Value::from(id);
            }
            if let Some(s) = sel {
                out["selector"] = s.clone();
            }
            if let Some(cf) = args.get("clear_first").and_then(serde_json::Value::as_bool) {
                out["clear_first"] = serde_json::Value::Bool(cf);
            }
            Ok(out)
        }
        "desktop.press_key" => {
            let key = args
                .get("key")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    "invalid-args: desktop.press_key requires {key: string}".to_string()
                })?;
            let key = key.trim();
            if key.is_empty() || key.len() > 64 {
                return Err("invalid-args: desktop.press_key key must be 1..64 chars".into());
            }
            let mut out = serde_json::json!({ "key": key });
            if let Some(mods) = args.get("modifiers").and_then(serde_json::Value::as_array) {
                out["modifiers"] = serde_json::Value::Array(mods.clone());
            }
            Ok(out)
        }
        "desktop.scroll" => {
            let el_id = args.get("element_id").and_then(serde_json::Value::as_u64);
            let sel = args.get("selector");
            if el_id.is_none() && sel.is_none() {
                return Err("invalid-args: desktop.scroll requires element_id or selector".into());
            }
            let dir = args
                .get("direction")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("down");
            let amount = args
                .get("amount")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(1.0);
            let mut out = serde_json::json!({ "direction": dir, "amount": amount });
            if let Some(id) = el_id {
                out["element_id"] = serde_json::Value::from(id);
            }
            if let Some(s) = sel {
                out["selector"] = s.clone();
            }
            Ok(out)
        }
        "desktop.read" => {
            let el_id = args.get("element_id").and_then(serde_json::Value::as_u64);
            let sel = args.get("selector");
            if el_id.is_none() && sel.is_none() {
                return Err("invalid-args: desktop.read requires element_id or selector".into());
            }
            let mut out = serde_json::json!({});
            if let Some(id) = el_id {
                out["element_id"] = serde_json::Value::from(id);
            }
            if let Some(s) = sel {
                out["selector"] = s.clone();
            }
            Ok(out)
        }
        "desktop.verify" => {
            let kind = args
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("desktop-element-state");
            let timeout_ms = args
                .get("timeout_ms")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(2000);
            Ok(serde_json::json!({
                "kind": kind,
                "selector": args.get("selector"),
                "expect": args.get("expect").cloned().unwrap_or(serde_json::Value::Null),
                "timeout_ms": timeout_ms,
            }))
        }
        "system.update_check" => {
            let channel = args
                .get("channel")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("stable");
            Ok(serde_json::json!({ "channel": channel }))
        }
        "system.update_apply" => {
            let target_version = args
                .get("target_version")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    "invalid-args: system.update_apply requires {target_version: string}"
                        .to_string()
                })?;
            Ok(serde_json::json!({ "target_version": target_version }))
        }
        _ => Err(format!("unknown-tool: {tool}")),
    }
}

// ---------------------------------------------------------------------------
// Negation detection (deterministic, no LLM)
// ---------------------------------------------------------------------------

const LEADING_NEGATIONS: &[&str] = &[
    "don't ",
    "do not ",
    "never ",
    "stop ",
    "cancel ",
    "actually stop ",
    "do n't ",
];

/// Returns true when the raw command text is a negation / countermand.
/// Covers leading `don't|do not|never|stop|cancel|actually stop` and
/// mid-sentence `don't <verb>` / `do not <verb>` countermands.
pub fn is_negated_command(text: &str) -> bool {
    let lower = text.to_lowercase();
    let trimmed = lower.trim_start_matches(|c: char| {
        c.is_whitespace() || c == '"' || c == '\'' || c == '(' || c == '['
    });
    for prefix in LEADING_NEGATIONS {
        if trimmed.starts_with(prefix) {
            return true;
        }
    }
    if trimmed == "don't"
        || trimmed == "do not"
        || trimmed == "stop"
        || trimmed == "cancel"
        || trimmed == "never"
    {
        return true;
    }
    // Mid-sentence countermand: "actually stop", "don't <verb>", "do not <verb>".
    trimmed.contains("actually stop")
        || trimmed.contains("don't ")
        || trimmed.contains("do not ")
        || trimmed.contains("do n't ")
}

// ---------------------------------------------------------------------------
// Cancel token: global generation counter + per-session flags
// ---------------------------------------------------------------------------

static GLOBAL_GENERATION: AtomicU64 = AtomicU64::new(0);
static CANCELLED_SESSIONS: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static CANCEL_REASON: Mutex<Option<String>> = Mutex::new(None);

fn cancelled_set() -> std::sync::MutexGuard<'static, Option<HashSet<String>>> {
    CANCELLED_SESSIONS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Global cancel: bumps the generation counter, marks every known session
/// cancelled, records the reason. Stops capture, aborts the current action,
/// and interrupts owned harness sessions (callers wire the latter in Wave-1).
pub fn cancel_now(reason: &str) {
    GLOBAL_GENERATION.fetch_add(1, Ordering::SeqCst);
    let mut set = cancelled_set();
    let known: Vec<String> = set
        .as_ref()
        .map(|s| s.iter().cloned().collect())
        .unwrap_or_default();
    let entry = set.get_or_insert_with(HashSet::new);
    for session in known {
        entry.insert(session);
    }
    // A bare global-cancel sentinel so sessions created after the cancel but
    // before the next reset are still observable via generation().
    entry.insert(String::new());
    if let Ok(mut slot) = CANCEL_REASON.lock() {
        *slot = Some(reason.to_string());
    }
}

/// Cancel one session without touching others.
pub fn cancel_session(session_id: &str) {
    cancelled_set()
        .get_or_insert_with(HashSet::new)
        .insert(session_id.to_string());
}

/// Returns true when the session (or a global cancel) revoked it.
pub fn is_cancelled(session_id: &str) -> bool {
    let set = cancelled_set();
    match set.as_ref() {
        Some(s) => s.contains(session_id) || s.contains(""),
        None => false,
    }
}

/// Current global-cancel generation (for Wave-1 delayed-completion re-checks).
pub fn cancel_generation() -> u64 {
    GLOBAL_GENERATION.load(Ordering::SeqCst)
}

/// Forget one session's cancel flag (session teardown).
pub fn clear_session(session_id: &str) {
    if let Some(set) = cancelled_set().as_mut() {
        set.remove(session_id);
    }
}

/// Last global-cancel reason, if any.
pub fn last_cancel_reason() -> Option<String> {
    CANCEL_REASON.lock().ok().and_then(|g| g.clone())
}

// ---------------------------------------------------------------------------
// Confirmation correlation IDs: literal `confirm:<uuid-v4>`
// ---------------------------------------------------------------------------

fn rand_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

/// Issue a `confirm:<uuid-v4>` correlation ID using the `rand` dep.
/// Format is part of the frozen contract; frontend renders it opaquely.
pub fn new_confirmation_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    // UUIDv4 version + variant bits.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let h = rand_hex(&bytes);
    format!(
        "confirm:{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// Validate a confirmation ID without allocating on failure paths.
pub fn is_confirmation_id(id: &str) -> bool {
    let hex = match id.strip_prefix("confirm:") {
        Some(rest) => rest,
        None => return false,
    };
    if hex.len() != 36 {
        return false;
    }
    for (i, b) in hex.bytes().enumerate() {
        match i {
            8 | 13 | 18 | 23 => {
                if b != b'-' {
                    return false;
                }
            }
            _ => {
                if !b.is_ascii_hexdigit() {
                    return false;
                }
            }
        }
    }
    // Version nibble must be 4.
    hex.as_bytes()[14] == b'4'
}

// ---------------------------------------------------------------------------
// Pending confirmation store & helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PendingConfirmation {
    pub envelope: ActionEnvelope,
    pub issued_at: Instant,
    pub decided: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmationRequest {
    pub id: String,
    pub confirmation_id: String,
    pub tool: String,
    pub risk: RiskClass,
    pub args_summary: String,
}

static PENDING_CONFIRMATIONS: Mutex<Option<HashMap<String, PendingConfirmation>>> =
    Mutex::new(None);

fn pending_confirmations_map(
) -> std::sync::MutexGuard<'static, Option<HashMap<String, PendingConfirmation>>> {
    PENDING_CONFIRMATIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// 10 minutes timeout for pending confirmations (auto-deny on expiration).
pub const CONFIRMATION_TTL: Duration = Duration::from_secs(600);

/// Summarize arguments safely via `crate::redaction::redact_text` per value.
/// User content or secret fragments are never included verbatim.
pub fn summarize_args(args: &serde_json::Value) -> String {
    match args {
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                return "(none)".to_string();
            }
            let mut parts: Vec<String> = Vec::with_capacity(map.len());
            for (key, val) in map {
                let redacted_val = match val {
                    serde_json::Value::String(s) => crate::redaction::redact_text(s).into_owned(),
                    _ => {
                        let rendered = val.to_string();
                        crate::redaction::redact_text(&rendered).into_owned()
                    }
                };
                parts.push(format!("{key}={redacted_val}"));
            }
            parts.sort();
            parts.join(", ")
        }
        serde_json::Value::Null => "(none)".to_string(),
        other => crate::redaction::redact_text(&other.to_string()).into_owned(),
    }
}

/// Issue a confirmation request for an action envelope.
/// Returns `{id, confirmation_id, tool, risk, args_summary}`.
/// Cleans up expired entries (> 10 min) on each call.
pub fn issue_confirmation(envelope: ActionEnvelope) -> ConfirmationRequest {
    let now = Instant::now();
    let id = new_confirmation_id();
    let args_summary = summarize_args(&envelope.args);
    let req = ConfirmationRequest {
        id: id.clone(),
        confirmation_id: id.clone(),
        tool: envelope.tool.clone(),
        risk: envelope.risk,
        args_summary,
    };

    let mut guard = pending_confirmations_map();
    let map = guard.get_or_insert_with(HashMap::new);

    // Evict expired entries (> 10 min)
    map.retain(|_, entry| now.duration_since(entry.issued_at) <= CONFIRMATION_TTL);

    map.insert(
        id,
        PendingConfirmation {
            envelope,
            issued_at: now,
            decided: None,
        },
    );

    req
}

/// Resolve a pending confirmation.
/// Returns `Some(ActionEnvelope)` if approved and valid (not expired).
/// Returns `None` if denied, expired (> 10 min auto-deny), or not found.
/// Lost-focus keeps the entry pending until explicit resolution or expiration.
pub fn resolve_confirmation(id: &str, approve: bool) -> Option<ActionEnvelope> {
    let now = Instant::now();
    let mut guard = pending_confirmations_map();
    let map = guard.as_mut()?;

    // Evict expired entries (> 10 min auto-deny)
    map.retain(|_, entry| now.duration_since(entry.issued_at) <= CONFIRMATION_TTL);

    let entry = map.remove(id)?;
    if now.duration_since(entry.issued_at) > CONFIRMATION_TTL {
        return None;
    }

    if approve {
        Some(entry.envelope)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Request action decision & preparation (core gate)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PolicyDecision {
    ExecuteNow {
        sanitized_args: serde_json::Value,
    },
    NeedConfirm {
        confirmation_id: String,
        tool: String,
        risk: RiskClass,
        args_summary: String,
    },
    Denied {
        reason: String,
    },
}

/// Core decision logic for `request_action`.
/// Validates args fail-closed (`unknown-tool`/`invalid-args`),
/// checks for caller-supplied negation text (`negation-detected`),
/// maps `authorize` outcome, and enforces `is_cancelled` pre-check.
pub fn decide_and_prepare(
    envelope: &ActionEnvelope,
    settings_text_for_negation: Option<&str>,
) -> PolicyDecision {
    let settings = AppSettings::default();
    decide_and_prepare_with_settings(envelope, settings_text_for_negation, &settings)
}

/// Variant of `decide_and_prepare` with explicit `AppSettings`.
pub fn decide_and_prepare_with_settings(
    envelope: &ActionEnvelope,
    settings_text_for_negation: Option<&str>,
    settings: &AppSettings,
) -> PolicyDecision {
    // 1. is_cancelled pre-check
    if is_cancelled(&envelope.session_id) {
        return PolicyDecision::Denied {
            reason: "session-cancelled".to_string(),
        };
    }

    // 2. is_negated_command check on caller-supplied text
    if let Some(text) = settings_text_for_negation {
        if is_negated_command(text) {
            return PolicyDecision::Denied {
                reason: "negation-detected".to_string(),
            };
        }
    }

    // 3. validate_args fail-closed (unknown-tool / invalid-args)
    let sanitized_args = match validate_args(&envelope.tool, &envelope.args) {
        Ok(args) => args,
        Err(err) => return PolicyDecision::Denied { reason: err },
    };

    // 4. authorize outcome mapping
    match authorize(envelope, settings) {
        PolicyOutcome::Deny => PolicyDecision::Denied {
            reason: "policy-denied".to_string(),
        },
        PolicyOutcome::RequireUnlock => PolicyDecision::Denied {
            reason: "require-unlock".to_string(),
        },
        PolicyOutcome::Confirm => {
            let mut env_to_confirm = envelope.clone();
            env_to_confirm.args = sanitized_args;
            let req = issue_confirmation(env_to_confirm);
            PolicyDecision::NeedConfirm {
                confirmation_id: req.confirmation_id,
                tool: req.tool,
                risk: req.risk,
                args_summary: req.args_summary,
            }
        }
        PolicyOutcome::Allow => PolicyDecision::ExecuteNow { sanitized_args },
    }
}

// ---------------------------------------------------------------------------
// Post-execution verification & delayed execution guard
// ---------------------------------------------------------------------------

/// Post-execution `verify` hook point. Returns `Ok(())` for `kind == "none"`,
/// typed `unverifiable` Err otherwise (Wave-2 desktop/browser fill real kinds).
pub fn verify_stub(contract: &VerificationContract) -> Result<(), String> {
    if contract.kind == "none" {
        Ok(())
    } else if contract.kind == "window-focused"
        || contract.kind == "window-closed"
        || contract.kind == "desktop-element-state"
        || contract.kind == "element-state"
        || contract.kind == "vision-fallback"
    {
        crate::desktop::verify_contract(contract)
    } else if contract.kind.starts_with("browser.")
        || matches!(
            contract.kind.as_str(),
            "element_present" | "element_hidden" | "text_contains" | "url_matches" | "title_is"
        )
    {
        crate::browser::verify_action(contract).map(|_| ())
    } else {
        Err(format!(
            "unverifiable: unsupported verification kind '{}'",
            contract.kind
        ))
    }
}

/// Delayed-completion guard: returns true if the session has not been cancelled.
pub fn still_executable(session_id: &str, _gen_at_issue: u64) -> bool {
    !is_cancelled(session_id)
}

/// Action execution result payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionExecutionResult {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk: Option<RiskClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(tool: &str, risk: RiskClass) -> ActionEnvelope {
        ActionEnvelope {
            tool: tool.into(),
            args: serde_json::json!({}),
            source: ActionSource::User,
            session_id: "s1".into(),
            risk,
            capability: "test".into(),
            verification: VerificationContract::default(),
        }
    }

    #[test]
    fn unknown_tool_denies_and_fails_validation() {
        let settings = AppSettings::default();
        let env = envelope("planner.invented", RiskClass::Safe);
        assert_eq!(authorize(&env, &settings), PolicyOutcome::Deny);
        assert!(validate_args("planner.invented", &serde_json::json!({})).is_err());
    }

    #[test]
    fn destructive_needs_confirm() {
        let settings = AppSettings::default();
        let env = envelope("harness.start", RiskClass::ExternalCommit);
        assert_eq!(authorize(&env, &settings), PolicyOutcome::Confirm);
    }

    #[test]
    fn negation_corpus() {
        assert!(is_negated_command("don't close Chrome"));
        assert!(is_negated_command("Do not delete that file"));
        assert!(is_negated_command("never send that email"));
        assert!(is_negated_command("stop listening"));
        assert!(is_negated_command("actually stop what you're doing"));
        assert!(is_negated_command("finish this, actually stop"));
        assert!(is_negated_command("please don't restart the machine"));
        assert!(!is_negated_command("close Chrome"));
        assert!(!is_negated_command("search for donut recipes"));
    }

    #[test]
    fn cancel_token_roundtrip() {
        let sid = "test-session-cancel-rt";
        assert!(!is_cancelled(sid));
        cancel_session(sid);
        assert!(is_cancelled(sid));
        clear_session(sid);
        assert!(!is_cancelled(sid));
    }

    #[test]
    fn confirmation_id_shape() {
        let id = new_confirmation_id();
        assert!(is_confirmation_id(&id));
        assert!(!is_confirmation_id("confirm:not-a-uuid"));
        assert!(!is_confirmation_id(
            "other:12345678-1234-4234-8234-1234567890ab"
        ));
    }
    #[test]
    fn decide_and_prepare_negation_denies() {
        let env = envelope("app.open", RiskClass::Sensitive);
        let dec = decide_and_prepare(&env, Some("don't close Chrome"));
        match dec {
            PolicyDecision::Denied { reason } => assert_eq!(reason, "negation-detected"),
            _ => panic!("expected negation-detected Denied, got {dec:?}"),
        }
    }

    #[test]
    fn decide_and_prepare_cancelled_session_denies() {
        let env = ActionEnvelope {
            tool: "browser.search".into(),
            args: serde_json::json!({ "query": "hello" }),
            source: ActionSource::User,
            session_id: "cancelled-session-1".into(),
            risk: RiskClass::Safe,
            capability: "browser.open".into(),
            verification: VerificationContract::default(),
        };
        cancel_session("cancelled-session-1");
        let dec = decide_and_prepare(&env, None);
        match dec {
            PolicyDecision::Denied { reason } => assert_eq!(reason, "session-cancelled"),
            _ => panic!("expected session-cancelled Denied, got {dec:?}"),
        }
        clear_session("cancelled-session-1");
    }

    #[test]
    fn decide_and_prepare_unknown_tool_denies() {
        let env = envelope("system.format", RiskClass::Destructive);
        let dec = decide_and_prepare(&env, None);
        match dec {
            PolicyDecision::Denied { reason } => assert!(reason.starts_with("unknown-tool")),
            _ => panic!("expected unknown-tool Denied, got {dec:?}"),
        }
    }

    #[test]
    fn decide_and_prepare_safe_tool_executes_now() {
        let env = ActionEnvelope {
            tool: "browser.search".into(),
            args: serde_json::json!({ "query": "rust language" }),
            source: ActionSource::User,
            session_id: "safe-session".into(),
            risk: RiskClass::Safe,
            capability: "browser.open".into(),
            verification: VerificationContract::default(),
        };
        let dec = decide_and_prepare(&env, None);
        match dec {
            PolicyDecision::ExecuteNow { sanitized_args } => {
                assert_eq!(sanitized_args["query"], "rust language");
            }
            _ => panic!("expected ExecuteNow, got {dec:?}"),
        }
    }

    #[test]
    fn decide_and_prepare_confirm_and_resolve_flow() {
        let env = ActionEnvelope {
            tool: "app.open".into(),
            args: serde_json::json!({ "app": "calculator" }),
            source: ActionSource::User,
            session_id: "confirm-session".into(),
            risk: RiskClass::Sensitive,
            capability: "desktop.launch".into(),
            verification: VerificationContract::default(),
        };
        let dec = decide_and_prepare(&env, None);
        let cid = match dec {
            PolicyDecision::NeedConfirm {
                confirmation_id,
                tool,
                risk,
                args_summary,
            } => {
                assert_eq!(tool, "app.open");
                assert_eq!(risk, RiskClass::Sensitive);
                assert!(args_summary.contains("app=calculator"));
                assert!(is_confirmation_id(&confirmation_id));
                confirmation_id
            }
            _ => panic!("expected NeedConfirm, got {dec:?}"),
        };

        // Resolve approve
        let approved = resolve_confirmation(&cid, true);
        assert!(approved.is_some());
        assert_eq!(approved.unwrap().tool, "app.open");

        // Second resolve is None
        assert!(resolve_confirmation(&cid, true).is_none());
    }

    #[test]
    fn resolve_confirmation_deny() {
        let env = ActionEnvelope {
            tool: "app.open".into(),
            args: serde_json::json!({ "app": "notepad" }),
            source: ActionSource::User,
            session_id: "deny-session".into(),
            risk: RiskClass::Sensitive,
            capability: "desktop.launch".into(),
            verification: VerificationContract::default(),
        };
        let req = issue_confirmation(env);
        let resolved = resolve_confirmation(&req.confirmation_id, false);
        assert!(resolved.is_none());
        assert!(resolve_confirmation(&req.confirmation_id, true).is_none());
    }

    #[test]
    fn verify_stub_behavior() {
        let none_contract = VerificationContract::default();
        assert!(verify_stub(&none_contract).is_ok());

        let visual_contract = VerificationContract {
            kind: "visual-diff".into(),
            selector: None,
            expect: serde_json::Value::Null,
            timeout_ms: 1000,
        };
        assert!(verify_stub(&visual_contract).is_err());
    }

    #[test]
    fn still_executable_tracks_cancellation() {
        let sid = "session-executable-test";
        assert!(still_executable(sid, 0));
        cancel_session(sid);
        assert!(!still_executable(sid, 0));
        clear_session(sid);
        assert!(still_executable(sid, 0));
    }
}
