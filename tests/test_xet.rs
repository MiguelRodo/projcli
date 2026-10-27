//! Exact engine integration is opt-in in dedicated CI; ordinary suites never fetch Xet.
use std::{fs,path::PathBuf};
use camino::{Utf8Path,Utf8PathBuf};
use proj::xet::{Engine,Snapshot};
fn path(value:impl AsRef<std::path::Path>)->Utf8PathBuf {Utf8PathBuf::from_path_buf(value.as_ref().to_owned()).unwrap()}
fn engine()->Option<Engine> {
    let program=std::env::var_os("PROJCLI_TEST_XET_ENGINE");
    assert!(program.is_some()||std::env::var_os("PROJCLI_TEST_XET_REQUIRED").is_none(),
        "dedicated Xet gate must exercise actual optional engine");
    program.map(|program|Engine::new(PathBuf::from(program)).unwrap())
}
#[test]
fn actual_xet_restart_versions_corruption_and_destination_preservation() {
    let Some(engine)=engine() else {return};
    let temp=tempfile::tempdir().unwrap();let root=path(temp.path());let project=root.join("project");
    let source=project.join("raw");let store=root.join("store");let recovered=project.join("restored");
    fs::create_dir_all(source.join("nested")).unwrap();fs::create_dir(source.join("empty")).unwrap();
    let mut bytes=Vec::new();let mut state=0x123456789abcdu64;
    for _ in 0..(4*1024*1024) {state^=state<<13;state^=state>>7;state^=state<<17;bytes.push(state as u8);}
    fs::write(source.join("nested/data.bin"),&bytes).unwrap();
    fs::write(source.join("Unicode-\u{00e9},quoted.txt"),b"hello, world\n").unwrap();
    let inputs=vec![("raw-data".into(),source.clone())];
    let first=proj::xet::publish(&project,&store,&inputs,"v1.2.3",&engine).unwrap();
    let seed=root.join("seed");copy(&store,&seed);
    #[cfg(unix)] {
        let database=store.join("engine/xet/xorbs/global_dedup_lookup.redb");
        let alias=root.join("external-database-alias");
        fs::hard_link(&database,&alias).unwrap();
        let before=fs::read(&alias).unwrap();
        assert!(proj::xet::verify(&project,&store,&first,&engine).is_err());
        assert_eq!(fs::read(&alias).unwrap(),before);
        fs::remove_file(alias).unwrap();
    }
    assert_eq!(first,proj::xet::publish(&project,&store,&inputs,"v1.2.3",&engine).unwrap());
    bytes.splice(128_000..128_000,vec![0x55;64*1024]);fs::write(source.join("nested/data.bin"),&bytes).unwrap();
    let second=proj::xet::publish(&project,&store,&inputs,"v1.2.4",&engine).unwrap();
    assert_ne!(first,second);
    assert!(proj::xet::verify(&project,&store,&first,&engine).is_ok());
    // Remove all working data/client caches. Only the persistent store and committed refs remain.
    fs::remove_dir_all(&source).unwrap();
    for relative in ["engine/xet/shard-cache","engine/xet/shard-session"] {
        let p=store.join(relative);if p.exists(){fs::remove_dir_all(p).unwrap();}
    }
    proj::xet::restore(&project,&store,&second,"raw-data",&recovered,true,&engine).unwrap();
    assert_eq!(fs::read(recovered.join("nested/data.bin")).unwrap(),bytes);
    assert!(recovered.join("empty").is_dir());
    fs::write(recovered.join("keep-extra-sentinel"),b"destination-only data").unwrap();
    let saved=tree(&recovered);
    // Modify an owned payload-pack clone by replacing the read-only inode, never original data.
    let damaged=root.join("damaged");copy(&seed,&damaged);
    let packs=damaged.join("engine/xet/xorbs/xorbs");
    let pack=fs::read_dir(&packs).unwrap().map(|e|e.unwrap().path())
        .find(|p|p.file_name().unwrap().to_str().unwrap().starts_with("default.")&&p.extension().unwrap_or_default()!="tagset").unwrap();
    let mut data=fs::read(&pack).unwrap();let index=data.len()/2;data[index]^=0x80;
    let replacement=pack.with_extension("replacement");fs::write(&replacement,&data).unwrap();fs::rename(replacement,&pack).unwrap();
    assert!(proj::xet::restore(&project,&damaged,&first,"raw-data",&recovered,true,&engine).is_err());
    assert_eq!(tree(&recovered),saved);
    // Missing content also fails, even if earlier reads populated local client caches.
    fs::remove_dir_all(&packs).unwrap();
    assert!(proj::xet::restore(&project,&damaged,&first,"raw-data",&recovered,true,&engine).is_err());
    assert_eq!(tree(&recovered),saved);
    // Invalid reference and stale cooperative claim are failures, without replacing outputs.
    assert!(proj::xet::verify(&project,&store,&"0".repeat(64),&engine).is_err());
    fs::create_dir_all(store.join(".proj")).unwrap();fs::write(store.join(".proj/build.lock"),"other-writer").unwrap();
    assert!(proj::xet::restore(&project,&store,&second,"raw-data",&recovered,true,&engine).is_err());
    assert_eq!(fs::read(store.join(".proj/build.lock")).unwrap(),b"other-writer");
    assert_eq!(tree(&recovered),saved);
}
fn copy(source:&Utf8Path,target:&Utf8Path) {
    fs::create_dir(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry=entry.unwrap();let target=target.join(entry.file_name().to_str().unwrap());
        let source=path(entry.path());
        if entry.file_type().unwrap().is_dir(){copy(&source,&target)}else{fs::copy(source,target).unwrap();}
    }
}
#[test]
fn xet_never_reinterprets_legacy_ids_or_unmarked_store() {
    let Some(engine)=engine() else{return};
    let temp=tempfile::tempdir().unwrap();let root=path(temp.path());let project=root.join("project");
    fs::create_dir(&project).unwrap();let source=project.join("raw");fs::create_dir(&source).unwrap();
    let store=root.join("old");fs::create_dir(&store).unwrap();fs::write(store.join("legacy-object"),"keep").unwrap();
    assert!(proj::xet::publish(&project,&store,&[("raw-data".into(),source)],"v1.0.0",&engine).is_err());
    assert_eq!(fs::read(store.join("legacy-object")).unwrap(),b"keep");
    assert!(!store.join("xet-format.json").exists());
    assert!(proj::xet::read_snapshot(&project,&store,&"0".repeat(64)).is_err());
}
#[test]
fn malformed_metadata_is_bounded_and_requires_explicit_backend() {
    // Pure metadata checks are always exercised by ordinary platform CI.
    let invalid=r#"{"schema":"legacy","backend":"xet","upstream":"latest","inventory":{},"roots":{},"payloads":{}}"#;
    assert!(serde_json::from_str::<Snapshot>(invalid).is_err());
}

fn tree(root:&Utf8Path)->std::collections::BTreeMap<String,Option<Vec<u8>>> {
    fn visit(root:&Utf8Path,path:&Utf8Path,result:&mut std::collections::BTreeMap<String,Option<Vec<u8>>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry=entry.unwrap();let value=crate::path(entry.path());
            let relative=value.strip_prefix(root).unwrap().as_str().replace('\\',"/");
            let directory=entry.file_type().unwrap().is_dir();
            result.insert(relative,if directory {None}else{Some(fs::read(&value).unwrap())});
            if directory {visit(root,&value,result);}
        }
    }
    let mut result=std::collections::BTreeMap::new();visit(root,root,&mut result);result
}
