//! Browser control module for ReflexDesk.
//!
//! Provides DOM, accessibility, and CDP-level browser automation behind the
//! policy gate with post-action semantic verification, per-install pairing
//! authentication, and strict redaction of sensitive password/payment fields.

use crate::policy::VerificationContract;
use crate::security;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    LazyLock, Mutex,
};
use std::time::Duration;
use tungstenite::{accept, Message, WebSocket};

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

static PAIRING_SECRET: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new(String::new()));

fn get_pairing_store() -> &'static Mutex<String> {
    &PAIRING_SECRET
}

pub fn initialize_pairing_secret() -> Result<(), String> {
    const SERVICE: &str = "com.reflexdesk.browser";
    const USER: &str = "extension-pairing";
    let entry = keyring::Entry::new(SERVICE, USER)
        .map_err(|e| format!("browser-pairing-store-unavailable: {e}"))?;
    let secret = match entry.get_password() {
        Ok(secret) if secret.len() == 32 && secret.chars().all(|c| c.is_ascii_hexdigit()) => secret,
        Ok(_) | Err(keyring::Error::NoEntry) => {
            use rand::RngCore;
            let mut bytes = [0u8; 16];
            rand::rng().fill_bytes(&mut bytes);
            let generated = bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            entry
                .set_password(&generated)
                .map_err(|e| format!("browser-pairing-store-write-failed: {e}"))?;
            generated
        }
        Err(error) => return Err(format!("browser-pairing-store-read-failed: {error}")),
    };
    let mut guard = get_pairing_store()
        .lock()
        .map_err(|_| "browser pairing lock poisoned".to_string())?;
    *guard = secret;
    Ok(())
}
/// Retrieve the active per-install pairing secret.
pub fn get_pairing_secret() -> String {
    get_pairing_store()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}

/// Test-only pairing override.
#[cfg(test)]
pub fn set_pairing_secret(secret: &str) {
    let mut guard = get_pairing_store()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
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

static BRIDGE_SOCKET: LazyLock<Mutex<Option<WebSocket<TcpStream>>>> =
    LazyLock::new(|| Mutex::new(None));
static BRIDGE_STARTED: AtomicBool = AtomicBool::new(false);
static SEEN_NONCES: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

fn bridge_nonce() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn record_bridge_error(error: &str) {
    let mut state = get_browser_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.extension_connected = false;
    state.last_error = Some(crate::redaction::redact_error(error));
}

/// Starts the authenticated browser-extension bridge on loopback only.
pub fn start_bridge() -> Result<(), String> {
    if get_pairing_secret().is_empty() {
        let error = "browser-pairing-unavailable: credential-store initialization failed";
        record_bridge_error(error);
        return Err(error.into());
    }
    if BRIDGE_STARTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(());
    }
    let listener = match TcpListener::bind(("127.0.0.1", DEFAULT_BRIDGE_PORT)) {
        Ok(listener) => listener,
        Err(error) => {
            BRIDGE_STARTED.store(false, Ordering::SeqCst);
            let message = format!("browser-bridge-bind-failed: {error}");
            record_bridge_error(&message);
            return Err(message);
        }
    };
    std::thread::Builder::new()
        .name("reflexdesk-browser-bridge".into())
        .spawn(move || {
            for incoming in listener.incoming() {
                let Ok(stream) = incoming else {
                    continue;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
                let Ok(mut socket) = accept(stream) else {
                    continue;
                };
                let authenticated = socket
                    .read()
                    .ok()
                    .and_then(|message| message.into_text().ok())
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                    .map(|handshake| {
                        handshake.get("type").and_then(Value::as_str) == Some("handshake")
                            && handshake.get("v").and_then(Value::as_u64) == Some(1)
                            && handshake
                                .get("pairing")
                                .and_then(Value::as_str)
                                .map(verify_pairing_token)
                                .unwrap_or(false)
                    })
                    .unwrap_or(false);
                if !authenticated {
                    let _ = socket.close(None);
                    continue;
                }
                if let Ok(mut current) = BRIDGE_SOCKET.lock() {
                    if let Some(mut previous) = current.take() {
                        let _ = previous.close(None);
                    }
                    *current = Some(socket);
                }
                let mut state = get_browser_state()
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                state.extension_connected = true;
                state.last_error = None;
            }
        })
        .map_err(|e| {
            BRIDGE_STARTED.store(false, Ordering::SeqCst);
            format!("browser-bridge-thread-failed: {e}")
        })?;
    Ok(())
}

fn bridge_call(action: &str, args: Value, tab_id: Option<u32>) -> Result<Value, String> {
    const MAX_MESSAGE_BYTES: usize = 2 * 1024 * 1024;
    let session = bridge_nonce();
    let nonce = bridge_nonce();
    {
        let mut seen = SEEN_NONCES
            .lock()
            .map_err(|_| "browser nonce lock poisoned")?;
        if seen.len() >= 4096 {
            seen.clear();
        }
        if !seen.insert(nonce.clone()) {
            return Err("browser-bridge-nonce-collision".into());
        }
    }
    let pairing = get_pairing_secret();
    let request = json!({
        "v": 1,
        "session": session,
        "pairing": pairing,
        "tabId": tab_id,
        "action": action,
        "args": args,
        "nonce": nonce,
    });
    let encoded = serde_json::to_string(&request).map_err(|e| e.to_string())?;
    if encoded.len() > MAX_MESSAGE_BYTES {
        return Err("browser-bridge-request-too-large".into());
    }

    let result = (|| {
        let mut guard = BRIDGE_SOCKET
            .lock()
            .map_err(|_| "browser bridge socket lock poisoned".to_string())?;
        let socket = guard.as_mut().ok_or_else(|| {
            "bridge-unavailable: authenticated browser extension is not connected".to_string()
        })?;
        socket
            .send(Message::Text(encoded.into()))
            .map_err(|e| format!("browser-bridge-send-failed: {e}"))?;
        let response_text = socket
            .read()
            .map_err(|e| format!("browser-bridge-timeout-or-disconnect: {e}"))?
            .into_text()
            .map_err(|_| "browser-bridge-invalid-response-type".to_string())?;
        if response_text.len() > MAX_MESSAGE_BYTES {
            return Err("browser-bridge-response-too-large".into());
        }
        let response: Value = serde_json::from_str(&response_text)
            .map_err(|e| format!("browser-bridge-invalid-json: {e}"))?;
        if response.get("v").and_then(Value::as_u64) != Some(1)
            || response.get("session").and_then(Value::as_str) != Some(session.as_str())
            || response.get("nonce").and_then(Value::as_str) != Some(nonce.as_str())
            || response.get("pairing").and_then(Value::as_str) != Some(pairing.as_str())
        {
            return Err("browser-bridge-auth-or-correlation-failed".into());
        }
        if !response
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let code = response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("action-failed");
            return Err(format!("browser-{code}"));
        }
        Ok(response.get("data").cloned().unwrap_or(response))
    })();
    if let Ok(mut seen) = SEEN_NONCES.lock() {
        seen.remove(&nonce);
    }
    if let Err(error) = &result {
        let transport_or_protocol_failure = error.contains("timeout")
            || error.contains("disconnect")
            || error.contains("send-failed")
            || error.contains("invalid-response")
            || error.contains("invalid-json")
            || error.contains("response-too-large")
            || error.contains("auth-or-correlation");
        if transport_or_protocol_failure {
            if let Ok(mut socket) = BRIDGE_SOCKET.lock() {
                *socket = None;
            }
            let mut state = get_browser_state()
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            state.extension_connected = false;
            state.last_error = Some(crate::redaction::redact_error(error));
        }
    }
    result
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
pub fn health() -> bool {
    BRIDGE_STARTED.load(Ordering::SeqCst)
        && BRIDGE_SOCKET
            .lock()
            .map(|socket| socket.is_some())
            .unwrap_or(false)
}

/// Get detailed browser subsystem status.
pub fn get_status() -> BrowserStatus {
    let state = get_browser_state()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    BrowserStatus {
        healthy: health(),
        extension_connected: health(),
        pairing_configured: !get_pairing_secret().is_empty(),
        bridge_port: DEFAULT_BRIDGE_PORT,
        last_error: state.last_error.clone(),
        tabs_count: state.tabs.len(),
    }
}

/// Cache the latest authenticated bridge snapshot for policy-sensitive checks.
pub fn update_tab_snapshot(tab: BrowserTab, snapshot: BrowserSnapshot) {
    let mut state = get_browser_state()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
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
            || el
                .name
                .as_deref()
                .map(|n| {
                    let n_low = n.to_lowercase();
                    SENSITIVE_KEYWORDS.iter().any(|k| n_low.contains(k))
                })
                .unwrap_or(false)
            || el
                .placeholder
                .as_deref()
                .map(|p| {
                    let p_low = p.to_lowercase();
                    SENSITIVE_KEYWORDS.iter().any(|k| p_low.contains(k))
                })
                .unwrap_or(false);

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
    let data = bridge_call("browser.tabs", json!({}), None)?;
    let tabs: Vec<BrowserTab> = serde_json::from_value(
        data.get("tabs")
            .cloned()
            .ok_or_else(|| "browser-bridge-response-missing-tabs".to_string())?,
    )
    .map_err(|e| format!("browser-bridge-invalid-tabs: {e}"))?;
    let mut state = get_browser_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.active_tab_id = tabs.iter().find(|tab| tab.active).map(|tab| tab.id);
    state.tabs = tabs.clone();
    Ok(tabs)
}

/// Open a URL in the browser. Uses security::sanitize_open_external to strictly validate target.
pub fn open(url: &str, new_tab: bool) -> Result<BrowserTab, String> {
    let sanitized_url = security::sanitize_open_external(url)?;
    let data = bridge_call(
        "browser.open",
        json!({ "url": sanitized_url, "newTab": new_tab, "active": true }),
        None,
    )?;
    let id = data
        .get("tabId")
        .and_then(Value::as_u64)
        .ok_or_else(|| "browser-bridge-response-missing-tab-id".to_string())? as u32;
    let tab = BrowserTab {
        id,
        title: data
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        url: data
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or(url)
            .to_string(),
        active: true,
        status: Some("loading".into()),
        incognito: None,
    };
    let mut state = get_browser_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.active_tab_id = Some(id);
    if let Some(existing) = state.tabs.iter_mut().find(|existing| existing.id == id) {
        *existing = tab.clone();
    } else {
        state.tabs.push(tab.clone());
    }
    Ok(tab)
}

/// Inspect semantic DOM & accessibility snapshot of active or specified tab.
pub fn inspect(tab_id: Option<u32>) -> Result<BrowserSnapshot, String> {
    let data = bridge_call("browser.inspect", json!({ "maxDepth": 16 }), tab_id)?;
    let mut snapshot: BrowserSnapshot = serde_json::from_value(data)
        .map_err(|e| format!("browser-bridge-invalid-snapshot: {e}"))?;
    redact_snapshot_for_planner(&mut snapshot);
    let tab = BrowserTab {
        id: snapshot.tab_id.unwrap_or_default(),
        title: snapshot.title.clone(),
        url: snapshot.url.clone(),
        active: true,
        status: Some("complete".into()),
        incognito: None,
    };
    update_tab_snapshot(tab, snapshot.clone());
    Ok(snapshot)
}

/// Locate elements in active or specified tab by semantic query or CSS selector.
pub fn find(tab_id: Option<u32>, query: &str, by: &str) -> Result<Vec<BrowserElement>, String> {
    let snapshot = inspect(tab_id)?;
    let data = bridge_call(
        "browser.find",
        json!({ "query": query, "by": by }),
        tab_id,
    )?;
    let refs: Vec<String> = serde_json::from_value(
        data.get("matches")
            .cloned()
            .ok_or_else(|| "browser-bridge-response-missing-matches".to_string())?,
    )
    .map_err(|error| format!("browser-bridge-invalid-matches: {error}"))?;
    let refs: HashSet<_> = refs.into_iter().collect();
    Ok(snapshot
        .elements
        .into_iter()
        .filter(|element| refs.contains(&element.ref_id))
        .collect())
}

/// Click an element by semantic ref.
pub fn click(tab_id: Option<u32>, ref_id: &str) -> Result<BrowserActionResult, String> {
    let data = bridge_call("browser.click", json!({ "ref": ref_id }), tab_id)?;
    serde_json::from_value(data).map_err(|e| format!("browser-bridge-invalid-action-result: {e}"))
}

/// Type text into an input or textarea element by ref.
pub fn type_text(
    tab_id: Option<u32>,
    ref_id: &str,
    text: &str,
    clear: bool,
    submit: bool,
) -> Result<BrowserActionResult, String> {
    let data = bridge_call(
        "browser.type",
        json!({ "ref": ref_id, "text": text, "clear": clear, "submit": submit }),
        tab_id,
    )?;
    serde_json::from_value(data).map_err(|e| format!("browser-bridge-invalid-action-result: {e}"))
}

/// Select an option in a dropdown element by ref.
pub fn select(
    tab_id: Option<u32>,
    ref_id: &str,
    value: &str,
) -> Result<BrowserActionResult, String> {
    let data = bridge_call(
        "browser.select",
        json!({ "ref": ref_id, "value": value }),
        tab_id,
    )?;
    serde_json::from_value(data).map_err(|e| format!("browser-bridge-invalid-action-result: {e}"))
}

/// Scroll active tab or element.
pub fn scroll(
    tab_id: Option<u32>,
    direction: &str,
    amount: i32,
) -> Result<BrowserActionResult, String> {
    let data = bridge_call(
        "browser.scroll",
        json!({ "direction": direction, "amount": amount }),
        tab_id,
    )?;
    serde_json::from_value(data).map_err(|e| format!("browser-bridge-invalid-action-result: {e}"))
}

/// Extract text, markdown, or HTML from active tab or element.
pub fn extract(
    tab_id: Option<u32>,
    ref_id: Option<&str>,
    format: &str,
) -> Result<BrowserActionResult, String> {
    let data = bridge_call(
        "browser.extract",
        json!({ "ref": ref_id, "format": format }),
        tab_id,
    )?;
    serde_json::from_value(data).map_err(|e| format!("browser-bridge-invalid-action-result: {e}"))
}

/// Wait for element condition in active tab.
pub fn wait(
    tab_id: Option<u32>,
    selector: &str,
    condition: &str,
    timeout_ms: u64,
) -> Result<BrowserActionResult, String> {
    let data = bridge_call(
        "browser.wait",
        json!({ "selector": selector, "condition": condition, "timeoutMs": timeout_ms.min(30_000) }),
        tab_id,
    )?;
    serde_json::from_value(data).map_err(|e| format!("browser-bridge-invalid-action-result: {e}"))
}

/// Trigger or observe a download via browser download manager.
pub fn download(url: &str, filename: Option<&str>) -> Result<BrowserActionResult, String> {
    let sanitized_url = security::sanitize_open_external(url)?;
    let data = bridge_call(
        "browser.download",
        json!({ "url": sanitized_url, "filename": filename }),
        None,
    )?;
    serde_json::from_value(data).map_err(|e| format!("browser-bridge-invalid-action-result: {e}"))
}

/// Post-action semantic verification hook.
pub fn verify_action(contract: &VerificationContract) -> Result<BrowserActionResult, String> {
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

    let kind = contract
        .kind
        .strip_prefix("browser.")
        .unwrap_or(&contract.kind);
    let data = bridge_call(
        "browser.verify",
        json!({
            "kind": kind,
            "selector": contract.selector,
            "expect": contract.expect,
            "timeoutMs": contract.timeout_ms.min(30_000)
        }),
        None,
    )?;
    let result: BrowserActionResult = serde_json::from_value(data)
        .map_err(|error| format!("browser-bridge-invalid-verification-result: {error}"))?;
    if result.verified != Some(true) {
        return Err(format!(
            "verification-failed: contract '{}' expected {:?}, actual {:?}",
            contract.kind, contract.expect, result.actual
        ));
    }
    Ok(result)
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
                    bounds: Some(ElementBounds {
                        x: 10.0,
                        y: 10.0,
                        width: 200.0,
                        height: 30.0,
                    }),
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
                    bounds: Some(ElementBounds {
                        x: 10.0,
                        y: 50.0,
                        width: 100.0,
                        height: 40.0,
                    }),
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
                    bounds: Some(ElementBounds {
                        x: 10.0,
                        y: 100.0,
                        width: 150.0,
                        height: 30.0,
                    }),
                },
            ],
            is_mutating: false,
            page_state: PageState::default(),
        }
    }

    #[test]
    fn test_pairing_secret_generation_and_verification() {
        set_pairing_secret("0123456789abcdef0123456789abcdef");
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
        assert!(is_sensitive_submission(
            "browser.type",
            &type_args,
            Some(&snapshot)
        ));

        // Clicking pay button triggers confirm
        let click_args = json!({ "ref": "b2" });
        assert!(is_sensitive_submission(
            "browser.click",
            &click_args,
            Some(&snapshot)
        ));

        // Selecting non-sensitive country does not trigger confirm
        let select_args = json!({ "ref": "b3", "value": "CA" });
        assert!(!is_sensitive_submission(
            "browser.select",
            &select_args,
            Some(&snapshot)
        ));
    }
}
