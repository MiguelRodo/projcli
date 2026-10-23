//! Projr-compatible build inventories, independent of CAS remote objects.
//! MD5 is retained for compatibility, not used as a security/authenticity proof.
use std::{collections::HashSet, fs, io::{Read, Write}};
use anyhow::Context;
use camino::Utf8Path;
use serde::{Deserialize, Serialize};

/// One historical projr inventory row. Empty labels use blank fn/hash strings;
/// versions include a leading v, and nonempty files use lowercase MD5 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ManifestRow {
    /// Explicit directory label.
    pub label: String,
    /// Slash-separated filename relative to that directory; blank for empty.
    pub r#fn: String,
    /// Build release version, including leading v.
    pub version: String,
    /// MD5 file-content digest; blank only with an empty filename.
    pub hash: String,
}

impl ManifestRow {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        crate::path_safety::validate_directory_label(&self.label)?;
        anyhow::ensure!(self.version.starts_with('v') && self.version.len() > 1
            && !self.version.chars().any(char::is_control), "Invalid manifest version");
        if self.r#fn.is_empty() {
            anyhow::ensure!(self.hash.is_empty(), "Empty manifest marker must have an empty hash");
        } else {
            anyhow::ensure!(!self.r#fn.contains(['\\', ':', '\0'])
                && self.r#fn.split('/').all(|part| !part.is_empty() && part != "." && part != ".."),
                "Unsafe manifest filename");
            anyhow::ensure!(self.hash.len() == 32 && self.hash.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "Invalid manifest MD5");
        }
        Ok(())
    }
}

/// Reads existing history, returning no rows only for an absent file.
/// Links, special files, malformed schemas and invalid rows fail closed.
pub fn read(path: &Utf8Path) -> anyhow::Result<Vec<ManifestRow>> {
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
        Ok(metadata) => {
            anyhow::ensure!(metadata.is_file(), "Manifest must be a regular file: {path}");
            crate::manifest_csv::decode(&fs::read(path)?)
                .with_context(|| format!("Cannot read manifest history {path}"))
        }
    }
}

/// Replaces a CSV using a same-directory temporary file and new inode, preserving
/// existing permissions and hard-link aliases. The parent must already exist.
/// Validates rows and destination guards before writing; errors preserve history.
pub fn write(path: &Utf8Path, rows: &[ManifestRow]) -> anyhow::Result<()> {
    let bytes = crate::manifest_csv::encode(rows)?;
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    let permissions = match fs::symlink_metadata(path) {
        Ok(metadata) => {
            anyhow::ensure!(metadata.is_file(), "Manifest must be a regular file: {path}");
            Some(metadata.permissions())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let parent = path.parent().context("Manifest has no parent directory")?;
    let mut staged = tempfile::Builder::new().prefix(".proj-manifest-").tempfile_in(parent)?;
    staged.write_all(&bytes)?;
    if let Some(permissions) = permissions { fs::set_permissions(staged.path(), permissions)?; }
    staged.flush()?;
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => anyhow::ensure!(metadata.is_file(), "Manifest destination changed: {path}"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(error) => return Err(error.into()),
    }
    staged.persist(path).map_err(|error| error.error)
        .with_context(|| format!("Cannot replace manifest {path}"))?;
    Ok(())
}

/// Retains first occurrences of exact rows in supplied order. Unlike concatenated
/// string keys, tuple equality cannot collapse distinct filenames/labels.
pub fn deduplicate(rows: impl IntoIterator<Item = ManifestRow>) -> Vec<ManifestRow> {
    let mut seen = HashSet::new();
    rows.into_iter().filter(|row| seen.insert(row.clone())).collect()
}

// Shared physical file inventory; no version/label or persistence policy.
pub(crate) fn inventory_directory(directory: &Utf8Path, cache: bool) -> anyhow::Result<Vec<(String, String)>> {
    fn visit(root: &Utf8Path, path: &Utf8Path, cache: bool, files: &mut Vec<(String, String)>) -> anyhow::Result<()> {
        crate::path_safety::resolve_path(path.as_std_path(), true)?;
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && path == root => return Ok(()),
            Err(error) => return Err(error).with_context(|| format!("Cannot inspect manifest source {path}")),
        };
        anyhow::ensure!(metadata.is_dir(), "Manifest source must be a real directory: {path}");
        for entry in fs::read_dir(path).with_context(|| format!("Cannot enumerate manifest source {path}"))? {
            let entry = entry.with_context(|| format!("Cannot inspect manifest entry in {path}"))?;
            let child = camino::Utf8PathBuf::try_from(entry.path()).context("Manifest filename must be UTF-8")?;
            let relative = child.strip_prefix(root)?.as_str().replace(std::path::MAIN_SEPARATOR, "/");
            if cache && relative.split('/').next() == Some("projr") { continue; }
            let ty = entry.file_type().with_context(|| format!("Cannot inspect manifest source type {child}"))?;
            if ty.is_dir() { visit(root, &child, cache, files)?; }
            else {
                anyhow::ensure!(ty.is_file(), "Manifest source must contain only real files/directories: {child}");
                crate::path_safety::resolve_path(child.as_std_path(), true)?;
                let mut file = fs::File::open(&child).with_context(|| format!("Cannot open manifest source {child}"))?;
                let mut digest = md5::Context::new();
                let mut buffer = [0u8; 65536];
                loop {
                    let n = file.read(&mut buffer).with_context(|| format!("Cannot hash manifest source {child}"))?;
                    if n == 0 { break; }
                    digest.consume(&buffer[..n]);
                }
                files.push((relative, format!("{:x}", digest.finalize())));
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    visit(directory, directory, cache, &mut files)?;
    files.sort();
    Ok(files)
}

/// Recursively inventories regular files using streaming MD5. Missing/empty
/// directories get an empty marker; inspection/read failures propagate.
/// Cache captures exclude the whole framework-owned projr subtree.
/// Links and special files are rejected; no file is executed or mutated.
pub fn capture_directory(label: &str, directory: &Utf8Path, version: &str, cache: bool) -> anyhow::Result<Vec<ManifestRow>> {
    let mut files = inventory_directory(directory, cache)?;
    if files.is_empty() { files.push((String::new(), String::new())); }
    files.into_iter().map(|(r#fn, hash)| {
        let row = ManifestRow { label: label.to_owned(), r#fn, version: version.to_owned(), hash };
        row.validate()?;
        Ok(row)
    }).collect()
}

pub(crate) fn capture_build(root: &Utf8Path, config: &crate::yml::ValidatedConfig, version: &str, pre: bool) -> anyhow::Result<Vec<ManifestRow>> {
    let mut labels: Vec<_> = config.directories.iter().collect();
    labels.sort_by_key(|(label, _)| *label);
    let mut rows = Vec::new();
    for (label, directory) in labels {
        let lower = label.to_ascii_lowercase();
        let cache = lower.starts_with("cache");
        if (lower.starts_with("raw") || cache) != pre || !directory.hash.unwrap_or(!cache) { continue; }
        rows.extend(capture_directory(label, &config.get_path(root, label)?, version, cache)?);
    }
    Ok(rows)
}
