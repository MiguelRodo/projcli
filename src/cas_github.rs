//! Immutable complete CAS-store ZIP transfer through one long-running Release.
use std::collections::BTreeMap;
use anyhow::{Context,Result};
use camino::Utf8Path;
use serde::{Serialize,Deserialize};
use crate::{cas_archive::{Kind,Import},github_release::GithubReleaseClient};
/// Trusted project locator; complete manifests stay inside the immutable store ZIP.
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    /// Explicit transport schema, distinct from existing CAS identities.
    pub schema:String,
    /// File-level content store; always cas.
    pub structure:String,
    /// One long-running storage Release tag.
    pub tag:String,
    /// Immutable complete-store ZIP filename.
    pub asset:String,
    /// BLAKE3 of complete ZIP bytes.
    pub hash:String,
    /// Exact compressed length.
    pub length:u64,
    /// Every included immutable snapshot ID and its existing encoding.
    pub snapshots:BTreeMap<String,Kind>,
}
impl Reference {
    pub(crate) fn validate(&self)->Result<()> {
        anyhow::ensure!(self.schema=="projcli.cas-github-reference.v1"&&self.structure=="cas","Unsupported CAS GitHub reference");
        crate::cas::validate_digest(&self.hash)?;
        anyhow::ensure!(self.asset==format!("proj-cas-store-{}.zip",self.hash),"CAS store name/digest disagree");
        anyhow::ensure!(!self.tag.is_empty()&&!self.tag.contains("@version")&&!self.tag.chars().any(char::is_control),
            "CAS requires a named long-running storage Release");
        anyhow::ensure!(self.length<2*1024*1024*1024u64&&!self.snapshots.is_empty()&&self.snapshots.len()<=100_000,"Invalid CAS export size/membership");
        for id in self.snapshots.keys(){crate::cas::validate_digest(id)?;}Ok(())
    }
}
/// Download a trusted export to private storage and deeply verify every snapshot.
/// max_bytes/max_expanded are hard bounds. No existing caller tree is replaced.
pub fn download(project:&Utf8Path,client:&GithubReleaseClient,reference:&Reference,
    max_bytes:u64,max_expanded:u64)->Result<Import> {
    reference.validate()?;
    let archive=crate::store_github::download(project,client,&reference.tag,&reference.asset,&reference.hash,reference.length,max_bytes)?;
    let imported=crate::cas_archive::import(&archive.path(),&reference.hash,reference.length,max_expanded)?;
    anyhow::ensure!(imported.snapshots()==&reference.snapshots,"CAS export snapshot membership differs from trusted reference");
    Ok(imported)
}
/// Upload a new immutable full-store ZIP, then verify its downloadable bytes and
/// every existing-format snapshot before returning a project-commit-eligible locator.
/// No old asset is deleted. Failure can leave an orphan ZIP: keep the prior locator.
pub fn upload(project:&Utf8Path,store:&Utf8Path,client:&GithubReleaseClient,tag:&str,
    create_release:bool,max_expanded:u64)->Result<Reference> {
    let export=crate::cas_archive::export(project,store)?;
    let reference=Reference {schema:"projcli.cas-github-reference.v1".into(),structure:"cas".into(),tag:tag.into(),
        asset:format!("proj-cas-store-{}.zip",export.hash),hash:export.hash.clone(),length:export.length,snapshots:export.snapshots.clone()};
    reference.validate()?;
    crate::store_github::upload(client,tag,&reference.asset,export.path(),create_release)?;
    download(project,client,&reference,reference.length,max_expanded)
        .context("CAS uploaded store was not verified; retain prior reference; a new orphan ZIP may remain")?;
    Ok(reference)
}
