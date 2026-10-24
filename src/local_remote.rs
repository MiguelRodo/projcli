//! Read-only inspection of ordinary projr local remote versions; independent of CAS.
use std::{cmp::Ordering, fs};
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

/// Ordinary local layouts; CAS remains a separate storage structure.
#[derive(Debug, Clone, Copy)]
pub enum LocalStructure {
    /// One label directory, with the selected version recorded in root VERSION.
    Latest,
    /// Numeric version directories below the label directory.
    Archive,
}

/// One discovered version. Listing does not verify file bytes or completeness.
#[derive(Debug, Serialize)]
pub struct LocalVersion {
    /// Leading-v numeric version, with dot-separated components.
    pub version: String,
    /// Actual directory, including any legacy empty suffix.
    pub path: Utf8PathBuf,
    /// Whether this is the remote's empty-directory marker variant.
    pub empty: bool,
    /// Whether the advertised latest target currently exists as a real directory.
    pub present: bool,
    /// Latest VERSION's trailing-asterisk marker; absence is not proof of trust.
    /// Archive listings do not infer trust and return None.
    pub metadata_untrusted: Option<bool>,
}

pub(crate) fn version_numbers(text: &str) -> Option<Vec<u64>> {
    let bare = text.strip_prefix('v').unwrap_or(text);
    let parts: Vec<_> = bare.split(['.', '-']).collect();
    if parts.len() < 2 || parts.iter().any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    parts.into_iter().map(|p| p.parse().ok()).collect()
}

pub(crate) fn compare_versions(left: &[u64], right: &[u64]) -> Ordering {
    (0..left.len().max(right.len())).map(|i|
        left.get(i).copied().unwrap_or(0).cmp(&right.get(i).copied().unwrap_or(0)))
        .find(|order| *order != Ordering::Equal).unwrap_or(Ordering::Equal)
}

fn directory_present(path: &Utf8Path) -> anyhow::Result<bool> {
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            anyhow::ensure!(metadata.is_dir(), "Local remote target must be a real directory: {path}");
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("Cannot inspect local remote target {path}")),
    }
}

/// Lists ordinary local versions without creating directories, modifying files,
/// loading credentials or contacting a service. Relative bases must be anchored
/// by the caller; CLI paths are anchored at its selected project root.
/// Archive results are newest-first by numeric components (not lexical order),
/// including empty variants. Latest reads an exact label entry in root VERSION
/// and discloses missing targets and the untrusted marker separately.
/// Rejects traversal labels, linked/special candidates, malformed advertised
/// versions and duplicate latest label entries. Unrelated archive names are
/// ignored; numeric-looking malformed version names fail. This is inspection,
/// not byte verification, source selection, restore or publication.
///
/// ```
/// use camino::Utf8Path;
/// use proj::local_remote::{list_versions, LocalStructure};
/// let parent = tempfile::tempdir()?;
/// let missing = parent.path().canonicalize()?.join("missing");
/// let versions = list_versions(Utf8Path::from_path(&missing).unwrap(),
///     "raw-data", LocalStructure::Archive, true)?;
/// assert!(versions.is_empty());
/// assert!(!missing.exists());
/// # Ok::<(), anyhow::Error>(())
/// ```
pub fn list_versions(base: &Utf8Path, label: &str, structure: LocalStructure, append_label: bool) -> anyhow::Result<Vec<LocalVersion>> {
    crate::path_safety::validate_directory_label(label)?;
    if !directory_present(base)? { return Ok(Vec::new()); }
    let target = if append_label { base.join(label) } else { base.to_owned() };
    match structure {
        LocalStructure::Latest => {
            let metadata_path = base.join("VERSION");
            crate::path_safety::resolve_path(metadata_path.as_std_path(), true)?;
            match fs::symlink_metadata(&metadata_path) {
                Ok(metadata) => anyhow::ensure!(metadata.is_file(), "Remote VERSION must be a regular file: {metadata_path}"),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(error) => return Err(error).with_context(|| format!("Cannot inspect remote VERSION {metadata_path}")),
            }
            let text = crate::fs_utils::read_managed_text(&metadata_path)?;
            let matches: Vec<_> = text.lines().filter_map(|line| line.split_once(':'))
                .filter(|(key, _)| key.trim() == label).collect();
            anyhow::ensure!(matches.len() <= 1, "Duplicate remote VERSION entries for label {label}");
            let Some((_, value)) = matches.first() else { return Ok(Vec::new()); };
            let value = value.trim();
            let untrusted = value.ends_with('*');
            let value = value.strip_suffix('*').unwrap_or(value);
            let numbers = version_numbers(value).context("Invalid advertised local remote version")?;
            let version = format!("v{}", numbers.iter().map(u64::to_string).collect::<Vec<_>>().join("."));
            let full = directory_present(&target)?;
            let empty_path = if append_label { base.join(format!("{label}-empty")) }
                else { Utf8PathBuf::from(format!("{target}-empty")) };
            let empty = !full && directory_present(&empty_path)?;
            Ok(vec![LocalVersion { version, path: if empty { empty_path } else { target },
                empty, present: full || empty, metadata_untrusted: Some(untrusted) }])
        }
        LocalStructure::Archive => {
            if !directory_present(&target)? { return Ok(Vec::new()); }
            let mut found = Vec::new();
            for entry in fs::read_dir(&target).with_context(|| format!("Cannot list local remote versions at {target}"))? {
                let entry = entry.with_context(|| format!("Cannot read local remote version entry at {target}"))?;
                let path = Utf8PathBuf::try_from(entry.path()).context("Local version filename must be UTF-8")?;
                let name = path.file_name().context("Local version entry has no filename")?;
                let empty = name.ends_with("-empty");
                let name = name.strip_suffix("-empty").unwrap_or(name);
                let bare = name.strip_prefix('v').unwrap_or(name);
                if !bare.as_bytes().first().is_some_and(u8::is_ascii_digit) { continue; }
                let numbers = version_numbers(name).with_context(|| format!("Invalid local archive version name: {path}"))?;
                anyhow::ensure!(directory_present(&path)?, "Local archive version disappeared: {path}");
                let version = format!("v{}", numbers.iter().map(u64::to_string).collect::<Vec<_>>().join("."));
                found.push((numbers, LocalVersion { version, path, empty, present: true, metadata_untrusted: None }));
            }
            found.sort_by(|(left, a), (right, b)| compare_versions(right, left)
                .then(a.empty.cmp(&b.empty)).then(a.path.cmp(&b.path)));
            Ok(found.into_iter().map(|(_, entry)| entry).collect())
        }
    }
}


/// Resolves one explicit ordinary local source without creating it.
/// Latest requires no archive version and uses its advertised VERSION entry.
/// Archive requires the exact numeric directory spelling (with optional leading
/// v); a full directory wins over its -empty variant. No automatic historical
/// version selection, byte verification or configured-source lookup is implied.
pub fn select_version(base: &Utf8Path, label: &str, structure: LocalStructure, archive_version: Option<&str>, append_label: bool) -> anyhow::Result<LocalVersion> {
    crate::path_safety::validate_directory_label(label)?;
    match structure {
        LocalStructure::Latest => {
            anyhow::ensure!(archive_version.is_none(), "Latest layout does not accept an archive version");
            let entry = list_versions(base, label, structure, append_label)?.into_iter().next()
                .context("Latest remote has no advertised version for the requested label")?;
            anyhow::ensure!(entry.present, "Advertised latest remote target is missing: {}", entry.path);
            Ok(entry)
        }
        LocalStructure::Archive => {
            let spelling = archive_version.context("Archive download requires an explicit remote version")?;
            let numbers = version_numbers(spelling).context("Invalid explicit archive version")?;
            anyhow::ensure!(directory_present(base)?, "Local remote base is missing: {base}");
            let target = if append_label { base.join(label) } else { base.to_owned() };
            let full = target.join(spelling);
            let (path, empty) = if directory_present(&full)? { (full, false) } else {
                let marker = target.join(format!("{spelling}-empty"));
                anyhow::ensure!(directory_present(&marker)?, "Requested local archive version is missing: {spelling}");
                (marker, true)
            };
            Ok(LocalVersion { version: format!("v{}", numbers.iter().map(u64::to_string).collect::<Vec<_>>().join(".")),
                path, empty, present: true, metadata_untrusted: None })
        }
    }
}

/// Outcome of a completed local download, independent of remote integrity trust.
#[derive(Debug, Serialize)]
pub struct LocalDownload {
    /// Normalized advertised/selected version, not a verified content identity.
    pub version: String,
    /// Explicit destination receiving the staged tree.
    pub destination: Utf8PathBuf,
    /// Whether the source was the ordinary empty-directory marker variant.
    pub empty: bool,
    /// Whether destination-only files were explicitly discarded.
    pub clear: bool,
}

pub(crate) fn validate_download_destination(root: &Utf8Path, source: &Utf8Path, destination: &Utf8Path) -> anyhow::Result<()> {
    crate::path_safety::validate_document_destination(root, source, destination)?;
    crate::path_safety::validate_directory_move(source, destination)?;
    let identity = crate::path_safety::resolve_path(destination.as_std_path(), true)?;
    let state = crate::path_safety::resolve_path(root.join(".projr").as_std_path(), true)?;
    anyhow::ensure!(!identity.starts_with(&state) && !state.starts_with(&identity),
        "Download destination overlaps protected project state: {destination}");
    Ok(())
}


pub(crate) fn download_permissions(path: &Utf8Path, relative: &Utf8Path, permissions: &mut std::collections::BTreeMap<Utf8PathBuf, fs::Permissions>) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path).with_context(|| format!("Cannot capture download permissions at {path}"))?;
    anyhow::ensure!(metadata.is_dir() || metadata.is_file(), "Download permissions require a real file/directory: {path}");
    permissions.insert(relative.to_owned(), metadata.permissions());
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            let path = Utf8PathBuf::try_from(entry?.path())?;
            download_permissions(&path, &relative.join(path.file_name().context("Download entry has no filename")?), permissions)?;
        }
    }
    Ok(())
}

// Existing staged files may be read-only. Remove matching private-stage files
// before the source copy; final captured permissions are applied after merging.
fn prepare_download_overlay(source: &Utf8Path, staged: &Utf8Path) -> anyhow::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = Utf8PathBuf::try_from(entry.path())?;
        let target = staged.join(path.file_name().context("Download entry has no filename")?);
        let kind = entry.file_type()?;
        if kind.is_dir() && target.is_dir() { prepare_download_overlay(&path, &target)?; }
        else if kind.is_file() {
            match fs::symlink_metadata(&target) {
                Ok(metadata) if metadata.is_file() => {
                    #[cfg(windows)]
                    if metadata.permissions().readonly() {
                        let mut permissions = metadata.permissions();
                        permissions.set_readonly(false);
                        fs::set_permissions(&target, permissions)?;
                    }
                    fs::remove_file(&target)?;
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

/// Downloads an already selected ordinary local version through private staging.
/// The destination parent must already exist. Existing destination-only files
/// survive unless clear is true; matching files receive source bytes. Empty
/// markers must contain no entries. Linked/special trees and overlapping paths,
/// project-root/ancestor and VCS/framework-state destinations fail closed.
///
/// All copying finishes before renaming an existing destination aside. A failed
/// install attempts to restore that original directory; a failed rollback retains
/// it at the reported private path. Hard-linked external aliases are not edited.
/// This is bounded recovery, not crash atomicity or concurrent-writer isolation.
/// Existing destination root permissions survive even with clear; merge mode
/// preserves existing file/directory permissions, and new entries retain source
/// permission flags. Ownership, ACLs and extended attributes are not preserved.
/// No source data, remote metadata, project version or configured paths change.
pub fn download_version(root: &Utf8Path, version: &LocalVersion, destination: &Utf8Path, clear: bool) -> anyhow::Result<LocalDownload> {
    anyhow::ensure!(version.present && directory_present(&version.path)?, "Selected local remote directory is missing");
    if version.empty {
        anyhow::ensure!(fs::read_dir(&version.path)?.next().transpose()?.is_none(),
            "Empty remote marker contains entries: {}", version.path);
    }
    install_directory(root, &version.path, destination, clear)?;
    Ok(LocalDownload { version: version.version.clone(), destination: destination.to_owned(), empty: version.empty, clear })
}

/// Shared guarded tree installation; callers provide a complete staged source.
pub(crate) fn install_directory(root: &Utf8Path, source: &Utf8Path, destination: &Utf8Path, clear: bool) -> anyhow::Result<()> {
    install_directory_checked(root,source,destination,clear,|_|Ok(()))
}

/// Complete private-stage verification before retaining or replacing the destination.
pub(crate) fn install_directory_checked(root:&Utf8Path,source:&Utf8Path,destination:&Utf8Path,clear:bool,
    verify:impl FnOnce(&Utf8Path)->anyhow::Result<()>)->anyhow::Result<()> {
    validate_download_destination(root, source, destination)?;
    let parent = destination.parent().context("Download destination has no parent")?;
    anyhow::ensure!(directory_present(parent)?, "Download destination parent must already exist: {parent}");
    let had_destination = directory_present(destination)?;
    let staging = tempfile::Builder::new().prefix(".proj-download-").tempdir_in(parent)?;
    let staged = Utf8PathBuf::try_from(staging.path().join("next"))?;
    let previous = Utf8PathBuf::try_from(staging.path().join("previous"))?;
    fs::create_dir(&staged)?;
    let mut permissions = std::collections::BTreeMap::new();
    download_permissions(source, Utf8Path::new(""), &mut permissions)?;
    if had_destination {
        if clear { permissions.insert(Utf8PathBuf::new(), fs::symlink_metadata(destination)?.permissions()); }
        else {
            download_permissions(destination, Utf8Path::new(""), &mut permissions)?;
            crate::fs_utils::copy_dir_recursive(destination, &staged)?;
        }
    }
    prepare_download_overlay(source, &staged)?;
    crate::fs_utils::copy_dir_recursive(source, &staged)?;
    // Children first: restrictive parent modes must not block metadata updates.
    let mut permissions: Vec<_> = permissions.into_iter().collect();
    permissions.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    for (relative, mode) in permissions { fs::set_permissions(staged.join(relative), mode)?; }
    verify(&staged)?;
    validate_download_destination(root, source, destination)?;
    anyhow::ensure!(directory_present(destination)? == had_destination, "Download destination presence changed during staging");
    if had_destination {
        fs::rename(destination, &previous).with_context(|| format!("Cannot retain existing download destination {destination}"))?;
    }
    if let Err(error) = fs::rename(&staged, destination) {
        if had_destination {
            if let Err(rollback) = fs::rename(&previous, destination) {
                let retained = staging.keep();
                anyhow::bail!("Cannot install local download: {error}; rollback failed: {rollback}; original destination retained at {}/previous", retained.display());
            }
        }
        return Err(error).with_context(|| format!("Cannot install staged local download at {destination}"));
    }
    let cleanup_path = staging.path().to_owned();
    staging.close().with_context(|| format!("Download installed at {destination}, but private staging cleanup failed at {}", cleanup_path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numeric_versions_follow_package_comparison_without_fixed_component_count() {
        assert_eq!(compare_versions(&version_numbers("v1.10.0").unwrap(), &version_numbers("v1.9.9").unwrap()), Ordering::Greater);
        assert_eq!(compare_versions(&version_numbers("1.2").unwrap(), &version_numbers("v1.2.0").unwrap()), Ordering::Equal);
        assert_eq!(version_numbers("v1.2.3-4.5"), Some(vec![1, 2, 3, 4, 5]));
        for bad in ["v1", "v1..2", "x1.2", "v1.2*", "../1.2", "v1.2-empty"] {
            assert!(version_numbers(bad).is_none(), "{bad}");
        }
    }
}
