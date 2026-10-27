//! Immutable whole-store ZIP transfer through one long-running GitHub Release.
//! Uses the same local Xet engine. No hosted Xet service, replacement or GC.
use anyhow::{Context,Result};
use camino::Utf8Path;
use serde::{Deserialize,Serialize};
use crate::{github_release::GithubReleaseClient,xet::{Engine,BACKEND,UPSTREAM}};
/// Trusted project-side locator for one verified complete-store export.
/// Keep this in version-controlled project metadata. An optional remote copy
/// is only convenience, not a substitute for trusting this digest/reference.
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    /// Explicit versioned transport schema, separate from backend/snapshot IDs.
    pub schema:String,
    /// Content structure remains xet regardless of transport.
    pub structure:String,
    /// Local engine compatibility domain.
    pub backend:String,
    /// Exact pinned format producer.
    pub upstream:String,
    /// One long-running storage Release tag, not a project version template.
    pub tag:String,
    /// Immutable complete-store ZIP asset name.
    pub asset:String,
    /// BLAKE3 of the ZIP bytes.
    pub hash:String,
    /// Exact compressed length.
    pub length:u64,
    /// Every committed binding in this complete store export.
    pub snapshots:Vec<String>,
}
impl Reference {
    pub(crate) fn validate(&self)->Result<()> {
        anyhow::ensure!(self.schema=="projcli.xet-github-reference.v1"&&self.structure=="xet"
            &&self.backend==BACKEND&&self.upstream==UPSTREAM,"Unsupported Xet GitHub reference");
        crate::cas::validate_digest(&self.hash)?;
        anyhow::ensure!(self.asset==format!("proj-xet-store-{}.zip",self.hash),"Xet store asset name/digest disagree");
        anyhow::ensure!(!self.tag.is_empty()&&!self.tag.contains("@version")&&!self.tag.chars().any(char::is_control),
            "Xet requires an explicit long-running storage Release tag");
        anyhow::ensure!(self.length<2*1024*1024*1024u64&&!self.snapshots.is_empty()
            &&self.snapshots.len()<=100_000,"Invalid Xet store reference size/membership");
        let mut previous=None;
        for id in &self.snapshots {
            crate::cas::validate_digest(id)?;
            anyhow::ensure!(previous.is_none_or(|p:&String|p<id),"Xet snapshot references must be sorted/unique");
            previous=Some(id);
        }Ok(())
    }
}
/// Download an explicitly trusted complete-store reference to private scratch,
/// verify downloadable bytes and every snapshot, then return the same local engine
/// store for restore or further ingest. max_bytes and max_expanded bound transfer
/// and ZIP expansion. No existing store or project reference is replaced.
pub fn download(project:&Utf8Path,client:&GithubReleaseClient,reference:&Reference,
    max_bytes:u64,max_expanded:u64,engine:&Engine)->Result<crate::xet_archive::Import> {
    reference.validate()?;
    anyhow::ensure!(reference.length<=max_bytes,"Xet store exceeds download budget");
    let temporary=tempfile::tempdir()?;
    let stage=Utf8Path::from_path(temporary.path()).context("Xet download path is not UTF-8")?;
    let archive=stage.join("store.zip");
    client.download_asset(project,&reference.tag,&reference.asset,&archive,max_bytes)?;
    let imported=crate::xet_archive::import(project,&archive,&reference.hash,reference.length,max_expanded,engine)?;
    anyhow::ensure!(imported.snapshots()==reference.snapshots,"Downloaded Xet snapshot membership differs from trusted reference");
    Ok(imported)
}
/// Export a cooperating closed writer, upload a new immutable ZIP, then download
/// and validate it before returning a project-commit-eligible reference.
/// Same-name retries reuse only independently validated exact bytes. No old
/// asset/reference is deleted or replaced; errors may leave an orphan new ZIP.
/// Callers must commit their project reference only after success. This library
/// has no implicit remote "latest" pointer or multi-machine writer protocol;
/// configured build integration must resolve that boundary explicitly.
pub fn upload(project:&Utf8Path,store:&Utf8Path,client:&GithubReleaseClient,tag:&str,
    create_release:bool,max_expanded:u64,engine:&Engine)->Result<Reference> {
    let export=crate::xet_archive::export(project,store,engine)?;
    let reference=Reference {schema:"projcli.xet-github-reference.v1".into(),structure:"xet".into(),
        backend:BACKEND.into(),upstream:UPSTREAM.into(),tag:tag.into(),
        asset:format!("proj-xet-store-{}.zip",export.hash),hash:export.hash.clone(),
        length:export.length,snapshots:export.snapshots.clone()};
    reference.validate()?;
    let existing=client.assets_if_exists(tag)?.unwrap_or_default();
    let matches:Vec<_>=existing.iter().filter(|asset|asset.name==reference.asset).collect();
    anyhow::ensure!(matches.len()<=1,"Ambiguous Xet store asset; no overwrite performed");
    if matches.is_empty() {
        client.upload_asset(tag,&reference.asset,export.path(),create_release)
            .context("Xet store upload was not verified; keep prior reference and inspect remote state before retry")?;
    }
    download(project,client,&reference,reference.length,max_expanded,engine)
        .context("Xet uploaded/downloaded store was not verified; retain prior reference; a new orphan ZIP may remain")?;
    Ok(reference)
}
