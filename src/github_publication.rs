//! Configured GitHub latest/archive publication with explicit none inspection and all four strategies.
//! All selected bytes, merged data ZIPs and metadata stage before remote writes.
use std::{collections::BTreeSet,fs};
use anyhow::Context;
use camino::{Utf8Path,Utf8PathBuf};
use serde_json::{Map,Value};
use crate::local_store::StoreStrategy;
use crate::{github_release::GithubReleaseClient,github_zip::{GithubStructure,MetadataKind}};

/// Validated configuration without credentials, HTTP or local mutation.
pub(crate) struct Spec {
    title:String,
    prefix:Option<String>,
    append:bool,
    latest:bool,
    strategy:StoreStrategy,
    labels:Vec<(String,Utf8PathBuf)>,
}
/// Frozen repository, API origin, token and tag selected before hooks.
/// Deliberately has no Debug implementation: the client may contain credentials.
pub(crate) struct Target {spec:Spec,client:GithubReleaseClient,tag:String}
fn folded(value:&str)->String {
    icu_normalizer::ComposingNormalizer::new_nfc().normalize(&value.to_uppercase()).into_owned()
}
fn name(spec:&Spec,label:&str,version:&str,empty:bool)->anyhow::Result<String> {
    crate::github_zip::directory_asset_name(spec.prefix.as_deref(),spec.append,label,if spec.latest {GithubStructure::Latest}else{GithubStructure::Archive},Some(version),empty)
}
/// Validate the executable inline subset without touching Git, HTTP or files.
pub(crate) fn validate(root:&Utf8Path,raw:Option<&Map<String,Value>>,config:&crate::yml::ValidatedConfig)->anyhow::Result<Vec<Spec>> {
    let declared:Map<String,Value>=config.directories.keys().map(|key|(key.clone(),Value::Null)).collect();
    let context=serde_json::json!({"directories":declared});
    let mut specs=Vec::new();
    for (title,value) in raw.into_iter().flatten() {
        anyhow::ensure!(!title.trim_matches(' ').is_empty()&&!title.chars().any(char::is_control),"Invalid GitHub destination title");
        let definition=crate::restore::definition(value)?;
        let mut labels=if let Some(Value::Array(values))=&definition.content {
            values.iter().map(|value|value.as_str().map(str::to_owned).context("Content labels must be strings")).collect::<anyhow::Result<Vec<_>>>()?
        }else{
            let mut labels=config.directories.keys().filter_map(|label|match crate::restore::content_has(definition.content.as_ref(),label,&context) {
                Ok(true)=>Some(Ok(label.clone())),Ok(false)=>None,Err(error)=>Some(Err(error)),
            }).collect::<anyhow::Result<Vec<_>>>()?;labels.sort();labels
        };
        for label in &labels {crate::restore::content_has(definition.content.as_ref(),label,&context)?;}
        crate::path_safety::validate_directory_labels(labels.iter().map(String::as_str))?;
        if labels.is_empty(){continue;}
        anyhow::ensure!(definition.cue.is_none(),"Use send.cue, not a title-level cue");
        let send:crate::publication::Send=serde_json::from_value(definition.send.unwrap_or(serde_json::json!({})))?;
        anyhow::ensure!(send.inspect.as_deref()==Some("none"),"Configured GitHub publication currently requires explicit send.inspect: none; manifest/file remain pending");
        let strategy=match send.strategy.as_deref().unwrap_or("upload-all") {
            "upload-all"=>StoreStrategy::UploadAll,"upload-missing"=>StoreStrategy::UploadMissing,
            "sync-diff"=>StoreStrategy::SyncDiff,"sync-purge"=>StoreStrategy::SyncPurge,
            _=>anyhow::bail!("Unknown GitHub send.strategy"),
        };
        anyhow::ensure!(matches!(send.cue.as_deref().unwrap_or("if-change"),"if-change"|"always"),"send.cue must be if-change or always");
        anyhow::ensure!(send.conflict.is_none(),"send.conflict has no working source consumer");
        let mut selected=Vec::new();
        for label in labels.drain(..) {
            let directory=config.directories.get(&label).context("GitHub publication label is undeclared")?;
            anyhow::ensure!(directory.hash.unwrap_or(!label.to_ascii_lowercase().starts_with("cache")),"GitHub publication requires CSV capture for {label}");
            let path=Utf8PathBuf::try_from(crate::path_safety::resolve_path(config.get_path(root,&label)?.as_std_path(),true)?)?;
            selected.push((label,path));
        }
        let spec=Spec{title:title.clone(),prefix:definition.path,append:definition.path_append_label.unwrap_or(true),latest:matches!(definition.structure,Some(crate::restore::Layout::Latest)),strategy,labels:selected};
        // Version suffix is constant across selected labels, so this catches
        // planned namespace aliases before the actual release version is known.
        let mut names=BTreeSet::from([folded("manifest.zip"),folded("VERSION.zip"),folded("CHANGELOG.zip")]);
        for (label,_) in &spec.labels {for empty in [false,true] {
            anyhow::ensure!(names.insert(folded(&name(&spec,label,"v0.0.1",empty)?)),"Configured GitHub asset namespace collision");
        }}
        specs.push(spec);
    }
    Ok(specs)
}
/// Freeze all security-sensitive routing before hooks. No request is sent.
pub(crate) fn capture(root:&Utf8Path,specs:Vec<Spec>,environment:&crate::run_environment::RunEnvironment,version:&str)->anyhow::Result<Vec<Target>> {
    if specs.is_empty(){return Ok(Vec::new());}
    let api=environment.text("GITHUB_API_URL").unwrap_or("https://api.github.com");
    let repository=crate::restore::repository_in(root,None,api,environment)?;
    let explicit=["GITHUB_PAT","GH_TOKEN","GITHUB_TOKEN"].iter()
        .find_map(|key|environment.text(key).filter(|value|!value.trim().is_empty()).map(str::to_owned));
    let origin:ureq::http::Uri=api.parse().context("Invalid GitHub API base")?;
    let public=origin.scheme_str()==Some("https")&&origin.host().is_some_and(|host|host.eq_ignore_ascii_case("api.github.com"))
        &&origin.port_u16().unwrap_or(443)==443;
    let token=match explicit {
        Some(token)=>token,
        None=>{anyhow::ensure!(public,"A non-public GitHub API requires an explicit environment token; github.com credential helpers are not used");
            crate::git::get_github_token_in(environment)?},
    };
    let mut tags=BTreeSet::new();let mut targets=Vec::new();
    for spec in specs {
        let tag=if spec.title=="@version" {version.into()}else{spec.title.trim_matches(' ').replace(' ',"-")};
        anyhow::ensure!(tags.insert(tag.clone()),"Configured GitHub titles resolve to the same Release tag");
        let client=GithubReleaseClient::new(&repository,api,Some(token.clone()))?;
        targets.push(Target{spec,client,tag});
    }
    Ok(targets)
}
fn metadata(client:&GithubReleaseClient,tag:&str,kind:MetadataKind,stage:&Utf8Path,assets:&BTreeSet<String>)->anyhow::Result<()> {
    let (name,member)=kind.names();
    if !assets.contains(name){return Ok(());}
    let archive=stage.join(format!("{name}.download"));
    client.download_asset(stage,tag,name,&archive,128*1024*1024)?;
    let extracted=crate::github_zip::extract_zip(&archive,128*1024*1024)?;
    let result=(||->anyhow::Result<()> {
        let root=Utf8Path::from_path(extracted.path()).context("Metadata staging path must be UTF-8")?;
        let entries=fs::read_dir(root)?.collect::<Result<Vec<_>,_>>()?;
        anyhow::ensure!(entries.len()==1&&entries[0].file_name()==std::ffi::OsStr::new(member)&&entries[0].file_type()?.is_file(),"GitHub metadata ZIP must contain exactly {member}");
        fs::copy(root.join(member),stage.join(member))?;Ok(())
    })();
    let cleanup=extracted.close();
    match (result,cleanup){
        (Err(error),Err(cleanup))=>Err(error.context(format!("Metadata cleanup also failed: {cleanup:#}"))),
        (Err(error),Ok(()))=>Err(error),(Ok(()),result)=>result
    }
}
fn current_archive(spec:&Spec,label:&str,current:&str,asset:&str)->anyhow::Result<bool> {
    let stem=crate::github_zip::directory_asset_name(spec.prefix.as_deref(),spec.append,label,GithubStructure::Latest,None,false)?;
    let prefix=folded(&(stem.strip_suffix(".zip").unwrap().to_owned()+"-v"));
    let asset=folded(asset);
    let Some(version)=asset.strip_prefix(&prefix).and_then(|value|value.strip_suffix(".ZIP")) else{return Ok(false)};
    let version=version.strip_suffix("-EMPTY").unwrap_or(version);
    if !version.as_bytes().first().is_some_and(u8::is_ascii_digit){return Ok(false);}
    let numbers=crate::local_remote::version_numbers(version).context("Invalid numeric GitHub archive asset")?;
    let current=crate::local_remote::version_numbers(current).context("Invalid current archive version")?;
    Ok(crate::local_remote::compare_versions(&numbers,&current).is_eq())
}
// Reuse the guarded directory merge; never re-read unrecorded project files.
fn data_zip(stage:&Utf8Path,source:&Utf8Path,files:&std::collections::BTreeMap<String,String>,
    existing:Option<&Utf8Path>,strategy:StoreStrategy)->anyhow::Result<(tempfile::NamedTempFile,bool)> {
    let (zip,empty)=crate::github_zip::recorded_directory_zip(source,files)?;
    let Some(existing)=existing else{return Ok((zip,empty));};
    let result=(|| {
        let extracted=crate::github_zip::extract_zip(Utf8Path::from_path(zip.path()).context("ZIP staging path must be UTF-8")?,128*1024*1024)?;
        let result=(|| {
            let selected=Utf8Path::from_path(extracted.path()).context("Extraction path must be UTF-8")?;
            crate::local_store::store_files(stage,selected,existing,strategy,false)?;
            crate::github_zip::directory_zip(existing)
        })();
        let cleanup=extracted.close();
        match (result,cleanup) {
            (Err(error),Err(cleanup))=>Err(error.context(format!("Selected ZIP cleanup failed: {cleanup:#}"))),
            (Err(error),Ok(()))=>Err(error),(Ok(_),Err(error))=>Err(error),(Ok(value),Ok(()))=>Ok(value),
        }
    })();
    let path=zip.path().to_owned();
    match (result,zip.close()) {
        (Err(error),Err(cleanup))=>Err(error.context(format!("Source ZIP cleanup failed at {}: {cleanup}",path.display()))),
        (Err(error),Ok(()))=>Err(error),(Ok(_),Err(error))=>Err(anyhow::anyhow!("Source ZIP cleanup failed at {}: {error}",path.display())),
        (Ok(value),Ok(()))=>Ok(value),
    }
}
/// Prepare all data/metadata archives, then upload data before shared metadata.
/// Existing canonical data merges selected bytes, retaining remote-only files.
/// Replacement is delete-then-upload, with honest partial/uncertain failure.
pub(crate) fn publish(root:&Utf8Path,targets:&[Target],version:&str)->anyhow::Result<()> {
    if targets.is_empty(){return Ok(());}
    let history=crate::manifest::read(&root.join("manifest.csv"))?;
    let mut prepared:Vec<Vec<(String,Option<tempfile::NamedTempFile>,bool)>>=Vec::new();
    let result=(||->anyhow::Result<()> {
    for target in targets {
        let catalog=target.client.assets_if_exists(&target.tag)?.unwrap_or_default();
        let mut actual=BTreeSet::new();
        for asset in &catalog {anyhow::ensure!(actual.insert(asset.name.clone()),"GitHub asset catalog contains duplicate names");}
        let mut expected=vec!["manifest.zip".to_owned(),"VERSION.zip".to_owned(),"CHANGELOG.zip".to_owned()];
        for (label,_) in &target.spec.labels {
            expected.push(name(&target.spec,label,version,false)?);expected.push(name(&target.spec,label,version,true)?);
        }
        for planned in &expected {for existing in &actual {
            anyhow::ensure!(planned==existing||folded(planned)!=folded(existing),"GitHub asset namespace alias: {planned}");
        }}
        for (label,_) in &target.spec.labels {
            for existing in &actual {
                anyhow::ensure!(target.spec.latest||!current_archive(&target.spec,label,version,existing)?
                    ||*existing==name(&target.spec,label,version,false)?||*existing==name(&target.spec,label,version,true)?,
                    "Current GitHub archive already exists with another numeric spelling; no upload performed");
            }
            if label=="output-r-data" {
                let legacy=crate::github_zip::directory_asset_name(target.spec.prefix.as_deref(),target.spec.append,"data",GithubStructure::Latest,None,false)?;
                let canonical=crate::github_zip::directory_asset_name(target.spec.prefix.as_deref(),target.spec.append,label,GithubStructure::Latest,None,false)?;
                if folded(&legacy)!=folded(&canonical) {
                    let prefix=legacy.strip_suffix(".zip").unwrap().to_owned()+"-v";
                    let empty=legacy.strip_suffix(".zip").unwrap().to_owned()+"-empty.zip";
                    anyhow::ensure!(!actual.iter().any(|name|if target.spec.latest {
                        folded(name)==folded(&legacy)||folded(name)==folded(&empty)
                    }else{folded(name).starts_with(&folded(&prefix))}),"Legacy GitHub data namespace requires explicit migration");
                }
            }
        }
        let temporary=tempfile::Builder::new().prefix("proj-github-publication-").tempdir_in(fs::canonicalize(std::env::temp_dir())?)?;
        let stage=Utf8Path::from_path(temporary.path()).context("GitHub staging path must be UTF-8")?;
        prepared.push(Vec::new());
        let uploads=prepared.last_mut().unwrap();
        let result=(||->anyhow::Result<()> {
            metadata(&target.client,&target.tag,MetadataKind::Manifest,stage,&actual)?;
            metadata(&target.client,&target.tag,MetadataKind::Version,stage,&actual)?;
            let mut rows=crate::manifest::read(&stage.join("manifest.csv"))?;
            let mut versions=crate::publication::version_entries(&stage.join("VERSION"))?;
            anyhow::ensure!(!target.spec.labels.iter().any(|(label,_)|label=="output-r-data")||!versions.iter().any(|(label,_)|label.eq_ignore_ascii_case("data")),"Legacy GitHub data advertisement requires explicit migration");
            crate::publication::set_version(&mut versions,"Project",version.into());
            fs::create_dir(stage.join("data"))?;
            for (label,source) in &target.spec.labels {
                let files=crate::publication::recorded_files(&history,label,version).context("Current GitHub CSV rows are missing or ambiguous")?;
                let full=name(&target.spec,label,version,false)?;
                let empty_name=name(&target.spec,label,version,true)?;
                anyhow::ensure!(!(actual.contains(&full)&&actual.contains(&empty_name)),"Both full and empty GitHub assets exist for {label}; no replacement performed");
                let existing=if actual.contains(&full){Some((full.as_str(),false))}
                    else if actual.contains(&empty_name){Some((empty_name.as_str(),true))}else{None};
                let merged=stage.join("data").join(label);
                // Even an empty source validates an existing ZIP before metadata changes.
                if let Some((asset,empty))=existing {
                    crate::github_zip::download_directory(&target.client,stage,&target.tag,asset,&merged,
                        128*1024*1024,128*1024*1024,true,empty)?;
                }
                if !files.is_empty()||existing.is_none() {
                    let (zip,empty)=data_zip(stage,source,&files,existing.map(|_|merged.as_path()),
                        if matches!(target.spec.strategy,StoreStrategy::SyncPurge) {StoreStrategy::SyncPurge}else{StoreStrategy::UploadAll})?;
                    let asset=if empty {empty_name.clone()}else{full.clone()};
                    uploads.push((asset.clone(),Some(zip),existing.is_some()));
                    if let Some((previous,_))=existing.filter(|(previous,_)|*previous!=asset) {
                        uploads.push((previous.into(),None,false));
                    }
                }
                // With no comparison missing/diff blindly select every source
                // file. Purge knows the complete replacement when nonempty.
                // Diff also advances untrusted metadata on an empty no-op.
                if !files.is_empty()||existing.is_none()||matches!(target.spec.strategy,StoreStrategy::SyncDiff) {
                    let untrusted=existing.is_some()&&!matches!(target.spec.strategy,StoreStrategy::SyncPurge);
                    crate::publication::set_version(&mut versions,label,format!("{version}{}",if untrusted {"*"}else{""}));
                }
                rows.retain(|row|!(row.label==*label&&row.version==version));
                rows.extend(history.iter().filter(|row|row.label==*label&&row.version==version).cloned());
            }
            crate::manifest::write(&stage.join("manifest.csv"),&crate::manifest::deduplicate(rows))?;
            crate::publication::set_version(&mut versions,"Project",version.into());
            fs::write(stage.join("VERSION"),versions.iter().map(|(label,version)|format!("{label}: {version}\n")).collect::<String>())?;
            for kind in [MetadataKind::Manifest,MetadataKind::Version] {
                uploads.push((kind.names().0.into(),Some(crate::github_zip::metadata_zip(stage,kind)?),true));
            }
            let changelog=root.join("CHANGELOG.md");
            if changelog.try_exists()? {
                crate::path_safety::resolve_path(changelog.as_std_path(),true)?;
                anyhow::ensure!(fs::symlink_metadata(&changelog)?.is_file(),"Project changelog must be regular");
                fs::copy(changelog,stage.join("CHANGELOG.md"))?;
                uploads.push(("CHANGELOG.zip".into(),Some(crate::github_zip::metadata_zip(stage,MetadataKind::Changelog)?),true));
            }
            Ok(())
        })();
        let cleanup_path=temporary.path().to_owned();
        let cleanup=match crate::fs_utils::prepare_owned_cleanup(&cleanup_path) {
            Ok(())=>temporary.close().map_err(anyhow::Error::from).with_context(||format!("Metadata staging cleanup failed at {}",cleanup_path.display())),
            Err(error)=>{let retained=temporary.keep();Err(error.context(format!("Metadata staging retained at {}",retained.display())))},
        };
        match (result,cleanup){
            (Err(error),Err(cleanup))=>return Err(error.context(format!("Metadata staging cleanup also failed: {cleanup}"))),
            (Err(error),Ok(()))=>return Err(error),(Ok(()),Err(error))=>return Err(error.into()),(Ok(()),Ok(()))=>{}
        }
    }
        for (target,uploads) in targets.iter().zip(&prepared) {
            for (name,zip,replace) in uploads {
                let result=if let Some(zip)=zip {
                    let source=Utf8Path::from_path(zip.path()).context("ZIP staging path must be UTF-8")?;
                    if *replace {target.client.replace_asset(&target.tag,name,source,true)}else{target.client.upload_asset(&target.tag,name,source,true)}.map(|_|())
                }else{target.client.delete_asset(&target.tag,name)};
                result.with_context(||format!("GitHub Release {} may be partially published; inspect before retry",target.tag))?;
            }
        }
        Ok(())
    })();
    let mut failures=Vec::new();
    for uploads in prepared {for (_,zip,_) in uploads {
        if let Some(zip)=zip {
            let path=zip.path().to_owned();
            if let Err(error)=zip.close(){failures.push(format!("{}: {error}",path.display()));}
        }
    }}
    if !failures.is_empty() {return Err(result.err().unwrap_or_else(||anyhow::anyhow!("GitHub publication completed")).context(format!("ZIP cleanup failed: {}",failures.join("; "))));}
    result
}
