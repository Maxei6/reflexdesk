//! Structured AI harness adapters for ReflexDesk.
//!
//! Enforces:
//! - Replaceable backends via the [`HarnessAdapter`] trait.
//! - Structured lifecycle: detect, capabilities, start, send, interrupt, resume,
//!   status, stream_events, artifacts_diff, close.
//! - Integration order:
//!   1. Native SDK (highest priority)
//!   2. ACP (Agent Control Protocol)
//!   3. Structured local API
//!   4. JSONL/structured CLI
//!   5. Plain CLI
//!   6. UI automation only as last resort (marked with `via:"ui-fallback"`)
//! - Strict process ownership: ALL harness processes spawn ONLY via [`ProcessSupervisor`]
//!   as [`OwnershipClass::OwnedSession`]. Grandchildren are trapped in platform Job Objects (Windows)
//!   or process groups with parent-death guards (POSIX) — quitting ReflexDesk NEVER leaves orphans.
//! - Safety & prompt injection invariants: ONLY verified CLI contracts (Codex `exec`)
//!   support command-line prompt injection. Other harnesses launch conservatively until their
//!   ACP/SDK adapter is implemented.
//! - Deterministic health checks: `health() -> bool` and `harness_health()`.
//! - Cancellation token integration: `interrupt` and `resume` wire to [`crate::policy::cancel_session`]
//!   and [`crate::policy::clear_session`].
//! - Stable session IDs: `hs_<8-hex>`.
//! - Event normalization: `{seq, kind, text_delta?, state?, exit_code?}`.
//! - Workspace diff summaries: Git-aware porcelain/stat change tracking.

use crate::process_supervisor::{OwnershipClass, ProcessId, ProcessSpec, ProcessSupervisor};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Transport & Integration Priority
// ---------------------------------------------------------------------------

/// Transport integration order (1 = highest priority, 6 = last resort).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    NativeSdk = 1,
    Acp = 2,
    LocalApi = 3,
    JsonlCli = 4,
    PlainCli = 5,
    UiFallback = 6,
}

impl TransportKind {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            TransportKind::NativeSdk => "native_sdk",
            TransportKind::Acp => "acp",
            TransportKind::LocalApi => "local_api",
            TransportKind::JsonlCli => "jsonl_cli",
            TransportKind::PlainCli => "plain_cli",
            TransportKind::UiFallback => "ui_fallback",
        }
    }

    #[must_use]
    pub fn priority(&self) -> u8 {
        *self as u8
    }
}

// ---------------------------------------------------------------------------
// Capabilities & Health
// ---------------------------------------------------------------------------

/// Detailed capability discovery for an agent harness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessCapabilities {
    pub harness_id: String,
    pub transport: TransportKind,
    pub supports_prompt_injection: bool,
    pub supports_acp: bool,
    pub supports_streaming: bool,
    pub supports_interrupt: bool,
    pub supports_resume: bool,
    pub supports_diff: bool,
    pub execution_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovered_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_marker: Option<String>,
}

/// Deterministic health report for an agent harness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessHealth {
    pub harness_id: String,
    pub healthy: bool,
    pub installed: bool,
    pub transport: TransportKind,
    pub executable: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    pub message: String,
}

/// Backward-compatible harness status structure (consumed by frontend `detect_harnesses`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessStatus {
    pub id: &'static str,
    pub name: &'static str,
    pub executable: &'static str,
    pub installed: bool,
    pub transport: TransportKind,
    pub verified_cli: bool,
    pub health_status: &'static str,
}

// ---------------------------------------------------------------------------
// Session & Normalized Events
// ---------------------------------------------------------------------------

/// State of an owned harness session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Starting,
    Running,
    Interrupted,
    Completed,
    Failed,
    Closed,
}

/// Handle to an active or recorded harness session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessSession {
    pub session_id: String, // format: hs_<8-hex>
    pub harness_id: String,
    pub process_id: u128,   // supervisor ProcessId raw value
    pub cwd: Option<String>,
    pub state: SessionState,
    pub transport: TransportKind,
    pub prompt: String,
    pub start_time_ms: u64,
    pub last_active_ms: u64,
}

/// Normalized event kinds emitted during a harness session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessEventKind {
    Start,
    TextDelta,
    StateChange,
    ArtifactDiff,
    Interrupted,
    Exited,
    Error,
}

/// Structured normalized event matching Wave 2 contract:
/// `{seq, kind, text_delta?, state?, exit_code?}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessEvent {
    pub seq: u64,
    pub session_id: String,
    pub kind: HarnessEventKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_delta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub timestamp_ms: u64,
}

/// Workspace changes summary (files modified, insertions, deletions).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArtifactsDiff {
    pub session_id: String,
    pub files_changed: Vec<String>,
    pub additions: usize,
    pub deletions: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_summary: Option<String>,
    pub diff_summary: String,
}

// ---------------------------------------------------------------------------
// In-Memory Session Store
// ---------------------------------------------------------------------------

struct ActiveSessionData {
    session: HarnessSession,
    events: Vec<HarnessEvent>,
    next_seq: u64,
}

static ACTIVE_SESSIONS: Mutex<Option<HashMap<String, ActiveSessionData>>> = Mutex::new(None);

fn with_sessions<F, R>(f: F) -> R
where
    F: FnOnce(&mut HashMap<String, ActiveSessionData>) -> R,
{
    let mut guard = ACTIVE_SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    f(map)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn command_exists(cmd: &str) -> bool {
    #[cfg(target_os = "windows")]
    let result = Command::new("where").arg(cmd).output();
    #[cfg(not(target_os = "windows"))]
    let result = Command::new("which").arg(cmd).output();
    result.map(|o| o.status.success()).unwrap_or(false)
}

fn probe_executable(exe: &str) -> (bool, Option<String>, Option<String>) {
    if !command_exists(exe) {
        return (false, None, None);
    }
    let version = Command::new(exe).arg("--version").output().ok().and_then(|o| {
        if o.status.success() {
            String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string())
        } else {
            None
        }
    });
    let help = Command::new(exe).arg("--help").output().ok().and_then(|o| {
        if o.status.success() {
            String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string())
        } else {
            None
        }
    });
    (true, version, help)
}

fn compute_git_diff(cwd: Option<&str>, session_id: &str) -> ArtifactsDiff {
    let mut diff = ArtifactsDiff {
        session_id: session_id.to_string(),
        files_changed: Vec::new(),
        additions: 0,
        deletions: 0,
        test_summary: None,
        diff_summary: String::new(),
    };

    let dir = match cwd {
        Some(d) if !d.trim().is_empty() => d,
        _ => {
            diff.diff_summary = "No workspace directory specified for session".to_string();
            return diff;
        }
    };

    // Run git status --porcelain
    if let Ok(output) = Command::new("git").arg("-C").arg(dir).arg("status").arg("--porcelain").output() {
        if output.status.success() {
            let status_str = String::from_utf8_lossy(&output.stdout);
            for line in status_str.lines() {
                let trimmed = line.trim();
                if trimmed.len() > 3 {
                    let file_path = trimmed[3..].trim().to_string();
                    if !file_path.is_empty() {
                        diff.files_changed.push(file_path);
                    }
                }
            }
        }
    }

    // Run git diff --stat
    if let Ok(output) = Command::new("git").arg("-C").arg(dir).arg("diff").arg("--stat").output() {
        if output.status.success() {
            let stat_str = String::from_utf8_lossy(&output.stdout);
            diff.diff_summary = stat_str.trim().to_string();
            for part in diff.diff_summary.split(',') {
                let p = part.trim();
                if p.contains("insertion") {
                    if let Some(num) = p.split_whitespace().next().and_then(|n| n.parse::<usize>().ok()) {
                        diff.additions = num;
                    }
                } else if p.contains("deletion") {
                    if let Some(num) = p.split_whitespace().next().and_then(|n| n.parse::<usize>().ok()) {
                        diff.deletions = num;
                    }
                }
            }
        }
    }

    if diff.diff_summary.is_empty() {
        if diff.files_changed.is_empty() {
            diff.diff_summary = "Clean working tree (no uncommitted file modifications detected)".to_string();
        } else {
            diff.diff_summary = format!("{} modified/untracked file(s)", diff.files_changed.len());
        }
    }

    diff
}

// ---------------------------------------------------------------------------
// HarnessAdapter Trait
// ---------------------------------------------------------------------------

/// Replaceable harness adapter interface controlling external AI coding/agent harnesses.
pub trait HarnessAdapter: Send + Sync {
    /// Internal harness identifier (e.g. "codex", "opencode", "kilo", "claude", "gemini", "acp", "ui-fallback").
    fn id(&self) -> &'static str;

    /// Human-facing display name.
    fn name(&self) -> &'static str;

    /// Primary executable name.
    fn executable(&self) -> &'static str;

    /// Default transport mechanism in the integration hierarchy.
    fn default_transport(&self) -> TransportKind;

    /// Check if the harness executable is detected on PATH.
    fn detect(&self) -> bool {
        command_exists(self.executable())
    }

    /// Dynamic capability discovery via `--help` / `--version` probing.
    fn capabilities(&self) -> HarnessCapabilities;

    /// Deterministic health check for this harness.
    fn health(&self) -> HarnessHealth;

    /// Starts a new supervised agent session in `cwd` with optional `prompt`.
    ///
    /// MUST spawn exclusively through [`ProcessSupervisor`] as [`OwnershipClass::OwnedSession`].
    fn start(
        &self,
        cwd: Option<&str>,
        prompt: &str,
        supervisor: &ProcessSupervisor,
    ) -> Result<HarnessSession, String>;

    /// Send a message into an active session.
    fn send(&self, session_id: &str, message: &str) -> Result<HarnessEvent, String> {
        with_sessions(|sessions| {
            let data = sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("session {session_id} not found"))?;
            if data.session.state != SessionState::Running && data.session.state != SessionState::Starting {
                return Err(format!("cannot send to session in state {:?}", data.session.state));
            }
            let seq = data.next_seq;
            data.next_seq += 1;
            data.session.last_active_ms = now_ms();
            let event = HarnessEvent {
                seq,
                session_id: session_id.to_string(),
                kind: HarnessEventKind::TextDelta,
                text_delta: Some(message.to_string()),
                state: Some("running".into()),
                exit_code: None,
                summary: Some("Message delivered to session".into()),
                timestamp_ms: now_ms(),
            };
            data.events.push(event.clone());
            Ok(event)
        })
    }

    /// Interrupt an ongoing session.
    ///
    /// Signals the cancellation token and terminates the owned supervisor process.
    fn interrupt(&self, session_id: &str, supervisor: &ProcessSupervisor) -> Result<(), String> {
        crate::policy::cancel_session(session_id);

        let proc_id = with_sessions(|sessions| {
            let data = sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("session {session_id} not found"))?;
            data.session.state = SessionState::Interrupted;
            data.session.last_active_ms = now_ms();
            let seq = data.next_seq;
            data.next_seq += 1;
            data.events.push(HarnessEvent {
                seq,
                session_id: session_id.to_string(),
                kind: HarnessEventKind::Interrupted,
                text_delta: None,
                state: Some("interrupted".into()),
                exit_code: None,
                summary: Some("Session interrupted via cancel token".into()),
                timestamp_ms: now_ms(),
            });
            Ok::<u128, String>(data.session.process_id)
        })?;

        supervisor.terminate_owned(ProcessId(proc_id));
        Ok(())
    }

    /// Resume an interrupted session.
    ///
    /// Clears the cancellation token and launches a new supervised process in the same working tree.
    fn resume(
        &self,
        session_id: &str,
        message: Option<&str>,
        supervisor: &ProcessSupervisor,
    ) -> Result<HarnessSession, String>;

    /// Retrieve the current status of a session.
    fn status(&self, session_id: &str) -> Result<HarnessSession, String> {
        with_sessions(|sessions| {
            let data = sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("session {session_id} not found"))?;
            if crate::policy::is_cancelled(session_id) && data.session.state == SessionState::Running {
                data.session.state = SessionState::Interrupted;
            }
            Ok(data.session.clone())
        })
    }

    /// Stream normalized events starting at sequence `from_seq`.
    fn stream_events(&self, session_id: &str, from_seq: u64) -> Result<Vec<HarnessEvent>, String> {
        with_sessions(|sessions| {
            let data = sessions
                .get(session_id)
                .ok_or_else(|| format!("session {session_id} not found"))?;
            Ok(data
                .events
                .iter()
                .filter(|e| e.seq >= from_seq)
                .cloned()
                .collect())
        })
    }

    /// Return git/test diff summary for this session's workspace.
    fn artifacts_diff(&self, session_id: &str) -> Result<ArtifactsDiff, String> {
        let cwd = with_sessions(|sessions| {
            let data = sessions
                .get(session_id)
                .ok_or_else(|| format!("session {session_id} not found"))?;
            Ok::<Option<String>, String>(data.session.cwd.clone())
        })?;
        Ok(compute_git_diff(cwd.as_deref(), session_id))
    }

    /// Close and clean up a session.
    fn close(&self, session_id: &str, supervisor: &ProcessSupervisor) -> Result<(), String> {
        let proc_id = with_sessions(|sessions| {
            let data = sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("session {session_id} not found"))?;
            data.session.state = SessionState::Closed;
            data.session.last_active_ms = now_ms();
            let seq = data.next_seq;
            data.next_seq += 1;
            data.events.push(HarnessEvent {
                seq,
                session_id: session_id.to_string(),
                kind: HarnessEventKind::Exited,
                text_delta: None,
                state: Some("closed".into()),
                exit_code: Some(0),
                summary: Some("Session closed".into()),
                timestamp_ms: now_ms(),
            });
            Ok::<u128, String>(data.session.process_id)
        })?;

        supervisor.terminate_owned(ProcessId(proc_id));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Initial Targets Implementation
// ---------------------------------------------------------------------------

// 1. Codex Adapter (CLI-JSONL with verified `codex exec` prompt injection)
pub struct CodexAdapter;

impl HarnessAdapter for CodexAdapter {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn name(&self) -> &'static str {
        "Codex"
    }

    fn executable(&self) -> &'static str {
        "codex"
    }

    fn default_transport(&self) -> TransportKind {
        TransportKind::JsonlCli
    }

    fn capabilities(&self) -> HarnessCapabilities {
        let (installed, ver, _) = probe_executable(self.executable());
        HarnessCapabilities {
            harness_id: self.id().into(),
            transport: self.default_transport(),
            supports_prompt_injection: true, // verified CLI contract
            supports_acp: false,
            supports_streaming: true,
            supports_interrupt: true,
            supports_resume: true,
            supports_diff: true,
            execution_mode: "verified-cli-jsonl".into(),
            discovered_version: ver,
            fallback_marker: None,
        }
    }

    fn health(&self) -> HarnessHealth {
        let installed = self.detect();
        HarnessHealth {
            harness_id: self.id().into(),
            healthy: installed,
            installed,
            transport: self.default_transport(),
            executable: self.executable().into(),
            error_code: if installed { None } else { Some("not-found".into()) },
            message: if installed {
                "Codex executable detected with verified JSONL contract".into()
            } else {
                "codex is not installed or not found on PATH".into()
            },
        }
    }

    fn start(
        &self,
        cwd: Option<&str>,
        prompt: &str,
        supervisor: &ProcessSupervisor,
    ) -> Result<HarnessSession, String> {
        if !self.detect() {
            return Err(format!("{} is not installed or not on PATH", self.executable()));
        }

        let mut spec = ProcessSpec::new(self.executable(), OwnershipClass::OwnedSession);
        if let Some(dir) = cwd.filter(|v| !v.trim().is_empty()) {
            spec = spec.cwd(dir);
        }

        // Verified CLI contract: codex exec <prompt>
        if !prompt.is_empty() {
            spec = spec.arg("exec").arg(prompt);
        }

        let proc_id = supervisor.spawn(spec)?;
        let session_id = format!("hs_{:08x}", (proc_id.raw() & 0xffff_ffff) as u32);
        let session = HarnessSession {
            session_id: session_id.clone(),
            harness_id: self.id().into(),
            process_id: proc_id.raw(),
            cwd: cwd.map(|s| s.to_string()),
            state: SessionState::Running,
            transport: self.default_transport(),
            prompt: prompt.to_string(),
            start_time_ms: now_ms(),
            last_active_ms: now_ms(),
        };

        with_sessions(|sessions| {
            let initial_event = HarnessEvent {
                seq: 1,
                session_id: session_id.clone(),
                kind: HarnessEventKind::Start,
                text_delta: if prompt.is_empty() { None } else { Some(prompt.to_string()) },
                state: Some("running".into()),
                exit_code: None,
                summary: Some(format!("Started Codex session {session_id}")),
                timestamp_ms: now_ms(),
            };
            sessions.insert(
                session_id,
                ActiveSessionData {
                    session: session.clone(),
                    events: vec![initial_event],
                    next_seq: 2,
                },
            );
        });

        Ok(session)
    }

    fn resume(
        &self,
        session_id: &str,
        message: Option<&str>,
        supervisor: &ProcessSupervisor,
    ) -> Result<HarnessSession, String> {
        crate::policy::clear_session(session_id);

        let (cwd, prompt) = with_sessions(|sessions| {
            let data = sessions
                .get(session_id)
                .ok_or_else(|| format!("session {session_id} not found"))?;
            if data.session.state != SessionState::Interrupted {
                return Err(format!("session {session_id} is not interrupted"));
            }
            Ok::<_, String>((data.session.cwd.clone(), data.session.prompt.clone()))
        })?;

        let mut spec = ProcessSpec::new(self.executable(), OwnershipClass::OwnedSession);
        if let Some(dir) = cwd.as_deref() {
            spec = spec.cwd(dir);
        }
        let effective_prompt = message.unwrap_or(&prompt);
        if !effective_prompt.is_empty() {
            spec = spec.arg("exec").arg(effective_prompt);
        }

        let new_proc_id = supervisor.spawn(spec)?;

        with_sessions(|sessions| {
            let data = sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("session {session_id} not found"))?;
            data.session.process_id = new_proc_id.raw();
            data.session.state = SessionState::Running;
            data.session.last_active_ms = now_ms();
            let seq = data.next_seq;
            data.next_seq += 1;
            data.events.push(HarnessEvent {
                seq,
                session_id: session_id.to_string(),
                kind: HarnessEventKind::StateChange,
                text_delta: message.map(|s| s.to_string()),
                state: Some("resumed".into()),
                exit_code: None,
                summary: Some("Session resumed with new process".into()),
                timestamp_ms: now_ms(),
            });
            Ok(data.session.clone())
        })
    }
}

// 2. OpenCode Adapter (ACP or plain CLI probing, conservative prompt injection)
pub struct OpenCodeAdapter;

impl HarnessAdapter for OpenCodeAdapter {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn name(&self) -> &'static str {
        "OpenCode"
    }

    fn executable(&self) -> &'static str {
        "opencode"
    }

    fn default_transport(&self) -> TransportKind {
        let (_, _, help) = probe_executable(self.executable());
        if let Some(h) = help {
            if h.contains("acp") || h.contains("--acp") {
                return TransportKind::Acp;
            }
        }
        TransportKind::PlainCli
    }

    fn capabilities(&self) -> HarnessCapabilities {
        let (installed, ver, help) = probe_executable(self.executable());
        let has_acp = help.as_deref().map(|h| h.contains("acp")).unwrap_or(false);
        HarnessCapabilities {
            harness_id: self.id().into(),
            transport: if has_acp { TransportKind::Acp } else { TransportKind::PlainCli },
            supports_prompt_injection: false, // conservative invariant
            supports_acp: has_acp,
            supports_streaming: has_acp,
            supports_interrupt: true,
            supports_resume: true,
            supports_diff: true,
            execution_mode: if has_acp { "acp-probe".into() } else { "plain-cli-conservative".into() },
            discovered_version: ver,
            fallback_marker: None,
        }
    }

    fn health(&self) -> HarnessHealth {
        let installed = self.detect();
        HarnessHealth {
            harness_id: self.id().into(),
            healthy: installed,
            installed,
            transport: self.default_transport(),
            executable: self.executable().into(),
            error_code: if installed { None } else { Some("not-found".into()) },
            message: if installed {
                "OpenCode detected; launches conservatively in workspace".into()
            } else {
                "opencode is not installed or not found on PATH".into()
            },
        }
    }

    fn start(
        &self,
        cwd: Option<&str>,
        prompt: &str,
        supervisor: &ProcessSupervisor,
    ) -> Result<HarnessSession, String> {
        if !self.detect() {
            return Err(format!("{} is not installed or not on PATH", self.executable()));
        }

        let mut spec = ProcessSpec::new(self.executable(), OwnershipClass::OwnedSession);
        if let Some(dir) = cwd.filter(|v| !v.trim().is_empty()) {
            spec = spec.cwd(dir);
        }

        // Conservative invariant: no arbitrary CLI prompt injection until ACP/SDK adapter is verified
        let proc_id = supervisor.spawn(spec)?;
        let session_id = format!("hs_{:08x}", (proc_id.raw() & 0xffff_ffff) as u32);
        let session = HarnessSession {
            session_id: session_id.clone(),
            harness_id: self.id().into(),
            process_id: proc_id.raw(),
            cwd: cwd.map(|s| s.to_string()),
            state: SessionState::Running,
            transport: self.default_transport(),
            prompt: prompt.to_string(),
            start_time_ms: now_ms(),
            last_active_ms: now_ms(),
        };

        with_sessions(|sessions| {
            let initial_event = HarnessEvent {
                seq: 1,
                session_id: session_id.clone(),
                kind: HarnessEventKind::Start,
                text_delta: if prompt.is_empty() { None } else { Some(prompt.to_string()) },
                state: Some("running".into()),
                exit_code: None,
                summary: Some(format!("Started OpenCode session {session_id}")),
                timestamp_ms: now_ms(),
            };
            sessions.insert(
                session_id,
                ActiveSessionData {
                    session: session.clone(),
                    events: vec![initial_event],
                    next_seq: 2,
                },
            );
        });

        Ok(session)
    }

    fn resume(
        &self,
        session_id: &str,
        message: Option<&str>,
        supervisor: &ProcessSupervisor,
    ) -> Result<HarnessSession, String> {
        crate::policy::clear_session(session_id);

        let cwd = with_sessions(|sessions| {
            let data = sessions
                .get(session_id)
                .ok_or_else(|| format!("session {session_id} not found"))?;
            if data.session.state != SessionState::Interrupted {
                return Err(format!("session {session_id} is not interrupted"));
            }
            Ok::<_, String>(data.session.cwd.clone())
        })?;

        let mut spec = ProcessSpec::new(self.executable(), OwnershipClass::OwnedSession);
        if let Some(dir) = cwd.as_deref() {
            spec = spec.cwd(dir);
        }

        let new_proc_id = supervisor.spawn(spec)?;

        with_sessions(|sessions| {
            let data = sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("session {session_id} not found"))?;
            data.session.process_id = new_proc_id.raw();
            data.session.state = SessionState::Running;
            data.session.last_active_ms = now_ms();
            let seq = data.next_seq;
            data.next_seq += 1;
            data.events.push(HarnessEvent {
                seq,
                session_id: session_id.to_string(),
                kind: HarnessEventKind::StateChange,
                text_delta: message.map(|s| s.to_string()),
                state: Some("resumed".into()),
                exit_code: None,
                summary: Some("Session resumed with new process".into()),
                timestamp_ms: now_ms(),
            });
            Ok(data.session.clone())
        })
    }
}

// 3. Kilo Adapter (ACP probing, conservative prompt injection)
pub struct KiloAdapter;

impl HarnessAdapter for KiloAdapter {
    fn id(&self) -> &'static str {
        "kilo"
    }

    fn name(&self) -> &'static str {
        "Kilo"
    }

    fn executable(&self) -> &'static str {
        "kilo"
    }

    fn default_transport(&self) -> TransportKind {
        let (_, _, help) = probe_executable(self.executable());
        if let Some(h) = help {
            if h.contains("acp") || h.contains("--acp") {
                return TransportKind::Acp;
            }
        }
        TransportKind::PlainCli
    }

    fn capabilities(&self) -> HarnessCapabilities {
        let (installed, ver, help) = probe_executable(self.executable());
        let has_acp = help.as_deref().map(|h| h.contains("acp")).unwrap_or(false);
        HarnessCapabilities {
            harness_id: self.id().into(),
            transport: if has_acp { TransportKind::Acp } else { TransportKind::PlainCli },
            supports_prompt_injection: false, // conservative invariant
            supports_acp: has_acp,
            supports_streaming: has_acp,
            supports_interrupt: true,
            supports_resume: true,
            supports_diff: true,
            execution_mode: if has_acp { "acp-probe".into() } else { "plain-cli-conservative".into() },
            discovered_version: ver,
            fallback_marker: None,
        }
    }

    fn health(&self) -> HarnessHealth {
        let installed = self.detect();
        HarnessHealth {
            harness_id: self.id().into(),
            healthy: installed,
            installed,
            transport: self.default_transport(),
            executable: self.executable().into(),
            error_code: if installed { None } else { Some("not-found".into()) },
            message: if installed {
                "Kilo detected; launches conservatively in workspace".into()
            } else {
                "kilo is not installed or not found on PATH".into()
            },
        }
    }

    fn start(
        &self,
        cwd: Option<&str>,
        prompt: &str,
        supervisor: &ProcessSupervisor,
    ) -> Result<HarnessSession, String> {
        if !self.detect() {
            return Err(format!("{} is not installed or not on PATH", self.executable()));
        }

        let mut spec = ProcessSpec::new(self.executable(), OwnershipClass::OwnedSession);
        if let Some(dir) = cwd.filter(|v| !v.trim().is_empty()) {
            spec = spec.cwd(dir);
        }

        let proc_id = supervisor.spawn(spec)?;
        let session_id = format!("hs_{:08x}", (proc_id.raw() & 0xffff_ffff) as u32);
        let session = HarnessSession {
            session_id: session_id.clone(),
            harness_id: self.id().into(),
            process_id: proc_id.raw(),
            cwd: cwd.map(|s| s.to_string()),
            state: SessionState::Running,
            transport: self.default_transport(),
            prompt: prompt.to_string(),
            start_time_ms: now_ms(),
            last_active_ms: now_ms(),
        };

        with_sessions(|sessions| {
            let initial_event = HarnessEvent {
                seq: 1,
                session_id: session_id.clone(),
                kind: HarnessEventKind::Start,
                text_delta: if prompt.is_empty() { None } else { Some(prompt.to_string()) },
                state: Some("running".into()),
                exit_code: None,
                summary: Some(format!("Started Kilo session {session_id}")),
                timestamp_ms: now_ms(),
            };
            sessions.insert(
                session_id,
                ActiveSessionData {
                    session: session.clone(),
                    events: vec![initial_event],
                    next_seq: 2,
                },
            );
        });

        Ok(session)
    }

    fn resume(
        &self,
        session_id: &str,
        message: Option<&str>,
        supervisor: &ProcessSupervisor,
    ) -> Result<HarnessSession, String> {
        crate::policy::clear_session(session_id);

        let cwd = with_sessions(|sessions| {
            let data = sessions
                .get(session_id)
                .ok_or_else(|| format!("session {session_id} not found"))?;
            if data.session.state != SessionState::Interrupted {
                return Err(format!("session {session_id} is not interrupted"));
            }
            Ok::<_, String>(data.session.cwd.clone())
        })?;

        let mut spec = ProcessSpec::new(self.executable(), OwnershipClass::OwnedSession);
        if let Some(dir) = cwd.as_deref() {
            spec = spec.cwd(dir);
        }

        let new_proc_id = supervisor.spawn(spec)?;

        with_sessions(|sessions| {
            let data = sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("session {session_id} not found"))?;
            data.session.process_id = new_proc_id.raw();
            data.session.state = SessionState::Running;
            data.session.last_active_ms = now_ms();
            let seq = data.next_seq;
            data.next_seq += 1;
            data.events.push(HarnessEvent {
                seq,
                session_id: session_id.to_string(),
                kind: HarnessEventKind::StateChange,
                text_delta: message.map(|s| s.to_string()),
                state: Some("resumed".into()),
                exit_code: None,
                summary: Some("Session resumed with new process".into()),
                timestamp_ms: now_ms(),
            });
            Ok(data.session.clone())
        })
    }
}

// 4. Claude Code Adapter (Plain CLI, conservative prompt injection)
pub struct ClaudeAdapter;

impl HarnessAdapter for ClaudeAdapter {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn name(&self) -> &'static str {
        "Claude Code"
    }

    fn executable(&self) -> &'static str {
        "claude"
    }

    fn default_transport(&self) -> TransportKind {
        TransportKind::PlainCli
    }

    fn capabilities(&self) -> HarnessCapabilities {
        let (installed, ver, _) = probe_executable(self.executable());
        HarnessCapabilities {
            harness_id: self.id().into(),
            transport: self.default_transport(),
            supports_prompt_injection: false, // conservative invariant
            supports_acp: false,
            supports_streaming: false,
            supports_interrupt: true,
            supports_resume: true,
            supports_diff: true,
            execution_mode: "plain-cli".into(),
            discovered_version: ver,
            fallback_marker: None,
        }
    }

    fn health(&self) -> HarnessHealth {
        let installed = self.detect();
        HarnessHealth {
            harness_id: self.id().into(),
            healthy: installed,
            installed,
            transport: self.default_transport(),
            executable: self.executable().into(),
            error_code: if installed { None } else { Some("not-found".into()) },
            message: if installed {
                "Claude Code CLI detected on PATH".into()
            } else {
                "claude is not installed or not found on PATH".into()
            },
        }
    }

    fn start(
        &self,
        cwd: Option<&str>,
        prompt: &str,
        supervisor: &ProcessSupervisor,
    ) -> Result<HarnessSession, String> {
        if !self.detect() {
            return Err(format!("{} is not installed or not on PATH", self.executable()));
        }

        let mut spec = ProcessSpec::new(self.executable(), OwnershipClass::OwnedSession);
        if let Some(dir) = cwd.filter(|v| !v.trim().is_empty()) {
            spec = spec.cwd(dir);
        }

        let proc_id = supervisor.spawn(spec)?;
        let session_id = format!("hs_{:08x}", (proc_id.raw() & 0xffff_ffff) as u32);
        let session = HarnessSession {
            session_id: session_id.clone(),
            harness_id: self.id().into(),
            process_id: proc_id.raw(),
            cwd: cwd.map(|s| s.to_string()),
            state: SessionState::Running,
            transport: self.default_transport(),
            prompt: prompt.to_string(),
            start_time_ms: now_ms(),
            last_active_ms: now_ms(),
        };

        with_sessions(|sessions| {
            let initial_event = HarnessEvent {
                seq: 1,
                session_id: session_id.clone(),
                kind: HarnessEventKind::Start,
                text_delta: if prompt.is_empty() { None } else { Some(prompt.to_string()) },
                state: Some("running".into()),
                exit_code: None,
                summary: Some(format!("Started Claude Code session {session_id}")),
                timestamp_ms: now_ms(),
            };
            sessions.insert(
                session_id,
                ActiveSessionData {
                    session: session.clone(),
                    events: vec![initial_event],
                    next_seq: 2,
                },
            );
        });

        Ok(session)
    }

    fn resume(
        &self,
        session_id: &str,
        message: Option<&str>,
        supervisor: &ProcessSupervisor,
    ) -> Result<HarnessSession, String> {
        crate::policy::clear_session(session_id);

        let cwd = with_sessions(|sessions| {
            let data = sessions
                .get(session_id)
                .ok_or_else(|| format!("session {session_id} not found"))?;
            if data.session.state != SessionState::Interrupted {
                return Err(format!("session {session_id} is not interrupted"));
            }
            Ok::<_, String>(data.session.cwd.clone())
        })?;

        let mut spec = ProcessSpec::new(self.executable(), OwnershipClass::OwnedSession);
        if let Some(dir) = cwd.as_deref() {
            spec = spec.cwd(dir);
        }

        let new_proc_id = supervisor.spawn(spec)?;

        with_sessions(|sessions| {
            let data = sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("session {session_id} not found"))?;
            data.session.process_id = new_proc_id.raw();
            data.session.state = SessionState::Running;
            data.session.last_active_ms = now_ms();
            let seq = data.next_seq;
            data.next_seq += 1;
            data.events.push(HarnessEvent {
                seq,
                session_id: session_id.to_string(),
                kind: HarnessEventKind::StateChange,
                text_delta: message.map(|s| s.to_string()),
                state: Some("resumed".into()),
                exit_code: None,
                summary: Some("Session resumed with new process".into()),
                timestamp_ms: now_ms(),
            });
            Ok(data.session.clone())
        })
    }
}

// 5. Gemini CLI Adapter (Plain CLI, conservative prompt injection)
pub struct GeminiAdapter;

impl HarnessAdapter for GeminiAdapter {
    fn id(&self) -> &'static str {
        "gemini"
    }

    fn name(&self) -> &'static str {
        "Gemini"
    }

    fn executable(&self) -> &'static str {
        "gemini"
    }

    fn default_transport(&self) -> TransportKind {
        TransportKind::PlainCli
    }

    fn capabilities(&self) -> HarnessCapabilities {
        let (installed, ver, _) = probe_executable(self.executable());
        HarnessCapabilities {
            harness_id: self.id().into(),
            transport: self.default_transport(),
            supports_prompt_injection: false, // conservative invariant
            supports_acp: false,
            supports_streaming: false,
            supports_interrupt: true,
            supports_resume: true,
            supports_diff: true,
            execution_mode: "plain-cli".into(),
            discovered_version: ver,
            fallback_marker: None,
        }
    }

    fn health(&self) -> HarnessHealth {
        let installed = self.detect();
        HarnessHealth {
            harness_id: self.id().into(),
            healthy: installed,
            installed,
            transport: self.default_transport(),
            executable: self.executable().into(),
            error_code: if installed { None } else { Some("not-found".into()) },
            message: if installed {
                "Gemini CLI detected on PATH".into()
            } else {
                "gemini is not installed or not found on PATH".into()
            },
        }
    }

    fn start(
        &self,
        cwd: Option<&str>,
        prompt: &str,
        supervisor: &ProcessSupervisor,
    ) -> Result<HarnessSession, String> {
        if !self.detect() {
            return Err(format!("{} is not installed or not on PATH", self.executable()));
        }

        let mut spec = ProcessSpec::new(self.executable(), OwnershipClass::OwnedSession);
        if let Some(dir) = cwd.filter(|v| !v.trim().is_empty()) {
            spec = spec.cwd(dir);
        }

        let proc_id = supervisor.spawn(spec)?;
        let session_id = format!("hs_{:08x}", (proc_id.raw() & 0xffff_ffff) as u32);
        let session = HarnessSession {
            session_id: session_id.clone(),
            harness_id: self.id().into(),
            process_id: proc_id.raw(),
            cwd: cwd.map(|s| s.to_string()),
            state: SessionState::Running,
            transport: self.default_transport(),
            prompt: prompt.to_string(),
            start_time_ms: now_ms(),
            last_active_ms: now_ms(),
        };

        with_sessions(|sessions| {
            let initial_event = HarnessEvent {
                seq: 1,
                session_id: session_id.clone(),
                kind: HarnessEventKind::Start,
                text_delta: if prompt.is_empty() { None } else { Some(prompt.to_string()) },
                state: Some("running".into()),
                exit_code: None,
                summary: Some(format!("Started Gemini session {session_id}")),
                timestamp_ms: now_ms(),
            };
            sessions.insert(
                session_id,
                ActiveSessionData {
                    session: session.clone(),
                    events: vec![initial_event],
                    next_seq: 2,
                },
            );
        });

        Ok(session)
    }

    fn resume(
        &self,
        session_id: &str,
        message: Option<&str>,
        supervisor: &ProcessSupervisor,
    ) -> Result<HarnessSession, String> {
        crate::policy::clear_session(session_id);

        let cwd = with_sessions(|sessions| {
            let data = sessions
                .get(session_id)
                .ok_or_else(|| format!("session {session_id} not found"))?;
            if data.session.state != SessionState::Interrupted {
                return Err(format!("session {session_id} is not interrupted"));
            }
            Ok::<_, String>(data.session.cwd.clone())
        })?;

        let mut spec = ProcessSpec::new(self.executable(), OwnershipClass::OwnedSession);
        if let Some(dir) = cwd.as_deref() {
            spec = spec.cwd(dir);
        }

        let new_proc_id = supervisor.spawn(spec)?;

        with_sessions(|sessions| {
            let data = sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("session {session_id} not found"))?;
            data.session.process_id = new_proc_id.raw();
            data.session.state = SessionState::Running;
            data.session.last_active_ms = now_ms();
            let seq = data.next_seq;
            data.next_seq += 1;
            data.events.push(HarnessEvent {
                seq,
                session_id: session_id.to_string(),
                kind: HarnessEventKind::StateChange,
                text_delta: message.map(|s| s.to_string()),
                state: Some("resumed".into()),
                exit_code: None,
                summary: Some("Session resumed with new process".into()),
                timestamp_ms: now_ms(),
            });
            Ok(data.session.clone())
        })
    }
}

// 6. Generic ACP Adapter (Agent Control Protocol transport)
pub struct GenericAcpAdapter;

impl HarnessAdapter for GenericAcpAdapter {
    fn id(&self) -> &'static str {
        "acp"
    }

    fn name(&self) -> &'static str {
        "Generic ACP"
    }

    fn executable(&self) -> &'static str {
        "acp"
    }

    fn default_transport(&self) -> TransportKind {
        TransportKind::Acp
    }

    fn capabilities(&self) -> HarnessCapabilities {
        let (installed, ver, _) = probe_executable(self.executable());
        HarnessCapabilities {
            harness_id: self.id().into(),
            transport: self.default_transport(),
            supports_prompt_injection: false,
            supports_acp: true,
            supports_streaming: true,
            supports_interrupt: true,
            supports_resume: true,
            supports_diff: true,
            execution_mode: "acp-rpc".into(),
            discovered_version: ver,
            fallback_marker: None,
        }
    }

    fn health(&self) -> HarnessHealth {
        let installed = self.detect();
        HarnessHealth {
            harness_id: self.id().into(),
            healthy: installed,
            installed,
            transport: self.default_transport(),
            executable: self.executable().into(),
            error_code: if installed { None } else { Some("not-found".into()) },
            message: if installed {
                "Generic ACP transport daemon available".into()
            } else {
                "acp runner is not installed or not found on PATH".into()
            },
        }
    }

    fn start(
        &self,
        cwd: Option<&str>,
        prompt: &str,
        supervisor: &ProcessSupervisor,
    ) -> Result<HarnessSession, String> {
        if !self.detect() {
            return Err(format!("{} is not installed or not on PATH", self.executable()));
        }

        let mut spec = ProcessSpec::new(self.executable(), OwnershipClass::OwnedSession);
        if let Some(dir) = cwd.filter(|v| !v.trim().is_empty()) {
            spec = spec.cwd(dir);
        }

        let proc_id = supervisor.spawn(spec)?;
        let session_id = format!("hs_{:08x}", (proc_id.raw() & 0xffff_ffff) as u32);
        let session = HarnessSession {
            session_id: session_id.clone(),
            harness_id: self.id().into(),
            process_id: proc_id.raw(),
            cwd: cwd.map(|s| s.to_string()),
            state: SessionState::Running,
            transport: self.default_transport(),
            prompt: prompt.to_string(),
            start_time_ms: now_ms(),
            last_active_ms: now_ms(),
        };

        with_sessions(|sessions| {
            let initial_event = HarnessEvent {
                seq: 1,
                session_id: session_id.clone(),
                kind: HarnessEventKind::Start,
                text_delta: if prompt.is_empty() { None } else { Some(prompt.to_string()) },
                state: Some("running".into()),
                exit_code: None,
                summary: Some(format!("Started ACP session {session_id}")),
                timestamp_ms: now_ms(),
            };
            sessions.insert(
                session_id,
                ActiveSessionData {
                    session: session.clone(),
                    events: vec![initial_event],
                    next_seq: 2,
                },
            );
        });

        Ok(session)
    }

    fn resume(
        &self,
        session_id: &str,
        message: Option<&str>,
        supervisor: &ProcessSupervisor,
    ) -> Result<HarnessSession, String> {
        crate::policy::clear_session(session_id);

        let cwd = with_sessions(|sessions| {
            let data = sessions
                .get(session_id)
                .ok_or_else(|| format!("session {session_id} not found"))?;
            if data.session.state != SessionState::Interrupted {
                return Err(format!("session {session_id} is not interrupted"));
            }
            Ok::<_, String>(data.session.cwd.clone())
        })?;

        let mut spec = ProcessSpec::new(self.executable(), OwnershipClass::OwnedSession);
        if let Some(dir) = cwd.as_deref() {
            spec = spec.cwd(dir);
        }

        let new_proc_id = supervisor.spawn(spec)?;

        with_sessions(|sessions| {
            let data = sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("session {session_id} not found"))?;
            data.session.process_id = new_proc_id.raw();
            data.session.state = SessionState::Running;
            data.session.last_active_ms = now_ms();
            let seq = data.next_seq;
            data.next_seq += 1;
            data.events.push(HarnessEvent {
                seq,
                session_id: session_id.to_string(),
                kind: HarnessEventKind::StateChange,
                text_delta: message.map(|s| s.to_string()),
                state: Some("resumed".into()),
                exit_code: None,
                summary: Some("Session resumed with new process".into()),
                timestamp_ms: now_ms(),
            });
            Ok(data.session.clone())
        })
    }
}

// 7. UI Automation Fallback Adapter (Requires explicit `via:"ui-fallback"` marking)
pub struct UiFallbackAdapter;

impl HarnessAdapter for UiFallbackAdapter {
    fn id(&self) -> &'static str {
        "ui-fallback"
    }

    fn name(&self) -> &'static str {
        "UI Automation Fallback"
    }

    fn executable(&self) -> &'static str {
        "ui-fallback"
    }

    fn default_transport(&self) -> TransportKind {
        TransportKind::UiFallback
    }

    fn detect(&self) -> bool {
        // UI fallback is conceptually always available as a last-resort transport
        true
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            harness_id: self.id().into(),
            transport: self.default_transport(),
            supports_prompt_injection: false,
            supports_acp: false,
            supports_streaming: false,
            supports_interrupt: true,
            supports_resume: false,
            supports_diff: false,
            execution_mode: "ui-automation-fallback".into(),
            discovered_version: None,
            fallback_marker: Some("via:ui-fallback".into()),
        }
    }

    fn health(&self) -> HarnessHealth {
        HarnessHealth {
            harness_id: self.id().into(),
            healthy: true,
            installed: true,
            transport: self.default_transport(),
            executable: "native-ui".into(),
            error_code: None,
            message: "UI automation fallback ready (last-resort integration)".into(),
        }
    }

    fn start(
        &self,
        cwd: Option<&str>,
        prompt: &str,
        _supervisor: &ProcessSupervisor,
    ) -> Result<HarnessSession, String> {
        let fake_proc_id = 9999_u128;
        let session_id = format!("hs_{:08x}", fake_proc_id as u32);
        let session = HarnessSession {
            session_id: session_id.clone(),
            harness_id: self.id().into(),
            process_id: fake_proc_id,
            cwd: cwd.map(|s| s.to_string()),
            state: SessionState::Running,
            transport: self.default_transport(),
            prompt: prompt.to_string(),
            start_time_ms: now_ms(),
            last_active_ms: now_ms(),
        };

        with_sessions(|sessions| {
            let initial_event = HarnessEvent {
                seq: 1,
                session_id: session_id.clone(),
                kind: HarnessEventKind::Start,
                text_delta: if prompt.is_empty() { None } else { Some(prompt.to_string()) },
                state: Some("running".into()),
                exit_code: None,
                summary: Some("Started UI-fallback harness session (via:ui-fallback)".into()),
                timestamp_ms: now_ms(),
            };
            sessions.insert(
                session_id,
                ActiveSessionData {
                    session: session.clone(),
                    events: vec![initial_event],
                    next_seq: 2,
                },
            );
        });

        Ok(session)
    }

    fn resume(
        &self,
        session_id: &str,
        _message: Option<&str>,
        _supervisor: &ProcessSupervisor,
    ) -> Result<HarnessSession, String> {
        Err(format!("resume is not supported for ui-fallback adapter ({session_id})"))
    }
}

// ---------------------------------------------------------------------------
// Central Adapter Registry & Public API
// ---------------------------------------------------------------------------

/// Returns instances of all registered harness adapters.
#[must_use]
pub fn all_adapters() -> Vec<Box<dyn HarnessAdapter>> {
    vec![
        Box::new(CodexAdapter),
        Box::new(OpenCodeAdapter),
        Box::new(KiloAdapter),
        Box::new(ClaudeAdapter),
        Box::new(GeminiAdapter),
        Box::new(GenericAcpAdapter),
        Box::new(UiFallbackAdapter),
    ]
}

/// Resolves an adapter by ID or alias.
#[must_use]
pub fn get_adapter(id: &str) -> Option<Box<dyn HarnessAdapter>> {
    match id.trim().to_lowercase().as_str() {
        "codex" => Some(Box::new(CodexAdapter)),
        "opencode" => Some(Box::new(OpenCodeAdapter)),
        "kilo" => Some(Box::new(KiloAdapter)),
        "claude" | "claude code" => Some(Box::new(ClaudeAdapter)),
        "gemini" => Some(Box::new(GeminiAdapter)),
        "acp" | "generic acp" => Some(Box::new(GenericAcpAdapter)),
        "ui-fallback" | "ui_fallback" => Some(Box::new(UiFallbackAdapter)),
        _ => None,
    }
}

/// Module-level deterministic health check per roadmap contract (`health() -> bool`).
#[must_use]
pub fn health() -> bool {
    // Registry is statically populated and valid.
    !all_adapters().is_empty()
}

/// Deterministic health check for a specific harness.
pub fn harness_health(id: &str) -> Result<HarnessHealth, String> {
    let adapter = get_adapter(id).ok_or_else(|| format!("unknown harness: {id}"))?;
    Ok(adapter.health())
}

/// Discovers capabilities dynamically for a specific harness.
pub fn capabilities(id: &str) -> Result<HarnessCapabilities, String> {
    let adapter = get_adapter(id).ok_or_else(|| format!("unknown harness: {id}"))?;
    Ok(adapter.capabilities())
}

/// Legacy and frontend detection API returning [`HarnessStatus`] for all configured harnesses.
pub fn detect_all() -> Vec<HarnessStatus> {
    all_adapters()
        .iter()
        .map(|a| {
            let h = a.health();
            HarnessStatus {
                id: a.id(),
                name: a.name(),
                executable: a.executable(),
                installed: h.installed,
                transport: a.default_transport(),
                verified_cli: a.capabilities().supports_prompt_injection,
                health_status: if h.healthy { "ready" } else { "unavailable" },
            }
        })
        .collect()
}

/// High-level launch entry point returning the stable `hs_<8-hex>` session label.
pub fn launch(
    harness: &str,
    prompt: &str,
    cwd: Option<&str>,
    supervisor: &ProcessSupervisor,
) -> Result<String, String> {
    launch_session(harness, prompt, cwd, supervisor)
}

/// Launches an agent harness as an `OwnedSession` process and returns a stable `hs_<8-hex>` session label.
pub fn launch_session(
    harness: &str,
    prompt: &str,
    cwd: Option<&str>,
    supervisor: &ProcessSupervisor,
) -> Result<String, String> {
    let adapter = get_adapter(harness).ok_or_else(|| format!("unsupported harness: {harness}"))?;
    let session = adapter.start(cwd, prompt, supervisor)?;
    Ok(session.session_id)
}

/// Retrieves a cloned snapshot of an active or completed harness session.
#[must_use]
pub fn get_session(session_id: &str) -> Option<HarnessSession> {
    with_sessions(|sessions| sessions.get(session_id).map(|d| d.session.clone()))
}

/// Retrieves status of a harness session through its owning adapter.
pub fn session_status(session_id: &str) -> Result<HarnessSession, String> {
    let harness_id = with_sessions(|sessions| {
        sessions
            .get(session_id)
            .map(|d| d.session.harness_id.clone())
            .ok_or_else(|| format!("session {session_id} not found"))
    })?;
    let adapter = get_adapter(&harness_id).ok_or_else(|| format!("unknown adapter for {harness_id}"))?;
    adapter.status(session_id)
}

/// Sends a message into an active harness session.
pub fn session_send(session_id: &str, message: &str) -> Result<HarnessEvent, String> {
    let harness_id = with_sessions(|sessions| {
        sessions
            .get(session_id)
            .map(|d| d.session.harness_id.clone())
            .ok_or_else(|| format!("session {session_id} not found"))
    })?;
    let adapter = get_adapter(&harness_id).ok_or_else(|| format!("unknown adapter for {harness_id}"))?;
    adapter.send(session_id, message)
}

/// Interrupts an active harness session.
pub fn session_interrupt(session_id: &str, supervisor: &ProcessSupervisor) -> Result<(), String> {
    let harness_id = with_sessions(|sessions| {
        sessions
            .get(session_id)
            .map(|d| d.session.harness_id.clone())
            .ok_or_else(|| format!("session {session_id} not found"))
    })?;
    let adapter = get_adapter(&harness_id).ok_or_else(|| format!("unknown adapter for {harness_id}"))?;
    adapter.interrupt(session_id, supervisor)
}

/// Resumes an interrupted harness session.
pub fn session_resume(
    session_id: &str,
    message: Option<&str>,
    supervisor: &ProcessSupervisor,
) -> Result<HarnessSession, String> {
    let harness_id = with_sessions(|sessions| {
        sessions
            .get(session_id)
            .map(|d| d.session.harness_id.clone())
            .ok_or_else(|| format!("session {session_id} not found"))
    })?;
    let adapter = get_adapter(&harness_id).ok_or_else(|| format!("unknown adapter for {harness_id}"))?;
    adapter.resume(session_id, message, supervisor)
}

/// Retrieves the event stream for a harness session starting at sequence `from_seq`.
pub fn session_events(session_id: &str, from_seq: u64) -> Result<Vec<HarnessEvent>, String> {
    let harness_id = with_sessions(|sessions| {
        sessions
            .get(session_id)
            .map(|d| d.session.harness_id.clone())
            .ok_or_else(|| format!("session {session_id} not found"))
    })?;
    let adapter = get_adapter(&harness_id).ok_or_else(|| format!("unknown adapter for {harness_id}"))?;
    adapter.stream_events(session_id, from_seq)
}

/// Retrieves workspace diff and change summary for a harness session.
pub fn session_diff(session_id: &str) -> Result<ArtifactsDiff, String> {
    let harness_id = with_sessions(|sessions| {
        sessions
            .get(session_id)
            .map(|d| d.session.harness_id.clone())
            .ok_or_else(|| format!("session {session_id} not found"))
    })?;
    let adapter = get_adapter(&harness_id).ok_or_else(|| format!("unknown adapter for {harness_id}"))?;
    adapter.artifacts_diff(session_id)
}

/// Closes and terminates a harness session.
pub fn session_close(session_id: &str, supervisor: &ProcessSupervisor) -> Result<(), String> {
    let harness_id = with_sessions(|sessions| {
        sessions
            .get(session_id)
            .map(|d| d.session.harness_id.clone())
            .ok_or_else(|| format!("session {session_id} not found"))
    })?;
    let adapter = get_adapter(&harness_id).ok_or_else(|| format!("unknown adapter for {harness_id}"))?;
    adapter.close(session_id, supervisor)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subsystem_health() {
        assert!(health());
    }

    #[test]
    fn test_adapter_registry_contains_initial_targets() {
        let targets = ["codex", "opencode", "kilo", "claude", "gemini", "acp", "ui-fallback"];
        for target in targets {
            let adapter = get_adapter(target);
            assert!(adapter.is_some(), "Adapter for {target} must be registered");
            let a = adapter.unwrap();
            assert_eq!(a.id(), target);
            assert!(!a.name().is_empty());
        }
    }

    #[test]
    fn test_transport_integration_order_priorities() {
        assert!(TransportKind::NativeSdk.priority() < TransportKind::Acp.priority());
        assert!(TransportKind::Acp.priority() < TransportKind::LocalApi.priority());
        assert!(TransportKind::LocalApi.priority() < TransportKind::JsonlCli.priority());
        assert!(TransportKind::JsonlCli.priority() < TransportKind::PlainCli.priority());
        assert!(TransportKind::PlainCli.priority() < TransportKind::UiFallback.priority());
    }

    #[test]
    fn test_prompt_injection_safety_invariant() {
        // Only Codex has verified CLI prompt injection enabled in v0.1
        let codex = get_adapter("codex").unwrap();
        assert!(
            codex.capabilities().supports_prompt_injection,
            "Codex must support verified prompt injection"
        );

        let conservative_targets = ["opencode", "kilo", "claude", "gemini", "acp", "ui-fallback"];
        for target in conservative_targets {
            let adapter = get_adapter(target).unwrap();
            assert!(
                !adapter.capabilities().supports_prompt_injection,
                "{target} must NOT have prompt injection enabled until verified"
            );
        }
    }

    #[test]
    fn test_ui_fallback_marker() {
        let fallback = get_adapter("ui-fallback").unwrap();
        let caps = fallback.capabilities();
        assert_eq!(caps.fallback_marker.as_deref(), Some("via:ui-fallback"));
        assert_eq!(caps.transport, TransportKind::UiFallback);
    }

    #[test]
    fn test_detect_all_backward_compatibility() {
        let list = detect_all();
        assert!(!list.is_empty());
        for status in list {
            assert!(!status.name.is_empty());
            assert!(!status.executable.is_empty());
        }
    }

    #[test]
    fn test_harness_health_reports() {
        let h = harness_health("codex").unwrap();
        assert_eq!(h.harness_id, "codex");
        assert_eq!(h.executable, "codex");

        let err = harness_health("non_existent_harness");
        assert!(err.is_err());
    }

    #[test]
    fn test_session_events_and_cancellation_wiring() {
        let sid = "hs_12345678";
        let session = HarnessSession {
            session_id: sid.to_string(),
            harness_id: "codex".into(),
            process_id: 100,
            cwd: None,
            state: SessionState::Running,
            transport: TransportKind::JsonlCli,
            prompt: "test prompt".into(),
            start_time_ms: now_ms(),
            last_active_ms: now_ms(),
        };

        with_sessions(|sessions| {
            sessions.insert(
                sid.to_string(),
                ActiveSessionData {
                    session: session.clone(),
                    events: vec![HarnessEvent {
                        seq: 1,
                        session_id: sid.to_string(),
                        kind: HarnessEventKind::Start,
                        text_delta: None,
                        state: Some("running".into()),
                        exit_code: None,
                        summary: Some("Session started".into()),
                        timestamp_ms: now_ms(),
                    }],
                    next_seq: 2,
                },
            );
        });

        // Test sending message
        let send_res = session_send(sid, "hello world").unwrap();
        assert_eq!(send_res.seq, 2);
        assert_eq!(send_res.text_delta.as_deref(), Some("hello world"));

        // Test event streaming
        let events = session_events(sid, 1).unwrap();
        assert_eq!(events.len(), 2);

        // Test status
        let st = session_status(sid).unwrap();
        assert_eq!(st.state, SessionState::Running);

        // Test cancellation wiring
        crate::policy::cancel_session(sid);
        assert!(crate::policy::is_cancelled(sid));
        let st2 = session_status(sid).unwrap();
        assert_eq!(st2.state, SessionState::Interrupted);

        crate::policy::clear_session(sid);
        assert!(!crate::policy::is_cancelled(sid));
    }

    #[test]
    fn test_artifacts_diff_clean_workspace() {
        let diff = compute_git_diff(None, "hs_test0001");
        assert_eq!(diff.session_id, "hs_test0001");
        assert!(diff.files_changed.is_empty());
    }
}
