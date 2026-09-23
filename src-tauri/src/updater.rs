//! Release update and verification manager for ReflexDesk.
//!
//! Provides channel selection (nightly/beta/stable), embedded public-key
//! verification, anti-downgrade enforcement, staged rollout cohort calculation,
//! verified artifact staging, and transaction coordination with `ModelManager`.
//! Installer activation and rollback are intentionally unavailable until the
//! signed platform updater is wired end to end.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Duration,
};

static UPDATE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Returns true while an update check, download, or staging operation is active.
pub fn is_update_in_progress() -> bool {
    UPDATE_IN_PROGRESS.load(Ordering::SeqCst)
}

struct UpdateTransaction;

impl UpdateTransaction {
    fn begin() -> Result<Self, String> {
        UPDATE_IN_PROGRESS
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| "update already in progress".to_string())?;
        Ok(Self)
    }
}

impl Drop for UpdateTransaction {
    fn drop(&mut self) {
        UPDATE_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
}

/// Release channels for ReflexDesk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    Stable,
    Beta,
    Nightly,
}

impl UpdateChannel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Nightly => "nightly",
        }
    }

    pub fn parse_channel(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "stable" => Ok(Self::Stable),
            "beta" => Ok(Self::Beta),
            "nightly" => Ok(Self::Nightly),
            other => Err(format!("unknown update channel: {other}")),
        }
    }
}

impl Default for UpdateChannel {
    fn default() -> Self {
        Self::Stable
    }
}

/// Current status of the updater engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum UpdateStatus {
    Idle,
    Checking,
    Available {
        current_version: String,
        target_version: String,
        release_notes: Option<String>,
        pub_date: Option<String>,
        artifact_url: String,
        artifact_sha256: String,
        rollout_percentage: u8,
        in_rollout_cohort: bool,
    },
    Downloading {
        target_version: String,
        percent: u8,
    },
    Staged {
        target_version: String,
        staged_path: String,
    },
    UpToDate {
        current_version: String,
    },
    Blocked {
        reason: String,
        code: String,
    },
    Failed {
        error: String,
    },
}


/// Platform artifact metadata in the release manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformArtifact {
    pub url: String,
    pub sha256: Option<String>,
    pub signature: Option<String>,
}

/// Parsed release manifest matching the Tauri updater format with staged rollout extensions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseManifest {
    pub version: String,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
    #[serde(default)]
    pub platforms: HashMap<String, PlatformArtifact>,
    #[serde(default = "default_rollout_percentage")]
    pub rollout_percentage: u8,
}

fn default_rollout_percentage() -> u8 {
    100
}

/// Verified update result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifiedUpdate {
    pub target_version: String,
    pub release_notes: Option<String>,
    pub pub_date: Option<String>,
    pub artifact_url: String,
    pub artifact_sha256: String,
    pub rollout_percentage: u8,
    pub in_rollout_cohort: bool,
}

/// Calculate deterministic rollout bucket (0..99) from client ID and target version.
pub fn calculate_rollout_bucket(client_id: &str, target_version: &str) -> u8 {
    let mut hash: u32 = 5381;
    for b in client_id.as_bytes() {
        hash = ((hash << 5).wrapping_add(hash)).wrapping_add(*b as u32);
    }
    for b in target_version.as_bytes() {
        hash = ((hash << 5).wrapping_add(hash)).wrapping_add(*b as u32);
    }
    (hash % 100) as u8
}

/// Check if client is within the rollout cohort percentage.
pub fn is_in_rollout(client_id: &str, target_version: &str, rollout_percentage: u8) -> bool {
    if rollout_percentage >= 100 {
        return true;
    }
    if rollout_percentage == 0 {
        return false;
    }
    let bucket = calculate_rollout_bucket(client_id, target_version);
    bucket < rollout_percentage
}

/// Semantic version components: major.minor.patch.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct SemVer {
    major: u64,
    minor: u64,
    patch: u64,
}

fn parse_semver(s: &str) -> Result<SemVer, String> {
    let clean = s.trim().trim_start_matches('v');
    let core = clean.split('-').next().unwrap_or(clean);
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() < 3 {
        return Err(format!("invalid semver string: {s}"));
    }
    let major = parts[0]
        .parse::<u64>()
        .map_err(|e| format!("invalid major in {s}: {e}"))?;
    let minor = parts[1]
        .parse::<u64>()
        .map_err(|e| format!("invalid minor in {s}: {e}"))?;
    let patch = parts[2]
        .parse::<u64>()
        .map_err(|e| format!("invalid patch in {s}: {e}"))?;
    Ok(SemVer {
        major,
        minor,
        patch,
    })
}

/// Checks whether target_version is strictly newer than current_version.
/// Enforces anti-downgrade invariant.
pub fn is_newer_version(current: &str, target: &str) -> Result<bool, String> {
    let cur = parse_semver(current)?;
    let tgt = parse_semver(target)?;
    Ok(tgt > cur)
}
fn signed_artifact_payload(
    version: &str,
    platform: &str,
    artifact_url: &str,
    sha256: &str,
) -> Vec<u8> {
    format!(
        "{version}\n{platform}\n{artifact_url}\n{}",
        sha256.to_ascii_lowercase()
    )
    .into_bytes()
}

fn verify_artifact_signature(
    public_key: &str,
    signature: &str,
    payload: &[u8],
) -> Result<(), String> {
    let key_bytes = BASE64
        .decode(public_key.trim())
        .map_err(|_| "embedded updater public key is not valid base64".to_string())?;
    let key_array: [u8; 32] = key_bytes.try_into().map_err(|_| {
        "embedded updater public key must contain exactly 32 Ed25519 bytes".to_string()
    })?;
    let verifying_key = VerifyingKey::from_bytes(&key_array)
        .map_err(|_| "embedded updater public key is invalid".to_string())?;
    let signature_bytes = BASE64
        .decode(signature.trim())
        .map_err(|_| "update artifact signature is not valid base64".to_string())?;
    let signature = Signature::from_slice(&signature_bytes).map_err(|_| {
        "update artifact signature must contain exactly 64 Ed25519 bytes".to_string()
    })?;
    verifying_key
        .verify(payload, &signature)
        .map_err(|_| "update artifact signature verification failed".to_string())
}

/// Verifies manifest integrity, anti-downgrade check, signature presence, and cohort eligibility.
pub fn verify_manifest(
    manifest_json: &str,
    current_version: &str,
    public_key: Option<&str>,
    target_platform: &str,
    client_id: &str,
) -> Result<VerifiedUpdate, String> {
    let manifest: ReleaseManifest = serde_json::from_str(manifest_json)
        .map_err(|e| format!("failed to parse update manifest: {e}"))?;

    // 2. Public key check: if public key is absent (unsigned build), fail closed
    let pubkey = match public_key {
        Some(k) if !k.trim().is_empty() => k.trim(),
        _ => {
            return Err("updater public key is not configured; updates are disabled".into());
        }
    };

    // 3. Platform artifact check
    let artifact = manifest
        .platforms
        .get(target_platform)
        .ok_or_else(|| format!("no update artifact found for platform {target_platform}"))?;

    // 4. Require a well-formed digest before verifying the signed metadata.
    let sha256 = artifact
        .sha256
        .as_deref()
        .ok_or_else(|| "missing sha256 checksum for update artifact".to_string())?;
    if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("invalid sha256 in update artifact".into());
    }

    // 5. Cryptographically authenticate exactly the metadata used for download.
    let signature = artifact
        .signature
        .as_deref()
        .ok_or_else(|| "update artifact is unsigned; rejected".to_string())?;
    let payload =
        signed_artifact_payload(&manifest.version, target_platform, &artifact.url, sha256);
    verify_artifact_signature(pubkey, signature, &payload)?;

    // Anti-downgrade is evaluated only after the manifest metadata is
    // authenticated, so an unsigned old manifest cannot suppress updates.
    if !is_newer_version(current_version, &manifest.version)? {
        return Err(format!(
            "downgrade or identical version rejected: current={current_version}, target={}",
            manifest.version
        ));
    }

    // 6. Staged rollout check
    let in_cohort = is_in_rollout(client_id, &manifest.version, manifest.rollout_percentage);

    Ok(VerifiedUpdate {
        target_version: manifest.version,
        release_notes: manifest.notes,
        pub_date: manifest.pub_date,
        artifact_url: artifact.url.clone(),
        artifact_sha256: sha256.to_string(),
        rollout_percentage: manifest.rollout_percentage,
        in_rollout_cohort: in_cohort,
    })
}

/// Updater service maintaining verified staging state and transaction coordination.
pub struct UpdaterService {
    status: Mutex<UpdateStatus>,
    channel: Mutex<UpdateChannel>,
    app_data_dir: PathBuf,
}

impl UpdaterService {
    pub fn new(app_data_dir: PathBuf) -> Self {
        Self {
            status: Mutex::new(UpdateStatus::Idle),
            channel: Mutex::new(UpdateChannel::Stable),
            app_data_dir,
        }
    }

    pub fn get_status(&self) -> UpdateStatus {
        self.status
            .lock()
            .map(|s| s.clone())
            .unwrap_or(UpdateStatus::Idle)
    }

    pub fn set_status(&self, status: UpdateStatus) {
        if let Ok(mut guard) = self.status.lock() {
            *guard = status;
        }
    }

    pub fn get_channel(&self) -> UpdateChannel {
        self.channel
            .lock()
            .map(|c| *c)
            .unwrap_or(UpdateChannel::Stable)
    }

    pub fn set_channel(&self, channel: UpdateChannel) {
        if let Ok(mut guard) = self.channel.lock() {
            *guard = channel;
        }
    }


    /// Checks if update is allowed given current model manager transactions.
    /// Transaction coordination: updates MUST NOT start during active model migration.
    pub fn check_can_update(
        &self,
        model_manager: &crate::model_manager::ModelManager,
    ) -> Result<(), String> {
        if model_manager.is_transaction_in_progress() {
            return Err("update blocked: model acquisition or migration transaction is actively in progress".into());
        }
        Ok(())
    }

    /// Prepares staging directory for update artifact.
    pub fn staging_dir(&self) -> PathBuf {
        self.app_data_dir.join("update_staging")
    }

    /// Prepares backup directory for rollback support.
    pub fn backup_dir(&self) -> PathBuf {
        self.app_data_dir.join("rollback_backup")
    }

    fn client_id(&self) -> Result<String, String> {
        let path = self.app_data_dir.join("updater-client-id");
        if let Ok(existing) = fs::read_to_string(&path) {
            let trimmed = existing.trim();
            if trimmed.len() == 32 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
                return Ok(trimmed.to_ascii_lowercase());
            }
        }
        fs::create_dir_all(&self.app_data_dir)
            .map_err(|e| format!("failed to create updater state directory: {e}"))?;
        use rand::RngCore;
        let mut bytes = [0u8; 16];
        rand::rng().fill_bytes(&mut bytes);
        let id = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        fs::write(&path, &id).map_err(|e| format!("failed to persist updater client id: {e}"))?;
        Ok(id)
    }

    fn configured_feed(&self) -> Result<String, String> {
        let key = format!(
            "REFLEXDESK_UPDATER_FEED_{}",
            self.get_channel().as_str().to_ascii_uppercase()
        );
        let feed = std::env::var(&key).map_err(|_| {
            format!(
                "update feed is not configured for {}",
                self.get_channel().as_str()
            )
        })?;
        let parsed = url::Url::parse(&feed).map_err(|_| "configured update feed URL is invalid")?;
        if parsed.scheme() != "https" || parsed.host_str().is_none() {
            return Err("configured update feed must use HTTPS with an explicit host".into());
        }
        Ok(feed)
    }

    pub fn fetch_verified_update(&self, client_id: &str) -> Result<VerifiedUpdate, String> {
        let public_key = std::env::var("TAURI_UPDATER_PUBLIC_KEY")
            .map_err(|_| "updater public key is not configured; updates are disabled")?;
        let feed = self.configured_feed()?;
        let response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| format!("failed to create update client: {e}"))?
            .get(feed)
            .send()
            .map_err(|e| format!("update feed request failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("update feed returned an error: {e}"))?;
        if response.content_length().unwrap_or(0) > 1024 * 1024 {
            return Err("update manifest exceeds 1 MiB limit".into());
        }
        let manifest = response
            .text()
            .map_err(|e| format!("failed to read update manifest: {e}"))?;
        if manifest.len() > 1024 * 1024 {
            return Err("update manifest exceeds 1 MiB limit".into());
        }
        verify_manifest(
            &manifest,
            env!("CARGO_PKG_VERSION"),
            Some(&public_key),
            &format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            client_id,
        )
    }

    pub fn download_and_stage(&self, update: &VerifiedUpdate) -> Result<PathBuf, String> {
        let _transaction = UpdateTransaction::begin()?;
        let parsed =
            url::Url::parse(&update.artifact_url).map_err(|_| "signed artifact URL is invalid")?;
        if parsed.scheme() != "https" || parsed.host_str().is_none() {
            return Err("signed update artifact URL must use HTTPS".into());
        }
        fs::create_dir_all(self.staging_dir())
            .map_err(|e| format!("failed to create update staging directory: {e}"))?;
        let part = self
            .staging_dir()
            .join(format!("reflexdesk-{}.part", update.target_version));
        let ready = self
            .staging_dir()
            .join(format!("reflexdesk-{}.verified", update.target_version));
        let mut response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(600))
            .build()
            .map_err(|e| format!("failed to create update client: {e}"))?
            .get(parsed)
            .send()
            .map_err(|e| format!("update artifact request failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("update artifact returned an error: {e}"))?;
        const MAX_ARTIFACT_BYTES: u64 = 1024 * 1024 * 1024;
        if response.content_length().unwrap_or(0) > MAX_ARTIFACT_BYTES {
            return Err("update artifact exceeds 1 GiB limit".into());
        }
        let mut file =
            File::create(&part).map_err(|e| format!("failed to create staged update: {e}"))?;
        let mut hash = Sha256::new();
        let mut total = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = response
                .read(&mut buffer)
                .map_err(|e| format!("failed reading update artifact: {e}"))?;
            if read == 0 {
                break;
            }
            total = total.saturating_add(read as u64);
            if total > MAX_ARTIFACT_BYTES {
                let _ = fs::remove_file(&part);

                return Err("update artifact exceeds 1 GiB limit".into());
            }
            hash.update(&buffer[..read]);
            file.write_all(&buffer[..read])
                .map_err(|e| format!("failed writing staged update: {e}"))?;
        }
        file.sync_all()
            .map_err(|e| format!("failed to sync staged update: {e}"))?;
        let actual = format!("{:x}", hash.finalize());
        if !actual.eq_ignore_ascii_case(&update.artifact_sha256) {
            let _ = fs::remove_file(&part);
            return Err("downloaded update SHA-256 does not match signed manifest".into());
        }
        if ready.exists() {
            fs::remove_file(&ready).map_err(|e| format!("failed replacing staged update: {e}"))?;
        }
        fs::rename(&part, &ready).map_err(|e| format!("failed to activate staged update: {e}"))?;
        Ok(ready)
    }
    pub fn stage_available(
        &self,
        target_version: &str,
        model_manager: &crate::model_manager::ModelManager,
    ) -> Result<PathBuf, String> {
        self.check_can_update(model_manager)?;
        let available = match self.get_status() {
            UpdateStatus::Available {
                target_version: version,
                release_notes,
                pub_date,
                artifact_url,
                artifact_sha256,
                rollout_percentage,
                in_rollout_cohort,
                ..
            } if version == target_version && in_rollout_cohort => VerifiedUpdate {
                target_version: version,
                release_notes,
                pub_date,
                artifact_url,
                artifact_sha256,
                rollout_percentage,
                in_rollout_cohort,
            },
            UpdateStatus::Available { .. } => {
                return Err(
                    "requested update does not match the authenticated available release".into(),
                )
            }
            _ => return Err("no authenticated update is available to stage".into()),
        };
        self.set_status(UpdateStatus::Downloading {
            target_version: target_version.to_string(),
            percent: 0,
        });
        match self.download_and_stage(&available) {
            Ok(path) => {
                self.set_status(UpdateStatus::Staged {
                    target_version: target_version.to_string(),
                    staged_path: path.display().to_string(),
                });
                Ok(path)
            }
            Err(error) => {
                self.set_status(UpdateStatus::Failed {
                    error: crate::redaction::redact_error(&error),
                });
                Err(error)
            }
        }
    }
    pub fn check_now(
        &self,
        model_manager: &crate::model_manager::ModelManager,
    ) -> Result<UpdateStatus, String> {
        if let Err(reason) = self.check_can_update(model_manager) {
            let blocked = UpdateStatus::Blocked {
                reason,
                code: "MODEL_MIGRATION_ACTIVE".into(),
            };
            self.set_status(blocked.clone());
            return Ok(blocked);
        }
        self.set_status(UpdateStatus::Checking);
        let client_id = self.client_id()?;
        let status = match self.fetch_verified_update(&client_id) {
            Ok(update) if !update.in_rollout_cohort => UpdateStatus::UpToDate {
                current_version: env!("CARGO_PKG_VERSION").to_string(),
            },
            Ok(update) => UpdateStatus::Available {
                current_version: env!("CARGO_PKG_VERSION").to_string(),
                target_version: update.target_version,
                release_notes: update.release_notes,
                pub_date: update.pub_date,
                artifact_url: update.artifact_url,
                artifact_sha256: update.artifact_sha256,
                rollout_percentage: update.rollout_percentage,
                in_rollout_cohort: true,
            },
            Err(error) => UpdateStatus::Blocked {
                reason: error,
                code: "UPDATE_CHECK_FAILED".into(),
            },
        };
        self.set_status(status.clone());
        Ok(status)
    }
}

// ============================================================================
// Tauri Commands
// ============================================================================

#[tauri::command]
pub fn get_update_status(updater: tauri::State<'_, UpdaterService>) -> UpdateStatus {
    updater.get_status()
}

#[tauri::command]
pub fn set_update_channel(
    updater: tauri::State<'_, UpdaterService>,
    channel: String,
) -> Result<String, String> {
    let channel = UpdateChannel::parse_channel(&channel)?;
    updater.set_channel(channel);
    Ok(channel.as_str().to_string())
}

#[tauri::command]
pub fn check_for_updates(
    updater: tauri::State<'_, UpdaterService>,
    model_mgr: tauri::State<'_, crate::model_manager::ModelManager>,
) -> Result<UpdateStatus, String> {
    updater.check_now(&model_mgr)
}


// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn signed_manifest(version: &str) -> (String, String) {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let url = "https://example.com/download.zip";
        let sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let payload = signed_artifact_payload(version, "windows-x86_64", url, sha256);
        let signature = BASE64.encode(signing_key.sign(&payload).to_bytes());
        let public_key = BASE64.encode(signing_key.verifying_key().to_bytes());
        (
            serde_json::json!({
                "version": version,
                "platforms": {
                    "windows-x86_64": {
                        "url": url,
                        "sha256": sha256,
                        "signature": signature
                    }
                }
            })
            .to_string(),
            public_key,
        )
    }

    #[test]
    fn test_channel_parsing() {
        assert_eq!(
            UpdateChannel::parse_channel("stable").unwrap(),
            UpdateChannel::Stable
        );
        assert_eq!(
            UpdateChannel::parse_channel("BETA").unwrap(),
            UpdateChannel::Beta
        );
        assert_eq!(
            UpdateChannel::parse_channel("nightly").unwrap(),
            UpdateChannel::Nightly
        );
        assert!(UpdateChannel::parse_channel("invalid").is_err());
    }

    #[test]
    fn test_semver_anti_downgrade() {
        assert!(is_newer_version("0.1.0", "0.2.0").unwrap());
        assert!(is_newer_version("0.1.0", "0.1.1").unwrap());
        assert!(is_newer_version("0.1.0", "1.0.0").unwrap());
        assert!(!is_newer_version("0.2.0", "0.1.0").unwrap());
        assert!(!is_newer_version("0.1.0", "0.1.0").unwrap());
    }

    #[test]
    fn test_rollout_cohort_deterministic() {
        let bucket1 = calculate_rollout_bucket("client_123", "0.2.0");
        let bucket2 = calculate_rollout_bucket("client_123", "0.2.0");
        assert_eq!(bucket1, bucket2);
        assert!(bucket1 < 100);

        assert!(is_in_rollout("client_123", "0.2.0", 100));
        assert!(!is_in_rollout("client_123", "0.2.0", 0));
    }

    #[test]
    fn test_verify_manifest_downgrade_rejected_after_authentication() {
        let (manifest, public_key) = signed_manifest("0.0.9");
        let error = verify_manifest(
            &manifest,
            "0.1.0",
            Some(&public_key),
            "windows-x86_64",
            "client_1",
        )
        .unwrap_err();
        assert!(error.contains("downgrade"));
    }

    #[test]
    fn test_verify_manifest_tampering_rejected_cryptographically() {
        let (manifest, public_key) = signed_manifest("0.2.0");
        let tampered = manifest.replace("download.zip", "evil.zip");
        let error = verify_manifest(
            &tampered,
            "0.1.0",
            Some(&public_key),
            "windows-x86_64",
            "client_1",
        )
        .unwrap_err();
        assert!(error.contains("signature verification failed"));
    }

    #[test]
    fn test_verify_manifest_missing_pubkey_fails_closed() {
        let manifest = r#"{
            "version": "0.2.0",
            "platforms": {
                "windows-x86_64": {
                    "url": "https://example.com/download.zip",
                    "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                    "signature": "VALID_SIGNATURE"
                }
            }
        }"#;
        let res = verify_manifest(manifest, "0.1.0", None, "windows-x86_64", "client_1");
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("public key is not configured"));
    }
}
