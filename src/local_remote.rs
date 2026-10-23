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

fn version_numbers(text: &str) -> Option<Vec<u64>> {
    let bare = text.strip_prefix('v').unwrap_or(text);
    let parts: Vec<_> = bare.split(['.', '-']).collect();
    if parts.len() < 2 || parts.iter().any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    parts.into_iter().map(|p| p.parse().ok()).collect()
}

fn compare_versions(left: &[u64], right: &[u64]) -> Ordering {
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
    anyhow::ensure!(!label.ends_with("-empty"), "Local remote labels must not end in reserved -empty");
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
