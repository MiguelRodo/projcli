//! Complete CAS stores, not ordinary archive ZIPs. Synthetic loopback transport only.
use std::{collections::BTreeMap,fs};
use camino::{Utf8Path,Utf8PathBuf};
use proj::snapshot_format::{TreeSnapshot,DirectoryRoot};
#[path="common/xet_github_mock.rs"] mod mock;
use mock::Server;
fn path(value:impl AsRef<std::path::Path>)->Utf8PathBuf {
    Utf8PathBuf::from_path_buf(value.as_ref().to_owned()).unwrap()
}
fn publish(project:&Utf8Path,store:&Utf8Path,source:&Utf8Path,version:&str)->String {
    let tree=proj::cas_tree::ingest_directory(project,store,store,source).unwrap();
    let snapshot=TreeSnapshot::new(version.into(),None,vec![DirectoryRoot {label:"raw-data".into(),tree}]).unwrap();
    proj::cas_tree::publish_snapshot(project,store,store,&snapshot).unwrap()
}
fn files(root:&Utf8Path)->BTreeMap<String,Vec<u8>> {
    fn visit(root:&Utf8Path,current:&Utf8Path,result:&mut BTreeMap<String,Vec<u8>>) {
        for entry in fs::read_dir(current).unwrap() {
            let entry=entry.unwrap();let item=path(entry.path());
            if entry.file_type().unwrap().is_dir(){visit(root,&item,result)}else{
                result.insert(item.strip_prefix(root).unwrap().as_str().into(),fs::read(item).unwrap());
            }
        }
    }let mut result=BTreeMap::new();visit(root,root,&mut result);result
}
fn fixture()->(tempfile::TempDir,Utf8PathBuf,Utf8PathBuf,Utf8PathBuf) {
    let temp=tempfile::tempdir().unwrap();let root=path(temp.path());
    let project=root.join("project");let source=project.join("raw");let store=root.join("store");
    fs::create_dir_all(source.join("empty")).unwrap();fs::create_dir(source.join("nested")).unwrap();
    fs::write(source.join("nested/data.csv"),b"first bytes\n").unwrap();
    fs::write(source.join("caf\u{e9},file.txt"),b"shared unchanged bytes\n").unwrap();
    (temp,project,source,store)
}
#[test]
fn complete_cas_export_fresh_import_deep_corruption_and_reexport() {
    let (temp,project,source,store)=fixture();let root=path(temp.path());
    let first=publish(&project,&store,&source,"v1.0.0");
    fs::write(source.join("nested/data.csv"),b"second bytes\n").unwrap();
    let second=publish(&project,&store,&source,"v1.0.1");
    let before=files(&store);let exported=proj::cas_archive::export(&project,&store).unwrap();
    assert_eq!(files(&store),before);
    assert_eq!(exported.snapshots.len(),2);
    let archive=root.join("portable.zip");fs::copy(exported.path(),&archive).unwrap();
    let hash=exported.hash.clone();let length=exported.length;drop(exported);
    fs::remove_dir_all(&source).unwrap();fs::remove_dir_all(&store).unwrap();
    let imported=proj::cas_archive::import(&archive,&hash,length,4*1024*1024).unwrap();
    let destination=project.join("restored");
    for (id,expected) in [(&first,b"first bytes\n".as_slice()),(&second,b"second bytes\n".as_slice())] {
        proj::cas_tree::restore_label(&project,imported.path(),imported.path(),id,"raw-data",&destination,true).unwrap();
        assert_eq!(fs::read(destination.join("nested/data.csv")).unwrap(),expected);assert!(destination.join("empty").is_dir());
    }
    fs::create_dir_all(&source).unwrap();fs::write(source.join("new.txt"),b"third").unwrap();
    let third=publish(&project,imported.path(),&source,"v1.0.2");
    let again=proj::cas_archive::export(&project,imported.path()).unwrap();
    let fresh=proj::cas_archive::import(again.path(),&again.hash,again.length,4*1024*1024).unwrap();
    assert!(fresh.snapshots().contains_key(&first)&&fresh.snapshots().contains_key(&second)&&fresh.snapshots().contains_key(&third));
    assert!(proj::cas_archive::import(&archive,&hash,length,1).is_err());
    let expanded=proj::github_zip::extract_zip(&archive,4*1024*1024).unwrap();let tree=path(expanded.path());
    let metadata=tree.join("cas-export.json");let mut manifest:serde_json::Value=serde_json::from_slice(&fs::read(&metadata).unwrap()).unwrap();
    let object=manifest["files"].as_object().unwrap().keys().find(|key|key.starts_with("objects/")).unwrap().clone();
    let object_path=tree.join(&object);
    #[cfg(unix)] {use std::os::unix::fs::PermissionsExt;fs::set_permissions(&object_path,fs::Permissions::from_mode(0o600)).unwrap();}
    #[cfg(windows)] {let mut permissions=fs::metadata(&object_path).unwrap().permissions();permissions.set_readonly(false);fs::set_permissions(&object_path,permissions).unwrap();}
    let mut bytes=fs::read(&object_path).unwrap();bytes[0]^=0x80;fs::write(&object_path,bytes).unwrap();
    manifest["files"][&object]["hash"]=serde_json::Value::String(proj::cas::hash_file(&object_path).unwrap());
    fs::write(&metadata,serde_json::to_vec(&manifest).unwrap()).unwrap();
    let (forged,_)=proj::github_zip::directory_zip(&tree).unwrap();let forged_path=path(forged.path());
    let error=proj::cas_archive::import(&forged_path,&proj::cas::hash_file(&forged_path).unwrap(),
        forged.as_file().metadata().unwrap().len(),4*1024*1024).err().expect("object-address corruption must fail");
    assert!(format!("{error:#}").contains("CAS object digest mismatch"));
    assert_eq!(fs::read(destination.join("nested/data.csv")).unwrap(),b"second bytes\n");
}
#[test]
fn actual_cas_store_mock_github_preserves_old_export_and_restores_all_versions() {
    let (_temp,project,source,store)=fixture();
    let first=publish(&project,&store,&source,"v1.0.0");
    let server=Server::start();
    let client=proj::github_release::GithubReleaseClient::new("owner/repo",&server.base,Some("fixture-token".into())).unwrap();
    let reference=proj::cas_github::upload(&project,&store,&client,"data",true,4*1024*1024).unwrap();
    let previous=server.state.lock().unwrap().assets.clone();
    fs::write(source.join("nested/data.csv"),b"second bytes\n").unwrap();let second=publish(&project,&store,&source,"v1.0.1");
    server.state.lock().unwrap().fail_cas=true;
    assert!(proj::cas_github::upload(&project,&store,&client,"data",true,4*1024*1024).is_err());
    assert_eq!(server.state.lock().unwrap().assets,previous);
    {let mut state=server.state.lock().unwrap();state.fail_cas=false;state.corrupt=true;}
    assert!(proj::cas_github::upload(&project,&store,&client,"data",true,4*1024*1024).is_err());
    {let state=server.state.lock().unwrap();assert!(state.deleted.is_empty());for (name,value) in &previous{assert_eq!(&state.assets[name],value);}}
    server.state.lock().unwrap().corrupt=false;
    let updated=proj::cas_github::upload(&project,&store,&client,"data",true,4*1024*1024).unwrap();
    assert_eq!(updated.structure,"cas");assert!(updated.snapshots.contains_key(&first)&&updated.snapshots.contains_key(&second));
    fs::remove_dir_all(&source).unwrap();fs::remove_dir_all(&store).unwrap();
    let imported=proj::cas_github::download(&project,&client,&updated,4*1024*1024,4*1024*1024).unwrap();
    let destination=project.join("restored");
    proj::cas_tree::restore_label(&project,imported.path(),imported.path(),&second,"raw-data",&destination,true).unwrap();
    assert_eq!(fs::read(destination.join("nested/data.csv")).unwrap(),b"second bytes\n");
    let old=proj::cas_github::download(&project,&client,&reference,4*1024*1024,4*1024*1024).unwrap();
    proj::cas_tree::restore_label(&project,old.path(),old.path(),&first,"raw-data",&destination,true).unwrap();
    assert_eq!(fs::read(destination.join("nested/data.csv")).unwrap(),b"first bytes\n");
    assert!(server.state.lock().unwrap().deleted.is_empty());server.finish();
}

#[test]
fn complete_cas_flat_all_modes_and_legacy_survive_export_without_project_ledger() {
    use proj::{catalogue::{Catalogue,Format,Inventory},snapshot_format::FlatSnapshot};
    for (format,filename) in [(Format::Csv,"manifest.csv"),(Format::Json,"manifest.json"),
        (Format::CsvDedupeFile,"manifest-dedupe-file.csv"),(Format::JsonDedupeFile,"manifest-dedupe-file.json")] {
        let (_temp,project,source,store)=fixture();let mut ids=Vec::new();
        for version in ["v1.0.0","v1.0.1"] {
            let directory=proj::cas_tree::ingest_flat_directory(&project,&store,&source,"raw-data").unwrap();
            let snapshot=FlatSnapshot::new(version.into(),None,vec![directory]).unwrap();
            ids.push(proj::cas_flat::publish_snapshot(&project,&store,std::slice::from_ref(&store),None,&snapshot,format).unwrap());
        }
        let history=Catalogue::parse(&fs::read(store.join("catalogues/v1").join(filename)).unwrap(),format).unwrap();
        assert!(matches!(&history.records[1].inventory,Inventory::Reference {ref_version} if ref_version=="v1.0.0"));
        let exported=proj::cas_archive::export(&project,&store).unwrap();
        fs::remove_dir_all(&store).unwrap();fs::remove_dir_all(&source).unwrap();
        let imported=proj::cas_archive::import(exported.path(),&exported.hash,exported.length,4*1024*1024).unwrap();
        let destination=project.join("restored");
        for id in &ids {
            assert_eq!(imported.snapshots()[id],proj::cas_archive::Kind::Flat);
            proj::cas_flat::restore_label(&project,imported.path(),imported.path(),id,"raw-data",&destination,true).unwrap();
            assert_eq!(fs::read(destination.join("nested/data.csv")).unwrap(),b"first bytes\n");
            assert!(destination.join("empty").is_dir());
        }
    }
    let (_temp,project,source,store)=fixture();
    proj::cas::ingest_directory(&project,&store,"raw-data",&source,"v1.0.0").unwrap();
    let leaf=fs::read_dir(store.join("manifests")).unwrap().next().unwrap().unwrap().path();
    let id=leaf.file_stem().unwrap().to_str().unwrap().to_owned();
    let exported=proj::cas_archive::export(&project,&store).unwrap();
    fs::remove_dir_all(&store).unwrap();fs::remove_dir_all(&source).unwrap();fs::remove_dir_all(project.join(".projr")).unwrap();
    let imported=proj::cas_archive::import(exported.path(),&exported.hash,exported.length,4*1024*1024).unwrap();
    assert_eq!(imported.snapshots()[&id],proj::cas_archive::Kind::Legacy);
    let destination=project.join("restored");
    proj::cas_tree::restore_legacy_label(&project,imported.path(),&id,"raw-data",&destination,true).unwrap();
    assert_eq!(fs::read(destination.join("nested/data.csv")).unwrap(),b"first bytes\n");
}
#[test]
fn cas_export_busy_claim_relative_paths_unknown_metadata_and_missing_objects_fail() {
    let (_temp,project,source,store)=fixture();publish(&project,&store,&source,"v1.0.0");
    assert!(proj::cas_archive::export(&project,Utf8Path::new("store")).is_err());
    fs::create_dir_all(store.join(".proj")).unwrap();fs::write(store.join(".proj/build.lock"),b"other writer").unwrap();
    assert!(proj::cas_archive::export(&project,&store).is_err());
    assert_eq!(fs::read(store.join(".proj/build.lock")).unwrap(),b"other writer");fs::remove_file(store.join(".proj/build.lock")).unwrap();
    fs::create_dir_all(store.join("catalogues/v1")).unwrap();fs::write(store.join("catalogues/v1/unknown.json"),b"{}").unwrap();
    assert!(proj::cas_archive::export(&project,&store).is_err());fs::remove_file(store.join("catalogues/v1/unknown.json")).unwrap();
    let shard=fs::read_dir(store.join("objects")).unwrap().next().unwrap().unwrap().path();
    let object=fs::read_dir(shard).unwrap().next().unwrap().unwrap().path();fs::remove_file(object).unwrap();
    assert!(proj::cas_archive::export(&project,&store).is_err());
}
