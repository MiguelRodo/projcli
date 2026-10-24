//! Owned legacy CAS fixtures: no live stores, credentials or global environment changes.
use std::{fs,collections::{BTreeMap,BTreeSet},sync::{Arc,Barrier}};
use camino::{Utf8Path,Utf8PathBuf};
struct Fixture {_owned:tempfile::TempDir,project:Utf8PathBuf,source:Utf8PathBuf,store:Utf8PathBuf}
fn fixture()->Fixture {
    let owned=tempfile::tempdir().unwrap();let base=Utf8PathBuf::try_from(owned.path().canonicalize().unwrap()).unwrap();
    let project=base.join("project");let source=project.join("input");fs::create_dir_all(&source).unwrap();
    fs::write(source.join("value"),b"shared bytes").unwrap();
    fs::write(source.join("caf\u{e9},notes"),b"shared bytes").unwrap();fs::write(source.join("zero"),b"").unwrap();
    Fixture {_owned:owned,project,source,store:base.join("store")}
}
fn files(root:&Utf8Path)->BTreeMap<String,Vec<u8>> {
    fn walk(root:&Utf8Path,path:&Utf8Path,output:&mut BTreeMap<String,Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry=entry.unwrap();let path=Utf8PathBuf::try_from(entry.path()).unwrap();
            if entry.file_type().unwrap().is_dir(){walk(root,&path,output)}else{
                assert!(entry.file_type().unwrap().is_file());
                output.insert(path.strip_prefix(root).unwrap().as_str().replace(std::path::MAIN_SEPARATOR,"/"),fs::read(path).unwrap());
            }
        }
    }
    let mut output=BTreeMap::new();if root.exists(){walk(root,root,&mut output)}output
}
fn rows(project:&Utf8Path)->Vec<csv::StringRecord> {
    csv::ReaderBuilder::new().has_headers(false).from_reader(fs::File::open(project.join(".projr/manifests.csv")).unwrap())
        .records().map(Result::unwrap).collect()
}
fn verify_store(store:&Utf8Path,source:&Utf8Path) {
    for entry in fs::read_dir(store.join("manifests")).unwrap() {
        let path=entry.unwrap().path();let bytes=fs::read(&path).unwrap();
        assert_eq!(blake3::hash(&bytes).to_hex().as_str(),path.file_stem().unwrap().to_str().unwrap());
        let manifest:proj::cas::DirectoryManifest=serde_json::from_slice(&bytes).unwrap();
        for file in manifest.files {
            let object=fs::read(store.join("objects").join(&file.hash[..2]).join(&file.hash[2..])).unwrap();
            assert_eq!(blake3::hash(&object).to_hex().as_str(),file.hash);
            // Earlier changed versions are checked through their recorded objects;
            // current-source equality belongs to the first unchanged/retry fixture.
            assert!(source.join(file.path).is_file());
        }
    }
}
#[test]
fn exact_retry_preserves_raw_ids_index_bytes_quoted_rows_and_source_bytes() {
    let fixture=fixture();let f=&fixture;let ledger=f.project.join(".projr/manifests.csv");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    let old=b"older,legacy,opaque,prior\r\n";fs::write(&ledger,old).unwrap();
    let before_source=files(&f.source);
    proj::cas::ingest_directory(&f.project,&f.store,"raw\"quoted",&f.source,"1,2").unwrap();
    let before=files(&f.store);let index=fs::read(&ledger).unwrap();assert!(index.starts_with(old));
    for _ in 0..2 {proj::cas::ingest_directory(&f.project,&f.store,"raw\"quoted",&f.source,"1,2").unwrap();}
    assert_eq!(fs::read(&ledger).unwrap(),index);assert_eq!(files(&f.store),before);assert_eq!(files(&f.source),before_source);
    let rows=rows(&f.project);assert_eq!(rows.len(),2);assert_eq!(&rows[1][0],"1,2");assert_eq!(&rows[1][1],"raw\"quoted");
    verify_store(&f.store,&f.source);assert_eq!(files(&f.store.join("objects")).len(),2);
    assert_eq!(files(&f.store.join("objects")).values().cloned().collect::<BTreeSet<_>>(),before_source.values().cloned().collect::<BTreeSet<_>>());
    assert!(!f.store.join(".proj/build.lock").exists());assert!(!f.project.join(".projr/.proj/build.lock").exists());
}
#[test]
fn changed_same_version_content_retains_legacy_rows_and_new_exact_retry_is_idempotent() {
    let f=fixture();proj::cas::ingest_directory(&f.project,&f.store,"raw",&f.source,"v1.0.0").unwrap();
    let first=files(&f.store.join("manifests"));let prior=fs::read(f.project.join(".projr/manifests.csv")).unwrap();
    fs::write(f.source.join("value"),b"changed bytes").unwrap();
    proj::cas::ingest_directory(&f.project,&f.store,"raw",&f.source,"v1.0.0").unwrap();
    let second=rows(&f.project);assert_eq!(second.len(),2);assert_ne!(second[0][2],second[1][2]);
    assert!(fs::read(f.project.join(".projr/manifests.csv")).unwrap().starts_with(&prior));
    for (path,bytes) in first{assert_eq!(files(&f.store.join("manifests"))[&path],bytes);}
    let index=fs::read(f.project.join(".projr/manifests.csv")).unwrap();
    proj::cas::ingest_directory(&f.project,&f.store,"raw",&f.source,"v1.0.0").unwrap();
    assert_eq!(fs::read(f.project.join(".projr/manifests.csv")).unwrap(),index);verify_store(&f.store,&f.source);
}
#[test]
fn store_or_shared_ledger_claim_blocks_without_new_references_or_owner_changes() {
    for ledger_claim in [false,true] {
        let f=fixture();let root=if ledger_claim{f.project.join(".projr")}else{f.store.clone()};
        let claim=root.join(".proj/build.lock");fs::create_dir_all(claim.parent().unwrap()).unwrap();fs::write(&claim,b"another-owner").unwrap();
        let before=files(&f.project);let stored=files(&f.store);
        let error=proj::cas::ingest_directory(&f.project,&f.store,"raw",&f.source,"v1.0.0").unwrap_err();
        assert!(format!("{error:#}").contains("claim"));assert_eq!(fs::read(&claim).unwrap(),b"another-owner");
        assert_eq!(files(&f.project),before);assert_eq!(files(&f.store),stored);
        assert!(!f.store.join("manifests").exists());assert!(!f.project.join(".projr/manifests.csv").exists());
    }
}
#[test]
fn two_stores_share_authoritative_ledger_and_competitors_can_retry_without_lost_or_duplicate_rows() {
    let f=fixture();let first=f.project.parent().unwrap().join("a-store");let second=f.project.parent().unwrap().join("z-store");
    let barrier=Arc::new(Barrier::new(2));let mut threads=Vec::new();
    for (store,label) in [(first.clone(),"raw-a"),(second.clone(),"raw-b")] {
        let root=f.project.clone();let source=f.source.clone();let barrier=barrier.clone();
        threads.push(std::thread::spawn(move||{barrier.wait();proj::cas::ingest_directory(&root,&store,label,&source,"v1.0.0")}));
    }
    let mut successes=0;
    for thread in threads {
        match thread.join().unwrap(){Ok(())=>successes+=1,Err(error)=>assert!(format!("{error:#}").contains("claim"),"{error:#}")}
    }
    assert!(successes>=1);
    for _ in 0..2 {for (store,label) in [(&first,"raw-a"),(&second,"raw-b")] {
        proj::cas::ingest_directory(&f.project,store,label,&f.source,"v1.0.0").unwrap();
    }}
    let rows=rows(&f.project);assert_eq!(rows.len(),2);
    assert_eq!(rows.iter().map(|row|row[1].to_owned()).collect::<BTreeSet<_>>(),BTreeSet::from(["raw-a".into(),"raw-b".into()]));
    verify_store(&first,&f.source);verify_store(&second,&f.source);
    assert!(!f.project.join(".projr/.proj/build.lock").exists());
}
#[test]
fn coincident_store_ledger_root_deduplicates_claims_without_reacquiring_parent_build() {
    let f=fixture();let build=f.project.join(".proj/build.lock");
    fs::create_dir_all(build.parent().unwrap()).unwrap();fs::write(&build,b"outer-build-owner").unwrap();
    let store=f.project.join(".projr");
    proj::cas::ingest_directory(&f.project,&store,"raw",&f.source,"v1.0.0").unwrap();
    let before=files(&store);
    proj::cas::ingest_directory(&f.project,&store,"raw",&f.source,"v1.0.0").unwrap();
    assert_eq!(rows(&f.project).len(),1);assert_eq!(files(&store),before);assert_eq!(fs::read(build).unwrap(),b"outer-build-owner");
    verify_store(&store,&f.source);
}
#[test]
fn malformed_index_fails_before_new_metadata_and_preserves_source_and_index_bytes() {
    let f=fixture();let ledger=f.project.join(".projr/manifests.csv");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();fs::write(&ledger,b"five,invalid,columns,in,index\n").unwrap();
    let before=files(&f.project);
    let error=proj::cas::ingest_directory(&f.project,&f.store,"raw",&f.source,"v1.0.0").unwrap_err();
    assert!(format!("{error:#}").contains("index"));assert_eq!(files(&f.project),before);assert!(files(&f.store).is_empty());
}
#[cfg(unix)]
#[test]
fn linked_source_and_ledger_coordination_fail_without_external_writes() {
    let f=fixture();let outside=f.project.parent().unwrap().join("outside");fs::create_dir(&outside).unwrap();fs::write(outside.join("keep"),b"preserved").unwrap();
    std::os::unix::fs::symlink(outside.join("keep"),f.source.join("link")).unwrap();
    assert!(proj::cas::ingest_directory(&f.project,&f.store,"raw",&f.source,"v1.0.0").is_err());
    assert!(!f.store.exists());assert!(!f.project.join(".projr").exists());fs::remove_file(f.source.join("link")).unwrap();
    fs::create_dir(f.project.join(".projr")).unwrap();
    std::os::unix::fs::symlink(&outside,f.project.join(".projr/.proj")).unwrap();
    assert!(proj::cas::ingest_directory(&f.project,&f.store,"raw",&f.source,"v1.0.0").is_err());
    assert_eq!(files(&outside),BTreeMap::from([("keep".into(),b"preserved".to_vec())]));assert!(!outside.join("build.lock").exists());
}
