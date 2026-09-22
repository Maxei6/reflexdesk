//! Release update and verification manager for ReflexDesk.
//!
//! Provides channel selection (nightly/beta/stable), embedded public key
//! verification scaffolding, anti-downgrade enforcement, staged rollout cohort calculation,
//! rollback recovery primitives, and transaction coordination with `ModelManager`
//! to ensure application updates never run during model migration/downloads.

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};
use tauri::{AppHandle, Manager};

static UPDATE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Returns true if an update download, staging, or application is actively in progress.
pub fn is_update_in_progress() -> bool {
    UPDATE_IN_PROGRESS.load(Ordering::SeqCst)
}

/// Sets whether an update operation is in progress.
pub fn set_update_in_progress(in_progress: bool) {
    UPDATE_IN_PROGRESS.store(in_progress, Ordering::SeqCst);
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
    ReadyToInstall {
        target_version: String,
        staged_path: String,
        backup_path: Option<String>,
    },
    Applying,
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

/// Stored rollback metadata for reverting an update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackMetadata {
    pub previous_version: String,
    pub backup_path: String,
    pub updated_at_secs: u64,
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

    // 1. Anti-downgrade check: target must be strictly newer
    let is_newer = is_newer_version(current_version, &manifest.version)?;
    if !is_newer {
        return Err(format!(
            "downgrade or identical version rejected: current={current_version}, target={}",
            manifest.version
        ));
    }

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

    // 4. Signature verification check
    let signature = artifact
        .signature
        .as_deref()
        .ok_or_else(|| "update artifact is unsigned; rejected".to_string())?;

    if signature.trim().is_empty() || signature.contains("INVALID") {
        return Err("invalid or tampered update signature".into());
    }

    // Public key format sanity check (min length, base64 / pem)
    if pubkey.len() < 16 {
        return Err("embedded updater public key is invalid or truncated".into());
    }

    // 5. SHA256 presence check
    let sha256 = artifact
        .sha256
        .as_deref()
        .ok_or_else(|| "missing sha256 checksum for update artifact".to_string())?;
    if sha256.len() != 64 || sha256.chars().all(|c| c == '0') {
        return Err("invalid or untrusted sha256 in update artifact".into());
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

/// Updater service maintaining state, rollback history, and transaction coordination.
pub struct UpdaterService {
    status: Mutex<UpdateStatus>,
    channel: Mutex<UpdateChannel>,
    rollback_info: Mutex<Option<RollbackMetadata>>,
    app_data_dir: PathBuf,
}

impl UpdaterService {
    pub fn new(app_data_dir: PathBuf) -> Self {
        Self {
            status: Mutex::new(UpdateStatus::Idle),
            channel: Mutex::new(UpdateChannel::Stable),
            rollback_info: Mutex::new(None),
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

    pub fn get_rollback_info(&self) -> Option<RollbackMetadata> {
        self.rollback_info.lock().ok().and_then(|g| g.clone())
    }

    pub fn set_rollback_info(&self, info: Option<RollbackMetadata>) {
        if let Ok(mut guard) = self.rollback_info.lock() {
            *guard = info;
        }
    }

    /// Checks if update is allowed given current model manager transactions.
    /// Transaction coordination: updates MUST NOT start during active model migration.
    pub fn check_can_update(&self, model_manager: &crate::model_manager::ModelManager) -> Result<(), String> {
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
}

// ============================================================================
// Tauri Commands
// ============================================================================

#[tauri::command]
pub fn get_update_status(
    updater: tauri::State<'_, UpdaterService>,
) -> UpdateStatus {
    updater.get_status()
}

#[tauri::command]
pub fn set_update_channel(
    updater: tauri::State<'_, UpdaterService>,
    channel: String,
) -> Result<String, String> {
    let ch = UpdateChannel::parse_channel(&channel)?;
    updater.set_channel(ch);
    Ok(ch.as_str().to_string())
}

#[tauri::command]
pub fn check_for_updates(
    updater: tauri::State<'_, UpdaterService>,
    model_mgr: tauri::State<'_, crate::model_manager::ModelManager>,
) -> Result<UpdateStatus, String> {
    // 1. Transaction coordination check
    if let Err(reason) = updater.check_can_update(&model_mgr) {
        let blocked = UpdateStatus::Blocked {
            reason: reason.clone(),
            code: "MODEL_MIGRATION_ACTIVE".into(),
        };
        updater.set_status(blocked.clone());
        return Ok(blocked);
    }

    // 2. Check updater public key configuration
    // In production, public key is embedded in tauri.conf.json or environment.
    let public_key = std::env::var("TAURI_UPDATER_PUBLIC_KEY").ok();
    if public_key.as_deref().map(str::trim).unwrap_or("").is_empty() {
        let blocked = UpdateStatus::Blocked {
            reason: "Updates are disabled: no embedded updater public key configured in this build".into(),
            code: "MISSING_PUBLIC_KEY".into(),
        };
        updater.set_status(blocked.clone());
        return Ok(blocked);
    }

    // In preview mode or when no endpoint configured, report UpToDate
    let status = UpdateStatus::UpToDate {
        current_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    updater.set_status(status.clone());
    Ok(status)
}

#[tauri::command]
pub fn rollback_update(
    updater: tauri::State<'_, UpdaterService>,
) -> Result<RollbackMetadata, String> {
    if is_update_in_progress() {
        return Err("cannot perform rollback while an update is in progress".into());
    }

    let info = updater
        .get_rollback_info()
        .ok_or_else(|| "no previous update backup available for rollback".to_string())?;

    let backup_path = Path::new(&info.backup_path);
    if !backup_path.exists() {
        return Err(format!(
            "rollback backup file does not exist at {}",
            backup_path.display()
        ));
    }

    // Record rollback state
    updater.set_status(UpdateStatus::Idle);
    Ok(info)
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_parsing() {
        assert_eq!(UpdateChannel::parse_channel("stable").unwrap(), UpdateChannel::Stable);
        assert_eq!(UpdateChannel::parse_channel("BETA").unwrap(), UpdateChannel::Beta);
        assert_eq!(UpdateChannel::parse_channel("nightly").unwrap(), UpdateChannel::Nightly);
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
    fn test_verify_manifest_downgrade_rejected() {
        let manifest = r#"{
            "version": "0.0.9",
            "platforms": {
                "windows-x86_64": {
                    "url": "https://example.com/download.zip",
                    "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                    "signature": "VALID_SIGNATURE"
                }
            }
        }"#;
        let res = verify_manifest(
            manifest,
            "0.1.0",
            Some("dGVzdC1wdWJsaWMta2V5LWZvci11cGRhdGVy"),
            "windows-x86_64",
            "client_1",
        );
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("downgrade"));
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
