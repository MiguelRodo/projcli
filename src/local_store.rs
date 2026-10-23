//! Explicit file-inspected local transfer policies; no remote metadata policy.
use std::{collections::BTreeMap, fs};
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

type Inventory = BTreeMap<String, String>;

/// Existing projr transfer strategies, applied to one explicit physical target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StoreStrategy {
    /// Upload every source file, retaining destination-only files.
    UploadAll,
    /// Upload source files absent from the destination; do not overwrite.
    UploadMissing,
    /// Upload new/different files and remove destination-only files.
    SyncDiff,
    /// Purge the target file set and upload every source file.
    SyncPurge,
}

/// A file-byte comparison and its requested transfer actions.
/// Same-byte paths are informational and may still be uploaded by upload-all
/// or sync-purge. A dry run returns applied=false without creating directories.
#[derive(Debug, Serialize)]
pub struct FileStorePlan {
    /// Explicit source directory.
    pub source: Utf8PathBuf,
    /// Concrete destination directory, not an inferred remote base/version.
    pub destination: Utf8PathBuf,
    /// Requested transfer policy.
    pub strategy: StoreStrategy,
    /// Every source file selected for upload, including overwrites.
    pub uploads: Vec<String>,
    /// Subset of uploads replacing existing destination files.
    pub overwrites: Vec<String>,
    /// Files to remove; purge lists every prior destination file.
    pub removals: Vec<String>,
    /// Paths whose existing and source MD5 bytes agree.
    pub same_bytes: Vec<String>,
    /// Whether the transfer completed; false for dry run.
    pub applied: bool,
}

pub(crate) fn actions(source: &Inventory, destination: &Inventory, strategy: StoreStrategy) -> (Vec<String>, Vec<String>, Vec<String>, Vec<String>) {
    let uploads: Vec<_> = source.iter().filter(|(path, hash)| match strategy {
        StoreStrategy::UploadAll | StoreStrategy::SyncPurge => true,
        StoreStrategy::UploadMissing => !destination.contains_key(*path),
        StoreStrategy::SyncDiff => destination.get(*path) != Some(*hash),
    }).map(|(path, _)| path.clone()).collect();
    let overwrites = uploads.iter().filter(|path| destination.contains_key(*path)).cloned().collect();
    let removals = destination.keys().filter(|path| match strategy {
        StoreStrategy::SyncPurge => true,
        StoreStrategy::SyncDiff => !source.contains_key(*path),
        _ => false,
    }).cloned().collect();
    let same = source.iter().filter(|(path, hash)| destination.get(*path) == Some(*hash))
        .map(|(path, _)| path.clone()).collect();
    (uploads, overwrites, removals, same)
}
fn inventory(path: &Utf8Path) -> anyhow::Result<Inventory> {
    Ok(crate::manifest::inventory_directory(path, false)?.into_iter().collect())
}

// Resolve against native directory entries without creating a probe file.
// Ambiguous case/normalisation/short-name aliases must not masquerade as missing.
/// Plans or executes one explicit physical-directory transfer with MD5 file
/// inspection. The default CLI policy is projr's sync-diff, but file inspection
/// must be explicitly selected. No YAML defaults or registry semantics change.
/// Does not write VERSION/CSV, choose archive versions, honour configured cues
/// or publish a build. Those integrations remain separate.
///
/// Requires a real source and an existing destination parent. Guards are shared
/// with local downloads. Complete byte inventories are compared before apply;
/// selected source copies are checked against the planned hashes. Private staging
/// precedes destination replacement. Existing permission flags survive for paths
/// retained with the same file/directory type. Ownership/ACL/xattrs and concurrent
/// writer/crash atomicity are not guaranteed. Post-install cleanup failure can
/// leave partial residue and is reported as an installed transfer, not rollback.
pub fn store_files(root: &Utf8Path, source: &Utf8Path, destination: &Utf8Path, strategy: StoreStrategy, dry_run: bool) -> anyhow::Result<FileStorePlan> {
    crate::path_safety::resolve_path(source.as_std_path(), true)?;
    anyhow::ensure!(fs::symlink_metadata(source)?.is_dir(), "Store source must be a real directory: {source}");
    crate::local_remote::validate_download_destination(root, source, destination)?;
    let parent = destination.parent().context("Store destination has no parent")?;
    crate::path_safety::resolve_path(parent.as_std_path(), true)?;
    anyhow::ensure!(fs::symlink_metadata(parent)?.is_dir(), "Store destination parent must already exist: {parent}");
    let original_source = inventory(source)?;
    let original_destination = inventory(destination)?;
    let (uploads, overwrites, removals, same_bytes) = actions(&original_source, &original_destination, strategy);
    let mirror = matches!(strategy, StoreStrategy::SyncDiff | StoreStrategy::SyncPurge);
    for name in &uploads {
        crate::path_safety::validate_native_names(destination, name, mirror)?;
        let target = destination.join(name);
        match fs::symlink_metadata(&target) {
            Ok(metadata) => anyhow::ensure!(mirror || metadata.is_file(), "Store file conflicts with destination directory: {target}"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
                || (mirror && error.kind() == std::io::ErrorKind::NotADirectory) => {}
            Err(error) => return Err(error).with_context(|| format!("Cannot inspect store upload destination {target}")),
        }
    }
    let mut plan = FileStorePlan { source: source.to_owned(), destination: destination.to_owned(),
        strategy, uploads, overwrites, removals, same_bytes, applied: false };
    if dry_run { return Ok(plan); }
    if plan.uploads.is_empty() && plan.removals.is_empty() && destination.exists() {
        plan.applied = true;
        return Ok(plan);
    }
    let staging = tempfile::Builder::new().prefix(".proj-store-source-").tempdir_in(parent)?;
    let staged = Utf8PathBuf::try_from(staging.path().to_owned())?;
    let execution: anyhow::Result<()> = (|| {
    let selected: Inventory = if mirror { original_source.clone() } else {
        plan.uploads.iter().map(|path| (path.clone(), original_source[path].clone())).collect()
    };
    for name in selected.keys() {
        let target = staged.join(name);
        fs::create_dir_all(target.parent().context("Store upload has no parent")?)?;
        fs::copy(source.join(name), &target).with_context(|| format!("Cannot stage store upload {name}"))?;
    }
    anyhow::ensure!(inventory(&staged)? == selected && inventory(source)? == original_source,
        "Store source bytes changed while staging");
    anyhow::ensure!(inventory(destination)? == original_destination, "Store destination bytes changed while staging");
    let mut permissions = BTreeMap::new();
    crate::local_remote::download_permissions(source, Utf8Path::new(""), &mut permissions)?;
    if mirror && destination.exists() {
        let mut previous = BTreeMap::new();
        crate::local_remote::download_permissions(destination, Utf8Path::new(""), &mut previous)?;
        for (relative, mode) in previous {
            if let Ok(staged_metadata) = fs::symlink_metadata(staged.join(&relative)) {
                if fs::symlink_metadata(destination.join(&relative))?.is_dir() == staged_metadata.is_dir() {
                    permissions.insert(relative, mode);
                }
            }
        }
    }
    let mut permissions: Vec<_> = permissions.into_iter().collect();
    permissions.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    for (relative, mode) in permissions {
        let target = staged.join(relative);
        if target.exists() { fs::set_permissions(target, mode)?; }
    }
    crate::local_remote::install_directory(root, &staged, destination, mirror)?;
        Ok(())
    })();
    let cleanup_path = staging.path().to_owned();
    let cleanup = staging.close();
    match (execution, cleanup) {
        (Err(error), Ok(())) => return Err(error),
        (Err(error), Err(cleanup)) => return Err(error).context(format!("Store-source staging cleanup also failed at {}: {cleanup}", cleanup_path.display())),
        (Ok(()), Err(error)) => return Err(error).with_context(|| format!("File transfer installed at {destination}, but source-staging cleanup failed at {}", cleanup_path.display())),
        (Ok(()), Ok(())) => {}
    }
    plan.applied = true;
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn four_policies_have_distinct_explicit_file_actions() {
        let source = BTreeMap::from([("new".into(),"n".into()), ("changed".into(),"new".into()), ("same".into(),"same".into())]);
        let destination = BTreeMap::from([("old".into(),"o".into()), ("changed".into(),"old".into()), ("same".into(),"same".into())]);
        for (strategy, upload, replace, remove) in [
            (StoreStrategy::UploadAll, vec!["changed","new","same"], vec!["changed","same"], vec![]),
            (StoreStrategy::UploadMissing, vec!["new"], vec![], vec![]),
            (StoreStrategy::SyncDiff, vec!["changed","new"], vec!["changed"], vec!["old"]),
            (StoreStrategy::SyncPurge, vec!["changed","new","same"], vec!["changed","same"], vec!["changed","old","same"]),
        ] {
            let (a,b,c,same) = actions(&source,&destination,strategy);
            assert_eq!(a,upload); assert_eq!(b,replace); assert_eq!(c,remove); assert_eq!(same,vec!["same"]);
        }
    }
}
