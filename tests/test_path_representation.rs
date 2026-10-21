use std::{fs, path::Path, process::Command};

fn query(root: &Path, cwd: &Path, format: &str, base: Option<&str>) -> std::process::Output {
    let mut command=Command::new(env!("CARGO_BIN_EXE_proj"));
    command.args(["--machine","path","get","--label","raw-data","--format",format,"--project"]).arg(root)
        .current_dir(cwd).env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE");
    if let Some(base)=base {command.args(["--relative-to",base]);}
    command.output().unwrap()
}

fn path(output: std::process::Output) -> std::path::PathBuf {
    assert!(output.status.success(),"{} {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
    let value:serde_json::Value=serde_json::from_slice(&output.stdout).unwrap();
    value["result"]["path"].as_str().unwrap().into()
}

#[test]
fn auto_and_explicit_bases_resolve_the_same_target_without_suffix_or_marker_guessing() {
    let project=tempfile::tempdir().unwrap();let root=project.path();
    fs::create_dir_all(root.join("analysis/deeper")).unwrap();fs::create_dir(root.join("_raw")).unwrap();
    fs::write(root.join("_proj.yml"),"directories:\n  raw-data:\n    path: _raw\n").unwrap();
    // Neither an active project marker nor a .qmd file manufactures caller render context.
    fs::write(root.join("_quarto.yml"),"project: {type: default}\n").unwrap();
    fs::write(root.join("analysis/report.qmd"),"Report\n").unwrap();
    let cwd=root.join("analysis/deeper");
    let auto=path(query(root,&cwd,"auto",None));assert!(!auto.is_absolute());
    let root_auto=path(query(root,root,"auto",None));assert!(!root_auto.is_absolute());
    assert_ne!(auto,root_auto);assert_eq!(root.join(root_auto).canonicalize().unwrap(),root.join("_raw").canonicalize().unwrap());
    assert_eq!(cwd.join(auto).canonicalize().unwrap(),root.join("_raw").canonicalize().unwrap());
    let absolute=path(query(root,&cwd,"absolute",None));assert!(absolute.is_absolute());
    assert_eq!(absolute.canonicalize().unwrap(),root.join("_raw").canonicalize().unwrap());
    for (base,anchor) in [("cwd",cwd.as_path()),("project",root)] {
        let relative=path(query(root,&cwd,"relative",Some(base)));
        assert!(!relative.is_absolute());assert_eq!(anchor.join(relative).canonicalize().unwrap(),absolute.canonicalize().unwrap());
    }
    let relative=path(query(root,&cwd,"relative",Some(root.to_str().unwrap())));
    assert_eq!(root.join(relative).canonicalize().unwrap(),absolute.canonicalize().unwrap());
    assert!(!query(root,&cwd,"relative",Some("document")).status.success());
    assert!(!query(root,&cwd,"auto",Some("project")).status.success());
    assert!(!query(root,&cwd,"absolute",Some("cwd")).status.success());
    assert!(!query(root,&cwd,"relative",Some("not-an-absolute-base")).status.success());
}

#[test]
fn a_literal_machine_after_terminator_does_not_switch_human_parse_errors_to_json() {
    let output=Command::new(env!("CARGO_BIN_EXE_proj")).args(["invalid-command","--","--machine"]).output().unwrap();
    assert!(!output.status.success());assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("protocol_version"));
}

#[cfg(unix)]
#[test]
fn only_actual_renderer_children_receive_render_context_and_overrides_still_work() {
    use std::os::unix::fs::PermissionsExt;
    for extension in ["Rmd","qmd"] {
        let project=tempfile::tempdir().unwrap();let root=project.path();
        fs::create_dir_all(root.join("analysis/deeper")).unwrap();fs::create_dir(root.join("cache")).unwrap();
        fs::create_dir(root.join("_raw")).unwrap();fs::create_dir(root.join("bin")).unwrap();
        fs::write(root.join("_proj.yml"),format!("directories:\n  cache:\n    path: cache\n  docs:\n    path: published\n  raw-data:\n    path: _raw\ndev:\n  scripts: [analysis.py, analysis/report.{extension}]\n")).unwrap();
        fs::write(root.join(format!("analysis/report.{extension}")),"Report\n").unwrap();
        let ordinary=format!(r#"import os,json,subprocess
from pathlib import Path
root=Path(os.environ['PROJ_PROJECT_ROOT'])
value=json.loads(subprocess.check_output([{binary},'--machine','path','get','--label','raw-data','--format','auto'],text=True))
assert not Path(value['result']['path']).is_absolute()
(root/'cache/ordinary').write_text(value['result']['path'])
"#,binary=serde_json::to_string(env!("CARGO_BIN_EXE_proj")).unwrap());
        fs::write(root.join("analysis.py"),ordinary).unwrap();
        let renderer=format!(r#"#!/usr/bin/env python3
import json,os,subprocess
from pathlib import Path
context=json.loads(Path(os.environ['PROJ_RUN_CONTEXT']).read_text())
if context.get('render') is None: raise SystemExit(0)
root=Path(os.environ['PROJ_PROJECT_ROOT'])
os.chdir(root/'analysis/deeper')
def query(fmt,base=None):
 args=[{binary},'--machine','path','get','--label','raw-data','--format',fmt]
 if base: args+=['--relative-to',base]
 return json.loads(subprocess.check_output(args,text=True))['result']['path']
auto=query('auto')
assert Path(auto).is_absolute() and Path(auto).samefile(root/'_raw')
relative=query('relative')
assert not Path(relative).is_absolute() and Path(relative).samefile(root/'_raw')
document=query('relative','document')
assert (root/'analysis'/document).samefile(root/'_raw')
project=query('relative','project')
assert (root/project).samefile(root/'_raw')
(root/'cache/rendered').write_text(json.dumps([auto,relative,document,os.environ['PROJ_RUN_CONTEXT']]))
"#,binary=serde_json::to_string(env!("CARGO_BIN_EXE_proj")).unwrap());
        for name in ["Rscript","quarto"] {
            let file=root.join("bin").join(name);fs::write(&file,&renderer).unwrap();fs::set_permissions(file,fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut paths=vec![root.join("bin")];paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
        let output=Command::new(env!("CARGO_BIN_EXE_proj")).args(["build-dev","--non-interactive","--project"]).arg(root).current_dir(root)
            .env("PATH",std::env::join_paths(paths).unwrap()).env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
            .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT").output().unwrap();
        assert!(output.status.success(),"{} {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        assert!(root.join("cache/ordinary").is_file());
        let seen:Vec<String>=serde_json::from_slice(&fs::read(root.join("cache/rendered")).unwrap()).unwrap();
        assert!(!Path::new(&seen[3]).exists(),"per-render context survived invocation");
    }
}


#[test]
fn explicit_render_descriptor_preserves_target_and_document_anchor_across_other_project_selection() {
    let project=tempfile::tempdir().unwrap();let root=project.path().canonicalize().unwrap();
    let other=project.path().join("other");fs::create_dir(&other).unwrap();
    fs::create_dir_all(root.join("analysis/deeper")).unwrap();fs::create_dir(root.join("_raw")).unwrap();
    fs::create_dir(other.join("raw-b")).unwrap();
    fs::write(other.join("_proj.yml"),"directories:\n  raw-data:\n    path: raw-b\n").unwrap();
    let document=root.join("analysis/report.qmd");fs::write(&document,"Report\n").unwrap();
    let context=project.path().join("context.json");
    let mut snapshot=serde_json::json!({"schema_version":4,"root":root,"profiles":[],"directories":{"raw-data":root.join("_raw")},"directory_order":["raw-data"],"safe_cache":null,"active_version":{"major":1,"minor":2,"patch":3,"dev":4},"development":true,"render":{"engine":"quarto","document":document}});
    fs::write(&context,serde_json::to_vec(&snapshot).unwrap()).unwrap();
    let request=|selected:&Path,format:&str,base:Option<&str>| {
        let mut cmd=Command::new(env!("CARGO_BIN_EXE_proj"));
        cmd.args(["--machine","--project"]).arg(selected).args(["path","get","--label","raw-data","--format",format])
            .current_dir(root.join("analysis/deeper")).env(proj::RUN_CONTEXT_ENV,&context).env_remove(proj::PROJECT_ROOT_ENV)
            .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE");
        if let Some(base)=base {cmd.args(["--relative-to",base]);}
        cmd.output().unwrap()
    };
    let auto=path(request(&root,"auto",None));assert!(auto.is_absolute());assert_eq!(auto.canonicalize().unwrap(),root.join("_raw").canonicalize().unwrap());
    let relative=path(request(&root,"relative",Some("document")));assert!(!relative.is_absolute());
    assert_eq!(document.parent().unwrap().join(relative).canonicalize().unwrap(),auto.canonicalize().unwrap());
    // Other-project selection discards A's mappings/version, not the fact this caller is rendering A.
    let other_auto=path(request(&other,"auto",None));assert!(other_auto.is_absolute());assert_eq!(other_auto.canonicalize().unwrap(),other.join("raw-b").canonicalize().unwrap());
    let other_document=path(request(&other,"relative",Some("document")));
    assert_eq!(document.parent().unwrap().join(other_document).canonicalize().unwrap(),other_auto.canonicalize().unwrap());
    snapshot["render"]["document"]=serde_json::Value::Null;
    fs::write(&context,serde_json::to_vec(&snapshot).unwrap()).unwrap();
    assert!(!request(&root,"relative",Some("document")).status.success());
    assert!(path(request(&root,"auto",None)).is_absolute());
}


#[cfg(windows)]
#[test]
fn ordinary_custom_drive_root_and_existing_target_share_a_relative_namespace() {
    use std::path::{Component,Prefix};
    let temp=tempfile::tempdir().unwrap();let root=temp.path();
    fs::create_dir(root.join("raw")).unwrap();
    fs::write(root.join("_proj.yml"),"directories:\n  raw:\n    path: raw\n").unwrap();
    let canonical=root.canonicalize().unwrap();
    let drive=match canonical.components().next().unwrap() {
        Component::Prefix(prefix)=>match prefix.kind() {Prefix::Disk(d)|Prefix::VerbatimDisk(d)=>d,_=>panic!("test requires disk temp root")},
        _=>panic!("test requires disk temp root"),
    };
    let base=format!("{}:\\",drive as char);
    let output=Command::new(env!("CARGO_BIN_EXE_proj")).args(["--machine","path","get","--label","raw","--format","relative","--relative-to",&base,"--project"])
        .arg(root).env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
    let relative=path(output);assert!(!relative.is_absolute());
    assert_eq!(std::path::Path::new(&base).join(relative).canonicalize().unwrap(),root.join("raw").canonicalize().unwrap());
}
