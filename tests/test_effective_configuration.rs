//! Actual merged/alias/profile/override fingerprints; no local environment mutation.
use std::{fs,path::Path,process::Command};
use serde_json::{Value,json};
fn build(root:&Path,args:&[&str])->Value {
    let result=Command::new(env!("CARGO_BIN_EXE_proj")).current_dir(root)
        .args(["--project"]).arg(root).args(["--non-interactive","build"]).args(args).env("PATH","")
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT").output().unwrap();
    assert!(result.status.success(),"{}",String::from_utf8_lossy(&result.stderr));
    let mut records:Vec<Value>=fs::read_dir(root.join(".proj/log")).unwrap().map(|e|e.unwrap().path())
        .filter(|p|p.extension().is_some_and(|ext|ext=="json"))
        .map(|p|serde_json::from_slice(&fs::read(p).unwrap()).unwrap()).collect();
    records.sort_by_key(|r|r["release_version"].as_str().unwrap().to_owned());
    let record=records.pop().unwrap();record["startup_provenance"]["configuration"].clone()
}
fn write(root:&Path,value:&Value) {fs::write(root.join("_proj.yml"),serde_json::to_vec(value).unwrap()).unwrap();}
#[test]
fn actual_builds_bind_merged_parameters_aliases_profiles_and_effective_cli_policy_without_secret_values() {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
    fs::write(root.join("VERSION"),"v1.2.3-19\n").unwrap();
    fs::create_dir(root.join("released")).unwrap();fs::write(root.join("released/result.csv"),b"abc").unwrap();
    let mut cfg=json!({"directories":{"output":{"path":"released"}},"parameters":{"factor":2,"token":"SECRET-ONE",
        "endpoint":"https://u:p@private.invalid/a"},"build":{"scripts":[],"git":false}});
    write(&root,&cfg);let first=build(&root,&[]);
    assert_eq!(first["effective"]["status"],"observed_redacted");
    assert_eq!(first["effective"]["redacted_values"],2);
    let hash=first["effective"]["blake3"].as_str().unwrap();assert_eq!(hash.len(),64);
    cfg["parameters"]["factor"]=json!(3);write(&root,&cfg);
    let changed=build(&root,&[]);assert_ne!(first["effective"]["blake3"],changed["effective"]["blake3"]);
    cfg["parameters"]["token"]=json!("SECRET-TWO");cfg["parameters"]["endpoint"]=json!("https://x:y@other.invalid/b");
    write(&root,&cfg);assert_eq!(build(&root,&[]),changed);
    let params=cfg.as_object_mut().unwrap().remove("parameters").unwrap();
    cfg["params"]=params;write(&root,&cfg);assert_eq!(build(&root,&[]),changed);
    fs::write(root.join("_projr-analysis.yml"),"params:\n  factor: 7\n").unwrap();
    let profile=build(&root,&["--profile","analysis"]);assert_ne!(profile["effective"]["blake3"],changed["effective"]["blake3"]);
    let override_policy=build(&root,&["--profile","analysis","--execution-dir","project"]);
    assert_ne!(profile["effective"]["blake3"],override_policy["effective"]["blake3"]);
    for entry in fs::read_dir(root.join(".proj/log")).unwrap() {
        let bytes=fs::read(entry.unwrap().path()).unwrap();let text=String::from_utf8_lossy(&bytes);
        for secret in ["SECRET-ONE","SECRET-TWO","private.invalid","other.invalid","https://","u:p","x:y"] {assert!(!text.contains(secret),"{secret}");}
    }
    assert!(!root.join(".git").exists());
}
#[test]
fn bounded_out_configuration_keeps_previously_supported_standalone_builds_working() {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
    let cfg=json!({"parameters":{"large":"x".repeat(4*1024*1024+1)},"build":{"scripts":[],"git":false,"output_run":false}});
    write(&root,&cfg);fs::write(root.join("VERSION"),"v1.2.3-19\n").unwrap();
    let evidence=build(&root,&[]);
    assert_eq!(evidence["effective"]["status"],"limit");assert!(evidence["effective"]["blake3"].is_null());
}
