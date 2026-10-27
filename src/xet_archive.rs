//! Closed-writer portable exports for the optional local Xet engine.
//! ZIP is transport, not a new content structure or hosted Xet protocol.
use std::{collections::BTreeMap,fs};
use anyhow::{Context,Result};
use camino::{Utf8Path,Utf8PathBuf};
use serde::{Deserialize,Serialize};
use crate::xet::{Engine,BACKEND,UPSTREAM};
const SCHEMA:&str="projcli.xet-export.v1";
const METADATA:&str="xet-export.json";
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(deny_unknown_fields)]
struct File {hash:String,length:u64}
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(deny_unknown_fields)]
struct Manifest {schema:String,backend:String,upstream:String,files:BTreeMap<String,File>,snapshots:Vec<String>}
/// Private complete-store ZIP. Carry its digest/length in a trusted reference;
/// a checksum stored only inside the ZIP is not proof of authenticity.
pub struct Export {
    archive:tempfile::NamedTempFile,
    /// BLAKE3 of complete transport bytes, independent of upstream addresses.
    pub hash:String,
    /// Transport size; sparse databases can expand before compression.
    pub length:u64,
    /// Every included immutable binding, independently reconstructed.
    pub snapshots:Vec<String>,
}
impl Export {
    /// Owned temporary archive, retained until this export is dropped.
    pub fn path(&self)->&Utf8Path {Utf8Path::from_path(self.archive.path()).expect("validated UTF-8 temporary path")}
}
/// A privately imported, validated store; never replaces an existing store.
/// Engine cache mutations stay in this disposable permission-aware temporary tree.
pub struct Import {tree:crate::github_zip::ExtractedZip,snapshots:Vec<String>}
impl Import {
    /// Keep this owner alive while using the private store path.
    pub fn path(&self)->&Utf8Path {Utf8Path::from_path(self.tree.path()).expect("validated UTF-8 temporary path")}
    /// Every committed snapshot carried by this export.
    pub fn snapshots(&self)->&[String] {&self.snapshots}
}
fn path(value:impl AsRef<std::path::Path>)->Result<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(value.as_ref().to_owned()).map_err(|_|anyhow::anyhow!("Xet export path is not UTF-8"))
}
fn persistent(name:&str)->bool {
    name=="xet-format.json"||name.starts_with("xet-snapshots/v1/")||name.starts_with("trees/v1/")
        ||name.starts_with("engine/xet/xorbs/")
}
fn disposable(name:&str)->bool {
    name==".proj"||name.starts_with(".proj/")||name=="engine/xet/shard-cache"||name.starts_with("engine/xet/shard-cache/")
        ||name=="engine/xet/shard-session"||name.starts_with("engine/xet/shard-session/")
}
fn inventory(root:&Utf8Path)->Result<BTreeMap<String,File>> {
    fn visit(root:&Utf8Path,current:&Utf8Path,depth:usize,work:&mut usize,result:&mut BTreeMap<String,File>)->Result<()> {
        anyhow::ensure!(depth<=64,"Xet export exceeds depth bound");
        for entry in fs::read_dir(current)? {
            *work+=1;anyhow::ensure!(*work<=100_000,"Xet export exceeds entry bound");
            let entry=entry?;let item=path(entry.path())?;
            let relative=item.strip_prefix(root)?.as_str().to_owned();
            crate::path_safety::portable_relative(&relative)?;
            let metadata=fs::symlink_metadata(&item)?;
            anyhow::ensure!(metadata.is_dir()||metadata.is_file(),"Xet export contains linked/special entry");
            if metadata.is_dir(){visit(root,&item,depth+1,work,result)?;}
            else {result.insert(relative,File {hash:crate::cas::hash_file(&item)?,length:metadata.len()});}
        }Ok(())
    }
    let mut result=BTreeMap::new();visit(root,root,0,&mut 0,&mut result)?;Ok(result)
}
fn snapshots(project:&Utf8Path,root:&Utf8Path,files:&BTreeMap<String,File>)->Result<Vec<String>> {
    let mut ids=Vec::new();
    for name in files.keys().filter(|name|name.starts_with("xet-snapshots/")) {
        let relative=name.strip_prefix("xet-snapshots/v1/").context("Unsupported Xet snapshot export namespace")?;
        let (prefix,suffix)=relative.split_once('/').context("Malformed Xet snapshot export path")?;
        let suffix=suffix.strip_suffix(".json").context("Malformed Xet snapshot export suffix")?;
        anyhow::ensure!(prefix.len()==2&&suffix.len()==62,"Malformed Xet snapshot export digest");
        let id=format!("{prefix}{suffix}");crate::cas::validate_digest(&id)?;
        crate::xet::read_snapshot(project,root,&id)?;ids.push(id);
    }
    anyhow::ensure!(!ids.is_empty(),"Xet export has no committed snapshots");Ok(ids)
}
fn validate(project:&Utf8Path,root:&Utf8Path,manifest:&Manifest,engine:&Engine)->Result<()> {
    anyhow::ensure!(manifest.schema==SCHEMA&&manifest.backend==BACKEND&&manifest.upstream==UPSTREAM,
        "Unsupported Xet export format/backend");
    anyhow::ensure!(manifest.files.len()<100_000,"Xet export metadata exceeds file limit");
    for (name,file) in &manifest.files {
        crate::path_safety::portable_relative(name)?;
        anyhow::ensure!(persistent(name),"Unexpected Xet export member");
        crate::cas::validate_digest(&file.hash)?;
    }
    let mut actual=inventory(root)?;actual.remove(METADATA);
    anyhow::ensure!(actual==manifest.files,"Xet export membership, length or digest mismatch");
    anyhow::ensure!(snapshots(project,root,&actual)?==manifest.snapshots,"Xet export snapshot membership differs");
    // No upstream success code alone is accepted as integrity proof.
    for id in &manifest.snapshots {crate::xet::verify(project,root,id,engine)?;}
    Ok(())
}
/// Export a cooperating closed writer under its claim, then verify a private copy.
/// Excludes only writer state/client caches; unknown persistent paths fail explicitly.
/// Includes all indexes, bindings and orphan packs, without GC or migration.
/// No hot-copy/power-loss guarantee. Temporary disk can exceed sparse allocation.
/// Original store payload bytes are not modified.
pub fn export(project:&Utf8Path,store:&Utf8Path,engine:&Engine)->Result<Export> {
    crate::xet::preflight(project,store)?;
    let copy=tempfile::tempdir()?;let copied=path(copy.path())?;
    {
        let _claim=crate::attempt::Attempt::acquire(store)?;
        crate::xet::guard_tree(store)?;
        for (name,file) in inventory(store)? {
            if disposable(&name){continue}
            anyhow::ensure!(persistent(&name),"Unrecognized Xet store file; export refuses silent omission");
            let target=copied.join(&name);fs::create_dir_all(target.parent().context("Missing export parent")?)?;
            fs::copy(store.join(&name),&target)?;
            anyhow::ensure!(fs::metadata(&target)?.len()==file.length&&crate::cas::hash_file(&target)?==file.hash,
                "Xet store changed during closed-writer export");
        }
    }
    let files=inventory(&copied)?;let ids=snapshots(project,&copied,&files)?;
    let mut manifest=Manifest {schema:SCHEMA.into(),backend:BACKEND.into(),upstream:UPSTREAM.into(),files,snapshots:ids};
    validate(project,&copied,&manifest,engine)?;
    // Validation can create client caches; they are not portable authoritative state.
    for relative in [".proj","engine/xet/shard-cache","engine/xet/shard-session"] {
        let target=copied.join(relative);if target.exists(){fs::remove_dir_all(target)?;}
    }
    let final_files=inventory(&copied)?;
    // Even reconstruction updates LocalClient's copied database/index. Record its
    // final closed bytes, while immutable bindings/trees must remain unchanged.
    let metadata=|files:&BTreeMap<String,File>|files.iter().filter(|(name,_)|!name.starts_with("engine/"))
        .map(|(name,file)|(name.clone(),file.clone())).collect::<BTreeMap<_,_>>();
    anyhow::ensure!(metadata(&final_files)==metadata(&manifest.files),"Engine changed immutable snapshot metadata during validation");
    for name in final_files.keys(){anyhow::ensure!(persistent(name),"Engine created unknown persistent state");}
    manifest.files=final_files;
    let bytes=serde_json::to_vec_pretty(&manifest)?;
    anyhow::ensure!(bytes.len()<=crate::snapshot_format::MAX_METADATA_BYTES,"Xet export metadata exceeds limit");
    fs::write(copied.join(METADATA),bytes)?;
    let (archive,empty)=crate::github_zip::directory_zip(&copied)?;
    anyhow::ensure!(!empty,"Empty Xet export");
    let archive_path=path(archive.path())?;
    let hash=crate::cas::hash_file(&archive_path)?;let length=archive.as_file().metadata()?.len();
    Ok(Export {archive,hash,length,snapshots:manifest.snapshots})
}
/// Check trusted transport digest/length before guarded extraction, then exact
/// membership and independent reconstruction of every committed snapshot.
/// No caller store/destination is replaced. max_expanded limits uncompressed bytes.
/// The same local engine operates on the returned private store, without a service.
pub fn import(project:&Utf8Path,archive:&Utf8Path,expected_hash:&str,expected_length:u64,max_expanded:u64,engine:&Engine)->Result<Import> {
    crate::cas::validate_digest(expected_hash)?;
    crate::path_safety::resolve_path(archive.as_std_path(),true)?;
    let metadata=fs::symlink_metadata(archive)?;
    anyhow::ensure!(metadata.is_file()&&metadata.len()==expected_length,"Xet archive length/type mismatch");
    anyhow::ensure!(crate::cas::hash_file(archive)?==expected_hash,"Xet archive transport digest mismatch");
    let tree=crate::github_zip::extract_zip(archive,max_expanded)?;
    let root=path(tree.path())?;crate::xet::guard_tree(&root)?;
    let manifest:Manifest=serde_json::from_slice(&crate::fs_utils::read_managed_bounded(&root.join(METADATA),
        crate::snapshot_format::MAX_METADATA_BYTES as u64)?)?;
    validate(project,&root,&manifest,engine)?;
    // Transport envelope is not mutable engine state. Keeping it would either
    // block re-export or make an old inventory look authoritative after ingest.
    fs::remove_file(root.join(METADATA))?;
    Ok(Import {tree,snapshots:manifest.snapshots})
}
