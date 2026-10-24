//! Internal attempt identity and cooperative single-writer ownership.
use std::{fs, io::Write, path::PathBuf};
use anyhow::Context;

/// Owns a cooperative claim for its coordination root (project or mutable remote store).
/// Project callers retain ownership through build finalization/recovery; flat CAS callers
/// retain ownership through authoritative history reads and snapshot publication.
/// A crash leaves a claim: fail closed rather than guessing whether its owner died.
pub(crate) struct Attempt {
    id: String,
    started_at: String,
    directory: PathBuf,
    claim: PathBuf,
    bytes: Vec<u8>,
    created_directory: bool,
}

impl Attempt {
    /// Opaque attempt identity with no user or host information.
    pub(crate) fn id(&self) -> &str { &self.id }
    /// UTC wall-clock time at ownership acquisition.
    pub(crate) fn started_at(&self) -> &str { &self.started_at }

    /// Claims the writer exclusively; existing claims fail closed without mutation.
    pub(crate) fn acquire(root: &camino::Utf8Path) -> anyhow::Result<Self> {
        crate::path_safety::validate_native_names(root, ".proj/build.lock", false)?;
        let directory = root.join(".proj").into_std_path_buf();
        crate::path_safety::resolve_path(&directory, true)?;
        let created_directory = match fs::create_dir(&directory) {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                anyhow::ensure!(directory.is_dir(), "Build coordination path is not a directory");
                false
            }
            Err(error) => return Err(error).context("Cannot create build coordination directory"),
        };
        let result = (|| {
            crate::path_safety::validate_native_names(root, ".proj/build.lock", false)?;
            let claim = directory.join("build.lock");
            crate::path_safety::resolve_path(&claim, true)?;
            let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&claim)
                .with_context(|| format!("Cannot claim project build writer at {}. Another build may be active; after a crash, remove only a confirmed stale claim", claim.display()))?;
            // Establish ownership before allocating identity resources: rejected
            // competitors never expose temporary files to the owner's Git staging.
            let mut owned = Self { id: String::new(), started_at: chrono::Utc::now().to_rfc3339(), directory: directory.clone(), claim, bytes: Vec::new(), created_directory };
            let identity_file = tempfile::Builder::new().prefix("attempt-").rand_bytes(24)
                .tempfile().context("Cannot allocate internal attempt identity")?;
            let identity = identity_file.path().file_name().and_then(|s| s.to_str())
                .context("Invalid internal attempt identity")?.to_owned();
            let bytes = serde_json::to_vec(&serde_json::json!({"attempt_id": &identity}))?;
            // Empty ownership is retained until write succeeds; on an I/O failure
            // close before cleanup on Windows and remove only the bytes we wrote.
            let write = file.write_all(&bytes).and_then(|_| file.flush());
            drop(file);
            match write {
                Ok(()) => { owned.bytes = bytes; owned.id = identity; },
                Err(error) => {
                    if let Ok(bytes) = fs::read(&owned.claim) { owned.bytes = bytes; }
                    return Err(error).context("Cannot write build attempt identity");
                }
            }
            Ok(owned)
        })();
        if result.is_err() && created_directory { let _ = fs::remove_dir(&directory); }
        result
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        // Arbitrary scripts are not a sandbox. Never remove a replaced/linked claim.
        if crate::path_safety::resolve_path(&self.claim, true).is_ok()
            && fs::symlink_metadata(&self.claim).is_ok_and(|metadata| metadata.is_file())
            && fs::read(&self.claim).is_ok_and(|bytes| bytes == self.bytes) {
            if let Err(error) = fs::remove_file(&self.claim) {
                log::warn!("Cannot release build writer claim {}: {error}", self.claim.display());
            }
        }
        if self.created_directory { let _ = fs::remove_dir(&self.directory); }
    }
}
