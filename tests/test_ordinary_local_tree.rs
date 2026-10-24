//! Metadata-only trees preserve ordinary physical backup layouts.
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



use proj::{snapshot_format::{DirectoryTree,EntryKind},catalogue::{Catalogue,Format,Inventory}};
fn setup(placement:&str,layout:&str,cue:&str)->(tempfile::TempDir,PathBuf,Value) {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap().join("project");
    for name in ["input/nested","home"]{fs::create_dir_all(root.join(name)).unwrap();}
    fs::write(root.join("input/value"),b"same bytes").unwrap();
    fs::write(root.join("input/nested/caf\u{e9},notes"),b"same bytes").unwrap();
    fs::write(root.join("input/zero"),b"").unwrap();fs::write(root.join("VERSION"),b"v1.2.3\n").unwrap();
    let config=json!({"directories":{"raw":{"path":"input"}},
        "remotes":{"local":{"delivery":{"path":"../store","structure":layout,"manifest":placement,"manifest-style":"tree",
            "send":{"inspect":"file","strategy":"sync-diff","cue":cue}}}},
        "build":{"scripts":[],"git":false,"dest":{"delivery":{}}}});
    save(&root,&config);(temp,root,config)
}
fn save(root:&Path,value:&Value){fs::write(root.join("_proj.yml"),serde_json::to_vec_pretty(value).unwrap()).unwrap();}
fn build(root:&Path){success(command(root).arg("build").output().unwrap());}
fn metadata(root:&Path)->PathBuf {
    let mut hash=blake3::Hasher::new_derive_key("projcli.ordinary-remote-name.v1");
    hash.update(&serde_json::to_vec(&("local","DELIVERY")).unwrap());let id=hash.finalize().to_hex().to_string();
    root.join(".proj/remotes").join(&id[..2]).join(&id[2..])
}
fn bindings(base:&Path,namespace:&str)->Vec<Value> {
    files(&base.join(namespace)).into_iter().map(|(name,bytes)|{
        let (prefix,rest)=name.split_once('/').unwrap();assert_eq!(prefix.len(),2);assert_eq!(rest.len(),67);
        serde_json::from_slice(&bytes).unwrap()
    }).collect()
}
fn tree_path(base:&Path,id:&str)->PathBuf {assert_eq!(id.len(),64);base.join("trees/v1").join(&id[..2]).join(format!("{}.json",&id[2..]))}
fn tree_files(base:&Path,id:&str)->BTreeMap<String,String> {
    fn walk(base:&Path,id:&str,prefix:&str,files:&mut BTreeMap<String,String>) {
        let tree=DirectoryTree::parse(&fs::read(tree_path(base,id)).unwrap()).unwrap();assert_eq!(tree.identity().unwrap(),id);
        for entry in tree.entries {
            let name=if prefix.is_empty(){entry.name}else{format!("{prefix}/{}",entry.name)};
            match entry.kind {EntryKind::File=>{assert!(files.insert(name,entry.hash).is_none());},
                EntryKind::Tree=>walk(base,&entry.hash,&name,files)}
        }
    }
    let mut result=BTreeMap::new();walk(base,id,"",&mut result);result
}
fn assert_tree(base:&Path,id:&str,payload:&Path) {
    let expected:BTreeMap<_,_>=files(payload).into_iter().map(|(name,bytes)|(name,blake3::hash(&bytes).to_hex().to_string())).collect();
    assert_eq!(tree_files(base,id),expected);
}
#[test]
fn layouts_placements_and_cues_keep_pure_roots_and_exact_physical_bytes_without_cas_objects() {
    for layout in ["latest","archive"]{for placement in ["auto","project","remote","both"]{for cue in ["always","if-change"] {
        let (_temp,root,_config)=setup(placement,layout,cue);let store=root.parent().unwrap().join("store");
        let project=metadata(&root);let remote=store.join(".projcli-manifests");let full=if placement=="project"{&project}else{&remote};
        build(&root);let refs=bindings(&project,"references/v1");assert_eq!(refs.len(),1);
        let first=&refs[0];assert_eq!(first["schema"],"projcli.ordinary-local-tree-snapshot.v1");
        let id=first["payloads"][0]["tree"].as_str().unwrap();
        assert_ne!(first["payloads"][0]["inventory_hash"],id);
        let payload=store.join(first["payloads"][0]["path"].as_str().unwrap());assert_tree(full,id,&payload);
        assert_eq!(project.join("trees/v1").exists(),placement!="remote");
        assert_eq!(remote.join("trees/v1").exists(),placement!="project");
        for name in files(&full.join("trees/v1")).keys() {
            let (prefix,rest)=name.split_once('/').unwrap();assert_eq!(prefix.len(),2);assert_eq!(rest.len(),67);
        }
        // Same existing v1 identity as native CAS, without converting the ordinary store.
        let witness=root.parent().unwrap().join("witness");
        let native=proj::cas_tree::ingest_directory(camino::Utf8Path::from_path(&root).unwrap(),
            camino::Utf8Path::from_path(&witness).unwrap(),camino::Utf8Path::from_path(&witness).unwrap(),
            camino::Utf8Path::from_path(&payload).unwrap()).unwrap();assert_eq!(native,id);
        let nodes=files(&full.join("trees/v1"));let first_data=files(&payload);
        build(&root);let refs=bindings(&project,"references/v1");assert_eq!(refs.len(),2);
        let second=refs.iter().find(|value|value["version"]=="v1.2.5").unwrap();
        assert_eq!(second["payloads"][0]["tree"],id);assert_eq!(files(&full.join("trees/v1")),nodes);
        let second_payload=store.join(second["payloads"][0]["path"].as_str().unwrap());
        assert_eq!(files(&second_payload),first_data);assert_tree(full,id,&second_payload);
        if layout=="archive" {assert_eq!(second["payloads"][0]["path"],if cue=="always"{"raw/v1.2.5"}else{"raw/v1.2.4"});}
        let history=Catalogue::parse(&fs::read(full.join("catalogues/v1/manifest.json")).unwrap(),Format::Json).unwrap();
        assert_eq!(history.records.len(),2);assert!(matches!(&history.records[1].inventory,Inventory::Reference{ref_version} if ref_version=="v1.2.4"));
        assert!(!store.join("objects").exists());assert!(!store.join(".proj/build.lock").exists());
    }}}
}
#[test]
fn style_switches_preserve_old_flat_ids_rows_and_tree_objects() {
    let (_temp,root,mut config)=setup("both","latest","if-change");let project=metadata(&root);let store=root.parent().unwrap().join("store");
    config["remotes"]["local"]["delivery"]["manifest-style"]=json!("flat");
    config["remotes"]["local"]["delivery"]["manifest-format"]=json!("csv");save(&root,&config);build(&root);
    let old_refs=files(&project.join("references/v1"));let old_csv=fs::read(project.join("catalogues/v1/manifest.csv")).unwrap();
    config["remotes"]["local"]["delivery"]["manifest-style"]=json!("tree");
    config["remotes"]["local"]["delivery"].as_object_mut().unwrap().remove("manifest-format");save(&root,&config);build(&root);
    let tree_refs=bindings(&project,"references/v1");let tree=tree_refs.iter().find(|value|value["version"]=="v1.2.5").unwrap();
    let id=tree["payloads"][0]["tree"].as_str().unwrap();assert_tree(&project,id,&store.join("raw"));
    let old_nodes=files(&project.join("trees/v1"));assert_eq!(fs::read(project.join("catalogues/v1/manifest.csv")).unwrap(),old_csv);
    config["remotes"]["local"]["delivery"]["manifest-style"]=json!("flat");
    config["remotes"]["local"]["delivery"]["manifest-format"]=json!("json-dedupe-file");save(&root,&config);build(&root);
    let refs=files(&project.join("references/v1"));
    for (name,bytes) in old_refs {assert_eq!(refs[&name],bytes);}
    assert_eq!(files(&project.join("trees/v1")),old_nodes);
    let history=Catalogue::parse(&fs::read(project.join("catalogues/v1/manifest-dedupe-file.json")).unwrap(),Format::JsonDedupeFile).unwrap();
    assert_eq!(history.records.len(),3);assert!(matches!(&history.records[2].inventory,Inventory::Reference{ref_version} if ref_version=="v1.2.4"));
}
#[test]
fn corrupt_required_directory_object_is_preserved_and_no_new_reference_is_published() {
    let (_temp,root,_config)=setup("both","latest","if-change");build(&root);
    let project=metadata(&root);let refs=bindings(&project,"references/v1");let id=refs[0]["payloads"][0]["tree"].as_str().unwrap();
    let path=tree_path(&project,id);fs::write(&path,b"corrupt required metadata").unwrap();
    let before=files(&project);let store=root.parent().unwrap().join("store");let data=files(&store.join("raw"));
    failed(command(&root).arg("build").output().unwrap(),"command_failed");
    assert_eq!(fs::read(path).unwrap(),b"corrupt required metadata");assert_eq!(files(&project),before);
    assert_eq!(files(&store.join("raw")),data);
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap().trim(),"Version: v1.2.5");
}
#[test]
fn tree_format_validation_is_explicit_before_any_backup_changes() {
    for mode in ["csv","csv-dedupe-file","json-dedupe-file"] {
        let (_temp,root,mut config)=setup("both","latest","if-change");
        config["remotes"]["local"]["delivery"]["manifest-format"]=json!(mode);save(&root,&config);
        assert!(!command(&root).arg("build").output().unwrap().status.success());
        assert!(!root.parent().unwrap().join("store").exists());assert!(bindings(&metadata(&root),"references/v1").is_empty());
    }
    let (_temp,root,mut config)=setup("both","latest","if-change");
    config["remotes"]["local"]["delivery"]["manifest-format"]=json!("json");save(&root,&config);build(&root);
}
#[test]
fn excessive_inferred_parent_records_fail_on_private_stage_preserving_previous_backup_history() {
    let (_temp,root,mut config)=setup("both","latest","if-change");build(&root);
    let store=root.parent().unwrap().join("store");let project=metadata(&root);let old_project=files(&project);
    // Keep project provenance small: seed only the owned backup with balanced
    // empty directories. Merge retains these destination-only folders.
    // Four balanced ten-way levels have11,110 dirs; 80,000 children
    // plus8,885 extra leaves make99,995. Maximum depth is6, not64.
    // Four prior children plus one requested new file =>100,000 stage children;
    // the inferred Tree root adds one and must fail BEFORE replacement.
    let data=store.join("raw");let mut samples=Vec::new();
    for index in 0..10_000 {
        let parent=data.join(format!("t{}/g{}/f{}/h{}",index/1000,(index/100)%10,(index/10)%10,index%10));
        for child in 0..8 {
            let ordinal=index*8+child;let mut leaf=parent.join(format!("b{child}"));
            if ordinal<8_885{leaf=leaf.join("c");}
            fs::create_dir_all(&leaf).unwrap();if matches!(ordinal,0|40_000|79_999){samples.push(leaf);}
        }
    }
    let before=files(&store);
    config["remotes"]["local"]["delivery"]["send"]["strategy"]=json!("upload-missing");
    fs::write(root.join("input/new"),b"must not replace prior backup").unwrap();save(&root,&config);
    let output=command(&root).arg("build").output().unwrap();assert!(!output.status.success());
    let diagnostic=String::from_utf8_lossy(&output.stderr);
    assert!(diagnostic.contains("expanded record limit"),"Unexpected boundary error: {}",diagnostic.chars().take(2000).collect::<String>());
    let after=files(&store);assert_eq!(after.len(),before.len(),"prior backup file membership changed");
    for (path,bytes) in &before {assert_eq!(after.get(path),Some(bytes),"prior backup changed at {path}");}
    assert!(!data.join("new").exists());for path in samples{assert!(path.is_dir());}
    assert_eq!(files(&project),old_project);assert!(!store.join(".proj/build.lock").exists());
}

#[cfg(unix)]
#[test]
fn linked_tree_namespace_fails_before_backup_or_external_metadata_is_changed() {
    let (_temp,root,_config)=setup("remote","latest","if-change");
    let store=root.parent().unwrap().join("store");let base=store.join(".projcli-manifests");
    let external=root.parent().unwrap().join("outside");fs::create_dir_all(&external).unwrap();fs::write(external.join("sentinel"),b"preserved").unwrap();
    fs::create_dir_all(&base).unwrap();std::os::unix::fs::symlink(&external,base.join("trees")).unwrap();
    assert!(!command(&root).arg("build").output().unwrap().status.success());assert!(!store.join("raw").exists());
    assert_eq!(files(&external),BTreeMap::from([("sentinel".into(),b"preserved".to_vec())]));assert!(!external.join("v1").exists());
}
