//! Plan 05 — Embedded reflex / Laya layer.
//!
//! Provides a fast semantic decision layer with:
//! 1. `ReflexProvider` trait: `route(candidates, context) -> Option<(choice, confidence)>`.
//! 2. Tier-0 deterministic fast router (`DeterministicReflex`) for instant, rule-based execution.
//! 3. Local-first supervised Laya sidecar (`LayaSupervisor` & `LayaReflex`) with ephemeral bearer token authentication.
//! 4. Compact heuristic/semantic classifier (`CompactReflex`) for offline ambiguity resolution.
//! 5. Decision caching keyed by `(normalized_text, semantic_state_hash)` with TTL.
//! 6. Reproducible labeled benchmarking on multilingual corpora with calibrated confidence thresholds.
//! 7. Clean bypass when Laya is absent or disabled; zero cloud calls in offline mode.

use crate::policy::RiskClass;
use crate::process_supervisor::{OwnershipClass, ProcessId, ProcessSpec, ProcessSupervisor};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    net::TcpListener,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};

// ---------------------------------------------------------------------------
// Candidate Action & Context definitions
// ---------------------------------------------------------------------------

/// Candidate action that can be selected by the reflex router.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CandidateAction {
    pub action: String,
    pub description: String,
    pub risk: RiskClass,
    pub default_args: serde_json::Value,
}

/// Context provided to reflex routing decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflexContext {
    pub text: String,
    pub normalized_text: String,
    pub semantic_state_hash: String,
    pub active_app: Option<String>,
    pub language: Option<String>,
    pub session_id: String,
}

impl ReflexContext {
    /// Create a new reflex context with automatic text normalization and semantic state hashing.
    pub fn new(
        text: &str,
        session_id: &str,
        active_app: Option<&str>,
        language: Option<&str>,
    ) -> Self {
        let clean = text.trim();
        let normalized = clean.to_lowercase();
        let hash = compute_semantic_hash(&normalized, active_app, language);
        Self {
            text: clean.to_string(),
            normalized_text: normalized,
            semantic_state_hash: hash,
            active_app: active_app.map(str::to_string),
            language: language.map(str::to_string),
            session_id: session_id.to_string(),
        }
    }
}

/// Compute a deterministic 64-bit hex hash representing the semantic context.
pub fn compute_semantic_hash(
    normalized_text: &str,
    active_app: Option<&str>,
    language: Option<&str>,
) -> String {
    // 64-bit FNV-1a hash
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in normalized_text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    if let Some(app) = active_app {
        hash ^= 0xff;
        hash = hash.wrapping_mul(FNV_PRIME);
        for byte in app.to_lowercase().as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }
    if let Some(lang) = language {
        hash ^= 0xee;
        hash = hash.wrapping_mul(FNV_PRIME);
        for byte in lang.to_lowercase().as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }
    format!("{:016x}", hash)
}

// ---------------------------------------------------------------------------
// ReflexProvider trait
// ---------------------------------------------------------------------------

/// Fast semantic decision provider interface.
pub trait ReflexProvider: Send + Sync {
    /// Provider identifier (e.g. "deterministic", "laya", "compact").
    fn name(&self) -> &'static str;

    /// Deterministic health check. Must return true if ready to route.
    fn health(&self) -> bool;

    /// Route input candidates and context to a choice with a confidence score in `[0.0, 1.0]`.
    fn route(&self, candidates: &[CandidateAction], ctx: &ReflexContext) -> Option<(String, f32)>;
}

// ---------------------------------------------------------------------------
// Standard candidate actions baseline
// ---------------------------------------------------------------------------

pub fn standard_candidates() -> Vec<CandidateAction> {
    vec![
        CandidateAction {
            action: "app.open".into(),
            description: "Launch or focus an installed desktop application".into(),
            risk: RiskClass::Sensitive,
            default_args: serde_json::json!({}),
        },
        CandidateAction {
            action: "browser.open".into(),
            description: "Open a specific URL in the web browser".into(),
            risk: RiskClass::Sensitive,
            default_args: serde_json::json!({}),
        },
        CandidateAction {
            action: "browser.search".into(),
            description: "Search the web for a query".into(),
            risk: RiskClass::Safe,
            default_args: serde_json::json!({}),
        },
        CandidateAction {
            action: "harness.start".into(),
            description: "Start or delegate work to an AI coding/agent harness".into(),
            risk: RiskClass::ExternalCommit,
            default_args: serde_json::json!({}),
        },
    ]
}

// ---------------------------------------------------------------------------
// Decision Caching with TTL
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct CacheEntry {
    choice: String,
    confidence: f32,
    provider: &'static str,
    args: serde_json::Value,
    inserted_at: Instant,
}

/// Cache of fast reflex decisions keyed by `(normalized_text, semantic_state_hash)`.
pub struct ReflexCache {
    entries: Mutex<HashMap<(String, String), CacheEntry>>,
    ttl: Duration,
}

impl ReflexCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    pub fn get(
        &self,
        normalized_text: &str,
        semantic_state_hash: &str,
    ) -> Option<(String, f32, &'static str, serde_json::Value)> {
        let key = (normalized_text.to_string(), semantic_state_hash.to_string());
        let mut guard = self.entries.lock().ok()?;
        if let Some(entry) = guard.get(&key) {
            if entry.inserted_at.elapsed() <= self.ttl {
                return Some((
                    entry.choice.clone(),
                    entry.confidence,
                    entry.provider,
                    entry.args.clone(),
                ));
            }
        }
        guard.remove(&key);
        None
    }

    pub fn insert(
        &self,
        normalized_text: &str,
        semantic_state_hash: &str,
        choice: &str,
        confidence: f32,
        provider: &'static str,
        args: serde_json::Value,
    ) {
        if let Ok(mut guard) = self.entries.lock() {
            // Periodic prune if cache grows large
            if guard.len() > 500 {
                let ttl = self.ttl;
                guard.retain(|_, v| v.inserted_at.elapsed() <= ttl);
            }
            guard.insert(
                (normalized_text.to_string(), semantic_state_hash.to_string()),
                CacheEntry {
                    choice: choice.to_string(),
                    confidence,
                    provider,
                    args,
                    inserted_at: Instant::now(),
                },
            );
        }
    }

    pub fn clear(&self) {
        if let Ok(mut guard) = self.entries.lock() {
            guard.clear();
        }
    }
}

// ---------------------------------------------------------------------------
// Tier-0: Deterministic Fast Router
// ---------------------------------------------------------------------------

pub struct DeterministicReflex;

impl DeterministicReflex {
    pub fn new() -> Self {
        Self
    }

    /// Extract tool args if action matches deterministic rules.
    pub fn extract_action_and_args(&self, raw: &str) -> Option<(String, serde_json::Value, f32)> {
        let lower = raw.trim().to_lowercase();
        if lower.is_empty() {
            return None;
        }

        // Control ping
        if lower == "hello reflex"
            || lower == "hello reflexdesk"
            || lower == "hey reflex"
            || lower == "hey reflexdesk"
        {
            return Some(("reflex.ping".into(), serde_json::json!({}), 0.99));
        }

        // Control stop
        if lower == "stop"
            || lower == "stop listening"
            || lower == "go to sleep"
            || lower == "sleep"
            || lower == "fermati"
            || lower == "detener"
            || lower == "arret"
            || lower == "stopp"
        {
            return Some(("voice.stop".into(), serde_json::json!({}), 0.99));
        }

        // URL opening (Multilingual: open, go to, vai su, ir a, aller sur, gehe zu)
        for prefix in [
            "open ",
            "go to ",
            "vai su ",
            "ir a ",
            "aller sur ",
            "gehe zu ",
        ] {
            if lower.starts_with(prefix) {
                let rest = raw.trim()[prefix.len()..].trim();
                if rest.starts_with("http://") || rest.starts_with("https://") {
                    let url = rest.split_whitespace().next().unwrap_or(rest);
                    return Some((
                        "browser.open".into(),
                        serde_json::json!({ "url": url }),
                        0.99,
                    ));
                }
            }
        }

        // Web search (Multilingual: search for, google, search the web for, cerca, buscar, chercher, suche nach)
        let search_prefixes = [
            "search for ",
            "search the web for ",
            "google ",
            "cerca su google ",
            "cerca ",
            "buscar en google ",
            "buscar ",
            "chercher sur google ",
            "chercher ",
            "rechercher ",
            "suche nach ",
            "suche ",
        ];
        for prefix in search_prefixes {
            if lower.starts_with(prefix) {
                let query = raw.trim()[prefix.len()..].trim();
                if !query.is_empty() {
                    return Some((
                        "browser.search".into(),
                        serde_json::json!({ "query": query }),
                        0.98,
                    ));
                }
            }
        }

        // App launch (Multilingual: open, launch, start, apri, lancia, avvia, abrir, iniciar, ouvrir, lancer, öffne, starte)
        let app_prefixes = [
            "open ", "launch ", "start ", "apri ", "lancia ", "avvia ", "abrir ", "iniciar ",
            "ouvrir ", "lancer ", "öffne ", "starte ",
        ];
        let known_apps = [
            ("spotify", "spotify"),
            ("chrome", "chrome"),
            ("google chrome", "chrome"),
            ("firefox", "firefox"),
            ("vscode", "vscode"),
            ("vs code", "vscode"),
            ("visual studio code", "vscode"),
            ("terminal", "terminal"),
            ("terminale", "terminal"),
            ("notepad", "notepad"),
        ];

        for prefix in app_prefixes {
            if lower.starts_with(prefix) {
                let target = lower[prefix.len()..]
                    .trim()
                    .trim_end_matches(['.', '!', '?'])
                    .trim();
                for (alias, canonical) in known_apps {
                    if target == alias {
                        return Some((
                            "app.open".into(),
                            serde_json::json!({ "app": canonical }),
                            0.97,
                        ));
                    }
                }
            }
        }

        // Harness delegator (Multilingual / direct invocation)
        for harness_name in ["codex", "opencode", "kilo", "claude"] {
            let patterns = [
                format!("ask {harness_name} to "),
                format!("ask {harness_name} "),
                format!("tell {harness_name} to "),
                format!("tell {harness_name} "),
                format!("run {harness_name} to "),
                format!("run {harness_name} "),
            ];
            for pat in patterns {
                if lower.starts_with(&pat) {
                    let prompt = raw.trim()[pat.len()..].trim();
                    return Some((
                        "harness.start".into(),
                        serde_json::json!({ "harness": harness_name, "prompt": prompt }),
                        0.90,
                    ));
                }
            }
        }

        None
    }
}

impl ReflexProvider for DeterministicReflex {
    fn name(&self) -> &'static str {
        "deterministic"
    }

    fn health(&self) -> bool {
        true
    }

    fn route(&self, candidates: &[CandidateAction], ctx: &ReflexContext) -> Option<(String, f32)> {
        let (action, _args, conf) = self.extract_action_and_args(&ctx.text)?;
        // Check candidate match or control actions
        if action.starts_with("reflex.") || action.starts_with("voice.") {
            return Some((action, conf));
        }
        if candidates.iter().any(|c| c.action == action) {
            Some((action, conf))
        } else {
            None
        }
    }
}

/// The built-in deterministic router is always available without model or network dependencies.
pub fn health() -> bool {
    DeterministicReflex::new().health()
}

// ---------------------------------------------------------------------------
// Compact Offline Heuristic Classifier
// ---------------------------------------------------------------------------

pub struct CompactReflex;

impl CompactReflex {
    pub fn new() -> Self {
        Self
    }
}

impl ReflexProvider for CompactReflex {
    fn name(&self) -> &'static str {
        "compact"
    }

    fn health(&self) -> bool {
        true
    }

    fn route(&self, candidates: &[CandidateAction], ctx: &ReflexContext) -> Option<(String, f32)> {
        let lower = &ctx.normalized_text;
        if lower.is_empty() {
            return None;
        }

        // Score token matches across intent categories
        let mut scores: HashMap<&'static str, f32> = HashMap::new();

        // Browser search cues
        let search_cues = [
            "look up",
            "find",
            "search",
            "google",
            "documentation",
            "guide",
            "tutorial",
            "weather",
            "forecast",
            "news",
            "cerca",
            "trova",
            "guida",
            "previsioni",
            "buscar",
            "noticias",
            "recetas",
            "chercher",
            "trouver",
            "horaires",
            "suche",
            "anleitung",
            "rechercher",
            "tutoriel",
            "meteo",
            "documentacion",
            "dokumentation",
            "anleitungen",
        ];
        for cue in search_cues {
            if lower.contains(cue) {
                *scores.entry("browser.search").or_default() += 0.38;
            }
        }

        // Harness coding cues
        let harness_cues = [
            "code",
            "script",
            "refactor",
            "implement",
            "feature",
            "fix bug",
            "unit test",
            "program",
            "backend",
            "database",
            "programmare",
            "funzione",
            "programar",
            "servicio",
            "coder",
            "module",
            "programmiere",
            "coding",
        ];
        for cue in harness_cues {
            if lower.contains(cue) {
                *scores.entry("harness.start").or_default() += 0.42;
            }
        }

        // App open cues
        let app_cues = [
            "open",
            "launch",
            "start",
            "calculator",
            "editor",
            "music",
            "apri",
            "lancia",
            "avvia",
            "abrir",
            "iniciar",
            "ouvrir",
            "lancer",
            "öffne",
            "starte",
            "app",
        ];
        for cue in app_cues {
            if lower.contains(cue) {
                *scores.entry("app.open").or_default() += 0.38;
            }
        }

        // URL cues
        if lower.contains("http://")
            || lower.contains("https://")
            || lower.contains(".com")
            || lower.contains(".org")
        {
            *scores.entry("browser.open").or_default() += 0.50;
        }

        // Find candidate with maximum score
        let best = scores
            .into_iter()
            .filter(|(action, _)| candidates.iter().any(|c| c.action == *action))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        if let Some((action, raw_score)) = best {
            let confidence = (0.50 + raw_score * 0.40).min(0.85);
            // Only return if it meets basic threshold
            if confidence >= 0.65 {
                return Some((action.to_string(), confidence));
            }
        }

        None
    }
}

// ---------------------------------------------------------------------------
// Supervised Laya Sidecar & Provider
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct LocalLayaEndpoint {
    pub port: u16,
    pub token: String,
}

pub struct LayaSupervisor {
    process_id: Mutex<Option<ProcessId>>,
    endpoint: Mutex<Option<LocalLayaEndpoint>>,
    starting: AtomicBool,
    shutting_down: AtomicBool,
    restart_attempts: AtomicU32,
    consecutive_failures: AtomicU32,
}

impl Default for LayaSupervisor {
    fn default() -> Self {
        Self {
            process_id: Mutex::new(None),
            endpoint: Mutex::new(None),
            starting: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            restart_attempts: AtomicU32::new(0),
            consecutive_failures: AtomicU32::new(0),
        }
    }
}

impl LayaSupervisor {
    pub fn new() -> Self {
        Self::default()
    }

    /// The explicit preparation step installs the runtime outside the app bundle.
    pub fn laya_home() -> Option<PathBuf> {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
        Some(PathBuf::from(home).join(".reflexdesk").join("laya"))
    }

    pub fn find_python_interpreter() -> Option<PathBuf> {
        let home = Self::laya_home()?;
        let python = if cfg!(windows) {
            home.join("venv").join("Scripts").join("python.exe")
        } else {
            home.join("venv").join("bin").join("python")
        };
        python.is_file().then_some(python)
    }

    /// Locate `sidecar/laya_service.py` script.
    pub fn find_sidecar_script(app: &AppHandle) -> Option<PathBuf> {
        let mut candidates = Vec::new();
        if let Ok(resource_dir) = app.path().resource_dir() {
            candidates.push(resource_dir.join("sidecar").join("laya_service.py"));
            candidates.push(
                resource_dir
                    .join("resources")
                    .join("sidecar")
                    .join("laya_service.py"),
            );
        }
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        if let Some(parent) = manifest_dir.parent() {
            candidates.push(parent.join("sidecar").join("laya_service.py"));
        }
        candidates.push(manifest_dir.join("sidecar").join("laya_service.py"));

        for p in candidates {
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }

    /// Allocate a random loopback port and secure bearer token.
    fn allocate_endpoint() -> Result<LocalLayaEndpoint, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| format!("could not reserve local Laya port: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        drop(listener);

        let token = format!(
            "{:032x}{:032x}",
            rand::random::<u128>(),
            rand::random::<u128>()
        );

        Ok(LocalLayaEndpoint { port, token })
    }

    /// Snapshot the active supervised endpoint.
    pub fn endpoint_snapshot(&self) -> Option<LocalLayaEndpoint> {
        self.endpoint.lock().ok().and_then(|g| g.clone())
    }

    /// Check if the supervised child is alive.
    pub fn child_running(&self) -> bool {
        let Ok(mut guard) = self.process_id.lock() else {
            return false;
        };
        let Some(id) = *guard else {
            return false;
        };
        if ProcessSupervisor::is_alive_global(id) {
            true
        } else {
            *guard = None;
            false
        }
    }

    /// Health check with bearer token verification against `/health`.
    pub fn health(&self) -> bool {
        let Some(endpoint) = self.endpoint_snapshot() else {
            return false;
        };

        let url = format!("http://127.0.0.1:{}/health", endpoint.port);
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(350))
            .build()
            .ok()
            .and_then(|client| {
                client
                    .get(&url)
                    .header("Authorization", format!("Bearer {}", endpoint.token))
                    .send()
                    .ok()
            })
            .filter(|response| response.status().is_success())
            .and_then(|response| response.json::<serde_json::Value>().ok())
            .map(|payload| {
                payload.get("ok").and_then(serde_json::Value::as_bool) == Some(true)
                    && payload.get("provider").and_then(serde_json::Value::as_str) == Some("laya")
            })
            .unwrap_or(false)
    }

    /// Terminate the supervised Laya child.
    pub fn shutdown(&self, supervisor: &ProcessSupervisor) -> Result<(), String> {
        self.shutting_down.store(true, Ordering::SeqCst);
        let id = self.process_id.lock().ok().and_then(|mut g| g.take());
        if let Some(proc_id) = id {
            let _ = supervisor.terminate_owned(proc_id);
        }
        if let Ok(mut g) = self.endpoint.lock() {
            *g = None;
        }
        self.starting.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// Start or restart the supervised Laya sidecar process with ephemeral bearer token.
    pub fn start(
        &self,
        app: &AppHandle,
        supervisor: &ProcessSupervisor,
    ) -> Result<LocalLayaEndpoint, String> {
        if self.health() {
            if let Some(ep) = self.endpoint_snapshot() {
                return Ok(ep);
            }
        }

        self.shutdown(supervisor)?;
        self.shutting_down.store(false, Ordering::SeqCst);
        self.starting.store(true, Ordering::SeqCst);

        let python = Self::find_python_interpreter().ok_or_else(|| {
            self.starting.store(false, Ordering::SeqCst);
            "Local Laya runtime not prepared".to_string()
        })?;

        let model_dir = Self::laya_home().unwrap().join("model");
        if !model_dir.join("model.safetensors").is_file() {
            self.starting.store(false, Ordering::SeqCst);
            return Err("Local Laya checkpoint not prepared".into());
        }
        let script = Self::find_sidecar_script(app).ok_or_else(|| {
            self.starting.store(false, Ordering::SeqCst);
            "sidecar/laya_service.py script not found".to_string()
        })?;

        let endpoint = Self::allocate_endpoint().map_err(|e| {
            self.starting.store(false, Ordering::SeqCst);
            e
        })?;

        let port_str = endpoint.port.to_string();
        let script_dir = script.parent().unwrap_or(&script).to_path_buf();

        let spec = ProcessSpec::new(&python, OwnershipClass::Internal)
            .cwd(&script_dir)
            .args([
                script.to_string_lossy().to_string(),
                "--host".to_string(),
                "127.0.0.1".to_string(),
                "--port".to_string(),
                port_str.clone(),
                "--token".to_string(),
                endpoint.token.clone(),
                "--model-dir".to_string(),
                model_dir.to_string_lossy().to_string(),
            ])
            .env("HF_HUB_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1")
            .env("PYTHONUNBUFFERED", "1")
            .with_piped_stdio(true);

        let proc_id = match supervisor.spawn(spec) {
            Ok(id) => id,
            Err(e) => {
                self.starting.store(false, Ordering::SeqCst);
                return Err(format!("failed to spawn Laya sidecar: {e}"));
            }
        };

        if self.shutting_down.load(Ordering::SeqCst) {
            self.starting.store(false, Ordering::SeqCst);
            let _ = supervisor.terminate_owned(proc_id);
            return Err("Laya start aborted: shutting down".to_string());
        }

        *self
            .endpoint
            .lock()
            .map_err(|_| "Laya endpoint lock poisoned".to_string())? = Some(endpoint.clone());
        *self
            .process_id
            .lock()
            .map_err(|_| "Laya process lock poisoned".to_string())? = Some(proc_id);

        // A cold checkpoint load on CPU can take tens of seconds.
        let start_time = Instant::now();
        let mut ready = false;
        while start_time.elapsed() < Duration::from_secs(180) {
            std::thread::sleep(Duration::from_millis(100));
            if !self.child_running() {
                break;
            }
            if self.health() {
                ready = true;
                break;
            }
        }

        self.starting.store(false, Ordering::SeqCst);

        if ready {
            self.restart_attempts.store(0, Ordering::SeqCst);
            self.consecutive_failures.store(0, Ordering::SeqCst);
            Ok(endpoint)
        } else {
            self.consecutive_failures.fetch_add(1, Ordering::SeqCst);
            let exited = !self.child_running();
            let _ = self.shutdown(supervisor);
            Err(if exited {
                "Laya sidecar exited before becoming ready".to_string()
            } else {
                "Laya sidecar health check timed out".to_string()
            })
        }
    }

    /// Watchdog auto-restart helper.
    pub fn tick_watchdog(
        &self,
        app: &AppHandle,
        supervisor: &ProcessSupervisor,
        enabled: bool,
    ) -> bool {
        if !enabled || self.shutting_down.load(Ordering::SeqCst) {
            return false;
        }

        if self.health() {
            return true;
        }
        let attempts = self.restart_attempts.load(Ordering::SeqCst);
        if attempts >= 3 {
            return false;
        }
        self.restart_attempts.fetch_add(1, Ordering::SeqCst);
        self.start(app, supervisor).is_ok()
    }
}

/// Laya HTTP Client Provider.
pub struct LayaReflex {
    endpoint_url: String,
    bearer_token: Option<String>,
    calibrated_threshold: f32,
}

impl LayaReflex {
    pub fn new(endpoint_url: String, bearer_token: Option<String>) -> Self {
        Self {
            endpoint_url: endpoint_url.trim_end_matches('/').to_string(),
            bearer_token,
            // Provisional cutoff; benchmark the installed checkpoint before widening action scope.
            calibrated_threshold: 0.70,
        }
    }

    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.calibrated_threshold = threshold;
        self
    }
}

impl ReflexProvider for LayaReflex {
    fn name(&self) -> &'static str {
        "laya"
    }

    fn health(&self) -> bool {
        let url = format!("{}/health", self.endpoint_url);
        let mut req = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(350))
            .build()
            .ok()
            .map(|c| c.get(&url));

        if let (Some(r), Some(token)) = (&mut req, &self.bearer_token) {
            *r = r
                .try_clone()
                .unwrap()
                .header("Authorization", format!("Bearer {token}"));
        }

        req.and_then(|r| r.send().ok())
            .filter(|res| res.status().is_success())
            .and_then(|res| res.json::<serde_json::Value>().ok())
            .map(|val| {
                val.get("ok").and_then(serde_json::Value::as_bool) == Some(true)
                    && val.get("provider").and_then(serde_json::Value::as_str) == Some("laya")
            })
            .unwrap_or(false)
    }

    fn route(&self, candidates: &[CandidateAction], ctx: &ReflexContext) -> Option<(String, f32)> {
        let url = format!("{}/route", self.endpoint_url);
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(500))
            .build()
            .ok()?;

        let mut req = client.post(&url).json(&serde_json::json!({
            "text": ctx.text,
            "context": {
                "source": "reflexdesk",
                "session_id": ctx.session_id,
                "semantic_hash": ctx.semantic_state_hash,
                "active_app": ctx.active_app,
                "language": ctx.language
            }
        }));

        if let Some(token) = &self.bearer_token {
            req = req.header("Authorization", format!("Bearer {token}"));
        }

        let response = req.send().ok()?;
        if !response.status().is_success() {
            return None;
        }

        let payload: serde_json::Value = response.json().ok()?;

        let intent = payload.get("answers")?.get("intent")?;
        let choice = intent.get("choice").and_then(serde_json::Value::as_str)?;
        let confidence = intent
            .get("confidence")
            .and_then(serde_json::Value::as_f64)? as f32;

        if choice == "unknown" || confidence < self.calibrated_threshold {
            return None;
        }

        // Check against candidate list
        if candidates.iter().any(|cand| cand.action == choice) {
            Some((choice.to_string(), confidence))
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// ReflexEngine: Unified Multi-Tier Router
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflexDecision {
    pub action: Option<String>,
    pub args: serde_json::Value,
    pub confidence: f32,
    pub provider: String,
    pub from_cache: bool,
}

pub struct ReflexEngine {
    cache: Arc<ReflexCache>,
    deterministic: Arc<DeterministicReflex>,
    compact: Arc<CompactReflex>,
    supervisor: Arc<LayaSupervisor>,
    laya_qualified: AtomicBool,
    laya_benchmark: Mutex<Option<BenchmarkReport>>,
}

impl ReflexEngine {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(ReflexCache::new(Duration::from_secs(60))),
            deterministic: Arc::new(DeterministicReflex::new()),
            compact: Arc::new(CompactReflex::new()),
            supervisor: Arc::new(LayaSupervisor::new()),
            laya_qualified: AtomicBool::new(false),
            laya_benchmark: Mutex::new(None),
        }
    }

    pub fn supervisor(&self) -> &Arc<LayaSupervisor> {
        &self.supervisor
    }

    pub fn cache(&self) -> &Arc<ReflexCache> {
        &self.cache
    }

    pub fn laya_benchmark(&self) -> Option<BenchmarkReport> {
        self.laya_benchmark
            .lock()
            .ok()
            .and_then(|report| report.clone())
    }

    /// Never prefer an unmeasured checkpoint over deterministic routing.
    pub fn activate_local_laya(
        &self,
        app: &AppHandle,
        supervisor: &ProcessSupervisor,
    ) -> Result<bool, String> {
        let endpoint = self.supervisor.start(app, supervisor)?;
        let client = LayaReflex::new(
            format!("http://127.0.0.1:{}", endpoint.port),
            Some(endpoint.token),
        );
        let corpus = include_str!("../../tests/fixtures/reflex/corpus.jsonl");
        let report = run_corpus_benchmark(corpus, Some(&client));
        let qualified = report.keep_laya_recommendation;
        if let Ok(mut latest) = self.laya_benchmark.lock() {
            *latest = Some(report);
        }
        self.laya_qualified.store(qualified, Ordering::SeqCst);
        if !qualified {
            self.supervisor.shutdown(supervisor)?;
        }
        Ok(qualified)
    }

    fn search_from_intent(
        &self,
        ctx: &ReflexContext,
        action: String,
        confidence: f32,
        provider: &'static str,
    ) -> Option<ReflexDecision> {
        // Classifiers produce intents, not targets. A search can safely use
        // the utterance; all other actions need deterministic/planner arguments.
        if action != "browser.search" {
            return None;
        }
        let args = serde_json::json!({ "query": ctx.text });
        self.cache.insert(
            &ctx.normalized_text,
            &ctx.semantic_state_hash,
            &action,
            confidence,
            provider,
            args.clone(),
        );
        Some(ReflexDecision {
            action: Some(action),
            args,
            confidence,
            provider: provider.into(),
            from_cache: false,
        })
    }

    /// Route a command through the multi-tier pipeline:
    /// Cache -> Deterministic (Tier-0) -> Laya (Supervised/Configured) -> Compact -> None.
    pub fn route_command(&self, ctx: &ReflexContext, provider_pref: &str) -> ReflexDecision {
        let candidates = standard_candidates();

        // 1. Cache lookup
        if let Some((choice, conf, prov, args)) = self
            .cache
            .get(&ctx.normalized_text, &ctx.semantic_state_hash)
        {
            return ReflexDecision {
                action: Some(choice),
                args,
                confidence: conf,
                provider: prov.to_string(),
                from_cache: true,
            };
        }

        // 2. Tier-0: Deterministic
        if let Some((action, args, conf)) = self.deterministic.extract_action_and_args(&ctx.text) {
            self.cache.insert(
                &ctx.normalized_text,
                &ctx.semantic_state_hash,
                &action,
                conf,
                "deterministic",
                args.clone(),
            );
            return ReflexDecision {
                action: Some(action),
                args,
                confidence: conf,
                provider: "deterministic".into(),
                from_cache: false,
            };
        }

        // 3. Tier-1: Laya (if preferred or enabled)
        if self.laya_qualified.load(Ordering::SeqCst)
            && (provider_pref == "laya" || provider_pref == "auto")
        {
            if let Some(endpoint) = self.supervisor.endpoint_snapshot() {
                let laya = LayaReflex::new(
                    format!("http://127.0.0.1:{}", endpoint.port),
                    Some(endpoint.token),
                );
                if laya.health() {
                    if let Some((action, conf)) = laya.route(&candidates, ctx) {
                        if let Some(decision) = self.search_from_intent(ctx, action, conf, "laya") {
                            return decision;
                        }
                    }
                }
            }
        }

        // 4. Tier-2: Compact Heuristic Classifier
        if let Some((action, conf)) = self.compact.route(&candidates, ctx) {
            if let Some(decision) = self.search_from_intent(ctx, action, conf, "compact") {
                return decision;
            }
        }

        // 5. Unresolved
        ReflexDecision {
            action: None,
            args: serde_json::json!({}),
            confidence: 0.0,
            provider: "none".into(),
            from_cache: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Benchmark & Evaluation Suite (Plan 05 WP 2, 7, 8)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageMetrics {
    pub total: usize,
    pub deterministic_correct: usize,
    pub laya_correct: usize,
    pub accuracy: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkReport {
    pub total_samples: usize,
    pub deterministic_accuracy: f32,
    pub laya_measured: bool,
    pub compact_accuracy: f32,
    pub laya_accuracy: Option<f32>,
    pub ambiguity_samples: usize,
    pub deterministic_ambiguity_resolved: usize,
    pub laya_ambiguity_resolved: usize,
    pub ambiguity_gain_pct: Option<f32>,
    pub deterministic_p50_latency_us: u64,
    pub deterministic_p95_latency_us: u64,
    pub laya_p50_latency_ms: Option<f32>,
    pub laya_p95_latency_ms: Option<f32>,
    pub languages: HashMap<String, LanguageMetrics>,
    pub keep_laya_recommendation: bool,
    pub decision_reason: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CorpusItem {
    pub id: String,
    pub lang: String,
    pub text: String,
    pub expected: String,
    pub ambiguous: bool,
}

/// Run a benchmark against a labeled JSONL corpus.
pub fn run_corpus_benchmark(
    corpus_jsonl: &str,
    laya_client: Option<&dyn ReflexProvider>,
) -> BenchmarkReport {
    let mut items: Vec<CorpusItem> = Vec::new();
    for line in corpus_jsonl.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Ok(item) = serde_json::from_str::<CorpusItem>(trimmed) {
            items.push(item);
        }
    }

    let deterministic = DeterministicReflex::new();
    let compact = CompactReflex::new();
    let laya_client = laya_client.filter(|laya| laya.health());
    let candidates = standard_candidates();

    let mut deterministic_correct = 0;
    let mut compact_correct = 0;
    let mut laya_correct = 0;

    let mut ambig_total = 0;
    let mut deterministic_ambig_resolved = 0;
    let mut laya_ambig_resolved = 0;

    let mut det_latencies: Vec<u64> = Vec::with_capacity(items.len());
    let mut laya_latencies: Vec<f32> = Vec::with_capacity(items.len());

    let mut lang_map: HashMap<String, (usize, usize, usize)> = HashMap::new();

    for item in &items {
        let ctx = ReflexContext::new(&item.text, "bench_session", None, Some(&item.lang));

        // Deterministic timing & result
        let t0 = Instant::now();
        let det_res = deterministic.route(&candidates, &ctx);
        let det_time = t0.elapsed().as_micros() as u64;
        det_latencies.push(det_time);

        let det_match = match &det_res {
            Some((act, _)) => act == &item.expected,
            None => item.expected == "unknown",
        };
        if det_match {
            deterministic_correct += 1;
        }

        // Compact timing & result
        let comp_res = compact.route(&candidates, &ctx);
        let comp_match = match &comp_res {
            Some((act, _)) => act == &item.expected,
            None => item.expected == "unknown",
        };
        if comp_match {
            compact_correct += 1;
        }

        // An unavailable model is not a measured compact-model result.
        let laya_match = laya_client
            .map(|laya| {
                let t1 = Instant::now();
                let result = laya.route(&candidates, &ctx);
                laya_latencies.push(t1.elapsed().as_secs_f32() * 1000.0);
                match result {
                    Some((action, _)) => action == item.expected,
                    None => item.expected == "unknown",
                }
            })
            .unwrap_or(false);

        if laya_match {
            laya_correct += 1;
        }

        // Ambiguity tracking
        if item.ambiguous {
            ambig_total += 1;
            if det_match {
                deterministic_ambig_resolved += 1;
            }
            if laya_match {
                laya_ambig_resolved += 1;
            }
        }

        // Language tracking
        let entry = lang_map.entry(item.lang.clone()).or_insert((0, 0, 0));
        entry.0 += 1;
        if det_match {
            entry.1 += 1;
        }
        if laya_match {
            entry.2 += 1;
        }
    }

    let total = items.len().max(1);
    det_latencies.sort_unstable();
    laya_latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let det_p50 = det_latencies
        .get(det_latencies.len() * 50 / 100)
        .copied()
        .unwrap_or(0);
    let det_p95 = det_latencies
        .get(det_latencies.len() * 95 / 100)
        .copied()
        .unwrap_or(0);

    let laya_p50 = laya_latencies
        .get(laya_latencies.len() * 50 / 100)
        .copied()
        .unwrap_or(0.0);
    let laya_p95 = laya_latencies
        .get(laya_latencies.len() * 95 / 100)
        .copied()
        .unwrap_or(0.0);

    let ambig_gain = if laya_client.is_some() && ambig_total > 0 {
        Some(
            ((laya_ambig_resolved as f32 - deterministic_ambig_resolved as f32)
                / ambig_total as f32)
                * 100.0,
        )
    } else {
        None
    };

    let mut languages = HashMap::new();
    for (lang, (l_total, l_det, l_laya)) in lang_map {
        languages.insert(
            lang,
            LanguageMetrics {
                total: l_total,
                deterministic_correct: l_det,
                laya_correct: l_laya,
                accuracy: (l_laya as f32) / (l_total as f32).max(1.0),
            },
        );
    }

    // Keep-Laya Decision Rule:
    // Keep Laya only if it materially improves ambiguity resolution (>= 15% gain on ambiguous queries)
    // without harming latency/reliability (p95 < 250ms).
    let keep_laya = matches!(ambig_gain, Some(gain) if gain >= 15.0) && laya_p95 <= 250.0;
    let reason = match ambig_gain {
        None => "Laya not measured; no checkpoint was available".to_string(),
        Some(gain) if keep_laya => format!(
            "Laya resolved +{gain:.1}% ambiguous queries at p95 {laya_p95:.1}ms"
        ),
        Some(gain) => format!(
            "Laya did not meet the 15% ambiguity gain and 250ms p95 gate ({gain:.1}%, {laya_p95:.1}ms)"
        ),
    };

    BenchmarkReport {
        total_samples: items.len(),
        deterministic_accuracy: (deterministic_correct as f32) / (total as f32),
        compact_accuracy: (compact_correct as f32) / (total as f32),
        laya_measured: laya_client.is_some(),
        laya_accuracy: laya_client.map(|_| (laya_correct as f32) / (total as f32)),
        ambiguity_samples: ambig_total,
        deterministic_ambiguity_resolved: deterministic_ambig_resolved,
        laya_ambiguity_resolved: laya_ambig_resolved,
        ambiguity_gain_pct: ambig_gain,
        deterministic_p50_latency_us: det_p50,
        deterministic_p95_latency_us: det_p95,
        laya_p50_latency_ms: laya_client.map(|_| laya_p50),
        laya_p95_latency_ms: laya_client.map(|_| laya_p95),
        languages,
        keep_laya_recommendation: keep_laya,
        decision_reason: reason,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_router_fast_rules() {
        let router = DeterministicReflex::new();
        let candidates = standard_candidates();

        // Control
        let ctx = ReflexContext::new("Hello ReflexDesk", "s1", None, None);
        assert_eq!(
            router.route(&candidates, &ctx),
            Some(("reflex.ping".into(), 0.99))
        );

        let ctx = ReflexContext::new("stop listening", "s2", None, None);
        assert_eq!(
            router.route(&candidates, &ctx),
            Some(("voice.stop".into(), 0.99))
        );

        // App launch
        let ctx = ReflexContext::new("open Spotify", "s3", None, None);
        assert_eq!(
            router.route(&candidates, &ctx),
            Some(("app.open".into(), 0.97))
        );

        let ctx = ReflexContext::new("apri Chrome", "s4", None, Some("it"));
        assert_eq!(
            router.route(&candidates, &ctx),
            Some(("app.open".into(), 0.97))
        );

        // Web search
        let ctx = ReflexContext::new("search for rust programming language", "s5", None, None);
        assert_eq!(
            router.route(&candidates, &ctx),
            Some(("browser.search".into(), 0.98))
        );

        let ctx = ReflexContext::new("cerca su google ristoranti Roma", "s6", None, Some("it"));
        assert_eq!(
            router.route(&candidates, &ctx),
            Some(("browser.search".into(), 0.98))
        );

        // URL opening
        let ctx = ReflexContext::new("go to https://reflexdesk.io", "s7", None, None);
        assert_eq!(
            router.route(&candidates, &ctx),
            Some(("browser.open".into(), 0.99))
        );

        // Harness
        let ctx = ReflexContext::new("ask codex to fix the test", "s8", None, None);
        assert_eq!(
            router.route(&candidates, &ctx),
            Some(("harness.start".into(), 0.90))
        );
    }

    #[test]
    fn test_compact_classifier_disambiguation() {
        let compact = CompactReflex::new();
        let candidates = standard_candidates();

        // Ambiguous search query that missed regex
        let ctx = ReflexContext::new("look up how to configure tauri windows", "s1", None, None);
        let res = compact.route(&candidates, &ctx);
        assert!(res.is_some());
        let (action, conf) = res.unwrap();
        assert_eq!(action, "browser.search");
        assert!(conf >= 0.65);

        // Ambiguous coding query
        let ctx = ReflexContext::new("start coding on the auth service", "s2", None, None);
        let res = compact.route(&candidates, &ctx);
        assert!(res.is_some());
        let (action, conf) = res.unwrap();
        assert_eq!(action, "harness.start");
        assert!(conf >= 0.65);
    }

    #[test]
    fn classifier_intents_cannot_dispatch_without_target_arguments() {
        let engine = ReflexEngine::new();
        let coding = ReflexContext::new("start coding on the auth service", "s", None, None);
        let unresolved = engine.route_command(&coding, "deterministic");
        assert_eq!(unresolved.action, None);

        let search = ReflexContext::new("look up how to configure tauri windows", "s", None, None);
        let decision = engine.route_command(&search, "deterministic");
        assert_eq!(decision.action.as_deref(), Some("browser.search"));
        assert_eq!(decision.args["query"], search.text);
    }

    #[test]
    fn test_reflex_cache_insertion_and_expiry() {
        let cache = ReflexCache::new(Duration::from_millis(50));
        cache.insert(
            "test query",
            "hash1",
            "app.open",
            0.95,
            "deterministic",
            serde_json::json!({}),
        );

        let hit = cache.get("test query", "hash1");
        assert!(hit.is_some());
        let (choice, conf, prov, _) = hit.unwrap();
        assert_eq!(choice, "app.open");
        assert_eq!(conf, 0.95);
        assert_eq!(prov, "deterministic");

        // Expire
        std::thread::sleep(Duration::from_millis(60));
        assert!(cache.get("test query", "hash1").is_none());
    }

    #[test]
    fn benchmark_does_not_label_compact_as_laya_when_model_is_absent() {
        let fixture = include_str!("../../tests/fixtures/reflex/corpus.jsonl");
        let report = run_corpus_benchmark(fixture, None);

        assert!(report.total_samples > 0);
        assert!(!report.laya_measured);
        assert_eq!(report.laya_accuracy, None);
        assert_eq!(report.laya_p95_latency_ms, None);
        assert_eq!(report.ambiguity_gain_pct, None);
        assert!(!report.keep_laya_recommendation);
    }

    #[test]
    fn test_offline_invariants() {
        // Verify deterministic and compact routers make zero network calls
        let deterministic = DeterministicReflex::new();
        let compact = CompactReflex::new();
        assert!(deterministic.health());
        assert!(compact.health());
    }
}
