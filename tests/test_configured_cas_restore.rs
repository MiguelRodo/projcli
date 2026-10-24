//! Explicit CAS recovery uses owned fixtures, never live datasets or credentials.
use std::{fs,path::{Path,PathBuf},process::{Command,Output},collections::BTreeMap};
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
fn failed(output:Output,code:&str) {
    assert!(!output.status.success());
    let value:Value=serde_json::from_slice(&output.stdout).expect("failure must be a structured response, not a panic");
    assert_eq!(value["protocol_version"],1);assert_eq!(value["ok"],false);
    assert_eq!(value["error"]["code"],code);
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

fn setup(placement:&str)->(tempfile::TempDir,PathBuf,Value) {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap().join("project");
    for name in ["input/empty","results/empty","cache","home"] {fs::create_dir_all(root.join(name)).unwrap();}
    for name in ["input","results"] {
        fs::write(root.join(name).join("value.txt"),b"shared content\n").unwrap();
        fs::write(root.join(name).join("zero"),b"").unwrap();
    }
    fs::write(root.join("cache/private"),b"never uploaded").unwrap();
    fs::write(root.join("VERSION"),b"v1.2.3\n").unwrap();
    let config=json!({"directories":{"raw":{"path":"input"},"output":{"path":"results"},
        "cache":{"path":"cache","safe":true,"hash":true}},
        "remotes":{"local":{"snapshots":{"path":"../store","structure":"cas","manifest":placement}}},
        "build":{"scripts":[],"git":false,"dest":{"snapshots":{}}}});
    save(&root,&config);(temp,root,config)
}
fn save(root:&Path,config:&Value) {fs::write(root.join("_proj.yml"),serde_json::to_vec_pretty(config).unwrap()).unwrap();}
fn metadata(root:&Path)->PathBuf {
    let mut hash=blake3::Hasher::new_derive_key("projcli.remote-name.v1");hash.update(b"SNAPSHOTS");
    let id=hash.finalize().to_hex().to_string();root.join(".proj/remotes").join(&id[..2]).join(&id[2..])
}
fn ids(base:&Path,namespace:&str)->Vec<String> {
    files(&base.join(namespace)).keys().map(|path|{
        let parts:Vec<_>=path.split('/').collect();assert_eq!(parts.len(),2);assert_eq!(parts[0].len(),2);
        let rest=parts[1].strip_suffix(".json").unwrap();assert_eq!(rest.len(),62);
        format!("{}{}",parts[0],rest)
    }).collect()
}

#[test]
fn named_snapshot_verify_and_restore_work_without_project_csv_or_current_upload_selection() {
    for placement in ["auto","remote","project","both"] {
        let (_temp,root,mut config)=setup(placement);let store=root.parent().unwrap().join("store");
        success(command(&root).arg("build").output().unwrap());
        let id=ids(&metadata(&root),"references/v1").pop().unwrap();
        fs::remove_file(root.join("manifest.csv")).unwrap();
        config["build"]["dest"]=json!({});
        config["remotes"]["local"]["snapshots"]["content"]=Value::Null;
        config["remotes"]["local"]["snapshots"]["manifest-style"]=json!("flat");
        save(&root,&config);
        let before=files(&root);let before_store=files(&store);
        let observation=success(command(&root).args(["remote","verify","--title","snapshots","--snapshot",&id]).output().unwrap());
        assert_eq!(observation["result"]["snapshot"]["snapshot_id"],id);
        assert_eq!(observation["result"]["snapshot"]["directories"].as_array().unwrap().len(),2);
        assert_eq!(files(&root),before);assert_eq!(files(&store),before_store);
        fs::write(root.join("input/value.txt"),b"damaged").unwrap();
        fs::write(root.join("input/extra"),b"merge retains until explicit clear").unwrap();
        let version=fs::read(root.join("VERSION")).unwrap();
        let merged=success(command(&root).args(["remote","restore","--title","snapshots","--snapshot",&id,"--label","raw"]).output().unwrap());
        assert_eq!(merged["result"]["snapshot_id"],id);assert_eq!(merged["result"]["restored"],true);
        assert_eq!(fs::read(root.join("input/extra")).unwrap(),b"merge retains until explicit clear");
        success(command(&root).args(["remote","restore","--title","snapshots","--snapshot",&id,"--label","raw",
            "--remote-version","1.2.4","--clear"]).output().unwrap());
        assert_eq!(files(&root.join("input")),BTreeMap::from([("value.txt".into(),b"shared content\n".to_vec()),("zero".into(),vec![])]));
        assert!(root.join("input/empty").is_dir());
        assert_eq!(fs::read(root.join("VERSION")).unwrap(),version);
        assert!(!root.join("manifest.csv").exists());assert!(!root.join(".projr/manifests.csv").exists());
        assert_eq!(files(&store),before_store);
        assert_eq!(files(&root.join(".proj")),before.iter().filter_map(|(name,bytes)|
            name.strip_prefix(".proj/").map(|name|(name.to_owned(),bytes.clone()))).collect());
    }
}
#[test]
fn a_fresh_project_can_restore_remote_metadata_without_the_old_project_index_or_root_reference() {
    let (_temp,root,mut config)=setup("remote");let store=root.parent().unwrap().join("store");
    success(command(&root).arg("build").output().unwrap());
    let id=ids(&store,"tree-snapshots/v1").pop().unwrap();let previous_store=files(&store);
    // Remove only this owned temporary project; the remote full metadata remains.
    fs::remove_dir_all(&root).unwrap();
    for name in ["input","results","cache","home"] {fs::create_dir_all(root.join(name)).unwrap();}
    config["build"]["dest"]=json!({});save(&root,&config);
    fs::write(root.join("VERSION"),b"Version: v9.0.0.7\n").unwrap();
    success(command(&root).args(["remote","restore","--title","snapshots","--snapshot",&id,"--label","raw","--clear"]).output().unwrap());
    assert_eq!(fs::read(root.join("input/value.txt")).unwrap(),b"shared content\n");
    assert_eq!(fs::read(root.join("VERSION")).unwrap(),b"Version: v9.0.0.7\n");
    assert!(!root.join(".proj").exists());assert!(!root.join("manifest.csv").exists());
    assert!(!root.join(".projr").exists());assert_eq!(files(&store),previous_store);
}
#[test]
fn missing_corrupt_and_mismatched_selection_never_clears_the_destination_or_writes_evidence() {
    let (_temp,root,_config)=setup("both");let store=root.parent().unwrap().join("store");
    success(command(&root).arg("build").output().unwrap());
    let id=ids(&store,"tree-snapshots/v1").pop().unwrap();
    fs::write(root.join("input/extra"),b"preserve local extra").unwrap();
    let baseline=files(&root);let stored=files(&store);let missing="0".repeat(64);
    for extra in [vec!["--snapshot",missing.as_str()],vec!["--snapshot","../unsafe"],
        vec!["--snapshot",id.as_str(),"--remote-version","v2.0.0"],
        vec!["--snapshot",id.as_str(),"--position","source"],
        vec!["--snapshot",id.as_str(),"--provider","github"],
        vec!["--snapshot",id.as_str(),"--repository","fixture/unused"],
        vec!["--snapshot",id.as_str(),"--max-bytes","1"],
        vec!["--snapshot",id.as_str(),"--max-expanded-bytes","1"],
        vec!["--snapshot",id.as_str(),"--max-bytes","0"]] {
        let output=command(&root).args(["remote","restore","--title","snapshots","--label","raw","--clear"]).args(extra).output().unwrap();
        failed(output,"command_failed");
        assert_eq!(files(&root),baseline);assert_eq!(files(&store),stored);
    }
    let hash=blake3::hash(b"shared content\n").to_hex().to_string();
    let blob=store.join("objects").join(&hash[..2]).join(&hash[2..]);fs::write(&blob,b"corrupt").unwrap();
    let corrupt_store=files(&store);
    failed(command(&root).args(["remote","verify","--title","snapshots","--snapshot",&id]).output().unwrap(),"query_failed");
    failed(command(&root).args(["remote","restore","--title","snapshots","--snapshot",&id,"--label","raw","--clear"]).output().unwrap(),"command_failed");
    assert_eq!(files(&root),baseline);assert_eq!(files(&store),corrupt_store);
}
#[test]
fn a_corrupt_present_project_copy_is_not_silently_ignored_when_remote_copy_is_healthy() {
    let (_temp,root,_config)=setup("both");let store=root.parent().unwrap().join("store");
    success(command(&root).arg("build").output().unwrap());
    let id=ids(&store,"tree-snapshots/v1").pop().unwrap();let project=metadata(&root);
    let tree=ids(&project,"trees/v1").pop().unwrap();
    let path=project.join("trees/v1").join(&tree[..2]).join(format!("{}.json",&tree[2..]));
    fs::write(&path,b"broken project tree").unwrap();let before=files(&root);let before_store=files(&store);
    for operation in ["verify","restore"] {
        let mut cli=command(&root);cli.args(["remote",operation,"--title","snapshots","--snapshot",&id]);
        if operation=="restore" {cli.args(["--label","raw","--clear"]);}
        failed(cli.output().unwrap(),if operation=="verify" {"query_failed"}else{"command_failed"});
        assert_eq!(files(&root),before);assert_eq!(files(&store),before_store);
    }
}
#[test]
fn current_legacy_write_definition_does_not_hide_an_existing_versioned_tree_snapshot() {
    let (_temp,root,mut config)=setup("remote");let store=root.parent().unwrap().join("store");
    success(command(&root).arg("build").output().unwrap());let id=ids(&store,"tree-snapshots/v1").pop().unwrap();
    config["remotes"]["local"]["snapshots"]=json!({"path":"../store","structure":"cas","content":["output"],"inspect":"manifest"});
    config["build"]["dest"]=json!({});save(&root,&config);
    fs::remove_file(root.join("manifest.csv")).unwrap();
    success(command(&root).args(["remote","restore","--title","snapshots","--snapshot",&id,"--label","raw","--clear"]).output().unwrap());
    assert_eq!(fs::read(root.join("input/value.txt")).unwrap(),b"shared content\n");
    assert!(!root.join(".projr/manifests.csv").exists());
}
#[test]
fn empty_root_without_objects_is_a_valid_explicit_restore_and_missing_full_metadata_is_not() {
    let (_temp,root,mut config)=setup("remote");
    fs::remove_dir_all(root.join("input")).unwrap();fs::create_dir(root.join("input")).unwrap();
    config["build"]["dest"]["snapshots"]["content"]=json!(["raw"]);save(&root,&config);
    success(command(&root).arg("build").output().unwrap());let store=root.parent().unwrap().join("store");
    let id=ids(&store,"tree-snapshots/v1").pop().unwrap();assert!(!store.join("objects").exists());
    fs::write(root.join("input/old"),b"only removed after verified empty snapshot").unwrap();
    success(command(&root).args(["remote","restore","--title","snapshots","--snapshot",&id,"--label","raw","--clear"]).output().unwrap());
    assert!(fs::read_dir(root.join("input")).unwrap().next().is_none());
    fs::remove_dir_all(store.join("tree-snapshots")).unwrap();let before=files(&root);
    failed(command(&root).args(["remote","verify","--title","snapshots","--snapshot",&id]).output().unwrap(),"query_failed");
    assert_eq!(files(&root),before);assert!(!ids(&metadata(&root),"references/v1").is_empty());
}
#[test]
fn snapshot_mode_requires_a_named_cas_store_and_rejects_noncas_without_mutation() {
    let (_temp,root,mut config)=setup("remote");
    config["remotes"]["local"]["ordinary"]=json!({"path":"../ordinary","structure":"archive"});
    config["build"]["dest"]=json!({});save(&root,&config);let before=files(&root);let id="0".repeat(64);
    for title in ["missing","ordinary"] {
        failed(command(&root).args(["remote","verify","--title",title,"--snapshot",&id]).output().unwrap(),"query_failed");
        failed(command(&root).args(["remote","restore","--title",title,"--snapshot",&id,"--label","raw","--clear"]).output().unwrap(),"command_failed");
        assert_eq!(files(&root),before);
    }
    failed(command(&root).args(["remote","restore","--snapshot",&id,"--label","raw"]).output().unwrap(),"invalid_arguments");
    assert_eq!(files(&root),before);assert!(!root.parent().unwrap().join("store").exists());
}
