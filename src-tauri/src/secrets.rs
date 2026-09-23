//! Secret storage and credential management for ReflexDesk.
//!
//! Provides:
//! - [`SecretStore`] trait with `set`, `get`, `delete`, and `list_metadata` methods.
//! - [`SecretRef`] opaque references stored in normal application settings.
//! - [`SecretMetadata`] containing timestamps and provider IDs without credentials.
//! - [`SecretBytes`] memory-zeroizing sensitive buffer that overwrites itself on Drop.
//! - [`OsVaultSecretStore`] backed by the platform credential service; filesystem metadata contains no secret bytes.
//! - Plaintext secret detection and quarantine guard for settings files.
//! - Provider connection status, key rotation, and sanitized `auth-invalid` error mapping.
//! - Harness process environment scrubbing to prevent leaking secrets to children.

use keyring::Entry;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zeroize::Zeroizing;

/// Opaque reference to a secret stored in a [`SecretStore`].
///
/// Normal configuration files (e.g. `settings.json`) and frontend state
/// store ONLY this reference — NEVER raw API keys, tokens, or passwords.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretRef {
    /// Provider name, e.g. "planner" or "openai".
    pub provider: String,
    /// Opaque secret identifier, e.g. "sec_a1b2c3d4".
    pub id: String,
}

/// Metadata about a stored secret.
///
/// Contains strictly safe metadata (timestamps, provider name, opaque ID).
/// Safe to include in diagnostics or user interfaces without redaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretMetadata {
    pub provider: String,
    pub id: String,
    pub created_at_ts: u64,
    pub updated_at_ts: u64,
    pub last_used_ts: Option<u64>,
}

/// Status of an external provider connection returned to the frontend.
///
/// Contains NO secret material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConnectionStatus {
    pub provider: String,
    pub connected: bool,
    pub secret_id: Option<String>,
    pub updated_at_ts: Option<u64>,
    /// One of: "connected", "auth-invalid", "unreachable", "disconnected".
    pub last_status: String,
    pub message: String,
}

/// Memory-zeroizing buffer for sensitive secret material.
pub struct SecretBytes(Zeroizing<Vec<u8>>);

impl SecretBytes {
    pub fn new(data: Vec<u8>) -> Self {
        Self(Zeroizing::new(data))
    }

    pub fn from_slice(slice: &[u8]) -> Self {
        Self(Zeroizing::new(slice.to_vec()))
    }

    pub fn expose_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }

    pub fn expose_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(self.0.as_slice())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretBytes([REDACTED; {} bytes])", self.0.len())
    }
}

/// Builds a bearer `Authorization` header from a stored credential.
///
/// Credentials are user-supplied, so they are validated as single-line ASCII
/// before reaching `HeaderValue`. This keeps request builders on the
/// non-panicking path (a credential containing CR/LF must fail closed, not
/// abort the process) and blocks header injection.
pub fn bearer_header(api_key: &SecretBytes) -> Result<reqwest::header::HeaderValue, String> {
    let raw = api_key
        .expose_str()
        .map_err(|_| "credential-invalid: stored key is not valid UTF-8".to_string())?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("credential-invalid: stored key is empty".into());
    }
    if !trimmed.chars().all(|c| c.is_ascii_graphic()) {
        return Err(
            "credential-invalid: stored key contains characters that are invalid in an HTTP credential"
                .into(),
        );
    }

    let mut value = Zeroizing::new(String::from("Bearer "));
    value.push_str(trimmed);
    reqwest::header::HeaderValue::from_str(&value)
        .map_err(|_| "credential-invalid: stored key is not a valid HTTP credential".to_string())
}

impl std::fmt::Display for SecretBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[REDACTED_SECRET]")
    }
}

/// Abstract interface for secure credential storage.
pub trait SecretStore: Send + Sync {
    /// Stores or rotates a secret for the given provider.
    ///
    /// If a secret already exists for the provider, it is securely replaced
    /// and metadata is updated.
    fn set(&self, provider: &str, secret: &[u8]) -> Result<SecretRef, String>;

    /// Retrieves the secret material into a zeroizing buffer.
    fn get(&self, secret_ref: &SecretRef) -> Result<SecretBytes, String>;

    /// Deletes the secret for the given reference.
    fn delete(&self, secret_ref: &SecretRef) -> Result<(), String>;

    /// Lists metadata for all stored secrets without exposing sensitive contents.
    fn list_metadata(&self) -> Result<Vec<SecretMetadata>, String>;
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn generate_secret_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 8];
    rand::rng().fill_bytes(&mut bytes);
    format!("sec_{:016x}", u64::from_le_bytes(bytes))
}

const KEYRING_SERVICE_PREFIX: &str = "com.reflexdesk.credentials";

/// OS credential-store backed secret vault.
///
/// Raw values are stored only by Windows Credential Manager, macOS Keychain,
/// or Linux Secret Service. The app directory contains allowlisted metadata
/// only. Backend unavailability fails closed; there is no plaintext fallback.
pub struct OsVaultSecretStore {
    vault_dir: PathBuf,
    metadata_lock: Mutex<()>,
}

impl OsVaultSecretStore {
    pub fn new<P: AsRef<Path>>(vault_dir: P) -> Result<Self, String> {
        let dir = vault_dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)
            .map_err(|e| format!("failed to create vault metadata dir: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
                .map_err(|e| format!("failed to secure vault metadata dir: {e}"))?;
        }

        let insecure_dir = dir.join("secrets");
        if insecure_dir.exists() {
            let has_payloads = fs::read_dir(&insecure_dir)
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .any(|entry| entry.path().is_file())
                })
                .unwrap_or(true);
            if has_payloads {
                // Never import raw bytes automatically. Quarantine the legacy
                // directory so the app can boot and ask the user to reconnect.
                let quarantine = dir.join(format!("insecure-secrets-quarantine-{}", now_ts()));
                fs::rename(&insecure_dir, quarantine)
                    .map_err(|_| "insecure-secret-quarantine-failed".to_string())?;
            } else {
                let _ = fs::remove_dir(&insecure_dir);
            }
        }

        Ok(Self {
            vault_dir: dir,
            metadata_lock: Mutex::new(()),
        })
    }

    fn metadata_path(&self) -> PathBuf {
        self.vault_dir.join("metadata.json")
    }

    fn service_name(provider: &str) -> String {
        let normalized: String = provider
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            .collect();
        format!(
            "{KEYRING_SERVICE_PREFIX}.{}",
            normalized.to_ascii_lowercase()
        )
    }

    fn entry(provider: &str, id: &str) -> Result<Entry, String> {
        Entry::new(&Self::service_name(provider), id)
            .map_err(|e| format!("credential-store-unavailable: {e}"))
    }

    fn read_metadata_map(&self) -> HashMap<String, SecretMetadata> {
        let path = self.metadata_path();
        if !path.exists() {
            return HashMap::new();
        }
        let Ok(data) = fs::read_to_string(path) else {
            return HashMap::new();
        };
        serde_json::from_str(&data).unwrap_or_default()
    }

    fn write_metadata_map(&self, map: &HashMap<String, SecretMetadata>) -> Result<(), String> {
        let path = self.metadata_path();
        let temp = path.with_extension("json.tmp");
        let payload = serde_json::to_vec_pretty(map)
            .map_err(|e| format!("failed to serialize credential metadata: {e}"))?;
        fs::write(&temp, payload)
            .map_err(|e| format!("failed to write credential metadata: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("failed to secure credential metadata: {e}"))?;
        }
        #[cfg(target_os = "windows")]
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|e| format!("failed to replace credential metadata: {e}"))?;
        }
        fs::rename(&temp, &path).map_err(|e| format!("failed to commit credential metadata: {e}"))
    }
}

impl SecretStore for OsVaultSecretStore {
    fn set(&self, provider: &str, secret: &[u8]) -> Result<SecretRef, String> {
        if provider.trim().is_empty() {
            return Err("provider name cannot be empty".into());
        }
        if secret.is_empty() {
            return Err("secret cannot be empty".into());
        }
        let _guard = self
            .metadata_lock
            .lock()
            .map_err(|_| "credential metadata lock poisoned".to_string())?;
        let mut map = self.read_metadata_map();
        let existing = map.values().find(|m| m.provider == provider).cloned();
        let id = generate_secret_id();
        let entry = Self::entry(provider, &id)?;
        entry
            .set_secret(secret)
            .map_err(|e| format!("credential-store-write-failed: {e}"))?;

        let now = now_ts();
        let metadata = SecretMetadata {
            provider: provider.to_string(),
            id: id.clone(),
            created_at_ts: existing.as_ref().map(|m| m.created_at_ts).unwrap_or(now),
            updated_at_ts: now,
            last_used_ts: None,
        };
        if let Some(old) = existing.as_ref() {
            map.remove(&old.id);
        }
        map.insert(id.clone(), metadata);
        if let Err(error) = self.write_metadata_map(&map) {
            let _ = entry.delete_credential();
            return Err(error);
        }
        if let Some(old) = existing {
            if let Ok(old_entry) = Self::entry(&old.provider, &old.id) {
                let _ = old_entry.delete_credential();
            }
        }
        Ok(SecretRef {
            provider: provider.to_string(),
            id,
        })
    }

    fn get(&self, secret_ref: &SecretRef) -> Result<SecretBytes, String> {
        let _guard = self
            .metadata_lock
            .lock()
            .map_err(|_| "credential metadata lock poisoned".to_string())?;
        let mut map = self.read_metadata_map();
        let metadata = map
            .get_mut(&secret_ref.id)
            .ok_or_else(|| "credential-not-found".to_string())?;
        if metadata.provider != secret_ref.provider {
            return Err("credential-provider-mismatch".into());
        }
        let decoded = Self::entry(&secret_ref.provider, &secret_ref.id)?
            .get_secret()
            .map_err(|e| format!("credential-store-read-failed: {e}"))?;
        metadata.last_used_ts = Some(now_ts());
        self.write_metadata_map(&map)?;
        Ok(SecretBytes::new(decoded))
    }

    fn delete(&self, secret_ref: &SecretRef) -> Result<(), String> {
        let _guard = self
            .metadata_lock
            .lock()
            .map_err(|_| "credential metadata lock poisoned".to_string())?;
        let mut map = self.read_metadata_map();
        let Some(metadata) = map.get(&secret_ref.id) else {
            return Ok(());
        };
        if metadata.provider != secret_ref.provider {
            return Err("credential-provider-mismatch".into());
        }
        let entry = Self::entry(&secret_ref.provider, &secret_ref.id)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(error) => return Err(format!("credential-store-delete-failed: {error}")),
        }
        map.remove(&secret_ref.id);
        self.write_metadata_map(&map)
    }

    fn list_metadata(&self) -> Result<Vec<SecretMetadata>, String> {
        let _guard = self
            .metadata_lock
            .lock()
            .map_err(|_| "credential metadata lock poisoned".to_string())?;
        let mut list: Vec<_> = self.read_metadata_map().into_values().collect();
        list.sort_by(|a, b| a.created_at_ts.cmp(&b.created_at_ts));
        Ok(list)
    }
}

/// In-memory secret store implementation for testing and isolated contexts.
pub struct InMemorySecretStore {
    secrets: RwLock<HashMap<String, Vec<u8>>>,
    metadata: RwLock<HashMap<String, SecretMetadata>>,
}

impl InMemorySecretStore {
    pub fn new() -> Self {
        Self {
            secrets: RwLock::new(HashMap::new()),
            metadata: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemorySecretStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore for InMemorySecretStore {
    fn set(&self, provider: &str, secret: &[u8]) -> Result<SecretRef, String> {
        if provider.trim().is_empty() {
            return Err("provider cannot be empty".into());
        }
        if secret.is_empty() {
            return Err("secret cannot be empty".into());
        }

        let mut secrets = self
            .secrets
            .write()
            .map_err(|_| "secrets lock poisoned".to_string())?;
        let mut metadata = self
            .metadata
            .write()
            .map_err(|_| "metadata lock poisoned".to_string())?;

        let existing_id = metadata
            .values()
            .find(|m| m.provider == provider)
            .map(|m| m.id.clone());

        let id = generate_secret_id();
        let now = now_ts();

        let meta = SecretMetadata {
            provider: provider.to_string(),
            id: id.clone(),
            created_at_ts: existing_id
                .as_ref()
                .and_then(|old_id| metadata.get(old_id).map(|m| m.created_at_ts))
                .unwrap_or(now),
            updated_at_ts: now,
            last_used_ts: None,
        };

        if let Some(old_id) = existing_id {
            secrets.remove(&old_id);
            metadata.remove(&old_id);
        }

        secrets.insert(id.clone(), secret.to_vec());
        metadata.insert(id.clone(), meta);

        Ok(SecretRef {
            provider: provider.to_string(),
            id,
        })
    }

    fn get(&self, secret_ref: &SecretRef) -> Result<SecretBytes, String> {
        let secrets = self
            .secrets
            .read()
            .map_err(|_| "secrets lock poisoned".to_string())?;
        let mut metadata = self
            .metadata
            .write()
            .map_err(|_| "metadata lock poisoned".to_string())?;

        let meta = metadata
            .get_mut(&secret_ref.id)
            .ok_or_else(|| "secret not found".to_string())?;

        if meta.provider != secret_ref.provider {
            return Err("secret provider mismatch".to_string());
        }

        let data = secrets
            .get(&secret_ref.id)
            .ok_or_else(|| "secret payload missing".to_string())?;

        meta.last_used_ts = Some(now_ts());
        Ok(SecretBytes::from_slice(data))
    }

    fn delete(&self, secret_ref: &SecretRef) -> Result<(), String> {
        let mut secrets = self
            .secrets
            .write()
            .map_err(|_| "secrets lock poisoned".to_string())?;
        let mut metadata = self
            .metadata
            .write()
            .map_err(|_| "metadata lock poisoned".to_string())?;

        if let Some(meta) = metadata.get(&secret_ref.id) {
            if meta.provider != secret_ref.provider {
                return Err("secret provider mismatch".to_string());
            }
            secrets.remove(&secret_ref.id);
            metadata.remove(&secret_ref.id);
        }
        Ok(())
    }

    fn list_metadata(&self) -> Result<Vec<SecretMetadata>, String> {
        let metadata = self
            .metadata
            .read()
            .map_err(|_| "metadata lock poisoned".to_string())?;
        let mut list: Vec<SecretMetadata> = metadata.values().cloned().collect();
        list.sort_by(|a, b| a.created_at_ts.cmp(&b.created_at_ts));
        Ok(list)
    }
}

/// Shared wrapper for application SecretStore managed via Tauri state.
pub struct AppSecretStore(pub Arc<dyn SecretStore>);

impl AppSecretStore {
    pub fn new(store: Arc<dyn SecretStore>) -> Self {
        Self(store)
    }
}

// ---------------------------------------------------------------------------
// Plaintext Secret Migration Guard
// ---------------------------------------------------------------------------

const SECRET_KEY_NAMES: &[&str] = &[
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

/// Checks if a JSON key name matches sensitive credential patterns.
///
/// Exempts keys ending in `_ref` (e.g. `planner_secret_ref`) which store opaque
/// `{ provider, id }` pointers rather than plaintext secret strings.
pub fn is_secret_shaped_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    if lower.ends_with("_ref") || lower.ends_with("_id") {
        return false;
    }
    SECRET_KEY_NAMES
        .iter()
        .any(|k| lower == *k || lower.contains(k))
}

/// Recursively inspects a JSON value for secret-shaped keys with plaintext values.
///
/// Returns the first detected sensitive key name if a plaintext credential is found.
pub fn detect_plaintext_secret_key(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                // If key is an opaque reference shape (like planner_secret_ref), allow it
                if k.ends_with("_ref") {
                    continue;
                }

                // If key looks like a secret and contains a string/array/number (not null or empty)
                if is_secret_shaped_key(k) {
                    match v {
                        serde_json::Value::Null => {}
                        serde_json::Value::String(s) if s.trim().is_empty() => {}
                        _ => return Some(k.clone()),
                    }
                }

                // Recurse into nested objects/arrays
                if let Some(detected) = detect_plaintext_secret_key(v) {
                    return Some(detected);
                }
            }
            None
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Some(detected) = detect_plaintext_secret_key(item) {
                    return Some(detected);
                }
            }
            None
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Harness Environment Sanitizer
// ---------------------------------------------------------------------------

/// Provider environment variables that must NEVER be leaked to harness child processes.
pub const HARNESS_SCRUBBED_ENV_VARS: &[&str] = &[
    "REFLEXDESK_PLANNER_API_KEY",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "GEMINI_API_KEY",
    "COHERE_API_KEY",
    "MISTRAL_API_KEY",
    "DEEPSEEK_API_KEY",
    "GROQ_API_KEY",
    "PERPLEXITY_API_KEY",
    "AWS_SECRET_ACCESS_KEY",
    "GITHUB_TOKEN",
];

/// Applies `env_remove` for all credential and provider keys to a process spec.
pub fn scrub_harness_env(
    mut spec: crate::process_supervisor::ProcessSpec,
) -> crate::process_supervisor::ProcessSpec {
    for &var in HARNESS_SCRUBBED_ENV_VARS {
        spec = spec.env_remove(var);
    }
    spec
}

// ---------------------------------------------------------------------------
// Provider Connection & Test Logic
// ---------------------------------------------------------------------------

/// Tests a provider endpoint with an API key, mapping 401/403 to sanitized `auth-invalid`.
///
/// `provider` is only used for the returned status label; the endpoint is the
/// caller's fixed, policy-checked probe URL.
pub fn test_provider_connection(
    provider: &str,
    endpoint: &str,
    api_key: &SecretBytes,
    persisted_allow_online: bool,
) -> ProviderConnectionStatus {
    use crate::security::check_endpoint_allowed;

    if let Err(e) = check_endpoint_allowed(endpoint, persisted_allow_online) {
        return ProviderConnectionStatus {
            provider: provider.to_string(),
            connected: false,
            secret_id: None,
            updated_at_ts: Some(now_ts()),
            last_status: "unreachable".into(),
            message: format!(
                "Online AI is disabled in settings, so the credential was not sent anywhere ({e})."
            ),
        };
    }

    let models_url = if endpoint.ends_with("/chat/completions") {
        format!("{}/models", endpoint.trim_end_matches("/chat/completions"))
    } else {
        format!("{}/models", endpoint.trim_end_matches('/'))
    };

    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(2500))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return ProviderConnectionStatus {
                provider: provider.to_string(),
                connected: false,
                secret_id: None,
                updated_at_ts: Some(now_ts()),
                last_status: "unreachable".into(),
                message: format!(
                    "HTTP client initialization failed: {}",
                    crate::redaction::redact_error(&e.to_string())
                ),
            };
        }
    };

    let header = match bearer_header(api_key) {
        Ok(header) => header,
        Err(e) => {
            return ProviderConnectionStatus {
                provider: provider.to_string(),
                connected: false,
                secret_id: None,
                updated_at_ts: Some(now_ts()),
                last_status: "auth-invalid".into(),
                message: format!("Stored credential was not sent: {e}"),
            };
        }
    };

    let resp = match client
        .get(&models_url)
        .header(reqwest::header::AUTHORIZATION, header)
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            return ProviderConnectionStatus {
                provider: provider.to_string(),
                connected: false,
                secret_id: None,
                updated_at_ts: Some(now_ts()),
                last_status: "unreachable".into(),
                message: format!(
                    "Connection failed: {}",
                    crate::redaction::redact_error(&e.to_string())
                ),
            };
        }
    };

    let status = resp.status();
    let (last_status, message) = if status == reqwest::StatusCode::UNAUTHORIZED
        || status == reqwest::StatusCode::FORBIDDEN
    {
        (
            "auth-invalid",
            "Authentication failed: invalid or expired provider API key. Please rotate credentials."
                .to_string(),
        )
    } else if status.is_success() {
        ("connected", "Provider connection verified successfully.".to_string())
    } else {
        ("unreachable", format!("Provider endpoint returned HTTP {status}"))
    };

    ProviderConnectionStatus {
        provider: provider.to_string(),
        connected: last_status == "connected",
        secret_id: None,
        updated_at_ts: Some(now_ts()),
        last_status: last_status.to_string(),
        message,
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secret_bytes_redacts_debug_and_display() {
        let bytes = SecretBytes::new(vec![1, 2, 3, 4, 5]);
        assert_eq!(bytes.len(), 5);
        assert_eq!(bytes.expose_bytes(), &[1, 2, 3, 4, 5]);
        assert_eq!(format!("{bytes:?}"), "SecretBytes([REDACTED; 5 bytes])");
        assert_eq!(format!("{bytes}"), "[REDACTED_SECRET]");
    }

    #[test]
    fn test_legacy_plaintext_directory_is_quarantined_without_import() {
        let temp_dir = std::env::temp_dir().join(format!("rd_test_vault_{}", generate_secret_id()));
        let legacy_dir = temp_dir.join("secrets");
        fs::create_dir_all(&legacy_dir).unwrap();
        fs::write(legacy_dir.join("legacy.bin"), b"plaintext-secret").unwrap();

        let store = OsVaultSecretStore::new(&temp_dir).unwrap();

        assert!(!legacy_dir.exists());
        assert!(store.list_metadata().unwrap().is_empty());
        let quarantined = fs::read_dir(&temp_dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("insecure-secrets-quarantine-")
            });
        assert!(quarantined);
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_in_memory_store_lifecycle() {
        let store = InMemorySecretStore::new();

        // Set
        let sref = store.set("planner", b"sk-test-secret-1234").unwrap();
        assert_eq!(sref.provider, "planner");
        assert!(sref.id.starts_with("sec_"));

        // Get
        let retrieved = store.get(&sref).unwrap();
        assert_eq!(retrieved.expose_bytes(), b"sk-test-secret-1234");
        assert_eq!(retrieved.expose_str().unwrap(), "sk-test-secret-1234");

        // Rotate
        let sref2 = store.set("planner", b"sk-test-secret-5678").unwrap();
        assert_eq!(sref2.provider, "planner");
        assert_ne!(sref.id, sref2.id);

        let retrieved2 = store.get(&sref2).unwrap();
        assert_eq!(retrieved2.expose_bytes(), b"sk-test-secret-5678");

        // Old id should be gone
        assert!(store.get(&sref).is_err());

        // List metadata
        let list = store.list_metadata().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].provider, "planner");
        assert_eq!(list[0].id, sref2.id);

        // Delete
        store.delete(&sref2).unwrap();
        assert!(store.get(&sref2).is_err());
        assert_eq!(store.list_metadata().unwrap().len(), 0);
    }

    #[test]
    #[ignore = "requires an interactive OS credential service; exercised by platform integration CI"]
    fn test_os_vault_store_lifecycle() {
        let temp_dir = std::env::temp_dir().join(format!("rd_test_vault_{}", generate_secret_id()));
        let store = OsVaultSecretStore::new(&temp_dir).unwrap();

        let sref = store.set("openai", b"sk-proj-xyz987654321").unwrap();
        assert_eq!(sref.provider, "openai");

        let retrieved = store.get(&sref).unwrap();
        assert_eq!(retrieved.expose_bytes(), b"sk-proj-xyz987654321");

        // Metadata check
        let meta = store.list_metadata().unwrap();
        assert_eq!(meta.len(), 1);
        assert_eq!(meta[0].provider, "openai");

        // Rotate
        let sref2 = store.set("openai", b"sk-proj-new-key").unwrap();
        assert_eq!(
            store.get(&sref2).unwrap().expose_bytes(),
            b"sk-proj-new-key"
        );
        assert!(store.get(&sref).is_err());

        // Delete
        store.delete(&sref2).unwrap();
        assert!(store.get(&sref2).is_err());
        assert_eq!(store.list_metadata().unwrap().len(), 0);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_plaintext_secret_detection() {
        // Plaintext API key must be detected
        let json_with_key: serde_json::Value = serde_json::json!({
            "language": "en",
            "api_key": "sk-secret-12345"
        });
        assert_eq!(
            detect_plaintext_secret_key(&json_with_key),
            Some("api_key".into())
        );

        // Nested token must be detected
        let json_nested: serde_json::Value = serde_json::json!({
            "nested": {
                "auth": {
                    "token": "bearer-token-val"
                }
            }
        });
        assert_eq!(
            detect_plaintext_secret_key(&json_nested),
            Some("token".into())
        );

        // Password must be detected
        let json_pass: serde_json::Value = serde_json::json!({
            "password": "super-secret-password"
        });
        assert_eq!(
            detect_plaintext_secret_key(&json_pass),
            Some("password".into())
        );

        // Opaque reference shape (planner_secret_ref) MUST NOT be flagged
        let json_valid_ref: serde_json::Value = serde_json::json!({
            "language": "en",
            "planner_secret_ref": {
                "provider": "planner",
                "id": "sec_0123456789abcdef"
            }
        });
        assert_eq!(detect_plaintext_secret_key(&json_valid_ref), None);

        // Standard settings JSON MUST NOT be flagged
        let json_clean: serde_json::Value = serde_json::json!({
            "language": "en",
            "shortcut": "Ctrl+Shift+Space",
            "start_at_login": true,
            "planner_endpoint": "http://127.0.0.1:11434/v1/chat/completions",
            "planner_model": "auto"
        });
        assert_eq!(detect_plaintext_secret_key(&json_clean), None);
    }
}
