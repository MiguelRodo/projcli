//! Explicit recovery of a named local CAS tree snapshot; upload selection is never executed.
use anyhow::{Context,Result};
use camino::{Utf8Path,Utf8PathBuf};
use serde_json::Value;
use crate::cas_tree::VerifiedSnapshot;

fn observe(root:&Utf8Path,title:&str,id:&str,profile:Option<&str>)
    ->Result<(crate::yml::ValidatedConfig,Utf8PathBuf,Utf8PathBuf,VerifiedSnapshot)> {
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
    let (metadata,verified)=selected.context("Requested complete tree snapshot was not found; a small project reference alone cannot restore it")?;
    Ok((config,content,metadata,verified))
}

/// Deeply verify one exact v1 tree snapshot from a named local CAS remote.
/// Both remote and project full-metadata locations are considered independently of current
/// upload selection/write style. Every present copy must verify. This is read-only and needs
/// no root manifest.csv or legacy project ledger. Missing/corrupt content fails explicitly;
/// this command does not select latest, scan arbitrary stores or reinterpret legacy flat IDs.
pub fn verify(root:&Utf8Path,title:&str,snapshot:&str,profile:Option<&str>)->Result<VerifiedSnapshot> {
    observe(root,title,snapshot,profile).map(|(_,_,_,verified)|verified)
}

/// Restore one declared folder from an explicit named v1 tree snapshot.
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
        let actual=&verified.snapshot.version;
        let numbers=crate::local_remote::version_numbers(expected);
        anyhow::ensure!(expected==actual||(numbers.is_some()&&numbers==crate::local_remote::version_numbers(actual)),
            "Requested version does not match the explicit CAS snapshot");
    }
    let destination=config.get_path(root,&label)?;
    crate::cas_tree::restore_label(root,&content,&metadata,snapshot,&label,&destination,clear)?;
    Ok(crate::restore::RestoreResult {label,destination,provider:Some("local".into()),title:Some(title.into()),
        version:Some(verified.snapshot.version),restored:true,skipped:None,snapshot_id:Some(snapshot.into())})
}
