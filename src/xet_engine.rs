//! Optional subprocess boundary around one pinned upstream local-disk implementation.
//! Engine panics/nonzero exits never become successful reconstructions.
use anyhow::{Context, Result};
use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
/// Audited upstream revision; no implicit backend-format upgrades.
pub const UPSTREAM: &str = "629562cd9fc37432317eb57811faf13a6935c601";
/// Fixed local engine/store compatibility domain.
pub const BACKEND: &str = "xet-local-v1";

/// Upstream reconstruction address, separate from proj's trusted BLAKE3 file digest.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Pointer {
    /// Upstream Merkle identifier, not a whole-file BLAKE3.
    pub hash: String,
    /// Expected reconstructed byte length.
    pub file_size: u64,
    /// Upstream optional SHA-256; not accepted as proof of reconstructed bytes.
    pub sha256: Option<String>,
}
impl Pointer {
    pub(crate) fn validate(&self) -> Result<()> {
        crate::cas::validate_digest(&self.hash)?;
        if let Some(hash) = &self.sha256 {
            crate::cas::validate_digest(hash)?;
        }
        Ok(())
    }
}
/// Explicit optional engine executable; no auto-download, login or dependency installation.
#[derive(Clone)]
pub struct Engine {
    program: PathBuf,
}
impl Engine {
    /// Validate an explicitly selected helper and its exact protocol/upstream pin.
    /// This executes the trusted local program, not a sandbox.
    pub fn new(program: impl AsRef<Path>) -> Result<Self> {
        let result = Self {
            program: fs::canonicalize(program)
                .context("Cannot resolve optional proj-xet-engine executable")?,
        };
        let home = tempfile::tempdir()?;
        let work = Utf8Path::from_path(home.path()).context("Xet temporary path is not UTF-8")?;
        let mut command = result.command(work)?;
        command.arg("info");
        result.run(&mut command, work)?;
        let info: serde_json::Value = serde_json::from_slice(
            &crate::fs_utils::read_managed_bounded(&work.join("stdout"), 16384)?,
        )?;
        anyhow::ensure!(
            info["protocol"] == 1 && info["upstream"] == UPSTREAM && info["backend"] == BACKEND,
            "Unsupported Xet helper protocol/upstream; install the documented pinned optional engine"
        );
        Ok(result)
    }
    /// Resolve once from captured absolute PATH entries; never auto-install.
    pub(crate) fn discover(environment: &crate::run_environment::RunEnvironment) -> Result<Self> {
        let name = if cfg!(windows) {
            "proj-xet-engine.exe"
        } else {
            "proj-xet-engine"
        };
        for directory in std::env::split_paths(environment.value("PATH").unwrap_or_default()) {
            if directory.as_os_str().is_empty() || !directory.is_absolute() {
                continue;
            }
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Self::new(candidate);
            }
        }
        anyhow::bail!(
            "structure: xet requires the optional pinned proj-xet-engine on PATH; no automatic installation"
        );
    }
    fn command(&self, work: &Utf8Path) -> Result<Command> {
        let mut command = Command::new(&self.program);
        command.env_clear().current_dir(work).stdin(Stdio::null());
        // Runtime/platform variables only. Never forward credentials, HF tokens or caches.
        for key in [
            "PATH",
            "SYSTEMROOT",
            "WINDIR",
            "COMSPEC",
            "PATHEXT",
            "LD_LIBRARY_PATH",
            "DYLD_LIBRARY_PATH",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
            .env("HOME", work)
            .env("USERPROFILE", work)
            .env("TMP", work)
            .env("TEMP", work)
            .env("TMPDIR", work);
        Ok(command)
    }
    fn run(&self, command: &mut Command, work: &Utf8Path) -> Result<()> {
        let stdout = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(work.join("stdout"))?;
        let stderr = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(work.join("stderr"))?;
        let child = command
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .context("Cannot start optional Xet helper")?;
        let mut owned = OwnedChild(child);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
        let status = loop {
            if let Some(status) = owned.0.try_wait()? {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                // Only our directly spawned helper is terminated, never a process search/group.
                owned
                    .0
                    .kill()
                    .context("Cannot stop timed-out owned Xet helper")?;
                owned
                    .0
                    .wait()
                    .context("Cannot reap timed-out owned Xet helper")?;
                anyhow::bail!(
                    "Xet helper exceeded its ten-minute operation limit; no destination was installed"
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        // Child diagnostics may contain native paths. Keep them private and return only status.
        anyhow::ensure!(
            status.success(),
            "Xet helper failed or panicked (status {status}); no reconstructed bytes were installed"
        );
        Ok(())
    }
    fn operation(
        &self,
        mode: &str,
        store: &Utf8Path,
        input: &Utf8Path,
        output: &Utf8Path,
    ) -> Result<()> {
        let temp = tempfile::tempdir()?;
        let work = Utf8Path::from_path(temp.path()).context("Xet temporary path is not UTF-8")?;
        let receipt = work.join("receipt.json");
        let mut command = self.command(work)?;
        command.args([
            mode,
            store.as_str(),
            input.as_str(),
            output.as_str(),
            receipt.as_str(),
        ]);
        self.run(&mut command, work)?;
        let receipt: serde_json::Value =
            serde_json::from_slice(&crate::fs_utils::read_managed_bounded(&receipt, 16384)?)?;
        anyhow::ensure!(
            receipt["finalized"] == true,
            "Xet helper did not confirm finalization"
        );
        Ok(())
    }
    pub(crate) fn upload(&self, store: &Utf8Path, input: &Utf8Path) -> Result<Pointer> {
        let temp = tempfile::tempdir()?;
        let work = Utf8Path::from_path(temp.path()).context("Xet temporary path is not UTF-8")?;
        let output = work.join("pointer.json");
        self.operation("upload", store, input, &output)?;
        let pointer: Pointer =
            serde_json::from_slice(&crate::fs_utils::read_managed_bounded(&output, 16384)?)?;
        pointer.validate()?;
        Ok(pointer)
    }
    pub(crate) fn download(
        &self,
        store: &Utf8Path,
        pointer: &Pointer,
        output: &Utf8Path,
    ) -> Result<()> {
        pointer.validate()?;
        anyhow::ensure!(
            !crate::yml::configuration_entry_exists(output)?,
            "Xet reconstruction requires a fresh private output"
        );
        let temp = tempfile::tempdir()?;
        let work = Utf8Path::from_path(temp.path()).context("Xet temporary path is not UTF-8")?;
        let input = work.join("pointer.json");
        fs::write(&input, serde_json::to_vec(pointer)?)?;
        self.operation("download", store, &input, output)?;
        let metadata =
            fs::symlink_metadata(output).context("Xet helper did not reconstruct a file")?;
        anyhow::ensure!(
            metadata.is_file() && metadata.len() == pointer.file_size,
            "Xet reconstructed length/type differs"
        );
        Ok(())
    }
}

// Normal Rust unwinding owns/reaps the child before the caller's store claim can drop.
// Abrupt parent/system termination is not covered; the stale claim then fails closed.
struct OwnedChild(std::process::Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
