//! projr 0.6+ directory manifest store (`.projr/manifests/`, `<remote>/manifests/`).
//!
//! A store holds a `FORMAT` marker (`1`, or `importing` during an unfinished
//! first import) and one path-encoded directory per label. Each label directory
//! has a `versions.csv` index (label, version, directory_version, snapshot) and
//! immutable `files-<sha256>.csv` lists whose identifier is the SHA-256 of their
//! exact bytes. The byte format follows SATVILab/projr@ab3ce8a
//! (R/manifest-store.R and src/manifest.cpp). File rows keep the existing MD5
//! semantics of [`ManifestRow`]; SHA-256 only identifies CSV bytes.
use crate::manifest::ManifestRow;
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Write,
};

/// The completed store format marker.
pub const FORMAT: &str = "1";
/// The marker written while a first import from a legacy CSV is unfinished.
pub const IMPORTING: &str = "importing";
/// projr's cooperative writer lock name inside a store.
pub const LOCK: &str = ".versions.lock";
const INDEX: &str = "versions.csv";
const INDEX_COLUMNS: [&str; 4] = ["label", "version", "directory_version", "snapshot"];
const FILE_COLUMNS: [&str; 2] = ["fn", "hash"];
const ATTRIBUTES: [&str; 2] = ["files-*.csv -text", "versions.csv -text"];

/// One `versions.csv` row: a label's build version and its file-list snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    /// Directory label, identical for every row of one index.
    pub label: String,
    /// Build version, including any leading `v`.
    pub version: String,
    /// Earliest version (in numeric order) with the same snapshot.
    pub directory_version: String,
    /// Lowercase SHA-256 of the snapshot CSV bytes.
    pub snapshot: String,
}

/// Encodes a label as projr's portable directory component.
///
/// Bytes in `[a-z0-9-]` stay literal; every other UTF-8 byte becomes `_` plus
/// two lowercase hex digits. A leading `-` or a Windows device name (`con`,
/// `prn`, `aux`, `nul`, `com1`-`com9`, `lpt1`-`lpt9`) has its first byte
/// escaped, so names stay distinct on case-insensitive filesystems.
///
/// ```
/// use proj::manifest_store::encode_component;
/// assert_eq!(encode_component("raw-data"), "raw-data");
/// assert_eq!(encode_component("aux"), "_61ux");
/// assert_eq!(encode_component("Data_1"), "_44ata_5f1");
/// ```
pub fn encode_component(label: &str) -> String {
    let device = matches!(label, "con" | "prn" | "aux" | "nul")
        || (label.len() == 4
            && (label.starts_with("com") || label.starts_with("lpt"))
            && (b'1'..=b'9').contains(&label.as_bytes()[3]));
    let reserved = device || label.starts_with('-');
    let mut encoded = String::new();
    for (index, byte) in label.bytes().enumerate() {
        let plain = byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-';
        if plain && !(index == 0 && reserved) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("_{byte:02x}"));
        }
    }
    encoded
}

/// Lowercase hexadecimal SHA-256 of exact bytes, used for snapshot identity.
///
/// ```
/// assert_eq!(
///     proj::manifest_store::sha256_hex(b""),
///     "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
/// );
/// ```
pub fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn push_record(out: &mut Vec<u8>, fields: &[&str]) {
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        out.push(b'"');
        for byte in field.bytes() {
            if byte == b'"' {
                out.push(b'"');
            }
            out.push(byte);
        }
        out.push(b'"');
    }
    out.push(b'\n');
}

/// Serializes one snapshot: `"fn","hash"` header, every field quoted, rows
/// sorted by the raw bytes of fn then hash and deduplicated, UTF-8 with LF.
/// An empty slice gives the header-only empty-directory snapshot.
///
/// ```
/// let bytes = proj::manifest_store::encode_files(&[("b", "2"), ("a", "1"), ("b", "2")]);
/// assert_eq!(bytes, b"\"fn\",\"hash\"\n\"a\",\"1\"\n\"b\",\"2\"\n");
/// ```
pub fn encode_files(files: &[(&str, &str)]) -> Vec<u8> {
    let mut files = files.to_vec();
    files.sort_unstable();
    files.dedup();
    let mut out = Vec::new();
    push_record(&mut out, &FILE_COLUMNS);
    for (name, hash) in files {
        push_record(&mut out, &[name, hash]);
    }
    out
}

/// Serializes a `versions.csv` index with quoted fields and LF records.
pub fn encode_index(entries: &[IndexEntry]) -> Vec<u8> {
    let mut out = Vec::new();
    push_record(&mut out, &INDEX_COLUMNS);
    for entry in entries {
        push_record(
            &mut out,
            &[
                &entry.label,
                &entry.version,
                &entry.directory_version,
                &entry.snapshot,
            ],
        );
    }
    out
}

fn decode_csv<const N: usize>(
    bytes: &[u8],
    columns: [&str; N],
) -> anyhow::Result<Vec<[String; N]>> {
    let mut reader = csv::Reader::from_reader(bytes);
    anyhow::ensure!(
        reader.headers()?.iter().eq(columns),
        "Invalid manifest columns; expected {}",
        columns.join(",")
    );
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.context("Invalid manifest store record")?;
        rows.push(std::array::from_fn(|index| record[index].to_owned()));
    }
    Ok(rows)
}

/// Decodes a snapshot's `fn,hash` rows without reordering them.
pub fn decode_files(bytes: &[u8]) -> anyhow::Result<Vec<(String, String)>> {
    Ok(decode_csv(bytes, FILE_COLUMNS)?
        .into_iter()
        .map(|[name, hash]| (name, hash))
        .collect())
}

/// Numeric version key: one optional leading `v`, then `.`/`-` separated
/// decimal components, as accepted by R's `numeric_version`.
fn version_key(version: &str) -> anyhow::Result<Vec<u64>> {
    let bare = version.strip_prefix('v').unwrap_or(version);
    bare.split(['.', '-'])
        .map(|part| {
            anyhow::ensure!(
                !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()),
                "Invalid manifest version: {version}"
            );
            part.parse()
                .with_context(|| format!("Invalid manifest version: {version}"))
        })
        .collect()
}

/// Orders entries by numeric version (then version bytes) and recomputes each
/// `directory_version` as the earliest version sharing its snapshot.
pub fn order_index(entries: Vec<IndexEntry>) -> anyhow::Result<Vec<IndexEntry>> {
    let mut keyed = entries
        .into_iter()
        .map(|entry| Ok((version_key(&entry.version)?, entry)))
        .collect::<anyhow::Result<Vec<_>>>()?;
    keyed.sort_by(|(left_key, left), (right_key, right)| {
        left_key
            .cmp(right_key)
            .then_with(|| left.version.cmp(&right.version))
    });
    let mut first: HashMap<String, String> = HashMap::new();
    Ok(keyed
        .into_iter()
        .map(|(_, mut entry)| {
            entry.directory_version = first
                .entry(entry.snapshot.clone())
                .or_insert_with(|| entry.version.clone())
                .clone();
            entry
        })
        .collect())
}

/// Decodes and validates one label's `versions.csv`, whose directory name is
/// `directory`. A header-only index is empty. Every row must share one
/// nonempty label encoding to `directory`, versions must be unique and
/// numeric, snapshots 64 lowercase hex digits and directory versions exact.
pub fn decode_index(bytes: &[u8], directory: &str) -> anyhow::Result<Vec<IndexEntry>> {
    let entries: Vec<_> = decode_csv(bytes, INDEX_COLUMNS)?
        .into_iter()
        .map(|[label, version, directory_version, snapshot]| IndexEntry {
            label,
            version,
            directory_version,
            snapshot,
        })
        .collect();
    let Some(first) = entries.first() else {
        return Ok(entries);
    };
    let mut versions = std::collections::HashSet::new();
    anyhow::ensure!(
        !first.label.is_empty()
            && encode_component(&first.label) == directory
            && entries.iter().all(|entry| {
                entry.label == first.label
                    && !entry.version.is_empty()
                    && versions.insert(entry.version.as_str())
                    && entry.snapshot.len() == 64
                    && entry
                        .snapshot
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            }),
        "Invalid directory manifest index in {directory}/{INDEX}"
    );
    let expected: HashMap<_, _> = order_index(entries.clone())?
        .into_iter()
        .map(|entry| (entry.version, entry.directory_version))
        .collect();
    anyhow::ensure!(
        entries
            .iter()
            .all(|entry| expected[&entry.version] == entry.directory_version),
        "Invalid directory versions in {directory}/{INDEX}"
    );
    Ok(entries)
}

/// Returns `Ok(None)` for an absent entry and rejects links and special files.
fn regular_bytes(path: &Utf8Path) -> anyhow::Result<Option<Vec<u8>>> {
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("Cannot inspect manifest store file {path}"))
        }
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_file(),
                "Manifest store file must be regular: {path}"
            );
            Ok(Some(
                fs::read(path).with_context(|| format!("Cannot read {path}"))?,
            ))
        }
    }
}

/// Whether a store has a completed `FORMAT` of `1`. Absent or `importing`
/// markers are not ready; any other marker is an unknown format error.
pub fn ready(store: &Utf8Path) -> anyhow::Result<bool> {
    let path = store.join("FORMAT");
    let Some(bytes) = regular_bytes(&path)? else {
        return Ok(false);
    };
    let text = std::str::from_utf8(&bytes).unwrap_or("");
    let line = text
        .strip_suffix('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .unwrap_or(text);
    match line {
        FORMAT => Ok(true),
        IMPORTING => Ok(false),
        _ => anyhow::bail!("Unknown manifest format in {path}"),
    }
}

/// Holds projr's `.versions.lock` (std file locking) until dropped.
fn lock(store: &Utf8Path, exclusive: bool) -> anyhow::Result<Option<fs::File>> {
    let path = store.join(LOCK);
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    let file = if exclusive {
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
    } else {
        fs::OpenOptions::new().read(true).write(true).open(&path)
    };
    let file = match file {
        Ok(file) => file,
        // Like projr, readers only coordinate through an existing writable lock.
        Err(error)
            if !exclusive
                && matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error).with_context(|| format!("Cannot open {path}")),
    };
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "Manifest store lock must be regular: {path}"
    );
    let locked = if exclusive {
        file.lock()
    } else {
        file.lock_shared()
    };
    locked.with_context(|| format!("Cannot lock {path}"))?;
    Ok(Some(file))
}

/// Label directories (encoded name, path) that currently hold an index entry.
fn index_directories(store: &Utf8Path) -> anyhow::Result<Vec<(String, Utf8PathBuf)>> {
    let mut directories = Vec::new();
    for entry in
        fs::read_dir(store).with_context(|| format!("Cannot enumerate manifest store {store}"))?
    {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path =
            Utf8PathBuf::try_from(entry.path()).context("Manifest store paths must be UTF-8")?;
        anyhow::ensure!(
            !kind.is_symlink(),
            "Manifest store entries must not be links: {path}"
        );
        if !kind.is_dir() {
            continue;
        }
        let index = path.join(INDEX);
        if fs::symlink_metadata(&index).is_ok() {
            let name = path
                .file_name()
                .context("Manifest directory has no name")?
                .to_owned();
            directories.push((name, index));
        }
    }
    directories.sort();
    Ok(directories)
}

fn read_index_at(path: &Utf8Path, directory: &str) -> anyhow::Result<Vec<IndexEntry>> {
    let bytes = regular_bytes(path)?.with_context(|| format!("Missing manifest index {path}"))?;
    decode_index(&bytes, directory).with_context(|| format!("Cannot read manifest index {path}"))
}

/// Reads the validated indexes of a ready store, optionally for one label.
pub fn read_index(store: &Utf8Path, label: Option<&str>) -> anyhow::Result<Vec<IndexEntry>> {
    if !ready(store)? {
        return Ok(Vec::new());
    }
    let _lock = lock(store, false)?;
    let directories = match label {
        Some(label) => {
            let name = encode_component(label);
            let path = store.join(&name).join(INDEX);
            if regular_bytes(&path)?.is_none() {
                return Ok(Vec::new());
            }
            vec![(name, path)]
        }
        None => index_directories(store)?,
    };
    let mut entries = Vec::new();
    for (name, path) in directories {
        entries.extend(read_index_at(&path, &name)?);
    }
    Ok(entries)
}

/// Relative path of an entry's immutable snapshot inside its store.
pub fn snapshot_path(entry: &IndexEntry) -> String {
    format!(
        "{}/files-{}.csv",
        encode_component(&entry.label),
        entry.snapshot
    )
}

/// Reads and checksum-verifies the snapshot of each entry, expanding them to
/// four-column rows. An empty snapshot yields one blank-file marker row.
pub fn expand(store: &Utf8Path, entries: &[IndexEntry]) -> anyhow::Result<Vec<ManifestRow>> {
    let mut snapshots: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut rows = Vec::new();
    for entry in entries {
        let relative = snapshot_path(entry);
        if !snapshots.contains_key(&relative) {
            let path = store.join(&relative);
            let bytes = regular_bytes(&path)?
                .with_context(|| format!("Missing manifest snapshot {path}"))?;
            anyhow::ensure!(
                sha256_hex(&bytes) == entry.snapshot,
                "Manifest snapshot checksum mismatch in {path}"
            );
            let files = decode_files(&bytes)
                .with_context(|| format!("Cannot read manifest snapshot {path}"))?;
            snapshots.insert(relative.clone(), files);
        }
        let files = &snapshots[&relative];
        let blank = [(String::new(), String::new())];
        let files = if files.is_empty() {
            &blank[..]
        } else {
            &files[..]
        };
        for (name, hash) in files {
            let row = ManifestRow {
                label: entry.label.clone(),
                r#fn: name.clone(),
                version: entry.version.clone(),
                hash: hash.clone(),
            };
            row.validate()
                .with_context(|| format!("Invalid manifest snapshot row for {}", entry.label))?;
            rows.push(row);
        }
    }
    Ok(rows)
}

/// Reads a ready store's rows, optionally restricted to one label and/or one
/// version. Rows are ordered by label, numeric version, then fn/hash bytes.
/// A store that is absent, or not yet ready, has no rows.
pub fn read(
    store: &Utf8Path,
    label: Option<&str>,
    version: Option<&str>,
) -> anyhow::Result<Vec<ManifestRow>> {
    let mut entries = read_index(store, label)?;
    entries.retain(|entry| version.is_none_or(|version| entry.version == version));
    entries.sort_by(|left, right| left.label.cmp(&right.label));
    expand(store, &entries)
}

/// Reads all rows of a ready store, otherwise a legacy four-column CSV.
pub fn read_or_legacy(store: &Utf8Path, legacy: &Utf8Path) -> anyhow::Result<Vec<ManifestRow>> {
    if ready(store)? {
        read(store, None, None)
    } else {
        crate::manifest::read(legacy)
    }
}

/// Whether a ready store or a legacy CSV records any history location.
pub fn history_exists(store: &Utf8Path, legacy: &Utf8Path) -> anyhow::Result<bool> {
    Ok(ready(store)? || regular_bytes(legacy)?.is_some())
}

/// The project store, `<root>/.projr/manifests`.
pub fn project_store(root: &Utf8Path) -> Utf8PathBuf {
    root.join(".projr").join("manifests")
}

/// Reads project history from `.projr/manifests`, or a projr-written legacy
/// root `manifest.csv` before the first store write.
pub fn read_project(root: &Utf8Path) -> anyhow::Result<Vec<ManifestRow>> {
    read_or_legacy(&project_store(root), &root.join("manifest.csv"))
}

/// Records rows in the project store, importing a legacy root `manifest.csv`
/// once. The build's `.proj/build.lock` claim serializes projcli writers, so
/// no `.versions.lock` file is created inside the Git-tracked project store.
pub fn write_project(root: &Utf8Path, rows: &[ManifestRow]) -> anyhow::Result<()> {
    write(
        &project_store(root),
        rows,
        Some(&root.join("manifest.csv")),
        false,
    )
}

fn stage(directory: &Utf8Path, bytes: &[u8]) -> anyhow::Result<tempfile::NamedTempFile> {
    let mut staged = tempfile::Builder::new()
        .prefix(".manifest-")
        .tempfile_in(directory)
        .with_context(|| format!("Cannot stage manifest file in {directory}"))?;
    staged.write_all(bytes)?;
    staged.flush()?;
    Ok(staged)
}

fn replace(directory: &Utf8Path, path: &Utf8Path, bytes: &[u8]) -> anyhow::Result<()> {
    let staged = stage(directory, bytes)?;
    regular_bytes(path)?;
    staged
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("Cannot replace {path}"))?;
    Ok(())
}

fn real_directory(path: &Utf8Path) -> anyhow::Result<()> {
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => anyhow::ensure!(
            metadata.is_dir(),
            "Manifest store path must be a directory: {path}"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).with_context(|| format!("Cannot create {path}"))?
        }
        Err(error) => return Err(error).with_context(|| format!("Cannot inspect {path}")),
    }
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    Ok(())
}

/// Records rows in a store, replacing each supplied (label, version) and
/// keeping every other version. Snapshot files are immutable and shared by
/// versions with identical file lists; an index is rewritten only on change.
///
/// When the store is not ready, a regular `legacy` CSV is imported first under
/// a `FORMAT` of `importing`, which a retry discards and re-imports; the legacy
/// file itself is never changed. Indexes without any `FORMAT` fail closed. All
/// rows and versions are validated before any index is written. `.gitattributes`
/// receives projr's `-text` rules. With `lock`, the write holds an exclusive
/// std file lock on `.versions.lock` (projr's lock name).
pub fn write(
    store: &Utf8Path,
    rows: &[ManifestRow],
    legacy: Option<&Utf8Path>,
    lock: bool,
) -> anyhow::Result<()> {
    real_directory(store)?;
    let _lock = if lock { self::lock(store, true)? } else { None };
    let ready = ready(store)?;
    let mut batches = Vec::new();
    if !ready && let Some(legacy) = legacy {
        batches.push(crate::manifest::read(legacy)?);
    }
    batches.push(rows.to_vec());
    let marker = store.join("FORMAT");
    if !ready {
        let indexes = index_directories(store)?;
        if regular_bytes(&marker)?.is_some() {
            // The importing marker identifies provisional indexes from a failed run.
            for (_, index) in indexes {
                fs::remove_file(&index)
                    .with_context(|| format!("Cannot discard provisional index {index}"))?;
            }
        } else {
            if let Some((_, index)) = indexes.first() {
                anyhow::bail!("Cannot initialize manifest store with an existing index: {index}");
            }
            replace(store, &marker, format!("{IMPORTING}\n").as_bytes())?;
        }
    }
    for batch in batches {
        for row in &batch {
            row.validate()?;
            version_key(&row.version)?;
        }
        let mut labels: BTreeMap<&str, Vec<&ManifestRow>> = BTreeMap::new();
        for row in &batch {
            labels.entry(row.label.as_str()).or_default().push(row);
        }
        for (label, rows) in labels {
            let directory = store.join(encode_component(label));
            real_directory(&directory)?;
            write_label(&directory, label, &rows)?;
        }
    }
    // Keep generated CSV bytes stable across checkouts with different Git EOL settings.
    let path = store.join(".gitattributes");
    let existing = regular_bytes(&path)?;
    let mut lines: Vec<String> = existing
        .as_deref()
        .map(|bytes| {
            String::from_utf8_lossy(bytes)
                .lines()
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let missing: Vec<_> = ATTRIBUTES
        .iter()
        .filter(|rule| !lines.iter().any(|line| line == *rule))
        .collect();
    if !missing.is_empty() {
        lines.extend(missing.into_iter().map(|rule| (*rule).to_owned()));
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        replace(store, &path, text.as_bytes())?;
    }
    if !ready {
        replace(store, &marker, format!("{FORMAT}\n").as_bytes())?;
    }
    Ok(())
}

fn write_label(directory: &Utf8Path, label: &str, rows: &[&ManifestRow]) -> anyhow::Result<()> {
    let name = directory
        .file_name()
        .context("Manifest directory has no name")?;
    let path = directory.join(INDEX);
    let old = match regular_bytes(&path)? {
        Some(bytes) => decode_index(&bytes, name)
            .with_context(|| format!("Cannot read manifest index {path}"))?,
        None => Vec::new(),
    };
    let mut groups: Vec<(&str, Vec<(&str, &str)>)> = Vec::new();
    for row in rows {
        let file = (row.r#fn.as_str(), row.hash.as_str());
        match groups
            .iter()
            .position(|(version, _)| *version == row.version)
        {
            Some(position) => groups[position].1.push(file),
            None => groups.push((row.version.as_str(), vec![file])),
        }
    }
    let mut incoming = Vec::new();
    for (version, mut files) in groups {
        // projr's historical empty directory is one row with blank fn and hash.
        if files
            .iter()
            .all(|(name, hash)| name.is_empty() && hash.is_empty())
        {
            files.clear();
        }
        let bytes = encode_files(&files);
        let snapshot = sha256_hex(&bytes);
        let target = directory.join(format!("files-{snapshot}.csv"));
        if regular_bytes(&target)?.is_none() {
            let staged = stage(directory, &bytes)?;
            match staged.persist_noclobber(&target) {
                Ok(_) => {}
                Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(error.error)
                        .with_context(|| format!("Cannot write manifest snapshot {target}"));
                }
            }
        }
        incoming.push(IndexEntry {
            label: label.to_owned(),
            version: version.to_owned(),
            directory_version: String::new(),
            snapshot,
        });
    }
    let mut index: Vec<_> = old
        .iter()
        .filter(|entry| !incoming.iter().any(|new| new.version == entry.version))
        .cloned()
        .collect();
    index.extend(incoming);
    let index = order_index(index)?;
    if index != old {
        replace(directory, &path, &encode_index(&index))?;
    }
    Ok(())
}

/// Relative paths projr publishes for a ready store: `FORMAT`, each label's
/// `versions.csv` and every referenced snapshot. Lock/legacy files are excluded.
pub fn published_files(store: &Utf8Path) -> anyhow::Result<Vec<String>> {
    let entries = read_index(store, None)?;
    let mut files = vec!["FORMAT".to_owned()];
    let mut labels: Vec<_> = entries
        .iter()
        .map(|entry| encode_component(&entry.label))
        .collect();
    labels.dedup();
    files.extend(labels.into_iter().map(|label| format!("{label}/{INDEX}")));
    for entry in &entries {
        let path = snapshot_path(entry);
        if !files.contains(&path) {
            files.push(path);
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(version: &str, snapshot: char) -> IndexEntry {
        IndexEntry {
            label: "output".into(),
            version: version.into(),
            directory_version: String::new(),
            snapshot: snapshot.to_string().repeat(64),
        }
    }

    #[test]
    fn components_match_projr_path_encoding() {
        assert_eq!(encode_component("output"), "output");
        assert_eq!(encode_component("aux"), "_61ux");
        assert_eq!(encode_component("com1"), "_63om1");
        assert_eq!(encode_component("com10"), "com10");
        assert_eq!(encode_component("-a"), "_2da");
        assert_eq!(encode_component("../CON"), "_2e_2e_2f_43_4f_4e");
        assert_eq!(encode_component("a_2f"), "a_5f2f");
        assert_eq!(encode_component("\u{3b2}"), "_ce_b2");
    }

    #[test]
    fn snapshot_bytes_sort_raw_bytes_and_quote_every_field() {
        let files = [
            ("u,\"a\"", "0002"),
            ("\u{3a9}\u{6c34}", "0003"),
            ("001", "0001"),
            ("NA", "0004"),
            ("line\nbreak", "0005"),
            ("001", "0001"),
            ("001", "0000"),
        ];
        let expected = "\"fn\",\"hash\"\n\"001\",\"0000\"\n\"001\",\"0001\"\n\"NA\",\"0004\"\n\
            \"line\nbreak\",\"0005\"\n\"u,\"\"a\"\"\",\"0002\"\n\"\u{3a9}\u{6c34}\",\"0003\"\n";
        assert_eq!(encode_files(&files), expected.as_bytes());
        let decoded = decode_files(expected.as_bytes()).unwrap();
        assert_eq!(decoded.len(), 6);
        assert_eq!(decoded[4].0, "u,\"a\"");
        assert_eq!(encode_files(&[]), b"\"fn\",\"hash\"\n");
        assert!(decode_files(b"\"hash\",\"fn\"\n").is_err());
    }

    #[test]
    fn index_orders_numerically_and_records_first_directory_version() {
        let index = order_index(vec![
            entry("v1.11", 'a'),
            entry("v1.9", 'a'),
            entry("v1.10", 'b'),
        ])
        .unwrap();
        let versions: Vec<_> = index.iter().map(|e| e.version.as_str()).collect();
        assert_eq!(versions, ["v1.9", "v1.10", "v1.11"]);
        let first: Vec<_> = index.iter().map(|e| e.directory_version.as_str()).collect();
        assert_eq!(first, ["v1.9", "v1.10", "v1.9"]);
        let bytes = encode_index(&index);
        assert!(bytes.starts_with(b"\"label\",\"version\",\"directory_version\",\"snapshot\"\n\"output\",\"v1.9\",\"v1.9\",\""));
        assert_eq!(decode_index(&bytes, "output").unwrap(), index);
        assert!(order_index(vec![entry("invalid", 'a')]).is_err());
        assert!(order_index(vec![entry("v1..2", 'a')]).is_err());
    }

    #[test]
    fn invalid_indexes_fail_closed() {
        let good = encode_index(&order_index(vec![entry("v1", 'a')]).unwrap());
        assert!(decode_index(&good, "other").is_err());
        let mut wrong_first = order_index(vec![entry("v1", 'a'), entry("v2", 'a')]).unwrap();
        wrong_first[1].directory_version = "v2".into();
        assert!(decode_index(&encode_index(&wrong_first), "output").is_err());
        let mut foreign = order_index(vec![entry("v1", 'a')]).unwrap();
        foreign[0].snapshot = "../foreign".into();
        assert!(decode_index(&encode_index(&foreign), "output").is_err());
        let duplicate = vec![entry("v1", 'a'), entry("v1", 'a')];
        assert!(decode_index(&encode_index(&duplicate), "output").is_err());
        assert!(
            decode_index(
                b"\"label\",\"version\",\"directory_version\",\"snapshot\"\n",
                "x"
            )
            .unwrap()
            .is_empty()
        );
    }
}
