//! Local flat CAS catalogues: append-only decoded history, immutable snapshot bindings and raw objects.
use std::{collections::HashMap,fs};
use anyhow::{Context,Result};
use camino::{Utf8Path,Utf8PathBuf};
use serde::{Serialize,Deserialize};
use crate::{catalogue::{Catalogue,Format},snapshot_format::{FlatSnapshot,FlatDirectory,TreeSnapshot,DirectoryRoot,MAX_METADATA_BYTES}};
const SCHEMA:&str="projcli.flat-catalogue-reference.v1";
const FORMATS:[Format;4]=[Format::Csv,Format::Json,Format::CsvDedupeFile,Format::JsonDedupeFile];

/// One logical artefact's content inventory; this hash is not a directory-tree object ID.
#[derive(Clone,Debug,PartialEq,Eq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryRoot {
    /// Exact logical directory label.
    pub label:String,
    /// Catalogue-validated hash of complete path/kind/file-digest membership.
    pub inventory_hash:String,
}
/// Immutable snapshot binding independent of CSV/JSON/dictionary serialization.
#[derive(Clone,Debug,PartialEq,Eq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlatReference {
    /// Explicit versioned reference schema.
    pub schema:String,
    /// Content and inventory digest algorithm.
    pub hash_algorithm:String,
    /// Supplied build version, separate from inventory identities.
    pub version:String,
    /// Sorted selected logical artefacts and their inventory hashes.
    pub directories:Vec<InventoryRoot>,
}
impl FlatReference {
    fn canonical(&self)->Result<Self> {
        anyhow::ensure!(self.schema==SCHEMA&&self.hash_algorithm=="blake3","Unsupported flat catalogue reference schema/algorithm");
        // Reuse bounded label/version/digest validation, without persisting or claiming tree objects.
        let shape=TreeSnapshot::new(self.version.clone(),None,self.directories.iter().map(|root|DirectoryRoot {
            label:root.label.clone(),tree:root.inventory_hash.clone()}).collect())?;
        anyhow::ensure!(!shape.directories.is_empty(),"Flat snapshot requires selected artefacts");
        Ok(Self {schema:SCHEMA.into(),hash_algorithm:"blake3".into(),version:shape.version,
            directories:shape.directories.into_iter().map(|root|InventoryRoot {label:root.label,inventory_hash:root.tree}).collect()})
    }
    fn from_history(history:&Catalogue,snapshot:&FlatSnapshot)->Result<Self> {
        let index:HashMap<_,_>=history.records.iter().map(|record|((record.label.as_str(),record.version.as_str()),record)).collect();
        let directories=snapshot.directories.iter().map(|directory|{
            let record=index.get(&(directory.label.as_str(),snapshot.version.as_str())).context("Prepared inventory is absent")?;
            Ok(InventoryRoot {label:directory.label.clone(),inventory_hash:record.inventory_hash.clone()})
        }).collect::<Result<_>>()?;
        Self {schema:SCHEMA.into(),hash_algorithm:"blake3".into(),version:snapshot.version.clone(),directories}.canonical()
    }
    /// Domain-separated identity of the canonical binding, not cosmetic encoding or history container.
    pub fn identity(&self)->Result<String> {
        let bytes=serde_json::to_vec(&self.canonical()?)?;
        let mut hash=blake3::Hasher::new_derive_key(SCHEMA);hash.update(&bytes);Ok(hash.finalize().to_hex().to_string())
    }
    /// Readable small binding; alone it is not a complete backup.
    pub fn readable_json(&self)->Result<Vec<u8>> {Ok(serde_json::to_vec_pretty(&self.canonical()?)?)}
    /// Parse an explicitly versioned bounded binding; legacy flat IDs are never inferred here.
    pub fn parse(bytes:&[u8])->Result<Self> {
        anyhow::ensure!(bytes.len()<=MAX_METADATA_BYTES,"Flat reference exceeds metadata limit");
        serde_json::from_slice::<Self>(bytes).context("Malformed flat catalogue reference")?.canonical()
    }
}
pub(crate) fn filename(format:Format)->&'static str {
    match format {
        Format::Csv=>"catalogues/v1/manifest.csv",
        Format::Json=>"catalogues/v1/manifest.json",
        Format::CsvDedupeFile=>"catalogues/v1/manifest-dedupe-file.csv",
        Format::JsonDedupeFile=>"catalogues/v1/manifest-dedupe-file.json",
    }
}
fn reference_path(base:&Utf8Path,id:&str,project:bool)->Result<Utf8PathBuf> {
    crate::cas::validate_digest(id)?;
    let namespace=if project {"references/flat-v1"}else{"flat-snapshots/v1"};
    crate::cas::checked_directory(&base.join(namespace))?;
    crate::cas_tree::native(base,&format!("{namespace}/{}/{}.json",&id[..2],&id[2..]))
}
/// Validate flat namespace routing before controlled writes; creates nothing.
pub(crate) fn preflight(root:&Utf8Path,content:&Utf8Path,metadata:&Utf8Path)->Result<()> {
    crate::cas_tree::preflight_storage(root,content,metadata,&[])?;
    for relative in ["catalogues/v1","flat-snapshots/v1"] {
        let path=crate::cas_tree::native(metadata,relative)?;
        crate::cas_tree::project_target(root,&path,true)?;crate::cas::checked_directory(&path)?;
    }
    for format in FORMATS {
        let path=crate::cas_tree::native(metadata,filename(format))?;
        if crate::yml::configuration_entry_exists(&path)? {
            anyhow::ensure!(fs::symlink_metadata(&path)?.is_file(),"Flat catalogue is not regular");
        }
    }Ok(())
}
fn semantic_prefix(left:&Catalogue,right:&Catalogue)->bool {
    left.records.len()<=right.records.len()&&left.records.iter().zip(&right.records).all(|(a,b)|
        a.label==b.label&&a.version==b.version&&a.created_at==b.created_at&&a.inventory_hash==b.inventory_hash)
}
pub(crate) fn load_history(bases:&[Utf8PathBuf])->Result<Catalogue> {
    let mut best=Catalogue::default();
    for base in bases {for format in FORMATS {
        let path=crate::cas_tree::native(base,filename(format))?;
        if !crate::yml::configuration_entry_exists(&path)? {continue}
        let bytes=crate::fs_utils::read_managed_bounded(&path,MAX_METADATA_BYTES as u64)?;
        let candidate=Catalogue::parse(&bytes,format)?;
        // Parsers resolve dictionary IDs and validate each complete/reference digest.
        anyhow::ensure!(semantic_prefix(&best,&candidate)||semantic_prefix(&candidate,&best),"Flat catalogue histories diverge; no history was selected");
        if candidate.records.len()>best.records.len(){best=candidate;}
    }}
    Ok(best)
}
pub(crate) fn append_idempotent(history:&Catalogue,snapshot:&FlatSnapshot)->Result<Catalogue> {
    history.validate()?;
    let snapshot=FlatSnapshot::parse(&snapshot.readable_json()?)?;
    let existing:std::collections::HashSet<_>=history.records.iter().filter(|record|record.version==snapshot.version)
        .map(|record|record.label.as_str()).collect();
    let labels:Vec<_>=snapshot.directories.iter().filter(|directory|existing.contains(directory.label.as_str()))
        .map(|directory|directory.label.clone()).collect();
    let resolved=history.expanded_snapshot(&snapshot.version,&labels)?;
    let observed:HashMap<_,_>=resolved.directories.iter().map(|directory|(directory.label.as_str(),directory)).collect();
    let mut pending=Vec::new();
    for directory in snapshot.directories {
        if let Some(previous)=observed.get(directory.label.as_str()) {
            anyhow::ensure!(*previous==&directory,"Existing artefact/version inventory conflicts with retry");
        }else{pending.push(directory);}
    }
    if pending.is_empty(){return Ok(history.clone())}
    history.with_snapshot(&FlatSnapshot::new(snapshot.version,snapshot.created_at,pending)?)
}
/// Read and bound possible encoded history before provider entry. Locked publication rechecks it.
pub(crate) fn prepare(root:&Utf8Path,content:&Utf8Path,bases:&[Utf8PathBuf],snapshot:&FlatSnapshot,format:Format)->Result<()> {
    anyhow::ensure!(!bases.is_empty()&&bases.len()<=2,"Flat publication requires one or two full metadata locations");
    let snapshot=FlatSnapshot::parse(&snapshot.readable_json()?)?;
    anyhow::ensure!(!snapshot.directories.is_empty(),"Flat publication requires selected artefacts");
    let mut unique=std::collections::HashSet::new();
    for base in bases {
        let physical=Utf8PathBuf::try_from(crate::path_safety::resolve_path(base.as_std_path(),true)?)?;
        anyhow::ensure!(unique.insert(crate::path_safety::portable_path_key(&physical)),"Flat metadata locations alias each other");
        preflight(root,content,base)?;
    }
    append_idempotent(&load_history(bases)?,&snapshot)?.encode(format)?;Ok(())
}
/// Validate a small project-reference namespace, without claiming complete backup metadata.
pub(crate) fn preflight_project_reference(root:&Utf8Path,base:&Utf8Path)->Result<()> {
    let root=Utf8PathBuf::try_from(crate::path_safety::resolve_path(root.as_std_path(),true)?)?;
    let base=Utf8PathBuf::try_from(crate::path_safety::resolve_path(base.as_std_path(),true)?)?;
    anyhow::ensure!(base.strip_prefix(&root)?.starts_with(".proj/remotes"),"Flat project references require managed remotes metadata");
    let path=crate::cas_tree::native(&base,"references/flat-v1")?;
    crate::cas_tree::project_target(&root,&path,true)?;crate::cas::checked_directory(&path)
}
fn write_reference(base:&Utf8Path,reference:&FlatReference,project:bool)->Result<()> {
    let id=reference.identity()?;let path=reference_path(base,&id,project)?;
    crate::cas_tree::publish_metadata(&path,&reference.readable_json()?,|bytes|{
        anyhow::ensure!(FlatReference::parse(bytes)?.identity()?==id,"Conflicting flat snapshot reference");Ok(())
    })
}
/// Publish selected flat inventories after raw objects exist and verify.
/// Existing decoded records are append-only. A cooperative store claim covers the
/// authoritative history read, atomic full-table replacements and immutable refs.
/// Required history copies precede full refs and the optional project root ref.
/// Later failure may leave partial metadata/objects; no cross-location rollback or GC.
pub fn publish_snapshot(root:&Utf8Path,content:&Utf8Path,bases:&[Utf8PathBuf],project_reference:Option<&Utf8Path>,
    snapshot:&FlatSnapshot,format:Format)->Result<String> {
    let snapshot=FlatSnapshot::parse(&snapshot.readable_json()?)?;
    prepare(root,content,bases,&snapshot,format)?;
    if let Some(base)=project_reference {preflight_project_reference(root,base)?;}
    crate::path_safety::resolve_path(content.as_std_path(),true)?;fs::create_dir_all(content)?;
    let _writer=crate::attempt::Attempt::acquire(content)?;
    let history=append_idempotent(&load_history(bases)?,&snapshot)?;
    let bytes=history.encode(format)?;let reference=FlatReference::from_history(&history,&snapshot)?;let id=reference.identity()?;
    crate::cas_tree::verify_flat_inventory(content,&bases[0],&snapshot)?;
    let result=(||->Result<()> {
        for base in bases {
            preflight(root,content,base)?;
            let path=crate::cas_tree::native(base,filename(format))?;
            if crate::yml::configuration_entry_exists(&path)?
                &&crate::fs_utils::read_managed_bounded(&path,MAX_METADATA_BYTES as u64)?==bytes {continue}
            crate::cas::write_metadata(&path,&bytes,false)?;
        }
        for base in bases {write_reference(base,&reference,false)?;}
        if let Some(base)=project_reference {write_reference(base,&reference,true)?;}
        Ok(())
    })();
    result.with_context(||format!("Flat CAS snapshot {id} may be partially published; inspect before retry"))?;
    Ok(id)
}
/// Presence of one exact full flat reference; absence is not a successful verification.
pub(crate) fn snapshot_exists(content:&Utf8Path,metadata:&Utf8Path,id:&str)->Result<bool> {
    crate::cas::checked_directory(&crate::cas_tree::native(content,"objects")?)?;
    crate::yml::configuration_entry_exists(&reference_path(metadata,id,false)?)
}
/// Deeply verified flat history observation; inventory hashes are not tree-object IDs.
#[derive(Clone,Debug,Serialize)]
pub struct VerifiedFlatSnapshot {
    /// Canonical immutable snapshot binding ID.
    pub snapshot_id:String,
    /// Exact supplied version and selected logical inventory roots.
    pub snapshot:FlatReference,
    /// Complete resolved membership with full file digests and empty-directory leaves.
    pub directories:Vec<FlatDirectory>,
}
/// Verify the full binding, version references/dictionaries, inventory digests and raw file bytes.
pub fn verify_snapshot(content:&Utf8Path,metadata:&Utf8Path,id:&str)->Result<VerifiedFlatSnapshot> {
    let bytes=crate::fs_utils::read_managed_bounded(&reference_path(metadata,id,false)?,MAX_METADATA_BYTES as u64)?;
    let reference=FlatReference::parse(&bytes)?;
    anyhow::ensure!(reference.identity()?==id,"Corrupt flat snapshot-reference identity");
    let history=load_history(&[metadata.to_owned()])?;
    let index:HashMap<_,_>=history.records.iter().map(|row|((row.label.as_str(),row.version.as_str()),row)).collect();
    for root in &reference.directories {
        let row=index.get(&(root.label.as_str(),reference.version.as_str())).context("Flat snapshot inventory is absent")?;
        anyhow::ensure!(row.inventory_hash==root.inventory_hash,"Flat snapshot inventory hash conflicts with reference");
    }
    let labels=reference.directories.iter().map(|root|root.label.clone()).collect::<Vec<_>>();
    let snapshot=history.expanded_snapshot(&reference.version,&labels)?;
    crate::cas_tree::verify_flat_inventory(content,metadata,&snapshot)?;
    Ok(VerifiedFlatSnapshot {snapshot_id:id.into(),snapshot:reference,directories:snapshot.directories})
}
/// Restore one selected flat inventory through fresh verified files and the shared installer.
pub fn restore_label(root:&Utf8Path,content:&Utf8Path,metadata:&Utf8Path,id:&str,label:&str,destination:&Utf8Path,clear:bool)
    ->Result<crate::cas_tree::TreeRestore> {
    crate::path_safety::validate_directory_label(label)?;
    let verified=verify_snapshot(content,metadata,id)?;
    let inventory=verified.directories.iter().find(|directory|directory.label==label).context("Flat snapshot has no requested label")?;
    crate::cas_tree::restore_flat_inventory(root,content,metadata,id,&verified.snapshot.version,inventory,destination,clear)
}
