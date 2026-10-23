//! Guarded ordinary local archive/latest publication with CSV or file comparison.
//! Archive/latest share guarded staging; no registry redesign is implied.
use std::{collections::BTreeMap,fs,io::Write};
use anyhow::Context;
use camino::{Utf8Path,Utf8PathBuf};
use serde::Deserialize;
use serde_json::{Map,Value};
use crate::local_store::StoreStrategy;
use crate::manifest::ManifestRow;

/// One validated ordinary local destination. Settings are frozen before hooks.
#[derive(Clone)]
pub(crate) struct LocalPublication {
    /// Descriptive title, not a provider identifier.
    title:String,
    /// Physical root holding per-label latest directories and shared metadata.
    pub(crate) base:Utf8PathBuf,
    labels:Vec<(String,Utf8PathBuf)>,
    append:bool,
    archive:bool,
    strategy:StoreStrategy,
    always:bool,
    // No comparison changes planning, not filesystem safety validation.
    inspect_none:bool,
    inspect_manifest:bool,
}
/// Shared typed send controls; each transport validates its executable subset.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Send {
    pub(crate) cue:Option<String>,
    pub(crate) inspect:Option<String>,
    pub(crate) strategy:Option<String>,
    pub(crate) conflict:Option<Value>,
}
fn identity(path:&Utf8Path)->anyhow::Result<Utf8PathBuf> {
    Ok(Utf8PathBuf::try_from(crate::path_safety::resolve_path(path.as_std_path(),true)?)?)
}
fn overlaps(a:&Utf8Path,b:&Utf8Path)->bool {
    // ponytail: conservative portable case comparison for planned missing paths;
    // avoids filesystem probe writes before the build's mutation boundary.
    let normalizer=icu_normalizer::ComposingNormalizer::new_nfc();
    let a=Utf8PathBuf::from(normalizer.normalize(&a.as_str().to_uppercase()).into_owned());
    let b=Utf8PathBuf::from(normalizer.normalize(&b.as_str().to_uppercase()).into_owned());
    a.starts_with(&b)||b.starts_with(&a)
}
fn native_name(path:&Utf8Path)->anyhow::Result<()> {
    let anchor=path.ancestors().last().context("Publication path has no root")?;
    crate::path_safety::validate_native_names(anchor,path.strip_prefix(anchor)?.as_str(),false)
}
fn destination(target:&LocalPublication,label:&str,empty:bool,version:&str)->Utf8PathBuf {
    let path=if target.append {target.base.join(label)} else {target.base.clone()};
    let path=if target.archive {path.join(version)}else{path};
    if empty {Utf8PathBuf::from(format!("{path}-empty"))}else{path}
}
/// Parse existing build.local definitions and preflight every selected namespace.
/// Archive (default) and latest execute. Null/false content
/// selects nothing; true uses declared non-cache labels. No directories are made.
pub(crate) fn configured(root:&Utf8Path,raw:Option<&Map<String,Value>>,config:&crate::yml::ValidatedConfig)->anyhow::Result<Vec<LocalPublication>> {
    let mut targets=Vec::new();
    let declared:Map<String,Value>=config.directories.keys().map(|key|(key.clone(),Value::Null)).collect();
    let content_context=serde_json::json!({"directories":declared});
    for (title,value) in raw.into_iter().flatten() {
        anyhow::ensure!(!title.is_empty()&&!title.chars().any(char::is_control),"Invalid local publication title");
        let definition=crate::restore::definition(value)?;
        anyhow::ensure!(definition.cue.is_none(),"Use send.cue; a title-level cue has inconsistent source semantics");
        let mut labels=Vec::new();
        // Preserve explicit content order; true follows the already validated
        // declared labels in a deterministic order, not HashMap iteration order.
        if let Some(Value::Array(list))=&definition.content {
            for item in list {labels.push(item.as_str().context("Content labels must be strings")?.to_owned());}
        } else {
            let mut names:Vec<_>=config.directories.keys().cloned().collect();names.sort();
            for label in names {
                if crate::restore::content_has(definition.content.as_ref(),&label,&content_context)? {labels.push(label);}
            }
        }
        for label in &labels {crate::restore::content_has(definition.content.as_ref(),label,&content_context)?;}
        crate::path_safety::validate_directory_labels(labels.iter().map(String::as_str))?;
        // Validate selected definitions; disabled definitions remain available to restore.
        if labels.is_empty() {continue;}
        let archive=!matches!(definition.structure,Some(crate::restore::Layout::Latest));
        let send:Send=serde_json::from_value(definition.send.clone().unwrap_or(serde_json::json!({})))?;
        let inspection=send.inspect.as_deref().unwrap_or("manifest");
        anyhow::ensure!(matches!(inspection,"file"|"none"|"manifest"),"send.inspect must be file, none or manifest");
        let inspect_none=inspection=="none";
        let inspect_manifest=inspection=="manifest";
        anyhow::ensure!(send.conflict.is_none(),"send.conflict has no working source consumer and is unsupported");
        let always=match send.cue.as_deref().unwrap_or("if-change") {
            "if-change"=>false,"always"=>true,_=>anyhow::bail!("send.cue must be if-change or always"),
        };
        let strategy=match send.strategy.as_deref().unwrap_or(if inspect_none {"upload-all"}else{"sync-diff"}) {
            "sync-diff"=>StoreStrategy::SyncDiff,"sync-purge"=>StoreStrategy::SyncPurge,
            "upload-all"=>StoreStrategy::UploadAll,"upload-missing"=>StoreStrategy::UploadMissing,
            _=>anyhow::bail!("Unsupported send.strategy"),
        };
        let path=definition.path.as_deref().context("Local publication requires explicit path")?;
        anyhow::ensure!(!path.trim().is_empty(),"Local publication requires nonempty path");
        let base=identity(&if Utf8Path::new(path).is_absolute(){path.into()}else{root.join(path)})?;
        anyhow::ensure!(!root.starts_with(&base),"Publication base contains the project root");
        for name in [".git",".hg",".svn",".projr",".proj"] {
            anyhow::ensure!(!overlaps(&base,&identity(&root.join(name))?),"Publication overlaps protected project state");
        }
        anyhow::ensure!(definition.path_append_label.unwrap_or(true),"Configured local publication currently requires path-append-label: true");
        let mut selected=Vec::new();
        for label in labels {
            let directory=config.directories.get(&label).context("Publication label is undeclared")?;
            anyhow::ensure!(directory.hash.unwrap_or(!label.to_ascii_lowercase().starts_with("cache")),
                "Publication content requires CSV capture; enable hash for label {label}");
            selected.push((label.clone(),identity(&config.get_path(root,&label)?)?));
        }
        targets.push(LocalPublication {title:title.clone(),base,labels:selected,append:definition.path_append_label.unwrap_or(true),archive,strategy,always,inspect_none,inspect_manifest});
    }
    if targets.is_empty() {return Ok(targets);}
    validate_namespaces(root,&targets,config)?;
    Ok(targets)
}
// Legacy reads remain supported, but publication must not silently create a
// second namespace or migrate/delete an existing remote's historical data.
fn reject_legacy_data(target:&LocalPublication)->anyhow::Result<()> {
    if !target.labels.iter().any(|(label,_)|label=="output-r-data") {return Ok(());}
    match fs::read_dir(&target.base) {
        Ok(entries)=>for entry in entries {
            let entry=entry.with_context(||format!("Cannot inspect publication base {}",target.base))?;
            let name=entry.file_name();
            if let Some(name)=name.to_str() {
                anyhow::ensure!(!name.eq_ignore_ascii_case("data")&&!name.eq_ignore_ascii_case("data-empty"),
                    "Legacy data namespace in {}; publication migration to output-r-data is unsupported",target.base);
            }
        },
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>return Ok(()),
        Err(error)=>return Err(error).with_context(||format!("Cannot inspect publication base {}",target.base)),
    }
    let path=target.base.join("VERSION");
    if path.try_exists()? {
        let text=crate::fs_utils::read_managed_text(&path)?;
        anyhow::ensure!(!text.lines().filter_map(|line|line.split_once(':')).any(|(label,_)|label.trim().eq_ignore_ascii_case("data")),
            "Legacy data VERSION advertisement in {}; publication migration to output-r-data is unsupported",target.base);
    }
    Ok(())
}
fn validate_namespaces(root:&Utf8Path,targets:&[LocalPublication],config:&crate::yml::ValidatedConfig)->anyhow::Result<()> {
    let sources=config.directories.keys().map(|label|identity(&config.get_path(root,label)?)).collect::<anyhow::Result<Vec<_>>>()?;
    let mut bases:Vec<&Utf8Path>=Vec::new();
    for target in targets {
        reject_legacy_data(target)?;
        for base in &bases {anyhow::ensure!(!overlaps(base,&target.base),"Configured publication bases overlap");}
        bases.push(&target.base);
        for source in &sources {anyhow::ensure!(!overlaps(source,&target.base),"Publication base overlaps a declared resource");}
        if let Some(remotes)=&config.remotes.local {
            for name in &config.dest {
                let cas=remotes.get(name).context("CAS target disappeared")?;
                anyhow::ensure!(!overlaps(&target.base,&identity(&cas.path)?),"Ordinary publication overlaps selected CAS store");
            }
        }
        let mut reserved:Vec<Utf8PathBuf>=Vec::new();
        for name in ["manifest.csv","VERSION","CHANGELOG.md"] {reserved.push(identity(&target.base.join(name))?);}
        for (label,_) in &target.labels {
            if target.archive {crate::local_remote::list_versions(&target.base,label,crate::local_remote::LocalStructure::Archive,target.append)?;}
            for empty in [false,true] {
                if target.archive&&empty {continue;}
                let path=identity(&if target.archive {target.base.join(label)}else{destination(target,label,empty,"")})?;
                for previous in &reserved {
                    anyhow::ensure!(!overlaps(&path,previous),"Publication namespace collision: {path} and {previous}");
                }
                reserved.push(path);
            }
        }
    }
    Ok(())
}
fn inventory(path:&Utf8Path)->anyhow::Result<BTreeMap<String,String>> {
    Ok(crate::manifest::inventory_directory(path,false)?.into_iter().collect())
}
fn write_bytes(path:&Utf8Path,bytes:&[u8])->anyhow::Result<()> {
    crate::path_safety::resolve_path(path.as_std_path(),true)?;
    let mode=match fs::symlink_metadata(path) {
        Ok(metadata)=>{anyhow::ensure!(metadata.is_file(),"Remote metadata must be regular: {path}");Some(metadata.permissions())},
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>None,
        Err(error)=>return Err(error.into()),
    };
    let mut temporary=tempfile::NamedTempFile::new_in(path.parent().context("Metadata has no parent")?)?;
    temporary.write_all(bytes)?;
    if let Some(mode)=mode {fs::set_permissions(temporary.path(),mode)?;}
    temporary.flush()?;
    crate::path_safety::resolve_path(path.as_std_path(),true)?;
    temporary.persist(path).map_err(|error|error.error).with_context(||format!("Cannot replace remote metadata {path}"))?;
    Ok(())
}
/// Read guarded ordinary remote VERSION entries without dropping history.
pub(crate) fn version_entries(path:&Utf8Path)->anyhow::Result<Vec<(String,String)>> {
    if !path.try_exists()? {return Ok(Vec::new());}
    let text=crate::fs_utils::read_managed_text(path)?;
    let mut result=Vec::new();
    for line in text.lines().filter(|line|!line.trim().is_empty()) {
        let (label,version)=line.split_once(':').context("Malformed remote VERSION")?;
        let label=label.trim();let version=version.trim();
        crate::path_safety::validate_directory_label(label)?;
        anyhow::ensure!(!result.iter().any(|(previous,_)|previous==label),"Duplicate remote VERSION label");
        anyhow::ensure!(crate::local_remote::version_numbers(version.strip_suffix('*').unwrap_or(version)).is_some(),"Invalid remote VERSION entry");
        result.push((label.to_owned(),version.to_owned()));
    }
    Ok(result)
}
/// Select one unambiguous label/version filename-to-MD5 inventory.
pub(crate) fn recorded_files(rows:&[ManifestRow],label:&str,version:&str)->Option<BTreeMap<String,String>> {
    let rows:Vec<_>=rows.iter().filter(|row|row.label==label&&row.version==version).collect();
    if rows.is_empty() {return None;}
    let mut files=BTreeMap::new();
    for row in &rows {
        if row.r#fn.is_empty() {return (rows.len()==1).then_some(BTreeMap::new());}
        if files.insert(row.r#fn.clone(),row.hash.clone()).is_some() {return None;}
    }
    Some(files)
}
fn trusted_comparison(target:&LocalPublication,versions:&[(String,String)],rows:&[ManifestRow],history:&[ManifestRow],label:&str,exists:bool)->Option<BTreeMap<String,String>> {
    if !target.inspect_manifest||!exists||matches!(target.strategy,StoreStrategy::UploadAll) {return None;}
    matching_history(versions,rows,history,label)
}
/// Return matching unstarred project/remote CSV rows; this is eligibility,
/// not an authentication or physical-byte guarantee.
pub(crate) fn matching_history(versions:&[(String,String)],rows:&[ManifestRow],history:&[ManifestRow],label:&str)->Option<BTreeMap<String,String>> {
    let (_,version)=versions.iter().find(|(key,value)|key==label&&!value.ends_with('*'))?;
    let local=recorded_files(history,label,version)?;
    let remote=recorded_files(rows,label,version)?;
    (local==remote).then_some(remote)
}

fn archive_comparison(target:&LocalPublication,remote:&[ManifestRow],versions:&[(String,String)],history:&[ManifestRow],label:&str,current:&str)->anyhow::Result<Option<BTreeMap<String,String>>> {
    use crate::local_remote::{LocalStructure,list_versions,version_numbers,compare_versions};
    let catalog=list_versions(&target.base,label,LocalStructure::Archive,target.append)?;
    let current_numbers=version_numbers(current).context("Invalid current archive version")?;
    let current_path=destination(target,label,false,current);
    for entry in &catalog {
        if compare_versions(&version_numbers(&entry.version).unwrap(),&current_numbers).is_eq() {
            let expected=destination(target,label,entry.empty,current);
            anyhow::ensure!(entry.path==expected,"Archive target has another numeric version spelling: {}",entry.path);
        }
    }
    let exists=current_path.try_exists()?||destination(target,label,true,current).try_exists()?;
    if target.inspect_none||matches!(target.strategy,StoreStrategy::UploadAll)||(target.always&&!exists) {return Ok(None);}
    if !target.inspect_manifest&&exists {return Ok(Some(inventory(&current_path)?));}
    let latest=match catalog.first(){Some(value)=>value,None=>return Ok(None)};
    let recorded=match archive_history(remote,versions,history,label,current,&latest.version) {
        Some(value)=>value,None=>return Ok(None),
    };
    if latest.empty {
        anyhow::ensure!(fs::read_dir(&latest.path)?.next().transpose()?.is_none(),"Archive empty marker contains files");
    }
    if target.inspect_manifest {Ok(Some(recorded))} else {
        let files=inventory(&latest.path)?;
        anyhow::ensure!(!latest.empty||files.is_empty(),"Archive empty marker contains files");
        Ok(Some(files))
    }
}
/// Select archive history only when newest catalog/VERSION/CSV agree and the
/// advertised version belongs to the continuous unchanged project-history suffix.
/// Physical ZIP/directory validation is separate and never establishes trust.
pub(crate) fn archive_history(remote:&[ManifestRow],versions:&[(String,String)],history:&[ManifestRow],label:&str,current:&str,latest_version:&str)->Option<BTreeMap<String,String>> {
    use crate::local_remote::{version_numbers,compare_versions};
    let (_,advertised)=match versions.iter().find(|(key,value)|key==label&&!value.ends_with('*')) {Some(value)=>value,None=>return None};
    if version_numbers(advertised)!=version_numbers(&latest_version) {return None;}
    let recorded=matching_history(versions,remote,history,label)?;
    let source=recorded_files(history,label,current)?;
    let current_numbers=version_numbers(current)?;
    let mut candidates:Vec<_>=history.iter().filter(|row|row.label==label)
        .filter_map(|row|version_numbers(&row.version).map(|numbers|(numbers,row.version.clone()))).collect();
    candidates.sort_by(|a,b|compare_versions(&b.0,&a.0));candidates.dedup();
    let mut earliest=current_numbers.clone();
    for (numbers,version) in candidates {
        if compare_versions(&numbers,&current_numbers).is_gt() {continue;}
        if recorded_files(history,label,&version).as_ref()!=Some(&source) {break;}
        earliest=numbers;
    }
    let numbers=version_numbers(advertised).unwrap();
    if compare_versions(&numbers,&earliest).is_lt()||compare_versions(&numbers,&current_numbers).is_gt() {return None;}
    Some(recorded)
}
/// Construct a target over a caller-owned private remote mirror. The caller
/// validates routing/labels and populates only the current and newest catalog
/// entries before invoking the shared publisher; no external destination here.
pub(crate) fn staging_target(base:Utf8PathBuf,labels:Vec<(String,Utf8PathBuf)>,archive:bool,strategy:StoreStrategy,always:bool,inspection:&str)->LocalPublication {
    LocalPublication{title:"GitHub private staging".into(),base,labels,append:true,archive,strategy,always,
        inspect_none:inspection=="none",inspect_manifest:inspection=="manifest"}
}
/// Replace one label value while retaining every other VERSION entry.
pub(crate) fn set_version(entries:&mut Vec<(String,String)>,label:&str,version:String) {
    if let Some((_,value))=entries.iter_mut().find(|(key,_)|key==label) {*value=version;}
    else {entries.push((label.into(),version));}
}
struct Prepared {
    target:LocalPublication,
    rows:Vec<ManifestRow>,
    versions:Vec<(String,String)>,
    files:tempfile::TempDir,
    // Frozen historical comparisons for every selected archive label.
    archive_comparisons:BTreeMap<String,Option<BTreeMap<String,String>>>,
}
/// Execute frozen local latest targets after the local release boundary.
/// Stages only current CSV-selected bytes for every target before the first
/// remote mutation. Returns labels whose current data needed file actions, a new
/// target or empty-variant normalization. Metadata-only updates are excluded.
/// Data precedes shared CSV/VERSION/changelog, like projr.
/// This is not a remote transaction: failure preserves the local release and
/// reports partial external effects; no automatic retry or remote rollback.
pub(crate) fn publish_local(root:&Utf8Path,targets:&[LocalPublication],version:&str)->anyhow::Result<Vec<String>> {
    if targets.is_empty(){return Ok(Vec::new());}
    let mut data_effects=Vec::new();
    let history=crate::manifest::read(&root.join("manifest.csv"))?;
    let changelog=root.join("CHANGELOG.md");
    let changelog=if changelog.try_exists()? {
        crate::path_safety::resolve_path(changelog.as_std_path(),true)?;
        anyhow::ensure!(fs::symlink_metadata(&changelog)?.is_file(),"Project changelog must be regular");
        Some(fs::read(&changelog)?)
    } else {None};
    let mut prepared=Vec::new();
    let result=(|| -> anyhow::Result<()> {
    for target in targets {
        reject_legacy_data(target)?;
        let rows=crate::manifest::read(&target.base.join("manifest.csv"))?;
        let versions=version_entries(&target.base.join("VERSION"))?;
        for name in ["manifest.csv","VERSION","CHANGELOG.md"] {
            crate::path_safety::resolve_path(target.base.join(name).as_std_path(),true)?;
            if let Ok(metadata)=fs::symlink_metadata(target.base.join(name)) {anyhow::ensure!(metadata.is_file(),"Remote metadata must be regular");}
        }
        let files=tempfile::Builder::new().prefix("proj-publication-").tempdir_in(fs::canonicalize(std::env::temp_dir())?)?;
        let mut archive_comparisons=BTreeMap::new();
        if target.archive {
            for (label,_) in &target.labels {
                archive_comparisons.insert(label.clone(),archive_comparison(target,&rows,&versions,&history,label,version)?);
            }
        }
        prepared.push(Prepared {target:target.clone(),rows,versions,files,archive_comparisons});
        let item=prepared.last().unwrap();
        let staged=Utf8Path::from_path(item.files.path()).context("Publication staging path must be UTF-8")?;
        for (label,source) in &target.labels {
            let selected:Vec<_>=history.iter().filter(|row|row.label==*label&&row.version==version).collect();
            anyhow::ensure!(!selected.is_empty(),"Current CSV has no publication rows for {label}");
            let directory=staged.join(label);fs::create_dir(&directory)?;
            let mut expected=BTreeMap::new();
            let mut modes=BTreeMap::new();
            match fs::symlink_metadata(source) {
                Ok(metadata)=>{anyhow::ensure!(metadata.is_dir(),"Publication source root must be a directory");modes.insert(Utf8PathBuf::new(),metadata.permissions());},
                Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{},
                Err(error)=>return Err(error.into()),
            }
            for row in &selected {
                if row.r#fn.is_empty(){anyhow::ensure!(selected.len()==1,"Ambiguous empty CSV marker");continue;}
                anyhow::ensure!(expected.insert(row.r#fn.clone(),row.hash.clone()).is_none(),"Duplicate current publication filename");
                let input=source.join(&row.r#fn);
                crate::path_safety::resolve_path(input.as_std_path(),true)?;
                anyhow::ensure!(fs::symlink_metadata(&input)?.is_file(),"CSV publication input must be regular");
                let output=directory.join(&row.r#fn);fs::create_dir_all(output.parent().unwrap())?;fs::copy(&input,&output)?;
                for parent in Utf8Path::new(&row.r#fn).parent().unwrap().ancestors() {
                    modes.insert(parent.to_owned(),fs::symlink_metadata(source.join(parent))?.permissions());
                }
            }
            anyhow::ensure!(inventory(&directory)?==expected,"Publication source bytes disagree with current CSV for {label}");
            let mut modes:Vec<_>=modes.into_iter().collect();
            modes.sort_by_key(|(relative,_)|std::cmp::Reverse(relative.components().count()));
            for (relative,mode) in modes {fs::set_permissions(directory.join(relative),mode)?;}
            native_name(&target.base)?;
            inventory(&destination(target,label,false,version))?;
            let empty=destination(target,label,true,version);
            crate::path_safety::resolve_path(empty.as_std_path(),true)?;
            if empty.try_exists()? {anyhow::ensure!(fs::read_dir(&empty)?.next().transpose()?.is_none(),"Empty remote marker contains entries");}
        }
    }
    for item in &mut prepared {
        let target=&item.target;
        let result=(|| -> anyhow::Result<()> {
            crate::path_safety::resolve_path(target.base.as_std_path(),true)?;
            native_name(&target.base)?;
            fs::create_dir_all(&target.base)?;
            native_name(&target.base)?;
            set_version(&mut item.versions,"Project",version.into());
            for (label,_) in &target.labels {
                let source=Utf8Path::from_path(item.files.path()).unwrap().join(label);
                let full=destination(target,label,false,version);let empty=destination(target,label,true,version);
                native_name(&full)?;native_name(&empty)?;
                let existed=full.try_exists()?||empty.try_exists()?;
                let source_files=inventory(&source)?;
                // projr none treats every source file as extra. Missing/diff
                // therefore upload blindly and retain extras. Purge only runs
                // when source files are present: an empty source cannot infer
                // destination extras without a comparison.
                let strategy=if target.inspect_none {
                    if matches!(target.strategy,StoreStrategy::SyncPurge)&&!source_files.is_empty() {StoreStrategy::SyncPurge}
                    else {StoreStrategy::UploadAll}
                }else{target.strategy};
                let comparison=if target.archive {item.archive_comparisons.get(label).cloned().flatten()}
                    else {trusted_comparison(target,&item.versions,&item.rows,&history,label,existed)};
                let actual=inventory(&full)?;
                let full_before=full.try_exists()?;
                let (uploads,_,removals,_)=crate::local_store::actions(&source_files,comparison.as_ref().unwrap_or(&actual),strategy);
                let changed=!uploads.is_empty()||!removals.is_empty();
                let metadata_missing_or_untrusted=!item.versions.iter().any(|(key,value)|key==label&&!value.ends_with('*'));
                let ensure=if target.archive {changed||target.always||comparison.is_none()}else{
                    target.inspect_none||changed||!existed||target.always
                    || (matches!(target.strategy,StoreStrategy::SyncDiff)
                        &&(metadata_missing_or_untrusted||(target.inspect_manifest&&comparison.is_none())))
                };
                if ensure {
                    fs::create_dir_all(full.parent().context("Publication destination has no parent")?)?;
                    native_name(&full)?;
                    if target.archive&&!existed {
                        // A new archive is always complete, never just the
                        // changed subset relative to an older comparison.
                        crate::local_store::store_files(root,&source,&full,StoreStrategy::SyncPurge,false)?;
                    } else if let Some(comparison)=&comparison {
                        crate::local_store::store_compared_files(root,&source,&full,strategy,comparison)?;
                    } else {crate::local_store::store_files(root,&source,&full,strategy,false)?;}
                }
                let mut normalized_empty=false;
                if full.try_exists()? {
                    if inventory(&full)?.is_empty() {
                        normalized_empty=full_before;
                        let mode=fs::symlink_metadata(&full)?.permissions();
                        fs::create_dir_all(&empty)?;fs::set_permissions(&empty,mode)?;
                        // Retire a verified file-empty tree through private staging;
                        // cleanup failures are reported after the visible change.
                        let retired=tempfile::Builder::new().prefix(".proj-retired-empty-").tempdir_in(&target.base)?;
                        crate::path_safety::resolve_path(full.as_std_path(),true)?;
                        fs::rename(&full,retired.path().join("previous"))?;
                        let path=retired.path().to_owned();
                        if let Err(error)=crate::fs_utils::prepare_owned_cleanup(retired.path()) {
                            let retained=retired.keep();
                            anyhow::bail!("Empty variant installed; cannot prepare cleanup: {error:#}; retained at {}",retained.display());
                        }
                        retired.close().with_context(||format!("Empty variant installed, but cleanup failed at {}",path.display()))?;
                    } else if empty.try_exists()? {fs::remove_dir(&empty)?;}
                }
                if (ensure&&(changed||!existed))||normalized_empty {data_effects.push(label.clone());}
                let update=if target.archive&&!existed {ensure}else{match target.strategy {
                    StoreStrategy::SyncDiff=>changed||ensure,
                    _=>changed||!existed,
                }};
                if update {
                    let untrusted=existed&&(matches!(target.strategy,StoreStrategy::UploadAll|StoreStrategy::UploadMissing)
                        ||(target.inspect_none&&matches!(target.strategy,StoreStrategy::SyncDiff)));
                    set_version(&mut item.versions,label,format!("{version}{}",if untrusted{"*"}else{""}));
                }
                item.rows.retain(|row|!(row.label==*label&&row.version==version));
                item.rows.extend(history.iter().filter(|row|row.label==*label&&row.version==version).cloned());
            }
            crate::manifest::write(&target.base.join("manifest.csv"),&crate::manifest::deduplicate(item.rows.clone()))?;
            let text=item.versions.iter().map(|(label,version)|format!("{label}: {version}\n")).collect::<String>();
            write_bytes(&target.base.join("VERSION"),text.as_bytes())?;
            if let Some(changelog)=&changelog {
                write_bytes(&target.base.join("CHANGELOG.md"),changelog)?;
            }
            Ok(())
        })();
        result.with_context(||format!("Local destination {} may be partially published; inspect before retry",target.title))?;
    }
    Ok(())
    })();
    let mut cleanup_errors=Vec::new();
    for item in prepared {
        let path=item.files.path().to_owned();
        if let Err(error)=crate::fs_utils::prepare_owned_cleanup(&path) {
            let retained=item.files.keep();
            cleanup_errors.push(format!("Cannot prepare staging cleanup: {error:#}; retained at {}",retained.display()));
        } else if let Err(error)=item.files.close() {
            cleanup_errors.push(format!("Staging cleanup failed at {}: {error}",path.display()));
        }
    }
    if !cleanup_errors.is_empty() {
        let cleanup=cleanup_errors.join("; ");
        return match result {
            Ok(())=>Err(anyhow::anyhow!("Remote publication completed; {cleanup}")),
            Err(error)=>Err(error.context(cleanup)),
        };
    }
    result.map(|()|data_effects)
}
