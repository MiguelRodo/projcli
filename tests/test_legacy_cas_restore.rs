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


fn legacy()->(tempfile::TempDir,PathBuf,Value,String) {
    let (temp,root,mut config)=setup("auto");
    fs::create_dir_all(root.join("input/nested")).unwrap();
    fs::write(root.join("input/nested/caf\u{e9},notes.txt"),b"quoted Unicode path\n").unwrap();
    config["remotes"]["local"]["snapshots"]=json!({"path":"../store","structure":"cas","content":["RAW"],"inspect":"manifest"});
    config["build"]["dest"]=json!(["snapshots"]);save(&root,&config);
    success(command(&root).arg("build").output().unwrap());
    let store=root.parent().unwrap().join("store");
    let manifests=files(&store.join("manifests"));assert_eq!(manifests.len(),1);
    let id=manifests.keys().next().unwrap().strip_suffix(".json").unwrap().to_owned();
    (temp,root,config,id)
}
#[test]
fn existing_flat_ids_restore_exact_files_without_conversion_or_project_csv() {
    let (_temp,root,mut config,id)=legacy();let store=root.parent().unwrap().join("store");
    let expected=files(&root.join("input"));let before_store=files(&store);
    let ledger=fs::read(root.join(".projr/manifests.csv")).unwrap();let version=fs::read(root.join("VERSION")).unwrap();
    fs::remove_file(root.join("manifest.csv")).unwrap();
    config["remotes"]["local"]["snapshots"]=json!({"path":"../store","structure":"cas","content":null,"manifest-style":"tree"});
    config["build"]["dest"]=json!({});save(&root,&config);
    let before=files(&root);
    let observation=success(command(&root).args(["remote","verify","--title","snapshots","--snapshot",&id]).output().unwrap());
    assert_eq!(observation["result"]["snapshot"]["snapshot_kind"],"legacy-flat");
    assert_eq!(observation["result"]["snapshot"]["snapshot_id"],id);
    assert_eq!(observation["result"]["snapshot"]["inventory"]["files"].as_array().unwrap().len(),3);
    assert_eq!(files(&root),before);
    fs::write(root.join("input/value.txt"),b"wrong").unwrap();fs::write(root.join("input/extra"),b"retain when merging").unwrap();
    let result=success(command(&root).args(["remote","restore","--title","snapshots","--snapshot",&id,"--label","raw"]).output().unwrap());
    assert_eq!(result["result"]["snapshot_kind"],"legacy-flat");
    assert_eq!(result["result"]["snapshot_id"],id);assert_eq!(result["result"]["restored"],true);
    assert!(root.join("input/extra").exists());
    success(command(&root).args(["remote","restore","--title","snapshots","--snapshot",&id,"--label","raw","--clear"]).output().unwrap());
    assert_eq!(files(&root.join("input")),expected);
    assert!(!root.join("input/empty").exists(),"legacy metadata never recorded empty directories");
    assert_eq!(fs::read(root.join("VERSION")).unwrap(),version);
    assert_eq!(fs::read(root.join(".projr/manifests.csv")).unwrap(),ledger);
    assert!(!root.join("manifest.csv").exists());assert_eq!(files(&store),before_store);
    assert!(!store.join("trees").exists());assert!(!root.join(".proj/remotes").exists());
}
#[test]
fn legacy_remote_alone_recovers_into_a_fresh_configured_project() {
    let (_temp,root,mut config,id)=legacy();let store=root.parent().unwrap().join("store");
    let expected=files(&root.join("input"));let before_store=files(&store);
    fs::remove_dir_all(&root).unwrap(); // Only the fixture-owned temporary project.
    for name in ["input","results","cache","home"] {fs::create_dir_all(root.join(name)).unwrap();}
    config["build"]["dest"]=json!({});save(&root,&config);fs::write(root.join("VERSION"),b"v8.0.0.7\n").unwrap();
    success(command(&root).args(["remote","restore","--title","snapshots","--snapshot",&id,"--label","raw","--clear"]).output().unwrap());
    assert_eq!(files(&root.join("input")),expected);assert_eq!(fs::read(root.join("VERSION")).unwrap(),b"v8.0.0.7\n");
    assert!(!root.join(".proj").exists());assert!(!root.join(".projr").exists());assert!(!root.join("manifest.csv").exists());
    assert_eq!(files(&store),before_store);
}
#[test]
fn corrupt_legacy_metadata_or_objects_fail_without_clearing_or_rewriting_anything() {
    let (_temp,root,_config,id)=legacy();let store=root.parent().unwrap().join("store");
    let path=store.join("manifests").join(format!("{id}.json"));let original=fs::read(&path).unwrap();
    fs::write(root.join("input/local-only"),b"keep").unwrap();let before=files(&root);
    fs::write(&path,b"tampered").unwrap();let corrupt=files(&store);
    failed(command(&root).args(["remote","verify","--title","snapshots","--snapshot",&id]).output().unwrap(),"query_failed");
    failed(command(&root).args(["remote","restore","--title","snapshots","--snapshot",&id,"--label","raw","--clear"]).output().unwrap(),"command_failed");
    assert_eq!(files(&root),before);assert_eq!(files(&store),corrupt);fs::write(&path,original).unwrap();
    let hash=blake3::hash(b"shared content\n").to_hex().to_string();
    let object=store.join("objects").join(&hash[..2]).join(&hash[2..]);fs::write(&object,b"bad object").unwrap();
    let corrupt=files(&store);
    failed(command(&root).args(["remote","verify","--title","snapshots","--snapshot",&id]).output().unwrap(),"query_failed");
    failed(command(&root).args(["remote","restore","--title","snapshots","--snapshot",&id,"--label","raw","--clear"]).output().unwrap(),"command_failed");
    assert_eq!(files(&root),before);assert_eq!(files(&store),corrupt);
}
#[test]
fn correctly_addressed_but_unsafe_flat_inventories_are_rejected_before_target_changes() {
    let (_temp,root,_config,_id)=legacy();let store=root.parent().unwrap().join("store");
    let hash=blake3::hash(b"shared content\n").to_hex().to_string();let before=files(&root);
    for paths in [vec!["../outside"],vec!["same","same"],vec!["file","file/child"],vec!["A","a"],vec!["C:/escape"]] {
        let payload=json!({"label":"RAW","version":"1.2.4","files":paths.iter().map(|path|json!({"path":path,"hash":hash})).collect::<Vec<_>>()});
        let bytes=serde_json::to_vec(&payload).unwrap();let id=blake3::hash(&bytes).to_hex().to_string();
        fs::write(store.join("manifests").join(format!("{id}.json")),&bytes).unwrap();let stored=files(&store);
        failed(command(&root).args(["remote","verify","--title","snapshots","--snapshot",&id]).output().unwrap(),"query_failed");
        failed(command(&root).args(["remote","restore","--title","snapshots","--snapshot",&id,"--label","raw","--clear"]).output().unwrap(),"command_failed");
        assert_eq!(files(&root),before);assert_eq!(files(&store),stored);
    }
}
#[test]
fn old_and_tree_formats_coexist_and_report_their_actual_identity_kind() {
    let (_temp,root,mut config,old)=legacy();let store=root.parent().unwrap().join("store");let legacy_bytes=files(&store.join("manifests"));
    config["remotes"]["local"]["snapshots"]=json!({"path":"../store","structure":"cas","content":["raw"]});
    config["build"]["dest"]=json!({"snapshots":{}});save(&root,&config);
    fs::write(root.join("input/value.txt"),b"new version").unwrap();
    success(command(&root).arg("build").output().unwrap());let tree=ids(&metadata(&root),"references/v1").pop().unwrap();
    assert_ne!(old,tree);assert_eq!(files(&store.join("manifests")),legacy_bytes);
    for (id,kind,expected) in [(&old,"legacy-flat",b"shared content\n".as_slice()),(&tree,"tree-v1",b"new version".as_slice())] {
        let result=success(command(&root).args(["remote","restore","--title","snapshots","--snapshot",id,"--label","raw","--clear"]).output().unwrap());
        assert_eq!(result["result"]["snapshot_kind"],kind);assert_eq!(fs::read(root.join("input/value.txt")).unwrap(),expected);
    }
    // Conflicting same-ID namespace presence must not select one format silently.
    fs::write(store.join("manifests").join(format!("{tree}.json")),b"not a valid legacy copy").unwrap();
    let before=files(&root);let stored=files(&store);
    failed(command(&root).args(["remote","verify","--title","snapshots","--snapshot",&tree]).output().unwrap(),"query_failed");
    assert_eq!(files(&root),before);assert_eq!(files(&store),stored);
}
