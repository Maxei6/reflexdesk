//! Security validators and boundary controls for ReflexDesk IPC and external calls.
//!
//! Provides pure validation functions enforcing:
//! - Loopback URL parsing and validation (rejecting prefix tricks and userinfo)
//! - Remote endpoint policy (persisted user consent strictly gates remote AI)
//! - Audio bounds and IPC payload caps before memory allocation
//! - Tool argument schema validation (delegating to policy registry)
//! - External URL sanitization (rejecting shell metacharacters and unsafe schemes)

use url::Url;

/// Required sample rate for the local speech engine (fixed at 16 kHz).
pub const REQUIRED_SAMPLE_RATE_HZ: u32 = 16_000;

/// Minimum audio duration in milliseconds (100 ms).
pub const MIN_AUDIO_DURATION_MS: u64 = 100;

/// Maximum audio duration in milliseconds (30,000 ms = 30 s).
pub const MAX_AUDIO_DURATION_MS: u64 = 30_000;

/// Bytes per 16-bit mono PCM sample.
pub const BYTES_PER_SAMPLE: usize = 2;

/// Minimum audio payload in bytes (100 ms at 16 kHz, 16-bit mono = 3,200 bytes).
pub const MIN_AUDIO_BYTES: usize =
    (REQUIRED_SAMPLE_RATE_HZ as usize / 10) * BYTES_PER_SAMPLE;

/// Maximum audio payload in bytes (30 s at 16 kHz, 16-bit mono = 960,000 bytes).
pub const MAX_AUDIO_BYTES: usize =
    (REQUIRED_SAMPLE_RATE_HZ as usize * 30) * BYTES_PER_SAMPLE;

/// Hard IPC audio payload cap enforced BEFORE full allocation (1 MiB = 1,048,576 bytes).
pub const MAX_IPC_AUDIO_PAYLOAD_BYTES: usize = 1_048_576;

/// Supported language codes for speech recognition.
pub const SUPPORTED_LANGUAGES: &[&str] = &[
    "auto", "en", "it", "es", "fr", "de", "pt", "nl", "tr", "ru",
    "ar", "hi", "ja", "ko", "vi", "uk", "zh",
];

/// Characters that can never appear raw in a well-formed URL, rejected before parsing.
/// Rationale: the Windows launcher uses `ShellExecuteW` directly (no `cmd /C`
/// string interpretation), so cmd-specific expansions (`%VAR%`, `!var!`, `$`)
/// are inert, and `& ; % ! $` are all legal URL characters (multi-param query
/// strings, percent-encoding). Blocking them would break legitimate targets
/// such as `https://example.com/?a=1&b=2` or `browser.search` result URLs.
/// What remains rejected can never occur raw in a valid URL and signals
/// smuggling or shell-concatenation attempts.
pub const FORBIDDEN_SHELL_METACHARS: &[char] = &[
    '|', '<', '>', '\\', '`', '^', '"', '\'',
];

/// Validates whether a given URL string points strictly to a local loopback interface.
///
/// Security properties enforced:
/// - Rejects ASCII control characters, newlines, carriage returns, and null bytes.
/// - Rejects backslashes (`\`) to prevent URL normalization and authority-smuggling tricks.
/// - Requires valid URL structure parseable by the standard WHATWG URL parser.
/// - Requires scheme to be strictly `http` or `https`.
/// - Rejects embedded userinfo / credentials (`user:pass@host`).
/// - Requires the parsed host to be EXACTLY `127.0.0.1` or `localhost`.
///   Prefix and domain suffix tricks (e.g. `127.0.0.1.evil.com`, `localhost.evil.com`)
///   are rejected because `host_str()` evaluates to the full domain.
/// - Rejects IPv6 (`[::1]`) and decimal/octal IP tricks to enforce deterministic loopback binding.
pub fn is_loopback_url(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.chars().any(|c| c.is_control()) {
        return false;
    }
    // Reject backslashes to avoid URL parser normalization ambiguities
    if s.contains('\\') {
        return false;
    }

    let parsed = match Url::parse(s) {
        Ok(u) => u,
        Err(_) => return false,
    };

    // Scheme must strictly be http or https
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return false,
    }

    // Reject userinfo / credentials
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return false;
    }

    // Exact host match: strictly "127.0.0.1" or "localhost"
    match parsed.host_str() {
        Some(host) => host == "127.0.0.1" || host.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

/// Validates whether remote AI execution is permitted based on persisted user settings.
///
/// Security contract:
/// - Online/remote AI is permitted ONLY when `persisted_allow_online_ai` is `true`.
/// - A caller-provided flag (`caller_flag == true`) CAN NEVER enable remote AI if
///   the user's persisted setting is `false`.
/// - If a caller attempts to request remote AI when the user has disabled it,
///   this fails closed with a typed error.
pub fn check_remote_allowed(
    persisted_allow_online_ai: bool,
    caller_flag: bool,
) -> Result<(), &'static str> {
    if caller_flag && !persisted_allow_online_ai {
        return Err("remote-ai-disabled: online AI is disabled by user settings; caller flag cannot enable remote access");
    }
    Ok(())
}

/// Checks whether an endpoint URL is permitted for use by planners or services.
///
/// Loopback endpoints (`127.0.0.1`, `localhost`) are always allowed locally.
/// Non-loopback endpoints require `persisted_allow_online_ai` to be `true`.
pub fn check_endpoint_allowed(
    endpoint: &str,
    persisted_allow_online_ai: bool,
) -> Result<(), &'static str> {
    if is_loopback_url(endpoint) {
        Ok(())
    } else if persisted_allow_online_ai {
        Ok(())
    } else {
        Err("remote-endpoint-prohibited: endpoint is remote but online AI is disabled in user settings")
    }
}

/// Validates speech audio transcript parameters against strict resource and format bounds.
///
/// Checks enforced:
/// 1. IPC payload size cap enforced BEFORE full memory allocation (`MAX_IPC_AUDIO_PAYLOAD_BYTES`).
/// 2. Sample rate must match the fixed allowlist: exactly 16,000 Hz.
/// 3. Audio duration bounds: between 100 ms (3,200 bytes) and 30 s (960,000 bytes) for 16-bit mono PCM.
/// 4. Language code must belong to the enumerated `SUPPORTED_LANGUAGES` allowlist.
pub fn check_transcript_bounds(
    len_bytes: usize,
    sample_rate_hz: u32,
    lang: &str,
) -> Result<(), &'static str> {
    // 1. Hard IPC payload cap before further processing
    if len_bytes > MAX_IPC_AUDIO_PAYLOAD_BYTES {
        return Err("payload-too-large: audio payload exceeds 1 MiB IPC cap");
    }

    // 2. Fixed sample rate allowlist
    if sample_rate_hz != REQUIRED_SAMPLE_RATE_HZ {
        return Err("unsupported-sample-rate: speech engine requires exactly 16000 Hz audio");
    }

    // 3. Audio duration bounds (100 ms minimum, 30 s maximum)
    if len_bytes < MIN_AUDIO_BYTES {
        return Err("audio-too-short: speech segment must be at least 100ms (3200 bytes at 16kHz 16-bit)");
    }
    if len_bytes > MAX_AUDIO_BYTES {
        return Err("audio-too-long: speech segment exceeds 30-second limit (960000 bytes at 16kHz 16-bit)");
    }

    // 4. Language allowlist validation
    let normalized = lang.trim();
    if normalized.is_empty() {
        return Err("invalid-language: language code cannot be empty");
    }
    let is_supported = SUPPORTED_LANGUAGES
        .iter()
        .any(|supported| supported.eq_ignore_ascii_case(normalized));
    if !is_supported {
        return Err("unsupported-language: language code is not in the supported speech allowlist");
    }

    Ok(())
}

/// Validates tool execution arguments by delegating to the central policy registry.
///
/// Fails closed if the tool is unknown or arguments do not conform to schema.
pub fn check_tool_args(
    tool: &str,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    crate::policy::validate_args(tool, args)
}

/// Sanitizes an external URL target before handing it to an OS browser launcher.
///
/// Security properties:
/// - Rejects empty or whitespace-only targets.
/// - Rejects control characters and whitespace (never raw-valid in URLs).
/// - Rejects characters that can never appear raw in a valid URL
///   (`|<>\`^"'`), blocking smuggling and shell-concatenation attempts.
/// - Enforces valid URL parsing via WHATWG URL parser.
/// - Strictly permits only `http` and `https` schemes (blocks `file:`, `javascript:`,
///   `data:`, `vbscript:`, `cmd:`, `powershell:`, `ms-settings:`, etc.).
/// - Rejects embedded userinfo credentials.
/// - Requires a valid host component.
///
/// Note for OS integration:
/// On Windows, invocation MUST avoid `cmd /C start` string interpretation, which is
/// vulnerable to parameter injection even after escaping. Use `ShellExecuteW` directly
/// via `windows-sys::Win32::UI::Shell::ShellExecuteW` or direct browser process launch.
pub fn sanitize_open_external(target: &str) -> Result<String, String> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return Err("target URL cannot be empty".into());
    }

    // Reject control characters and whitespace (spaces can never appear raw
    // in a valid URL; they must be percent-encoded as %20).
    if trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("target URL contains prohibited whitespace or control characters".into());
    }

    // Reject characters that can never appear raw in a valid URL.
    if let Some(bad_char) = trimmed.chars().find(|c| FORBIDDEN_SHELL_METACHARS.contains(c)) {
        return Err(format!(
            "target URL contains prohibited shell metacharacter: '{bad_char}'"
        ));
    }

    // Parse URL strictly
    let parsed = Url::parse(trimmed)
        .map_err(|e| format!("invalid URL format: {e}"))?;

    // Strictly enforce http and https schemes
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "prohibited URL scheme '{other}'; only http and https are permitted"
            ));
        }
    }

    // Reject embedded user credentials
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("target URL must not contain embedded credentials".into());
    }

    // Require valid host
    if parsed.host_str().is_none() {
        return Err("target URL must have a valid host".into());
    }

    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_loopback_valid() {
        assert!(is_loopback_url("http://127.0.0.1:8787"));
        assert!(is_loopback_url("http://127.0.0.1:1420"));
        assert!(is_loopback_url("http://127.0.0.1:8080/v1/health"));
        assert!(is_loopback_url("http://localhost:3000"));
        assert!(is_loopback_url("http://localhost"));
        assert!(is_loopback_url("https://localhost:8443/test?a=1"));
        assert!(is_loopback_url("http://LOCALHOST:8080"));
    }

    #[test]
    fn test_loopback_rejected_tricks() {
        // Host prefix tricks
        assert!(!is_loopback_url("http://127.0.0.1.attacker.com"));
        assert!(!is_loopback_url("http://localhost.attacker.com"));
        assert!(!is_loopback_url("http://127.0.0.1.evil.com:8080"));

        // Userinfo tricks
        assert!(!is_loopback_url("http://127.0.0.1@attacker.com"));
        assert!(!is_loopback_url("http://user:pass@127.0.0.1:8000"));
        assert!(!is_loopback_url("http://localhost:secret@attacker.com"));

        // Normalization and path tricks
        assert!(!is_loopback_url("http://127.0.0.1\\attacker.com"));
        assert!(!is_loopback_url("http://127.0.0.1/../../attacker.com"));

        // Non-http schemes
        assert!(!is_loopback_url("file:///etc/passwd"));
        assert!(!is_loopback_url("javascript:alert(1)"));
        assert!(!is_loopback_url("data:text/html,evil"));
        assert!(!is_loopback_url("ws://127.0.0.1:8080"));

        // IPv6 (only exact 127.0.0.1/localhost allowed by spec)
        assert!(!is_loopback_url("http://[::1]:8080"));

        // Remote IPs
        assert!(!is_loopback_url("http://192.168.1.1:8080"));
        assert!(!is_loopback_url("http://10.0.0.1:8080"));
        assert!(!is_loopback_url("http://8.8.8.8:8080"));

        // Control characters / newlines
        assert!(!is_loopback_url("http://127.0.0.1:8080\r\nHost: evil.com"));
        assert!(!is_loopback_url("http://127.0.0.1\0.evil.com"));
    }

    #[test]
    fn test_remote_allowed_policy() {
        // Persisted false -> caller flag cannot enable
        assert!(check_remote_allowed(false, false).is_ok());
        assert!(check_remote_allowed(false, true).is_err());

        // Persisted true -> allowed
        assert!(check_remote_allowed(true, false).is_ok());
        assert!(check_remote_allowed(true, true).is_ok());

        // Check endpoint helper
        assert!(check_endpoint_allowed("http://127.0.0.1:8787", false).is_ok());
        assert!(check_endpoint_allowed("https://api.openai.com/v1", false).is_err());
        assert!(check_endpoint_allowed("https://api.openai.com/v1", true).is_ok());
    }

    #[test]
    fn test_transcript_bounds() {
        // Valid 16kHz audio: 1600 samples (3200 bytes = 100ms)
        assert!(check_transcript_bounds(3_200, 16_000, "en").is_ok());
        // Valid 16kHz audio: 480000 samples (960000 bytes = 30s)
        assert!(check_transcript_bounds(960_000, 16_000, "auto").is_ok());
        assert!(check_transcript_bounds(160_000, 16_000, "it").is_ok());

        // Too short (< 100 ms)
        assert!(check_transcript_bounds(3_198, 16_000, "en").is_err());

        // Too long (> 30 s)
        assert!(check_transcript_bounds(960_002, 16_000, "en").is_err());

        // Payload cap exceeded (> 1 MiB)
        assert!(check_transcript_bounds(1_048_577, 16_000, "en").is_err());

        // Unsupported sample rate
        assert!(check_transcript_bounds(3_200, 44_100, "en").is_err());
        assert!(check_transcript_bounds(3_200, 8_000, "en").is_err());

        // Unsupported or invalid language
        assert!(check_transcript_bounds(3_200, 16_000, "").is_err());
        assert!(check_transcript_bounds(3_200, 16_000, "klingon").is_err());
        assert!(check_transcript_bounds(3_200, 16_000, "xx").is_err());
    }

    #[test]
    fn test_sanitize_open_external_valid() {
        assert_eq!(
            sanitize_open_external("https://reflexdesk.io").unwrap(),
            "https://reflexdesk.io"
        );
        assert_eq!(
            sanitize_open_external("http://localhost:8080/docs").unwrap(),
            "http://localhost:8080/docs"
        );
        assert_eq!(
            sanitize_open_external("https://github.com/reflexdesk/burbot").unwrap(),
            "https://github.com/reflexdesk/burbot"
        );
        // Legal URL characters the launcher must accept: multi-param queries,
        // percent-encoding (browser.search output), fragments.
        assert!(sanitize_open_external("https://example.com/?a=1&b=2").is_ok());
        assert!(sanitize_open_external("https://www.google.com/search?q=hello%20world").is_ok());
        assert!(sanitize_open_external("https://example.com/a;b").is_ok());
    }

    #[test]
    fn test_sanitize_open_external_rejected() {
        // Characters that can never appear raw in a valid URL
        assert!(sanitize_open_external("https://example.com/|calc.exe").is_err());
        assert!(sanitize_open_external("https://example.com/`id`").is_err());
        assert!(sanitize_open_external("https://example.com/^dir").is_err());
        assert!(sanitize_open_external("https://example.com/\"evil\"").is_err());
        assert!(sanitize_open_external("https://example.com/a<b").is_err());
        assert!(sanitize_open_external("https://example.com/a\\b").is_err());

        // Non-http schemes
        assert!(sanitize_open_external("file:///C:/Windows/System32/cmd.exe").is_err());
        assert!(sanitize_open_external("javascript:alert(1)").is_err());
        assert!(sanitize_open_external("powershell:Start-Process calc").is_err());
        assert!(sanitize_open_external("data:text/html,<script>alert(1)</script>").is_err());

        // Control characters
        assert!(sanitize_open_external("https://example.com/\0evil").is_err());
        assert!(sanitize_open_external("https://example.com/\r\nevil").is_err());

        // Empty
        assert!(sanitize_open_external("").is_err());
        assert!(sanitize_open_external("   ").is_err());
    }

    #[test]
    fn test_check_tool_args_delegates_to_policy() {
        // app.open with valid args
        let valid_app = json!({ "app": "Finder" });
        assert!(check_tool_args("app.open", &valid_app).is_ok());

        // app.open with invalid args
        let invalid_app = json!({ "wrong": 123 });
        assert!(check_tool_args("app.open", &invalid_app).is_err());

        // Unknown tool fails closed
        assert!(check_tool_args("unknown.tool", &json!({})).is_err());
    }
}
