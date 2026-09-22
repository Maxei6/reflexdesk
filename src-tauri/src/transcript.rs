//! Wave-0 Rust-owned transcript submission.
//!
//! Replaces the frontend-trusted `reflexdesk://transcript` broadcast path:
//! the overlay MUST call `submit_transcript` (overlay capability only) with a
//! Rust-issued session nonce. Rust validates bounds, then re-emits the
//! verified `reflexdesk://transcript-verified` event that `src/main.js`
//! executes from. The raw broadcast path is deleted from the frontend.

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

/// Max transcript payload (chars). Enforced before further processing so a
/// malicious renderer cannot force large allocations.
pub const MAX_TRANSCRIPT_CHARS: usize = 1_000;
/// Nonces live this long; overlay refreshes via `transcript_nonce`.
pub const NONCE_TTL_SECS: u64 = 300;

#[derive(Debug, Clone, Serialize)]
pub struct VerifiedTranscript {
    pub text: String,
    pub session_id: String,
    pub stt_latency_ms: Option<u64>,
}

struct NonceEntry {
    issued_at_secs: u64,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn new_nonce() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(32);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    format!("tn_{out}")
}

pub struct TranscriptGate {
    nonces: Mutex<HashMap<String, NonceEntry>>,
    sessions: Mutex<HashSet<String>>,
}

impl Default for TranscriptGate {
    fn default() -> Self {
        Self {
            nonces: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashSet::new()),
        }
    }
}

impl TranscriptGate {
    /// Issue a single-use session nonce for the overlay.
    pub fn issue_nonce(&self) -> String {
        let nonce = new_nonce();
        if let Ok(mut guard) = self.nonces.lock() {
            let now = now_secs();
            guard.retain(|_, e| now.saturating_sub(e.issued_at_secs) < NONCE_TTL_SECS);
            guard.insert(
                nonce.clone(),
                NonceEntry {
                    issued_at_secs: now,
                },
            );
        }
        nonce
    }

    fn consume_nonce(&self, nonce: &str) -> bool {
        let mut guard = match self.nonces.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        let Some(entry) = guard.remove(nonce) else {
            return false;
        };
        now_secs().saturating_sub(entry.issued_at_secs) < NONCE_TTL_SECS
    }

    /// Check whether a nonce is currently valid without consuming it.
    /// Used for gating intermediate streaming chunks/partials to the authenticated overlay session.
    pub fn is_nonce_valid(&self, nonce: &str) -> bool {
        let guard = match self.nonces.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        if let Some(entry) = guard.get(nonce) {
            now_secs().saturating_sub(entry.issued_at_secs) < NONCE_TTL_SECS
        } else {
            false
        }
    }

    /// Validate + emit. Returns the verified payload on success.
    pub fn submit(
        &self,
        app: &AppHandle,
        text: &str,
        nonce: &str,
        session_id: &str,
        stt_latency_ms: Option<u64>,
    ) -> Result<VerifiedTranscript, String> {
        if !self.consume_nonce(nonce) {
            return Err("invalid-transcript-nonce".into());
        }
        let clean = text.trim();
        if clean.is_empty() {
            return Err("invalid-args: transcript text is empty".into());
        }
        if clean.chars().count() > MAX_TRANSCRIPT_CHARS {
            return Err("invalid-args: transcript exceeds 1000 chars".into());
        }
        // Control-character / null-byte rejection (IPC hygiene).
        if clean.contains('\0') {
            return Err("invalid-args: transcript contains null byte".into());
        }
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.insert(session_id.to_string());
        }
        let verified = VerifiedTranscript {
            text: clean.to_string(),
            session_id: session_id.to_string(),
            stt_latency_ms,
        };
        let _ = app.emit("reflexdesk://transcript-verified", &verified);
        Ok(verified)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonce_is_single_use() {
        let gate = TranscriptGate::default();
        let n = gate.issue_nonce();
        assert!(gate.consume_nonce(&n));
        assert!(!gate.consume_nonce(&n));
        assert!(!gate.consume_nonce("tn_bogus"));
    }

    #[test]
    fn nonce_valid_without_consuming() {
        let gate = TranscriptGate::default();
        let n = gate.issue_nonce();
        assert!(gate.is_nonce_valid(&n));
        assert!(gate.is_nonce_valid(&n)); // still valid after checking
        assert!(!gate.is_nonce_valid("tn_bogus"));
        assert!(gate.consume_nonce(&n));
        assert!(!gate.is_nonce_valid(&n)); // invalid after consume
    }
}
