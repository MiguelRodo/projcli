//! Read-only identity checks before managed directory deletion or replacement.
use std::{fs, path::{Component, Path, PathBuf}};
use anyhow::Context;
use camino::Utf8Path;

/// Resolves existing prefixes, optionally rejecting symlinks, without creating paths.
/// Missing suffixes are retained; filesystem errors other than absence propagate.
pub(crate) fn resolve_path(path: &Path, reject_symlinks: bool) -> anyhow::Result<PathBuf> {
    let absolute = if path.is_absolute() { path.to_owned() } else { std::env::current_dir()?.join(path) };
    let mut resolved = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::CurDir => continue,
            Component::ParentDir => { resolved.pop(); }
            Component::Prefix(_) | Component::RootDir => resolved.push(part.as_os_str()),
            Component::Normal(name) => {
                resolved.push(name);
                match fs::symlink_metadata(&resolved) {
                    Ok(metadata) => {
                        if reject_symlinks && metadata.file_type().is_symlink() {
                            anyhow::bail!("Managed write path contains a symlink: {}", resolved.display());
                        }
                        resolved = fs::canonicalize(&resolved)
                            .with_context(|| format!("Cannot resolve path {}", resolved.display()))?;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error).with_context(|| format!("Cannot inspect path {}", resolved.display())),
                }
            }
        }
    }
    Ok(resolved)
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

/// Rejects unsafe managed cleanup destinations before production analysis writes.
/// External resource paths are allowed when disjoint from protected inputs/state.
pub(crate) fn validate_build_paths(root: &Utf8Path, config: &crate::yml::ValidatedConfig, inputs: &[camino::Utf8PathBuf]) -> anyhow::Result<()> {
    let root_identity = resolve_path(root.as_std_path(), false)?;
    let staging = resolve_path(root.join("_tmp/projr").as_std_path(), true)?;
    let mut directories = Vec::new();
    for (label, directory) in &config.directories {
        let lower = label.to_lowercase();
        let writable = lower.starts_with("cache") || lower.starts_with("output") || lower.starts_with("docs");
        let path = if directory.path.is_absolute() { directory.path.clone() } else { root.join(&directory.path) };
        let identity = resolve_path(path.as_std_path(), writable)?;
        if writable {
            if root_identity.starts_with(&identity) {
                anyhow::bail!("Managed directory '{label}' must not be the project root or an ancestor");
            }
            for protected in [root_identity.join(".git"), root_identity.join(".svn")] {
                if overlaps(&identity, &protected) { anyhow::bail!("Managed directory '{label}' overlaps version-control metadata"); }
            }
            for input in inputs {
                if resolve_path(input.as_std_path(), false)?.starts_with(&identity) {
                    anyhow::bail!("Managed directory '{label}' contains selected script/hook {input}");
                }
            }
        }
        if !lower.starts_with("cache") && overlaps(&identity, &staging) {
            anyhow::bail!("Resource directory '{label}' overlaps build staging");
        }
        directories.push((label, identity, writable));
    }
    for (index, (left_label, left, left_writable)) in directories.iter().enumerate() {
        for (right_label, right, right_writable) in directories.iter().skip(index + 1) {
            if (*left_writable || *right_writable) && overlaps(left, right) {
                anyhow::bail!("Managed/resource directories overlap: '{left_label}' and '{right_label}'");
            }
        }
    }
    Ok(())
}

/// Requires a real, disjoint promotion source and safe destination before deletion.
pub(crate) fn validate_directory_move(source: &Utf8Path, destination: &Utf8Path) -> anyhow::Result<()> {
    if !source.is_dir() { anyhow::bail!("Promotion source is not an existing directory: {source}"); }
    let source_identity = resolve_path(source.as_std_path(), true)?;
    let destination_identity = resolve_path(destination.as_std_path(), true)?;
    if overlaps(&source_identity, &destination_identity) {
        anyhow::bail!("Promotion source and destination must be disjoint: {source}, {destination}");
    }
    if destination.exists() && !destination.is_dir() {
        anyhow::bail!("Promotion destination is not a directory: {destination}");
    }
    // Existing protected files survive cleanup, so reject links before copy can follow them.
    for name in ["CHANGELOG.md", ".gitignore", "README.md"] {
        resolve_path(destination.join(name).as_std_path(), true)?;
    }
    Ok(())
}
