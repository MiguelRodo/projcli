//! Bounded startup observations, not a frozen execution tree or an environment lock.
use std::{collections::HashSet,fs,io::Read,process::Output};
use camino::{Utf8Path,Utf8PathBuf};
use serde::{Deserialize,Serialize};

const MAX_SOURCES:usize=256;
const MAX_FILE_BYTES:u64=8*1024*1024;
const MAX_TOTAL_BYTES:u64=64*1024*1024;
const MAX_PROBE_BYTES:usize=16*1024;
const DEPENDENCIES:&[&str]=&["renv.lock","DESCRIPTION","pyproject.toml","requirements.txt",
    "requirements-dev.txt","poetry.lock","uv.lock","Pipfile.lock","Project.toml","Manifest.toml",
    "Cargo.lock","environment.yml","environment.yaml"];

/// One observed file; missing, linked, unreadable and limited evidence is explicit.
#[derive(Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileEvidence {
    name:String,
    outside_project:bool,
    name_redacted:bool,
    status:String,
    bytes:Option<u64>,
    blake3:Option<String>,
}

/// Sanitized version evidence from an existing preflight probe; never raw output or argv.
#[derive(Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunnerEvidence {
    name:String,
    status:String,
    reported_version:Option<String>,
}

impl RunnerEvidence {
    /// Retain only a bounded numeric version, never vendor text, paths or environment.
    pub(crate) fn from_probe(name:&str,output:&Output)->Self {
        let bounded=output.stdout.len().saturating_add(output.stderr.len())<=MAX_PROBE_BYTES;
        let version=if bounded&&output.status.success() {
            let prefix=match name {
                "python"|"python3"=>r"^Python ",
                "Rscript"=>r"^(?:Rscript \(R\)|R scripting front-end) version ",
                "quarto"=>r"^",
                _=>return Self {name:name.into(),status:"unknown_version".into(),reported_version:None},
            };
            let pattern=regex::Regex::new(&format!(r"(?m){prefix}([0-9]+(?:\.[0-9]+){{1,2}})(?:\s|[-+]|$)")).unwrap();
            [&output.stdout,&output.stderr].into_iter().find_map(|bytes|{
                let text=String::from_utf8_lossy(bytes);
                pattern.captures(&text).map(|value|value[1].to_owned())
            })
        }else{None};
        Self {name:name.into(),status:if !bounded {"probe_limit"}else if !output.status.success() {
            "probe_failed"
        }else if version.is_some() {"reported"}else{"unknown_version"}.into(),reported_version:version}
    }
}

/// Safe effective policy projection, omitting arbitrary analysis/configuration values.
#[derive(Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigurationEvidence {
    scope:String,
    policy:serde_json::Value,
    policy_blake3:String,
    #[serde(default,skip_serializing_if="Option::is_none")]
    effective:Option<crate::config_evidence::Fingerprint>,
}

impl ConfigurationEvidence {
    /// Retain safe flags/counts plus an opaque bounded merged-analysis fingerprint, never raw config.
    pub(crate) fn capture(config:&crate::yml::ValidatedConfig,execution:crate::build::ExecutionDirectory,
        clear:crate::yml::OutputClearStage,cache_selected:bool,analysis:Option<crate::config_evidence::Fingerprint>)->Self {
        let policy=serde_json::json!({
            "execution_directory":format!("{execution:?}").to_ascii_lowercase(),
            "output_clear_stage":format!("{clear:?}").to_ascii_lowercase(),
            "path_representation":format!("{:?}",config.settings.path_representation.unwrap_or(crate::paths::PathFormat::Auto)).to_ascii_lowercase(),
            "output_run":config.output_run.unwrap_or(true),
            "r_session":format!("{:?}",config.config.r.session).to_ascii_lowercase(),
            "rscript_configured":config.config.r.rscript.is_some(),
            "git_commit":config.git.commit,"git_push":config.git.push,
            "declared_directories":config.directories.len(),"cache_selected":cache_selected,
            "hashed_directories":config.directories.iter().filter(|(label,directory)|
                directory.hash.unwrap_or(!label.to_ascii_lowercase().starts_with("cache"))).count()
        });
        let policy_blake3=blake3::hash(&serde_json::to_vec(&policy).unwrap()).to_hex().to_string();
        Self {scope:"validated_policy_and_merged_analysis".into(),effective:analysis.map(|value|value.bind_policy(&policy_blake3)),policy,policy_blake3}
    }
}

/// Startup evidence is captured before start hooks. It does not prove later executed bytes.
#[derive(Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Snapshot {
    captured_at:String,
    stage:String,
    source_scope:String,
    source_state:String,
    source_head:Option<String>,
    selected_sources:Vec<FileEvidence>,
    selected_sources_omitted:usize,
    implicit_renderer_project:bool,
    dependency_scope:String,
    dependencies:Vec<FileEvidence>,
    runner_scope:String,
    runners:Vec<RunnerEvidence>,
    configuration:ConfigurationEvidence,
}

fn observe(path:&Utf8Path,name:String,outside_project:bool,name_redacted:bool,budget:&mut u64)->FileEvidence {
    let mut evidence=FileEvidence {name,outside_project,name_redacted,status:"unavailable".into(),bytes:None,blake3:None};
    let result=(||->anyhow::Result<()> {
        crate::path_safety::resolve_path(path.as_std_path(),true)?;
        let metadata=match fs::symlink_metadata(path) {
            Ok(metadata)=>metadata,
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{
                evidence.status="missing".into();return Ok(());
            },
            Err(error)=>return Err(error.into()),
        };
        if !metadata.is_file(){return Ok(());}
        if *budget==0||metadata.len()>MAX_FILE_BYTES||metadata.len()>*budget {
            evidence.status="limit".into();return Ok(());
        }
        let limit=MAX_FILE_BYTES.min(*budget);
        let mut file=fs::File::open(path)?.take(limit+1);
        let mut hasher=blake3::Hasher::new();let mut bytes=0;let mut buffer=[0u8;65536];
        loop {
            let count=file.read(&mut buffer)?;
            if count==0{break;}
            bytes+=count as u64;
            *budget=budget.saturating_sub(count as u64);
            if bytes>limit {evidence.status="limit".into();return Ok(());}
            hasher.update(&buffer[..count]);
        }
        evidence.status="observed".into();evidence.bytes=Some(bytes);
        evidence.blake3=Some(hasher.finalize().to_hex().to_string());
        Ok(())
    })();
    // Unknown evidence must not turn a previously supported build into a failure.
    if result.is_err(){evidence.status="unavailable".into();}
    evidence
}

impl Snapshot {
    /// Observe selected sources and fixed root dependency declarations within explicit byte/count caps.
    /// External source paths get opaque ordinal names; no personal absolute paths are serialized.
    pub(crate) fn capture(root:&Utf8Path,sources:&[Utf8PathBuf],implicit_renderer_project:bool,
        source_head:Option<String>,git_preflight:bool,runners:Vec<RunnerEvidence>,
        configuration:ConfigurationEvidence)->Self {
        let mut budget=MAX_TOTAL_BYTES;let mut seen=HashSet::<&Utf8PathBuf>::new();let mut selected_sources=Vec::new();let mut omitted=0;
        for path in sources {
            if !seen.insert(path){continue;}
            if selected_sources.len()==MAX_SOURCES {omitted+=1;continue;}
            let identity=crate::path_safety::resolve_path(path.as_std_path(),true).ok();
            let relative=identity.as_ref().and_then(|path|path.strip_prefix(root.as_std_path()).ok())
                .and_then(|path|path.to_str()).map(|path|path.replace(std::path::MAIN_SEPARATOR,"/"));
            let outside=relative.is_none();
            let safe_name=relative.filter(|name|name.len()<=256
                && !name.contains([':', '@', '?', '#']) && !name.chars().any(char::is_control));
            let name_redacted=safe_name.is_none();
            let name=safe_name.unwrap_or_else(||format!("source-{}",selected_sources.len()+1));
            selected_sources.push(observe(path,name,outside,name_redacted,&mut budget));
        }
        let dependencies=DEPENDENCIES.iter().map(|name|observe(&root.join(name),(*name).into(),false,false,&mut budget)).collect();
        Self {captured_at:chrono::Utc::now().to_rfc3339(),stage:"before_start_hooks".into(),source_scope:"selected_scripts_and_hooks_only".into(),
            source_state:if git_preflight {"git_preflight_accepted"}else{"git_not_queried"}.into(),
            source_head,selected_sources,selected_sources_omitted:omitted,implicit_renderer_project,
            dependency_scope:"root_declarations_only".into(),dependencies,runner_scope:"preflight_reported_versions_only".into(),runners,configuration}
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    fn probe(bytes:Vec<u8>)->Output {
        #[cfg(unix)]
        let status={use std::os::unix::process::ExitStatusExt;std::process::ExitStatus::from_raw(0)};
        #[cfg(windows)]
        let status={use std::os::windows::process::ExitStatusExt;std::process::ExitStatus::from_raw(0)};
        Output {status,stdout:bytes,stderr:Vec::new()}
    }
    #[test]
    fn runner_metadata_retains_only_recognised_numeric_versions() {
        let output=probe(b"Python 3.12.7\nhttps://user:secret@private.invalid/path?token=TOKEN hostname=personal-host\n".to_vec());
        let evidence=RunnerEvidence::from_probe("python3",&output);
        assert_eq!(evidence.reported_version.as_deref(),Some("3.12.7"));
        let text=serde_json::to_string(&evidence).unwrap();
        for secret in ["user:secret","private.invalid","TOKEN","personal-host"] {assert!(!text.contains(secret));}
        assert_eq!(RunnerEvidence::from_probe("quarto",&probe(b"https://10.1.2.3/private\n".to_vec())).status,"unknown_version");
        assert_eq!(RunnerEvidence::from_probe("quarto",&probe(vec![b'a';MAX_PROBE_BYTES+1])).status,"probe_limit");
        assert_eq!(RunnerEvidence::from_probe("Rscript",&probe(b"Rscript (R) version 4.5.1 (2026-01-01)\n".to_vec())).reported_version.as_deref(),Some("4.5.1"));
    }
}
