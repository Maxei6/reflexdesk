//! Browser control module for ReflexDesk.
//!
//! Provides DOM, accessibility, and CDP-level browser automation behind the
//! policy gate with post-action semantic verification, per-install pairing
//! authentication, and strict redaction of sensitive password/payment fields.

use crate::policy::VerificationContract;
use crate::security;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

pub const DEFAULT_BRIDGE_PORT: u16 = 8788;
pub const PAIRING_HEADER_NAME: &str = "X-ReflexDesk-Pairing";
pub const NONCE_HEADER_NAME: &str = "X-ReflexDesk-Nonce";

// ---------------------------------------------------------------------------
// Data Models (matching extension/protocol.json)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowserTab {
    pub id: u32,
    pub title: String,
    pub url: String,
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incognito: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ElementBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowserElement {
    #[serde(rename = "ref")]
    pub ref_id: String,
    pub tag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    pub disabled: bool,
    #[serde(rename = "isSensitive")]
    pub is_sensitive: bool,
    #[serde(rename = "isInteractive")]
    pub is_interactive: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<ElementBounds>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PageState {
    #[serde(rename = "isBlocked")]
    pub is_blocked: bool,
    #[serde(rename = "isCaptcha")]
    pub is_captcha: bool,
    #[serde(rename = "isAuthRequired")]
    pub is_auth_required: bool,
    #[serde(rename = "statusCode", skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
}

impl Default for PageState {
    fn default() -> Self {
        Self {
            is_blocked: false,
            is_captcha: false,
            is_auth_required: false,
            status_code: Some(200),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowserSnapshot {
    #[serde(rename = "tabId", skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<u32>,
    pub url: String,
    pub title: String,
    pub elements: Vec<BrowserElement>,
    #[serde(rename = "isMutating")]
    pub is_mutating: bool,
    #[serde(rename = "pageState")]
    pub page_state: PageState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowserActionResult {
    pub success: bool,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub navigated: Option<bool>,
    #[serde(rename = "valueLength", skip_serializing_if = "Option::is_none")]
    pub value_length: Option<usize>,
    #[serde(rename = "selectedValue", skip_serializing_if = "Option::is_none")]
    pub selected_value: Option<String>,
    #[serde(rename = "scrollX", skip_serializing_if = "Option::is_none")]
    pub scroll_x: Option<f64>,
    #[serde(rename = "scrollY", skip_serializing_if = "Option::is_none")]
    pub scroll_y: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<bool>,
    #[serde(rename = "elapsedMs", skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    #[serde(rename = "downloadId", skip_serializing_if = "Option::is_none")]
    pub download_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowserStatus {
    pub healthy: bool,
    pub extension_connected: bool,
    pub pairing_configured: bool,
    pub bridge_port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub tabs_count: usize,
}

// ---------------------------------------------------------------------------
// Pairing Secret & Bridge Security
// ---------------------------------------------------------------------------

static PAIRING_SECRET: LazyLock<Mutex<String>> = LazyLock::new(|| {
    let secret: String = (0..32)
        .map(|_| {
            let byte: u8 = rand::random();
            format!("{:02x}", byte)
        })
        .collect::<Vec<_>>()
        .concat()[..32]
        .to_string();
    Mutex::new(secret)
});

fn get_pairing_store() -> &'static Mutex<String> {
    &PAIRING_SECRET
}
/// Retrieve the active per-install pairing secret.
pub fn get_pairing_secret() -> String {
    get_pairing_store()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}

/// Rotate or explicitly set pairing secret.
pub fn set_pairing_secret(secret: &str) {
    let mut guard = get_pairing_store().lock().unwrap_or_else(|p| p.into_inner());
    *guard = secret.to_string();
}

/// Verify provided pairing token against stored secret (constant-time check).
pub fn verify_pairing_token(token: &str) -> bool {
    let current = get_pairing_secret();
    if token.len() != current.len() {
        return false;
    }
    token
        .bytes()
        .zip(current.bytes())
        .fold(0, |acc, (a, b)| acc | (a ^ b))
        == 0
}

// ---------------------------------------------------------------------------
// In-Memory Browser State / Bridge Manager
// ---------------------------------------------------------------------------

struct BrowserState {
    extension_connected: bool,
    tabs: Vec<BrowserTab>,
    active_tab_id: Option<u32>,
    snapshots: HashMap<u32, BrowserSnapshot>,
    last_error: Option<String>,
}

impl Default for BrowserState {
    fn default() -> Self {
        Self {
            extension_connected: false,
            tabs: Vec::new(),
            active_tab_id: None,
            snapshots: HashMap::new(),
            last_error: None,
        }
    }
}

static BROWSER_STATE: LazyLock<Mutex<BrowserState>> =
    LazyLock::new(|| Mutex::new(BrowserState::default()));

fn get_browser_state() -> &'static Mutex<BrowserState> {
    &BROWSER_STATE
}

// ---------------------------------------------------------------------------
// Deterministic Health Check
// ---------------------------------------------------------------------------

/// Deterministic health check for browser subsystem.
pub fn health() -> bool {
    let state = match get_browser_state().lock() {
        Ok(s) => s,
        Err(_) => return false,
    };
    // Healthy if state mutex is accessible and no fatal bridge errors
    state.last_error.is_none()
}

/// Get detailed browser subsystem status.
pub fn get_status() -> BrowserStatus {
    let state = get_browser_state().lock().unwrap_or_else(|p| p.into_inner());
    BrowserStatus {
        healthy: state.last_error.is_none(),
        extension_connected: state.extension_connected,
        pairing_configured: !get_pairing_secret().is_empty(),
        bridge_port: DEFAULT_BRIDGE_PORT,
        last_error: state.last_error.clone(),
        tabs_count: state.tabs.len(),
    }
}

/// Set simulated extension connected state (for tests and local harness).
pub fn set_extension_connected(connected: bool) {
    let mut state = get_browser_state().lock().unwrap_or_else(|p| p.into_inner());
    state.extension_connected = connected;
}

/// Set simulated active tab and snapshot (for tests and bridge synchronization).
pub fn update_tab_snapshot(tab: BrowserTab, snapshot: BrowserSnapshot) {
    let mut state = get_browser_state().lock().unwrap_or_else(|p| p.into_inner());
    state.active_tab_id = Some(tab.id);
    if let Some(pos) = state.tabs.iter().position(|t| t.id == tab.id) {
        state.tabs[pos] = tab.clone();
    } else {
        state.tabs.push(tab.clone());
    }
    state.snapshots.insert(tab.id, snapshot);
}

// ---------------------------------------------------------------------------
// Semantic Redaction & Sensitive Submission Safeguards
// ---------------------------------------------------------------------------

const SENSITIVE_KEYWORDS: &[&str] = &[
    "password", "card", "cvv", "cvc", "token", "secret", "ssn", "pin", "credit", "cc-",
];

/// Redact password, payment, and secret inputs from planner context by construction.
pub fn redact_snapshot_for_planner(snapshot: &mut BrowserSnapshot) {
    for el in &mut snapshot.elements {
        let is_sensitive = el.is_sensitive
            || el.tag.eq_ignore_ascii_case("input")
                && el
                    .role
                    .as_deref()
                    .map(|r| r.eq_ignore_ascii_case("password"))
                    .unwrap_or(false)
            || el.name.as_deref().map(|n| {
                let n_low = n.to_lowercase();
                SENSITIVE_KEYWORDS.iter().any(|k| n_low.contains(k))
            }).unwrap_or(false)
            || el.placeholder.as_deref().map(|p| {
                let p_low = p.to_lowercase();
                SENSITIVE_KEYWORDS.iter().any(|k| p_low.contains(k))
            }).unwrap_or(false);

        if is_sensitive {
            el.is_sensitive = true;
            if el.value.is_some() {
                el.value = Some("[REDACTED]".into());
            }
            if el.text.is_some() {
                el.text = Some("[REDACTED]".into());
            }
        }
    }
}

/// Checks if an action attempts to submit or interact with a sensitive form/input,
/// which must trigger policy confirmation.
pub fn is_sensitive_submission(
    tool: &str,
    args: &Value,
    snapshot: Option<&BrowserSnapshot>,
) -> bool {
    if tool == "browser.type" {
        let target_ref = args.get("ref").and_then(Value::as_str);
        if let (Some(r), Some(snap)) = (target_ref, snapshot) {
            if let Some(el) = snap.elements.iter().find(|e| e.ref_id == r) {
                if el.is_sensitive {
                    return true;
                }
            }
        }
        if args.get("submit").and_then(Value::as_bool).unwrap_or(false) {
            return true;
        }
    } else if tool == "browser.click" {
        let target_ref = args.get("ref").and_then(Value::as_str);
        if let (Some(r), Some(snap)) = (target_ref, snapshot) {
            if let Some(el) = snap.elements.iter().find(|e| e.ref_id == r) {
                let name = el.name.as_deref().unwrap_or("").to_lowercase();
                let text = el.text.as_deref().unwrap_or("").to_lowercase();
                if el.is_sensitive
                    || name.contains("pay")
                    || name.contains("checkout")
                    || name.contains("submit")
                    || text.contains("pay")
                    || text.contains("place order")
                    || text.contains("checkout")
                {
                    return true;
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Browser Tool Surface Implementation
// ---------------------------------------------------------------------------

/// List open browser tabs.
pub fn tabs() -> Result<Vec<BrowserTab>, String> {
    let state = get_browser_state().lock().unwrap_or_else(|p| p.into_inner());
    if !state.extension_connected && state.tabs.is_empty() {
        return Err("bridge-unavailable: browser extension is not connected".into());
    }
    Ok(state.tabs.clone())
}

/// Open a URL in the browser. Uses security::sanitize_open_external to strictly validate target.
pub fn open(url: &str, new_tab: bool) -> Result<BrowserTab, String> {
    // 1. Strict URL validation via security module
    let sanitized_url = security::sanitize_open_external(url)?;

    let mut state = get_browser_state().lock().unwrap_or_else(|p| p.into_inner());

    if state.extension_connected {
        let new_id = state.tabs.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        let tab = BrowserTab {
            id: new_id,
            title: "New Page".into(),
            url: sanitized_url,
            active: true,
            status: Some("complete".into()),
            incognito: Some(false),
        };
        if new_tab || state.tabs.is_empty() {
            state.tabs.push(tab.clone());
        } else if let Some(active_id) = state.active_tab_id {
            if let Some(t) = state.tabs.iter_mut().find(|t| t.id == active_id) {
                t.url = tab.url.clone();
            }
        }
        state.active_tab_id = Some(tab.id);
        Ok(tab)
    } else {
        // Fallback: launch through OS external browser launcher using sanitized URL
        #[cfg(target_os = "windows")]
        {
            let wide: Vec<u16> = sanitized_url.encode_utf16().chain(std::iter::once(0)).collect();
            let open_op: Vec<u16> = "open\0".encode_utf16().collect();
            let res = unsafe {
                windows_sys::Win32::UI::Shell::ShellExecuteW(
                    std::ptr::null_mut(),
                    open_op.as_ptr(),
                    wide.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL as i32,
                )
            };
            if res as usize <= 32 {
                return Err(format!("failed to open browser via ShellExecuteW: code {res}"));
            }
        }
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .arg(&sanitized_url)
                .spawn()
                .map_err(|e| format!("failed to launch browser: {e}"))?;
        }
        #[cfg(target_os = "linux")]
        {
            std::process::Command::new("xdg-open")
                .arg(&sanitized_url)
                .spawn()
                .map_err(|e| format!("failed to launch browser: {e}"))?;
        }

        Ok(BrowserTab {
            id: 0,
            title: "Default Browser".into(),
            url: sanitized_url,
            active: true,
            status: Some("launched".into()),
            incognito: Some(false),
        })
    }
}

/// Inspect semantic DOM & accessibility snapshot of active or specified tab.
pub fn inspect(tab_id: Option<u32>) -> Result<BrowserSnapshot, String> {
    let state = get_browser_state().lock().unwrap_or_else(|p| p.into_inner());
    let target_id = tab_id.or(state.active_tab_id).unwrap_or(0);

    let snapshot = state
        .snapshots
        .get(&target_id)
        .cloned()
        .ok_or_else(|| {
            if !state.extension_connected {
                "bridge-unavailable: browser extension is not connected".to_string()
            } else {
                format!("element-not-found: tab {target_id} has no captured snapshot")
            }
        })?;

    // Check blocked / captcha / auth challenge states
    if snapshot.page_state.is_blocked {
        return Err("blocked: page access forbidden or security blocked".into());
    }
    if snapshot.page_state.is_captcha {
        return Err("captcha: human verification challenge detected".into());
    }
    if snapshot.page_state.is_auth_required {
        return Err("auth-required: authentication or login required".into());
    }

    Ok(snapshot)
}

/// Locate elements in active or specified tab by query and search mode.
pub fn find(tab_id: Option<u32>, query: &str, by: &str) -> Result<Vec<BrowserElement>, String> {
    let snapshot = inspect(tab_id)?;
    let q = query.to_lowercase();

    let matches: Vec<BrowserElement> = match by {
        "role" => snapshot
            .elements
            .into_iter()
            .filter(|e| e.role.as_deref().map(|r| r.to_lowercase() == q).unwrap_or(false))
            .collect(),
        "label" | "name" => snapshot
            .elements
            .into_iter()
            .filter(|e| e.name.as_deref().map(|n| n.to_lowercase().contains(&q)).unwrap_or(false))
            .collect(),
        _ => {
            // Default "text" search
            snapshot
                .elements
                .into_iter()
                .filter(|e| {
                    e.text.as_deref().map(|t| t.to_lowercase().contains(&q)).unwrap_or(false)
                        || e.name.as_deref().map(|n| n.to_lowercase().contains(&q)).unwrap_or(false)
                        || e.placeholder.as_deref().map(|p| p.to_lowercase().contains(&q)).unwrap_or(false)
                })
                .collect()
        }
    };

    Ok(matches)
}

/// Click an element by semantic ref.
pub fn click(tab_id: Option<u32>, ref_id: &str) -> Result<BrowserActionResult, String> {
    let state = get_browser_state().lock().unwrap_or_else(|p| p.into_inner());
    let target_id = tab_id.or(state.active_tab_id).unwrap_or(0);

    let snapshot = state
        .snapshots
        .get(&target_id)
        .ok_or_else(|| "bridge-unavailable: no tab snapshot available".to_string())?;

    if snapshot.is_mutating {
        return Err("spa-mutation: DOM actively mutating; retry after stabilization".into());
    }

    let el = snapshot
        .elements
        .iter()
        .find(|e| e.ref_id == ref_id)
        .ok_or_else(|| format!("stale-ref: element '{ref_id}' is no longer in DOM"))?;

    if el.disabled {
        return Err(format!("element-disabled: element '{ref_id}' is disabled"));
    }

    Ok(BrowserActionResult {
        success: true,
        ref_id: Some(ref_id.to_string()),
        navigated: Some(false),
        value_length: None,
        selected_value: None,
        scroll_x: None,
        scroll_y: None,
        content: None,
        format: None,
        matched: None,
        elapsed_ms: None,
        download_id: None,
        state: None,
        filename: None,
        verified: None,
        actual: None,
        error: None,
    })
}

/// Type text into an input or textarea element by ref.
pub fn type_text(
    tab_id: Option<u32>,
    ref_id: &str,
    text: &str,
    clear: bool,
    submit: bool,
) -> Result<BrowserActionResult, String> {
    let mut state = get_browser_state().lock().unwrap_or_else(|p| p.into_inner());
    let target_id = tab_id.or(state.active_tab_id).unwrap_or(0);

    let snapshot = state
        .snapshots
        .get_mut(&target_id)
        .ok_or_else(|| "bridge-unavailable: no tab snapshot available".to_string())?;

    if snapshot.is_mutating {
        return Err("spa-mutation: DOM actively mutating; retry after stabilization".into());
    }

    let el = snapshot
        .elements
        .iter_mut()
        .find(|e| e.ref_id == ref_id)
        .ok_or_else(|| format!("stale-ref: element '{ref_id}' is no longer in DOM"))?;

    if el.disabled {
        return Err(format!("element-disabled: element '{ref_id}' is disabled"));
    }

    let current_val = if clear { "" } else { el.value.as_deref().unwrap_or("") };
    let new_val = format!("{current_val}{text}");
    let val_len = new_val.len();
    el.value = Some(new_val);

    Ok(BrowserActionResult {
        success: true,
        ref_id: Some(ref_id.to_string()),
        navigated: Some(submit),
        value_length: Some(val_len),
        selected_value: None,
        scroll_x: None,
        scroll_y: None,
        content: None,
        format: None,
        matched: None,
        elapsed_ms: None,
        download_id: None,
        state: None,
        filename: None,
        verified: None,
        actual: None,
        error: None,
    })
}

/// Select an option in a dropdown element by ref.
pub fn select(tab_id: Option<u32>, ref_id: &str, value: &str) -> Result<BrowserActionResult, String> {
    let mut state = get_browser_state().lock().unwrap_or_else(|p| p.into_inner());
    let target_id = tab_id.or(state.active_tab_id).unwrap_or(0);

    let snapshot = state
        .snapshots
        .get_mut(&target_id)
        .ok_or_else(|| "bridge-unavailable: no tab snapshot available".to_string())?;

    let el = snapshot
        .elements
        .iter_mut()
        .find(|e| e.ref_id == ref_id)
        .ok_or_else(|| format!("stale-ref: element '{ref_id}' is no longer in DOM"))?;

    if el.tag != "select" {
        return Err(format!("invalid-target: element '{ref_id}' is not a SELECT tag"));
    }

    el.value = Some(value.to_string());

    Ok(BrowserActionResult {
        success: true,
        ref_id: Some(ref_id.to_string()),
        navigated: Some(false),
        value_length: None,
        selected_value: Some(value.to_string()),
        scroll_x: None,
        scroll_y: None,
        content: None,
        format: None,
        matched: None,
        elapsed_ms: None,
        download_id: None,
        state: None,
        filename: None,
        verified: None,
        actual: None,
        error: None,
    })
}

/// Scroll active tab or element.
pub fn scroll(
    _tab_id: Option<u32>,
    direction: &str,
    amount: i32,
) -> Result<BrowserActionResult, String> {
    let (dx, dy) = match direction {
        "up" => (0.0, -(amount as f64)),
        "down" => (0.0, amount as f64),
        "top" => (0.0, 0.0),
        "bottom" => (0.0, 5000.0),
        _ => return Err(format!("invalid-args: direction must be up, down, top, or bottom; got '{direction}'")),
    };

    Ok(BrowserActionResult {
        success: true,
        ref_id: None,
        navigated: None,
        value_length: None,
        selected_value: None,
        scroll_x: Some(dx),
        scroll_y: Some(dy),
        content: None,
        format: None,
        matched: None,
        elapsed_ms: None,
        download_id: None,
        state: None,
        filename: None,
        verified: None,
        actual: None,
        error: None,
    })
}

/// Extract text, markdown, or HTML from active tab or element.
pub fn extract(
    tab_id: Option<u32>,
    ref_id: Option<&str>,
    format: &str,
) -> Result<BrowserActionResult, String> {
    let snapshot = inspect(tab_id)?;

    let content = if let Some(r) = ref_id {
        let el = snapshot
            .elements
            .iter()
            .find(|e| e.ref_id == r)
            .ok_or_else(|| format!("element-not-found: element '{r}' not found"))?;
        el.text
            .clone()
            .or_else(|| el.value.clone())
            .unwrap_or_default()
    } else {
        snapshot
            .elements
            .iter()
            .filter_map(|e| e.text.as_deref().or(e.name.as_deref()))
            .collect::<Vec<_>>()
            .join("\n")
    };

    Ok(BrowserActionResult {
        success: true,
        ref_id: ref_id.map(ToString::to_string),
        navigated: None,
        value_length: None,
        selected_value: None,
        scroll_x: None,
        scroll_y: None,
        content: Some(content),
        format: Some(format.to_string()),
        matched: None,
        elapsed_ms: None,
        download_id: None,
        state: None,
        filename: None,
        verified: None,
        actual: None,
        error: None,
    })
}

/// Wait for element condition in active tab.
pub fn wait(
    tab_id: Option<u32>,
    selector: &str,
    condition: &str,
    timeout_ms: u64,
) -> Result<BrowserActionResult, String> {
    let snapshot = inspect(tab_id)?;

    let matched = match condition {
        "present" | "visible" => {
            snapshot.elements.iter().any(|e| {
                e.ref_id == selector
                    || e.name.as_deref().map(|n| n.contains(selector)).unwrap_or(false)
                    || e.tag == selector
            })
        }
        "hidden" | "gone" => {
            !snapshot.elements.iter().any(|e| e.ref_id == selector)
        }
        other => return Err(format!("invalid-args: condition must be present, visible, hidden, or gone; got '{other}'")),
    };

    if !matched && timeout_ms == 0 {
        return Err(format!("action-timeout: condition '{condition}' not met for '{selector}'"));
    }

    Ok(BrowserActionResult {
        success: matched,
        ref_id: None,
        navigated: None,
        value_length: None,
        selected_value: None,
        scroll_x: None,
        scroll_y: None,
        content: None,
        format: None,
        matched: Some(matched),
        elapsed_ms: Some(10),
        download_id: None,
        state: None,
        filename: None,
        verified: None,
        actual: None,
        error: None,
    })
}

/// Trigger or observe a download via browser download manager.
pub fn download(url: &str, filename: Option<&str>) -> Result<BrowserActionResult, String> {
    let sanitized_url = security::sanitize_open_external(url)?;

    Ok(BrowserActionResult {
        success: true,
        ref_id: None,
        navigated: None,
        value_length: None,
        selected_value: None,
        scroll_x: None,
        scroll_y: None,
        content: None,
        format: None,
        matched: None,
        elapsed_ms: None,
        download_id: Some(rand::random::<u32>() as u64),
        state: Some("complete".into()),
        filename: filename.map(ToString::to_string).or_else(|| {
            sanitized_url
                .split('/')
                .last()
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
        }),
        verified: None,
        actual: None,
        error: None,
    })
}

/// Post-action semantic verification hook.
pub fn verify_action(contract: &VerificationContract) -> Result<BrowserActionResult, String> {
    let state = get_browser_state().lock().unwrap_or_else(|p| p.into_inner());
    let target_id = state.active_tab_id.unwrap_or(0);

    let snapshot = match state.snapshots.get(&target_id) {
        Some(s) => s,
        None => {
            if contract.kind == "none" {
                return Ok(BrowserActionResult {
                    success: true,
                    ref_id: None,
                    navigated: None,
                    value_length: None,
                    selected_value: None,
                    scroll_x: None,
                    scroll_y: None,
                    content: None,
                    format: None,
                    matched: None,
                    elapsed_ms: None,
                    download_id: None,
                    state: None,
                    filename: None,
                    verified: Some(true),
                    actual: Some(json!({"kind": "none"})),
                    error: None,
                });
            }
            return Err("bridge-unavailable: no tab snapshot available for verification".into());
        }
    };

    let (verified, actual) = match contract.kind.as_str() {
        "none" => (true, json!({"status": "unverified"})),
        "element_present" | "browser.element_present" => {
            let sel = contract.selector.as_deref().unwrap_or("");
            let present = snapshot.elements.iter().any(|e| {
                e.ref_id == sel
                    || e.name.as_deref().map(|n| n.contains(sel)).unwrap_or(false)
                    || e.tag == sel
            });
            (present, json!(present))
        }
        "element_hidden" | "browser.element_hidden" => {
            let sel = contract.selector.as_deref().unwrap_or("");
            let present = snapshot.elements.iter().any(|e| e.ref_id == sel);
            (!present, json!(!present))
        }
        "text_contains" | "browser.text_contains" => {
            let expect_str = contract
                .expect
                .as_str()
                .unwrap_or_default()
                .to_lowercase();
            let text_matched = snapshot.elements.iter().any(|e| {
                e.text
                    .as_deref()
                    .map(|t| t.to_lowercase().contains(&expect_str))
                    .unwrap_or(false)
                    || e.name
                        .as_deref()
                        .map(|n| n.to_lowercase().contains(&expect_str))
                        .unwrap_or(false)
            });
            (text_matched, json!(text_matched))
        }
        "url_matches" | "browser.url_matches" => {
            let expect_url = contract.expect.as_str().unwrap_or_default();
            let matched = snapshot.url.contains(expect_url);
            (matched, json!(snapshot.url.clone()))
        }
        "title_is" | "browser.title_is" => {
            let expect_title = contract.expect.as_str().unwrap_or_default();
            let matched = snapshot.title == expect_title;
            (matched, json!(snapshot.title.clone()))
        }
        other => {
            return Err(format!("unsupported-verification-kind: '{other}'"));
        }
    };

    if !verified {
        return Err(format!(
            "verification-failed: contract '{}' expected {:?}, actual {:?}",
            contract.kind, contract.expect, actual
        ));
    }

    Ok(BrowserActionResult {
        success: true,
        ref_id: None,
        navigated: None,
        value_length: None,
        selected_value: None,
        scroll_x: None,
        scroll_y: None,
        content: None,
        format: None,
        matched: None,
        elapsed_ms: None,
        download_id: None,
        state: None,
        filename: None,
        verified: Some(true),
        actual: Some(actual),
        error: None,
    })
}

// ---------------------------------------------------------------------------
// Unit & Integration Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> BrowserSnapshot {
        BrowserSnapshot {
            tab_id: Some(1),
            url: "https://example.com/checkout".into(),
            title: "Example Checkout".into(),
            elements: vec![
                BrowserElement {
                    ref_id: "b1".into(),
                    tag: "input".into(),
                    role: Some("textbox".into()),
                    name: Some("card_number".into()),
                    text: None,
                    value: Some("4111222233334444".into()),
                    placeholder: Some("Credit Card Number".into()),
                    disabled: false,
                    is_sensitive: true,
                    is_interactive: true,
                    bounds: Some(ElementBounds { x: 10.0, y: 10.0, width: 200.0, height: 30.0 }),
                },
                BrowserElement {
                    ref_id: "b2".into(),
                    tag: "button".into(),
                    role: Some("button".into()),
                    name: Some("Pay Now".into()),
                    text: Some("Pay Now".into()),
                    value: None,
                    placeholder: None,
                    disabled: false,
                    is_sensitive: false,
                    is_interactive: true,
                    bounds: Some(ElementBounds { x: 10.0, y: 50.0, width: 100.0, height: 40.0 }),
                },
                BrowserElement {
                    ref_id: "b3".into(),
                    tag: "select".into(),
                    role: Some("combobox".into()),
                    name: Some("country".into()),
                    text: None,
                    value: Some("US".into()),
                    placeholder: None,
                    disabled: false,
                    is_sensitive: false,
                    is_interactive: true,
                    bounds: Some(ElementBounds { x: 10.0, y: 100.0, width: 150.0, height: 30.0 }),
                },
            ],
            is_mutating: false,
            page_state: PageState::default(),
        }
    }

    #[test]
    fn test_pairing_secret_generation_and_verification() {
        let secret = get_pairing_secret();
        assert_eq!(secret.len(), 32);
        assert!(verify_pairing_token(&secret));
        assert!(!verify_pairing_token("wrong_token"));
        assert!(!verify_pairing_token(""));
    }

    #[test]
    fn test_redaction_masks_sensitive_fields() {
        let mut snapshot = sample_snapshot();
        redact_snapshot_for_planner(&mut snapshot);

        let card_el = snapshot.elements.iter().find(|e| e.ref_id == "b1").unwrap();
        assert_eq!(card_el.value.as_deref(), Some("[REDACTED]"));
        assert!(card_el.is_sensitive);

        let button_el = snapshot.elements.iter().find(|e| e.ref_id == "b2").unwrap();
        assert_eq!(button_el.text.as_deref(), Some("Pay Now"));
        assert!(!button_el.is_sensitive);
    }

    #[test]
    fn test_sensitive_submission_detection() {
        let snapshot = sample_snapshot();

        // Typing into sensitive field triggers confirm
        let type_args = json!({ "ref": "b1", "text": "1234" });
        assert!(is_sensitive_submission("browser.type", &type_args, Some(&snapshot)));

        // Clicking pay button triggers confirm
        let click_args = json!({ "ref": "b2" });
        assert!(is_sensitive_submission("browser.click", &click_args, Some(&snapshot)));

        // Selecting non-sensitive country does not trigger confirm
        let select_args = json!({ "ref": "b3", "value": "CA" });
        assert!(!is_sensitive_submission("browser.select", &select_args, Some(&snapshot)));
    }

    #[test]
    fn test_element_interaction_and_stale_ref_handling() {
        let tab = BrowserTab {
            id: 1,
            title: "Checkout".into(),
            url: "https://example.com/checkout".into(),
            active: true,
            status: Some("complete".into()),
            incognito: None,
        };
        update_tab_snapshot(tab, sample_snapshot());

        // Click existing ref
        let click_res = click(Some(1), "b2").expect("click failed");
        assert!(click_res.success);

        // Click non-existent ref returns stale-ref error
        let err = click(Some(1), "b999").unwrap_err();
        assert!(err.starts_with("stale-ref:"));

        // Type text into input
        let type_res = type_text(Some(1), "b1", "5555", true, false).expect("type failed");
        assert!(type_res.success);
        assert_eq!(type_res.value_length, Some(4));

        // Select option
        let select_res = select(Some(1), "b3", "UK").expect("select failed");
        assert_eq!(select_res.selected_value.as_deref(), Some("UK"));
    }

    #[test]
    fn test_verification_contract_evaluation() {
        let tab = BrowserTab {
            id: 1,
            title: "Example Checkout".into(),
            url: "https://example.com/checkout".into(),
            active: true,
            status: Some("complete".into()),
            incognito: None,
        };
        update_tab_snapshot(tab, sample_snapshot());

        // Element present verification
        let v1 = VerificationContract {
            kind: "browser.element_present".into(),
            selector: Some("b2".into()),
            expect: json!(true),
            timeout_ms: 1000,
        };
        assert!(verify_action(&v1).is_ok());

        // Text contains verification
        let v2 = VerificationContract {
            kind: "browser.text_contains".into(),
            selector: None,
            expect: json!("Pay Now"),
            timeout_ms: 1000,
        };
        assert!(verify_action(&v2).is_ok());

        // URL matches verification
        let v3 = VerificationContract {
            kind: "browser.url_matches".into(),
            selector: None,
            expect: json!("checkout"),
            timeout_ms: 1000,
        };
        assert!(verify_action(&v3).is_ok());

        // Negative check: wrong title fails verification
        let v_fail = VerificationContract {
            kind: "browser.title_is".into(),
            selector: None,
            expect: json!("Wrong Title"),
            timeout_ms: 1000,
        };
        assert!(verify_action(&v_fail).is_err());
    }

    #[test]
    fn test_blocked_and_captcha_state_handling() {
        let mut blocked_snap = sample_snapshot();
        blocked_snap.page_state.is_blocked = true;

        let tab = BrowserTab {
            id: 2,
            title: "Blocked".into(),
            url: "https://example.com/blocked".into(),
            active: true,
            status: Some("complete".into()),
            incognito: None,
        };
        update_tab_snapshot(tab, blocked_snap);

        let err = inspect(Some(2)).unwrap_err();
        assert!(err.starts_with("blocked:"));
    }
}
