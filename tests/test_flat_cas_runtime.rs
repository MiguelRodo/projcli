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


use proj::{catalogue::{Catalogue,Format,Inventory},snapshot_format::FlatSnapshot};
use camino::Utf8Path;
fn upath(path:&Path)->&Utf8Path {Utf8Path::from_path(path).unwrap()}
fn flat(mode:&str,placement:&str)->(tempfile::TempDir,PathBuf,Value) {
    let (temp,root,mut config)=setup(placement);
    for name in ["input","results"] {
        fs::write(root.join(name).join("repeat.txt"),b"shared content\n").unwrap();
        fs::create_dir_all(root.join(name).join("nested")).unwrap();
        fs::write(root.join(name).join("nested/caf\u{e9},notes.txt"),b"shared content\n").unwrap();
    }
    config["remotes"]["local"]["snapshots"]["manifest-style"]=json!("flat");
    config["remotes"]["local"]["snapshots"]["manifest-format"]=json!(mode);save(&root,&config);
    (temp,root,config)
}
fn format(mode:&str)->Format {serde_json::from_value(json!(mode)).unwrap()}
fn catalogue_path(base:&Path,mode:&str)->PathBuf {
    base.join(match mode {"csv"=>"catalogues/v1/manifest.csv","json"=>"catalogues/v1/manifest.json",
        "csv-dedupe-file"=>"catalogues/v1/manifest-dedupe-file.csv","json-dedupe-file"=>"catalogues/v1/manifest-dedupe-file.json",_=>panic!("fixture mode")})
}
fn history(base:&Path,mode:&str)->Catalogue {
    Catalogue::parse(&fs::read(catalogue_path(base,mode)).unwrap(),format(mode)).unwrap()
}
fn observed(root:&Path,store:&Path,version:&str,labels:&[&str])->FlatSnapshot {
    FlatSnapshot::new(version.into(),None,labels.iter().map(|label|{
        let source=if *label=="raw" {"input"}else{"results"};
        proj::cas_tree::ingest_flat_directory(upath(root),upath(store),upath(&root.join(source)),label).unwrap()
    }).collect()).unwrap()
}
#[test]
fn four_formats_and_placements_publish_version_references_and_restore_complete_membership() {
    let mut canonical_id=None;
    for mode in ["csv","json","csv-dedupe-file","json-dedupe-file"] {for placement in ["auto","remote","project","both"] {
        let (_temp,root,_config)=flat(mode,placement);let store=root.parent().unwrap().join("store");let project=metadata(&root);
        let expected=files(&root.join("input"));
        success(command(&root).arg("build").output().unwrap());
        let id=ids(&project,"references/flat-v1").pop().unwrap();
        if let Some(previous)=&canonical_id {assert_eq!(&id,previous)}else{canonical_id=Some(id.clone());}
        let full=if placement=="project" {&project}else{&store};
        assert_eq!(ids(&store,"flat-snapshots/v1"),if placement=="project" {vec![]}else{vec![id.clone()]});
        assert_eq!(ids(&project,"flat-snapshots/v1"),if matches!(placement,"project"|"both") {vec![id.clone()]}else{vec![]});
        assert!(!store.join("trees").exists());assert!(!project.join("trees").exists());
        assert_eq!(files(&store.join("objects")).len(),2);assert!(!store.join(".proj/build.lock").exists());
        let first=history(full,mode);assert_eq!(first.records.len(),2);
        for row in &first.records {assert!(matches!(row.inventory,Inventory::Complete {..}));}
        let wire=fs::read(catalogue_path(full,mode)).unwrap();
        if mode.starts_with("csv") {
            let mut reader=csv::Reader::from_reader(wire.as_slice());assert_eq!(reader.headers().unwrap().len(),13);
            let rows:Vec<_>=reader.records().map(Result::unwrap).collect();
            assert_eq!(rows.iter().any(|row|row.get(3)==Some("dictionary")),mode.contains("dedupe-file"));
            let paths:std::collections::BTreeSet<_>=rows.iter().filter(|row|row.get(3)==Some("file")&&row.get(5)==Some("raw")&&row.get(6)==Some("v1.2.4"))
                .map(|row|row.get(9).unwrap().to_owned()).collect();
            assert_eq!(paths,expected.keys().cloned().collect());
        }else{
            let wire:Value=serde_json::from_slice(&wire).unwrap();
            assert_eq!(wire.get("dictionary").is_some(),mode.contains("dedupe-file"));
        }
        success(command(&root).arg("build").output().unwrap());
        let second=history(full,mode);assert_eq!(second.records.len(),4);
        for row in second.records.iter().filter(|row|row.version=="v1.2.5") {
            assert!(matches!(&row.inventory,Inventory::Reference {ref_version} if ref_version=="v1.2.4"));
        }
        let snapshot=proj::cas_flat::verify_snapshot(upath(&store),upath(full),&id).unwrap();
        assert_eq!(snapshot.directories.len(),2);assert_eq!(snapshot.directories[0].files.len(),4);
        let stored=files(&store);let project_bytes=files(&project);let version=fs::read(root.join("VERSION")).unwrap();let csv=fs::read(root.join("manifest.csv")).unwrap();
        fs::write(root.join("input/value.txt"),b"damaged").unwrap();
        let result=success(command(&root).args(["remote","restore","--title","snapshots","--snapshot",&id,"--label","raw","--clear"]).output().unwrap());
        assert_eq!(result["result"]["snapshot_kind"],"flat-catalogue-v1");assert_eq!(files(&root.join("input")),expected);
        assert!(root.join("input/empty").is_dir());assert_eq!(files(&store),stored);assert_eq!(files(&project),project_bytes);
        assert_eq!(fs::read(root.join("VERSION")).unwrap(),version);assert_eq!(fs::read(root.join("manifest.csv")).unwrap(),csv);
        let mut root_csv=csv::Reader::from_reader(csv.as_slice());
        assert_eq!(root_csv.headers().unwrap().iter().collect::<Vec<_>>(),["label","fn","version","hash"]);
    }}
}
#[test]
fn changes_renames_additions_deletions_and_reversion_keep_earlier_complete_inventories() {
    for mode in ["csv","json","csv-dedupe-file","json-dedupe-file"] {
        let (_temp,root,_config)=flat(mode,"remote");let store=root.parent().unwrap().join("store");
        success(command(&root).arg("build").output().unwrap());let first=ids(&store,"flat-snapshots/v1").pop().unwrap();
        fs::write(root.join("input/value.txt"),b"changed").unwrap();
        fs::rename(root.join("input/repeat.txt"),root.join("input/renamed.txt")).unwrap();
        fs::remove_file(root.join("input/zero")).unwrap();fs::write(root.join("input/added.txt"),b"new").unwrap();
        success(command(&root).arg("build").output().unwrap());
        let changed=history(&store,mode);let raw=changed.records.iter().find(|row|row.label=="raw"&&row.version=="v1.2.5").unwrap();
        assert!(matches!(raw.inventory,Inventory::Complete {..}));
        let directory=changed.expanded("raw","v1.2.5").unwrap();
        let paths:Vec<_>=directory.files.iter().map(|file|file.path.as_str()).collect();
        assert_eq!(paths,["added.txt","nested/caf\u{e9},notes.txt","renamed.txt","value.txt"]);
        success(command(&root).args(["remote","restore","--title","snapshots","--snapshot",&first,"--label","raw","--clear"]).output().unwrap());
        success(command(&root).arg("build").output().unwrap());
        let reverted=history(&store,mode);
        let row=reverted.records.iter().find(|row|row.label=="raw"&&row.version=="v1.2.6").unwrap();
        assert!(matches!(&row.inventory,Inventory::Reference {ref_version} if ref_version=="v1.2.4"));
        assert_eq!(reverted.expanded("raw","v1.2.6").unwrap(),reverted.expanded("raw","v1.2.4").unwrap());
    }
}
#[test]
fn mode_switches_and_partial_same_version_retries_preserve_history_and_identity() {
    let (_temp,root,_config)=flat("csv","both");let store=root.parent().unwrap().join("store");let project=metadata(&root);
    let bases=vec![upath(&store).to_owned(),upath(&project).to_owned()];
    let snapshot=observed(&root,&store,"v1.0.0",&["raw"]);
    let mut id=None;let mut previous=BTreeMap::new();
    for mode in ["csv","json","csv-dedupe-file","json-dedupe-file"] {
        let current=proj::cas_flat::publish_snapshot(upath(&root),upath(&store),&bases,Some(upath(&project)),&snapshot,format(mode)).unwrap();
        if let Some(id)=&id {assert_eq!(&current,id)}else{id=Some(current);}
        for (path,bytes) in &previous {assert_eq!(fs::read(store.join(path)).unwrap(),*bytes);}
        previous=files(&store);assert_eq!(history(&store,mode).records.len(),1);
    }
    let whole=observed(&root,&store,"v1.0.0",&["raw","output"]);
    proj::cas_flat::publish_snapshot(upath(&root),upath(&store),&bases,Some(upath(&project)),&whole,Format::Json).unwrap();
    assert_eq!(history(&store,"json").records.len(),2);
    let baseline=files(&store);let baseline_project=files(&project);
    proj::cas_flat::publish_snapshot(upath(&root),upath(&store),&bases,Some(upath(&project)),&whole,Format::Json).unwrap();
    assert_eq!(files(&store),baseline);assert_eq!(files(&project),baseline_project);
    fs::write(root.join("input/value.txt"),b"conflicting same version").unwrap();
    let conflict=observed(&root,&store,"v1.0.0",&["raw"]);
    let baseline=files(&store);
    assert!(proj::cas_flat::publish_snapshot(upath(&root),upath(&store),&bases,None,&conflict,Format::Json).is_err());
    assert_eq!(files(&store),baseline);assert_eq!(files(&project),baseline_project);
}
#[test]
fn stale_store_claim_and_corrupt_catalogue_fail_without_replacing_history_or_destination() {
    let (_temp,root,_config)=flat("json","remote");let store=root.parent().unwrap().join("store");
    success(command(&root).arg("build").output().unwrap());let id=ids(&store,"flat-snapshots/v1").pop().unwrap();
    let path=catalogue_path(&store,"json");let original=fs::read(&path).unwrap();
    let mut bad:Value=serde_json::from_slice(&original).unwrap();bad["records"][0]["inventory_hash"]=json!("0".repeat(64));
    fs::write(&path,serde_json::to_vec(&bad).unwrap()).unwrap();let before=files(&root);let stored=files(&store);
    failed(command(&root).args(["remote","verify","--title","snapshots","--snapshot",&id]).output().unwrap(),"query_failed");
    failed(command(&root).args(["remote","restore","--title","snapshots","--snapshot",&id,"--label","raw","--clear"]).output().unwrap(),"command_failed");
    assert_eq!(files(&root),before);assert_eq!(files(&store),stored);fs::write(&path,original).unwrap();
    let other=FlatSnapshot::new("v1.2.4".into(),None,vec![proj::snapshot_format::FlatDirectory {
        label:"output".into(),files:vec![proj::snapshot_format::FileRecord {path:"different-name".into(),
        hash:blake3::hash(b"shared content\n").to_hex().to_string()}],empty_directories:vec![]}]).unwrap();
    let divergent=Catalogue::default().with_snapshot(&other).unwrap();
    let divergent_path=catalogue_path(&store,"csv");
    fs::write(&divergent_path,divergent.encode(Format::Csv).unwrap()).unwrap();
    let stored=files(&store);
    failed(command(&root).args(["remote","verify","--title","snapshots","--snapshot",&id]).output().unwrap(),"query_failed");
    assert_eq!(files(&store),stored);fs::remove_file(divergent_path).unwrap();
    let snapshot=observed(&root,&store,"v2.0.0",&["raw"]);fs::create_dir_all(store.join(".proj")).unwrap();
    fs::write(store.join(".proj/build.lock"),b"other owner").unwrap();let stored=files(&store);
    assert!(proj::cas_flat::publish_snapshot(upath(&root),upath(&store),&[upath(&store).to_owned()],None,&snapshot,Format::Json).is_err());
    assert_eq!(files(&store),stored);assert_eq!(fs::read(store.join(".proj/build.lock")).unwrap(),b"other owner");
}
// Permission witness requires the unprivileged users provided by remote workflows.
#[cfg(unix)]
#[test]
fn second_history_copy_failure_retries_without_duplicate_rows_and_preserves_hardlinked_old_bytes() {
    use std::os::unix::fs::PermissionsExt;
    let (_temp,root,_config)=flat("csv","both");let store=root.parent().unwrap().join("store");let project=metadata(&root);
    let bases=vec![upath(&store).to_owned(),upath(&project).to_owned()];
    let first=observed(&root,&store,"v1.0.0",&["raw"]);
    proj::cas_flat::publish_snapshot(upath(&root),upath(&store),&bases,Some(upath(&project)),&first,Format::Csv).unwrap();
    let path=catalogue_path(&project,"csv");let original=fs::read(&path).unwrap();
    let outside=root.parent().unwrap().join("previous-external-history");fs::hard_link(&path,&outside).unwrap();
    fs::write(root.join("input/value.txt"),b"changed").unwrap();
    let next=observed(&root,&store,"v1.0.1",&["raw"]);
    let parent=path.parent().unwrap();let permissions=fs::metadata(parent).unwrap().permissions();
    fs::set_permissions(parent,fs::Permissions::from_mode(0o500)).unwrap();
    let failed_copy=proj::cas_flat::publish_snapshot(upath(&root),upath(&store),&bases,Some(upath(&project)),&next,Format::Csv);
    fs::set_permissions(parent,permissions).unwrap();
    assert!(failed_copy.unwrap_err().to_string().contains("partially published"));
    assert_eq!(history(&store,"csv").records.len(),2);assert_eq!(history(&project,"csv").records.len(),1);
    assert_eq!(ids(&store,"flat-snapshots/v1").len(),1);assert_eq!(ids(&project,"references/flat-v1").len(),1);
    proj::cas_flat::publish_snapshot(upath(&root),upath(&store),&bases,Some(upath(&project)),&next,Format::Csv).unwrap();
    assert_eq!(history(&store,"csv").records.len(),2);assert_eq!(history(&project,"csv").records.len(),2);
    assert_eq!(ids(&store,"flat-snapshots/v1").len(),2);assert_eq!(fs::read(&outside).unwrap(),original);
    assert!(!store.join(".proj/build.lock").exists());
}

#[test]
fn broken_file_dictionaries_and_snapshot_bindings_fail_as_structured_readonly_errors() {
    for mode in ["csv-dedupe-file","json-dedupe-file"] {
        let (_temp,root,_config)=flat(mode,"remote");let store=root.parent().unwrap().join("store");
        success(command(&root).arg("build").output().unwrap());let id=ids(&store,"flat-snapshots/v1").pop().unwrap();
        let path=catalogue_path(&store,mode);let original=fs::read(&path).unwrap();
        if mode.starts_with("csv") {
            let mut reader=csv::Reader::from_reader(original.as_slice());let header=reader.headers().unwrap().clone();
            let mut writer=csv::Writer::from_writer(Vec::new());writer.write_record(&header).unwrap();let mut removed=false;
            for row in reader.records() {let row=row.unwrap();if !removed&&row.get(3)==Some("dictionary") {removed=true;continue}writer.write_record(&row).unwrap();}
            assert!(removed);fs::write(&path,writer.into_inner().unwrap()).unwrap();
        }else{
            let mut value:Value=serde_json::from_slice(&original).unwrap();value["dictionary"].as_array_mut().unwrap().remove(0);
            fs::write(&path,serde_json::to_vec(&value).unwrap()).unwrap();
        }
        let before=files(&root);let stored=files(&store);
        failed(command(&root).args(["remote","verify","--title","snapshots","--snapshot",&id]).output().unwrap(),"query_failed");
        failed(command(&root).args(["remote","restore","--title","snapshots","--snapshot",&id,"--label","raw","--clear"]).output().unwrap(),"command_failed");
        assert_eq!(files(&root),before);assert_eq!(files(&store),stored);fs::write(&path,original).unwrap();
        let reference=store.join("flat-snapshots/v1").join(&id[..2]).join(format!("{}.json",&id[2..]));
        let mut value:Value=serde_json::from_slice(&fs::read(&reference).unwrap()).unwrap();value["version"]=json!("v9.0.0");
        fs::write(&reference,serde_json::to_vec(&value).unwrap()).unwrap();let stored=files(&store);
        failed(command(&root).args(["remote","verify","--title","snapshots","--snapshot",&id]).output().unwrap(),"query_failed");
        assert_eq!(files(&root),before);assert_eq!(files(&store),stored);
    }
}
#[test]
fn csv_is_the_new_flat_default_and_restore_cannot_target_its_own_metadata() {
    let (_temp,root,mut config)=flat("csv","remote");let store=root.parent().unwrap().join("store");
    config["remotes"]["local"]["snapshots"].as_object_mut().unwrap().remove("manifest-format");save(&root,&config);
    success(command(&root).arg("build").output().unwrap());assert!(catalogue_path(&store,"csv").is_file());
    assert!(!catalogue_path(&store,"csv-dedupe-file").exists());
    let id=ids(&store,"flat-snapshots/v1").pop().unwrap();
    config["build"]["dest"]=json!({});config["directories"]["raw"]["path"]=json!("../store/catalogues");save(&root,&config);
    let before=files(&root);let stored=files(&store);
    failed(command(&root).args(["remote","restore","--title","snapshots","--snapshot",&id,"--label","raw","--clear"]).output().unwrap(),"command_failed");
    assert_eq!(files(&root),before);assert_eq!(files(&store),stored);
}

#[test]
fn failed_flat_publication_preserves_local_release_and_distribution_evidence() {
    let (_temp,root,_config)=flat("csv","remote");let store=root.parent().unwrap().join("store");
    let hash=blake3::hash(b"shared content\n").to_hex().to_string();
    let path=store.join("objects").join(&hash[..2]).join(&hash[2..]);
    fs::create_dir_all(path.parent().unwrap()).unwrap();fs::write(&path,b"corrupt existing bytes").unwrap();
    failed(command(&root).arg("build").output().unwrap(),"command_failed");
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap().trim(),"Version: v1.2.4");
    assert_eq!(fs::read(path).unwrap(),b"corrupt existing bytes");
    assert!(ids(&store,"flat-snapshots/v1").is_empty());assert!(ids(&metadata(&root),"references/flat-v1").is_empty());
    let evidence:Vec<Value>=files(&root.join(".proj/log")).values().filter_map(|bytes|serde_json::from_slice(bytes).ok()).collect();
    assert!(evidence.iter().any(|value|value["status"]=="failed"&&value["distribution_may_have_started"]==true
        &&value["operations"].as_array().is_some_and(|operations|operations.iter().any(|operation|
            operation["provider"]=="cas_flat"&&operation["status"]=="failed"))));
}
