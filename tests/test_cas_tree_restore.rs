//! Real private filesystem recovery; no live stores, credentials or process environment changes.
use std::{fs,collections::BTreeMap};
use camino::{Utf8Path,Utf8PathBuf};
use proj::{cas_tree,snapshot_format::{TreeSnapshot,DirectoryRoot}};
struct Fixture {
    _temp:tempfile::TempDir,project:Utf8PathBuf,store:Utf8PathBuf,id:String,
}
fn bytes(root:&Utf8Path)->BTreeMap<String,Vec<u8>> {
    fn visit(root:&Utf8Path,path:&Utf8Path,rows:&mut BTreeMap<String,Vec<u8>>) {
        for item in fs::read_dir(path).unwrap() {
            let item=item.unwrap();let path=Utf8PathBuf::try_from(item.path()).unwrap();
            let kind=item.file_type().unwrap();
            if kind.is_dir(){visit(root,&path,rows);}
            else if kind.is_file(){rows.insert(path.strip_prefix(root).unwrap().to_string(),fs::read(path).unwrap());}
        }
    }
    let mut rows=BTreeMap::new();if root.exists(){visit(root,root,&mut rows);}rows
}
impl Fixture {
    fn new(empty:bool,project_metadata:bool)->(Self,Utf8PathBuf) {
        let temp=tempfile::tempdir().unwrap();
        let parent=Utf8PathBuf::try_from(temp.path().canonicalize().unwrap()).unwrap();
        let project=parent.join("project");let source=project.join("source");let store=parent.join("store");
        fs::create_dir_all(&source).unwrap();
        if !empty {
            fs::create_dir_all(source.join("nested/empty")).unwrap();
            fs::write(source.join("nested/caf\u{e9}.txt"),b"exact nested result\n").unwrap();
            fs::write(source.join("zero"),b"").unwrap();
        }
        let metadata=if project_metadata {project.join(".proj/cas-metadata")}else{store.clone()};
        let tree=cas_tree::ingest_directory(&project,&store,&metadata,&source).unwrap();
        let snapshot=TreeSnapshot::new("v1.2.3".into(),None,vec![DirectoryRoot {label:"output".into(),tree}]).unwrap();
        let id=cas_tree::publish_snapshot(&project,&store,&metadata,&snapshot).unwrap();
        (Self {_temp:temp,project,store,id},metadata)
    }
    fn restore(&self,metadata:&Utf8Path,destination:&Utf8Path,clear:bool)->anyhow::Result<cas_tree::TreeRestore> {
        cas_tree::restore_label(&self.project,&self.store,metadata,&self.id,"output",destination,clear)
    }
}
fn object(store:&Utf8Path,content:&[u8])->Utf8PathBuf {
    let hash=blake3::hash(content).to_hex().to_string();store.join("objects").join(&hash[..2]).join(&hash[2..])
}

#[test]
fn selected_remote_snapshot_restores_exact_bytes_after_the_working_project_is_lost() {
    let (fixture,metadata)=Fixture::new(false,false);let before=bytes(&fixture.store);
    fs::remove_dir_all(&fixture.project).unwrap(); // Only the fixture-owned temporary working copy.
    fs::create_dir(&fixture.project).unwrap();
    fs::write(fixture.project.join("VERSION"),b"v9.0.0.7\n").unwrap();
    let destination=fixture.project.join("recovered");
    let result=fixture.restore(&metadata,&destination,true).unwrap();
    assert_eq!(result.snapshot_id,fixture.id);assert_eq!(result.label,"output");
    assert_eq!(result.version,"v1.2.3");assert_eq!(result.files,2);assert!(result.clear);
    assert_eq!(bytes(&destination),BTreeMap::from([
        ("nested/caf\u{e9}.txt".into(),b"exact nested result\n".to_vec()),("zero".into(),vec![])]));
    assert!(destination.join("nested/empty").is_dir());
    assert_eq!(fs::read(fixture.project.join("VERSION")).unwrap(),b"v9.0.0.7\n");
    assert!(!fixture.project.join(".proj").exists());assert!(!fixture.project.join(".projr").exists());
    assert_eq!(bytes(&fixture.store),before);
    assert_eq!(fs::read_dir(&fixture.project).unwrap().count(),2);
}

#[test]
fn merge_and_clear_are_explicit_and_matching_hardlinked_files_do_not_mutate_external_aliases() {
    let (fixture,metadata)=Fixture::new(false,false);let before=bytes(&fixture.store);
    let destination=fixture.project.join("recovered");fs::create_dir_all(destination.join("nested/empty")).unwrap();
    let outside=fixture.project.parent().unwrap().join("outside");fs::write(&outside,b"external original").unwrap();
    fs::hard_link(&outside,destination.join("nested/caf\u{e9}.txt")).unwrap();
    fs::write(destination.join("extra"),b"keep unless clear").unwrap();
    fs::write(destination.join("nested/empty/retained"),b"merge keeps this").unwrap();
    let result=fixture.restore(&metadata,&destination,false).unwrap();assert!(!result.clear);
    assert_eq!(fs::read(destination.join("extra")).unwrap(),b"keep unless clear");
    assert_eq!(fs::read(destination.join("nested/empty/retained")).unwrap(),b"merge keeps this");
    assert_eq!(fs::read(destination.join("nested/caf\u{e9}.txt")).unwrap(),b"exact nested result\n");
    assert_eq!(fs::read(&outside).unwrap(),b"external original");
    fixture.restore(&metadata,&destination,true).unwrap();
    assert!(!destination.join("extra").exists());assert!(destination.join("nested/empty").is_dir());
    assert!(fs::read_dir(destination.join("nested/empty")).unwrap().next().is_none());
    assert_eq!(fs::read(&outside).unwrap(),b"external original");
    assert_eq!(bytes(&fixture.store),before);
}

#[test]
fn missing_corrupt_or_unknown_selection_preserves_the_complete_existing_destination() {
    let (fixture,metadata)=Fixture::new(false,false);
    let destination=fixture.project.join("recovered");fs::create_dir(&destination).unwrap();
    fs::write(destination.join("previous"),b"untouched").unwrap();
    let baseline=bytes(&fixture.project);
    let blob=object(&fixture.store,b"exact nested result\n");
    let original=fs::read(&blob).unwrap();fs::write(&blob,b"corrupt").unwrap();
    assert!(fixture.restore(&metadata,&destination,true).is_err());
    assert_eq!(bytes(&fixture.project),baseline);
    fs::remove_file(&blob).unwrap();assert!(fixture.restore(&metadata,&destination,true).is_err());
    assert_eq!(bytes(&fixture.project),baseline);fs::write(&blob,&original).unwrap();
    for (id,label) in [("0".repeat(64),"output"),(fixture.id.clone(),"absent")] {
        assert!(cas_tree::restore_label(&fixture.project,&fixture.store,&metadata,&id,label,&destination,true).is_err());
        assert_eq!(bytes(&fixture.project),baseline);
    }
    assert_eq!(fs::read_dir(&fixture.project).unwrap().count(),2);
}

#[test]
fn destination_controls_and_store_overlap_fail_before_any_restore_writes() {
    let (fixture,metadata)=Fixture::new(false,false);
    let project_before=bytes(&fixture.project);let store_before=bytes(&fixture.store);
    for destination in [fixture.project.clone(),fixture.project.join("VERSION"),fixture.project.join("version"),
        fixture.project.join("ver\u{17f}ion"),fixture.project.join("manifest.csv"),fixture.project.join(".proj"),
        fixture.project.join(".git"),fixture.project.join("_proj.yml"),fixture.store.join("objects"),
        fixture.store.join("trees/v1"),fixture.store.join("tree-snapshots/v1"),
        fixture.project.join("missing-parent/output")] {
        assert!(fixture.restore(&metadata,&destination,true).is_err(),"{destination}");
        assert_eq!(bytes(&fixture.project),project_before);assert_eq!(bytes(&fixture.store),store_before);
    }
    assert!(fixture.restore(&metadata,Utf8Path::new("relative"),true).is_err());
}

#[test]
fn empty_snapshot_without_any_file_objects_and_project_side_metadata_can_restore() {
    for project_metadata in [false,true] {
        let (fixture,metadata)=Fixture::new(true,project_metadata);
        assert!(!fixture.store.join("objects").exists());
        let destination=fixture.project.join("empty-output");fs::create_dir(&destination).unwrap();
        fs::write(destination.join("previous"),b"preserve in merge mode").unwrap();
        let metadata_before=bytes(&metadata);
        fixture.restore(&metadata,&destination,false).unwrap();assert!(destination.join("previous").exists());
        let result=fixture.restore(&metadata,&destination,true).unwrap();assert_eq!(result.files,0);
        assert!(fs::read_dir(destination).unwrap().next().is_none());
        assert_eq!(bytes(&metadata),metadata_before);assert!(!fixture.store.join("objects").exists());
    }
}

#[cfg(unix)]
#[test]
fn linked_and_special_existing_destination_entries_fail_without_external_mutation() {
    use std::os::unix::fs::symlink;
    let (fixture,metadata)=Fixture::new(false,false);
    let outside=fixture.project.parent().unwrap().join("outside");fs::write(&outside,b"external").unwrap();
    let destination=fixture.project.join("recovered");fs::create_dir(&destination).unwrap();
    symlink(&outside,destination.join("link")).unwrap();
    assert!(fixture.restore(&metadata,&destination,false).is_err());
    assert!(fs::symlink_metadata(destination.join("link")).unwrap().file_type().is_symlink());
    assert_eq!(fs::read(&outside).unwrap(),b"external");
    fs::remove_file(destination.join("link")).unwrap();
    assert!(std::process::Command::new("mkfifo").arg(destination.join("pipe")).status().unwrap().success());
    assert!(fixture.restore(&metadata,&destination,false).is_err());
    assert_eq!(fs::read(&outside).unwrap(),b"external");
    assert_eq!(fs::read_dir(&destination).unwrap().count(),1);
}

#[cfg(unix)]
#[test]
fn staging_creation_failure_preserves_destination_and_store() {
    use std::os::unix::fs::PermissionsExt;
    let (fixture,metadata)=Fixture::new(false,false);
    let parent=fixture.project.join("parent");fs::create_dir(&parent).unwrap();
    let destination=parent.join("recovered");fs::create_dir(&destination).unwrap();
    fs::write(destination.join("previous"),b"untouched").unwrap();
    let before=bytes(&fixture.store);
    fs::set_permissions(&parent,fs::Permissions::from_mode(0o555)).unwrap();
    let result=fixture.restore(&metadata,&destination,true);
    fs::set_permissions(&parent,fs::Permissions::from_mode(0o755)).unwrap();
    // Root-like runners can bypass mode restrictions. Require failure/preservation otherwise.
    if result.is_err() {
        assert_eq!(fs::read(destination.join("previous")).unwrap(),b"untouched");
        assert_eq!(fs::read_dir(&parent).unwrap().count(),1);
    } else {
        assert_eq!(fs::read(destination.join("nested/caf\u{e9}.txt")).unwrap(),b"exact nested result\n");
    }
    assert_eq!(bytes(&fixture.store),before);
}


#[test]
fn legacy_flat_manifests_and_lookup_ledger_survive_rejected_and_normal_tree_restore() {
    let (fixture,metadata)=Fixture::new(false,false);
    proj::cas::ingest_directory(&fixture.project,&fixture.store,"output",&fixture.project.join("source"),"v1.2.3").unwrap();
    let before=bytes(&fixture.store);let ledger=fs::read(fixture.project.join(".projr/manifests.csv")).unwrap();
    assert!(fixture.restore(&metadata,&fixture.store.join("manifests"),true).is_err());
    assert_eq!(bytes(&fixture.store),before);
    fixture.restore(&metadata,&fixture.project.join("recovered"),true).unwrap();
    assert_eq!(bytes(&fixture.store),before);
    assert_eq!(fs::read(fixture.project.join(".projr/manifests.csv")).unwrap(),ledger);
}

#[test]
fn selected_empty_directory_alias_never_replaces_or_widens_the_existing_private_directory() {
    let (fixture,metadata)=Fixture::new(false,false);
    let destination=fixture.project.join("recovered");
    fs::create_dir_all(destination.join("nested/Empty")).unwrap();
    fs::write(destination.join("previous"),b"keep private").unwrap();
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(destination.join("nested/Empty"),fs::Permissions::from_mode(0o700)).unwrap();
    }
    let before=bytes(&destination);
    let error=fixture.restore(&metadata,&destination,false).unwrap_err();
    let text=format!("{error:#}");
    assert!(text.contains("alias")||text.contains("directory"),"{text}");
    assert_eq!(bytes(&destination),before);
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(destination.join("nested/Empty")).unwrap().permissions().mode()&0o777,0o700);
    }
    assert_eq!(fs::read_dir(&fixture.project).unwrap().count(),2);
}

#[test]
fn callback_failure_cleans_all_private_copies_without_unlocking_the_original_directory() {
    let (fixture,metadata)=Fixture::new(false,false);
    let destination=fixture.project.join("recovered");
    fs::create_dir_all(destination.join("NESTED")).unwrap();
    fs::create_dir(destination.join("locked")).unwrap();
    fs::write(destination.join("locked/private"),b"private original").unwrap();
    fs::write(destination.join("NESTED/previous"),b"previous").unwrap();
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(destination.join("locked"),fs::Permissions::from_mode(0o500)).unwrap();
    }
    let before=bytes(&destination);let store_before=bytes(&fixture.store);
    assert!(fixture.restore(&metadata,&destination,false).is_err());
    assert_eq!(bytes(&destination),before);assert_eq!(bytes(&fixture.store),store_before);
    // On case-sensitive filesystems the copied stage has portable aliases; on
    // case-insensitive filesystems selected paths retain the wrong native case.
    // Both fail after copying the unrelated restrictive private directory.
    assert_eq!(fs::read_dir(&fixture.project).unwrap().count(),2);
    assert!(!fs::read_dir(&fixture.project).unwrap().any(|entry|
        entry.unwrap().file_name().to_string_lossy().starts_with(".proj-")));
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        let path=destination.join("locked");
        let original=fs::metadata(&path).unwrap().permissions().mode()&0o777;
        // Restore only our fixture's original permissions for portable TempDir cleanup.
        fs::set_permissions(&path,fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(original,0o500);
    }
}
