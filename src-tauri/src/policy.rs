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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
            name: "harness.start",
            risk: RiskClass::ExternalCommit,
            capability: "harness.start",
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
                .ok_or_else(|| "invalid-args: browser.search requires {query: string}".to_string())?;
            let query = query.trim();
            if query.is_empty() || query.len() > 500 {
                return Err("invalid-args: browser.search query must be 1..500 chars".into());
            }
            Ok(serde_json::json!({ "query": query }))
        }
        "harness.start" => {
            let harness = args
                .get("harness")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "invalid-args: harness.start requires {harness: string}".to_string())?;
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
    let known: Vec<String> = set.as_ref().map(|s| s.iter().cloned().collect()).unwrap_or_default();
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
    rand::rngs::OsRng.fill_bytes(&mut bytes);
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
        assert!(!is_confirmation_id("other:12345678-1234-4234-8234-1234567890ab"));
    }
}
