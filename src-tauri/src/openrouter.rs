//! OpenRouter audio provider adapter — opt-in cloud speech-to-text and
//! text-to-speech.
//!
//! Security contract:
//! - Every request is gated by [`check_endpoint_allowed`], so nothing in this
//!   module can reach the network while the user has online AI disabled.
//! - The API key is read from the OS credential vault per call, used only to
//!   build an `Authorization` header, and never returned to the renderer.
//! - Request bodies, audio bytes, and credentials are never logged; error
//!   strings carry only a bounded, redacted provider detail.
//! - Both directions are bounded: PCM in is capped by the caller's 30 s speech
//!   limit and MP3 out is capped by [`MAX_TTS_AUDIO_BYTES`].
//!
//! Endpoints (fixed; the user cannot retarget speech at another host):
//! - `POST {API_BASE}/audio/transcriptions` — JSON `{model, input_audio:{data,format}}`
//! - `POST {API_BASE}/audio/speech` — JSON `{model, input, voice, response_format}`

use std::{io::Read, time::Duration};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::{
    redaction::redact_error,
    secrets::SecretBytes,
    security::check_endpoint_allowed,
    settings::AppSettings,
};

/// Fixed OpenRouter API base URL.
pub const API_BASE: &str = "https://openrouter.ai/api/v1";

/// Default transcription model (OpenRouter's documented Whisper endpoint).
pub const DEFAULT_STT_MODEL: &str = "openai/whisper-large-v3";
/// Default speech synthesis model.
pub const DEFAULT_TTS_MODEL: &str = "x-ai/grok-voice-tts-1.0";
/// Default speech synthesis voice.
pub const DEFAULT_TTS_VOICE: &str = "eve";

/// Hard cap on spoken-reply text sent to a remote voice.
pub const MAX_TTS_INPUT_CHARS: usize = 600;
/// Hard cap on the returned audio payload (8 MiB).
pub const MAX_TTS_AUDIO_BYTES: usize = 8 * 1024 * 1024;

const MAX_MODEL_ID_CHARS: usize = 128;
const MAX_VOICE_ID_CHARS: usize = 64;
const MAX_ERROR_DETAIL_CHARS: usize = 300;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

const MODEL_ID_PUNCTUATION: &str = "-_.:/@+";
const VOICE_ID_PUNCTUATION: &str = "-_.";

/// Bounded audio payload returned to the renderer for playback.
///
/// Carries no credential material; the renderer receives only the synthesized
/// bytes, never the API key.
#[derive(Debug, Clone, Serialize)]
pub struct SpeechAudio {
    pub content_type: String,
    pub byte_length: usize,
    pub audio_base64: String,
}

// ---------------------------------------------------------------------------
// Identifier validation
// ---------------------------------------------------------------------------

fn validate_token(
    kind: &str,
    value: &str,
    max_chars: usize,
    punctuation: &str,
) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!(
            "openrouter-{kind}-invalid: no {kind} is configured"
        ));
    }
    if trimmed.chars().count() > max_chars {
        return Err(format!(
            "openrouter-{kind}-invalid: {kind} exceeds {max_chars} characters"
        ));
    }
    let ok = trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || punctuation.contains(c));
    if !ok {
        return Err(format!(
            "openrouter-{kind}-invalid: {kind} may only contain letters, digits and '{punctuation}'"
        ));
    }
    Ok(trimmed.to_string())
}

/// Validates an OpenRouter model identifier (e.g. `openai/whisper-large-v3`).
pub fn validate_model_id(model: &str) -> Result<String, String> {
    validate_token("model", model, MAX_MODEL_ID_CHARS, MODEL_ID_PUNCTUATION)
}

/// Validates an OpenRouter voice identifier (e.g. `alloy`).
pub fn validate_voice_id(voice: &str) -> Result<String, String> {
    validate_token("voice", voice, MAX_VOICE_ID_CHARS, VOICE_ID_PUNCTUATION)
}

/// Returns the language hint to send, or `None` to let the model auto-detect.
///
/// "auto" means the user selected automatic detection (also the case when a
/// secondary spoken language is configured) and must never be sent as a hint.
/// Anything outside the speech-language allowlist is dropped rather than
/// forwarded, so a renderer string can never reach the request body verbatim.
pub fn language_hint(language: &str) -> Option<&str> {
    let trimmed = language.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
        return None;
    }
    crate::security::SUPPORTED_LANGUAGES
        .iter()
        .find(|supported| supported.eq_ignore_ascii_case(trimmed))
        .copied()
}

// ---------------------------------------------------------------------------
// Opt-in gate + credential access
// ---------------------------------------------------------------------------

/// Fails closed unless the user has explicitly enabled online AI.
pub fn ensure_online_allowed(settings: &AppSettings) -> Result<(), String> {
    if settings.allow_online_ai {
        Ok(())
    } else {
        Err(
            "online-ai-disabled: turn on 'Allow online AI and voice' in Settings to use OpenRouter"
                .into(),
        )
    }
}

/// Verifies that OpenRouter speech can run at all for the current settings.
///
/// Used to surface an actionable error *before* capture starts, so a missing
/// key is never mistaken for a failed transcription.
pub fn check_speech_ready(settings: &AppSettings) -> Result<(), String> {
    ensure_online_allowed(settings)?;
    if settings.openrouter_secret_ref.is_none() {
        return Err(
            "openrouter-key-missing: connect an OpenRouter API key in Settings to use cloud speech"
                .into(),
        );
    }
    Ok(())
}

/// Resolves the vault-held OpenRouter credential for one request.
pub fn api_key(app: &AppHandle, settings: &AppSettings) -> Result<SecretBytes, String> {
    check_speech_ready(settings)?;
    let secret_ref = settings
        .openrouter_secret_ref
        .as_ref()
        .ok_or_else(|| "openrouter-key-missing: no OpenRouter credential is connected".to_string())?;
    app.state::<crate::secrets::AppSecretStore>().0.get(secret_ref)
}

// ---------------------------------------------------------------------------
// HTTP plumbing
// ---------------------------------------------------------------------------

fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| {
            format!(
                "openrouter-client-failed: {}",
                redact_error(&e.to_string())
            )
        })
}

/// Builds the bearer header for an OpenRouter request.
///
/// Delegates to the shared credential helper so vault keys are validated
/// identically everywhere and no request builder can panic on user input.
fn authorization_header(api_key: &SecretBytes) -> Result<reqwest::header::HeaderValue, String> {
    crate::secrets::bearer_header(api_key)
        .map_err(|e| format!("openrouter-key-invalid: {e}"))
}

fn bounded_detail(response: reqwest::blocking::Response) -> String {
    let mut bytes = Vec::new();
    let _ = response
        .take((MAX_ERROR_DETAIL_CHARS * 4) as u64)
        .read_to_end(&mut bytes);
    let text = String::from_utf8_lossy(&bytes);
    let collapsed: String = text
        .chars()
        .take(MAX_ERROR_DETAIL_CHARS)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    redact_error(collapsed.trim())
}

fn http_error_message(
    operation: &str,
    status: reqwest::StatusCode,
    response: reqwest::blocking::Response,
) -> String {
    let hint = match status.as_u16() {
        401 | 403 => "OpenRouter rejected the API key; rotate it in Settings.",
        402 => "OpenRouter reported insufficient credits for this request.",
        429 => "OpenRouter rate-limited the request; try again shortly.",
        500..=599 => "OpenRouter is temporarily unavailable; try again shortly.",
        _ => "OpenRouter rejected the request.",
    };
    let detail = bounded_detail(response);
    if detail.is_empty() {
        format!("openrouter-http-{status}: {operation} failed. {hint}")
    } else {
        format!("openrouter-http-{status}: {operation} failed. {hint} ({detail})")
    }
}

// ---------------------------------------------------------------------------
// Speech-to-text
// ---------------------------------------------------------------------------

/// Transcribes a PCM WAV payload through OpenRouter.
///
/// `api_base` is a parameter (rather than the [`API_BASE`] constant) so
/// behavioral tests can point the transport at a local stub.
pub fn transcribe_wav_at(
    api_base: &str,
    api_key: &SecretBytes,
    model: &str,
    allow_online: bool,
    wav: &[u8],
    language: &str,
) -> Result<String, String> {
    let endpoint = format!("{}/audio/transcriptions", api_base.trim_end_matches('/'));
    check_endpoint_allowed(&endpoint, allow_online)
        .map_err(|e| format!("openrouter-blocked: {e}"))?;

    let model = validate_model_id(model)?;
    if wav.len() <= 44 {
        return Err("openrouter-audio-invalid: no speech samples to transcribe".into());
    }
    if wav.len() > crate::security::MAX_AUDIO_BYTES + 44 {
        return Err(
            "openrouter-audio-invalid: speech segment exceeds the 30 second command limit".into(),
        );
    }

    let header = authorization_header(api_key)?;

    let mut body = serde_json::Map::new();
    body.insert("model".into(), serde_json::Value::String(model));
    body.insert(
        "input_audio".into(),
        serde_json::json!({ "data": STANDARD.encode(wav), "format": "wav" }),
    );
    if let Some(hint) = language_hint(language) {
        body.insert("language".into(), serde_json::Value::String(hint.into()));
    }

    let response = http_client()?
        .post(&endpoint)
        .header(reqwest::header::AUTHORIZATION, header)
        .json(&serde_json::Value::Object(body))
        .send()
        .map_err(|e| {
            format!(
                "openrouter-unreachable: speech-to-text request failed: {}",
                redact_error(&e.to_string())
            )
        })?;

    let status = response.status();
    if !status.is_success() {
        return Err(http_error_message("speech-to-text", status, response));
    }

    let mut bytes = Vec::new();
    response
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("openrouter-invalid-response: {}", redact_error(&e.to_string())))?;
    if bytes.len() > 64 * 1024 {
        return Err("openrouter-response-too-large: transcription response exceeded 64 KiB".into());
    }
    let payload: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("openrouter-invalid-response: {}", redact_error(&e.to_string())))?;

    let text = payload
        .get("text")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if text.is_empty() {
        return Err("openrouter-empty-transcript: the speech model returned no text".into());
    }
    Ok(text)
}

// ---------------------------------------------------------------------------
// Text-to-speech
// ---------------------------------------------------------------------------

/// Synthesizes speech through OpenRouter and returns bounded MP3 bytes.
pub fn synthesize_at(
    api_base: &str,
    api_key: &SecretBytes,
    model: &str,
    voice: &str,
    text: &str,
    allow_online: bool,
) -> Result<SpeechAudio, String> {
    let endpoint = format!("{}/audio/speech", api_base.trim_end_matches('/'));
    check_endpoint_allowed(&endpoint, allow_online)
        .map_err(|e| format!("openrouter-blocked: {e}"))?;

    let model = validate_model_id(model)?;
    let voice = validate_voice_id(voice)?;

    let input = text.trim();
    if input.is_empty() {
        return Err("openrouter-speech-invalid: there is nothing to speak".into());
    }
    if input.chars().count() > MAX_TTS_INPUT_CHARS {
        return Err(format!(
            "openrouter-speech-invalid: spoken reply exceeds {MAX_TTS_INPUT_CHARS} characters"
        ));
    }

    let header = authorization_header(api_key)?;

    let response = http_client()?
        .post(&endpoint)
        .header(reqwest::header::AUTHORIZATION, header)
        .json(&serde_json::json!({
            "model": model,
            "input": input,
            "voice": voice,
            "response_format": "mp3",
        }))
        .send()
        .map_err(|e| {
            format!(
                "openrouter-unreachable: text-to-speech request failed: {}",
                redact_error(&e.to_string())
            )
        })?;

    let status = response.status();
    if !status.is_success() {
        return Err(http_error_message("text-to-speech", status, response));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    if !content_type.starts_with("audio/mpeg") {
        let detail = bounded_detail(response);
        let described = if content_type.is_empty() {
            "no content type".to_string()
        } else {
            content_type
        };
        return Err(if detail.is_empty() {
            format!("openrouter-invalid-response: text-to-speech returned {described}, not MP3")
        } else {
            format!(
                "openrouter-invalid-response: text-to-speech returned {described}, not MP3 ({detail})"
            )
        });
    }

    let mut bytes = Vec::new();
    response
        .take(MAX_TTS_AUDIO_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| {
            format!(
                "openrouter-unreachable: could not read the synthesized audio: {}",
                redact_error(&e.to_string())
            )
        })?;

    if bytes.len() > MAX_TTS_AUDIO_BYTES {
        return Err(
            "openrouter-response-too-large: the synthesized audio exceeded the 8 MiB playback limit"
                .into(),
        );
    }
    if bytes.is_empty() {
        return Err("openrouter-invalid-response: the synthesized audio was empty".into());
    }

    Ok(SpeechAudio {
        content_type,
        byte_length: bytes.len(),
        audio_base64: STANDARD.encode(&bytes),
    })
}

// ---------------------------------------------------------------------------
// Unit tests — behavioral proof against a local stub server
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        net::TcpListener,
        sync::mpsc::{self, Receiver},
        thread,
    };

    /// Minimal single-request HTTP/1.1 stub. Returns the configured response
    /// and reports the raw request (headers + body) back to the test.
    fn stub(status_line: &str, content_type: &str, body: Vec<u8>) -> (String, Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
        let addr = listener.local_addr().expect("stub addr");
        let (tx, rx) = mpsc::channel();
        let status_line = status_line.to_string();
        let content_type = content_type.to_string();

        thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = Vec::new();
            let mut chunk = [0u8; 2048];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        request.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&request);
                        if let Some(head_end) = text.find("\r\n\r\n") {
                            let content_length = text
                                .lines()
                                .find_map(|line| {
                                    let (name, value) = line.split_once(':')?;
                                    name.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().ok())?
                                })
                                .unwrap_or(0);
                            if request.len() >= head_end + 4 + content_length {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }

            let _ = tx.send(String::from_utf8_lossy(&request).to_string());
            let head = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        });

        (format!("http://{addr}/api/v1"), rx)
    }

    fn key(value: &str) -> SecretBytes {
        SecretBytes::from_slice(value.as_bytes())
    }

    fn wav(samples: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; 44];
        bytes.extend(std::iter::repeat(1u8).take(samples * 2));
        bytes
    }

    #[test]
    fn language_hint_omits_automatic_detection() {
        assert_eq!(language_hint("auto"), None);
        assert_eq!(language_hint("  "), None);
        assert_eq!(language_hint("AUTO"), None);
        assert_eq!(language_hint("it"), Some("it"));
        // Not in the speech allowlist: dropped instead of forwarded verbatim.
        assert_eq!(language_hint("klingon\r\nX: 1"), None);
    }

    #[test]
    fn remote_transport_is_blocked_while_online_ai_is_disabled() {
        let error = transcribe_wav_at(API_BASE, &key("sk-test"), DEFAULT_STT_MODEL, false, &wav(1600), "en")
            .expect_err("offline transport must fail");
        assert!(error.starts_with("openrouter-blocked"), "{error}");

        let error = synthesize_at(API_BASE, &key("sk-test"), DEFAULT_TTS_MODEL, "alloy", "Done.", false)
            .expect_err("offline transport must fail");
        assert!(error.starts_with("openrouter-blocked"), "{error}");
    }

    #[test]
    fn transcribe_sends_documented_json_and_decodes_text() {
        let (base, request) = stub(
            "200 OK",
            "application/json",
            br#"{"text":"open  chrome  "}"#.to_vec(),
        );

        let text =
            transcribe_wav_at(&base, &key("sk-or-test"), "openai/whisper-large-v3", true, &wav(1600), "it")
                .expect("transcribe");

        assert_eq!(text, "open  chrome");
        let sent = request.recv().expect("stub captured request");
        assert!(sent.starts_with("POST /api/v1/audio/transcriptions "), "{sent}");
        assert!(sent.to_ascii_lowercase().contains("authorization: bearer sk-or-test"));
        assert!(sent.contains("\"input_audio\""));
        assert!(sent.contains("\"format\":\"wav\""));
        assert!(sent.contains("\"model\":\"openai/whisper-large-v3\""));
        assert!(sent.contains("\"language\":\"it\""));
    }

    #[test]
    fn transcribe_omits_language_hint_for_auto_detection() {
        let (base, request) = stub("200 OK", "application/json", br#"{"text":"hello"}"#.to_vec());

        transcribe_wav_at(&base, &key("sk-or-test"), DEFAULT_STT_MODEL, true, &wav(1600), "auto")
            .expect("transcribe");

        let sent = request.recv().expect("stub captured request");
        assert!(!sent.contains("language"), "{sent}");
    }

    #[test]
    fn provider_http_errors_are_sanitized_and_actionable() {
        let (base, _request) = stub(
            "401 Unauthorized",
            "application/json",
            br#"{"error":{"message":"Invalid API key"}}"#.to_vec(),
        );

        let error = transcribe_wav_at(&base, &key("sk-or-secret-value"), DEFAULT_STT_MODEL, true, &wav(1600), "en")
            .expect_err("401 must fail");

        assert!(error.contains("401"), "{error}");
        assert!(error.contains("rotate"), "{error}");
        assert!(!error.contains("sk-or-secret-value"), "key leaked: {error}");
    }

    #[test]
    fn malformed_key_fails_closed_without_panicking() {
        let error = transcribe_wav_at(
            "http://127.0.0.1:9/api/v1",
            &key("sk-or-test\r\nX-Injected: 1"),
            DEFAULT_STT_MODEL,
            true,
            &wav(1600),
            "en",
        )
        .expect_err("header injection must fail");
        assert!(error.starts_with("openrouter-key-invalid"), "{error}");
    }

    #[test]
    fn invalid_identifiers_are_rejected_before_any_request() {
        assert!(validate_model_id("openai/whisper-large-v3").is_ok());
        assert!(validate_model_id("").is_err());
        assert!(validate_model_id("evil model").is_err());
        assert!(validate_voice_id("alloy").is_ok());
        assert!(validate_voice_id("alloy\nX: 1").is_err());
    }

    #[test]
    fn speech_returns_bounded_audio_and_rejects_non_audio() {
        let mp3 = vec![0x49u8, 0x44, 0x33, 0x04];
        let (base, request) = stub("200 OK", "audio/mpeg", mp3.clone());

        let audio = synthesize_at(&base, &key("sk-or-test"), DEFAULT_TTS_MODEL, "alloy", "Done.", true)
            .expect("synthesize");

        assert_eq!(audio.content_type, "audio/mpeg");
        assert_eq!(audio.byte_length, 4);
        assert_eq!(STANDARD.decode(audio.audio_base64).expect("base64"), mp3);

        let sent = request.recv().expect("stub captured request");
        assert!(sent.starts_with("POST /api/v1/audio/speech "), "{sent}");
        assert!(sent.contains("\"response_format\":\"mp3\""));
        assert!(sent.contains("\"voice\":\"alloy\""));

        let (base, _request) = stub(
            "200 OK",
            "application/json",
            br#"{"error":"bad voice"}"#.to_vec(),
        );
        let error = synthesize_at(&base, &key("sk-or-test"), DEFAULT_TTS_MODEL, "alloy", "Done.", true)
            .expect_err("non-audio payload must fail");
        assert!(error.starts_with("openrouter-invalid-response"), "{error}");
    }

    #[test]
    fn oversize_speech_response_is_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
        let addr = listener.local_addr().expect("stub addr");
        thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut scratch = [0u8; 2048];
            let _ = stream.read(&mut scratch);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nConnection: close\r\n\r\n",
            );
            let chunk = vec![0u8; 256 * 1024];
            // One byte past the cap is enough to prove the read is bounded.
            let mut remaining = MAX_TTS_AUDIO_BYTES + 1;
            while remaining > 0 {
                let size = remaining.min(chunk.len());
                if stream.write_all(&chunk[..size]).is_err() {
                    break;
                }
                remaining -= size;
            }
        });

        let error = synthesize_at(
            &format!("http://{addr}/api/v1"),
            &key("sk-or-test"),
            DEFAULT_TTS_MODEL,
            "alloy",
            "Done.",
            true,
        )
        .expect_err("oversize audio must fail");
        assert!(error.starts_with("openrouter-response-too-large"), "{error}");
    }

    #[test]
    fn spoken_reply_length_is_bounded() {
        let long = "a".repeat(MAX_TTS_INPUT_CHARS + 1);
        let error = synthesize_at(
            "http://127.0.0.1:9/api/v1",
            &key("sk-or-test"),
            DEFAULT_TTS_MODEL,
            "alloy",
            &long,
            true,
        )
        .expect_err("oversize reply must fail");
        assert!(error.starts_with("openrouter-speech-invalid"), "{error}");
    }
}