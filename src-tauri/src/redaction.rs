//! Wave-0 frozen redaction helpers. Every observability / diagnostics /
//! logging sink in later waves MUST route through these before persisting or
//! emitting anything that could carry user content.
//!
//! Allowlisted fields (the ONLY content classes that may leave the machine in
//! diagnostics): app/runtime versions, hardware class, lifecycle phases,
//! latency numbers, adapter availability. Everything else — audio bytes,
//! secrets, clipboard/file contents, full browser text, raw transcripts,
//! planner prompts — is deny-by-default.
use std::borrow::Cow;

/// Fields allowed verbatim in diagnostics/observability payloads.
/// Anything not on this list goes through `redact_text` or is dropped.
pub fn is_allowlisted_field(field: &str) -> bool {
    matches!(
        field,
        "app_version"
            | "build"
            | "arch"
            | "os"
            | "hardware_class"
            | "phase"
            | "latency_ms"
            | "stt_latency_ms"
            | "benchmark_ms"
            | "adapter"
            | "adapter_available"
            | "runtime_version"
            | "model_id"
            | "model_revision"
            | "outcome"
            | "code"
            | "component"
            | "op"
            | "session"
            | "v"
            | "ts"
    )
}

const SECRET_KEYS: &[&str] = &[
    "api_key",
    "apikey",
    "token",
    "password",
    "secret",
    "authorization",
    "bearer",
    "private_key",
    "client_secret",
];

fn contains_secret_key(lower: &str) -> bool {
    SECRET_KEYS.iter().any(|k| lower.contains(k))
}

/// Redact free text: never return user content verbatim. Short
/// status fragments (<= 24 chars, no secret shape) pass through so UI status
/// strings stay readable; everything else becomes a length-only placeholder.
pub fn redact_text(text: &str) -> Cow<'_, str> {
    if text.is_empty() {
        return Cow::Borrowed("");
    }
    let lower = text.to_lowercase();
    if contains_secret_key(&lower) {
        return Cow::Owned(format!("[redacted-secret {} chars]", text.len()));
    }
    if text.len() <= 24 && !text.contains(['\n', '\r']) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(format!("[redacted {} chars]", text.chars().count()))
}

/// Redact a URL to scheme + host only. Strips userinfo, path, query, fragment
/// (all common exfiltration / session-token carriers).
pub fn redact_url(url: &str) -> String {
    let without_scheme = match url.split_once("://") {
        Some((scheme, rest)) => {
            let host = rest
                .split(['/', '?', '#', '@'])
                .last()
                .unwrap_or(rest);
            // If there was userinfo, `split('@')` above kept only the tail;
            // host is the last segment which is correct for host-only output.
            return format!("{scheme}://{host}");
        }
        None => url,
    };
    let _ = without_scheme;
    // No scheme: return host up to first delimiter.
    url.split(['/', '?', '#', ' '])
        .next()
        .unwrap_or("[redacted-url]")
        .to_string()
}

/// Redact an error string: removes `key=value` / `key: value` secret pairs,
/// bearer tokens, and long hex blobs that look like keys.
pub fn redact_error(err: &str) -> String {
    let mut out = String::with_capacity(err.len().min(512));
    for token in err.split_whitespace() {
        let lower = token.to_lowercase();
        if contains_secret_key(&lower) {
            out.push_str("[redacted-secret] ");
        } else if token.len() >= 32 && token.chars().all(|c| c.is_ascii_hexdigit()) {
            out.push_str("[redacted-hex] ");
        } else if lower.starts_with("bearer_") || lower.starts_with("sk-") {
            out.push_str("[redacted-token] ");
        } else {
            // Clip `key=secret-value` pairs to `key=[redacted]`.
            if let Some((k, _)) = token.split_once('=') {
                if contains_secret_key(&k.to_lowercase()) {
                    out.push_str(k);
                    out.push_str("=[redacted] ");
                    continue;
                }
            }
            out.push_str(token);
            out.push(' ');
        }
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_gates_versions_not_transcripts() {
        assert!(is_allowlisted_field("app_version"));
        assert!(is_allowlisted_field("latency_ms"));
        assert!(!is_allowlisted_field("transcript"));
        assert!(!is_allowlisted_field("audio"));
        assert!(!is_allowlisted_field("prompt"));
    }

    #[test]
    fn long_text_never_passes_verbatim() {
        let raw = "this is a fairly long transcript the user spoke aloud";
        let redacted = redact_text(raw);
        assert!(!redacted.contains("transcript"));
        assert!(redacted.contains("redacted"));
    }

    #[test]
    fn secret_text_flagged() {
        let redacted = redact_text("my api_key is abcdef");
        assert!(redacted.contains("redacted-secret"));
    }

    #[test]
    fn url_keeps_host_only() {
        assert_eq!(
            redact_url("https://example.com/path?q=1#frag"),
            "https://example.com"
        );
        assert_eq!(
            redact_url("https://user:pass@example.com/a"),
            "https://example.com"
        );
    }

    #[test]
    fn error_scrubs_pairs() {
        let out = redact_error("request failed api_key=supersecret status=500");
        assert!(!out.contains("supersecret"));
        assert!(out.contains("status=500"));
    }
}
