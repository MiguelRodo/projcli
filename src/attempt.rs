//! Internal attempt identity and cooperative single-writer ownership.
use std::{fs, io::Write, path::PathBuf};
use anyhow::Context;

/// Owns the project claim until all build finalization and recovery have finished.
/// A crash leaves a claim: fail closed rather than guessing whether its owner died.
pub(crate) struct Attempt {
    directory: PathBuf,
    claim: PathBuf,
    bytes: Vec<u8>,
    created_directory: bool,
}

impl Attempt {
    /// Claims the writer exclusively; existing claims fail closed without mutation.
    pub(crate) fn acquire(root: &camino::Utf8Path) -> anyhow::Result<Self> {
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
            let claim = directory.join("build.lock");
            crate::path_safety::resolve_path(&claim, true)?;
            // Reuse tempfile's random, exclusive names; no hostname, username or PID.
            let mut candidate = tempfile::Builder::new().prefix("attempt-").rand_bytes(24)
                .tempfile_in(&directory).context("Cannot allocate internal attempt identity")?;
            let identity = candidate.path().file_name().and_then(|s| s.to_str())
                .context("Invalid internal attempt identity")?.to_owned();
            let bytes = serde_json::to_vec(&serde_json::json!({"attempt_id": identity}))?;
            candidate.write_all(&bytes)?;
            candidate.flush()?;
            candidate.persist_noclobber(&claim).map_err(|error| anyhow::anyhow!(
                "Cannot claim project build writer at {}: {}. Another build may be active; after a crash, remove only a confirmed stale claim.",
                claim.display(), error.error))?;
            Ok(Self { directory: directory.clone(), claim, bytes, created_directory })
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
