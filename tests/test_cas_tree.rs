//! Explicit filesystem CAS APIs only; fixtures never mutate process environment or real stores.
use std::{fs,path::{Path,PathBuf}};
use camino::{Utf8Path,Utf8PathBuf};
use proj::{cas_tree,snapshot_format::{DirectoryTree,TreeEntry,EntryKind,TreeSnapshot,DirectoryRoot,FlatSnapshot}};

struct Fixture {
    _temp:tempfile::TempDir,project:Utf8PathBuf,source:Utf8PathBuf,store:Utf8PathBuf,
}
impl Fixture {
    fn new()->Self {
        let temp=tempfile::tempdir().unwrap();
        let base=Utf8PathBuf::try_from(temp.path().canonicalize().unwrap()).unwrap();
        let project=base.join("project");let source=project.join("input");let store=base.join("store");
        fs::create_dir_all(source.join("unchanged")).unwrap();
        fs::create_dir(source.join("changed")).unwrap();fs::create_dir(source.join("empty")).unwrap();
        fs::write(source.join("unchanged/a.txt"),b"alpha").unwrap();
        fs::write(source.join("changed/b.txt"),b"beta").unwrap();fs::write(source.join("zero"),b"").unwrap();
        Self {_temp:temp,project,source,store}
    }
    fn ingest(&self,metadata:&Utf8Path)->String {
        cas_tree::ingest_directory(&self.project,&self.store,metadata,&self.source).unwrap()
    }
    fn snapshot(&self,root:&str,version:&str)->TreeSnapshot {
        TreeSnapshot::new(version.into(),None,vec![DirectoryRoot {label:"output".into(),tree:root.into()}]).unwrap()
    }
    fn publish(&self,metadata:&Utf8Path,snapshot:&TreeSnapshot)->String {
        cas_tree::publish_snapshot(&self.project,&self.store,metadata,snapshot).unwrap()
    }
}
fn tree_path(metadata:&Utf8Path,id:&str)->Utf8PathBuf {
    metadata.join("trees/v1").join(&id[..2]).join(format!("{}.json",&id[2..]))
}
fn snapshot_path(metadata:&Utf8Path,id:&str)->Utf8PathBuf {
    metadata.join("tree-snapshots/v1").join(&id[..2]).join(format!("{}.json",&id[2..]))
}
fn object_path(store:&Utf8Path,bytes:&[u8])->Utf8PathBuf {
    let hash=blake3::hash(bytes).to_hex().to_string();
    store.join("objects").join(&hash[..2]).join(&hash[2..])
}
fn trees(metadata:&Utf8Path,id:&str)->DirectoryTree {
    let value=DirectoryTree::parse(&fs::read(tree_path(metadata,id)).unwrap()).unwrap();
    assert_eq!(value.identity().unwrap(),id);value
}
fn files(root:&Path)->Vec<PathBuf> {
    if !root.exists(){return Vec::new()}
    let mut found=Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry=entry.unwrap();let kind=entry.file_type().unwrap();
        if kind.is_dir(){found.extend(files(&entry.path()));}
        else if kind.is_file(){found.push(entry.path());}
    }
    found.sort();found
}
fn write_tree(metadata:&Utf8Path,tree:&DirectoryTree)->String {
    let id=tree.identity().unwrap();let path=tree_path(metadata,&id);
    fs::create_dir_all(path.parent().unwrap()).unwrap();fs::write(path,tree.readable_json().unwrap()).unwrap();id
}

#[test]
fn complete_remote_graph_survives_project_loss_and_reuses_unchanged_subtrees() {
    let fixture=Fixture::new();
    let root1=fixture.ingest(&fixture.store);let tree1=trees(&fixture.store,&root1);
    let snapshot1=fixture.snapshot(&root1,"v1.2.3");
    let id1=fixture.publish(&fixture.store,&snapshot1);
    assert_eq!(fixture.ingest(&fixture.store),root1);
    assert_eq!(fixture.publish(&fixture.store,&snapshot1),id1);
    assert_eq!(files(fixture.store.join("objects").as_std_path()).len(),3);
    assert_eq!(files(fixture.store.join("trees/v1").as_std_path()).len(),4);
    assert_eq!(files(fixture.store.join("tree-snapshots/v1").as_std_path()).len(),1);
    assert!(!fixture.project.join(".projr/manifests.csv").exists());
    assert!(!fixture.project.join(".proj").exists());
    let snapshot_bytes=fs::read(snapshot_path(&fixture.store,&id1)).unwrap();
    let root_bytes=fs::read(tree_path(&fixture.store,&root1)).unwrap();

    fs::write(fixture.source.join("changed/b.txt"),b"beta changed").unwrap();
    let root2=fixture.ingest(&fixture.store);assert_ne!(root1,root2);
    let tree2=trees(&fixture.store,&root2);
    let child=|tree:&DirectoryTree,name:&str|tree.entries.iter().find(|e|e.name==name).unwrap().hash.clone();
    assert_eq!(child(&tree1,"unchanged"),child(&tree2,"unchanged"));
    assert_ne!(child(&tree1,"changed"),child(&tree2,"changed"));
    assert_eq!(child(&tree1,"empty"),child(&tree2,"empty"));
    assert_eq!(files(fixture.store.join("objects").as_std_path()).len(),4);
    assert_eq!(files(fixture.store.join("trees/v1").as_std_path()).len(),6);
    let id2=fixture.publish(&fixture.store,&fixture.snapshot(&root2,"v1.2.4"));
    assert_eq!(fs::read(snapshot_path(&fixture.store,&id1)).unwrap(),snapshot_bytes);
    assert_eq!(fs::read(tree_path(&fixture.store,&root1)).unwrap(),root_bytes);

    fs::remove_dir_all(&fixture.project).unwrap(); // Only this owned temporary working copy.
    let old=cas_tree::verify_snapshot(&fixture.store,&fixture.store,&id1).unwrap();
    let current=cas_tree::verify_snapshot(&fixture.store,&fixture.store,&id2).unwrap();
    let old_flat=FlatSnapshot::parse(&old.readable_flat_json().unwrap()).unwrap();
    assert_eq!(old_flat.directories[0].empty_directories,vec!["empty"]);
    assert_eq!(old_flat.directories[0].files.iter().map(|f|f.path.as_str()).collect::<Vec<_>>(),
        vec!["changed/b.txt","unchanged/a.txt","zero"]);
    let beta=|rows:&proj::cas_tree::VerifiedSnapshot|rows.directories[0].files.iter()
        .find(|f|f.path=="changed/b.txt").unwrap().hash.clone();
    assert_eq!(beta(&old),blake3::hash(b"beta").to_hex().as_str());
    assert_eq!(beta(&current),blake3::hash(b"beta changed").to_hex().as_str());
}

#[test]
fn metadata_location_and_pretty_spacing_do_not_change_content_or_snapshot_ids() {
    let fixture=Fixture::new();let project_metadata=fixture.project.join(".proj/cas-metadata");
    let project_root=fixture.ingest(&project_metadata);let snapshot=fixture.snapshot(&project_root,"v1.2.3");
    let id=fixture.publish(&project_metadata,&snapshot);
    assert!(!fixture.store.join("trees").exists());
    assert!(cas_tree::verify_snapshot(&fixture.store,&project_metadata,&id).is_ok());
    let missing=cas_tree::verify_snapshot(&fixture.store,&fixture.store,&id).unwrap_err();
    assert!(format!("{missing:#}").contains("Missing CAS snapshot reference"));
    let remote_root=fixture.ingest(&fixture.store);assert_eq!(project_root,remote_root);
    assert_eq!(fixture.publish(&fixture.store,&snapshot),id);
    let compact=serde_json::to_vec(&snapshot).unwrap();
    let path=snapshot_path(&fixture.store,&id);fs::write(&path,&compact).unwrap();
    assert_eq!(fixture.publish(&fixture.store,&snapshot),id);
    assert_eq!(fs::read(&path).unwrap(),compact);
    let root=trees(&fixture.store,&remote_root);let compact_tree=serde_json::to_vec(&root).unwrap();
    fs::write(tree_path(&fixture.store,&remote_root),&compact_tree).unwrap();
    assert_eq!(fixture.ingest(&fixture.store),remote_root);
    assert_eq!(fs::read(tree_path(&fixture.store,&remote_root)).unwrap(),compact_tree);
}

#[test]
fn deep_verification_distinguishes_missing_malformed_and_corrupt_and_never_repairs_them() {
    let fixture=Fixture::new();let root=fixture.ingest(&fixture.store);
    let id=fixture.publish(&fixture.store,&fixture.snapshot(&root,"v1.2.3"));
    let object=object_path(&fixture.store,b"alpha");let original=fs::read(&object).unwrap();
    fs::write(&object,b"corrupt").unwrap();
    let error=cas_tree::verify_snapshot(&fixture.store,&fixture.store,&id).unwrap_err();
    assert!(format!("{error:#}").contains("Corrupt CAS file-object bytes"));
    let count=files(fixture.store.join("tree-snapshots/v1").as_std_path()).len();
    assert!(cas_tree::publish_snapshot(&fixture.project,&fixture.store,&fixture.store,
        &fixture.snapshot(&root,"v1.2.4")).is_err());
    assert_eq!(files(fixture.store.join("tree-snapshots/v1").as_std_path()).len(),count);
    assert_eq!(fs::read(&object).unwrap(),b"corrupt");
    fs::remove_file(&object).unwrap();
    let error=cas_tree::verify_snapshot(&fixture.store,&fixture.store,&id).unwrap_err();
    assert!(format!("{error:#}").contains("Missing CAS file object"));
    fs::write(&object,&original).unwrap();

    let path=tree_path(&fixture.store,&root);let bytes=fs::read(&path).unwrap();
    fs::write(&path,b"{}").unwrap();
    let error=cas_tree::verify_snapshot(&fixture.store,&fixture.store,&id).unwrap_err();
    assert!(format!("{error:#}").contains("Malformed CAS directory object"));
    fs::write(&path,&bytes).unwrap();
    let mut changed=trees(&fixture.store,&root);changed.entries[0].hash="f".repeat(64);
    fs::write(&path,changed.readable_json().unwrap()).unwrap();
    let error=cas_tree::verify_snapshot(&fixture.store,&fixture.store,&id).unwrap_err();
    assert!(format!("{error:#}").contains("Corrupt CAS directory-object identity"));
    fs::remove_file(&path).unwrap();
    let error=cas_tree::verify_snapshot(&fixture.store,&fixture.store,&id).unwrap_err();
    assert!(format!("{error:#}").contains("Missing CAS directory object"));
    fs::write(&path,&bytes).unwrap();
    let reference=snapshot_path(&fixture.store,&id);let reference_bytes=fs::read(&reference).unwrap();
    fs::write(&reference,fixture.snapshot(&root,"different metadata").readable_json().unwrap()).unwrap();
    let error=cas_tree::verify_snapshot(&fixture.store,&fixture.store,&id).unwrap_err();
    assert!(format!("{error:#}").contains("Corrupt CAS snapshot-reference identity"));
    fs::write(&reference,&reference_bytes).unwrap();
    assert!(cas_tree::verify_snapshot(&fixture.store,&fixture.store,&id).is_ok());
}

#[test]
fn absent_authorities_case_aliases_and_source_namespace_overlap_fail_before_writes() {
    let fixture=Fixture::new();
    for relative in ["VERSION","version","ver\u{17f}ion",".g\u{131}t","DESCRIPTION","_proj.yml","_PROJR-Future.YAML",
        "_quarto.yml.local","manifest.csv",".hg",".agents",".proj/log",".PROJ/LOG",".proj/BUILD.LOCK"] {
        let metadata=fixture.project.join(relative);
        let error=cas_tree::ingest_directory(&fixture.project,&fixture.store,&metadata,&fixture.source).unwrap_err();
        assert!(!format!("{error:#}").is_empty());
        assert!(!metadata.exists(),"{relative}");
        assert!(!fixture.store.exists(),"{relative}");
    }
    for (content,metadata) in [(fixture.source.join("archive"),fixture.store.clone()),
        (fixture.store.clone(),fixture.source.join("metadata")),
        (fixture.store.clone(),fixture.store.join("objects")),
        (fixture.project.join(".git/archive"),fixture.store.clone())] {
        assert!(cas_tree::ingest_directory(&fixture.project,&content,&metadata,&fixture.source).is_err());
        assert!(!content.exists());assert!(!metadata.exists());
    }
    assert!(cas_tree::ingest_directory(&fixture.project,&fixture.store,&fixture.store,&fixture.project).is_err());
    assert!(!fixture.store.exists());
    assert_eq!(fs::read(fixture.source.join("unchanged/a.txt")).unwrap(),b"alpha");
}

#[test]
fn concurrent_identical_snapshot_publication_is_idempotent_and_references_are_immutable() {
    let fixture=Fixture::new();let root=fixture.ingest(&fixture.store);
    let snapshot=fixture.snapshot(&root,"v1.2.3");
    let ids=std::thread::scope(|scope| {
        let handles:Vec<_>=(0..8).map(|_|scope.spawn(||cas_tree::publish_snapshot(
            &fixture.project,&fixture.store,&fixture.store,&snapshot).unwrap())).collect();
        handles.into_iter().map(|handle|handle.join().unwrap()).collect::<Vec<_>>()
    });
    assert!(ids.iter().all(|id|id==&ids[0]));
    assert_eq!(files(fixture.store.join("tree-snapshots/v1").as_std_path()).len(),1);
    let path=snapshot_path(&fixture.store,&ids[0]);let original=fs::read(&path).unwrap();
    fs::write(&path,b"not metadata").unwrap();
    assert!(cas_tree::publish_snapshot(&fixture.project,&fixture.store,&fixture.store,&snapshot).is_err());
    assert_eq!(fs::read(&path).unwrap(),b"not metadata");
    fs::write(&path,&original).unwrap();
    assert_eq!(fixture.publish(&fixture.store,&snapshot),ids[0]);
}

#[test]
fn graph_expansion_and_depth_are_bounded_before_any_successful_reference() {
    for deep in [false,true] {
        let fixture=Fixture::new();
        let mut id=write_tree(&fixture.store,&DirectoryTree::new(vec![]).unwrap());
        for _ in 0..if deep {65}else{20} {
            let entries=if deep {vec![TreeEntry {name:"child".into(),kind:EntryKind::Tree,hash:id.clone()}]}
                else {["a","b"].iter().map(|name|TreeEntry {name:(*name).into(),kind:EntryKind::Tree,hash:id.clone()}).collect()};
            id=write_tree(&fixture.store,&DirectoryTree::new(entries).unwrap());
        }
        let error=cas_tree::publish_snapshot(&fixture.project,&fixture.store,&fixture.store,&fixture.snapshot(&id,"v1")).unwrap_err();
        let text=format!("{error:#}");
        assert!(text.contains(if deep {"depth"}else{"expanded"}),"{text}");
        assert!(!fixture.store.join("tree-snapshots").exists());
    }
}

#[cfg(unix)]
#[test]
fn links_and_special_sources_or_objects_fail_without_external_mutation() {
    use std::os::unix::fs::symlink;
    let fixture=Fixture::new();
    let outside=fixture.project.parent().unwrap().join("external");
    fs::write(&outside,b"external").unwrap();
    symlink(&outside,fixture.source.join("linked")).unwrap();
    assert!(cas_tree::ingest_directory(&fixture.project,&fixture.store,&fixture.store,&fixture.source).is_err());
    assert!(!fixture.store.exists());fs::remove_file(fixture.source.join("linked")).unwrap();
    let status=std::process::Command::new("mkfifo").arg(fixture.source.join("pipe")).status().unwrap();
    assert!(status.success());
    assert!(cas_tree::ingest_directory(&fixture.project,&fixture.store,&fixture.store,&fixture.source).is_err());
    assert!(!fixture.store.exists());fs::remove_file(fixture.source.join("pipe")).unwrap();
    let root=fixture.ingest(&fixture.store);let id=fixture.publish(&fixture.store,&fixture.snapshot(&root,"v1"));
    let object=object_path(&fixture.store,b"alpha");fs::remove_file(&object).unwrap();symlink(&outside,&object).unwrap();
    assert!(cas_tree::verify_snapshot(&fixture.store,&fixture.store,&id).is_err());
    assert_eq!(fs::read(&outside).unwrap(),b"external");
}

#[test]
fn native_hash_prefix_alias_is_rejected_by_both_ingestion_and_verification() {
    let fixture=Fixture::new();
    fs::remove_dir_all(&fixture.source).unwrap();fs::create_dir(&fixture.source).unwrap();
    let bytes=(0..1000).map(|n|format!("native-alias-{n}").into_bytes())
        .find(|bytes|blake3::hash(bytes).to_hex().as_str()[..2].bytes().any(|b|b.is_ascii_alphabetic())).unwrap();
    let digest=blake3::hash(&bytes).to_hex().to_string();
    let lower=fixture.store.join("objects").join(&digest[..2]);
    let upper=fixture.store.join("objects").join(digest[..2].to_ascii_uppercase());
    fs::create_dir_all(&upper).unwrap();
    if !lower.exists(){return;} // Native case-sensitive filesystem; the pure planned-path tests still run.
    fs::write(upper.join("keep"),b"preserve native alias").unwrap();
    fs::write(fixture.source.join("file"),&bytes).unwrap();
    assert!(cas_tree::ingest_directory(&fixture.project,&fixture.store,&fixture.store,&fixture.source).is_err());
    assert_eq!(fs::read(upper.join("keep")).unwrap(),b"preserve native alias");
    assert_eq!(files(fixture.store.join("objects").as_std_path()).len(),1);
    assert!(!fixture.store.join("tree-snapshots").exists());
    // A manually supplied complete reference must not make the reader accept the alias either.
    fs::write(upper.join(&digest[2..]),&bytes).unwrap();
    let tree=DirectoryTree::new(vec![TreeEntry {name:"file".into(),kind:EntryKind::File,hash:digest}]).unwrap();
    let root=write_tree(&fixture.store,&tree);let snapshot=fixture.snapshot(&root,"v1");
    let id=snapshot.identity().unwrap();let path=snapshot_path(&fixture.store,&id);
    fs::create_dir_all(path.parent().unwrap()).unwrap();fs::write(path,snapshot.readable_json().unwrap()).unwrap();
    let error=cas_tree::verify_snapshot(&fixture.store,&fixture.store,&id).unwrap_err();
    assert!(format!("{error:#}").contains("native filesystem alias"));
    assert_eq!(fs::read(upper.join("keep")).unwrap(),b"preserve native alias");
}

#[test]
fn new_objects_coexist_with_unchanged_legacy_flat_cas_and_lookup_ledger() {
    let fixture=Fixture::new();
    proj::cas::ingest_directory(&fixture.project,&fixture.store,"output",&fixture.source,"v1.2.3").unwrap();
    let ledger=fs::read(fixture.project.join(".projr/manifests.csv")).unwrap();
    let old=files(fixture.store.join("manifests").as_std_path());
    assert_eq!(old.len(),1);let old_bytes=fs::read(&old[0]).unwrap();
    let legacy:proj::DirectoryManifest=serde_json::from_slice(&old_bytes).unwrap();
    assert_eq!(legacy.label,"output");assert_eq!(legacy.version,"v1.2.3");
    assert_eq!(old[0].file_stem().unwrap().to_str().unwrap(),blake3::hash(&old_bytes).to_hex().as_str());
    let root=fixture.ingest(&fixture.store);
    let id=fixture.publish(&fixture.store,&fixture.snapshot(&root,"v1.2.4"));
    assert!(cas_tree::verify_snapshot(&fixture.store,&fixture.store,&id).is_ok());
    assert_eq!(fs::read(fixture.project.join(".projr/manifests.csv")).unwrap(),ledger);
    assert_eq!(fs::read(&old[0]).unwrap(),old_bytes);
    assert_eq!(files(fixture.store.join("manifests").as_std_path()),old);
}
