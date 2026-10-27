//! Configured Xet uses synthetic fixtures, no credentials or live remote writes.
use std::{fs,path::{Path,PathBuf},process::{Command,Output}};
use serde_json::{json,Value};
fn command(root:&Path,engine:Option<&Path>)->Command {
    let mut command=Command::new(env!("CARGO_BIN_EXE_proj"));command.current_dir(root).env_clear();
    for key in ["SYSTEMROOT","WINDIR","COMSPEC","PATHEXT","TEMP","TMP","TMPDIR"] {
        if let Some(value)=std::env::var_os(key){command.env(key,value);}
    }
    let name=if cfg!(windows) {"proj-xet-engine.exe"}else{"proj-xet-engine"};
    let mut paths:Vec<_>=std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter(|directory|!directory.join(name).is_file()).collect();
    if let Some(engine)=engine {paths.insert(0,engine.parent().unwrap().to_owned());}
    command.env("PATH",std::env::join_paths(paths).unwrap())
        .env("HOME",root.join("home")).env("USERPROFILE",root.join("home"))
        .args(["--machine","--non-interactive","--project"]).arg(root);command
}
fn success(output:Output)->Value {
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    let value:Value=serde_json::from_slice(&output.stdout).unwrap();assert_eq!(value["ok"],true);value
}
fn failed(output:Output) {
    assert!(!output.status.success());
    let value:Value=serde_json::from_slice(&output.stdout).expect("failure must be structured, not a CLI panic");
    assert_eq!(value["ok"],false);
}
fn setup()->(tempfile::TempDir,PathBuf,Value) {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap().join("project with spaces");
    for name in ["input/empty","results","cache","home"]{fs::create_dir_all(root.join(name)).unwrap();}
    fs::write(root.join("input/data.csv"),b"name,value\none,1\ntwo,2\n").unwrap();
    fs::write(root.join("results/keep.txt"),b"unselected output\n").unwrap();
    fs::write(root.join("VERSION"),b"v1.2.3\n").unwrap();
    let config=json!({"directories":{"raw-data":{"path":"input"},"output":{"path":"results"},
        "cache":{"path":"cache","safe":true}},
        "remotes":{"local":{"backup":{"path":"../store","structure":"xet","content":true}}},
        "build":{"scripts":[],"git":false,"dest":{"backup":{"content":["raw-data"]}}}});
    save(&root,&config);(temp,root,config)
}
fn save(root:&Path,value:&Value) {fs::write(root.join("_proj.yml"),serde_json::to_vec_pretty(value).unwrap()).unwrap();}
fn ids(base:&Path)->Vec<String> {
    let mut result=Vec::new();
    for first in fs::read_dir(base).unwrap() {
        let first=first.unwrap();
        for leaf in fs::read_dir(first.path()).unwrap() {
            let leaf=leaf.unwrap();result.push(format!("{}{}",first.file_name().to_str().unwrap(),
                leaf.file_name().to_str().unwrap().strip_suffix(".json").unwrap()));
        }
    }result
}
#[test]
fn queries_preview_and_empty_destination_selection_need_no_engine_and_do_not_create_store() {
    let (_temp,root,mut config)=setup();let store=root.parent().unwrap().join("store");
    success(command(&root,None).args(["yml","read"]).output().unwrap());
    let version=fs::read(root.join("VERSION")).unwrap();
    success(command(&root,None).arg("build-dev").output().unwrap());
    assert_eq!(fs::read(root.join("VERSION")).unwrap(),version);
    assert!(!store.exists());assert!(!root.join(".proj/remotes").exists());
    config["build"]["dest"]=json!({});save(&root,&config);
    success(command(&root,None).arg("build").output().unwrap());
    assert!(!store.exists());assert!(!root.join(".proj/remotes").exists());
}
#[test]
fn missing_helper_and_invalid_backend_options_fail_before_hooks_or_version_changes() {
    let (_temp,root,mut config)=setup();let store=root.parent().unwrap().join("store");
    fs::write(root.join("start.py"),"from pathlib import Path\nPath('hook-ran').write_text('bad')\n").unwrap();
    config["build"]["hooks"]=json!({"pre":["start.py"]});save(&root,&config);
    let version=fs::read(root.join("VERSION")).unwrap();
    let missing=command(&root,None).arg("build").output().unwrap();
    let expected=if cfg!(unix) {"optional pinned proj-xet-engine"}else{"Unix hard-link validation"};
    assert!(String::from_utf8_lossy(&missing.stderr).contains(expected),
        "must specifically fail helper/platform preflight: {}",String::from_utf8_lossy(&missing.stderr));
    failed(missing);
    assert_eq!(fs::read(root.join("VERSION")).unwrap(),version);assert!(!root.join("hook-ran").exists());assert!(!store.exists());
    for (key,value) in [("structure",json!("local-xet")),("manifest",json!("both")),
        ("manifest-style",json!("flat")),("manifest-format",json!("csv"))] {
        let mut invalid=config.clone();invalid["remotes"]["local"]["backup"][key]=value;save(&root,&invalid);
        failed(command(&root,None).arg("build").output().unwrap());
        assert_eq!(fs::read(root.join("VERSION")).unwrap(),version);assert!(!root.join("hook-ran").exists());assert!(!store.exists());
    }
}
#[test]
fn actual_configured_build_records_root_and_fresh_project_restores_without_upload_selection() {
    let program=std::env::var_os("PROJCLI_TEST_XET_ENGINE");
    assert!(program.is_some()||std::env::var_os("PROJCLI_TEST_XET_REQUIRED").is_none(),"required real engine missing");
    let Some(program)=program else{return};let engine=PathBuf::from(program);
    let (_temp,root,mut config)=setup();let store=root.parent().unwrap().join("store");
    let expected=fs::read(root.join("input/data.csv")).unwrap();
    success(command(&root,Some(&engine)).arg("build").output().unwrap());
    let ids=ids(&store.join("xet-snapshots/v1"));assert_eq!(ids.len(),1);let id=&ids[0];
    let observation=success(command(&root,Some(&engine)).args(["remote","verify","--title","backup","--snapshot",id]).output().unwrap());
    assert_eq!(observation["result"]["snapshot"]["snapshot_kind"],"xet-local-v1");
    let inventory=&observation["result"]["snapshot"]["observation"]["inventory"];
    assert_eq!(inventory["version"],"v1.2.4");assert_eq!(inventory["directories"].as_array().unwrap().len(),1);
    assert_eq!(inventory["directories"][0]["label"],"raw-data");
    assert_eq!(inventory["directories"][0]["files"][0]["hash"],blake3::hash(&expected).to_hex().as_str());
    let mut hash=blake3::Hasher::new_derive_key("projcli.remote-name.v1");hash.update(b"BACKUP");let key=hash.finalize().to_hex().to_string();
    let reference=root.join(".proj/remotes").join(&key[..2]).join(&key[2..]).join("xet-references/v1").join(&id[..2]).join(format!("{}.json",&id[2..]));
    let reference:Value=serde_json::from_slice(&fs::read(reference).unwrap()).unwrap();
    assert_eq!(reference["snapshot"],id.as_str());assert_eq!(reference["backend"],"xet-local-v1");
    assert!(reference.get("payloads").is_none());assert!(!store.join("objects").exists());
    assert_eq!(fs::read(root.join("results/keep.txt")).unwrap(),b"unselected output\n");
    #[cfg(unix)] {
        use std::os::unix::fs::MetadataExt;
        let database=store.join("engine/xet/xorbs/global_dedup_lookup.redb");
        let inode=fs::metadata(&database).unwrap().ino();
        fs::create_dir_all(store.join(".proj")).unwrap();
        let claim=store.join(".proj/build.lock");fs::write(&claim,b"fixture-active-reader").unwrap();
        let before=tree(&store);
        success(command(&root,None).arg("build-dev").output().unwrap());
        assert_eq!(tree(&store),before,"preview must not open/copy/restore mutable store state");
        assert_eq!(fs::metadata(&database).unwrap().ino(),inode,"preview must not replace the live database inode");
        assert_eq!(fs::read(&claim).unwrap(),b"fixture-active-reader");
        fs::remove_file(claim).unwrap();fs::remove_dir(store.join(".proj")).unwrap();
    }
    let fresh=root.parent().unwrap().join("fresh");
    for name in ["input","results","cache","home"]{fs::create_dir_all(fresh.join(name)).unwrap();}
    config["build"]["dest"]=json!({});config["directories"]["raw-data"]["source"]=json!({"local":"backup"});
    save(&fresh,&config);fs::write(fresh.join("VERSION"),b"v9.0.0-7\n").unwrap();
    fs::remove_dir_all(root.join("input")).unwrap(); // no working-source recovery route
    success(command(&fresh,Some(&engine)).args(["remote","restore","--title","backup","--snapshot",id,"--label","raw-data","--clear"]).output().unwrap());
    assert_eq!(fs::read(fresh.join("input/data.csv")).unwrap(),expected);assert!(fresh.join("input/empty").is_dir());
    assert_eq!(fs::read(fresh.join("VERSION")).unwrap(),b"v9.0.0-7\n");
    assert!(!fresh.join(".proj").exists());assert!(!fresh.join("manifest.csv").exists());
    println!("Configured structure:xet build, Git root reference, selected-label inventory and fresh explicit restore passed.");
}

#[cfg(unix)]
#[test]
fn unsafe_second_store_fails_preflight_before_any_first_store_publication_or_hooks() {
    let program=std::env::var_os("PROJCLI_TEST_XET_ENGINE");
    assert!(program.is_some()||std::env::var_os("PROJCLI_TEST_XET_REQUIRED").is_none());
    let Some(program)=program else{return};let engine=PathBuf::from(program);
    let (_temp,root,mut config)=setup();let parent=root.parent().unwrap();
    let bad=parent.join("bad-store");fs::create_dir(&bad).unwrap();
    let external=parent.join("external");fs::write(&external,b"must remain unchanged").unwrap();
    std::os::unix::fs::symlink(&external,bad.join("unsafe-link")).unwrap();
    config["remotes"]["local"]["unsafe"]=json!({"path":"../bad-store","structure":"xet","content":["raw-data"]});
    config["build"]["dest"]=json!({"backup":{"content":["raw-data"]},"unsafe":{}});
    config["build"]["hooks"]=json!({"pre":["start.py"]});
    fs::write(root.join("start.py"),"from pathlib import Path\nPath('hook-ran').write_text('bad')\n").unwrap();
    save(&root,&config);let version=fs::read(root.join("VERSION")).unwrap();
    let output=command(&root,Some(&engine)).arg("build").output().unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("symlink"),"{}",String::from_utf8_lossy(&output.stderr));
    failed(output);
    assert_eq!(fs::read(root.join("VERSION")).unwrap(),version);
    assert_eq!(fs::read(external).unwrap(),b"must remain unchanged");
    assert!(!root.join("hook-ran").exists());assert!(!parent.join("store").exists());
    assert!(!root.join(".proj/remotes").exists());
}

#[cfg(unix)]
fn tree(root:&Path)->std::collections::BTreeMap<PathBuf,Option<Vec<u8>>> {
    fn visit(root:&Path,path:&Path,result:&mut std::collections::BTreeMap<PathBuf,Option<Vec<u8>>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry=entry.unwrap();let value=entry.path();let directory=entry.file_type().unwrap().is_dir();
            result.insert(value.strip_prefix(root).unwrap().to_owned(),if directory {None}else{Some(fs::read(&value).unwrap())});
            if directory {visit(root,&value,result)}
        }
    }
    let mut result=std::collections::BTreeMap::new();visit(root,root,&mut result);result
}
#[cfg(unix)]
#[test]
fn unsupported_existing_store_format_fails_before_helper_lookup_and_hooks() {
    let (_temp,root,mut config)=setup();let store=root.parent().unwrap().join("store");
    fs::create_dir(&store).unwrap();fs::write(store.join("xet-format.json"),b"{}").unwrap();
    config["build"]["hooks"]=json!({"pre":["start.py"]});
    fs::write(root.join("start.py"),"from pathlib import Path\nPath('hook-ran').write_text('bad')\n").unwrap();
    save(&root,&config);let before=fs::read(root.join("VERSION")).unwrap();
    let output=command(&root,None).arg("build").output().unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("Unsupported Xet store version"));
    failed(output);assert_eq!(fs::read(root.join("VERSION")).unwrap(),before);
    assert_eq!(fs::read(store.join("xet-format.json")).unwrap(),b"{}");assert!(!root.join("hook-ran").exists());
}

#[cfg(unix)]
#[test]
fn unsafe_project_reference_namespace_fails_before_helper_and_store_entry() {
    let (_temp,root,mut config)=setup();
    let mut hash=blake3::Hasher::new_derive_key("projcli.remote-name.v1");hash.update(b"BACKUP");
    let key=hash.finalize().to_hex().to_string();let metadata=root.join(".proj/remotes").join(&key[..2]).join(&key[2..]);
    fs::create_dir_all(&metadata).unwrap();
    let external=root.parent().unwrap().join("foreign");fs::create_dir(&external).unwrap();
    fs::write(external.join("keep"),b"protected").unwrap();
    std::os::unix::fs::symlink(&external,metadata.join("xet-references")).unwrap();
    config["build"]["hooks"]=json!({"pre":["start.py"]});
    fs::write(root.join("start.py"),"from pathlib import Path\nPath('hook-ran').write_text('bad')\n").unwrap();
    save(&root,&config);let version=fs::read(root.join("VERSION")).unwrap();
    let output=command(&root,None).arg("build").output().unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("symlink"));
    failed(output);assert_eq!(fs::read(root.join("VERSION")).unwrap(),version);
    assert!(!root.parent().unwrap().join("store").exists());assert!(!root.join("hook-ran").exists());
    assert_eq!(fs::read(external.join("keep")).unwrap(),b"protected");
}
