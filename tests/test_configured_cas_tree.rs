//! Owned temporary CAS build fixtures; no live provider or credential changes.
use std::{fs,path::{Path,PathBuf},process::{Command,Output},collections::BTreeMap};
use serde_json::{json,Value};
use camino::Utf8Path;
use proj::{cas_tree};
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
fn upath(path:&Path)->&Utf8Path {Utf8Path::from_path(path).unwrap()}
#[test]
fn actual_build_places_full_metadata_as_selected_and_keeps_a_small_project_reference() {
    for placement in ["auto","project","remote","both"] {
        let (_temp,root,_config)=setup(placement);let store=root.parent().unwrap().join("store");let project=metadata(&root);
        success(command(&root).arg("build").output().unwrap());
        let refs=ids(&project,"references/v1");assert_eq!(refs.len(),1);
        let full_remote=placement!="project";let full_project=matches!(placement,"project"|"both");
        assert_eq!(ids(&store,"tree-snapshots/v1"),if full_remote {refs.clone()}else{vec![]});
        assert_eq!(ids(&project,"tree-snapshots/v1"),if full_project {refs.clone()}else{vec![]});
        assert_eq!(files(&store.join("objects")).len(),2);
        assert_eq!(ids(if full_remote {&store}else{&project},"trees/v1").len(),2);
        let full=if full_remote {&store}else{&project};
        let snapshot=cas_tree::read_snapshot(upath(&store),upath(full),&refs[0]).unwrap();
        assert_eq!(snapshot.version,"v1.2.4");assert_eq!(snapshot.directories.len(),2);
        assert_eq!(snapshot.directories[0].tree,snapshot.directories[1].tree);
        assert_eq!(snapshot.directories.iter().map(|item|item.label.as_str()).collect::<Vec<_>>(),["output","raw"]);
        cas_tree::verify_snapshot(upath(&store),upath(full),&refs[0]).unwrap();
        let before=files(&store);let version=fs::read(root.join("VERSION")).unwrap();
        let destination=root.join("recovered");
        cas_tree::restore_label(upath(&root),upath(&store),upath(full),&refs[0],"raw",upath(&destination),true).unwrap();
        assert_eq!(files(&destination),BTreeMap::from([("value.txt".into(),b"shared content\n".to_vec()),("zero".into(),vec![])]));
        assert!(destination.join("empty").is_dir());assert_eq!(files(&store),before);
        assert_eq!(fs::read(root.join("VERSION")).unwrap(),version);
        assert!(!root.join(".projr/manifests.csv").exists());
        let objects=files(&store.join("objects"));let trees=files(&full.join("trees/v1"));
        success(command(&root).arg("build").output().unwrap());
        assert_eq!(files(&store.join("objects")),objects);assert_eq!(files(&full.join("trees/v1")),trees);
        assert_eq!(ids(&project,"references/v1").len(),2);
        assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap().trim(),"Version: v1.2.5.1");
    }
}
#[test]
fn selected_subset_and_disabled_content_do_not_publish_other_labels() {
    let (_temp,root,mut config)=setup("remote");
    config["build"]["dest"]["snapshots"]["content"]=json!(["raw"]);save(&root,&config);
    success(command(&root).arg("build").output().unwrap());
    let store=root.parent().unwrap().join("store");let id=ids(&store,"tree-snapshots/v1").pop().unwrap();
    let snapshot=cas_tree::read_snapshot(upath(&store),upath(&store),&id).unwrap();
    assert_eq!(snapshot.directories.len(),1);assert_eq!(snapshot.directories[0].label,"raw");
    for disabled in [Value::Null,json!([]),json!(false)] {
        let (_temp,root,mut config)=setup("both");config["remotes"]["local"]["snapshots"]["content"]=disabled;save(&root,&config);
        success(command(&root).arg("build").output().unwrap());
        assert!(!root.parent().unwrap().join("store").exists());assert!(!root.join(".proj/remotes").exists());
    }
}
#[test]
fn corrupt_existing_blob_is_preserved_and_failure_retains_the_release_without_a_success_reference() {
    let (_temp,root,_config)=setup("both");let store=root.parent().unwrap().join("store");
    let hash=blake3::hash(b"shared content\n").to_hex().to_string();
    let blob=store.join("objects").join(&hash[..2]).join(&hash[2..]);fs::create_dir_all(blob.parent().unwrap()).unwrap();
    fs::write(&blob,b"corrupt previous object").unwrap();
    let output=command(&root).arg("build").output().unwrap();assert!(!output.status.success());
    assert_eq!(fs::read(&blob).unwrap(),b"corrupt previous object");
    assert!(ids(&store,"tree-snapshots/v1").is_empty());assert!(ids(&metadata(&root),"references/v1").is_empty());
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap().trim(),"Version: v1.2.4");
    let logs=files(&root.join(".proj/log"));assert!(logs.values().any(|bytes|{
        serde_json::from_slice::<Value>(bytes).ok().is_some_and(|value|value["distribution_may_have_started"]==true)
    }));
}
#[test]
fn preview_restores_selected_store_and_project_metadata_bytes_without_publication() {
    let (_temp,root,mut config)=setup("both");let store=root.parent().unwrap().join("store");
    success(command(&root).arg("build").output().unwrap());
    config["build"]["hooks"]=json!({"pre":["pre.py"]});
    fs::write(root.join("pre.py"),b"from pathlib import Path\nPath(\x27../store/preview-only\x27).write_text(\x27temporary\x27)\nPath(\x27.proj/remotes/preview-only\x27).write_text(\x27temporary\x27)\n").unwrap();
    save(&root,&config);
    let before_store=files(&store);let before_meta=files(&root.join(".proj/remotes"));let version=fs::read(root.join("VERSION")).unwrap();
    success(command(&root).arg("build-dev").output().unwrap());
    assert_eq!(files(&store),before_store);assert_eq!(files(&root.join(".proj/remotes")),before_meta);
    assert_eq!(fs::read(root.join("VERSION")).unwrap(),version);
}

#[test]
fn both_copy_failure_reports_partial_publication_and_never_writes_a_successful_project_reference() {
    let (_temp,root,_config)=setup("both");let store=root.parent().unwrap().join("store");let project=metadata(&root);
    // Derive the explicit expected snapshot using the public codec/store in a separate owned namespace.
    let calculation=root.parent().unwrap().join("calculation");
    let tree=cas_tree::ingest_directory(upath(&root),upath(&calculation),upath(&calculation),upath(&root.join("input"))).unwrap();
    let snapshot=proj::snapshot_format::TreeSnapshot::new("v1.2.4".into(),None,
        ["raw","output"].into_iter().map(|label|proj::snapshot_format::DirectoryRoot {label:label.into(),tree:tree.clone()}).collect()).unwrap();
    let id=snapshot.identity().unwrap();
    let conflict=project.join("tree-snapshots/v1").join(&id[..2]).join(format!("{}.json",&id[2..]));
    fs::create_dir_all(conflict.parent().unwrap()).unwrap();let outside=root.parent().unwrap().join("external-original");
    fs::write(&outside,b"retained conflicting metadata").unwrap();fs::hard_link(&outside,&conflict).unwrap();
    let output=command(&root).arg("build").output().unwrap();assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("partially published"));
    assert_eq!(ids(&store,"tree-snapshots/v1"),[id.clone()]);
    cas_tree::verify_snapshot(upath(&store),upath(&store),&id).unwrap();
    assert_eq!(fs::read(&outside).unwrap(),b"retained conflicting metadata");
    assert_eq!(fs::read(&conflict).unwrap(),b"retained conflicting metadata");
    assert!(ids(&project,"references/v1").is_empty());
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap().trim(),"Version: v1.2.4");
}
#[test]
fn legacy_name_list_and_mapping_subset_preserve_existing_flat_ids_and_label_spelling() {
    let (_temp,root,mut config)=setup("auto");let store=root.parent().unwrap().join("store");
    config["remotes"]["local"]["snapshots"]=json!({"path":"../store","structure":"cas","content":["RAW","output"],"inspect":"file"});
    config["build"]["dest"]=json!(["snapshots"]);save(&root,&config);
    success(command(&root).arg("build").output().unwrap());
    let previous=files(&store.join("manifests"));assert_eq!(previous.len(),2);
    let ledger=fs::read_to_string(root.join(".projr/manifests.csv")).unwrap();
    config["build"]["dest"]=json!({"snapshots":{"content":["raw"]}});save(&root,&config);
    success(command(&root).arg("build").output().unwrap());
    let after=files(&store.join("manifests"));assert_eq!(after.len(),3);
    for (name,bytes) in &previous {assert_eq!(after.get(name),Some(bytes));}
    let current=fs::read_to_string(root.join(".projr/manifests.csv")).unwrap();
    for row in ledger.lines().skip(1) {assert!(current.lines().any(|candidate|candidate==row));}
    let added:Vec<_>=after.iter().filter(|(name,_)|!previous.contains_key(*name)).collect();
    let value:Value=serde_json::from_slice(added[0].1).unwrap();assert_eq!(value["label"],"RAW");
    assert!(!store.join("trees").exists());assert!(!root.join(".proj/remotes").exists());
}
#[test]
fn unsupported_or_overlapping_targets_fail_before_hooks_and_preserve_exact_version() {
    for (key,value) in [("path",json!("input/store")),("manifest-style",json!("flat")),
        ("manifest-format",json!("csv")),("inspect",json!("file"))] {
        let (_temp,root,mut config)=setup("remote");config["remotes"]["local"]["snapshots"][key]=value;
        config["build"]["hooks"]=json!({"pre":["pre.py"]});
        fs::write(root.join("pre.py"),b"from pathlib import Path\nPath('unexpected-hook').write_text('ran')\n").unwrap();
        save(&root,&config);let before=fs::read(root.join("VERSION")).unwrap();
        assert!(!command(&root).arg("build").output().unwrap().status.success());
        assert_eq!(fs::read(root.join("VERSION")).unwrap(),before);assert!(!root.join("unexpected-hook").exists());
        assert!(!root.parent().unwrap().join("store").exists());assert!(!root.join(".proj/remotes").exists());
    }
}
