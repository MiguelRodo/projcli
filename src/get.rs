//! Shared verified download backend for `proj restore` and `proj get`.
//!
//! Follows SATVILab/projr@5f915ef `R/get.R`, `R/get-install.R`,
//! `R/get-project.R` and `R/restore-build.R`. Every selected label's files are
//! checked against a build's `.projr/manifests` snapshot (MD5 per file) before
//! any destination changes. Each label is staged beside its destination, all
//! labels are staged before the first is replaced, and replacement is a
//! rename-swap that restores earlier labels if a later swap fails; an original
//! that cannot be moved back is kept and its path reported.
//!
//! projcli keeps its stronger guards: link and special-file rejection on both
//! sides, bounded GitHub transfers and ZIP extraction, no credentials in errors,
//! and no Authorization header on download redirects. It does not lock the
//! output directory against concurrent writers.
use crate::github_release::{GithubReleaseClient, ReleaseAsset};
use crate::github_zip::{GithubStructure, directory_asset_name};
use crate::manifest::ManifestRow;
use crate::manifest_store;
use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use serde_json::Value;
use std::{collections::BTreeMap, fs};

/// A snapshot's files: sorted `(path, md5)` pairs.
pub(crate) type Files = Vec<(String, String)>;

/// One label's recorded file list at one build version.
#[derive(Clone, Default)]
pub(crate) struct Snapshot {
    /// None when the index names a snapshot whose file list is unavailable.
    pub(crate) files: Option<Files>,
    /// Versions with this file list up to and including the requested one.
    pub(crate) versions: Vec<String>,
    /// The four-column rows, as recorded.
    pub(crate) rows: Vec<ManifestRow>,
    /// SHA-256 of the canonical file list.
    pub(crate) id: Option<String>,
}

/// One installed or verified label.
#[derive(Debug, Serialize)]
pub struct Installed {
    /// Directory label.
    pub label: String,
    /// Final directory.
    pub path: Utf8PathBuf,
    /// `verified` when the directory already matched, `installed` otherwise.
    pub status: &'static str,
}

/// Adds exactly one leading `v`.
pub(crate) fn v_add(version: &str) -> String {
    format!("v{}", version.strip_prefix('v').unwrap_or(version))
}

/// Validates a build version (`^v?[0-9]+([.-][0-9]+)*$`) and adds a leading `v`.
pub(crate) fn check_version(version: &str) -> Result<String> {
    let bare = version.strip_prefix('v').unwrap_or(version);
    anyhow::ensure!(
        bare.split(['.', '-'])
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())),
        "Invalid build version: {version}"
    );
    Ok(v_add(bare))
}

fn version_key(version: &str) -> Vec<u64> {
    version
        .strip_prefix('v')
        .unwrap_or(version)
        .split(['.', '-'])
        .map(|part| part.parse().unwrap_or(u64::MAX))
        .collect()
}

fn sort_versions(versions: &mut Vec<String>) {
    versions.sort_by(|a, b| version_key(a).cmp(&version_key(b)).then(a.cmp(b)));
    versions.dedup();
}

/// projr's `.restore_build_files`: a lone blank row is an empty directory;
/// otherwise paths must be unique, relative and normalized, hashes lowercase MD5.
pub(crate) fn files_of(rows: &[ManifestRow]) -> Result<Files> {
    if rows.len() == 1 && rows[0].r#fn.is_empty() && rows[0].hash.is_empty() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for row in rows {
        let name = &row.r#fn;
        anyhow::ensure!(
            !name.is_empty()
                && !name.starts_with('/')
                && !name.contains('\\')
                && !name.contains("//")
                && !name.ends_with('/')
                && !(name.len() >= 2 && name.as_bytes()[1] == b':')
                && name.split('/').all(|part| part != "." && part != "..")
                && row.hash.len() == 32
                && row
                    .hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "Invalid file list in build snapshot."
        );
        files.push((name.clone(), row.hash.clone()));
    }
    files.sort();
    anyhow::ensure!(
        files.windows(2).all(|pair| pair[0].0 != pair[1].0),
        "Invalid file list in build snapshot."
    );
    Ok(files)
}

fn missing(path: &Utf8Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error).with_context(|| format!("Cannot inspect {path}")),
    }
}

/// Reads one label's snapshot at `version` from a `.projr/manifests` store, or
/// from a legacy four-column `manifest.csv` when the store is not ready
/// (projr `.restore_build_snapshot`). None when the version is not recorded.
pub(crate) fn snapshot(
    store: &Utf8Path,
    csv: &Utf8Path,
    label: &str,
    version: &str,
) -> Result<Option<Snapshot>> {
    let (rows, id, mut versions) = if manifest_store::ready(store)? {
        let index = manifest_store::read_index(store, Some(label))?;
        let Some(hit) = index.iter().find(|entry| entry.version == version) else {
            return Ok(None);
        };
        if missing(&store.join(manifest_store::snapshot_path(hit)))? {
            return Ok(Some(Snapshot {
                id: Some(hit.snapshot.clone()),
                ..Default::default()
            }));
        }
        let versions = index
            .iter()
            .filter(|entry| entry.snapshot == hit.snapshot)
            .map(|entry| entry.version.clone())
            .collect();
        let rows = manifest_store::expand(store, std::slice::from_ref(hit))?;
        (rows, hit.snapshot.clone(), versions)
    } else {
        let history: Vec<_> = crate::manifest::read(csv)?
            .into_iter()
            .filter(|row| row.label == label)
            .collect();
        let rows: Vec<_> = history
            .iter()
            .filter(|row| row.version == version)
            .cloned()
            .collect();
        if rows.is_empty() {
            return Ok(None);
        }
        let files = files_of(&rows)?;
        let pairs: Vec<_> = files
            .iter()
            .map(|(name, hash)| (name.as_str(), hash.as_str()))
            .collect();
        let id = manifest_store::sha256_hex(&manifest_store::encode_files(&pairs));
        let mut groups: BTreeMap<&str, Vec<ManifestRow>> = BTreeMap::new();
        for row in &history {
            groups.entry(&row.version).or_default().push(row.clone());
        }
        let versions = groups
            .into_iter()
            .filter(|(_, group)| files_of(group).is_ok_and(|other| other == files))
            .map(|(version, _)| version.to_owned())
            .collect();
        (rows, id, versions)
    };
    sort_versions(&mut versions);
    if let Some(position) = versions.iter().position(|v| v == version) {
        versions.truncate(position + 1);
    }
    Ok(Some(Snapshot {
        files: Some(files_of(&rows)?),
        versions,
        rows,
        id: Some(id),
    }))
}

/// Versions in a store's index sharing `id`; a later upload can repair an archive.
fn matching_versions(store: &Utf8Path, label: &str, id: Option<&str>) -> Result<Vec<String>> {
    let Some(id) = id else {
        return Ok(Vec::new());
    };
    if !manifest_store::ready(store)? {
        return Ok(Vec::new());
    }
    Ok(manifest_store::read_index(store, Some(label))?
        .into_iter()
        .filter(|entry| entry.snapshot == id)
        .map(|entry| entry.version)
        .collect())
}

/// MD5 of every regular file below `path`; links and special files are errors.
pub(crate) fn hash_dir(path: &Utf8Path) -> Result<Files> {
    if missing(path)? {
        return Ok(Vec::new());
    }
    crate::manifest::inventory_directory(path, false)
}

/// Where a source's files come from.
pub(crate) enum Kind {
    /// A checkout's (or producer's) own manifest store: snapshots only, and the
    /// payload of a recorded empty directory.
    Metadata,
    /// A producing project's own directories (`get local --path`).
    Project {
        root: Utf8PathBuf,
        dirs: BTreeMap<String, Utf8PathBuf>,
    },
    /// A local remote's `<base>/<label>/<version>` hierarchy.
    Local {
        base: Utf8PathBuf,
        archive: bool,
        append: bool,
    },
    /// A GitHub Release's flat ZIP assets.
    Github {
        client: Box<GithubReleaseClient>,
        tag: String,
        assets: Vec<ReleaseAsset>,
        prefix: Option<String>,
        append: bool,
        archive: bool,
        direct_tag: bool,
    },
}

/// One manifest source in priority order.
pub(crate) struct Source {
    pub(crate) kind: Kind,
    pub(crate) store: Utf8PathBuf,
    pub(crate) csv: Utf8PathBuf,
    /// A short description for results; never a URL or credential.
    pub(crate) name: String,
}

impl Source {
    fn metadata_like(&self) -> bool {
        matches!(self.kind, Kind::Metadata | Kind::Project { .. })
    }
}

/// Budgets and the private workspace for one call.
pub(crate) struct Transfer<'a> {
    pub(crate) workspace: &'a Utf8Path,
    pub(crate) max_bytes: u64,
    pub(crate) max_expanded_bytes: u64,
}

/// A private, canonical temporary workspace (macOS temp paths cross a symlink).
pub(crate) fn workspace() -> Result<(tempfile::TempDir, Utf8PathBuf)> {
    let directory = tempfile::Builder::new()
        .prefix("projr-get-")
        .tempdir_in(fs::canonicalize(std::env::temp_dir())?)?;
    let path = Utf8PathBuf::try_from(fs::canonicalize(directory.path())?)
        .context("Temporary path must be UTF-8")?;
    Ok((directory, path))
}

/// The GitHub API base and environment token used by restore and get.
pub(crate) fn github_client(repository: &str) -> Result<GithubReleaseClient> {
    let token = ["GITHUB_PAT", "GH_TOKEN", "GITHUB_TOKEN"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.trim().is_empty()));
    GithubReleaseClient::new(repository, &github_api(), token)
}

/// `GITHUB_API_URL`, else the public API.
pub(crate) fn github_api() -> String {
    std::env::var("GITHUB_API_URL").unwrap_or_else(|_| "https://api.github.com".into())
}

/// A GitHub Release target before any network request.
pub(crate) struct GithubTarget {
    pub(crate) repository: String,
    pub(crate) tag: String,
    pub(crate) prefix: Option<String>,
    pub(crate) append: bool,
    pub(crate) archive: bool,
    pub(crate) direct_tag: bool,
}

/// Lists a Release and downloads its manifest metadata into the workspace.
/// None when the Release does not exist.
pub(crate) fn github_source(
    target: GithubTarget,
    transfer: &Transfer<'_>,
) -> Result<Option<Source>> {
    let client = github_client(&target.repository)?;
    let Some(assets) = client.assets_if_exists(&target.tag)? else {
        return Ok(None);
    };
    let metadata = tempfile::Builder::new()
        .prefix("metadata-")
        .tempdir_in(transfer.workspace)?
        .keep();
    let metadata = Utf8PathBuf::try_from(metadata).context("Workspace path must be UTF-8")?;
    let limit = transfer.max_bytes.min(256 * 1024 * 1024);
    if assets.iter().any(|asset| asset.name == "manifest.zip") {
        let zip = metadata.join("manifest.zip");
        client.download_asset(transfer.workspace, &target.tag, "manifest.zip", &zip, limit)?;
        let extracted = crate::github_zip::extract_zip(
            &zip,
            transfer.max_expanded_bytes.min(1024 * 1024 * 1024),
        )?;
        let installed = Utf8Path::from_path(extracted.path())
            .context("Extraction path must be UTF-8")
            .and_then(|path| crate::github_zip::install_manifest_tree(path, &metadata));
        extracted.close()?;
        installed?;
    } else if assets.iter().any(|asset| asset.name == "manifest.csv") {
        client.download_asset(
            transfer.workspace,
            &target.tag,
            "manifest.csv",
            &metadata.join("manifest.csv"),
            limit,
        )?;
    }
    Ok(Some(Source {
        name: format!("github:{}@{}", target.repository, target.tag),
        store: metadata.join("manifests"),
        csv: metadata.join("manifest.csv"),
        kind: Kind::Github {
            client: Box::new(client),
            tag: target.tag,
            assets,
            prefix: target.prefix,
            append: target.append,
            archive: target.archive,
            direct_tag: target.direct_tag,
        },
    }))
}

/// A local remote's sources at `base` (`manifests/` store or legacy CSV).
pub(crate) fn local_source(name: String, base: Utf8PathBuf, archive: bool, append: bool) -> Source {
    Source {
        name,
        store: base.join("manifests"),
        csv: base.join("manifest.csv"),
        kind: Kind::Local {
            base,
            archive,
            append,
        },
    }
}

/// A checkout's own manifest store.
pub(crate) fn metadata_source(root: &Utf8Path) -> Source {
    Source {
        name: "project".into(),
        store: manifest_store::project_store(root),
        csv: root.join("manifest.csv"),
        kind: Kind::Metadata,
    }
}

fn copy_verified(source: &Utf8Path, dest: &Utf8Path, expected: &Files) -> Result<bool> {
    crate::fs_utils::copy_dir_recursive(source, dest)?;
    if hash_dir(dest)? == *expected {
        return Ok(true);
    }
    for entry in fs::read_dir(dest)? {
        let path = entry?.path();
        if fs::symlink_metadata(&path)?.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
    }
    Ok(false)
}

fn same_names(left: &Files, right: &Files) -> bool {
    left.len() == right.len() && left.iter().zip(right).all(|(a, b)| a.0 == b.0)
}

/// Copies the first candidate directory whose file list matches `expected`.
fn fetch_directories(
    candidates: &[Utf8PathBuf],
    dest: &Utf8Path,
    expected: &Files,
) -> Result<bool> {
    for path in candidates {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(metadata) if metadata.file_type().is_symlink() => {
                anyhow::bail!("Cannot verify symbolic links in {path}.")
            }
            _ => continue,
        }
        let real =
            Utf8PathBuf::try_from(fs::canonicalize(path)?).context("Source path must be UTF-8")?;
        let files = crate::manifest::inventory_directory(&real, false)
            .with_context(|| format!("Cannot verify {path}"))?;
        if !same_names(&files, expected) {
            continue;
        }
        if copy_verified(&real, dest, expected)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn fetch(
    source: &Source,
    label: &str,
    version: &str,
    snapshot: &Snapshot,
    expected: &Files,
    dest: &Utf8Path,
    transfer: &Transfer<'_>,
) -> Result<bool> {
    let mut versions = vec![version.to_owned()];
    for other in &snapshot.versions {
        if !versions.contains(other) {
            versions.push(other.clone());
        }
    }
    match &source.kind {
        Kind::Metadata => Ok(true),
        Kind::Project { dirs, .. } => {
            let Some(path) = dirs.get(label) else {
                return Ok(false);
            };
            if expected.is_empty() && missing(path)? {
                return Ok(true);
            }
            fetch_directories(std::slice::from_ref(path), dest, expected)
        }
        Kind::Local {
            base,
            archive,
            append,
        } => {
            // A latest layout without a label directory mixes data with remote
            // metadata; projcli does not read it.
            if !append && !archive {
                return Ok(false);
            }
            let mut candidates = Vec::new();
            for version in &versions {
                let mut path = base.clone();
                if *append {
                    path.push(label);
                }
                if *archive {
                    path.push(version);
                }
                if expected.is_empty() {
                    path = Utf8PathBuf::from(format!("{path}-empty"));
                }
                if !candidates.contains(&path) {
                    candidates.push(path);
                }
            }
            fetch_directories(&candidates, dest, expected)
        }
        Kind::Github {
            client,
            tag,
            assets,
            prefix,
            append,
            archive,
            direct_tag,
        } => {
            // A recorded empty directory has no payload to transfer.
            if expected.is_empty() {
                return Ok(true);
            }
            let structure = if *archive {
                GithubStructure::Archive
            } else {
                GithubStructure::Latest
            };
            let mut candidates = Vec::new();
            for version in &versions {
                candidates.push(directory_asset_name(
                    prefix.as_deref(),
                    *append,
                    label,
                    structure,
                    Some(version),
                    false,
                )?);
            }
            // A concrete tag identifies a Release directly, including its latest layout.
            if *direct_tag {
                candidates.push(directory_asset_name(
                    None,
                    true,
                    label,
                    GithubStructure::Latest,
                    None,
                    false,
                )?);
            }
            let Some(name) = candidates
                .into_iter()
                .find(|name| assets.iter().any(|asset| &asset.name == name))
            else {
                return Ok(false);
            };
            let download = tempfile::Builder::new()
                .prefix("asset-")
                .tempdir_in(transfer.workspace)?;
            let zip = Utf8Path::from_path(download.path())
                .context("Workspace path must be UTF-8")?
                .join("asset.zip");
            client.download_asset(transfer.workspace, tag, &name, &zip, transfer.max_bytes)?;
            let extracted = crate::github_zip::extract_zip(&zip, transfer.max_expanded_bytes)?;
            let result = (|| {
                let root = Utf8Path::from_path(extracted.path())
                    .context("Extraction path must be UTF-8")?;
                anyhow::ensure!(
                    same_names(
                        &crate::manifest::inventory_directory(root, false)?,
                        expected
                    ),
                    "Archive file list does not match the selected build snapshot."
                );
                anyhow::ensure!(
                    copy_verified(root, dest, expected)?,
                    "Archive file hash does not match the selected build snapshot."
                );
                Ok(true)
            })();
            let cleanup = extracted.close();
            let result = result?;
            cleanup?;
            Ok(result)
        }
    }
}

/// Rejects destinations that are files or links, contain the project root,
/// fall inside its `.git`, `.projr` or `_projr`, overlap one another or overlap
/// `protected` source paths (projr `.restore_build_check_paths` and the
/// overlap checks of `.get_install`).
pub(crate) fn check_destinations(
    root: Option<&Utf8Path>,
    paths: &[Utf8PathBuf],
    protected: &[Utf8PathBuf],
) -> Result<()> {
    let overlaps = |a: &std::path::Path, b: &std::path::Path| a.starts_with(b) || b.starts_with(a);
    let mut identities = Vec::new();
    for path in paths {
        if let Ok(metadata) = fs::symlink_metadata(path) {
            anyhow::ensure!(
                metadata.is_dir(),
                "Restore destination is a file or link: {path}"
            );
        }
        identities.push(crate::path_safety::resolve_path(path.as_std_path(), true)?);
    }
    if let Some(root) = root {
        let root = crate::path_safety::resolve_path(root.as_std_path(), false)?;
        for (path, identity) in paths.iter().zip(&identities) {
            anyhow::ensure!(
                !root.starts_with(identity)
                    && [".git", ".projr", "_projr"]
                        .iter()
                        .all(|name| !identity.starts_with(root.join(name))),
                "Overlapping or protected restore directory: {path}"
            );
        }
    }
    for (index, identity) in identities.iter().enumerate() {
        for other in &identities[index + 1..] {
            anyhow::ensure!(
                !overlaps(identity, other),
                "Overlapping or protected restore directory: {}",
                paths[index]
            );
        }
        for path in protected {
            let protected = crate::path_safety::resolve_path(path.as_std_path(), false)?;
            anyhow::ensure!(
                !overlaps(identity, &protected),
                "The download destination overlaps the source: {path}"
            );
        }
    }
    Ok(())
}

/// One label's install request.
pub(crate) struct Install<'a> {
    pub(crate) sources: &'a [Source],
    pub(crate) labels: &'a [String],
    pub(crate) paths: &'a [Utf8PathBuf],
    pub(crate) version: &'a str,
    pub(crate) overwrite: bool,
    /// The flag that allows replacing local changes, for messages.
    pub(crate) option: &'a str,
}

struct Stage {
    index: usize,
    directory: tempfile::TempDir,
    existing: Files,
}

/// Stages every label beside its destination, verifying each file's MD5 against
/// the snapshot, before replacing any destination (projr `.get_install`).
/// Matching directories are left untouched. Without `overwrite`, local files that
/// differ from the snapshot (including extra files) are an error; files that match
/// are kept. Sources are tried in order, metadata first. Two sources with
/// different snapshots for one label are an error.
pub(crate) fn install(request: Install<'_>, transfer: &Transfer<'_>) -> Result<Vec<Installed>> {
    let mut stages: Vec<Stage> = Vec::new();
    let mut outcomes = Vec::new();
    for (index, label) in request.labels.iter().enumerate() {
        let path = &request.paths[index];
        let existing = hash_dir(path)?;
        let present = !missing(path)?;
        let mut expected: Option<Files> = None;
        let mut project: Option<Snapshot> = None;
        let mut found = false;
        let mut staged = false;
        for source in request.sources {
            let mut snap =
                snapshot(&source.store, &source.csv, label, request.version)?.unwrap_or_default();
            if source.metadata_like() {
                project = Some(snap.clone());
            }
            let project_id = project.as_ref().and_then(|p| p.id.as_deref());
            if let (Some(id), Some(project_id)) = (snap.id.as_deref(), project_id) {
                anyhow::ensure!(
                    id == project_id,
                    "Sources disagree about {label} at {}.",
                    request.version
                );
            }
            match (&snap.files, project.as_ref().and_then(|p| p.files.as_ref())) {
                (None, None) => continue,
                (None, Some(_)) => snap = project.clone().unwrap_or_default(),
                (Some(_), _) => {
                    if let Some(project) = &project {
                        snap.versions.extend(project.versions.iter().cloned());
                        sort_versions(&mut snap.versions);
                    }
                }
            }
            let files = snap.files.clone().unwrap_or_default();
            if let Some(expected) = &expected {
                anyhow::ensure!(
                    *expected == files,
                    "Sources disagree about {label} at {}.",
                    request.version
                );
            }
            expected = Some(files.clone());
            // A recorded empty directory needs no payload from any source.
            if matches!(source.kind, Kind::Metadata) && !files.is_empty() {
                continue;
            }
            if !source.metadata_like() {
                snap.versions
                    .extend(matching_versions(&source.store, label, snap.id.as_deref())?);
                sort_versions(&mut snap.versions);
            }
            if present && existing == files {
                found = true;
                break;
            }
            if !request.overwrite
                && existing.iter().any(|(name, hash)| {
                    files
                        .binary_search_by(|(other, _)| other.cmp(name))
                        .map_or(true, |at| files[at].1 != *hash)
                })
            {
                anyhow::bail!(
                    "Local files in {path} differ from {}. Use {} to replace them.",
                    request.version,
                    request.option
                );
            }
            if !staged {
                let parent = path.parent().context("Destination has no parent")?;
                fs::create_dir_all(parent).with_context(|| format!("Cannot create {parent}"))?;
                crate::path_safety::resolve_path(parent.as_std_path(), true)?;
                stages.push(Stage {
                    index,
                    directory: tempfile::Builder::new()
                        .prefix(".projr-get-")
                        .tempdir_in(parent)?,
                    existing: existing.clone(),
                });
                staged = true;
            }
            let stage = stages.last().unwrap();
            let dest = Utf8PathBuf::try_from(stage.directory.path().join("new"))
                .context("Staging path must be UTF-8")?;
            fs::create_dir(&dest)?;
            if fetch(
                source,
                label,
                request.version,
                &snap,
                &files,
                &dest,
                transfer,
            )? {
                found = true;
                break;
            }
            fs::remove_dir_all(&dest)?;
        }
        anyhow::ensure!(
            found,
            "No matching files for {label} at {}. No matching archive was found.",
            request.version
        );
        let staged_new = stages.last().is_some_and(|stage| {
            stage.index == index && stage.directory.path().join("new").is_dir()
        });
        if !staged_new {
            if staged {
                stages.pop();
            }
            outcomes.push(Installed {
                label: label.clone(),
                path: path.clone(),
                status: "verified",
            });
        } else {
            outcomes.push(Installed {
                label: label.clone(),
                path: path.clone(),
                status: "installed",
            });
        }
    }
    // Every label is staged; nothing has changed yet. Fail if a destination changed.
    for stage in &stages {
        let path = &request.paths[stage.index];
        anyhow::ensure!(
            hash_dir(path)? == stage.existing,
            "{path} changed while restoring; nothing was replaced"
        );
    }
    let mut moved: Vec<usize> = Vec::new();
    let mut installed: Vec<usize> = Vec::new();
    let mut failure = None;
    for (position, stage) in stages.iter().enumerate() {
        let path = &request.paths[stage.index];
        if !missing(path)? {
            if let Err(error) = fs::rename(path, stage.directory.path().join("original")) {
                failure = Some(
                    anyhow::Error::from(error).context(format!("Could not move existing {path}")),
                );
                break;
            }
            moved.push(position);
        }
        if let Err(error) = fs::rename(stage.directory.path().join("new"), path) {
            failure = Some(anyhow::Error::from(error).context(format!("Could not install {path}")));
            break;
        }
        installed.push(position);
    }
    let Some(failure) = failure else {
        for stage in stages {
            stage.directory.close()?;
        }
        return Ok(outcomes);
    };
    let mut kept = Vec::new();
    for position in installed {
        let _ = fs::remove_dir_all(&request.paths[stages[position].index]);
    }
    for (position, stage) in stages.into_iter().enumerate() {
        let path = &request.paths[stage.index];
        if moved.contains(&position)
            && fs::rename(stage.directory.path().join("original"), path).is_err()
        {
            kept.push(format!(
                "{}",
                stage.directory.keep().join("original").display()
            ));
        }
    }
    if kept.is_empty() {
        Err(failure)
    } else {
        Err(failure.context(format!("Original files retained in {}", kept.join(", "))))
    }
}

/// A lenient view of a remote definition in a producer's configuration: only the
/// fields get needs, so another tool's extra keys do not block a download.
#[derive(Default)]
pub(crate) struct RemoteView {
    pub(crate) path: Option<String>,
    pub(crate) tag: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) repo: Option<String>,
    pub(crate) archive: bool,
    pub(crate) append: bool,
    pub(crate) disabled: bool,
}

/// The definition registered as `remotes.<provider>.<title>` in `yml`.
pub(crate) fn remote_view(yml: &Value, provider: &str, title: &str) -> Option<RemoteView> {
    let value = yml.get("remotes")?.get(provider)?.get(title)?;
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(RemoteView {
        path: text("path"),
        tag: text("tag"),
        title: text("title"),
        repo: text("repo"),
        archive: text("structure").as_deref() != Some("latest"),
        append: value
            .get("path-append-label")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        disabled: value.get("source") == Some(&Value::Bool(false)),
    })
}

/// A build's recorded identity: its version and the destinations it resolved.
#[derive(Default)]
pub(crate) struct Record {
    pub(crate) version: Option<String>,
    pub(crate) uuid: Option<String>,
    /// GitHub title -> (repository, tag).
    pub(crate) github: BTreeMap<String, (Option<String>, String)>,
    /// Local title -> recorded path (relative to the project, or projr's absolute).
    pub(crate) local: BTreeMap<String, String>,
    /// The build's merged configuration (projr BUILDLOG only).
    pub(crate) yml: Option<Value>,
}

fn buildlog_versions(lines: &[&str]) -> Vec<(usize, String)> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let hashes = line.bytes().take_while(|b| *b == b'#').count();
            if !(2..=4).contains(&hashes) {
                return None;
            }
            let rest = line[hashes..].strip_prefix(' ')?;
            let (version, _) = rest.split_once(':')?;
            check_version(version)
                .ok()
                .filter(|_| version.starts_with('v'))
                .map(|_| (index, version.to_owned()))
        })
        .collect()
}

/// Reads a projr `.projr/BUILDLOG.md` entry: the newest one, or `version`'s
/// (SATVILab/projr@5f915ef `R/get-project.R:1-48`). Its YAML blocks give the
/// build's configuration and the destinations resolved from placeholders.
pub(crate) fn buildlog_record(text: &str, version: Option<&str>) -> Result<Option<Record>> {
    let lines: Vec<&str> = text.lines().collect();
    let headers = buildlog_versions(&lines);
    let position = match version {
        None => 0,
        Some(version) => match headers.iter().position(|(_, v)| v == version) {
            Some(position) => position,
            None => return Ok(None),
        },
    };
    let Some((start, found)) = headers.get(position).cloned() else {
        return Ok(None);
    };
    let end = headers
        .get(position + 1)
        .map_or(lines.len(), |(next, _)| *next);
    let entry = &lines[start..end];
    let block = |heading: &str| -> Result<Option<Value>> {
        let Some(at) = entry.iter().position(|line| *line == heading) else {
            return Ok(None);
        };
        let rest = &entry[at + 1..];
        let Some(open) = rest.iter().position(|line| *line == "```yaml") else {
            return Ok(None);
        };
        let rest = &rest[open + 1..];
        let close = rest
            .iter()
            .position(|line| *line == "```")
            .with_context(|| format!("Incomplete YAML in the build log for {found}."))?;
        Ok(Some(
            serde_yaml::from_str(&rest[..close].join("\n"))
                .with_context(|| format!("Invalid YAML in the build log for {found}."))?,
        ))
    };
    let mut record = Record {
        version: Some(found.clone()),
        yml: block("**`projr` config**")?,
        ..Default::default()
    };
    if let Some(destinations) = block("**Resolved archive destinations**")? {
        let recorded = |provider: &str| -> Vec<(String, String)> {
            destinations
                .get(provider)
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .filter_map(|(title, value)| {
                    let destination = value.get("destination")?.as_str()?;
                    Some((title.clone(), destination.to_owned()))
                })
                .collect()
        };
        for (title, tag) in recorded("github") {
            record.github.insert(title, (None, tag));
        }
        record.local.extend(recorded("local"));
    }
    Ok(Some(record))
}

/// The newest completed production build's record in a project, or `version`'s:
/// projcli attempt records first, then a projr `.projr/BUILDLOG.md`.
pub(crate) fn project_record(root: &Utf8Path, version: Option<&str>) -> Result<Record> {
    let version = match version {
        Some(version) => Some(version.to_owned()),
        None => crate::history::last_completed(root)?,
    };
    if let Some(version) = &version
        && let Some(destinations) = crate::history::recorded_destinations(root, version)?
    {
        let mut record = Record {
            version: Some(version.clone()),
            uuid: Some(destinations.uuid),
            ..Default::default()
        };
        for target in destinations.targets {
            match (target.provider.as_str(), target.tag, target.path) {
                ("github", Some(tag), _) => {
                    record.github.insert(target.title, (target.repository, tag));
                }
                ("local", _, Some(path)) => {
                    record.local.insert(target.title, path);
                }
                _ => {}
            }
        }
        return Ok(record);
    }
    let log = root.join(".projr/BUILDLOG.md");
    if crate::yml::configuration_entry_exists(&log)? {
        let text = crate::fs_utils::read_managed_text(&log)?;
        if let Some(record) = buildlog_record(&text, version.as_deref())? {
            return Ok(record);
        }
    }
    Ok(Record {
        version,
        ..Default::default()
    })
}

/// The checkout's VERSION when it names a completed (non-development) build.
pub(crate) fn completed_version_file(root: &Utf8Path) -> Option<String> {
    crate::version::version_get_from(root)
        .filter(|version| version.dev == 0)
        .map(|version| version.to_string(true))
}

/// The provider `proj get` reads from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// Another local analysis or a registered local remote.
    Local,
    /// A GitHub repository's Releases or a registered GitHub remote.
    Github,
}

/// `proj get local|github` arguments (projr `projr_get_local/github`).
pub struct GetRequest {
    /// `--path` (local) or `--repo owner/repo[@branch]` (GitHub).
    pub direct: Option<String>,
    /// A remote title in the current project's configuration.
    pub remote: Option<String>,
    /// A concrete GitHub Release tag.
    pub tag: Option<String>,
    /// Labels to copy; empty copies the labels recorded for the build.
    pub labels: Vec<String>,
    /// Exact build version.
    pub version: Option<String>,
    /// Directory receiving `<out>/<label>/`.
    pub out: Utf8PathBuf,
    /// Allow replacing changed or extra files in the selected output directories.
    pub overwrite: bool,
    /// Profiles for the current project's configuration (`--remote`).
    pub profile: Option<String>,
    /// Maximum compressed bytes per GitHub asset.
    pub max_bytes: u64,
    /// Maximum expanded bytes per GitHub ZIP.
    pub max_expanded_bytes: u64,
}

/// The verified result of `proj get`.
#[derive(Debug, Serialize)]
pub struct GetOutcome {
    /// The build version copied.
    pub version: String,
    /// Each label's final directory.
    pub labels: Vec<Installed>,
}

struct Producer {
    root: Option<Utf8PathBuf>,
    yml: Value,
    record: Record,
    version: Option<String>,
    head: Option<String>,
    repository: Option<String>,
    metadata: Option<Source>,
}

const METADATA_LIMIT: u64 = 16 * 1024 * 1024;

/// Reads a producer's metadata from one commit of a GitHub repository: its
/// configuration, BUILDLOG and the manifest indexes and snapshots for the build
/// (SATVILab/projr@5f915ef `R/get-project.R:99-230`).
fn github_producer(
    repository: &str,
    branch: Option<&str>,
    version: Option<&str>,
    labels: &[String],
    workspace: &Utf8Path,
) -> Result<Producer> {
    let client = github_client(repository)?;
    let branch = match branch {
        Some(branch) => branch.to_owned(),
        None => client.default_branch()?,
    };
    let commit = client.branch_commit(&branch)?;
    let read = |path: &str| client.file_at(path, &commit, METADATA_LIMIT);
    let log = match read(".projr/BUILDLOG.md")? {
        Some(bytes) => Some(bytes),
        None => read("BUILDLOG.md")?,
    };
    let log = String::from_utf8(log.unwrap_or_default()).context("BUILDLOG is not UTF-8")?;
    let record = buildlog_record(&log, version)?.unwrap_or(Record {
        version: version.map(str::to_owned),
        ..Default::default()
    });
    let mut config = None;
    for path in [
        "_projr.yml",
        ".projr/config/_projr.yml",
        ".projr/config/default.yml",
        "_projr/config/_projr.yml",
        "_projr/config/default.yml",
    ] {
        if let Some(bytes) = read(path)? {
            config = Some(
                serde_yaml::from_slice::<Value>(&bytes)
                    .context("Invalid producer configuration")?,
            );
            break;
        }
    }
    let yml = config.or_else(|| record.yml.clone()).with_context(|| {
        format!(
            "No projr configuration in {repository} on {branch}. Supply --tag to read a release directly."
        )
    })?;
    let mut version = version.map(str::to_owned).or(record.version.clone());
    if version.is_none()
        && let Some(bytes) = read("VERSION")?
    {
        let text = String::from_utf8_lossy(&bytes).trim().to_owned();
        let bare = text.strip_prefix('v').unwrap_or(&text);
        if !bare.is_empty()
            && bare
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        {
            version = Some(v_add(bare));
        }
    }
    let mut metadata = None;
    if let Some(version) = &version {
        let base = workspace.join("producer");
        let store = base.join("manifests");
        fs::create_dir_all(&store)?;
        let mut prefix = ".projr/manifests";
        let mut marker = read(&format!("{prefix}/FORMAT"))?;
        if marker.is_none() {
            prefix = "_projr/manifests";
            marker = read(&format!("{prefix}/FORMAT"))?;
        }
        if let Some(marker) = marker {
            fs::write(store.join("FORMAT"), marker)?;
            let directories: Vec<String> = if labels.is_empty() {
                client
                    .directories_at(prefix, &commit)?
                    .into_iter()
                    .filter(|name| {
                        !name.is_empty()
                            && name.bytes().all(|b| {
                                b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b)
                            })
                    })
                    .collect()
            } else {
                labels
                    .iter()
                    .map(|label| manifest_store::encode_component(label))
                    .collect()
            };
            let fetch = |file: &str| -> Result<()> {
                if let Some(bytes) = read(&format!("{prefix}/{file}"))? {
                    let target = store.join(file);
                    fs::create_dir_all(target.parent().context("Invalid manifest path")?)?;
                    fs::write(target, bytes)?;
                }
                Ok(())
            };
            for directory in &directories {
                fetch(&format!("{directory}/versions.csv"))?;
            }
            for entry in manifest_store::read_index(&store, None)? {
                if &entry.version == version {
                    fetch(&manifest_store::snapshot_path(&entry))?;
                }
            }
        } else if let Some(legacy) = read("manifest.csv")? {
            fs::write(base.join("manifest.csv"), legacy)?;
        }
        metadata = Some(Source {
            name: "producer".into(),
            store,
            csv: base.join("manifest.csv"),
            kind: Kind::Metadata,
        });
    }
    Ok(Producer {
        root: None,
        yml,
        record,
        version,
        head: Some(format!("ref: refs/heads/{branch}")),
        repository: Some(repository.to_owned()),
        metadata,
    })
}

fn local_producer(
    root: &Utf8Path,
    profile: Option<&str>,
    version: Option<&str>,
) -> Result<Producer> {
    let record = project_record(root, version)?;
    let version = version
        .map(str::to_owned)
        .or(record.version.clone())
        .or_else(|| completed_version_file(root));
    Ok(Producer {
        root: Some(root.to_owned()),
        yml: crate::yml::get_combined_yml(profile, root)?,
        record,
        version,
        head: crate::template::Templates::checkout(root),
        repository: None,
        metadata: None,
    })
}

/// Reads `VERSION` from a remote: an older single-version file or a `Project:` line.
fn archive_version(text: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let value = if lines.len() == 1 && !lines[0].contains(':') {
        lines[0].to_owned()
    } else {
        let projects: Vec<_> = lines
            .iter()
            .filter_map(|line| line.strip_prefix("Project:"))
            .map(str::trim)
            .collect();
        if projects.len() != 1 {
            return None;
        }
        projects[0].to_owned()
    };
    check_version(value.strip_suffix('*').unwrap_or(&value)).ok()
}

fn source_version(source: &Source, transfer: &Transfer<'_>) -> Result<Option<String>> {
    match &source.kind {
        Kind::Github {
            client,
            tag,
            assets,
            ..
        } => {
            if !assets.iter().any(|asset| asset.name == "VERSION.zip") {
                return Ok(None);
            }
            let download = tempfile::Builder::new()
                .prefix("version-")
                .tempdir_in(transfer.workspace)?;
            let zip = Utf8Path::from_path(download.path())
                .context("Workspace path must be UTF-8")?
                .join("VERSION.zip");
            client.download_asset(transfer.workspace, tag, "VERSION.zip", &zip, METADATA_LIMIT)?;
            let extracted = crate::github_zip::extract_zip(&zip, METADATA_LIMIT)?;
            let path = Utf8Path::from_path(extracted.path())
                .context("Extraction path must be UTF-8")?
                .join("VERSION");
            let text = if path.is_file() {
                Some(crate::fs_utils::read_managed_text(&path)?)
            } else {
                None
            };
            extracted.close()?;
            Ok(text.as_deref().and_then(archive_version))
        }
        Kind::Local { base, .. } => {
            let path = base.join("VERSION");
            if !crate::yml::configuration_entry_exists(&path)? {
                return Ok(None);
            }
            Ok(archive_version(&crate::fs_utils::read_managed_text(&path)?))
        }
        _ => Ok(None),
    }
}

fn labels_at(source: &Source, version: &str) -> Result<Vec<String>> {
    let mut labels: Vec<String> = Vec::new();
    let found: Vec<String> = if manifest_store::ready(&source.store)? {
        manifest_store::read_index(&source.store, None)?
            .into_iter()
            .filter(|entry| entry.version == version)
            .map(|entry| entry.label)
            .collect()
    } else {
        crate::manifest::read(&source.csv)?
            .into_iter()
            .filter(|row| row.version == version)
            .map(|row| row.label)
            .collect()
    };
    for label in found {
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    Ok(labels)
}

fn expand_home(path: &str) -> Utf8PathBuf {
    match (path.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => Utf8PathBuf::from(home).join(rest),
        _ => Utf8PathBuf::from(path),
    }
}

/// Resolve a remote's destination: a concrete tag, the build's recorded
/// destination, else its template. With `optional`, a template that needs an
/// unrecorded `@uuid`, an unknown `@version` or a missing branch is skipped
/// (SATVILab/projr@5f915ef `R/get-project.R:50-97`). Returns the target and
/// any recorded repository.
fn remote_target(
    provider: Provider,
    title: &str,
    view: Option<&RemoteView>,
    producer: &Producer,
    version: Option<&str>,
    optional: bool,
) -> Result<Option<(String, Option<String>)>> {
    match provider {
        Provider::Github => {
            if let Some((repository, tag)) = producer.record.github.get(title) {
                return Ok(Some((tag.clone(), repository.clone())));
            }
        }
        Provider::Local => {
            if let Some(path) = producer.record.local.get(title) {
                return Ok(Some((path.clone(), None)));
            }
        }
    }
    let template = match provider {
        Provider::Local => view
            .and_then(|view| view.path.clone())
            .unwrap_or_else(|| "_archive".into()),
        Provider::Github => view
            .and_then(|view| view.tag.clone().or(view.title.clone()))
            .unwrap_or_else(|| title.to_owned()),
    };
    let templates = crate::template::Templates::for_record(
        version.map(str::to_owned),
        producer.head.clone(),
        producer.record.uuid.clone(),
    );
    match templates.expand(&template) {
        Ok(target) if provider == Provider::Github => {
            Ok(Some((crate::github_publication::tag_format(&target), None)))
        }
        Ok(target) => Ok(Some((target, None))),
        Err(_) if optional => Ok(None),
        Err(error) => Err(error.context(format!("Cannot resolve the destination of {title}"))),
    }
}

fn repository_name(text: &str) -> bool {
    let parts: Vec<_> = text.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && *part != "."
                && *part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        })
}

/// Copies directories from another build into `<out>/<label>/` after verifying
/// every file against that build's manifest (projr `projr_get_local` and
/// `projr_get_github`, SATVILab/projr@5f915ef `R/get.R`).
///
/// A local `--path` must be the producing analysis's root; its own directories
/// and registered local remotes (plus the default `_archive`) are searched. A
/// GitHub `--repo` reads the producer's configuration, BUILDLOG and manifests
/// from one commit of its branch, then its registered GitHub remotes (plus
/// `archive`). `--remote` names a remote in the current project; `--tag` reads a
/// Release directly. Without `--version`, the producer's completed build (or the
/// remote's advertised project version) is used.
pub fn get(root: &Utf8Path, provider: Provider, request: &GetRequest) -> Result<GetOutcome> {
    let direct_flag = if provider == Provider::Local {
        "--path"
    } else {
        "--repo"
    };
    anyhow::ensure!(
        request.direct.is_some() != request.remote.is_some(),
        "Supply exactly one of {direct_flag} and --remote."
    );
    anyhow::ensure!(
        provider == Provider::Github || request.tag.is_none(),
        "--tag applies only to GitHub"
    );
    for label in &request.labels {
        crate::path_safety::validate_directory_label(label)?;
    }
    let version = request.version.as_deref().map(check_version).transpose()?;
    let (_guard, workspace) = workspace()?;
    let transfer = Transfer {
        workspace: &workspace,
        max_bytes: request.max_bytes,
        max_expanded_bytes: request.max_expanded_bytes,
    };
    let mut producer = if request.remote.is_some() {
        let profiles = crate::run_context::select_profiles(request.profile.as_deref(), root)?;
        local_producer(root, Some(&profiles.join(",")), version.as_deref())?
    } else if provider == Provider::Local {
        let direct = request.direct.as_deref().unwrap_or_default();
        let path = Utf8PathBuf::try_from(
            fs::canonicalize(direct).context("Cannot find the producing analysis")?,
        )
        .context("Producer path must be UTF-8")?;
        anyhow::ensure!(
            crate::ignore::root_from(&path).as_deref() == Some(path.as_path()),
            "--path must be the root of the producing projr analysis."
        );
        local_producer(&path, None, version.as_deref())?
    } else {
        let direct = request.direct.as_deref().unwrap_or_default();
        let (repository, branch) = match direct.split_once('@') {
            Some((repository, branch)) => (repository, Some(branch)),
            None => (direct, None),
        };
        anyhow::ensure!(
            repository_name(repository)
                && branch.is_none_or(|b| !b.is_empty() && !b.chars().any(char::is_whitespace)),
            "--repo must be owner/repo or owner/repo@branch."
        );
        if request.tag.is_none() {
            github_producer(
                repository,
                branch,
                version.as_deref(),
                &request.labels,
                &workspace,
            )?
        } else {
            Producer {
                root: None,
                yml: Value::Null,
                record: Record::default(),
                version: None,
                head: None,
                repository: Some(repository.to_owned()),
                metadata: None,
            }
        }
    };
    let direct_source = request.remote.is_none() && request.tag.is_none();
    let mut version = if direct_source {
        version.or(producer.version.clone())
    } else {
        version
    };
    let mut sources = Vec::new();
    if provider == Provider::Local
        && request.remote.is_none()
        && let Some(producer_root) = &producer.root
    {
        let config = crate::restore::projected_config(&producer.yml, producer_root)?;
        let mut dirs = BTreeMap::new();
        for label in config.directories.keys() {
            dirs.insert(label.clone(), config.get_path(producer_root, label)?);
        }
        sources.push(Source {
            name: "project".into(),
            store: manifest_store::project_store(producer_root),
            csv: producer_root.join("manifest.csv"),
            kind: Kind::Project {
                root: producer_root.clone(),
                dirs,
            },
        });
    }
    let has_metadata = producer.metadata.is_some();
    if let Some(metadata) = producer.metadata.take() {
        sources = vec![metadata];
    }
    let provider_key = if provider == Provider::Local {
        "local"
    } else {
        "github"
    };
    let yml = producer.record.yml.as_ref().unwrap_or(&producer.yml);
    let titles: Vec<String> = if let Some(remote) = &request.remote {
        let bare = match remote.split_once(':') {
            Some((kind, rest)) if kind == provider_key && !rest.is_empty() => rest,
            _ => remote.as_str(),
        };
        vec![bare.to_owned()]
    } else if let Some(tag) = &request.tag {
        vec![tag.clone()]
    } else {
        let mut titles: Vec<String> = Vec::new();
        for value in [yml, &producer.yml] {
            for title in value
                .get("remotes")
                .and_then(|v| v.get(provider_key))
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|map| map.keys())
            {
                if !titles.contains(title) {
                    titles.push(title.clone());
                }
            }
        }
        if !titles.iter().any(|title| title == "archive") {
            titles.push("archive".into());
        }
        titles
    };
    for title in titles {
        let view = if request.tag.is_some() && request.remote.is_none() {
            None
        } else {
            remote_view(yml, provider_key, &title)
                .or_else(|| remote_view(&producer.yml, provider_key, &title))
        };
        anyhow::ensure!(
            view.is_some() || request.remote.is_none(),
            "Unknown {provider_key} remote: {title}"
        );
        if direct_source && view.as_ref().is_some_and(|view| view.disabled) {
            continue;
        }
        let target = match (&request.tag, &request.remote) {
            (Some(tag), None) => Some((tag.clone(), None)),
            _ => remote_target(
                provider,
                &title,
                view.as_ref(),
                &producer,
                version.as_deref().or(producer.version.as_deref()),
                direct_source,
            )?,
        };
        let Some((target, recorded_repository)) = target else {
            continue;
        };
        let view = view.unwrap_or(RemoteView {
            archive: true,
            append: true,
            ..Default::default()
        });
        match provider {
            Provider::Local => {
                let path = expand_home(&target);
                let base = if path.is_absolute() {
                    path
                } else {
                    producer.root.as_deref().unwrap_or(root).join(path)
                };
                let source =
                    local_source(format!("local:{title}"), base, view.archive, view.append);
                if manifest_store::ready(&source.store)? || !missing(&source.csv)? || has_metadata {
                    sources.push(source);
                }
            }
            Provider::Github => {
                let repository = match view
                    .repo
                    .clone()
                    .or(recorded_repository)
                    .or(producer.repository.clone())
                {
                    Some(repository) => repository,
                    None => crate::restore::repository_in(
                        root,
                        None,
                        &github_api(),
                        &crate::run_environment::RunEnvironment::inherited(),
                    )?,
                };
                anyhow::ensure!(
                    repository_name(&repository),
                    "Invalid GitHub repository: {repository}"
                );
                if let Some(source) = github_source(
                    GithubTarget {
                        repository,
                        tag: target,
                        prefix: view.path.clone(),
                        append: view.append,
                        archive: view.archive,
                        direct_tag: request.tag.is_some() && request.remote.is_none(),
                    },
                    &transfer,
                )? {
                    sources.push(source);
                }
            }
        }
    }
    anyhow::ensure!(
        !sources.is_empty(),
        "No published manifest found for the selected source."
    );
    if version.is_none() {
        version = source_version(&sources[0], &transfer)?;
    }
    let version =
        version.context("No completed build recorded at this source. Supply --version.")?;
    let mut labels = request.labels.clone();
    if labels.is_empty() {
        for source in sources
            .iter()
            .filter(|source| !matches!(source.kind, Kind::Metadata))
        {
            for label in labels_at(source, &version)? {
                if !labels.contains(&label) {
                    labels.push(label);
                }
            }
        }
        if has_metadata {
            let metadata = &sources[0];
            for label in labels_at(metadata, &version)? {
                if labels.contains(&label) {
                    continue;
                }
                let Some(snap) = snapshot(&metadata.store, &metadata.csv, &label, &version)? else {
                    continue;
                };
                let mut present = false;
                for source in &sources[1..] {
                    present |=
                        !matching_versions(&source.store, &label, snap.id.as_deref())?.is_empty();
                }
                if !present {
                    for source in &sources[1..] {
                        for other in &snap.versions {
                            present |= labels_at(source, other)?.contains(&label);
                        }
                    }
                }
                if present {
                    labels.push(label);
                }
            }
        }
    }
    anyhow::ensure!(!labels.is_empty(), "No published labels for {version}.");
    for label in &labels {
        crate::path_safety::validate_directory_label(label)?;
    }
    fs::create_dir_all(&request.out).with_context(|| format!("Cannot create {}", request.out))?;
    let out = Utf8PathBuf::try_from(fs::canonicalize(&request.out)?)
        .context("Output path must be UTF-8")?;
    let paths: Vec<Utf8PathBuf> = labels.iter().map(|label| out.join(label)).collect();
    let mut protected = Vec::new();
    for source in &sources {
        match &source.kind {
            Kind::Local { base, .. } => protected.push(base.clone()),
            Kind::Project { root, dirs } => {
                protected.push(root.clone());
                protected.extend(dirs.values().cloned());
            }
            _ => {}
        }
    }
    check_destinations(None, &paths, &protected)?;
    let labels = install(
        Install {
            sources: &sources,
            labels: &labels,
            paths: &paths,
            version: &version,
            overwrite: request.overwrite,
            option: "--overwrite",
        },
        &transfer,
    )?;
    Ok(GetOutcome { version, labels })
}
