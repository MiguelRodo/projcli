//! Independent verification of retained integrated M3 store and recovered bytes.
use std::{fs,path::Path,collections::BTreeMap};
use serde_json::Value;
fn files(root:&Path)->BTreeMap<String,Vec<u8>> {
    fn walk(root:&Path,path:&Path,result:&mut BTreeMap<String,Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry=entry.unwrap();let path=entry.path();
            if entry.file_type().unwrap().is_dir(){walk(root,&path,result)}else{
                assert!(entry.file_type().unwrap().is_file());
                result.insert(path.strip_prefix(root).unwrap().to_str().unwrap().replace(std::path::MAIN_SEPARATOR,"/"),fs::read(path).unwrap());
            }
        }
    }
    let mut result=BTreeMap::new();walk(root,root,&mut result);result
}
#[test]
#[ignore="run after the remote integrated M3 fixture writes its evidence"]
fn integrated_m3_snapshot_membership_matches_actual_recovered_and_stored_bytes() {
    let root=std::path::PathBuf::from(std::env::var_os("PROJCLI_M3_EVIDENCE").expect("M3 artifact path required"));
    for name in ["tree","flat-csv","flat-file-dictionary"] {
        let case=root.join(name);let store=case.join("store");
        let evidence:Value=serde_json::from_slice(&fs::read(case.join("verification.json")).unwrap()).unwrap();
        let id=evidence["result"]["snapshot"]["snapshot_id"].as_str().unwrap();
        let path=camino::Utf8Path::from_path(&store).unwrap();
        let directories=if name=="tree" {proj::cas_tree::verify_snapshot(path,path,id).unwrap().directories}
            else{proj::cas_flat::verify_snapshot(path,path,id).unwrap().directories};
        assert_eq!(directories.len(),2);
        for directory in directories {
            let recovered=case.join(format!("recovered-{}",directory.label));let actual=files(&recovered);
            let expected:BTreeMap<_,_>=directory.files.iter().map(|file|(file.path.clone(),file.hash.clone())).collect();
            let observed:BTreeMap<_,_>=actual.iter().map(|(name,bytes)|(name.clone(),blake3::hash(bytes).to_hex().to_string())).collect();
            assert_eq!(observed,expected);
            for empty in directory.empty_directories {assert!(recovered.join(empty).is_dir());}
        }
        let raw=files(&case.join("recovered-raw"));assert_eq!(raw["input.csv"],b"value\n21\n");
        let output=files(&case.join("recovered-output"));assert_eq!(output,BTreeMap::from([("result.csv".into(),b"value\n42\n".to_vec())]));
        let manifest=proj::manifest_csv::decode(&fs::read(case.join("manifest.csv")).unwrap()).unwrap();
        for row in manifest.iter().filter(|row|row.version=="v1.2.5"&&!row.r#fn.is_empty()) {
            let bytes=if row.label=="raw"{&raw[&row.r#fn]}else{assert_eq!(row.label,"output");&output[&row.r#fn]};
            assert_eq!(row.hash,format!("{:x}",md5::compute(bytes)));
        }
    }
}
