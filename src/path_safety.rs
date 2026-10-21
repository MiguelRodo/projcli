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

/// Rejects labels that could escape a derived cache directory.
pub(crate) fn validate_directory_label(label: &str) -> anyhow::Result<()> {
    if label.is_empty() || label == "." || label == ".." || label.contains('/') || label.contains('\\')
        || label.contains(':') || label.ends_with('.') || label.ends_with(' ') || label.chars().any(char::is_control) {
        anyhow::bail!("Directory label must be a nonempty portable name without separators or traversal: {label}");
    }
    Ok(())
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

/// Rejects unsafe managed cleanup destinations before production analysis writes.
/// External resource paths are allowed when disjoint from protected inputs/state.
pub(crate) fn validate_build_paths(root: &Utf8Path, config: &crate::yml::ValidatedConfig, inputs: &[camino::Utf8PathBuf]) -> anyhow::Result<()> {
    let root_identity = resolve_path(root.as_std_path(), false)?;
    let staging = if config.directories.keys().any(|label| label.eq_ignore_ascii_case("cache")) {
        Some(resolve_path(config.get_path(root, "cache")?.join("projr").as_std_path(), true)?)
    } else { None };
    let mut directories = Vec::new();
    for (label, directory) in &config.directories {
        validate_directory_label(label)?;
        let lower = label.to_lowercase();
        let writable = lower.starts_with("cache") || crate::paths::is_managed_output(label);
        let path = if directory.path.is_absolute() { directory.path.clone() } else { root.join(&directory.path) };
        let identity = resolve_path(path.as_std_path(), writable)?;
        if writable {
            if path.exists() && !path.is_dir() {
                anyhow::bail!("Managed directory '{label}' is not a directory");
            }
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
        if let Some(staging) = &staging {
            if !lower.starts_with("cache") && overlaps(&identity, staging) {
                anyhow::bail!("Resource directory '{label}' overlaps build staging");
            }
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


/// Validates every derived cleanup base before the first cleanup mutation.
pub(crate) fn validate_cleanup_paths(root: &Utf8Path, config: &crate::yml::ValidatedConfig, version: &str) -> anyhow::Result<()> {
    validate_build_paths(root, config, &[])?;
    if version.contains('/') || version.contains('\\') || version == "." || version == ".." {
        anyhow::bail!("Invalid cache version component");
    }
    let cache = config.get_path(root, "cache")?;
    resolve_path(cache.join("projr").join(version).as_std_path(), true)?;
    resolve_path(cache.join("projr").join(version).join("docs").as_std_path(), true)?;
    for label in config.directories.keys() {
        if crate::paths::is_output_or_data(label) {
            resolve_path(cache.join("projr").join(version).join(label).as_std_path(), true)?;
        }
    }
    Ok(())
}

/// Validates a direct document-promotion destination before creating it.
pub(crate) fn validate_document_destination(root: &Utf8Path, input: &Utf8Path, destination: &Utf8Path) -> anyhow::Result<()> {
    let root = resolve_path(root.as_std_path(), false)?;
    let destination = resolve_path(destination.as_std_path(), true)?;
    if root.starts_with(&destination) || resolve_path(input.as_std_path(), false)?.starts_with(&destination) {
        anyhow::bail!("Document destination contains the project root or selected input");
    }
    for protected in [root.join(".git"), root.join(".svn")] {
        if overlaps(&destination, &protected) { anyhow::bail!("Document destination overlaps protected project state"); }
    }
    if destination.exists() && !destination.is_dir() { anyhow::bail!("Document destination is not a directory"); }
    Ok(())
}


/// Conservatively bounds project-render inputs instead of claiming exact renderer discovery.
/// Source-like documents in managed cleanup trees must be moved to separate source paths.
pub(crate) fn validate_project_source_overlap(root: &Utf8Path, config: &crate::yml::ValidatedConfig) -> anyhow::Result<()> {
    // ponytail: conservative O(tree) scan; a renderer input manifest can narrow rejections later.
    let mut pending = Vec::new();
    for label in config.directories.keys() {
        let lower = label.to_lowercase();
        if lower.starts_with("cache") || crate::paths::is_managed_output(label) {
            let path = config.get_path(root, label)?;
            if path.is_dir() { pending.push(path.into_std_path_buf()); }
        }
    }
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).with_context(|| format!("Cannot inspect project-render cleanup tree {}", directory.display()))? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_symlink() { anyhow::bail!("Cannot safely bound project-render inputs through linked cleanup-tree entry {}", path.display()); }
            if kind.is_dir() { pending.push(path); }
            else if path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "qmd" | "rmd" | "md" | "ipynb")) {
                anyhow::bail!("Project-render input/source-like document overlaps managed cleanup tree: {}. Keep project sources and managed outputs separate", path.display());
            }
        }
    }
    Ok(())
}
