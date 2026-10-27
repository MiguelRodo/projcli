//! Portable closed-writer CAS stores; no change to existing object/snapshot IDs.
use std::{collections::BTreeMap,fs};
use anyhow::{Context,Result};
use camino::{Utf8Path,Utf8PathBuf};
use serde::{Deserialize,Serialize};
use crate::store_archive::{File,inventory};
const SCHEMA:&str="projcli.cas-export.v1";
const METADATA:&str="cas-export.json";
/// Existing immutable snapshot encoding, never inferred by reinterpreting an ID.
#[derive(Clone,Copy,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(rename_all="kebab-case")]
pub enum Kind {
    /// Recursive pure directory graph.
    Tree,
    /// CSV/JSON catalogue binding.
    Flat,
    /// Original raw-JSON flat manifest.
    Legacy,
}
#[derive(Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {schema:String,files:BTreeMap<String,File>,snapshots:BTreeMap<String,Kind>}
/// Owned complete-store ZIP with a trusted external checksum and exact length.
pub struct Export {
    archive:tempfile::NamedTempFile,
    /// BLAKE3 of the transport bytes, not an individual snapshot ID.
    pub hash:String,
    /// Exact compressed transport length.
    pub length:u64,
    /// Every included deeply verified existing-format snapshot.
    pub snapshots:BTreeMap<String,Kind>,
}
impl Export {
    /// Private ZIP retained until this export owner is dropped.
    pub fn path(&self)->&Utf8Path {Utf8Path::from_path(self.archive.path()).expect("validated UTF-8 temporary path")}
}
/// Owned private imported store. No existing caller store is overwritten.
pub struct Import {tree:crate::github_zip::ExtractedZip,snapshots:BTreeMap<String,Kind>}
impl Import {
    /// Private complete store for existing CAS readers or further ingestion.
    pub fn path(&self)->&Utf8Path {Utf8Path::from_path(self.tree.path()).expect("validated UTF-8 temporary path")}
    /// Existing-format snapshot IDs included in this export.
    pub fn snapshots(&self)->&BTreeMap<String,Kind> {&self.snapshots}
}
fn path(value:impl AsRef<std::path::Path>)->Result<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(value.as_ref().to_owned()).map_err(|_|anyhow::anyhow!("CAS export path is not UTF-8"))
}
fn persistent(name:&str)->bool {
    ["objects/","trees/v1/","tree-snapshots/v1/","flat-snapshots/v1/","manifests/"]
        .iter().any(|prefix|name.starts_with(prefix))
        ||[crate::catalogue::Format::Csv,crate::catalogue::Format::Json,crate::catalogue::Format::CsvDedupeFile,crate::catalogue::Format::JsonDedupeFile]
            .iter().any(|format|name==crate::cas_flat::filename(*format))
}
fn snapshot_id(name:&str,prefix:&str,sharded:bool)->Result<String> {
    let relative=name.strip_prefix(prefix).context("Invalid CAS snapshot namespace")?;
    let value=relative.strip_suffix(".json").context("CAS snapshot must use JSON binding")?;
    let id=if sharded {
        let (first,rest)=value.split_once('/').context("CAS snapshot must be sharded")?;
        anyhow::ensure!(first.len()==2&&rest.len()==62,"Malformed CAS snapshot digest path");
        format!("{first}{rest}")
    }else{value.into()};
    crate::cas::validate_digest(&id)?;Ok(id)
}
fn snapshots(root:&Utf8Path,files:&BTreeMap<String,File>)->Result<BTreeMap<String,Kind>> {
    let mut result=BTreeMap::new();
    for (name,file) in files {
        anyhow::ensure!(persistent(name),"Unknown CAS store file; no silent archive omission");
        if let Some(relative)=name.strip_prefix("objects/") {
            let (first,rest)=relative.split_once('/').context("Malformed CAS object path")?;
            anyhow::ensure!(first.len()==2&&rest.len()==62,"Malformed CAS object shard");
            let id=format!("{first}{rest}");crate::cas::validate_digest(&id)?;
            anyhow::ensure!(file.hash==id,"CAS object digest mismatch in complete export");
        }else if name.starts_with("trees/") {
            let id=snapshot_id(name,"trees/v1/",true)?;
            let bytes=crate::fs_utils::read_managed_bounded(&root.join(name),crate::snapshot_format::MAX_METADATA_BYTES as u64)?;
            anyhow::ensure!(crate::snapshot_format::DirectoryTree::parse(&bytes)?.identity()?==id,"Corrupt CAS directory object");
        }else{
            let selection=if name.starts_with("tree-snapshots/") {Some((snapshot_id(name,"tree-snapshots/v1/",true)?,Kind::Tree))}
                else if name.starts_with("flat-snapshots/"){Some((snapshot_id(name,"flat-snapshots/v1/",true)?,Kind::Flat))}
                else if name.starts_with("manifests/"){Some((snapshot_id(name,"manifests/",false)?,Kind::Legacy))}
                else {None};
            if let Some((id,kind))=selection {
                match kind {
                    Kind::Tree=>{crate::cas_tree::verify_snapshot(root,root,&id)?;},
                    Kind::Flat=>{crate::cas_flat::verify_snapshot(root,root,&id)?;},
                    Kind::Legacy=>{crate::cas_tree::verify_legacy_snapshot(root,&id)?;},
                }
                anyhow::ensure!(result.insert(id,kind).is_none(),"CAS snapshot ID occurs in multiple incompatible formats");
            }
        }
    }
    anyhow::ensure!(!result.is_empty(),"CAS export contains no complete snapshot");
    // Validate any optional catalogue history, including direct version references/dictionaries.
    crate::cas_flat::load_history(&[root.to_owned()])?;
    Ok(result)
}
fn validate(root:&Utf8Path,manifest:&Manifest)->Result<()> {
    anyhow::ensure!(manifest.schema==SCHEMA,"Unsupported CAS export schema");
    let mut actual=inventory(root)?;actual.remove(METADATA);
    anyhow::ensure!(actual==manifest.files,"CAS export membership/length/digest mismatch");
    anyhow::ensure!(snapshots(root,&actual)?==manifest.snapshots,"CAS export snapshot membership differs");Ok(())
}
/// Copy a cooperating closed CAS store under its writer claim, excluding only
/// coordination files. Recheck all objects (including orphans), directory identities
/// and every tree/flat/legacy snapshot. Full metadata must be present in this store:
/// project-only metadata must not be silently invented or migrated.
/// The original store is unchanged. Low-level tree writers do not all honor this
/// claim: callers must stop all writers first. No GC, hot-copy or power-loss claim.
pub fn export(project:&Utf8Path,store:&Utf8Path)->Result<Export> {
    anyhow::ensure!(project.is_absolute()&&store.is_absolute(),"CAS export project/store paths must be absolute");
    let physical=Utf8PathBuf::try_from(crate::path_safety::resolve_path(store.as_std_path(),true)?)?;
    let store=physical.as_path();
    crate::cas_tree::project_target(project,store,false)?;
    anyhow::ensure!(fs::symlink_metadata(store)?.is_dir(),"CAS export requires an existing real store");
    let copy=tempfile::tempdir()?;let copied=path(copy.path())?;
    {
        let _claim=crate::attempt::Attempt::acquire(store)?;
        for (name,file) in inventory(store)? {
            if name.starts_with(".proj/"){continue}
            anyhow::ensure!(persistent(&name),"Unknown CAS store file; export refuses silent omission");
            let target=copied.join(&name);fs::create_dir_all(target.parent().context("Missing CAS copy parent")?)?;
            fs::copy(store.join(&name),&target)?;
            anyhow::ensure!(fs::metadata(&target)?.len()==file.length&&crate::cas::hash_file(&target)?==file.hash,
                "CAS source changed during closed-writer export");
        }
    }
    let files=inventory(&copied)?;let snapshots=snapshots(&copied,&files)?;
    let manifest=Manifest {schema:SCHEMA.into(),files,snapshots};
    validate(&copied,&manifest)?;
    let bytes=serde_json::to_vec_pretty(&manifest)?;
    anyhow::ensure!(bytes.len()<=crate::snapshot_format::MAX_METADATA_BYTES,"CAS export metadata exceeds limit");
    fs::write(copied.join(METADATA),bytes)?;
    let (archive,empty)=crate::github_zip::directory_zip(&copied)?;anyhow::ensure!(!empty,"Empty CAS archive");
    let hash=crate::cas::hash_file(&path(archive.path())?)?;let length=archive.as_file().metadata()?.len();
    Ok(Export {archive,hash,length,snapshots:manifest.snapshots})
}
/// Trusted checksum/length precede existing bounded ZIP guards and complete
/// object/graph/inventory verification. Returns a fresh private store only;
/// never replaces good bytes. The transport envelope is removed after validation
/// so the store can accept new snapshots and be exported again.
pub fn import(archive:&Utf8Path,hash:&str,length:u64,max_expanded:u64)->Result<Import> {
    crate::cas::validate_digest(hash)?;
    crate::path_safety::resolve_path(archive.as_std_path(),true)?;
    let meta=fs::symlink_metadata(archive)?;
    anyhow::ensure!(meta.is_file()&&meta.len()==length,"CAS archive length/type mismatch");
    anyhow::ensure!(crate::cas::hash_file(archive)?==hash,"CAS archive transport digest mismatch");
    let tree=crate::github_zip::extract_zip(archive,max_expanded)?;
    let root=path(tree.path())?;
    let manifest:Manifest=serde_json::from_slice(&crate::fs_utils::read_managed_bounded(&root.join(METADATA),crate::snapshot_format::MAX_METADATA_BYTES as u64)?)?;
    validate(&root,&manifest)?;fs::remove_file(root.join(METADATA))?;
    Ok(Import {tree,snapshots:manifest.snapshots})
}
