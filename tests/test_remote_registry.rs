//! Actual build/restore fixture; all data and configuration are owned temporary files.
use std::{fs,path::Path,process::{Command,Output},collections::BTreeMap};
use serde_json::{json,Value};
fn command(root:&Path)->Command {
    let mut command=Command::new(env!("CARGO_BIN_EXE_proj"));command.current_dir(root);
    for (key,_) in std::env::vars_os() {
        let name=key.to_string_lossy().to_ascii_uppercase();
        if ["GIT_","GH_","GITHUB_","PROJR_","PROJCLI_"].iter().any(|prefix|name.starts_with(prefix))
            ||matches!(name.as_str(),"QUARTO_PROFILE"|"SSH_AUTH_SOCK") {command.env_remove(key);}
    }
    command.env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env("HOME",root.join("home")).env("USERPROFILE",root.join("home")).env("XDG_CONFIG_HOME",root.join("home"))
        .env("GIT_CONFIG_NOSYSTEM","1").env("GIT_CONFIG_GLOBAL",root.join("home/gitconfig")).env("GIT_TERMINAL_PROMPT","0")
        .args(["--machine","--non-interactive","--project"]).arg(root);
    command
}
fn success(output:Output)->Value {
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}
fn files(root:&Path)->BTreeMap<String,Vec<u8>> {
    fn collect(root:&Path,path:&Path,rows:&mut BTreeMap<String,Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry=entry.unwrap();let path=entry.path();let kind=entry.file_type().unwrap();
            if kind.is_dir(){collect(root,&path,rows);}
            else {assert!(kind.is_file());rows.insert(path.strip_prefix(root).unwrap().to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR,"/"),fs::read(path).unwrap());}
        }
    }
    let mut rows=BTreeMap::new();if root.exists(){collect(root,root,&mut rows);}rows
}
fn snapshot(root:&Path)->BTreeMap<String,Option<Vec<u8>>> {
    fn collect(root:&Path,path:&Path,rows:&mut BTreeMap<String,Option<Vec<u8>>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry=entry.unwrap();let path=entry.path();let kind=entry.file_type().unwrap();
            let name=path.strip_prefix(root).unwrap().to_string_lossy().replace(std::path::MAIN_SEPARATOR,"/");
            if kind.is_dir(){rows.insert(name,None);collect(root,&path,rows);}
            else {assert!(kind.is_file());rows.insert(name,Some(fs::read(path).unwrap()));}
        }
    }
    let mut rows=BTreeMap::new();collect(root,root,&mut rows);rows
}
fn recovered_snapshot(root:&Path,before:&BTreeMap<String,Option<Vec<u8>>>)->BTreeMap<String,Option<Vec<u8>>> {
    let mut after=snapshot(root);let directory=".proj/log/local-failures";let prefix=format!("{directory}/");
    let added:Vec<_>=after.keys().filter(|path|path.starts_with(prefix.as_str())&&!before.contains_key(*path)).cloned().collect();
    let mut diagnostics=0;let mut ignores=0;
    for path in added {
        let relative=path.strip_prefix(prefix.as_str()).unwrap();
        assert!(!relative.contains('/'),"unexpected diagnostic subtree");
        let bytes=after.remove(&path).unwrap().expect("unexpected diagnostic directory");
        if relative==".gitignore" {assert_eq!(bytes,b"*\n");ignores+=1;continue;}
        assert!(bytes.len()<=1_048_576);
        let value:Value=serde_json::from_slice(&bytes).unwrap();let id=value["attempt_id"].as_str().unwrap();
        assert!(id.starts_with("attempt-")&&id.len()==32&&id.bytes().all(|byte|byte.is_ascii_alphanumeric()||byte==b'-'));
        assert_eq!(relative,format!("{id}.json"));assert_eq!(value["schema_version"],1);
        assert_eq!(value["status"],"failed");assert_eq!(value["stage"],"preflight");
        assert_eq!(value["distribution_may_have_started"],false);
        assert_eq!(value["version_before"],"v1.2.3");assert_eq!(value["version_after"],"v1.2.3");
        let keys:std::collections::BTreeSet<_>=value.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys,std::collections::BTreeSet::from(["schema_version","attempt_id","started_at","recorded_at",
            "stage","status","distribution_may_have_started","version_before","version_after"]));
        diagnostics+=1;
    }
    assert_eq!(diagnostics,1);assert_eq!(ignores,1);
    for parent in [".proj",".proj/log",directory] {
        if !before.contains_key(parent){assert_eq!(after.remove(parent),Some(None));}
    }
    assert!(!root.join(".proj/build.lock").exists());after
}
fn setup()->(tempfile::TempDir,std::path::PathBuf,Value) {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap().join("project");
    for name in ["input","results","cache","home"] {fs::create_dir_all(root.join(name)).unwrap();}
    fs::write(root.join("input/input.csv"),b"current B\n").unwrap();
    fs::write(root.join("results/report.csv"),b"local output\n").unwrap();
    fs::write(root.join("VERSION"),b"v1.2.3\n").unwrap();
    let config=json!({
        "directories":{"raw":{"path":"input","source":{"local":"A"}},"output":{"path":"results"},"cache":{"path":"cache","safe":true}},
        "remotes":{"local":{
            "A":{"path":"../store-A","structure":"archive","content":["raw"]},
            "B":{"path":"../store-B","structure":"archive","send":{"inspect":"file","cue":"always"}}}},
        "build":{"scripts":[],"git":false,"dest":{"B":{"content":["raw"]}}}
    });
    fs::write(root.join("_proj.yml"),serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    (temp,root,config)
}
#[test]
fn actual_cli_uploads_only_to_b_and_restores_a_after_its_upload_selection_is_absent() {
    let (_temp,root,mut config)=setup();let parent=root.parent().unwrap();
    fs::create_dir_all(parent.join("store-A/raw/v1.0.0")).unwrap();
    fs::write(parent.join("store-A/raw/v1.0.0/input.csv"),b"historical A\n").unwrap();
    let historical=format!("label,fn,version,hash\nraw,input.csv,v1.0.0,{:x}\n",md5::compute(b"historical A\n"));
    fs::write(root.join("manifest.csv"),&historical).unwrap();
    fs::write(parent.join("store-A/manifest.csv"),&historical).unwrap();
    fs::create_dir(parent.join("store-A/preserved-empty")).unwrap();
    let original_a=snapshot(&parent.join("store-A"));
    success(command(&root).arg("build").output().unwrap());
    assert_eq!(snapshot(&parent.join("store-A")),original_a);
    assert_eq!(fs::read(parent.join("store-B/raw/v1.2.4/input.csv")).unwrap(),b"current B\n");
    assert!(!parent.join("store-B/output").exists());assert!(!parent.join("store-B/cache").exists());
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap().trim(),"Version: v1.2.4.1");
    let original_b=snapshot(&parent.join("store-B"));
    let manifest=fs::read(root.join("manifest.csv")).unwrap();let version=fs::read(root.join("VERSION")).unwrap();
    for explicit_title in [false,true] {
        if explicit_title {
            config["directories"]["raw"].as_object_mut().unwrap().remove("source");
            config["build"]["dest"]=json!({});
            fs::write(root.join("_proj.yml"),serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        }
        fs::write(root.join("input/input.csv"),b"damaged local").unwrap();
        fs::write(root.join("input/local-only"),b"clear only on successful restore").unwrap();
        let mut restore=command(&root);
        restore.args(["remote","restore","--label","raw","--position","source","--remote-version","v1.0.0",
            "--clear","--max-bytes","100000","--max-expanded-bytes","100000"]);
        if explicit_title {restore.args(["--title","A"]);}
        let result=success(restore.output().unwrap());
        assert_eq!(result["result"]["title"],"A");assert_eq!(result["result"]["restored"],true);
        assert_eq!(files(&root.join("input")),BTreeMap::from([("input.csv".into(),b"historical A\n".to_vec())]));
        assert_eq!(fs::read(root.join("manifest.csv")).unwrap(),manifest);assert_eq!(fs::read(root.join("VERSION")).unwrap(),version);
        assert_eq!(snapshot(&parent.join("store-A")),original_a);assert_eq!(snapshot(&parent.join("store-B")),original_b);
    }
}
#[test]
fn registered_but_unselected_remotes_need_no_credentials_and_make_no_store() {
    for (selection,remote_null) in [(None,false),(Some(json!({})),false),(Some(json!({"B":{"content":[]}})),false),
        (Some(json!({"B":{}})),true),(Some(json!({"unused":{}})),true)] {
        let (_temp,root,mut config)=setup();
        config["directories"]["raw"].as_object_mut().unwrap().remove("source");
        config["remotes"]["github"]=json!({"unused":{"content":["output"]}});
        if remote_null {
            config["remotes"]["local"]["B"]["content"]=Value::Null;
            config["remotes"]["github"]["unused"]["content"]=Value::Null;
        }
        match selection {Some(selection)=>{config["build"]["dest"]=selection;},None=>{config["build"].as_object_mut().unwrap().remove("dest");}}
        fs::write(root.join("_proj.yml"),serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        // Any accidental GitHub capture would reject this nonpublic API without a token.
        success(command(&root).env("GITHUB_API_URL","http://127.0.0.1:1").arg("build").output().unwrap());
        assert!(!root.parent().unwrap().join("store-A").exists());assert!(!root.parent().unwrap().join("store-B").exists());
    }
}
#[test]
fn unknown_registry_reference_preserves_inputs_before_hooks_with_only_the_approved_local_diagnostic() {
    let (_temp,root,mut config)=setup();
    config["build"]["dest"]=json!({"unknown":{}});
    config["build"]["hooks"]=json!({"pre":["pre.py"]});
    fs::write(root.join("pre.py"),b"from pathlib import Path\nPath('hook-ran').write_text('unexpected')\n").unwrap();
    fs::write(root.join("_proj.yml"),serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    let before=snapshot(&root);
    let output=command(&root).arg("build").output().unwrap();assert!(!output.status.success());
    let error:Value=serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["code"],"command_failed");
    assert!(String::from_utf8_lossy(&output.stderr).contains("not registered"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("unknown"));
    assert!(!root.join("hook-ran").exists());
    assert_eq!(recovered_snapshot(&root,&before),before);assert!(!root.parent().unwrap().join("store-B").exists());
}


#[test]
fn conflicting_registry_and_inline_source_definitions_fail_readonly_before_clearing() {
    let (_temp,root,mut config)=setup();
    config["build"]["local"]=json!({"A":{"path":"../spoof","content":["raw"]}});
    fs::write(root.join("_proj.yml"),serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    let before=snapshot(&root);
    let output=command(&root).args(["remote","restore","--label","raw","--title","A","--clear",
        "--max-bytes","100000","--max-expanded-bytes","100000"]).output().unwrap();
    assert!(!output.status.success());assert!(String::from_utf8_lossy(&output.stderr).contains("both"));
    assert_eq!(snapshot(&root),before);
}
