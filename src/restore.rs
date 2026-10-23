//! Configured single-label restore using existing projr inline remote definitions.
//! No automatic publication or future top-level registry schema is introduced.
use std::fs;
use anyhow::Context;
use camino::{Utf8Path,Utf8PathBuf};
use serde::{Deserialize,Serialize};
use serde_json::Value;
use crate::local_remote::{LocalStructure,version_numbers,compare_versions};
use crate::github_zip::{GithubStructure,directory_asset_name};

/// Explicit configured restore controls. Missing filters search directory source
/// before eligible build destinations, preserving merged declaration order.
#[derive(Default)]
pub struct RestoreRequest {
    /// Declared label to restore (required; no implicit role or path).
    pub label:String,
    /// Restrict lookup to source or dest; None searches both in that order.
    pub position:Option<String>,
    /// Restrict the provider to local or github.
    pub provider:Option<String>,
    /// Restrict the existing inline title/reference.
    pub title:Option<String>,
    /// Exact archive version override; None selects newest remote version.
    pub remote_version:Option<String>,
    /// Explicit GitHub owner/repository, avoiding any Git dependency.
    pub repository:Option<String>,
    /// Explicit active profiles; local overrides remain highest.
    pub profile:Option<String>,
    /// Clear destination-only files only after valid complete staging.
    pub clear:bool,
    /// Maximum compressed GitHub asset bytes.
    pub max_bytes:u64,
    /// Maximum expanded GitHub ZIP bytes.
    pub max_expanded_bytes:u64,
}
/// One result. A skipped empty/unrecorded label never clears local files.
#[derive(Debug,Serialize)]
pub struct RestoreResult {
    /// Canonical configured label.
    pub label:String,
    /// Explicit final configured target.
    pub destination:Utf8PathBuf,
    /// Selected provider, absent when local history says nothing to restore.
    pub provider:Option<String>,
    /// Selected title, distinct from the transport's normalized tag.
    pub title:Option<String>,
    /// Selected version; not a CSV-integrity/trust guarantee.
    pub version:Option<String>,
    /// Whether transfer and installation completed.
    pub restored:bool,
    /// no-recorded-files or empty-remote, otherwise None.
    pub skipped:Option<String>,
}
#[derive(Clone,Copy,Deserialize)]
#[serde(rename_all="lowercase")]
enum Layout {Latest,Archive}
#[derive(Clone,Deserialize)]
#[serde(deny_unknown_fields,rename_all="kebab-case")]
struct Definition {
    path:Option<String>,
    structure:Option<Layout>,
    path_append_label:Option<bool>,
    content:Option<Value>,
    source:Option<Value>,
    send:Option<Value>,
    get:Option<Value>,
    cue:Option<Value>,
    id:Option<Value>,
}
struct Source {provider:String,title:String,definition:Definition}
fn definition(value:&Value)->anyhow::Result<Definition> {
    let parsed:Definition=serde_json::from_value(value.clone()).context("Invalid ordinary restore remote definition")?;
    if let Some(get)=&parsed.get {
        anyhow::ensure!(get.is_object()&&get.as_object().unwrap().is_empty(),
            "get strategy/conflict are not implemented; restore uses explicit --clear");
    }
    // Restore does not execute upload policy. Keep those nodes typed but do not
    // silently enable them in builds (the normal build parser still rejects them).
    if let Some(send)=&parsed.send {anyhow::ensure!(send.is_object(),"send must be a mapping");}
    if let Some(cue)=&parsed.cue {anyhow::ensure!(cue.is_string(),"cue must be a string");}
    if let Some(id)=&parsed.id {anyhow::ensure!(id.is_string(),"id must be a string");}
    Ok(parsed)
}
fn referenced(raw:&Value,provider:&str,title:&str,inline:Option<&Value>)->anyhow::Result<Definition> {
    anyhow::ensure!(!title.is_empty()&&!title.chars().any(char::is_control),"Invalid remote title");
    let shared=raw.get("build").and_then(|v|v.get(provider)).and_then(|v|v.get(title));
    let value=match (shared,inline) {
        (Some(shared),Some(inline))=>crate::yml::deep_merge(shared.clone(),inline.clone()),
        (Some(shared),None)=>shared.clone(),
        (None,Some(inline))=>inline.clone(),
        (None,None)=>anyhow::bail!("Directory source references an undefined build remote"),
    };
    definition(&value)
}
fn provider_allowed(provider:&str,request:&RestoreRequest)->bool {
    request.provider.as_deref().is_none_or(|filter|filter==provider)
}
fn title_allowed(title:&str,request:&RestoreRequest)->bool {
    request.title.as_deref().is_none_or(|filter|filter==title)
}
fn content_has(value:Option<&Value>,label:&str,raw:&Value)->anyhow::Result<bool> {
    match value {
        None|Some(Value::Null)|Some(Value::Bool(false))=>Ok(false),
        Some(Value::Bool(true))=>Ok(!label.to_ascii_lowercase().starts_with("cache")),
        Some(Value::Array(labels))=>{
            let declared=raw["directories"].as_object().context("Directories must be a mapping")?;
            for label in labels {
                let label=label.as_str().context("Remote content must contain label strings")?;
                anyhow::ensure!(declared.contains_key(label),"Remote content contains an undeclared label");
            }
            Ok(labels.iter().any(|item|item.as_str()==Some(label)))
        },
        _=>anyhow::bail!("Remote content must be boolean or a label list"),
    }
}
fn select_source(raw:&Value,label:&str,request:&RestoreRequest)->anyhow::Result<Source> {
    if request.position.as_deref()!=Some("dest") {
        if let Some(sources)=raw["directories"][label].get("source").filter(|v|!v.is_null()) {
            for (provider,entries) in sources.as_object().context("Directory source must be a provider mapping")? {
                anyhow::ensure!(matches!(provider.as_str(),"local"|"github"),"Unsupported source provider");
                if !provider_allowed(provider,request) {continue;}
                let candidates:Vec<(String,Option<&Value>)>=match entries {
                    Value::String(title)=>vec![(title.clone(),None)],
                    Value::Array(titles)=>titles.iter().map(|v|Ok((v.as_str().context("Source references must be strings")?.into(),None))).collect::<anyhow::Result<_>>()?,
                    Value::Object(titles)=>titles.iter().map(|(title,value)|(title.clone(),Some(value))).collect(),
                    Value::Null=>vec![],
                    _=>anyhow::bail!("Directory source must contain a title, title list or inline definitions"),
                };
                for (title,inline) in candidates {
                    if title_allowed(&title,request) {return Ok(Source {definition:referenced(raw,provider,&title,inline)?,provider:provider.clone(),title});}
                }
            }
        }
    }
    if request.position.as_deref()!=Some("source") {
        let mut fallback=None;
        if let Some(build)=raw.get("build").and_then(Value::as_object) {
            for (provider,entries) in build {
                if !matches!(provider.as_str(),"local"|"github")||!provider_allowed(provider,request) {continue;}
                for (title,value) in entries.as_object().context("Build remote provider must contain named definitions")? {
                    let definition=definition(value)?;
                    if !title_allowed(title,request)||!content_has(definition.content.as_ref(),label,raw)? {continue;}
                    let preferred=match definition.source.as_ref() {
                        Some(Value::Bool(false))=>continue,
                        Some(Value::Bool(true))=>true,
                        Some(Value::Array(labels))=>{
                            anyhow::ensure!(labels.iter().all(Value::is_string),"Remote source preference must contain label strings");
                            labels.iter().any(|v|v.as_str()==Some(label))
                        },
                        None|Some(Value::Null)=>false,
                        _=>anyhow::bail!("Destination source must be boolean or a label list"),
                    };
                    let source=Source {provider:provider.clone(),title:title.clone(),definition};
                    if preferred {return Ok(source);}
                    if fallback.is_none() {fallback=Some(source);}
                }
            }
        }
        if let Some(source)=fallback {return Ok(source);}
    }
    anyhow::bail!("No configured restore source matches the requested label and filters")
}
fn projected_config(raw:&Value,root:&Utf8Path)->anyhow::Result<crate::yml::ValidatedConfig> {
    let mut projection=raw.clone();
    if let Some(build)=projection.get_mut("build").and_then(Value::as_object_mut) {
        build.remove("local");build.remove("github");
    }
    if let Some(directories)=projection.get_mut("directories").and_then(Value::as_object_mut) {
        for directory in directories.values_mut() {
            if let Some(directory)=directory.as_object_mut() {directory.remove("source");}
        }
    }
    let config:crate::yml::ProjConfig=serde_json::from_value(projection).context("Invalid restore project configuration")?;
    config.validate_and_resolve(root,false)
}
fn repository(root:&Utf8Path,explicit:Option<&str>,api:&str)->anyhow::Result<String> {
    if let Some(repository)=explicit {return Ok(repository.into());}
    let environment=crate::run_environment::RunEnvironment::inherited();
    let mut command=crate::git::git_command_in(Some(root),&environment)?;
    let output=command.current_dir(root).args(["remote","get-url","origin"]).output()?;
    anyhow::ensure!(output.status.success(),"Cannot resolve Git origin; supply --repository owner/repo");
    let url=std::str::from_utf8(&output.stdout).context("Git origin is not UTF-8")?.trim();
    let (host,path)=if let Some(url)=url.strip_prefix("https://") {
        url.split_once('/').context("Git origin is not a repository URL")?
    } else if let Some(url)=url.strip_prefix("git@") {
        url.split_once(':').context("Git origin is not an SSH repository URL")?
    } else {anyhow::bail!("Unsupported Git origin; supply --repository owner/repo");};
    let api:ureq::http::Uri=api.parse().context("Invalid API base")?;
    let expected=if api.host().is_some_and(|host|host.eq_ignore_ascii_case("api.github.com")) {"github.com"}
        else {api.host().context("API base has no host")?};
    anyhow::ensure!(host.eq_ignore_ascii_case(expected),"Git origin does not match the API host; supply --repository explicitly");
    Ok(path.strip_suffix(".git").unwrap_or(path).into())
}
fn github_version(client:&crate::github_release::GithubReleaseClient,root:&Utf8Path,source:&Source,label:&str,request:&RestoreRequest,tag:&str)->anyhow::Result<(String,String,bool)> {
    let definition=&source.definition;let prefix=definition.path.as_deref();let append=definition.path_append_label.unwrap_or(true);
    let assets=client.assets(tag)?;
    if matches!(definition.structure.unwrap_or(Layout::Archive),Layout::Archive) {
        let mut stems=Vec::new();
        for remote_label in if label=="output-r-data" {vec![label,"data"]} else {vec![label]} {
            let stem=directory_asset_name(prefix,append,remote_label,GithubStructure::Latest,None,false)?;
            let stem=format!("{}-v",stem.strip_suffix(".zip").unwrap());
            if !stems.contains(&stem) {stems.push(stem);}
        }
        let mut namespaces=std::collections::BTreeSet::new();
        let mut found=Vec::new();
        for asset in assets {
            let Some((namespace,version))=stems.iter().find_map(|stem|asset.name.strip_prefix(stem).and_then(|v|v.strip_suffix(".zip")).map(|v|(stem,v))) else {continue;};
            let empty=version.ends_with("-empty");let version=version.strip_suffix("-empty").unwrap_or(version);
            if !version.as_bytes().first().is_some_and(u8::is_ascii_digit) {continue;}
            let numbers=version_numbers(version).context("Invalid numeric-looking GitHub archive asset version")?;
            namespaces.insert(namespace.clone());
            if request.remote_version.as_deref().is_some_and(|wanted|wanted.strip_prefix('v').unwrap_or(wanted)!=version) {continue;}
            found.push((numbers,format!("v{version}"),asset.name,empty));
        }
        anyhow::ensure!(namespaces.len()<=1,"Canonical and legacy data remote namespaces conflict");
        found.sort_by(|a,b|compare_versions(&b.0,&a.0).then(a.3.cmp(&b.3)).then(a.2.cmp(&b.2)));
        let (_,version,name,empty)=found.into_iter().next().context("Requested GitHub archive version is unavailable")?;
        Ok((version,name,empty))
    } else {
        let transfer=tempfile::Builder::new().prefix("proj-restore-meta-").tempdir_in(fs::canonicalize(std::env::temp_dir())?)?;
        let downloaded=(|| {
        let path=Utf8Path::from_path(transfer.path()).context("Metadata path must be UTF-8")?.join("VERSION.zip");
        client.download_asset(root,tag,"VERSION.zip",&path,request.max_bytes.min(4*1024*1024))?;
        let extracted=crate::github_zip::extract_zip(&path,request.max_expanded_bytes.min(16*1024*1024))?;
        let result=(|| {
            let directory=Utf8Path::from_path(extracted.path()).context("Metadata path must be UTF-8")?;
            let entries=fs::read_dir(directory)?.collect::<Result<Vec<_>,_>>()?;
            anyhow::ensure!(entries.len()==1&&entries[0].file_name()=="VERSION","VERSION.zip must contain exactly VERSION");
            let text=crate::fs_utils::read_managed_text(&directory.join("VERSION"))?;
            let matches:Vec<_>=text.lines().filter_map(|line|line.split_once(':')).filter(|(key,_)|key.trim()==label||(label=="output-r-data"&&key.trim().eq_ignore_ascii_case("data"))).collect();
            anyhow::ensure!(matches.len()==1,"VERSION.zip label is missing or ambiguous");
            let value=matches[0].1.trim();
            let version=value.strip_suffix('*').unwrap_or(value);
            anyhow::ensure!(version_numbers(version).is_some(),"Invalid advertised GitHub version");
            let version=format!("v{}",version.strip_prefix('v').unwrap_or(version));
            if let Some(wanted)=&request.remote_version {anyhow::ensure!(wanted.trim_start_matches('v')==version.trim_start_matches('v'),"Requested version is not the advertised latest");}
            let remote_label=matches[0].0.trim();
            let full=directory_asset_name(prefix,append,remote_label,GithubStructure::Latest,None,false)?;
            let empty=directory_asset_name(prefix,append,remote_label,GithubStructure::Latest,None,true)?;
            if assets.iter().any(|asset|asset.name==full) {Ok((version,full,false))}
            else if assets.iter().any(|asset|asset.name==empty) {Ok((version,empty,true))}
            else {anyhow::bail!("Advertised GitHub latest asset is unavailable");}
        })();
        let cleanup=extracted.close();
        match (result,cleanup) {
            (Ok(result),Ok(()))=>Ok(result),
            (Err(error),Ok(()))=>Err(error),
            (Ok(_),Err(error))=>Err(error),
            (Err(error),Err(cleanup))=>Err(error.context(format!("Metadata extraction cleanup also failed: {cleanup:#}"))),
        }
        })();
        let cleanup_path=transfer.path().to_owned();
        let cleanup=transfer.close().with_context(||format!("Metadata staging cleanup failed at {}",cleanup_path.display()));
        match (downloaded,cleanup) {
            (Ok(result),Ok(()))=>Ok(result),
            (Err(error),Ok(()))=>Err(error),
            (Ok(_),Err(error))=>Err(error),
            (Err(error),Err(cleanup))=>Err(error.context(format!("Metadata staging cleanup also failed: {cleanup:#}"))),
        }
    }
}
/// Restores one declared final directory using existing merged inline config.
/// Requires active project configuration and a valid root manifest.csv, matching
/// projr's history gate. Source entries precede eligible build destinations;
/// source:true/label preferences beat fallback, source:false excludes. There is
/// no retry against another source after a selected source fails. Empty/unrecorded
/// labels are skipped without clearing. Local and GitHub latest/archive downloads
/// use the existing staged installers. The CSV is not an integrity/trust check.
/// Ordinary inline remote fields are recognized only here: normal builds still
/// reject unsupported publication fields rather than silently ignoring them.
pub fn restore_label(root:&Utf8Path,request:&RestoreRequest)->anyhow::Result<RestoreResult> {
    anyhow::ensure!(request.position.as_deref().is_none_or(|v|matches!(v,"source"|"dest")),"Restore position must be source or dest");
    anyhow::ensure!(request.provider.as_deref().is_none_or(|v|matches!(v,"local"|"github")),"Restore provider must be local or github");
    let profiles=crate::run_context::select_profiles(request.profile.as_deref(),root)?;
    crate::yml::require_active_configuration_for_profiles(root,&profiles)?;
    let selected_profiles=profiles.join(",");
    let raw=crate::yml::get_combined_yml(Some(&selected_profiles),root)?;
    let config=projected_config(&raw,root)?;
    let label=config.directories.keys().find(|label|label.eq_ignore_ascii_case(&request.label)).context("Restore label is not declared")?.clone();
    let destination=config.get_path(root,&label)?;
    let manifest_path=root.join("manifest.csv");
    anyhow::ensure!(fs::symlink_metadata(&manifest_path)?.is_file(),"Restore requires a regular root manifest.csv");
    let rows=crate::manifest::read(&manifest_path)?;
    let mut result=RestoreResult {label:label.clone(),destination:destination.clone(),provider:None,title:None,version:None,restored:false,skipped:None};
    if !rows.iter().any(|row|(row.label==label||(label=="output-r-data"&&row.label.eq_ignore_ascii_case("data")))&&!row.r#fn.is_empty()) {
        result.skipped=Some("no-recorded-files".into());return Ok(result);
    }
    let source=select_source(&raw,&label,request)?;
    result.provider=Some(source.provider.clone());result.title=Some(source.title.clone());
    if source.provider=="local" {
        let path=source.definition.path.as_deref().context("Local restore source requires an explicit path")?;
        anyhow::ensure!(!path.is_empty(),"Local restore path must not be empty");
        let base=if Utf8Path::new(path).is_absolute(){Utf8PathBuf::from(path)}else{root.join(path)};
        let layout=match source.definition.structure.unwrap_or(Layout::Archive) {Layout::Latest=>LocalStructure::Latest,Layout::Archive=>LocalStructure::Archive};
        let append=source.definition.path_append_label.unwrap_or(true);
        let mut catalogs=Vec::new();
        for remote_label in if label=="output-r-data" {vec![label.as_str(),"data"]} else {vec![label.as_str()]} {
            let versions=crate::local_remote::list_versions(&base,remote_label,layout,append)?;
            if !versions.is_empty() {
                if !catalogs.iter().any(|(_,previous):&(&str,Vec<crate::local_remote::LocalVersion>)|
                    previous.iter().map(|v|(&v.path,&v.version,v.empty,v.present,v.metadata_untrusted))
                        .eq(versions.iter().map(|v|(&v.path,&v.version,v.empty,v.present,v.metadata_untrusted)))) {
                    catalogs.push((remote_label,versions));
                }
            }
        }
        anyhow::ensure!(catalogs.len()<=1,"Canonical and legacy data remote namespaces conflict");
        let (remote_label,versions)=catalogs.into_iter().next().context("Local source has no available version")?;
        let selected=if matches!(layout,LocalStructure::Archive)&&request.remote_version.is_some() {
            crate::local_remote::select_version(&base,remote_label,layout,request.remote_version.as_deref(),append)?
        } else {
            let selected=versions.into_iter().next().unwrap();
            if let Some(wanted)=&request.remote_version {
                anyhow::ensure!(version_numbers(wanted)==version_numbers(&selected.version),"Requested version is not the advertised latest");
            }
            selected
        };
        anyhow::ensure!(selected.present,"Advertised local restore source is unavailable");
        result.version=Some(selected.version.clone());
        if selected.empty {
            anyhow::ensure!(fs::read_dir(&selected.path)?.next().is_none(),"Empty local marker contains data");
            result.skipped=Some("empty-remote".into());return Ok(result);
        }
        crate::local_remote::download_version(root,&selected,&destination,request.clear)?;
    } else {
        let api=std::env::var("GITHUB_API_URL").unwrap_or_else(|_|"https://api.github.com".into());
        let repository=repository(root,request.repository.as_deref(),&api)?;
        let token=["GITHUB_PAT","GH_TOKEN","GITHUB_TOKEN"].iter().find_map(|key|std::env::var(key).ok().filter(|v|!v.trim().is_empty()));
        let client=crate::github_release::GithubReleaseClient::new(&repository,&api,token)?;
        let tag=if source.title=="@version" {crate::run_version_from(root)?.to_string(true)}
            else {source.title.trim_matches(' ').replace(' ',"-")};
        let (version,name,empty)=github_version(&client,root,&source,&label,request,&tag)?;
        result.version=Some(version);
        if empty {result.skipped=Some("empty-remote".into());return Ok(result);}
        crate::github_zip::download_directory(&client,root,&tag,&name,&destination,request.max_bytes,request.max_expanded_bytes,request.clear,false)?;
    }
    result.restored=true;Ok(result)
}
