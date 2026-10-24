//! Actual ordinary payload catalogue fixtures, without live remotes or credentials.
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


use proj::catalogue::{Catalogue,Format,Inventory};
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
fn setup(mode:&str,placement:&str,layout:&str,strategy:&str)->(tempfile::TempDir,PathBuf,Value) {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap().join("project");
    for name in ["input","home"] {fs::create_dir_all(root.join(name)).unwrap();}
    fs::write(root.join("input/caf\u{e9},notes.txt"),b"shared\n").unwrap();
    fs::write(root.join("input/again"),b"shared\n").unwrap();
    fs::write(root.join("VERSION"),b"v1.2.3\n").unwrap();
    let config=json!({"directories":{"raw":{"path":"input"}},
        "remotes":{"local":{"delivery":{"path":"../store","structure":layout,"manifest":placement,
            "manifest-format":mode,"send":{"inspect":"file","strategy":strategy}}}},
        "build":{"scripts":[],"git":false,"dest":{"delivery":{}}}});
    save(&root,&config);(temp,root,config)
}
fn save(root:&Path,config:&Value) {fs::write(root.join("_proj.yml"),serde_json::to_vec_pretty(config).unwrap()).unwrap();}
fn build(root:&Path){success(command(root).arg("build").output().unwrap());}
fn project_metadata(root:&Path)->PathBuf {
    let mut hash=blake3::Hasher::new_derive_key("projcli.ordinary-remote-name.v1");
    hash.update(&serde_json::to_vec(&("local","DELIVERY")).unwrap());let id=hash.finalize().to_hex().to_string();
    root.join(".proj/remotes").join(&id[..2]).join(&id[2..])
}
fn catalogue(base:&Path,mode:&str)->Catalogue {
    let filename=match mode {"csv"=>"manifest.csv","json"=>"manifest.json",
        "csv-dedupe-file"=>"manifest-dedupe-file.csv","json-dedupe-file"=>"manifest-dedupe-file.json",_=>panic!("mode")};
    Catalogue::parse(&fs::read(base.join("catalogues/v1").join(filename)).unwrap(),serde_json::from_value::<Format>(json!(mode)).unwrap()).unwrap()
}
fn bindings(base:&Path,kind:&str)->Vec<Value> {
    files(&base.join(kind)).into_iter().map(|(path,bytes)|{
        let parts:Vec<_>=path.split('/').collect();assert_eq!(parts.len(),2);
        assert_eq!(parts[0].len(),2);assert_eq!(parts[1].len(),67);
        serde_json::from_slice(&bytes).unwrap()
    }).collect()
}
fn directory(history:&Catalogue,version:&str)->proj::snapshot_format::FlatDirectory {
    history.expanded_snapshot(version,&["raw".into()]).unwrap().directories.remove(0)
}
fn assert_bytes(history:&Catalogue,version:&str,expected:&BTreeMap<String,Vec<u8>>) {
    let actual=directory(history,version);
    let hashes:BTreeMap<_,_>=actual.files.into_iter().map(|file|(file.path,file.hash)).collect();
    let expected=expected.iter().map(|(path,bytes)|(path.clone(),blake3::hash(bytes).to_hex().to_string())).collect();
    assert_eq!(hashes,expected);
}
#[test]
fn four_formats_and_placements_record_actual_latest_bytes_and_direct_version_references() {
    for mode in ["csv","json","csv-dedupe-file","json-dedupe-file"] {for placement in ["auto","project","remote","both"] {
        let (_temp,root,_config)=setup(mode,placement,"latest","upload-missing");
        let store=root.parent().unwrap().join("store");let project=project_metadata(&root);let remote=store.join(".projcli-manifests");
        fs::create_dir_all(store.join("raw")).unwrap();
        fs::write(store.join("raw/retained"),b"not in project").unwrap();
        fs::write(store.join("raw/again"),b"older bytes").unwrap();
        build(&root);let expected=files(&store.join("raw"));
        assert_eq!(expected["again"],b"older bytes");
        let full=if placement=="project"{&project}else{&remote};
        let history=catalogue(full,mode);assert_bytes(&history,"v1.2.4",&expected);
        assert_eq!(history.records.len(),1);assert_eq!(fs::read(root.join("input/again")).unwrap(),b"shared\n");
        assert!(fs::read_to_string(store.join("VERSION")).unwrap().contains("raw: v1.2.4*"));
        let first=bindings(&project,"references/v1");assert_eq!(first.len(),1);
        assert_eq!(first[0]["payloads"][0]["path"],"raw");assert_eq!(first[0]["payloads"][0]["advertised_version"],"v1.2.4*");
        assert_eq!(remote.join("catalogues").exists(),placement!="project");
        assert_eq!(project.join("catalogues").exists(),placement!="remote");
        build(&root);let history=catalogue(full,mode);
        assert_bytes(&history,"v1.2.5",&expected);
        assert!(matches!(&history.records[1].inventory,Inventory::Reference{ref_version} if ref_version=="v1.2.4"));
        assert_eq!(files(&store.join("raw")),expected);
        assert!(!store.join("objects").exists());assert!(!store.join("trees").exists());
        assert!(!store.join(".proj/build.lock").exists());
        assert_eq!(bindings(&project,"references/v1").len(),2);
        let legacy=proj::manifest::read(camino::Utf8Path::from_path(&store.join("manifest.csv")).unwrap()).unwrap();
        assert_eq!(legacy.len(),4);
    }}
}
#[test]
fn reused_archive_records_build_version_and_actual_older_location_then_changed_bytes() {
    for mode in ["csv","json","csv-dedupe-file","json-dedupe-file"] {
        let (_temp,root,_config)=setup(mode,"both","archive","sync-diff");
        let store=root.parent().unwrap().join("store");let remote=store.join(".projcli-manifests");
        build(&root);let original=files(&store.join("raw/v1.2.4"));build(&root);
        assert!(!store.join("raw/v1.2.5").exists());
        let history=catalogue(&remote,mode);assert_bytes(&history,"v1.2.5",&original);
        let refs=bindings(&remote,"snapshots/v1");
        let reused=refs.iter().find(|value|value["version"]=="v1.2.5").unwrap();
        assert_eq!(reused["layout"],"archive");assert_eq!(reused["payloads"][0]["path"],"raw/v1.2.4");
        fs::rename(root.join("input/again"),root.join("input/renamed")).unwrap();
        fs::write(root.join("input/new"),b"new").unwrap();
        build(&root);let changed=files(&store.join("raw/v1.2.6"));
        assert_bytes(&catalogue(&remote,mode),"v1.2.6",&changed);
        assert_eq!(files(&store.join("raw/v1.2.4")),original);
    }
}
#[test]
fn same_release_conflict_fails_before_replacing_data_and_format_switch_keeps_old_history() {
    let (_temp,root,mut config)=setup("csv","both","latest","sync-purge");
    let store=root.parent().unwrap().join("store");let remote=store.join(".projcli-manifests");
    build(&root);let saved=files(&store);
    fs::write(root.join("input/again"),b"changed").unwrap();fs::write(root.join("VERSION"),b"v1.2.3\n").unwrap();
    failed(command(&root).arg("build").output().unwrap(),"command_failed");
    assert_eq!(files(&store),saved);
    fs::write(root.join("VERSION"),b"v1.2.4.1\n").unwrap();
    config["remotes"]["local"]["delivery"]["manifest-format"]=json!("json-dedupe-file");save(&root,&config);
    build(&root);assert_eq!(catalogue(&remote,"csv").records.len(),1);
    let history=catalogue(&remote,"json-dedupe-file");assert_eq!(history.records.len(),2);
    assert_bytes(&history,"v1.2.5",&files(&store.join("raw")));
}
#[test]
fn empty_inventory_and_preflight_metadata_namespace_corruption_and_store_claim_fail_closed() {
    let (_temp,root,mut config)=setup("json","remote","archive","sync-diff");
    for entry in fs::read_dir(root.join("input")).unwrap(){fs::remove_file(entry.unwrap().path()).unwrap();}
    build(&root);let store=root.parent().unwrap().join("store");let remote=store.join(".projcli-manifests");
    assert!(store.join("raw/v1.2.4-empty").is_dir());let history=catalogue(&remote,"json");
    assert!(directory(&history,"v1.2.4").files.is_empty());
    let refs=bindings(&remote,"snapshots/v1");assert_eq!(refs[0]["payloads"][0]["path"],"raw/v1.2.4-empty");
    assert_eq!(refs[0]["payloads"][0]["empty"],true);
    let old=files(&store);
    fs::create_dir_all(store.join(".proj")).unwrap();fs::write(store.join(".proj/build.lock"),b"other owner").unwrap();
    failed(command(&root).arg("build").output().unwrap(),"command_failed");
    let mut expected=old.clone();expected.insert(".proj/build.lock".into(),b"other owner".to_vec());assert_eq!(files(&store),expected);
    fs::remove_file(store.join(".proj/build.lock")).unwrap();
    fs::write(remote.join("catalogues/v1/manifest.json"),b"{}").unwrap();
    let corrupted=files(&store);
    failed(command(&root).arg("build").output().unwrap(),"command_failed");assert_eq!(files(&store),corrupted);
    config["directories"]=json!({".projcli-manifests":{"path":"input"}});
    config["remotes"]["local"]["delivery"]["content"]=json!([".projcli-manifests"]);save(&root,&config);
    let output=command(&root).arg("build").output().unwrap();assert!(!output.status.success());
    assert_eq!(files(&store),corrupted);
}
#[test]
fn registry_defaults_enable_csv_but_legacy_inline_remains_unchanged_and_invalid_fields_fail() {
    let (_temp,root,mut config)=setup("csv","auto","latest","sync-diff");
    config["remotes"]["local"]["delivery"].as_object_mut().unwrap().remove("manifest-format");
    config["remotes"]["local"]["delivery"].as_object_mut().unwrap().remove("manifest");save(&root,&config);
    build(&root);let store=root.parent().unwrap().join("store");assert_eq!(catalogue(&store.join(".projcli-manifests"),"csv").records.len(),1);
    let (_temp,root,mut config)=setup("csv","auto","latest","sync-diff");
    let mut definition=config["remotes"]["local"]["delivery"].take();
    for key in ["manifest-format","manifest"]{definition.as_object_mut().unwrap().remove(key);}
    config.as_object_mut().unwrap().remove("remotes");config["build"].as_object_mut().unwrap().remove("dest");
    config["build"]["local"]=json!({"delivery":definition});save(&root,&config);build(&root);
    let store=root.parent().unwrap().join("store");assert!(!store.join(".projcli-manifests").exists());assert!(!project_metadata(&root).exists());
    for (key,value) in [("manifest",Value::Null),("manifest-style",json!("tree")),("manifest-format",json!("unknown"))] {
        config["build"]["local"]["delivery"][key]=value;save(&root,&config);
        let before=files(&store);assert!(!command(&root).arg("build").output().unwrap().status.success());assert_eq!(files(&store),before);
        config["build"]["local"]["delivery"].as_object_mut().unwrap().remove(key);
    }
}
#[cfg(unix)]
#[test]
fn linked_metadata_is_rejected_before_any_target_bytes_change() {
    let (_temp,root,_config)=setup("csv","both","latest","sync-diff");build(&root);
    let store=root.parent().unwrap().join("store");let remote=store.join(".projcli-manifests");
    let outside=root.parent().unwrap().join("outside");fs::write(&outside,b"unchanged").unwrap();
    let path=remote.join("catalogues/v1/manifest.csv");fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&outside,&path).unwrap();
    let previous=files(&store.join("raw"));assert!(!command(&root).arg("build").output().unwrap().status.success());
    assert_eq!(files(&store.join("raw")),previous);assert_eq!(fs::read(outside).unwrap(),b"unchanged");
}

#[test]
fn aliased_inline_metadata_titles_fail_before_hooks_or_external_writes() {
    let (_temp,root,mut config)=setup("json","project","latest","sync-diff");
    let mut definition=config["remotes"]["local"]["delivery"].take();
    definition["content"]=json!(true);
    let mut second=definition.clone();second["path"]=json!("../second");
    config.as_object_mut().unwrap().remove("remotes");
    config["build"].as_object_mut().unwrap().remove("dest");
    config["build"]["local"]=json!({"A":definition,"a":second});
    config["build"]["hooks"]=json!({"pre":["start.py"]});
    fs::write(root.join("start.py"),"from pathlib import Path\nPath('hook-ran').write_text('bad')\n").unwrap();
    save(&root,&config);
    let before=snapshot(&root);assert!(!command(&root).arg("build").output().unwrap().status.success());
    assert_eq!(recovered_snapshot(&root,&before),before);assert!(!root.join("hook-ran").exists());
    assert!(!root.parent().unwrap().join("store").exists());
    assert!(!root.parent().unwrap().join("second").exists());
}
#[test]
fn no_action_empty_normalization_conflict_preserves_existing_directory_shape() {
    let (_temp,root,_config)=setup("csv","both","latest","upload-missing");build(&root);
    let store=root.parent().unwrap().join("store");
    for base in [root.join("input"),store.join("raw")] {
        for entry in fs::read_dir(&base).unwrap(){fs::remove_file(entry.unwrap().path()).unwrap();}
    }
    fs::create_dir(store.join("raw/empty-child")).unwrap();
    fs::write(root.join("VERSION"),b"v1.2.3\n").unwrap();
    let before=files(&store);failed(command(&root).arg("build").output().unwrap(),"command_failed");
    assert_eq!(files(&store),before);assert!(store.join("raw/empty-child").is_dir());assert!(!store.join("raw-empty").exists());
}
#[test]
fn oversized_existing_binding_fails_bounded_and_preserves_it() {
    let (_temp,root,_config)=setup("json","both","latest","sync-diff");build(&root);
    let store=root.parent().unwrap().join("store");let full=store.join(".projcli-manifests/snapshots/v1");
    let relative=files(&full).into_keys().next().unwrap();let binding=full.join(relative);
    let file=fs::OpenOptions::new().write(true).truncate(true).open(&binding).unwrap();
    file.set_len((proj::snapshot_format::MAX_METADATA_BYTES+1) as u64).unwrap();drop(file);
    fs::write(root.join("VERSION"),b"v1.2.3\n").unwrap();let data=files(&store.join("raw"));
    let output=command(&root).arg("build").output().unwrap();
    assert!(!output.status.success());assert_eq!(files(&store.join("raw")),data);
    assert_eq!(fs::metadata(binding).unwrap().len(),(proj::snapshot_format::MAX_METADATA_BYTES+1) as u64);
    let response:Value=serde_json::from_slice(&output.stdout).unwrap();assert_eq!(response["error"]["code"],"command_failed");
    assert!(String::from_utf8_lossy(&output.stderr).contains("bounded regular file"));
}

#[test]
fn native_metadata_base_aliases_fail_on_case_folding_filesystems() {
    let (_temp,root,_config)=setup("csv","both","latest","sync-diff");
    let store=root.parent().unwrap().join("store");
    fs::create_dir_all(store.join(".PROJCLI-MANIFESTS")).unwrap();
    fs::write(store.join(".PROJCLI-MANIFESTS/sentinel"),b"preserved").unwrap();
    let alias=store.join(".projcli-manifests").exists();
    let output=command(&root).arg("build").output().unwrap();
    if alias {
        assert!(!output.status.success());assert!(!store.join("raw").exists());
    }else{success(output);}
    assert_eq!(fs::read(store.join(".PROJCLI-MANIFESTS/sentinel")).unwrap(),b"preserved");
}
