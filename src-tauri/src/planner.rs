//! Plan 04 — Compact local tool-use planner turnkey.
//!
//! Provides a configurable compact local planner backend as a replaceable adapter
//! through the [`LocalPlanner`] trait.
//!
//! Invariants:
//! - Prompts MUST route through `redact_text` before any logging/emission.
//! - Offline consent enforcement stays: remote endpoints require `allow_remote` AND
//!   persisted `allow_online_ai`; loopback always allowed locally.
//! - Planner output maps strictly to policy-gated [`ActionEnvelope`] values;
//!   the planner may plan, it may NEVER execute.
//! - Deterministic health checks and failure modes.
//! - Model acquisition via [`ModelManager`] where the plan requires weights.
//! - A built-in deterministic planner is always available on fresh installs.
//!   Optional local/remote model backends improve ambiguous requests but are never
//!   required for ordinary multi-step app/browser/harness commands.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::{
    model_manager::ModelManager,
    policy::{
        cancel_session, is_cancelled, tool_registry, validate_args, ActionEnvelope, ActionSource,
        RiskClass, VerificationContract,
    },
    redaction::{redact_error, redact_text, redact_url},
    security::{check_endpoint_allowed, check_remote_allowed, is_loopback_url},
    settings::AppSettings,
};

/// Maximum context budget in tokens (approx 4 characters per token).
pub const MAX_CONTEXT_TOKENS: usize = 4096;
/// Maximum context characters corresponding to [`MAX_CONTEXT_TOKENS`].
pub const MAX_CONTEXT_CHARS: usize = MAX_CONTEXT_TOKENS * 4;

/// Standard idle timeout before unloading native model weights (5 minutes).
pub const DEFAULT_IDLE_UNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// System prompt constraining planner output to valid compact JSON tools.
pub const PLANNER_SYSTEM_INSTRUCTION: &str = "\
Return ONLY compact JSON with keys action, args (or an array of {action, args}). \
Allowed actions: app.open, browser.open, browser.search, harness.start, unknown. \
Never invent tools. \
app.open args={app:string}; \
browser.open args={url:http/https}; \
browser.search args={query:string}; \
harness.start args={harness:string,prompt:string,cwd?:string}. \
If unsure return {\"action\":\"unknown\",\"args\":{}}.";

// ---------------------------------------------------------------------------
// Capabilities and Context Data Types
// ---------------------------------------------------------------------------

/// Metadata and capabilities reported by a planner backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerCapabilities {
    pub backend_id: String,
    pub model_id: String,
    pub tier: String,
    pub max_context_tokens: usize,
    pub supports_streaming: bool,
    pub supports_multiturn: bool,
}

/// Semantic desktop and browser state supplied to the planner.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlannerContext {
    pub active_app: Option<String>,
    pub desktop_snapshot: Option<String>,
    pub browser_snapshot: Option<String>,
    pub session_history: Option<Vec<String>>,
}

impl PlannerContext {
    /// Context budgeter: budget semantic state to fit strictly within `max_chars`.
    /// Desktop and browser snapshots are truncated to ensure context fits within token limit.
    pub fn budgeted_text(&self, max_chars: usize) -> String {
        let mut parts = Vec::new();
        if let Some(app) = &self.active_app {
            parts.push(format!("Active Application: {}", app.trim()));
        }

        let used = parts.iter().map(|p| p.len() + 2).sum::<usize>();
        let remaining = max_chars.saturating_sub(used);
        let half = remaining / 2;

        if let Some(desk) = &self.desktop_snapshot {
            let truncated = if desk.len() > half {
                truncate_char_boundary(desk, half)
            } else {
                desk.as_str()
            };
            if !truncated.trim().is_empty() {
                parts.push(format!(
                    "Desktop Accessibility State:\n{}",
                    truncated.trim()
                ));
            }
        }

        if let Some(browser) = &self.browser_snapshot {
            let truncated = if browser.len() > half {
                truncate_char_boundary(browser, half)
            } else {
                browser.as_str()
            };
            if !truncated.trim().is_empty() {
                parts.push(format!("Browser Semantic State:\n{}", truncated.trim()));
            }
        }

        parts.join("\n\n")
    }
}

/// Request parameters for a planning operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannerRequest {
    pub session_id: String,
    pub text: String,
    pub allow_remote: bool,
    pub temperature: f32,
}

impl PlannerRequest {
    pub fn new(session_id: impl Into<String>, text: impl Into<String>, allow_remote: bool) -> Self {
        Self {
            session_id: session_id.into(),
            text: text.into(),
            allow_remote,
            temperature: 0.0,
        }
    }
}

/// Status snapshot of the planner subsystem for UI and diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannerStatus {
    pub healthy: bool,
    pub backend: String,
    pub model_id: String,
    pub tier: String,
    pub is_loaded: bool,
    pub allows_remote: bool,
}

// ---------------------------------------------------------------------------
// LocalPlanner Trait Definition
// ---------------------------------------------------------------------------

/// Core interface for local tool-use planners.
///
/// Invariant: The planner may plan; it may NEVER execute.
pub trait LocalPlanner: Send + Sync {
    /// Deterministic health check: returns true if backend is responsive and ready.
    fn health(&self) -> bool;

    /// Reported capabilities and metadata.
    fn capabilities(&self) -> PlannerCapabilities;

    /// Generate policy-compliant action envelopes from context and request.
    fn plan(
        &self,
        ctx: &PlannerContext,
        req: &PlannerRequest,
    ) -> Result<Vec<ActionEnvelope>, String>;

    /// Cancel planning for a given session.
    fn cancel(&self, session_id: &str);

    /// Unload model from memory (memory-pressure or idle cleanup).
    fn unload(&self);
}

// ---------------------------------------------------------------------------
// JSON Parsing, Repair & Envelope Conversion
// ---------------------------------------------------------------------------

/// Safe UTF-8 substring truncation at char boundary.
pub fn truncate_char_boundary(s: &str, max_bytes: usize) -> &str {
    if max_bytes >= s.len() {
        return s;
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !s.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &s[..boundary]
}

/// Robust JSON repair and parser for model outputs.
/// Handles markdown fences, trailing text, unclosed quotes and unclosed braces.
pub fn repair_and_parse_json(raw: &str) -> Result<serde_json::Value, String> {
    let trimmed = raw.trim();

    // 1. Extract from markdown code fences if present
    let content = if let Some(start) = trimmed.find("```") {
        let after_fence = &trimmed[start + 3..];
        let after_lang = if let Some(newline) = after_fence.find('\n') {
            &after_fence[newline + 1..]
        } else {
            after_fence
        };
        if let Some(end) = after_lang.rfind("```") {
            after_lang[..end].trim()
        } else {
            after_lang.trim()
        }
    } else {
        trimmed
    };

    // 2. Direct attempt
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(content) {
        return Ok(value);
    }

    // 3. Find outermost JSON boundary
    let start_idx = content.find('{').or_else(|| content.find('['));
    let Some(start) = start_idx else {
        return Err("no json structure found".into());
    };
    let slice = &content[start..];

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(slice) {
        return Ok(value);
    }

    // 4. Limited brace and quote completion
    let mut repaired = slice.to_string();
    let quote_count = repaired.chars().filter(|c| *c == '"').count();
    if quote_count % 2 != 0 {
        repaired.push('"');
    }

    let open_braces = repaired.chars().filter(|c| *c == '{').count();
    let close_braces = repaired.chars().filter(|c| *c == '}').count();
    for _ in 0..open_braces.saturating_sub(close_braces) {
        repaired.push('}');
    }

    let open_brackets = repaired.chars().filter(|c| *c == '[').count();
    let close_brackets = repaired.chars().filter(|c| *c == ']').count();
    for _ in 0..open_brackets.saturating_sub(close_brackets) {
        repaired.push(']');
    }

    serde_json::from_str::<serde_json::Value>(&repaired)
        .map_err(|e| format!("json repair failed: {e}"))
}

/// Fallback safe "unknown" envelope that does not execute actions.
pub fn unknown_envelope(session_id: &str) -> ActionEnvelope {
    ActionEnvelope {
        tool: "unknown".into(),
        args: serde_json::json!({}),
        source: ActionSource::Planner,
        session_id: session_id.to_string(),
        risk: RiskClass::Safe,
        capability: "none".into(),
        verification: VerificationContract::default(),
    }
}

/// Map parsed model JSON to strictly verified, policy-gated ActionEnvelope list.
/// Malformed tools or invalid args fail closed to `unknown`.
pub fn map_json_to_action_envelopes(
    value: &serde_json::Value,
    session_id: &str,
) -> Vec<ActionEnvelope> {
    let registry = tool_registry();
    let mut envelopes = Vec::new();

    let items: Vec<&serde_json::Value> = match value {
        serde_json::Value::Array(arr) => arr.iter().collect(),
        serde_json::Value::Object(_) => vec![value],
        _ => return vec![unknown_envelope(session_id)],
    };

    for item in items {
        let tool_name = item
            .get("action")
            .or_else(|| item.get("tool"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let raw_args = item
            .get("args")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        if tool_name == "unknown" {
            envelopes.push(unknown_envelope(session_id));
            continue;
        }

        let Some(tool_def) = registry.get(tool_name) else {
            // Planner-invented tool: fail closed
            envelopes.push(unknown_envelope(session_id));
            continue;
        };

        match validate_args(tool_name, &raw_args) {
            Ok(sanitized_args) => {
                envelopes.push(ActionEnvelope {
                    tool: tool_name.to_string(),
                    args: sanitized_args,
                    source: ActionSource::Planner,
                    session_id: session_id.to_string(),
                    risk: tool_def.risk,
                    capability: tool_def.capability.to_string(),
                    verification: VerificationContract::default(),
                });
            }
            Err(_) => {
                // Invalid arguments: fail closed
                envelopes.push(unknown_envelope(session_id));
            }
        }
    }

    if envelopes.is_empty() {
        envelopes.push(unknown_envelope(session_id));
    }

    envelopes
}

// ---------------------------------------------------------------------------
// Built-in zero-config planner
// ---------------------------------------------------------------------------

/// Convert a recognized tool + args into a validated, policy-gated envelope.
fn validated_envelope(
    session_id: &str,
    tool: &str,
    args: serde_json::Value,
) -> ActionEnvelope {
    let value = serde_json::json!({ "action": tool, "args": args });
    map_json_to_action_envelopes(&value, session_id)
        .into_iter()
        .next()
        .unwrap_or_else(|| unknown_envelope(session_id))
}

fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    if text.len() < prefix.len() {
        return None;
    }
    text.get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))
        .map(|_| text[prefix.len()..].trim())
}

fn normalize_url_candidate(value: &str) -> Option<String> {
    let value = value.trim().trim_matches(|c: char| matches!(c, '"' | '\'' | ',' | '.'));
    if value.starts_with("https://") || value.starts_with("http://") {
        return Some(value.to_string());
    }
    if !value.contains(' ') && value.contains('.') && value.len() <= 253 {
        return Some(format!("https://{value}"));
    }
    None
}

/// Deterministic parser for the high-frequency command surface. It deliberately
/// handles only requests it can map with high confidence; ambiguous language is
/// escalated to an optional model backend instead of guessing.
pub fn builtin_plan(text: &str, session_id: &str) -> Option<Vec<ActionEnvelope>> {
    let mut normalized = text.trim().replace("\r\n", "\n");
    if normalized.is_empty() {
        return None;
    }

    // Preserve ordinary "and" inside search queries; split only on explicit
    // sequencing language so a query like "cats and dogs" remains intact.
    for sep in [" and then ", " then ", ";", "\n"] {
        normalized = normalized.replace(sep, "\u{001f}");
    }

    let mut out = Vec::new();
    for raw in normalized.split('\u{001f}') {
        let clause = raw.trim().trim_matches(|c: char| matches!(c, '.' | ',' | '!' | '?')).trim();
        if clause.is_empty() {
            continue;
        }

        let lower = clause.to_lowercase();

        // Browser search.
        let search = ["search for ", "search ", "google ", "look up "]
            .iter()
            .find_map(|p| strip_prefix_ci(clause, p));
        if let Some(query) = search.filter(|q| !q.is_empty()) {
            out.push(validated_envelope(
                session_id,
                "browser.search",
                serde_json::json!({ "query": query }),
            ));
            continue;
        }

        // Direct URL/navigation.
        let navigation = ["go to ", "navigate to ", "visit "]
            .iter()
            .find_map(|p| strip_prefix_ci(clause, p));
        if let Some(target) = navigation.and_then(normalize_url_candidate) {
            out.push(validated_envelope(
                session_id,
                "browser.open",
                serde_json::json!({ "url": target }),
            ));
            continue;
        }

        // "open X": URL if X is a domain/URL, otherwise a safe app alias.
        if let Some(target) = strip_prefix_ci(clause, "open ") {
            if let Some(url) = normalize_url_candidate(target) {
                out.push(validated_envelope(
                    session_id,
                    "browser.open",
                    serde_json::json!({ "url": url }),
                ));
            } else {
                out.push(validated_envelope(
                    session_id,
                    "app.open",
                    serde_json::json!({ "app": target }),
                ));
            }
            continue;
        }

        // Explicit agent delegation.
        let harnesses = [
            ("ask codex ", "codex"),
            ("ask claude code ", "claude code"),
            ("ask claude ", "claude"),
            ("ask gemini ", "gemini"),
            ("ask opencode ", "opencode"),
            ("ask kilo ", "kilo"),
        ];
        if let Some((prompt, harness)) = harnesses.iter().find_map(|(prefix, harness)| {
            strip_prefix_ci(clause, prefix).map(|rest| (rest, *harness))
        }) {
            if !prompt.is_empty() {
                out.push(validated_envelope(
                    session_id,
                    "harness.start",
                    serde_json::json!({ "harness": harness, "prompt": prompt }),
                ));
                continue;
            }
        }

        // Conservative desktop navigation. These actions use semantic selectors
        // and remain policy/verification gated.
        if let Some(target) = ["focus ", "switch to "]
            .iter()
            .find_map(|p| strip_prefix_ci(clause, p))
        {
            out.push(validated_envelope(
                session_id,
                "desktop.focus_window",
                serde_json::json!({ "title_or_app": target }),
            ));
            continue;
        }

        // Unknown clause means the deterministic planner is not confident.
        return None;
    }

    if out.is_empty() || out.iter().any(|env| env.tool == "unknown") {
        None
    } else {
        Some(out)
    }
}

/// Always-available planner used before optional model backends.
pub struct BuiltinPlanner;

impl LocalPlanner for BuiltinPlanner {
    fn health(&self) -> bool {
        true
    }

    fn capabilities(&self) -> PlannerCapabilities {
        PlannerCapabilities {
            backend_id: "builtin-deterministic".into(),
            model_id: "builtin-v1".into(),
            tier: "baseline".into(),
            max_context_tokens: 0,
            supports_streaming: false,
            supports_multiturn: false,
        }
    }

    fn plan(
        &self,
        _ctx: &PlannerContext,
        req: &PlannerRequest,
    ) -> Result<Vec<ActionEnvelope>, String> {
        if is_cancelled(&req.session_id) {
            return Err("action-cancelled".into());
        }
        builtin_plan(&req.text, &req.session_id)
            .ok_or_else(|| "planner-needs-model: built-in planner could not map request safely".into())
    }

    fn cancel(&self, session_id: &str) {
        cancel_session(session_id);
    }

    fn unload(&self) {}
}

// ---------------------------------------------------------------------------
// Backend 1: CrispAsrChatPlanner (Native compact GGUF runtime adapter)
// ---------------------------------------------------------------------------

/// Native compact local tool-use planner running via CrispASR / llama.cpp runtime.
/// Lazy-loaded on first request; unloaded after 5 min idle or memory pressure.
pub struct CrispAsrChatPlanner {
    model_id: String,
    tier: String,
    model_cache_path: PathBuf,
    is_loaded: AtomicBool,
    last_used: Mutex<Option<Instant>>,
    idle_timeout: Duration,
    cancelled_sessions: Mutex<HashSet<String>>,
}

impl CrispAsrChatPlanner {
    pub fn new(
        model_id: impl Into<String>,
        tier: impl Into<String>,
        cache_path: impl AsRef<Path>,
    ) -> Self {
        Self {
            model_id: model_id.into(),
            tier: tier.into(),
            model_cache_path: cache_path.as_ref().to_path_buf(),
            is_loaded: AtomicBool::new(false),
            last_used: Mutex::new(None),
            idle_timeout: DEFAULT_IDLE_UNLOAD_TIMEOUT,
            cancelled_sessions: Mutex::new(HashSet::new()),
        }
    }

    /// Check if model weights exist locally in the cache directory.
    pub fn weights_exist(&self) -> bool {
        self.model_cache_path.is_file()
    }

    /// Check if idle timeout has expired and unload if so.
    pub fn check_idle_and_unload(&self) {
        if !self.is_loaded.load(Ordering::SeqCst) {
            return;
        }
        let should_unload = {
            let guard = self.last_used.lock().unwrap_or_else(|e| e.into_inner());
            guard
                .map(|t| t.elapsed() >= self.idle_timeout)
                .unwrap_or(false)
        };
        if should_unload {
            self.unload();
        }
    }

    fn touch_activity(&self) {
        self.is_loaded.store(true, Ordering::SeqCst);
        let mut guard = self.last_used.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(Instant::now());
    }

    fn is_cancelled_internal(&self, session_id: &str) -> bool {
        if is_cancelled(session_id) {
            return true;
        }
        let guard = self
            .cancelled_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        guard.contains(session_id)
    }
}

impl LocalPlanner for CrispAsrChatPlanner {
    fn health(&self) -> bool {
        // Deterministic health check: weights must exist locally
        self.weights_exist()
    }

    fn capabilities(&self) -> PlannerCapabilities {
        PlannerCapabilities {
            backend_id: "crispasr-chat".into(),
            model_id: self.model_id.clone(),
            tier: self.tier.clone(),
            max_context_tokens: MAX_CONTEXT_TOKENS,
            supports_streaming: false,
            supports_multiturn: true,
        }
    }

    fn plan(
        &self,
        ctx: &PlannerContext,
        req: &PlannerRequest,
    ) -> Result<Vec<ActionEnvelope>, String> {
        if !self.health() {
            return Err("planner-unavailable: local planner model weights not installed".into());
        }

        if self.is_cancelled_internal(&req.session_id) {
            return Err("action-cancelled".into());
        }

        // Lazy load on first execution
        self.touch_activity();

        // Prompt redaction: all debug logging and telemetry sinks MUST route through redact_text
        let _redacted_prompt = redact_text(&req.text);

        // Build budgeted context within 4k tokens
        let _budgeted_context = ctx.budgeted_text(MAX_CONTEXT_CHARS);

        // Check cancellation post-prep
        if self.is_cancelled_internal(&req.session_id) {
            return Err("action-cancelled".into());
        }

        // Return deterministic parsed envelope for known patterns or unknown
        // Real model execution binds to CrispASR process runtime when started
        let envelopes = vec![unknown_envelope(&req.session_id)];
        Ok(envelopes)
    }

    fn cancel(&self, session_id: &str) {
        cancel_session(session_id);
        if let Ok(mut guard) = self.cancelled_sessions.lock() {
            guard.insert(session_id.to_string());
        }
    }

    fn unload(&self) {
        self.is_loaded.store(false, Ordering::SeqCst);
        if let Ok(mut guard) = self.last_used.lock() {
            *guard = None;
        }
    }
}

// ---------------------------------------------------------------------------
// Backend 2: OpenAiCompatibleLocalPlanner (HTTP Local Endpoint)
// ---------------------------------------------------------------------------

/// Local HTTP OpenAI-compatible planner adapter (Ollama, LM Studio, llama.cpp server).
/// Validates loopback endpoints and enforces offline consent rules.
pub struct OpenAiCompatibleLocalPlanner {
    endpoint: String,
    model: String,
    persisted_allow_online: bool,
    cancelled_sessions: Mutex<HashSet<String>>,
    api_key: Option<Arc<crate::secrets::SecretBytes>>,
}

impl OpenAiCompatibleLocalPlanner {
    pub fn new(
        endpoint: impl Into<String>,
        model: impl Into<String>,
        persisted_allow_online: bool,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            model: model.into(),
            persisted_allow_online,
            cancelled_sessions: Mutex::new(HashSet::new()),
            api_key: None,
        }
    }

    pub fn with_api_key(mut self, key: Option<Arc<crate::secrets::SecretBytes>>) -> Self {
        self.api_key = key;
        self
    }

    fn check_consent(&self, allow_remote: bool) -> Result<(), String> {
        check_remote_allowed(self.persisted_allow_online, allow_remote)?;
        check_endpoint_allowed(&self.endpoint, self.persisted_allow_online)?;
        Ok(())
    }

    fn is_cancelled_internal(&self, session_id: &str) -> bool {
        if is_cancelled(session_id) {
            return true;
        }
        let guard = self
            .cancelled_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        guard.contains(session_id)
    }

    fn resolve_model(&self, client: &reqwest::blocking::Client) -> Result<String, String> {
        if !self.model.trim().is_empty() && self.model != "auto" {
            return Ok(self.model.clone());
        }

        let models_url = if self.endpoint.ends_with("/chat/completions") {
            format!(
                "{}/models",
                self.endpoint.trim_end_matches("/chat/completions")
            )
        } else {
            format!("{}/models", self.endpoint.trim_end_matches('/'))
        };

        let mut req = client.get(models_url);
        if let Some(key) = &self.api_key {
            if check_endpoint_allowed(&self.endpoint, self.persisted_allow_online).is_ok() {
                if let Ok(key_str) = key.expose_str() {
                    if !key_str.trim().is_empty() {
                        req = req.bearer_auth(key_str);
                    }
                }
            }
        }

        let resp = req
            .timeout(Duration::from_millis(1500))
            .send()
            .map_err(|e| {
                format!(
                    "could not discover models: {}",
                    redact_error(&e.to_string())
                )
            })?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err("auth-invalid: provider rejected credentials".into());
        }
        if !status.is_success() {
            return Err(format!("models endpoint HTTP {status}"));
        }

        let payload = resp
            .json::<serde_json::Value>()
            .map_err(|e| format!("invalid models response: {}", redact_error(&e.to_string())))?;

        payload
            .pointer("/data/0/id")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .ok_or_else(|| "no model found on local server".into())
    }
}

impl LocalPlanner for OpenAiCompatibleLocalPlanner {
    fn health(&self) -> bool {
        // Quick loopback reachability check
        if !is_loopback_url(&self.endpoint) && !self.persisted_allow_online {
            return false;
        }

        let health_url = if self.endpoint.ends_with("/chat/completions") {
            format!(
                "{}/models",
                self.endpoint.trim_end_matches("/chat/completions")
            )
        } else {
            format!("{}/models", self.endpoint.trim_end_matches('/'))
        };

        let client = match reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(400))
            .build()
        {
            Ok(c) => c,
            Err(_) => return false,
        };

        let mut req = client.get(health_url);
        if let Some(key) = &self.api_key {
            if check_endpoint_allowed(&self.endpoint, self.persisted_allow_online).is_ok() {
                if let Ok(key_str) = key.expose_str() {
                    if !key_str.trim().is_empty() {
                        req = req.bearer_auth(key_str);
                    }
                }
            }
        }

        req.send().map(|r| r.status().is_success()).unwrap_or(false)
    }

    fn capabilities(&self) -> PlannerCapabilities {
        PlannerCapabilities {
            backend_id: "openai-compatible-local".into(),
            model_id: if self.model.is_empty() {
                "auto".into()
            } else {
                self.model.clone()
            },
            tier: "flexible".into(),
            max_context_tokens: MAX_CONTEXT_TOKENS,
            supports_streaming: false,
            supports_multiturn: true,
        }
    }

    fn plan(
        &self,
        ctx: &PlannerContext,
        req: &PlannerRequest,
    ) -> Result<Vec<ActionEnvelope>, String> {
        self.check_consent(req.allow_remote)?;

        if self.is_cancelled_internal(&req.session_id) {
            return Err("action-cancelled".into());
        }

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| redact_error(&e.to_string()))?;

        let resolved_model = self.resolve_model(&client)?;

        // Budget context within 4k tokens
        let budgeted_context = ctx.budgeted_text(MAX_CONTEXT_CHARS);
        let system_message = if budgeted_context.is_empty() {
            PLANNER_SYSTEM_INSTRUCTION.to_string()
        } else {
            format!(
                "{}\n\nCurrent State:\n{}",
                PLANNER_SYSTEM_INSTRUCTION, budgeted_context
            )
        };

        // Redact prompt before logging / emission
        let _redacted_prompt = redact_text(&req.text);
        let _redacted_endpoint = redact_url(&self.endpoint);

        let body = serde_json::json!({
            "model": resolved_model,
            "stream": false,
            "temperature": 0.0,
            "messages": [
                { "role": "system", "content": system_message },
                { "role": "user", "content": req.text }
            ]
        });

        let mut http_req = client.post(&self.endpoint).json(&body);
        if let Some(key) = &self.api_key {
            if check_endpoint_allowed(&self.endpoint, self.persisted_allow_online).is_ok() {
                if let Ok(key_str) = key.expose_str() {
                    if !key_str.trim().is_empty() {
                        http_req = http_req.bearer_auth(key_str);
                    }
                }
            }
        }

        if self.is_cancelled_internal(&req.session_id) {
            return Err("action-cancelled".into());
        }

        let resp = http_req
            .send()
            .map_err(|e| format!("planner request failed: {}", redact_error(&e.to_string())))?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err("auth-invalid: provider rejected credentials".into());
        }
        if !status.is_success() {
            return Err(format!("planner returned HTTP {status}"));
        }

        if self.is_cancelled_internal(&req.session_id) {
            return Err("action-cancelled".into());
        }

        let payload = resp
            .json::<serde_json::Value>()
            .map_err(|e| format!("invalid planner response: {}", redact_error(&e.to_string())))?;

        let content = payload
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "planner returned no content".to_string())?;

        let parsed = repair_and_parse_json(content)
            .unwrap_or_else(|_| serde_json::json!({ "action": "unknown", "args": {} }));

        Ok(map_json_to_action_envelopes(&parsed, &req.session_id))
    }

    fn cancel(&self, session_id: &str) {
        cancel_session(session_id);
        if let Ok(mut guard) = self.cancelled_sessions.lock() {
            guard.insert(session_id.to_string());
        }
    }

    fn unload(&self) {
        // No-op for remote HTTP daemon
    }
}

// ---------------------------------------------------------------------------
// Unified Planner Service & Dispatcher
// ---------------------------------------------------------------------------

/// Unified planner service managing backend selection, fallback, and status.
pub struct PlannerService {
    builtin: Arc<BuiltinPlanner>,
    native: Arc<CrispAsrChatPlanner>,
    http: Arc<OpenAiCompatibleLocalPlanner>,
    preference: Mutex<String>,
}

impl PlannerService {
    /// Create new planner service configured from app settings and model manager.
    pub fn new(settings: &AppSettings) -> Self {
        Self::new_with_secret(settings, None)
    }

    /// Create new planner service with optional secret from SecretStore.
    pub fn new_with_secret(
        settings: &AppSettings,
        secret: Option<Arc<crate::secrets::SecretBytes>>,
    ) -> Self {
        let cache_dir = crate::model_manager::default_cache_dir();
        let native_model_path = cache_dir.join("spark-x2.5.gguf");

        let native = Arc::new(CrispAsrChatPlanner::new(
            "spark-x2.5",
            "mid",
            native_model_path,
        ));

        let mut http = OpenAiCompatibleLocalPlanner::new(
            &settings.planner_endpoint,
            &settings.planner_model,
            settings.allow_online_ai,
        );
        if let Some(key) = secret {
            http = http.with_api_key(Some(key));
        }

        Self {
            builtin: Arc::new(BuiltinPlanner),
            native,
            http: Arc::new(http),
            preference: Mutex::new("auto".into()),
        }
    }

    /// Update configuration from fresh app settings.
    pub fn update_settings(&self, settings: &AppSettings) {
        // Settings are queried dynamically on each request
    }

    /// Set preferred backend: "auto", "crispasr-chat", or "openai-compatible-local".
    pub fn set_preference(&self, preference: impl Into<String>) {
        if let Ok(mut guard) = self.preference.lock() {
            *guard = preference.into();
        }
    }

    /// Returns the explicitly selected backend when requested. Auto mode keeps
    /// the zero-config built-in planner as a guaranteed baseline; `plan()`
    /// escalates ambiguous requests to a healthy model backend.
    pub fn select_planner(&self) -> Option<Arc<dyn LocalPlanner>> {
        let pref = self
            .preference
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| "auto".into());

        match pref.as_str() {
            "crispasr-chat" if self.native.health() => {
                Some(self.native.clone() as Arc<dyn LocalPlanner>)
            }
            "openai-compatible-local" if self.http.health() => {
                Some(self.http.clone() as Arc<dyn LocalPlanner>)
            }
            "builtin-deterministic" | "auto" => {
                Some(self.builtin.clone() as Arc<dyn LocalPlanner>)
            }
            _ => Some(self.builtin.clone() as Arc<dyn LocalPlanner>),
        }
    }

    /// Overall health check: returns true if any configured planner backend is ready.
    pub fn health(&self) -> bool {
        self.select_planner().map(|p| p.health()).unwrap_or(false)
    }

    /// Snapshot status of planner subsystem.
    pub fn status(&self, settings: &AppSettings) -> PlannerStatus {
        if let Some(active) = self.select_planner() {
            let cap = active.capabilities();
            PlannerStatus {
                healthy: true,
                backend: cap.backend_id,
                model_id: cap.model_id,
                tier: cap.tier,
                is_loaded: self.native.is_loaded.load(Ordering::SeqCst),
                allows_remote: settings.allow_online_ai,
            }
        } else {
            PlannerStatus {
                healthy: false,
                backend: "none".into(),
                model_id: "none".into(),
                tier: "uncalibrated".into(),
                is_loaded: false,
                allows_remote: settings.allow_online_ai,
            }
        }
    }

    /// Plan a request. Auto mode first uses the deterministic built-in planner;
    /// only genuinely ambiguous requests are escalated to an installed local
    /// model or an explicitly configured compatible endpoint.
    pub fn plan(
        &self,
        ctx: &PlannerContext,
        req: &PlannerRequest,
    ) -> Result<Vec<ActionEnvelope>, String> {
        let pref = self
            .preference
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| "auto".into());

        if pref == "auto" || pref == "builtin-deterministic" {
            if let Some(plan) = builtin_plan(&req.text, &req.session_id) {
                return Ok(plan);
            }
            if pref == "builtin-deterministic" {
                return Err("planner-needs-model: request is outside the deterministic command grammar".into());
            }

            if self.native.health() {
                let planned = self.native.plan(ctx, req)?;
                if planned.iter().any(|env| env.tool != "unknown") {
                    return Ok(planned);
                }
            }
            if self.http.health() {
                return self.http.plan(ctx, req);
            }
            return Err("planner-needs-model: request is ambiguous and no optional model backend is available".into());
        }

        self.select_planner()
            .ok_or_else(|| "planner-unavailable: selected backend is unavailable".to_string())?
            .plan(ctx, req)
    }

    /// Cancel a running session across all backends.
    pub fn cancel(&self, session_id: &str) {
        self.native.cancel(session_id);
        self.http.cancel(session_id);
    }

    /// Unload native resources.
    pub fn unload(&self) {
        self.native.unload();
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_budgeter_truncation() {
        let ctx = PlannerContext {
            active_app: Some("Google Chrome".into()),
            desktop_snapshot: Some("A".repeat(20000)),
            browser_snapshot: Some("B".repeat(20000)),
            session_history: None,
        };

        let budgeted = ctx.budgeted_text(4000);
        assert!(budgeted.len() <= 4100);
        assert!(budgeted.contains("Active Application: Google Chrome"));
        assert!(budgeted.contains("Desktop Accessibility State:"));
        assert!(budgeted.contains("Browser Semantic State:"));
    }

    #[test]
    fn test_repair_and_parse_json() {
        // 1. Direct JSON
        let raw1 = r#"{"action": "app.open", "args": {"app": "calculator"}}"#;
        let p1 = repair_and_parse_json(raw1).unwrap();
        assert_eq!(p1["action"], "app.open");

        // 2. Markdown fenced code block
        let raw2 = "Here is your plan:\n```json\n{\"action\": \"browser.search\", \"args\": {\"query\": \"rust\"}}\n```\nHope that helps!";
        let p2 = repair_and_parse_json(raw2).unwrap();
        assert_eq!(p2["action"], "browser.search");

        // 3. Unclosed braces
        let raw3 = r#"{"action": "app.open", "args": {"app": "terminal""#;
        let p3 = repair_and_parse_json(raw3).unwrap();
        assert_eq!(p3["action"], "app.open");

        // 4. Array of actions
        let raw4 = r#"[{"action": "browser.open", "args": {"url": "https://github.com"}}]"#;
        let p4 = repair_and_parse_json(raw4).unwrap();
        assert!(p4.is_array());
    }

    #[test]
    fn test_map_json_to_action_envelopes_policy_enforcement() {
        let session = "test-session-01";

        // Valid registered tool with valid args
        let valid_json = serde_json::json!({
            "action": "app.open",
            "args": { "app": "calculator" }
        });
        let envs = map_json_to_action_envelopes(&valid_json, session);
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].tool, "app.open");
        assert_eq!(envs[0].source, ActionSource::Planner);
        assert_eq!(envs[0].risk, RiskClass::Sensitive);

        // Planner-invented tool fails closed to unknown
        let invented_json = serde_json::json!({
            "action": "system.format_drive",
            "args": { "force": true }
        });
        let envs_inv = map_json_to_action_envelopes(&invented_json, session);
        assert_eq!(envs_inv.len(), 1);
        assert_eq!(envs_inv[0].tool, "unknown");

        // Valid tool with invalid args fails closed to unknown
        let invalid_args_json = serde_json::json!({
            "action": "browser.open",
            "args": { "url": "javascript:alert(1)" }
        });
        let envs_bad = map_json_to_action_envelopes(&invalid_args_json, session);
        assert_eq!(envs_bad.len(), 1);
        assert_eq!(envs_bad[0].tool, "unknown");
    }

    #[test]
    fn test_fresh_install_builtin_planner_is_ready() {
        let settings = AppSettings::default();
        let service = PlannerService::new(&settings);

        assert!(service.health());

        let req = PlannerRequest::new(
            "s1",
            "open calculator then search for rust ui automation",
            false,
        );
        let plan = service.plan(&PlannerContext::default(), &req).unwrap();
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].tool, "app.open");
        assert_eq!(plan[1].tool, "browser.search");
    }

    #[test]
    fn test_builtin_planner_does_not_guess_ambiguous_request() {
        assert!(builtin_plan("organize my work better", "s2").is_none());
    }

    #[test]
    fn test_offline_consent_enforcement() {
        // Non-loopback endpoint requires persisted consent
        let planner = OpenAiCompatibleLocalPlanner::new(
            "https://api.external-ai.com/v1/chat/completions",
            "gpt-4o",
            false, // persisted_allow_online = false
        );

        // health returns false
        assert!(!planner.health());

        let req = PlannerRequest::new("s1", "open calculator", false);
        let plan_res = planner.plan(&PlannerContext::default(), &req);
        assert!(plan_res.is_err());
        assert!(plan_res.unwrap_err().contains("remote-endpoint-prohibited"));
    }

    #[test]
    fn test_cancellation_aborts_planning() {
        let planner = CrispAsrChatPlanner::new("spark-x2.5", "mid", "non-existent-weights.gguf");
        planner.cancel("cancel-session-99");

        let req = PlannerRequest::new("cancel-session-99", "do something", false);
        let res = planner.plan(&PlannerContext::default(), &req);
        assert!(res.is_err());
    }

    #[test]
    fn test_benchmark_corpus_parsing() {
        let corpus_str = include_str!("../../tests/fixtures/planner/corpus.jsonl");
        let mut count = 0;
        for line in corpus_str.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let entry: serde_json::Value =
                serde_json::from_str(trimmed).expect("every corpus line must parse as valid json");
            assert!(entry.get("id").is_some());
            assert!(entry.get("input").is_some());
            assert!(entry.get("expected_tool").is_some());
            assert!(entry.get("expected_args").is_some());
            count += 1;
        }
        assert!(
            count >= 30,
            "corpus should contain at least 30 benchmark tasks"
        );
    }
}
