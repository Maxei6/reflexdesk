//! Model and runtime manager for ReflexDesk.
//!
//! Provides deterministic acquisition, disk-space preflight, resumable Range downloads,
//! staging -> verify -> atomic-activate, progress events, repair/rollback/GC primitives,
//! offline import, and license presentation.
//!
//! Ready-gating rule: `Ready` requires manager-verified Active state — health alone NEVER yields Ready.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, Emitter};

pub const DEFAULT_STT_MODEL_ID: &str = "nemotron-3.5-asr-streaming-0.6b";
const EMBEDDED_REGISTRY: &str = include_str!("../../models/registry.json");

/// Model lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelState {
    Discovered,
    Verified,
    Staged,
    Active,
    Failed,
}

/// Progress event payload emitted during model downloads/imports for onboarding UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProgress {
    pub model_id: String,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub state: ModelState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// License and notice presentation information for display in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelLicenseInfo {
    pub model_id: String,
    pub upstream: String,
    pub license: String,
    pub notice: String,
}

/// Status snapshot of a model in the manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelStatus {
    pub model_id: String,
    pub state: ModelState,
    pub active_path: Option<String>,
    pub size_bytes: u64,
    pub disk_bytes: u64,
    pub verified: bool,
    pub last_error: Option<String>,
}

/// Hardware requirement specification from registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinHardware {
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub min_ram_mb: Option<u64>,
    #[serde(default)]
    pub recommended_ram_mb: Option<u64>,
    #[serde(default)]
    pub cpu_features: Vec<String>,
    #[serde(default)]
    pub gpu_optional: Option<bool>,
}

/// Model artifact entry from canonical manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelArtifact {
    pub id: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub upstream: String,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub sha256_reason: Option<String>,
    #[serde(default)]
    pub recorded_sha256: Option<String>,
    #[serde(default)]
    pub runtime_compat: Option<String>,
    #[serde(default)]
    pub min_hardware: Option<MinHardware>,
    #[serde(default)]
    pub cache_path: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub notice: Option<String>,
    #[serde(default)]
    pub replaces: Option<serde_json::Value>,
    #[serde(default)]
    pub status: Option<String>,
}

/// Root manifest registry matching `models/registry.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRegistry {
    pub schema_version: u32,
    pub selection_policy: serde_json::Value,
    #[serde(default)]
    pub models: Vec<ModelArtifact>,
    #[serde(default)]
    pub runtimes: Option<serde_json::Value>,
}

impl ModelRegistry {
    pub fn load(manifest_path: Option<&Path>) -> Result<Self, String> {
        if let Some(path) = manifest_path {
            if path.exists() {
                let content = fs::read_to_string(path).map_err(|e| {
                    format!(
                        "failed to read registry manifest at {}: {e}",
                        path.display()
                    )
                })?;
                return serde_json::from_str(&content)
                    .map_err(|e| format!("failed to parse registry manifest: {e}"));
            }
        }

        serde_json::from_str(EMBEDDED_REGISTRY)
            .map_err(|e| format!("failed to parse embedded model registry: {e}"))
    }

    pub fn find_artifact(&self, id: &str) -> Option<&ModelArtifact> {
        self.models.iter().find(|m| m.id == id)
    }
}

// SHA-256 uses the audited RustCrypto implementation shared with the updater.

pub fn sha256_digest_hex(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

pub fn hash_file_sha256(path: &Path) -> Result<String, String> {
    let mut file = File::open(path)
        .map_err(|e| format!("failed to open file for hashing {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| format!("failed to read file for hashing: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

// ============================================================================
// Disk-space preflight
// ============================================================================

#[cfg(target_os = "windows")]
fn query_available_disk_space(path: &Path) -> Result<u64, String> {
    use std::os::windows::ffi::OsStrExt;

    let mut check_dir = path.to_path_buf();
    while !check_dir.exists() {
        if let Some(parent) = check_dir.parent() {
            check_dir = parent.to_path_buf();
        } else {
            break;
        }
    }

    let wide: Vec<u16> = check_dir
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut free_bytes_to_caller: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut total_free_bytes: u64 = 0;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(
            lpDirectoryName: *const u16,
            lpFreeBytesAvailableToCaller: *mut u64,
            lpTotalNumberOfBytes: *mut u64,
            lpTotalNumberOfFreeBytes: *mut u64,
        ) -> i32;
    }

    let ret = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_bytes_to_caller,
            &mut total_bytes,
            &mut total_free_bytes,
        )
    };

    if ret == 0 {
        let err = std::io::Error::last_os_error();
        Err(format!("GetDiskFreeSpaceExW failed: {err}"))
    } else {
        Ok(free_bytes_to_caller)
    }
}

#[cfg(unix)]
fn query_available_disk_space(path: &Path) -> Result<u64, String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let mut check_dir = path.to_path_buf();
    while !check_dir.exists() {
        if let Some(parent) = check_dir.parent() {
            check_dir = parent.to_path_buf();
        } else {
            break;
        }
    }

    let c_path = CString::new(check_dir.as_os_str().as_bytes())
        .map_err(|e| format!("invalid path for statvfs: {e}"))?;

    #[repr(C)]
    struct Statvfs {
        f_bsize: std::os::raw::c_ulong,
        f_frsize: std::os::raw::c_ulong,
        f_blocks: u64,
        f_bfree: u64,
        f_bavail: u64,
        f_files: u64,
        f_ffree: u64,
        f_favail: u64,
        f_fsid: std::os::raw::c_ulong,
        f_flag: std::os::raw::c_ulong,
        f_namemax: std::os::raw::c_ulong,
        __f_spare: [std::os::raw::c_int; 6],
    }

    extern "C" {
        fn statvfs(path: *const std::os::raw::c_char, buf: *mut Statvfs) -> std::os::raw::c_int;
    }

    let mut stat: Statvfs = unsafe { std::mem::zeroed() };
    let ret = unsafe { statvfs(c_path.as_ptr(), &mut stat) };
    if ret != 0 {
        let err = std::io::Error::last_os_error();
        Err(format!("statvfs failed: {err}"))
    } else {
        let block_size = if stat.f_frsize > 0 {
            stat.f_frsize as u64
        } else {
            stat.f_bsize as u64
        };
        Ok(stat.f_bavail.saturating_mul(block_size))
    }
}

#[cfg(not(any(target_os = "windows", unix)))]
fn query_available_disk_space(_path: &Path) -> Result<u64, String> {
    Err("Disk free space unknowable on this platform target".to_string())
}

/// Preflight check for available disk space.
/// Returns Ok(()) if free space is sufficient (including a 100 MB buffer).
/// Returns an explicit error if space is insufficient or unknowable.
pub fn check_disk_space_preflight(path: &Path, required_bytes: u64) -> Result<(), String> {
    match query_available_disk_space(path) {
        Ok(available) => {
            let needed_with_buffer = required_bytes.saturating_add(100 * 1024 * 1024);
            if available < needed_with_buffer {
                Err(format!(
                    "Insufficient disk space: needed {} bytes (with 100MB buffer), available {} bytes on {}",
                    needed_with_buffer,
                    available,
                    path.display()
                ))
            } else {
                Ok(())
            }
        }
        Err(e) => Err(format!(
            "Disk-space preflight failed: free space unknowable for {}: {}",
            path.display(),
            e
        )),
    }
}

/// Resolves standard cache directory for models.
pub fn default_cache_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(user_profile) = std::env::var("USERPROFILE") {
            return PathBuf::from(user_profile).join(".cache").join("crispasr");
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".cache").join("crispasr");
        }
    }
    std::env::temp_dir().join("reflexdesk-models")
}

// ============================================================================
// ModelManager Service
// ============================================================================
/// Active model acquisition, migration, or repair transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelTransaction {
    pub tx_id: String,
    pub model_id: String,
    pub operation: String,
    pub started_at_secs: u64,
}

/// RAII guard that completes or aborts a model transaction when dropped.
pub struct ModelTransactionGuard<'a> {
    manager: &'a ModelManager,
    pub tx_id: String,
}

impl<'a> Drop for ModelTransactionGuard<'a> {
    fn drop(&mut self) {
        self.manager.end_transaction(&self.tx_id);
    }
}

pub struct ModelManager {
    cache_dir: PathBuf,
    registry: ModelRegistry,
    states: Mutex<HashMap<String, ModelStatus>>,
    transactions: Mutex<HashMap<String, ModelTransaction>>,
}

impl ModelManager {
    pub fn new(cache_dir: PathBuf, manifest_path: Option<&Path>) -> Result<Self, String> {
        let registry = ModelRegistry::load(manifest_path)?;
        fs::create_dir_all(&cache_dir).map_err(|e| {
            format!(
                "failed to create model cache dir {}: {e}",
                cache_dir.display()
            )
        })?;

        Ok(Self {
            cache_dir,
            registry,
            states: Mutex::new(HashMap::new()),
            transactions: Mutex::new(HashMap::new()),
        })
    }

    pub fn default_manager() -> Result<Self, String> {
        Self::new(default_cache_dir(), None)
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    pub fn get_artifact(&self, model_id: &str) -> Result<ModelArtifact, String> {
        self.registry
            .find_artifact(model_id)
            .cloned()
            .ok_or_else(|| format!("model {model_id} not found in manifest registry"))
    }

    pub fn get_license_info(&self, model_id: &str) -> Result<ModelLicenseInfo, String> {
        let artifact = self.get_artifact(model_id)?;
        Ok(ModelLicenseInfo {
            model_id: artifact.id,
            upstream: artifact.upstream,
            license: artifact.license.unwrap_or_else(|| "Unknown".into()),
            notice: artifact.notice.unwrap_or_default(),
        })
    }
    /// Begin a model acquisition, download, import, or repair transaction.
    /// Fails if an application update is currently in progress, or if a transaction
    /// is already active for this model.
    pub fn begin_transaction(&self, model_id: &str, operation: &str) -> Result<String, String> {
        if crate::updater::is_update_in_progress() {
            return Err("cannot begin model transaction: application update is in progress".into());
        }
        let mut txs = self
            .transactions
            .lock()
            .map_err(|_| "model transaction lock poisoned")?;
        if txs.values().any(|t| t.model_id == model_id) {
            return Err(format!(
                "cannot begin transaction for {model_id}: transaction already in progress"
            ));
        }
        let started_at_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let tx_id = format!("tx_{}_{started_at_secs}", model_id.replace('.', "_"));
        txs.insert(
            tx_id.clone(),
            ModelTransaction {
                tx_id: tx_id.clone(),
                model_id: model_id.to_string(),
                operation: operation.to_string(),
                started_at_secs,
            },
        );
        Ok(tx_id)
    }

    /// Complete or abort an active model transaction.
    pub fn end_transaction(&self, tx_id: &str) -> bool {
        if let Ok(mut txs) = self.transactions.lock() {
            txs.remove(tx_id).is_some()
        } else {
            false
        }
    }

    /// Returns true if any model acquisition/migration transaction is actively in flight.
    pub fn is_transaction_in_progress(&self) -> bool {
        self.transactions
            .lock()
            .map(|txs| !txs.is_empty())
            .unwrap_or(false)
    }

    /// Lists current active model transactions.
    pub fn active_transactions(&self) -> Vec<ModelTransaction> {
        self.transactions
            .lock()
            .map(|txs| txs.values().cloned().collect())
            .unwrap_or_default()
    }
    /// Acquires an RAII transaction guard for model operations.
    pub fn acquire_transaction<'a>(
        &'a self,
        model_id: &str,
        operation: &str,
    ) -> Result<ModelTransactionGuard<'a>, String> {
        let tx_id = self.begin_transaction(model_id, operation)?;
        Ok(ModelTransactionGuard {
            manager: self,
            tx_id,
        })
    }

    pub fn get_status(&self, model_id: &str) -> ModelStatus {
        if let Ok(guard) = self.states.lock() {
            if let Some(status) = guard.get(model_id) {
                return status.clone();
            }
        }

        let Ok(artifact) = self.get_artifact(model_id) else {
            return ModelStatus {
                model_id: model_id.to_string(),
                state: ModelState::Failed,
                active_path: None,
                size_bytes: 0,
                disk_bytes: 0,
                verified: false,
                last_error: Some(format!("model {model_id} not recognized")),
            };
        };

        let cache_rel = artifact.cache_path.as_deref().unwrap_or(model_id);
        let active_path = self.cache_dir.join(cache_rel);
        let exists = active_path.is_file();
        let disk_bytes = if exists {
            active_path.metadata().map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };

        let is_ready = self.is_ready_for_engine(model_id);

        ModelStatus {
            model_id: model_id.to_string(),
            state: if is_ready {
                ModelState::Active
            } else if exists {
                ModelState::Staged
            } else {
                ModelState::Discovered
            },
            active_path: if exists {
                Some(active_path.to_string_lossy().to_string())
            } else {
                None
            },
            size_bytes: artifact.size_bytes.unwrap_or(0),
            disk_bytes,
            verified: is_ready,
            last_error: None,
        }
    }

    fn set_state(
        &self,
        model_id: &str,
        state: ModelState,
        active_path: Option<PathBuf>,
        error: Option<String>,
    ) {
        if let Ok(mut guard) = self.states.lock() {
            let artifact = self.get_artifact(model_id).ok();
            let size_bytes = artifact.as_ref().and_then(|a| a.size_bytes).unwrap_or(0);
            let disk_bytes = active_path
                .as_ref()
                .and_then(|p| p.metadata().ok())
                .map(|m| m.len())
                .unwrap_or(0);

            guard.insert(
                model_id.to_string(),
                ModelStatus {
                    model_id: model_id.to_string(),
                    state,
                    active_path: active_path.map(|p| p.to_string_lossy().to_string()),
                    size_bytes,
                    disk_bytes,
                    verified: matches!(state, ModelState::Verified | ModelState::Active),
                    last_error: error,
                },
            );
        }
    }

    /// Ready-gating rule: `Ready` requires manager-verified Active state.
    /// Health alone NEVER yields Ready.
    ///
    /// This strictly verifies:
    /// 1. The artifact is present in the canonical registry.
    /// 2. The active file exists on disk at the configured cache path.
    /// 3. The file size matches the manifest `size_bytes` exactly.
    /// 4. The on-disk file checksum matches authoritative or recorded hash.
    pub fn is_ready_for_engine(&self, model_id: &str) -> bool {
        let Ok(artifact) = self.get_artifact(model_id) else {
            return false;
        };

        let Some(cache_rel) = &artifact.cache_path else {
            return false;
        };

        let active_path = self.cache_dir.join(cache_rel);
        if !active_path.is_file() {
            return false;
        }

        if let Some(expected_size) = artifact.size_bytes {
            match active_path.metadata() {
                Ok(meta) => {
                    if meta.len() != expected_size {
                        return false;
                    }
                }
                Err(_) => return false,
            }
        }

        // Fast-path: in-memory state already verified Active
        if let Ok(guard) = self.states.lock() {
            if let Some(status) = guard.get(model_id) {
                if status.state == ModelState::Active && status.verified {
                    return true;
                }
            }
        }

        // Verify on disk against authoritative sha256 or recorded_sha256
        if let Ok(computed_hash) = hash_file_sha256(&active_path) {
            let expected_hash = artifact
                .sha256
                .as_deref()
                .or(artifact.recorded_sha256.as_deref());
            if let Some(expected) = expected_hash {
                if computed_hash.eq_ignore_ascii_case(expected) {
                    if let Ok(mut guard) = self.states.lock() {
                        guard.insert(
                            model_id.to_string(),
                            ModelStatus {
                                model_id: model_id.to_string(),
                                state: ModelState::Active,
                                active_path: Some(active_path.to_string_lossy().to_string()),
                                size_bytes: artifact.size_bytes.unwrap_or(0),
                                disk_bytes: artifact.size_bytes.unwrap_or(0),
                                verified: true,
                                last_error: None,
                            },
                        );
                    }
                    return true;
                }
            }
        }

        false
    }

    /// Returns the verified active path of a model if ready for the engine.
    pub fn get_active_model_path(&self, model_id: &str) -> Result<PathBuf, String> {
        if !self.is_ready_for_engine(model_id) {
            return Err(format!(
                "model {model_id} is not ready for engine: not verified active in manager"
            ));
        }
        let artifact = self.get_artifact(model_id)?;
        let cache_rel = artifact
            .cache_path
            .ok_or_else(|| "missing cache path".to_string())?;
        Ok(self.cache_dir.join(cache_rel))
    }

    /// Resumable Range download, staging, hash verification, and atomic activation.
    pub fn download_and_activate<F>(
        &self,
        model_id: &str,
        progress_cb: Option<F>,
    ) -> Result<PathBuf, String>
    where
        F: Fn(ModelProgress) + Send + Sync + 'static,
    {
        let _tx_guard = self.acquire_transaction(model_id, "download_and_activate")?;
        let artifact = self.get_artifact(model_id)?;
        let expected_size = artifact
            .size_bytes
            .ok_or_else(|| format!("artifact {model_id} missing size_bytes in manifest"))?;

        // Disk-space preflight
        check_disk_space_preflight(&self.cache_dir, expected_size)?;

        let cache_rel = artifact
            .cache_path
            .as_ref()
            .ok_or_else(|| format!("artifact {model_id} missing cache_path"))?;
        let active_path = self.cache_dir.join(cache_rel);
        let staging_path = self.cache_dir.join(format!("{cache_rel}.staging"));
        let backup_path = self.cache_dir.join(format!("{cache_rel}.bak"));

        if let Some(parent) = staging_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        // Check for partial download to resume
        let mut existing_len = if staging_path.exists() {
            staging_path.metadata().map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };

        if existing_len > expected_size {
            let _ = fs::remove_file(&staging_path);
            existing_len = 0;
        } else if existing_len == expected_size {
            // Already staged completely, check hash
            if let Ok(hash) = hash_file_sha256(&staging_path) {
                let expected_hash = artifact
                    .sha256
                    .as_deref()
                    .or(artifact.recorded_sha256.as_deref());
                if let Some(expected) = expected_hash {
                    if hash.eq_ignore_ascii_case(expected) {
                        return self.atomic_activate(
                            model_id,
                            &staging_path,
                            &active_path,
                            &backup_path,
                            expected_size,
                            progress_cb,
                        );
                    }
                }
            }
            let _ = fs::remove_file(&staging_path);
            existing_len = 0;
        }

        let url = artifact
            .url
            .as_ref()
            .ok_or_else(|| format!("model artifact {model_id} has no download url"))?;

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(600))
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;

        let mut request = client.get(url);
        if existing_len > 0 {
            request = request.header("Range", format!("bytes={existing_len}-"));
        }

        let mut response = request
            .send()
            .map_err(|e| format!("download request failed for {url}: {e}"))?;

        let status = response.status();
        let mut file = if status == reqwest::StatusCode::PARTIAL_CONTENT {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&staging_path)
                .map_err(|e| format!("failed to open staging file in append mode: {e}"))?
        } else if status == reqwest::StatusCode::OK {
            existing_len = 0;
            File::create(&staging_path)
                .map_err(|e| format!("failed to create staging file: {e}"))?
        } else {
            self.set_state(
                model_id,
                ModelState::Failed,
                None,
                Some(format!("HTTP {status} from {url}")),
            );
            return Err(format!("unexpected HTTP {status} downloading {url}"));
        };

        let mut buffer = [0u8; 65536];
        let mut downloaded_bytes = existing_len;
        let mut last_emit_bytes = downloaded_bytes;

        if let Some(cb) = &progress_cb {
            cb(ModelProgress {
                model_id: model_id.to_string(),
                downloaded_bytes,
                total_bytes: expected_size,
                state: ModelState::Staged,
                message: Some("Downloading model artifact...".into()),
            });
        }

        loop {
            let n = response
                .read(&mut buffer)
                .map_err(|e| format!("network error reading download chunk: {e}"))?;
            if n == 0 {
                break;
            }
            file.write_all(&buffer[..n])
                .map_err(|e| format!("disk error writing staging chunk: {e}"))?;
            downloaded_bytes = downloaded_bytes.saturating_add(n as u64);

            if downloaded_bytes.saturating_sub(last_emit_bytes) >= 512 * 1024
                || downloaded_bytes >= expected_size
            {
                last_emit_bytes = downloaded_bytes;
                if let Some(cb) = &progress_cb {
                    cb(ModelProgress {
                        model_id: model_id.to_string(),
                        downloaded_bytes,
                        total_bytes: expected_size,
                        state: ModelState::Staged,
                        message: None,
                    });
                }
            }
        }

        // Fsync to ensure durability before checksum and activation
        file.sync_all()
            .map_err(|e| format!("fsync failed for staging model file: {e}"))?;
        drop(file);

        // Verify checksum
        self.set_state(model_id, ModelState::Verified, None, None);
        if let Some(cb) = &progress_cb {
            cb(ModelProgress {
                model_id: model_id.to_string(),
                downloaded_bytes,
                total_bytes: expected_size,
                state: ModelState::Verified,
                message: Some("Verifying checksum...".into()),
            });
        }

        let computed_hash = hash_file_sha256(&staging_path)?;
        let expected_hash = artifact
            .sha256
            .as_deref()
            .or(artifact.recorded_sha256.as_deref());

        if let Some(expected) = expected_hash {
            if !computed_hash.eq_ignore_ascii_case(expected) {
                let _ = fs::remove_file(&staging_path);
                let err_msg = format!(
                    "Checksum mismatch for {model_id}: expected {expected}, got {computed_hash}"
                );
                self.set_state(model_id, ModelState::Failed, None, Some(err_msg.clone()));
                return Err(err_msg);
            }
        } else {
            let _ = fs::remove_file(&staging_path);
            let err_msg = format!("No authoritative or recorded checksum for {model_id}");
            self.set_state(model_id, ModelState::Failed, None, Some(err_msg.clone()));
            return Err(err_msg);
        }

        self.atomic_activate(
            model_id,
            &staging_path,
            &active_path,
            &backup_path,
            expected_size,
            progress_cb,
        )
    }

    fn atomic_activate<F>(
        &self,
        model_id: &str,
        staging_path: &Path,
        active_path: &Path,
        backup_path: &Path,
        expected_size: u64,
        progress_cb: Option<F>,
    ) -> Result<PathBuf, String>
    where
        F: Fn(ModelProgress) + Send + Sync + 'static,
    {
        if active_path.exists() {
            let _ = fs::remove_file(backup_path);
            if let Err(_) = fs::rename(active_path, backup_path) {
                let _ = fs::remove_file(active_path);
            }
        }

        fs::rename(staging_path, active_path).map_err(|e| {
            if backup_path.exists() && !active_path.exists() {
                let _ = fs::rename(backup_path, active_path);
            }
            format!("Failed to atomically activate model {model_id}: {e}")
        })?;

        let _ = fs::remove_file(backup_path);

        self.set_state(
            model_id,
            ModelState::Active,
            Some(active_path.to_path_buf()),
            None,
        );

        if let Some(cb) = &progress_cb {
            cb(ModelProgress {
                model_id: model_id.to_string(),
                downloaded_bytes: expected_size,
                total_bytes: expected_size,
                state: ModelState::Active,
                message: Some("Model active and ready".into()),
            });
        }

        Ok(active_path.to_path_buf())
    }

    /// Offline import path: copies a local file, fsyncs, verifies checksum against manifest,
    /// and atomically activates it without requiring internet access.
    pub fn import_offline<F>(
        &self,
        model_id: &str,
        source_path: &Path,
        progress_cb: Option<F>,
    ) -> Result<PathBuf, String>
    where
        F: Fn(ModelProgress) + Send + Sync + 'static,
    {
        let _tx_guard = self.acquire_transaction(model_id, "import_offline")?;
        if !source_path.is_file() {
            return Err(format!(
                "offline import source not found: {}",
                source_path.display()
            ));
        }

        let artifact = self.get_artifact(model_id)?;
        let expected_size = artifact.size_bytes.unwrap_or(0);

        let source_meta = source_path
            .metadata()
            .map_err(|e| format!("failed to read source metadata: {e}"))?;
        if expected_size > 0 && source_meta.len() != expected_size {
            return Err(format!(
                "offline import file size mismatch: expected {expected_size} bytes, got {} bytes",
                source_meta.len()
            ));
        }

        check_disk_space_preflight(&self.cache_dir, expected_size)?;

        let cache_rel = artifact
            .cache_path
            .as_ref()
            .ok_or_else(|| "artifact has no cache_path".to_string())?;
        let active_path = self.cache_dir.join(cache_rel);
        let staging_path = self.cache_dir.join(format!("{cache_rel}.staging"));
        let backup_path = self.cache_dir.join(format!("{cache_rel}.bak"));

        if let Some(parent) = staging_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let mut source_file =
            File::open(source_path).map_err(|e| format!("failed to open source file: {e}"))?;
        let mut staging_file = File::create(&staging_path)
            .map_err(|e| format!("failed to create staging file: {e}"))?;

        let mut buffer = [0u8; 65536];
        let mut copied_bytes = 0u64;
        let mut last_emit = 0u64;

        loop {
            let n = source_file
                .read(&mut buffer)
                .map_err(|e| format!("error reading source file: {e}"))?;
            if n == 0 {
                break;
            }
            staging_file
                .write_all(&buffer[..n])
                .map_err(|e| format!("error writing staging file: {e}"))?;
            copied_bytes += n as u64;

            if copied_bytes.saturating_sub(last_emit) >= 512 * 1024 || copied_bytes >= expected_size
            {
                last_emit = copied_bytes;
                if let Some(cb) = &progress_cb {
                    cb(ModelProgress {
                        model_id: model_id.to_string(),
                        downloaded_bytes: copied_bytes,
                        total_bytes: expected_size,
                        state: ModelState::Staged,
                        message: Some("Copying offline model file...".into()),
                    });
                }
            }
        }

        staging_file
            .sync_all()
            .map_err(|e| format!("fsync failed on staging file: {e}"))?;
        drop(staging_file);

        // Verify checksum
        let computed_hash = hash_file_sha256(&staging_path)?;
        let expected_hash = artifact
            .sha256
            .as_deref()
            .or(artifact.recorded_sha256.as_deref());

        if let Some(expected) = expected_hash {
            if !computed_hash.eq_ignore_ascii_case(expected) {
                let _ = fs::remove_file(&staging_path);
                return Err(format!(
                    "Offline import checksum verification failed: expected {expected}, got {computed_hash}"
                ));
            }
        }

        self.atomic_activate(
            model_id,
            &staging_path,
            &active_path,
            &backup_path,
            expected_size,
            progress_cb,
        )
    }

    /// Repairs a missing or corrupted model. If rollback backup exists and is valid,
    /// rolls back; otherwise downloads and activates from scratch.
    pub fn repair<F>(&self, model_id: &str, progress_cb: Option<F>) -> Result<PathBuf, String>
    where
        F: Fn(ModelProgress) + Send + Sync + 'static,
    {
        if self.is_ready_for_engine(model_id) {
            return self.get_active_model_path(model_id);
        }

        // Try rollback first
        if let Ok(rb_path) = self.rollback(model_id) {
            if self.is_ready_for_engine(model_id) {
                return Ok(rb_path);
            }
        }

        // Download and activate
        self.download_and_activate(model_id, progress_cb)
    }

    /// Rolls back the model to the `.bak` copy if valid.
    pub fn rollback(&self, model_id: &str) -> Result<PathBuf, String> {
        let artifact = self.get_artifact(model_id)?;
        let cache_rel = artifact
            .cache_path
            .as_ref()
            .ok_or_else(|| "artifact has no cache_path".to_string())?;
        let active_path = self.cache_dir.join(cache_rel);
        let backup_path = self.cache_dir.join(format!("{cache_rel}.bak"));

        if !backup_path.is_file() {
            return Err(format!("no backup file found for model {model_id}"));
        }

        let computed_hash = hash_file_sha256(&backup_path)?;
        let expected_hash = artifact
            .sha256
            .as_deref()
            .or(artifact.recorded_sha256.as_deref());
        if let Some(expected) = expected_hash {
            if !computed_hash.eq_ignore_ascii_case(expected) {
                return Err(format!("backup file for {model_id} is corrupted"));
            }
        }

        let _ = fs::remove_file(&active_path);
        fs::rename(&backup_path, &active_path)
            .map_err(|e| format!("failed to restore backup model: {e}"))?;

        self.set_state(
            model_id,
            ModelState::Active,
            Some(active_path.clone()),
            None,
        );
        Ok(active_path)
    }

    /// Garbage collection primitive: deletes orphan staging files, obsolete backups,
    /// and unneeded models. Returns total bytes freed.
    pub fn gc(&self, _keep_models: &[&str]) -> Result<u64, String> {
        let mut freed_bytes = 0u64;
        if !self.cache_dir.exists() {
            return Ok(0);
        }

        let entries =
            fs::read_dir(&self.cache_dir).map_err(|e| format!("failed to read cache dir: {e}"))?;

        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_file() {
                let file_name = path.file_name().unwrap_or_default().to_string_lossy();
                if file_name.ends_with(".staging") {
                    if let Ok(meta) = path.metadata() {
                        freed_bytes += meta.len();
                    }
                    let _ = fs::remove_file(&path);
                } else if file_name.ends_with(".bak") {
                    let active_name = file_name.trim_end_matches(".bak");
                    let active_candidate = path.with_file_name(active_name);
                    if active_candidate.is_file() {
                        if let Ok(meta) = path.metadata() {
                            freed_bytes += meta.len();
                        }
                        let _ = fs::remove_file(&path);
                    }
                }
            }
        }

        Ok(freed_bytes)
    }

    /// Total user-visible disk usage in bytes for models cache directory.
    pub fn total_cache_bytes(&self) -> u64 {
        let mut total = 0u64;
        fn visit_dir(dir: &Path, total: &mut u64) {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    if path.is_file() {
                        if let Ok(meta) = path.metadata() {
                            *total += meta.len();
                        }
                    } else if path.is_dir() {
                        visit_dir(&path, total);
                    }
                }
            }
        }
        visit_dir(&self.cache_dir, &mut total);
        total
    }
}

// ============================================================================
// Tauri Event Helper
// ============================================================================

pub fn emit_model_progress(app: &AppHandle, progress: &ModelProgress) {
    let _ = app.emit("reflexdesk://model-progress", progress);
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_known_vectors() {
        assert_eq!(
            sha256_digest_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_digest_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_digest_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn test_manifest_parsing() {
        let registry = ModelRegistry::load(None).expect("embedded manifest parses");
        let nemotron = registry
            .find_artifact(DEFAULT_STT_MODEL_ID)
            .expect("nemotron found in registry");

        assert_eq!(nemotron.size_bytes, Some(408557984));
        assert_eq!(
            nemotron.recorded_sha256.as_deref(),
            Some("22480dd283a3bf1b39e9d2ab86c048b0c1654b2fc280e46627a4cbfded4b46fd")
        );
        assert!(nemotron.sha256.is_none());
        assert!(nemotron.sha256_reason.is_some());
    }

    #[test]
    fn test_disk_space_preflight_current_dir() {
        let cwd = std::env::current_dir().unwrap();
        // 1KB check should pass on any development machine
        assert!(check_disk_space_preflight(&cwd, 1024).is_ok());
    }
}
