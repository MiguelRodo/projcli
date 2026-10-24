//! Explicit recovery of a named local CAS snapshot; old flat and v1 trees retain distinct identities.
use anyhow::{Context,Result};
use camino::{Utf8Path,Utf8PathBuf};
use serde_json::Value;
use crate::cas_tree::{VerifiedSnapshot,VerifiedLegacySnapshot};

/// A deeply checked observation; the tag identifies the actual stored format.
/// The legacy raw-JSON digest is never reported as a content-only tree root.
#[derive(Clone,Debug,serde::Serialize)]
#[serde(tag="snapshot_kind")]
pub enum SnapshotVerification {
    /// Existing v1 directory graph and complete observed inventories.
    #[serde(rename="tree-v1")]
    Tree {
        /// Verified graph observation, flattened into the existing tree response shape.
        #[serde(flatten)]
        observation:VerifiedSnapshot,
    },
    /// Versioned flat catalogue binding and complete resolved observations.
    #[serde(rename="flat-catalogue-v1")]
    FlatCatalogue {
        /// Full hashes are resolved from version references and optional file dictionaries.
        #[serde(flatten)]
        observation:crate::cas_flat::VerifiedFlatSnapshot,
    },
    /// Original flat JSON identity/version and complete observed file inventory.
    #[serde(rename="legacy-flat")]
    LegacyFlat {
        /// Validated old inventory; no tree identities or empty-directory history invented.
        #[serde(flatten)]
        observation:VerifiedLegacySnapshot,
    },
}
impl SnapshotVerification {
    fn version(&self)->&str {
        match self {Self::Tree {observation}=>&observation.snapshot.version,
            Self::LegacyFlat {observation}=>&observation.version,
            Self::FlatCatalogue {observation}=>&observation.snapshot.version}
    }
    fn kind(&self)->&'static str {
        match self {Self::Tree {..}=>"tree-v1",Self::LegacyFlat {..}=>"legacy-flat",Self::FlatCatalogue {..}=>"flat-catalogue-v1"}
    }
}

fn observe(root:&Utf8Path,title:&str,id:&str,profile:Option<&str>)
    ->Result<(crate::yml::ValidatedConfig,Utf8PathBuf,Utf8PathBuf,SnapshotVerification)> {
    crate::cas::validate_digest(id)?;
    let profiles=crate::run_context::select_profiles(profile,root)?;
    crate::yml::require_active_configuration_for_profiles(root,&profiles)?;
    let raw=crate::yml::get_combined_yml(Some(&profiles.join(",")),root)?;
    let config=crate::restore::projected_config(&raw,root)?;
    let value=raw.get("remotes").and_then(|value|value.get("local")).and_then(|value|value.get(title))
        .context("Explicit CAS recovery requires a named remotes.local definition")?;
    anyhow::ensure!(value.get("structure").and_then(Value::as_str)==Some("cas"),"Selected remote is not local CAS");
    let path=if crate::cas_remote::legacy_shape(value) {
        serde_json::from_value::<crate::yml::LocalRemoteConfig>(value.clone())?.path
    }else{crate::cas_remote::Definition::parse(value)?.path};
    let content=if path.is_absolute(){path}else{root.join(path)};
    let project=crate::cas_remote::project_metadata(root,title);
    let mut selected:Option<(Utf8PathBuf,VerifiedSnapshot)>=None;
    // Current write placement/style and upload subsets must not hide an older explicit snapshot.
    // A corrupt present copy is an error, never permission to silently use another copy.
    for metadata in [&content,&project] {
        if !crate::cas_tree::snapshot_exists(&content,metadata,id)? {continue}
        let verified=crate::cas_tree::verify_snapshot(&content,metadata,id)?;
        if let Some((_,previous))=&selected {
            anyhow::ensure!(previous.snapshot==verified.snapshot,"Conflicting CAS snapshot copies");
        }else{selected=Some((metadata.clone(),verified));}
    }
    let mut flat:Option<(Utf8PathBuf,crate::cas_flat::VerifiedFlatSnapshot)>=None;
    for metadata in [&content,&project] {
        if !crate::cas_flat::snapshot_exists(&content,metadata,id)? {continue}
        let verified=crate::cas_flat::verify_snapshot(&content,metadata,id)?;
        if let Some((_,previous))=&flat {
            anyhow::ensure!(previous.snapshot==verified.snapshot,"Conflicting flat snapshot copies");
        }else{flat=Some((metadata.clone(),verified));}
    }
    anyhow::ensure!(selected.is_none()||flat.is_none(),"Snapshot ID occurs in incompatible format namespaces");
    if crate::cas::legacy_manifest_exists(&content,id)? {
        anyhow::ensure!(selected.is_none()&&flat.is_none(),"Snapshot ID occurs in incompatible format namespaces; selection is ambiguous");
        let verified=crate::cas_tree::verify_legacy_snapshot(&content,id)?;
        return Ok((config,content.clone(),content,SnapshotVerification::LegacyFlat {observation:verified}));
    }
    if let Some((metadata,verified))=flat {return Ok((config,content,metadata,SnapshotVerification::FlatCatalogue {observation:verified}))}
    let (metadata,verified)=selected.context("Requested complete snapshot was not found; a small project reference alone cannot restore it")?;
    Ok((config,content,metadata,SnapshotVerification::Tree {observation:verified}))
}

/// Deeply verify one exact old flat or v1 tree snapshot from a named local CAS remote.
/// Both remote and project full-metadata locations are considered independently of current
/// upload selection/write style. Every present copy must verify. This is read-only and needs
/// no root manifest.csv or legacy project ledger. Missing/corrupt content fails explicitly;
/// this command does not select latest or scan arbitrary stores. Legacy flat metadata uses
/// its original raw-JSON identity/namespace and never becomes a v1 tree identity.
pub fn verify(root:&Utf8Path,title:&str,snapshot:&str,profile:Option<&str>)->Result<SnapshotVerification> {
    observe(root,title,snapshot,profile).map(|(_,_,_,verified)|verified)
}

/// Restore one declared folder from an explicit named old-flat or v1-tree snapshot.
/// Selection is the exact title/ID/label, not current upload eligibility. The optional version
/// is a guard against selecting the wrong snapshot. Deep checks precede the existing guarded
/// staging/installer; clear=false keeps destination-only files. VERSION, CSV, metadata and
/// store bytes are unchanged. An empty selected root is a valid explicit inventory.
pub fn restore(root:&Utf8Path,title:&str,snapshot:&str,label:&str,profile:Option<&str>,
    remote_version:Option<&str>,clear:bool)->Result<crate::restore::RestoreResult> {
    crate::path_safety::validate_directory_label(label)?;
    let (config,content,metadata,verified)=observe(root,title,snapshot,profile)?;
    let label=config.directories.keys().find(|candidate|candidate.eq_ignore_ascii_case(label))
        .context("CAS restore label is not declared")?.clone();
    if let Some(expected)=remote_version {
        let actual=verified.version();
        let numbers=crate::local_remote::version_numbers(expected);
        anyhow::ensure!(expected==actual||(numbers.is_some()&&numbers==crate::local_remote::version_numbers(actual)),
            "Requested version does not match the explicit CAS snapshot");
    }
    let destination=config.get_path(root,&label)?;
    let version=verified.version().to_owned();let kind=verified.kind().to_owned();
    match verified {
        SnapshotVerification::Tree {..}=>crate::cas_tree::restore_label(root,&content,&metadata,snapshot,&label,&destination,clear)?,
        SnapshotVerification::LegacyFlat {..}=>crate::cas_tree::restore_legacy_label(root,&content,snapshot,&label,&destination,clear)?,
        SnapshotVerification::FlatCatalogue {..}=>crate::cas_flat::restore_label(root,&content,&metadata,snapshot,&label,&destination,clear)?,
    };
    Ok(crate::restore::RestoreResult {label,destination,provider:Some("local".into()),title:Some(title.into()),
        version:Some(version),restored:true,skipped:None,snapshot_id:Some(snapshot.into()),snapshot_kind:Some(kind)})
}
