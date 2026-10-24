//! Bounded GitHub flat history bundles and deferred project publication.
use std::{fs,io::Write,collections::{HashMap,BTreeMap}};
use anyhow::{Context,Result};
use camino::{Utf8Path,Utf8PathBuf};
use serde::{Serialize,Deserialize};
use crate::{catalogue::Format,snapshot_format::{FlatSnapshot,FlatDirectory,TreeSnapshot,DirectoryRoot,MAX_METADATA_BYTES},
    cas_remote::Placement,ordinary_manifest::Policy};
pub(crate) const ASSET:&str="projcli-manifests-v1.zip";
const LIMIT:u64=128*1024*1024;
const SCHEMA:&str="projcli.github-snapshot.v1";
/// One physically materialized or retained Release payload.
#[derive(Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Payload {
    pub(crate) label:String,
    pub(crate) inventory_hash:String,
    pub(crate) asset:String,
    pub(crate) empty:bool,
    pub(crate) advertised_version:Option<String>,
}
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {schema:String,hash_algorithm:String,repository:String,tag:String,version:String,layout:String,payloads:Vec<Payload>}
impl Binding {
    fn canonical(mut self)->Result<Self> {
        anyhow::ensure!(self.schema==SCHEMA&&self.hash_algorithm=="blake3","Unsupported GitHub snapshot schema/algorithm");
        anyhow::ensure!(matches!(self.layout.as_str(),"latest"|"archive"),"Unsupported GitHub snapshot layout");
        let parts:Vec<_>=self.repository.split('/').collect();
        anyhow::ensure!(parts.len()==2&&parts.iter().all(|part|!part.is_empty()&&!matches!(*part,"."|"..")
            &&part.bytes().all(|byte|byte.is_ascii_alphanumeric()||b"-_.".contains(&byte))),"Invalid snapshot repository");
        anyhow::ensure!(!self.tag.is_empty()&&self.tag.len()<=512&&!self.tag.chars().any(char::is_control),"Invalid snapshot tag");
        let shape=TreeSnapshot::new(self.version.clone(),None,self.payloads.iter().map(|payload|
            DirectoryRoot {label:payload.label.clone(),tree:payload.inventory_hash.clone()}).collect())?;
        anyhow::ensure!(!shape.directories.is_empty(),"GitHub snapshot requires selected folders");
        for payload in &self.payloads {
            crate::path_safety::validate_directory_label(&payload.asset)?;
            anyhow::ensure!(payload.asset.ends_with(".zip"),"Snapshot payload must name a ZIP asset");
            if let Some(version)=&payload.advertised_version {
                anyhow::ensure!(crate::local_remote::version_numbers(version.trim_end_matches('*')).is_some(),"Invalid snapshot VERSION advertisement");
            }
        }
        self.payloads.sort_by(|a,b|a.label.as_bytes().cmp(b.label.as_bytes()));Ok(self)
    }
    fn parse(bytes:&[u8])->Result<Self> {
        anyhow::ensure!(bytes.len()<=MAX_METADATA_BYTES,"GitHub binding exceeds metadata limit");
        serde_json::from_slice::<Self>(bytes).context("Malformed GitHub snapshot binding")?.canonical()
    }
    fn identity(&self)->Result<String> {
        let mut hash=blake3::Hasher::new_derive_key(SCHEMA);hash.update(&serde_json::to_vec(self)?);Ok(hash.finalize().to_hex().to_string())
    }
}
/// Validate fixed project metadata paths before hooks, without creating them.
pub(crate) fn project_preflight(root:&Utf8Path,project:&Utf8Path)->Result<()> {
    crate::path_safety::validate_native_names(root,project.strip_prefix(root)?.as_str(),false)?;
    crate::cas_tree::project_target(root,project,true)?;
    for path in ["catalogues/v1","snapshots/v1","references/v1"] {
        crate::cas::checked_directory(&crate::cas_tree::native(project,path)?)?;
    }
    crate::cas_flat::load_history(&[project.to_owned()])?;Ok(())
}
fn binding_path(base:&Utf8Path,id:&str,small:bool)->Result<Utf8PathBuf> {
    crate::cas::validate_digest(id)?;
    crate::cas_tree::native(base,&format!("{}/{}/{}.json",if small{"references/v1"}else{"snapshots/v1"},&id[..2],&id[2..]))
}
fn validate_binding(rows:&HashMap<(&str,&str),&crate::catalogue::ArtefactVersion>,id:&str,bytes:&[u8])->Result<()> {
    let binding=Binding::parse(bytes)?;
    anyhow::ensure!(binding.identity()?==id,"Corrupt GitHub snapshot binding identity");
    for payload in &binding.payloads {
        let row=rows.get(&(payload.label.as_str(),binding.version.as_str())).context("GitHub binding inventory is missing")?;
        anyhow::ensure!(row.inventory_hash==payload.inventory_hash,"GitHub binding inventory hash conflicts");
    }Ok(())
}
/// Validate only supported full metadata members; no arbitrary files are installed.
fn validate_bundle(base:&Utf8Path)->Result<BTreeMap<String,String>> {
    let history=crate::cas_flat::load_history(&[base.to_owned()])?;
    let files=crate::manifest::inventory_directory(base,false)?;
    let rows:HashMap<_,_>=history.records.iter().map(|row|((row.label.as_str(),row.version.as_str()),row)).collect();
    anyhow::ensure!(files.len()<=100_000,"GitHub metadata bundle has too many files");
    let mut total=0u64;
    for (relative,_) in &files {
        let path=base.join(&relative);let length=fs::symlink_metadata(&path)?.len();
        total=total.checked_add(length).context("Metadata size overflow")?;
        anyhow::ensure!(length<=MAX_METADATA_BYTES as u64&&total<=LIMIT,"GitHub metadata bundle exceeds byte limits");
        if [Format::Csv,Format::Json,Format::CsvDedupeFile,Format::JsonDedupeFile].iter()
            .any(|format|relative==crate::cas_flat::filename(*format)){continue}
        let parts:Vec<_>=relative.split('/').collect();
        anyhow::ensure!(parts.len()==4&&parts[0]=="snapshots"&&parts[1]=="v1"&&parts[2].len()==2,"Unsupported GitHub metadata member");
        let rest=parts[3].strip_suffix(".json").context("Unsupported GitHub metadata extension")?;
        let id=format!("{}{rest}",parts[2]);crate::cas::validate_digest(&id)?;
        validate_binding(&rows,&id,&crate::fs_utils::read_managed_bounded(&path,MAX_METADATA_BYTES as u64)?)?;
    }Ok(files.into_iter().collect())
}
/// Download only the named metadata bundle into private staging, with compressed/expanded bounds.
pub(crate) fn download(client:&crate::github_release::GithubReleaseClient,tag:&str,stage:&Utf8Path,base:&Utf8Path)->Result<()> {
    crate::github_zip::download_directory(client,stage,tag,ASSET,base,LIMIT,LIMIT,true,false)?;
    validate_bundle(base).map(|_|())
}
fn temporary(bytes:&[u8])->Result<tempfile::NamedTempFile> {
    let mut file=tempfile::Builder::new().prefix("proj-github-record-").tempfile_in(fs::canonicalize(std::env::temp_dir())?)?;
    file.write_all(bytes)?;file.flush()?;Ok(file)
}
/// Disk-backed project record, deliberately not installed during private preparation.
pub(crate) struct Prepared {
    project:Utf8PathBuf,format:Format,project_full:bool,id:String,
    history:tempfile::NamedTempFile,binding:tempfile::NamedTempFile,
}
impl Prepared {
    /// Publish selected full project metadata after all required HTTP success.
    pub(crate) fn finish_full(&self,root:&Utf8Path)->Result<()> {
        project_preflight(root,&self.project)?;
        let history=crate::fs_utils::read_managed_bounded(Utf8Path::from_path(self.history.path()).context("Invalid staging path")?,MAX_METADATA_BYTES as u64)?;
        let binding=crate::fs_utils::read_managed_bounded(Utf8Path::from_path(self.binding.path()).context("Invalid staging path")?,MAX_METADATA_BYTES as u64)?;
        if self.project_full {
            let path=crate::cas_tree::native(&self.project,crate::cas_flat::filename(self.format))?;
            crate::cas::write_metadata(&path,&history,false)?;
            immutable(&binding_path(&self.project,&self.id,false)?,&binding)?;
        }
        Ok(())
    }
    /// Publish the small project binding after every required full metadata copy.
    pub(crate) fn finish_reference(&self,root:&Utf8Path)->Result<()> {
        project_preflight(root,&self.project)?;
        let binding=crate::fs_utils::read_managed_bounded(Utf8Path::from_path(self.binding.path()).context("Invalid staging path")?,MAX_METADATA_BYTES as u64)?;
        immutable(&binding_path(&self.project,&self.id,true)?,&binding)
    }
    /// Explicit cleanup, with remote/project effects retained and accurately reported on failure.
    pub(crate) fn close(self)->Result<()> {
        let history=self.history.close();let binding=self.binding.close();
        history.context("Cannot clean private GitHub history")?;binding.context("Cannot clean private GitHub binding")?;Ok(())
    }
}
fn immutable(path:&Utf8Path,bytes:&[u8])->Result<()> {
    crate::cas_tree::publish_metadata(path,bytes,|existing|{
        anyhow::ensure!(existing==bytes,"Conflicting GitHub snapshot binding; preserved");Ok(())
    })
}
/// Stage actual-payload history and optional new remote bundle; no external/project writes.
pub(crate) fn prepare(root:&Utf8Path,project:&Utf8Path,bundle:&Utf8Path,policy:&Policy,repository:&str,tag:&str,version:&str,
    latest:bool,directories:Vec<FlatDirectory>,mut payloads:Vec<Payload>)->Result<(Prepared,Option<tempfile::NamedTempFile>)> {
    project_preflight(root,project)?;validate_bundle(bundle)?;
    let history=crate::cas_flat::load_history(&[project.to_owned(),bundle.to_owned()])?;
    let snapshot=FlatSnapshot::new(version.into(),None,directories)?;
    let history=crate::cas_flat::append_idempotent(&history,&snapshot)?;
    let rows:HashMap<_,_>=history.records.iter().map(|row|((row.label.as_str(),row.version.as_str()),row)).collect();
    for payload in &mut payloads {
        payload.inventory_hash=rows.get(&(payload.label.as_str(),version))
            .context("GitHub prepared inventory missing")?.inventory_hash.clone();
    }
    let binding=Binding {schema:SCHEMA.into(),hash_algorithm:"blake3".into(),repository:repository.into(),
        tag:tag.into(),version:version.into(),layout:(if latest{"latest"}else{"archive"}).into(),payloads}.canonical()?;
    let id=binding.identity()?;let binding=serde_json::to_vec_pretty(&binding)?;
    anyhow::ensure!(binding.len()<=MAX_METADATA_BYTES,"GitHub binding exceeds metadata limit");
    let history=history.encode(policy.format)?;
    // Always retain disk-backed project preparation before the first HTTP mutation.
    let prepared=Prepared {project:project.into(),format:policy.format,
        project_full:matches!(policy.placement,Placement::Auto|Placement::Project|Placement::Both),
        id:id.clone(),history:temporary(&history)?,binding:temporary(&binding)?};
    let remote=if policy.placement!=Placement::Project {
        let path=crate::cas_tree::native(bundle,crate::cas_flat::filename(policy.format))?;
        crate::cas::write_metadata(&path,&history,false)?;
        immutable(&binding_path(bundle,&id,false)?,&binding)?;
        let members=validate_bundle(bundle)?;
        let (zip,empty)=crate::github_zip::recorded_directory_zip(bundle,&members)?;
        anyhow::ensure!(!empty&&fs::metadata(zip.path())?.len()<=LIMIT,"Generated GitHub metadata bundle exceeds download limits");
        Some(zip)
    }else{None};
    Ok((prepared,remote))
}

/// Observe the exact prepared archive, including the writer's deliberate omission
/// of empty child directories. Empty sentinels are transport metadata, not user files.
pub(crate) fn observe_zip(zip:&tempfile::NamedTempFile,label:&str,empty:bool)->Result<FlatDirectory> {
    let extracted=crate::github_zip::extract_zip(Utf8Path::from_path(zip.path()).context("Invalid ZIP staging path")?,LIMIT)?;
    let result=(||->Result<FlatDirectory> {
        let path=Utf8Path::from_path(extracted.path()).context("Invalid extracted path")?;
        if empty {
            let entries:Vec<_>=fs::read_dir(path)?.collect::<std::io::Result<_>>()?;
            let marker=path.join("projr-empty");
            anyhow::ensure!(entries.len()==1&&marker.is_file()&&fs::metadata(&marker)?.len()==0,"Invalid prepared empty ZIP");
            fs::remove_file(marker)?;
        }
        crate::ordinary_manifest::observe(path,label)
    })();
    let cleanup=extracted.close();
    match(result,cleanup) {
        (Ok(value),Ok(()))=>Ok(value),(Err(error),Ok(()))=>Err(error),
        (Ok(_),Err(error))=>Err(error),(Err(error),Err(cleanup))=>Err(error.context(format!("Prepared ZIP cleanup also failed: {cleanup:#}"))),
    }
}
