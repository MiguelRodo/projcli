//! Verified local Xet snapshots; existing file-CAS formats and IDs remain unchanged.
//! Uses the existing flat inventory and directory-tree codecs, plus backend addresses.
//! Whole-file BLAKE3 and length are checked independently of upstream's download result.
use crate::snapshot_format::{DirectoryRoot, FlatSnapshot, MAX_METADATA_BYTES, TreeSnapshot};
pub use crate::xet_engine::{BACKEND, Engine, Pointer, UPSTREAM};
use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
};
const SCHEMA: &str = "projcli.xet-snapshot.v1";

/// Immutable Xet binding over the same complete readable inventory used elsewhere.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    /// Separate format namespace; legacy CAS metadata is never reinterpreted.
    pub schema: String,
    /// Pinned local engine/storage compatibility domain.
    pub backend: String,
    /// Exact upstream format producer required by this reader.
    pub upstream: String,
    /// Complete logical-label/path/BLAKE3 inventory, without a competing file list.
    pub inventory: FlatSnapshot,
    /// Existing pure directory roots, independent of version and Xet pack layout.
    pub roots: TreeSnapshot,
    /// Full-file BLAKE3 to upstream reconstruction address and trusted length.
    pub payloads: BTreeMap<String, Pointer>,
}
impl Snapshot {
    fn canonical(&self) -> Result<Self> {
        anyhow::ensure!(
            self.schema == SCHEMA && self.backend == BACKEND && self.upstream == UPSTREAM,
            "Unsupported Xet snapshot/backend; no implicit upgrade"
        );
        let inventory = FlatSnapshot::parse(&self.inventory.readable_json()?)?;
        anyhow::ensure!(
            !inventory.directories.is_empty(),
            "Xet snapshot needs a selected directory"
        );
        let forest = crate::ordinary_tree::Forest::build(&inventory)?;
        let roots = TreeSnapshot::new(
            inventory.version.clone(),
            inventory.created_at.clone(),
            forest
                .roots
                .iter()
                .map(|(label, (tree, _))| DirectoryRoot {
                    label: label.clone(),
                    tree: tree.clone(),
                })
                .collect(),
        )?;
        anyhow::ensure!(
            self.roots == roots,
            "Xet roots disagree with complete inventory"
        );
        let hashes: BTreeSet<_> = inventory
            .directories
            .iter()
            .flat_map(|d| d.files.iter().map(|f| &f.hash))
            .collect();
        anyhow::ensure!(
            hashes.len() == self.payloads.len(),
            "Xet payload membership differs from inventory"
        );
        for hash in hashes {
            let pointer = self
                .payloads
                .get(hash)
                .context("Missing Xet payload binding")?;
            pointer.validate()?;
        }
        let mut result = self.clone();
        result.inventory = inventory;
        result.roots = roots;
        Ok(result)
    }
    /// Canonical backend-specific snapshot identity; not an upstream Xet or legacy CAS ID.
    pub fn identity(&self) -> Result<String> {
        let mut hash = blake3::Hasher::new_derive_key(SCHEMA);
        hash.update(&serde_json::to_vec(&self.canonical()?)?);
        Ok(hash.finalize().to_hex().to_string())
    }
    /// Bounded complete JSON. Cosmetic formatting is outside canonical identity.
    pub fn readable_json(&self) -> Result<Vec<u8>> {
        let bytes = serde_json::to_vec_pretty(&self.canonical()?)?;
        anyhow::ensure!(
            bytes.len() <= MAX_METADATA_BYTES,
            "Xet metadata exceeds limit"
        );
        Ok(bytes)
    }
}
fn physical(path: &Utf8Path) -> Result<Utf8PathBuf> {
    Ok(Utf8PathBuf::try_from(crate::path_safety::resolve_path(
        path.as_std_path(),
        true,
    )?)?)
}
fn storage(root: &Utf8Path, store: &Utf8Path) -> Result<Utf8PathBuf> {
    anyhow::ensure!(
        root.is_absolute() && store.is_absolute(),
        "Xet project/store paths must be absolute"
    );
    let store = physical(store)?;
    crate::cas_tree::project_target(root, &store, false)?;
    anyhow::ensure!(
        !crate::path_safety::portable_paths_overlap(&store, &physical(&root.join(".proj"))?),
        "Xet store overlaps project state"
    );
    crate::cas::checked_directory(&store)?;
    Ok(store)
}
fn snapshot_path(store: &Utf8Path, id: &str) -> Result<Utf8PathBuf> {
    crate::cas::validate_digest(id)?;
    crate::cas_tree::native(
        store,
        &format!("xet-snapshots/v1/{}/{}.json", &id[..2], &id[2..]),
    )
}
/// Reject links/devices in the engine-owned tree before allowing native library access.
/// Bounds prevent malicious imported directory graphs from becoming unbounded traversals.
pub(crate) fn guard_tree(root: &Utf8Path) -> Result<()> {
    #[cfg(not(unix))]
    {
        let _ = root;
        anyhow::bail!("Experimental Xet store access currently requires Unix hard-link validation");
    }
    #[cfg(unix)]
    {
        fn visit(path: &Utf8Path, depth: usize, work: &mut usize) -> Result<()> {
            use std::os::unix::fs::MetadataExt;
            anyhow::ensure!(
                depth <= 64 && *work <= 100_000,
                "Xet store tree exceeds bounds"
            );
            crate::path_safety::resolve_path(path.as_std_path(), true)?;
            let meta = fs::symlink_metadata(path)?;
            anyhow::ensure!(
                !meta.is_file() || meta.nlink() == 1,
                "Xet mutable store contains a multiply-linked file"
            );
            anyhow::ensure!(
                meta.is_file() || meta.is_dir(),
                "Xet store contains a linked/special entry"
            );
            if meta.is_dir() {
                for entry in fs::read_dir(path)? {
                    *work += 1;
                    visit(&Utf8PathBuf::try_from(entry?.path())?, depth + 1, work)?;
                }
            }
            Ok(())
        }
        if crate::yml::configuration_entry_exists(root)? {
            visit(root, 0, &mut 0)?;
        }
        Ok(())
    }
}

fn marker(store: &Utf8Path, create: bool) -> Result<()> {
    let path = crate::cas_tree::native(store, "xet-format.json")?;
    let bytes = serde_json::to_vec(
        &serde_json::json!({"schema":"projcli.xet-store.v1","backend":BACKEND,"upstream":UPSTREAM}),
    )?;
    if crate::yml::configuration_entry_exists(&path)? {
        anyhow::ensure!(
            crate::fs_utils::read_managed_bounded(&path, 16384)? == bytes,
            "Unsupported Xet store version; never upgrade implicitly"
        );
    } else {
        anyhow::ensure!(create, "Missing Xet store format marker");
        // Refuse to adopt arbitrary pre-existing upstream data without an explicit migration.
        anyhow::ensure!(
            !crate::yml::configuration_entry_exists(&store.join("engine"))?
                && !crate::yml::configuration_entry_exists(&store.join("xet-snapshots"))?,
            "Unmarked Xet data requires explicit migration"
        );
        for entry in fs::read_dir(store)? {
            let name = entry?.file_name();
            anyhow::ensure!(
                name == ".proj",
                "Unmarked nonempty store requires an explicit migration"
            );
        }
        crate::cas::write_metadata(&path, &bytes, true)?;
    }
    Ok(())
}
/// Validate existing format/type boundaries before hooks without creating/opening the engine.
pub(crate) fn preflight(project: &Utf8Path, store: &Utf8Path) -> Result<()> {
    let store = storage(project, store)?;
    guard_tree(&store)?;
    if crate::yml::configuration_entry_exists(&store.join("xet-format.json"))? {
        marker(&store, false)?;
    } else if crate::yml::configuration_entry_exists(&store)? {
        for entry in fs::read_dir(&store)? {
            anyhow::ensure!(
                entry?.file_name() == ".proj",
                "Unmarked nonempty Xet store requires explicit migration"
            );
        }
    }
    Ok(())
}

fn verify_file(path: &Utf8Path, hash: &str, length: u64) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() == length,
        "Xet reconstructed file length/type mismatch"
    );
    anyhow::ensure!(
        crate::cas::hash_file(path)? == hash,
        "Xet reconstructed file digest mismatch; destination preserved"
    );
    Ok(())
}
fn reconstruct(
    engine: &Engine,
    store: &Utf8Path,
    hash: &str,
    pointer: &Pointer,
    target: &Utf8Path,
) -> Result<()> {
    engine.download(&store.join("engine"), pointer, target)?;
    verify_file(target, hash, pointer.file_size)
}
/// Read and validate an explicit immutable backend binding; no helper or data mutation.
/// A successful metadata read alone does not establish payload integrity.
pub fn read_snapshot(project: &Utf8Path, store: &Utf8Path, id: &str) -> Result<Snapshot> {
    let store = storage(project, store)?;
    marker(&store, false)?;
    let bytes = crate::fs_utils::read_managed_bounded(
        &snapshot_path(&store, id)?,
        MAX_METADATA_BYTES as u64,
    )?;
    let snapshot: Snapshot = serde_json::from_slice(&bytes)?;
    let snapshot = snapshot.canonical()?;
    let forest = crate::ordinary_tree::Forest::build(&snapshot.inventory)?;
    let mut graph = crate::ordinary_tree::Verification::default();
    for (tree, expected) in forest.roots.values() {
        anyhow::ensure!(
            &graph.inventory(tree, |id| crate::ordinary_tree::read(&store, id))? == expected,
            "Stored Xet directory tree disagrees with inventory"
        );
    }
    anyhow::ensure!(snapshot.identity()? == id, "Xet snapshot identity mismatch");
    Ok(snapshot)
}
/// Ingest selected label roots with cooperative single-writer exclusion.
/// Source names/types/complete inventories are validated before engine writes. Each file is
/// copied+hashed into private staging, finalized by upstream, then reconstructed and freshly
/// checked before any complete snapshot reference is committed. No full raw blobs are retained.
/// Failure can leave unreferenced chunks, never a new successful reference. No GC/power-loss
/// or atomic live-dataset claim; source changes between observation/copy fail explicitly.
pub fn publish(
    project: &Utf8Path,
    store: &Utf8Path,
    sources: &[(String, Utf8PathBuf)],
    version: &str,
    engine: &Engine,
) -> Result<String> {
    let store = storage(project, store)?;
    let mut directories = Vec::new();
    for (label, source) in sources {
        anyhow::ensure!(source.is_absolute(), "Xet source paths must be absolute");
        let source = physical(source)?;
        anyhow::ensure!(
            !crate::path_safety::portable_paths_overlap(&source, &store),
            "Xet source/store overlap"
        );
        crate::cas_tree::preflight_storage(project, &store, &store, std::slice::from_ref(&source))?;
        directories.push(crate::cas_tree::observe_directory(&source, label)?);
    }
    let inventory = FlatSnapshot::new(version.into(), None, directories)?;
    anyhow::ensure!(
        !inventory.directories.is_empty(),
        "Xet publication requires selected directories"
    );
    guard_tree(&store)?;
    fs::create_dir_all(&store)?;
    let _claim = crate::attempt::Attempt::acquire(&store)?;
    marker(&store, true)?;
    guard_tree(&store)?;
    let work = crate::store_archive::scratch()?;
    let stage = Utf8Path::from_path(work.path()).context("Xet staging path is not UTF-8")?;
    let mut payloads = BTreeMap::new();
    for directory in &inventory.directories {
        let source = sources
            .iter()
            .find(|(label, _)| label == &directory.label)
            .context("Xet source disappeared")?
            .1
            .clone();
        for file in &directory.files {
            if payloads.contains_key(&file.hash) {
                continue;
            }
            let path = source.join(&file.path);
            crate::path_safety::resolve_path(path.as_std_path(), true)?;
            anyhow::ensure!(
                fs::symlink_metadata(&path)?.is_file(),
                "Xet source is not a regular file"
            );
            let input = stage.join("input");
            let output = stage.join("output");
            let mut reader = fs::File::open(&path)?;
            anyhow::ensure!(
                reader.metadata()?.is_file(),
                "Xet input handle is not regular"
            );
            let mut writer = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&input)?;
            let mut hash = blake3::Hasher::new();
            let mut buffer = [0u8; 65536];
            let mut length = 0u64;
            loop {
                let n = reader.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
                writer.write_all(&buffer[..n])?;
                length += n as u64;
            }
            writer.sync_all()?;
            drop(writer);
            anyhow::ensure!(
                hash.finalize().to_hex().as_str() == file.hash,
                "Xet source changed after inventory observation"
            );
            let pointer = engine.upload(&store.join("engine"), &input)?;
            anyhow::ensure!(
                pointer.file_size == length,
                "Xet uploaded pointer length mismatch"
            );
            reconstruct(engine, &store, &file.hash, &pointer, &output)?;
            fs::remove_file(input)?;
            fs::remove_file(output)?;
            payloads.insert(file.hash.clone(), pointer);
        }
    }
    let forest = crate::ordinary_tree::Forest::build(&inventory)?;
    let roots = TreeSnapshot::new(
        version.into(),
        None,
        forest
            .roots
            .iter()
            .map(|(label, (tree, _))| DirectoryRoot {
                label: label.clone(),
                tree: tree.clone(),
            })
            .collect(),
    )?;
    let snapshot = Snapshot {
        schema: SCHEMA.into(),
        backend: BACKEND.into(),
        upstream: UPSTREAM.into(),
        inventory,
        roots,
        payloads,
    };
    let id = snapshot.identity()?;
    forest.publish(project, &store)?;
    crate::cas::write_metadata(
        &snapshot_path(&store, &id)?,
        &snapshot.readable_json()?,
        true,
    )?;
    Ok(id)
}
/// Reconstruct every unique file to private scratch and independently check BLAKE3/length.
/// A writer claim excludes mutation during validation; even reads can update upstream caches.
/// No caller destination is touched. Upstream panic/nonzero exits become ordinary errors.
pub fn verify(project: &Utf8Path, store: &Utf8Path, id: &str, engine: &Engine) -> Result<Snapshot> {
    let store = storage(project, store)?;
    guard_tree(&store)?;
    let _claim = crate::attempt::Attempt::acquire(&store)?;
    let snapshot = read_snapshot(project, &store, id)?;
    let temp = crate::store_archive::scratch()?;
    let stage = Utf8Path::from_path(temp.path()).context("Xet scratch is not UTF-8")?;
    for (hash, pointer) in &snapshot.payloads {
        let target = stage.join("file");
        reconstruct(engine, &store, hash, pointer, &target)?;
        fs::remove_file(target)?;
    }
    Ok(snapshot)
}
/// Restore one exact label, using private reconstruction, independent byte verification and
/// the existing checked installer. Missing/corrupt content never replaces a good destination.
/// Only the selected label's payloads are reconstructed. clear=false preserves extra files;
/// selected files and directories still verify after private merge, before final replacement.
pub fn restore(
    project: &Utf8Path,
    store: &Utf8Path,
    id: &str,
    label: &str,
    destination: &Utf8Path,
    clear: bool,
    engine: &Engine,
) -> Result<Snapshot> {
    let store = storage(project, store)?;
    let destination = physical(destination)?;
    crate::cas_tree::project_target(project, &destination, false)?;
    anyhow::ensure!(
        !crate::path_safety::portable_paths_overlap(&store, &destination),
        "Xet restore overlaps store"
    );
    guard_tree(&store)?;
    let _claim = crate::attempt::Attempt::acquire(&store)?;
    let snapshot = read_snapshot(project, &store, id)?;
    let inventory = snapshot
        .inventory
        .directories
        .iter()
        .find(|d| d.label == label)
        .context("Xet snapshot has no requested label")?;
    let parent = destination
        .parent()
        .context("Xet destination has no parent")?;
    let temp = tempfile::Builder::new()
        .prefix(".proj-xet-restore-")
        .tempdir_in(parent)?;
    let staged = Utf8PathBuf::try_from(temp.path().join("selected"))?;
    fs::create_dir(&staged)?;
    for directory in &inventory.empty_directories {
        fs::create_dir_all(staged.join(directory))?;
    }
    for file in &inventory.files {
        let target = staged.join(&file.path);
        fs::create_dir_all(target.parent().context("Xet file has no parent")?)?;
        reconstruct(
            engine,
            &store,
            &file.hash,
            &snapshot.payloads[&file.hash],
            &target,
        )?;
    }
    crate::local_remote::install_directory_checked(
        project,
        &staged,
        &destination,
        clear,
        |stage| {
            let actual = crate::cas_tree::observe_directory(stage, label)?;
            if clear {
                anyhow::ensure!(&actual == inventory, "Xet installed inventory mismatch");
            } else {
                for file in &inventory.files {
                    anyhow::ensure!(
                        actual.files.contains(file),
                        "Xet merged file missing/corrupt"
                    );
                }
                for directory in &inventory.empty_directories {
                    anyhow::ensure!(
                        stage.join(directory).is_dir(),
                        "Xet merged directory missing"
                    );
                }
            }
            Ok(())
        },
    )?;
    crate::fs_utils::close_owned_staging(temp)?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot_format::{FileRecord, FlatDirectory};
    fn snapshot() -> Snapshot {
        let inventory = FlatSnapshot::new(
            "v1.2.3".into(),
            None,
            vec![FlatDirectory {
                label: "raw-data".into(),
                files: vec![FileRecord {
                    path: "file".into(),
                    hash: "0".repeat(64),
                }],
                empty_directories: vec![],
            }],
        )
        .unwrap();
        let forest = crate::ordinary_tree::Forest::build(&inventory).unwrap();
        let roots = TreeSnapshot::new(
            inventory.version.clone(),
            None,
            forest
                .roots
                .iter()
                .map(|(label, (tree, _))| DirectoryRoot {
                    label: label.clone(),
                    tree: tree.clone(),
                })
                .collect(),
        )
        .unwrap();
        Snapshot {
            schema: SCHEMA.into(),
            backend: BACKEND.into(),
            upstream: UPSTREAM.into(),
            inventory,
            roots,
            payloads: BTreeMap::from([(
                "0".repeat(64),
                Pointer {
                    hash: "1".repeat(64),
                    file_size: 4,
                    sha256: Some("2".repeat(64)),
                },
            )]),
        }
    }
    #[test]
    fn identities_separate_backend_pointers_and_trusted_content() {
        let value = snapshot();
        let id = value.identity().unwrap();
        let bytes = value.readable_json().unwrap();
        assert_eq!(
            serde_json::from_slice::<Snapshot>(&bytes)
                .unwrap()
                .identity()
                .unwrap(),
            id
        );
        let mut changed = value.clone();
        changed.payloads.get_mut(&"0".repeat(64)).unwrap().hash = "3".repeat(64);
        assert_ne!(changed.identity().unwrap(), id);
        assert_eq!(changed.roots, value.roots); // same content root, different backend address
    }
    #[test]
    fn reject_unknown_backend_bad_membership_and_contradictory_roots() {
        let value = snapshot();
        let mut changed = value.clone();
        changed.upstream = "latest".into();
        assert!(changed.identity().is_err());
        let mut changed = value.clone();
        changed.payloads.clear();
        assert!(changed.identity().is_err());
        let mut changed = value.clone();
        changed.payloads.get_mut(&"0".repeat(64)).unwrap().hash = "../escape".into();
        assert!(changed.identity().is_err());
        let mut changed = value;
        changed.roots.directories[0].tree = "4".repeat(64);
        assert!(changed.identity().is_err());
    }
}
