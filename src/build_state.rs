//! Exact-byte backups for the explicitly owned local build state.
//! No filesystem atomicity, external-script rollback or concurrent-writer guarantee.
use anyhow::Context;
use std::{fs, path::{Path, PathBuf}};

pub(crate) struct BuildState {
    backup: tempfile::TempDir,
    entries: Vec<(PathBuf, Option<PathBuf>)>,
}

impl BuildState {
    pub(crate) fn capture(cache: Option<&Path>, paths: &[PathBuf]) -> anyhow::Result<Self> {
        // Disposable project cache is not a safe location for recovery bytes.
        let system_temp = fs::canonicalize(std::env::temp_dir()).context("Cannot resolve system temporary directory")?;
        let backup = tempfile::Builder::new().prefix("proj-build-recovery-").tempdir_in(system_temp)?;
        let recovery = fs::canonicalize(backup.path()).context("Cannot resolve private recovery storage")?;
        if let Some(cache) = cache {
            let cache = crate::path_safety::resolve_path(cache, true)?;
            if recovery.starts_with(&cache) || cache.starts_with(&recovery) {
                anyhow::bail!("Private recovery storage overlaps disposable project cache");
            }
        }
        let mut state = Self { backup, entries: Vec::new() };
        // Callers provide the concrete managed files/directories, not a whole-project copy.
        let mut selected: Vec<PathBuf> = paths.iter().map(|path| {
            let mut target = crate::path_safety::resolve_path(path, true)?;
            // Also remove parent directories created solely for a previously absent target.
            while !target.try_exists()? {
                let parent = target.parent().context("Managed target has no existing ancestor")?;
                if parent.try_exists()? { break; }
                target = parent.to_owned();
            }
            Ok(target)
        }).collect::<anyhow::Result<_>>()?;
        selected.sort();
        selected.dedup();
        let mut roots: Vec<PathBuf> = Vec::new();
        for target in selected {
            if roots.iter().any(|parent| target.starts_with(parent)) { continue; }
            if cache.is_some_and(|cache| cache.starts_with(&target) || target.starts_with(cache))
                || target.starts_with(&recovery) || recovery.starts_with(&target) {
                anyhow::bail!("Managed build backup overlaps cache: {}", target.display());
            }
            roots.push(target.clone());
            let saved = state.backup.path().join(state.entries.len().to_string());
            let original = match fs::symlink_metadata(&target) {
                Ok(_) => { copy_exact(&target, &saved)?; Some(saved) }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error).with_context(|| format!("Cannot inspect managed state {}", target.display())),
            };
            state.entries.push((target, original));
        }
        Ok(state)
    }

    /// Restore all entries even if one fails. Keep recovery bytes and both diagnostics.
    pub(crate) fn restore(self, primary: anyhow::Error) -> anyhow::Error {
        self.restore_result(Err(primary)).expect_err("A primary failure remains a failure")
    }

    /// Previews restore captured state after success as well as failure.
    pub(crate) fn restore_result(self, result: anyhow::Result<()>) -> anyhow::Result<()> {
        let mut failures = Vec::new();
        for (target, saved) in &self.entries {
            if let Err(error) = restore_entry(target, saved.as_deref()) {
                failures.push(format!("{}: {error:#}", target.display()));
            }
        }
        if failures.is_empty() { result }
        else {
            let diagnostic = if self.backup.path().is_dir() {
                let retained = self.backup.keep();
                format!("Managed-state restoration also failed: {}; recovery backup retained at {}", failures.join("; "), retained.display())
            } else {
                format!("Managed-state restoration also failed: {}; recovery backup is unavailable", failures.join("; "))
            };
            Err(match result {
                Err(primary) => primary.context(diagnostic),
                Ok(()) => anyhow::anyhow!(diagnostic),
            })
        }
    }

}

fn copy_exact(source: &Path, destination: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() { anyhow::bail!("Cannot back up linked managed state: {}", source.display()); }
    if metadata.is_file() { fs::copy(source, destination)?; }
    else if metadata.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_exact(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else { anyhow::bail!("Unsupported managed-state entry: {}", source.display()); }
    fs::set_permissions(destination, metadata.permissions())?;
    Ok(())
}

fn restore_entry(target: &Path, saved: Option<&Path>) -> anyhow::Result<()> {
    let parent = target.parent().context("Managed state has no parent")?;
    crate::path_safety::resolve_path(parent, true)?;
    // Prepare and validate all recovery bytes before removing the current target.
    // A missing/corrupt backup must never turn recovery into further data loss.
    let prepared = if let Some(saved) = saved {
        fs::create_dir_all(parent)?;
        let staging = tempfile::Builder::new().prefix(".proj-restore-").tempdir_in(parent)?;
        copy_exact(saved, &staging.path().join("entry"))?;
        Some(staging)
    } else { None };
    match fs::symlink_metadata(target) {
        Ok(metadata) => {
            // Refuse substituted links, including directory junctions, rather than following them.
            if metadata.file_type().is_symlink() { anyhow::bail!("Managed state was replaced with a symlink"); }
            if metadata.is_dir() { fs::remove_dir_all(target)?; }
            else if metadata.is_file() { fs::remove_file(target)?; }
            else { anyhow::bail!("Managed state was replaced with an unsupported entry"); }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if let Some(prepared) = prepared {
        fs::rename(prepared.path().join("entry"), target)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_preserves_bytes_absence_empty_directories_and_external_resources() {
        let project = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let docs = external.path().canonicalize().unwrap().join("published");
        fs::create_dir_all(docs.join("empty")).unwrap();
        fs::write(docs.join("report.html"), b"old\0bytes").unwrap();
        fs::write(root.join("_quarto.yml"), "# exact comments\nproject:\n  type: default\n").unwrap();
        let absent = root.join("new-parent/nested/result");
        let paths = vec![docs.clone(), root.join("VERSION"), root.join("_quarto.yml"), absent.clone()];
        let state = BuildState::capture(Some(&root.join("cache")), &paths).unwrap();
        fs::remove_dir_all(&docs).unwrap();
        fs::create_dir(&docs).unwrap();
        fs::write(docs.join("new.html"), "new").unwrap();
        fs::create_dir_all(absent.parent().unwrap()).unwrap();
        fs::write(&absent, "new").unwrap();
        fs::write(root.join("VERSION"), "v9.0.0").unwrap();
        fs::write(root.join("_quarto.yml"), "changed").unwrap();
        let error = state.restore(anyhow::anyhow!("render failed"));
        assert_eq!(error.to_string(), "render failed");
        assert_eq!(fs::read(docs.join("report.html")).unwrap(), b"old\0bytes");
        assert!(docs.join("empty").is_dir());
        assert!(!docs.join("new.html").exists());
        assert!(!root.join("VERSION").exists());
        assert!(!root.join("new-parent").exists());
        assert_eq!(fs::read_to_string(root.join("_quarto.yml")).unwrap(), "# exact comments\nproject:\n  type: default\n");
    }

    #[cfg(unix)]
    #[test]
    fn failed_restore_preserves_primary_error_backup_and_external_sentinel() {
        let project = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let output = root.join("output");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("previous"), "release").unwrap();
        let state = BuildState::capture(Some(&root.join("cache")), &[output.clone()]).unwrap();
        let backup_path = state.backup.path().to_owned();
        fs::remove_dir_all(&output).unwrap();
        fs::write(external.path().join("keep"), "external").unwrap();
        std::os::unix::fs::symlink(external.path(), &output).unwrap();
        let error = state.restore(anyhow::anyhow!("analysis exited 17"));
        let diagnostic = format!("{error:#}");
        assert!(diagnostic.contains("analysis exited 17"));
        assert!(diagnostic.contains("restoration also failed"));
        assert!(backup_path.is_dir());
        assert_eq!(fs::read_to_string(backup_path.join("0/previous")).unwrap(), "release");
        assert_eq!(fs::read_to_string(external.path().join("keep")).unwrap(), "external");
    }

    #[test]
    fn successful_preview_is_failure_if_recovery_is_unavailable_without_deleting_current_bytes() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let target = root.join("VERSION");
        fs::write(&target, "original version").unwrap();
        let state = BuildState::capture(None, &[target.clone()]).unwrap();
        fs::remove_dir_all(state.backup.path()).unwrap();
        let error = state.restore_result(Ok(())).unwrap_err();
        assert!(format!("{error:#}").contains("recovery backup is unavailable"));
        assert_eq!(fs::read_to_string(target).unwrap(), "original version");
    }

    #[test]
    fn unavailable_backup_does_not_delete_intact_current_state() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let target = root.join("VERSION");
        fs::write(&target, "v1.2.3\n").unwrap();
        let state = BuildState::capture(None, &[target.clone()]).unwrap();
        fs::remove_dir_all(state.backup.path()).unwrap();
        let error = state.restore(anyhow::anyhow!("analysis failed"));
        assert_eq!(fs::read_to_string(&target).unwrap(), "v1.2.3\n");
        let message = format!("{error:#}");
        assert!(message.contains("analysis failed"));
        assert!(message.contains("backup is unavailable"));
        assert!(!message.contains("backup retained"));
    }

}
