use serde::{Serialize, Deserialize};

/// Represents a single file within a hashed directory structure.
///
/// This entry contains the file's relative path and its BLAKE3 hash.
///
/// # Examples
///
/// ```rust
/// use proj::cas::FileEntry;
///
/// let entry = FileEntry {
///     path: "data/file.txt".to_string(),
///     hash: "a1b2c3d4e5f6...".to_string(),
/// };
/// assert_eq!(entry.path, "data/file.txt");
/// ```
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileEntry {
    /// Path relative to the labeled directory root
    pub path: String,
    /// BLAKE3 hash of file contents
    pub hash: String,
}

/// Represents the hashed structural layout of a directory.
///
/// Includes the label, version, and a sorted list of all recursive file entries.
///
/// # Examples
///
/// ```rust
/// use proj::cas::{DirectoryManifest, FileEntry};
///
/// let manifest = DirectoryManifest {
///     label: "raw".to_string(),
///     version: "1.0.0".to_string(),
///     files: vec![
///         FileEntry {
///             path: "data/a.txt".to_string(),
///             hash: "hashA".to_string(),
///         },
///         FileEntry {
///             path: "data/b.txt".to_string(),
///             hash: "hashB".to_string(),
///         },
///     ],
/// };
/// assert_eq!(manifest.files.len(), 2);
/// ```
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct DirectoryManifest {
    /// The directory label (e.g. "raw_data")
    pub label: String,
    /// The version string associated with this ingestion
    pub version: String,
    /// Sorted list of file entries
    pub files: Vec<FileEntry>,
}

use camino::Utf8Path;
use anyhow::Result;
use std::fs::{self, File};
use std::io::{self, Write, Read};
use chrono::Utc;
use anyhow::Context;
use tempfile::NamedTempFile;

/// Computes the BLAKE3 hash of a file's contents.
///
/// # Arguments
/// * `path` - The path to the file to hash.
///
/// # Returns
/// A hexadecimal string of the BLAKE3 hash.
///
/// # Errors
/// Returns an `io::Error` if the file cannot be opened or read.
///
/// # Examples
///
/// ```rust
/// use std::io::Write;
/// use tempfile::NamedTempFile;
/// use proj::cas::hash_file;
///
/// let mut file = NamedTempFile::new().unwrap();
/// write!(file, "hello world").unwrap();
/// let path = camino::Utf8Path::from_path(file.path()).unwrap();
/// let hash = hash_file(path).unwrap();
/// // "hello world" BLAKE3 hash: d74981efa70a0c880b8d8c1985d075dbcbf679b99a5f9914e5aaf96b831a9e24
/// assert_eq!(hash, "d74981efa70a0c880b8d8c1985d075dbcbf679b99a5f9914e5aaf96b831a9e24");
/// ```
pub fn hash_file(path: &Utf8Path) -> Result<String> {
    let mut file = File::open(path.as_std_path()).context("Failed to open file for hashing")?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0; 65536]; // 64 KB buffer
    loop {
        let n = file.read(&mut buffer).context("Failed to read file for hashing")?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Ingests a directory into the CAS remote storage and logs it to the project ledger.
///
/// # Arguments
/// * `project_root` - The root path of the active workspace.
/// * `cas_remote_root` - The target remote CAS storage directory.
/// * `label` - The label of the directory being ingested (e.g., "raw").
/// * `source_dir` - The path to the directory to ingest.
/// * `version` - The version string for this ingestion.
///
/// # Errors
/// Returns an `io::Error` if files cannot be read/written, or directories cannot be created.

use crate::yml::{ValidatedConfig, InspectStrategy};

/// Verifies the structural integrity of a configured remote.
///
/// # Arguments
/// * `remote_title` - The name of the remote to inspect.
/// * `validated` - The validated configuration containing remote data.
///
/// # Errors
/// Returns an `io::Error` if the remote isn't found, manifest lacks files, or an integrity constraint fails.
fn validate_digest(digest: &str) -> Result<()> {
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) {
        anyhow::bail!("CAS digest must be 64 lowercase hexadecimal ASCII characters");
    }
    Ok(())
}

fn validate_logical_path(path: &str) -> Result<()> {
    if path.is_empty() || path.contains(['\\', ':', '\0'])
        || path.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
        anyhow::bail!("Ambiguous or unsafe CAS logical path");
    }
    Ok(())
}

fn object_path(objects: &Utf8Path, digest: &str) -> Result<camino::Utf8PathBuf> {
    validate_digest(digest)?;
    Ok(objects.join(&digest[..2]).join(&digest[2..]))
}

fn checked_directory(path: &Utf8Path) -> Result<()> {
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    if path.exists() && !path.is_dir() { anyhow::bail!("CAS directory path is not a directory: {path}"); }
    Ok(())
}

fn validate_manifest(manifest: &DirectoryManifest) -> Result<()> {
    if manifest.label.is_empty() || manifest.version.is_empty() || manifest.label.contains('\0') || manifest.version.contains('\0') {
        anyhow::bail!("CAS label/version must be nonempty and contain no NUL");
    }
    let mut seen = std::collections::HashSet::<&str>::new();
    for entry in &manifest.files {
        validate_logical_path(&entry.path)?;
        validate_digest(&entry.hash)?;
        if !seen.insert(entry.path.as_str()) { anyhow::bail!("Duplicate CAS logical path"); }
    }
    for entry in &manifest.files {
        for (index, character) in entry.path.char_indices() {
            if character == '/' && seen.contains(&entry.path[..index]) {
                anyhow::bail!("CAS manifest contains a file/directory path collision");
            }
        }
    }
    Ok(())
}

pub fn verify_remote_integrity(remote_title: &str, validated: &ValidatedConfig) -> Result<()> {
    let remotes = validated.remotes.local.as_ref().context("No remotes configured")?;
    let remote = remotes.get(remote_title).context("Configured CAS remote not found")?;
    checked_directory(&remote.path)?;
    if !remote.path.is_dir() { anyhow::bail!("CAS remote directory does not exist"); }
    let manifests = remote.path.join("manifests");
    let objects = remote.path.join("objects");
    checked_directory(&manifests)?;
    checked_directory(&objects)?;
    if !manifests.exists() { return Ok(()); }
    for entry in fs::read_dir(&manifests)? {
        let path = camino::Utf8PathBuf::try_from(entry?.path()).context("Non-UTF-8 CAS manifest path")?;
        if path.extension() != Some("json") { continue; }
        crate::path_safety::resolve_path(path.as_std_path(), true)?;
        let digest = path.file_stem().context("CAS manifest has no digest filename")?;
        validate_digest(digest)?;
        let bytes = fs::read(&path)?;
        if blake3::hash(&bytes).to_hex().as_str() != digest { anyhow::bail!("CAS manifest digest does not match its bytes"); }
        let manifest: DirectoryManifest = serde_json::from_slice(&bytes).context("Invalid CAS manifest JSON")?;
        validate_manifest(&manifest)?;
        if remote.inspect == InspectStrategy::File {
            for entry in &manifest.files {
                let object = object_path(&objects, &entry.hash)?;
                crate::path_safety::resolve_path(object.as_std_path(), true)?;
                if !object.is_file() || hash_file(&object)? != entry.hash {
                    anyhow::bail!("CAS object is missing or its bytes do not match the digest");
                }
            }
        }
    }
    Ok(())
}

/// Copy and hash the same stream: never re-open a source under a previously computed digest.
fn store_object(reader: &mut impl Read, objects: &Utf8Path) -> Result<String> {
    checked_directory(objects)?;
    fs::create_dir_all(objects)?;
    let mut staged = NamedTempFile::new_in(objects)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0; 65536];
    loop {
        let count = reader.read(&mut buffer).context("Cannot read CAS source stream")?;
        if count == 0 { break; }
        staged.write_all(&buffer[..count]).context("Cannot stage CAS object bytes")?;
        hasher.update(&buffer[..count]);
    }
    staged.flush()?;
    staged.as_file().sync_all()?;
    let digest = hasher.finalize().to_hex().to_string();
    let target = object_path(objects, &digest)?;
    let parent = target.parent().context("CAS object has no prefix directory")?;
    checked_directory(parent)?;
    fs::create_dir_all(parent)?;
    crate::path_safety::resolve_path(target.as_std_path(), true)?;
    if target.exists() {
        if !target.is_file() || hash_file(&target)? != digest { anyhow::bail!("Existing CAS object is corrupt"); }
        return Ok(digest);
    }
    match staged.persist_noclobber(&target) {
        Ok(_) => {}
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
            crate::path_safety::resolve_path(target.as_std_path(), true)?;
            if !target.is_file() || hash_file(&target)? != digest { anyhow::bail!("Conflicting CAS object publication"); }
        }
        Err(error) => return Err(error.error).context("Cannot publish CAS object"),
    }
    Ok(digest)
}

fn write_metadata(path: &Utf8Path, bytes: &[u8], immutable: bool) -> Result<()> {
    let parent = path.parent().context("CAS metadata has no parent")?;
    checked_directory(parent)?;
    fs::create_dir_all(parent)?;
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    let mut staged = NamedTempFile::new_in(parent)?;
    staged.write_all(bytes)?;
    staged.flush()?;
    staged.as_file().sync_all()?;
    if immutable {
        if path.exists() {
            if fs::read(path)? != bytes { anyhow::bail!("Existing CAS manifest conflicts with its digest"); }
            return Ok(());
        }
        match staged.persist_noclobber(path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                crate::path_safety::resolve_path(path.as_std_path(), true)?;
                if fs::read(path)? != bytes { anyhow::bail!("Conflicting CAS manifest publication"); }
            }
            Err(error) => return Err(error.error).context("Cannot publish CAS manifest"),
        }
    } else {
        staged.persist(path).map_err(|error| error.error).context("Cannot replace complete CAS project index")?;
    }
    Ok(())
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) { format!("\"{}\"", value.replace('"', "\"\"")) }
    else { value.to_owned() }
}

pub fn ingest_directory(project_root: &Utf8Path, cas_remote_root: &Utf8Path, label: &str, source_dir: &Utf8Path, version: &str) -> Result<()> {
    validate_manifest(&DirectoryManifest { label: label.to_owned(), version: version.to_owned(), files: vec![] })?;
    let source = crate::path_safety::resolve_path(source_dir.as_std_path(), true)?;
    if !source.is_dir() { anyhow::bail!("CAS ingestion source must be an existing directory"); }
    let remote = crate::path_safety::resolve_path(cas_remote_root.as_std_path(), true)?;
    let ledger = project_root.join(".projr/manifests.csv");
    let ledger_identity = crate::path_safety::resolve_path(ledger.as_std_path(), true)?;
    if source.starts_with(&remote) || remote.starts_with(&source) || ledger_identity.starts_with(&source) {
        anyhow::bail!("CAS source overlaps its storage or project metadata");
    }
    checked_directory(cas_remote_root)?;
    checked_directory(&cas_remote_root.join("objects"))?;
    checked_directory(&cas_remote_root.join("manifests"))?;
    checked_directory(&project_root.join(".projr"))?;
    let mut index = match fs::read(&ledger) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error).context("Cannot read existing CAS project index"),
    };
    fn scan(directory: &std::path::Path, root: &std::path::Path, files: &mut Vec<(String, camino::Utf8PathBuf)>) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = entry.file_type()?;
            let path = entry.path();
            if metadata.is_symlink() { anyhow::bail!("Linked CAS source entries are unsupported"); }
            if metadata.is_dir() { scan(&path, root, files)?; }
            else if metadata.is_file() {
                let components: Vec<_> = path.strip_prefix(root)?.components().map(|component| component.as_os_str().to_str().context("Non-UTF-8 CAS source filename")).collect::<Result<_>>()?;
                let logical = components.join("/");
                validate_logical_path(&logical)?;
                files.push((logical, camino::Utf8PathBuf::try_from(path)?));
            } else { anyhow::bail!("Unsupported CAS source entry"); }
        }
        Ok(())
    }
    let mut selected = Vec::new();
    scan(&source, &source, &mut selected)?;
    selected.sort_by(|left, right| left.0.cmp(&right.0));
    let objects = cas_remote_root.join("objects");
    let mut files = Vec::new();
    for (logical, path) in selected {
        crate::path_safety::resolve_path(path.as_std_path(), true)?;
        let mut file = File::open(&path).context("Cannot open CAS source file")?;
        let digest = store_object(&mut file, &objects)?;
        files.push(FileEntry { path: logical, hash: digest });
    }
    let manifest = DirectoryManifest { label: label.to_owned(), version: version.to_owned(), files };
    validate_manifest(&manifest)?;
    let bytes = serde_json::to_vec(&manifest)?;
    let digest = blake3::hash(&bytes).to_hex().to_string();
    write_metadata(&cas_remote_root.join("manifests").join(format!("{digest}.json")), &bytes, true)?;
    if !index.is_empty() && !index.ends_with(b"\n") { index.push(b'\n'); }
    index.extend_from_slice(format!("{},{},{},{}\n", csv_field(version), csv_field(label), digest, Utc::now().to_rfc3339()).as_bytes());
    write_metadata(&ledger, &index, false)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use tempfile::tempdir;
    use std::fs;


    #[test]
    fn test_hash_file() {
        let dir = tempdir().unwrap();
        let file_path = Utf8PathBuf::try_from(dir.path().join("test.txt")).unwrap();
        fs::write(&file_path, "test data").unwrap();

        let hash = hash_file(&file_path).unwrap();
        // precalculated blake3 hash for "test data"
        assert_eq!(hash, "6a953581d60dbebc9749b56d2383277fb02b58d260b4ccf6f119108fa0f1d4ef");
    }

    #[test]
    fn test_ingest_directory_empty() {
        let root = tempdir().unwrap();
        let physical_root = root.path().canonicalize().unwrap();
        let cas_remote = Utf8PathBuf::try_from(physical_root.join("cas")).unwrap();
        let proj_root = Utf8PathBuf::try_from(physical_root.join("proj")).unwrap();
        let source_dir = proj_root.join("data").join("raw");

        fs::create_dir_all(cas_remote.as_std_path()).unwrap();
        fs::create_dir_all(source_dir.as_std_path()).unwrap();

        ingest_directory(&proj_root, &cas_remote, "raw", &source_dir, "v0.0.1").unwrap();

        let manifests_dir = cas_remote.join("manifests");
        let manifest_files: Vec<_> = fs::read_dir(manifests_dir.as_std_path()).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(manifest_files.len(), 1);

        let manifest_content = fs::read_to_string(&manifest_files[0]).unwrap();
        let manifest: DirectoryManifest = serde_json::from_str(&manifest_content).unwrap();

        assert_eq!(manifest.label, "raw");
        assert_eq!(manifest.version, "v0.0.1");
        assert_eq!(manifest.files.len(), 0);

        let ledger_path = proj_root.join(".projr").join("manifests.csv");
        assert!(ledger_path.exists());
    }

    #[test]
    fn copied_stream_digest_binds_stored_bytes_even_when_source_path_changes() {
        struct ReplacedSource { original: std::io::Cursor<Vec<u8>>, path: std::path::PathBuf, replaced: bool }
        impl Read for ReplacedSource {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if !self.replaced {
                    fs::write(&self.path, b"replacement-path-bytes")?;
                    self.replaced = true;
                }
                self.original.read(buffer)
            }
        }
        let directory = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(directory.path().canonicalize().unwrap()).unwrap();
        let source = root.join("source");
        fs::write(&source, b"opened-source-bytes").unwrap();
        let mut reader = ReplacedSource { original: io::Cursor::new(b"opened-source-bytes".to_vec()), path: source.clone().into_std_path_buf(), replaced: false };
        let objects = root.join("objects");
        let digest = store_object(&mut reader, &objects).unwrap();
        assert_eq!(fs::read(object_path(&objects, &digest).unwrap()).unwrap(), b"opened-source-bytes");
        assert_eq!(digest, blake3::hash(b"opened-source-bytes").to_hex().to_string());
        assert_ne!(digest, hash_file(&source).unwrap());
    }

    #[test]
    fn failed_stream_never_publishes_an_object() {
        struct FailedRead(bool);
        impl Read for FailedRead {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if self.0 { return Err(io::Error::other("injected read failure")); }
                self.0 = true; buffer[..3].copy_from_slice(b"abc"); Ok(3)
            }
        }
        let directory = tempdir().unwrap();
        let objects = Utf8PathBuf::try_from(directory.path().canonicalize().unwrap()).unwrap().join("objects");
        assert!(store_object(&mut FailedRead(false), &objects).is_err());
        assert_eq!(fs::read_dir(objects).unwrap().count(), 0);
    }

    #[test]
    fn failed_metadata_publication_preserves_existing_bytes() {
        let directory = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(directory.path().canonicalize().unwrap()).unwrap();
        let manifest = root.join("manifest.json");
        fs::write(&manifest, "original manifest").unwrap();
        assert!(write_metadata(&manifest, b"conflicting manifest", true).is_err());
        assert_eq!(fs::read(&manifest).unwrap(), b"original manifest");
        let index = root.join("index.csv");
        fs::create_dir(&index).unwrap();
        fs::write(index.join("keep"), "unrelated bytes").unwrap();
        assert!(write_metadata(&index, b"new index", false).is_err());
        assert_eq!(fs::read(index.join("keep")).unwrap(), b"unrelated bytes");
        fs::remove_dir_all(&index).unwrap();
        fs::write(&index, "old index\n").unwrap();
        write_metadata(&index, b"complete replacement\n", false).unwrap();
        assert_eq!(fs::read(&index).unwrap(), b"complete replacement\n");
    }

}
