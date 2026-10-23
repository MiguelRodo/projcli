//! Independent BLAKE3/CSV byte validation for the remote integrated M2 artifacts.
use std::fs;
use serde_json::Value;
#[test]
#[ignore="run after the remote integrated M2 fixture writes its evidence"]
fn integrated_m2_record_matches_actual_source_input_and_final_bytes() {
    let root=std::path::PathBuf::from(std::env::var_os("PROJCLI_M2_EVIDENCE").expect("M2 artifact path required"));
    for name in ["first","second","retry"] {
        let case=root.join(name);
        let record:Value=serde_json::from_slice(&fs::read(case.join("record.json")).unwrap()).unwrap();
        let source=&record["startup_provenance"]["selected_sources"][0];
        assert_eq!(source["name"],"analysis.py");
        assert_eq!(source["blake3"],blake3::hash(&fs::read(root.join("analysis.py")).unwrap()).to_hex().as_str());
        let dependencies=record["startup_provenance"]["dependencies"].as_array().unwrap();
        let declaration=dependencies.iter().find(|file|file["name"]=="requirements.txt").unwrap();
        assert_eq!(declaration["blake3"],blake3::hash(&fs::read(root.join("requirements.txt")).unwrap()).to_hex().as_str());
        let retained=fs::read(case.join("final.csv")).unwrap();
        let reference=&record["final_outputs"]["rows"];
        assert_eq!(reference["bytes"],retained.len());
        assert_eq!(reference["blake3"],blake3::hash(&retained).to_hex().as_str());
        let final_rows=proj::manifest_csv::decode(&retained).unwrap();
        assert_eq!(record["final_outputs"]["inventory"]["canonical_csv_blake3"],
            blake3::hash(&proj::manifest_csv::encode(&final_rows).unwrap()).to_hex().as_str());
        let result=fs::read(case.join("result.csv")).unwrap();
        assert_eq!(final_rows.iter().find(|row|row.r#fn=="result.csv").unwrap().hash,format!("{:x}",md5::compute(result)));
        let manifest=proj::manifest_csv::decode(&fs::read(case.join("manifest.csv")).unwrap()).unwrap();
        let current=record["release_version"].as_str().unwrap();
        let input:Vec<_>=manifest.iter().filter(|row|row.version==current&&row.label=="raw").cloned().collect();
        assert_eq!(input[0].hash,format!("{:x}",md5::compute(fs::read(case.join("input.csv")).unwrap())));
        assert_eq!(record["inventory"]["inputs"]["canonical_csv_blake3"],
            blake3::hash(&proj::manifest_csv::encode(&input).unwrap()).to_hex().as_str());
    }
}
