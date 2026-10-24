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
            Component::Prefix(_) => resolved.push(part.as_os_str()),
            Component::RootDir => {
                resolved.push(part.as_os_str());
                // Normalize the existing root even when every following component
                // is absent (Disk/VerbatimDisk and UNC share root on Windows).
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
            },
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

/// Conservatively rejects portable case aliases without excluding Unicode labels.
pub(crate) fn validate_directory_labels<'a>(labels: impl IntoIterator<Item = &'a str>) -> anyhow::Result<()> {
    let mut identities = std::collections::HashSet::new();
    for label in labels {
        validate_directory_label(label)?;
        anyhow::ensure!(identities.insert(label.to_uppercase()), "Directory labels collide under portable case comparison: {label}");
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
    let coordination = resolve_path(root.join(".proj").as_std_path(), true)?;
    if let Some(remotes) = &config.remotes.local {
        for remote in remotes.values() {
            let destination = resolve_path(remote.path.as_std_path(), true)?;
            anyhow::ensure!(!overlaps(&destination, &coordination),
                "CAS publication/recovery destination overlaps build coordination state");
        }
    }
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
            for protected in [root_identity.join(".git"), root_identity.join(".svn"), root_identity.join(".proj")] {
                if overlaps(&identity, &protected) { anyhow::bail!("Managed directory '{label}' overlaps protected project metadata"); }
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
        if crate::paths::is_generated_output(label) {
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
    for protected in [root.join(".git"), root.join(".svn"), root.join(".proj")] {
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


#[cfg(test)]
mod portable_label_tests {
    use super::validate_directory_labels;
    #[test]
    fn unicode_case_pairs_are_rejected_but_single_unicode_labels_remain_valid() {
        assert!(validate_directory_labels(["output-Ä", "output-ä"]).is_err());
        assert!(validate_directory_labels(["output-Ä", "data-é"]).is_ok());
        assert!(validate_directory_labels(["output-ä"]).is_ok());
    }
}

/// Rejects an existing native alias whose actual directory entry has another spelling.
pub(crate) fn validate_native_names(base: &Utf8Path, relative: &str, mirror: bool) -> anyhow::Result<()> {
    let mut current = base.to_owned();
    for component in Utf8Path::new(relative).iter() {
        let entries = match fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
                || (mirror && error.kind() == std::io::ErrorKind::NotADirectory) => return Ok(()),
            Err(error) => return Err(error).with_context(|| format!("Cannot inspect native store names at {current}")),
        };
        let mut exact = false;
        for entry in entries {
            let entry = entry.with_context(|| format!("Cannot read native store name at {current}"))?;
            exact |= entry.file_name() == std::ffi::OsStr::new(component);
        }
        current.push(component);
        if !exact {
            match fs::symlink_metadata(&current) {
                Ok(_) => {
                    // An immutable publisher can install the exact requested entry
                    // between the first enumeration and metadata lookup. Recheck
                    // spelling once; an alias still has no exact directory entry.
                    let parent=current.parent().context("Native store entry has no parent")?;
                    let mut appeared=false;
                    for entry in fs::read_dir(parent).with_context(||format!("Cannot recheck native store names at {parent}"))? {
                        appeared |= entry.with_context(||format!("Cannot reread native store name at {parent}"))?
                            .file_name()==std::ffi::OsStr::new(component);
                    }
                    anyhow::ensure!(appeared,"Ambiguous native filesystem alias in store destination: {current}");
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error).with_context(|| format!("Cannot inspect native store destination {current}")),
            }
        }
    }
    Ok(())
}


/// Validate a stored relative path without filesystem reads; shared by archives and manifests.
pub(crate) fn portable_relative(name:&str)->anyhow::Result<camino::Utf8PathBuf> {
    anyhow::ensure!(!name.is_empty() && !name.starts_with('/') && !name.contains('\\'),
        "Stored path must be a relative portable path");
    let name=name.strip_suffix('/').unwrap_or(name);
    for part in name.split('/') {
        crate::path_safety::validate_directory_label(part)?;
        let stem=part.split('.').next().unwrap().to_ascii_uppercase();
        anyhow::ensure!(!matches!(stem.as_str(),"CON"|"PRN"|"AUX"|"NUL"|"CONIN$"|"CONOUT$"|"CLOCK$")
            && !(stem.len()==4 && (stem.starts_with("COM")||stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3],b'1'..=b'9'))
            && !["COM","LPT"].iter().any(|prefix| matches!(stem.strip_prefix(prefix),Some("\u{b9}"|"\u{b2}"|"\u{b3}"))),
            "Stored path contains a reserved native device name");
    }
    Ok(name.into())
}

/// NFC/case comparison key for planned paths only; never use this spelling for I/O.
pub(crate) fn portable_path_key(path:&Utf8Path)->camino::Utf8PathBuf {
    camino::Utf8PathBuf::from(icu_normalizer::ComposingNormalizer::new_nfc()
        .normalize(&path.as_str().to_uppercase()).into_owned())
}

/// Conservative namespace overlap, including absent portable case/normalization aliases.
pub(crate) fn portable_paths_overlap(left:&Utf8Path,right:&Utf8Path)->bool {
    let left=portable_path_key(left);let right=portable_path_key(right);
    left.starts_with(&right)||right.starts_with(&left)
}
