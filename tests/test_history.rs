//! Actual standalone builds and read-only history, without ambient configuration.
use std::{fs,path::Path,process::Command};
use serde_json::{Value,json};
fn command(root:&Path,args:&[&str])->std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_proj")).args(["--non-interactive","--project"]).arg(root)
        .args(args).current_dir(root).env("PATH","")
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT").output().unwrap()
}
fn history(root:&Path,id:Option<&str>)->Value {
    let mut args=vec!["--machine","history"];if let Some(id)=id{args.extend(["--attempt",id])}
    let result=command(root,&args);
    assert!(result.status.success(),"{}",String::from_utf8_lossy(&result.stderr));
    serde_json::from_slice::<Value>(&result.stdout).unwrap()["result"]["history"].clone()
}
fn setup()->(tempfile::TempDir,std::path::PathBuf) {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
    fs::write(root.join("VERSION"),b"v1.2.3-19\n").unwrap();
    fs::write(root.join("_proj.yml"),"directories:\n  output: {path: released}\n  output-excluded: {path: excluded, hash: false}\nbuild:\n  scripts: []\n  git: false\n").unwrap();
    fs::create_dir(root.join("released")).unwrap();fs::create_dir(root.join("excluded")).unwrap();
    fs::write(root.join("released/é,[x]&.csv"),b"abc").unwrap();
    fs::write(root.join("excluded/private.csv"),b"excluded").unwrap();
    (temp,root)
}
fn build(root:&Path) {let result=command(root,&["build"]);assert!(result.status.success(),"{}",String::from_utf8_lossy(&result.stderr));}
fn record_path(root:&Path,id:&str)->std::path::PathBuf {root.join(format!(".proj/log/{id}.json"))}
#[test]
fn history_lists_exact_final_rows_after_rename_delete_and_rebuild_without_git_or_config() {
    let (_temp,root)=setup();build(&root);
    let first=history(&root,None)["entries"][0]["attempt_id"].as_str().unwrap().to_owned();
    let first_bytes=fs::read(record_path(&root,&first)).unwrap();
    let detail=history(&root,Some(&first));
    assert_eq!(detail["entries"][0]["status"],"completed");
    let rows=&detail["entries"][0]["final_files"]["rows"];
    assert_eq!(rows.as_array().unwrap().len(),1);
    assert_eq!(rows[0]["fn"],"é,[x]&.csv");assert_eq!(rows[0]["hash"],"900150983cd24fb0d6963f7d28e17f72");
    fs::rename(root.join("released/é,[x]&.csv"),root.join("released/new.csv")).unwrap();
    fs::write(root.join("released/new.csv"),b"hello").unwrap();build(&root);
    let summary=history(&root,None);assert_eq!(summary["entries"].as_array().unwrap().len(),2);
    let table=command(&root,&["history"]);
    let table=String::from_utf8(table.stdout).unwrap();
    let summary_table=table.split("\n## ").next().unwrap();
    assert_eq!(summary_table.lines().filter(|line|line.starts_with("| ")).count(),4);
    let second=summary["entries"][0]["attempt_id"].as_str().unwrap();assert_ne!(first,second);
    let second_rows=history(&root,Some(second))["entries"][0]["final_files"]["rows"].clone();
    assert_eq!(second_rows[0]["fn"],"new.csv");assert_eq!(second_rows[0]["hash"],"5d41402abc4b2a76b9719d911017c592");
    fs::remove_file(root.join("released/new.csv")).unwrap();build(&root);
    let latest=history(&root,None)["entries"][0]["attempt_id"].as_str().unwrap().to_owned();
    assert_eq!(history(&root,Some(&latest))["entries"][0]["final_files"]["rows"][0]["fn"],"");
    fs::remove_file(root.join("_proj.yml")).unwrap();fs::remove_file(root.join("VERSION")).unwrap();
    fs::remove_file(root.join("manifest.csv")).unwrap();
    assert_eq!(history(&root,Some(&first)),detail);
    let human=command(&root,&["history","--attempt",&first]);assert!(human.status.success());
    let human=String::from_utf8(human.stdout).unwrap();
    assert!(human.contains(&first)&&human.contains("v1.2.4"));
    assert!(human.contains("é")&&human.contains("&#38;"));assert!(!human.contains("[x]"));
    assert_eq!(fs::read(record_path(&root,&first)).unwrap(),first_bytes);
    assert!(!root.join(".proj/build.lock").exists());
}
#[test]
fn corrupted_missing_linked_and_traversal_history_fails_without_writes() {
    let (_temp,root)=setup();build(&root);
    let id=history(&root,None)["entries"][0]["attempt_id"].as_str().unwrap().to_owned();
    let path=record_path(&root,&id);let bytes=fs::read(&path).unwrap();
    let record:Value=serde_json::from_slice(&bytes).unwrap();
    let csv=root.join(".proj/log").join(record["final_outputs"]["rows"]["file"].as_str().unwrap());
    let csv_bytes=fs::read(&csv).unwrap();
    for bad in ["../VERSION","attempt-../../../../../../../../x"] {
        assert!(!command(&root,&["history","--attempt",bad]).status.success());
    }
    fs::write(&csv,b"corrupt").unwrap();
    assert!(!command(&root,&["--machine","history","--attempt",&id]).status.success());
    fs::remove_file(&csv).unwrap();assert!(!command(&root,&["history","--attempt",&id]).status.success());
    fs::write(&csv,&csv_bytes).unwrap();
    let mut bad=record.clone();bad["final_outputs"]["rows"]["file"]=json!("../VERSION");
    fs::write(&path,serde_json::to_vec(&bad).unwrap()).unwrap();
    assert!(!command(&root,&["history","--attempt",&id]).status.success());
    bad=record.clone();bad["git"]["head"]=json!("https://user:secret@private.invalid/token");
    fs::write(&path,serde_json::to_vec(&bad).unwrap()).unwrap();
    let rejected=command(&root,&["--machine","history"]);
    assert!(!rejected.status.success());assert!(!String::from_utf8_lossy(&rejected.stdout).contains("secret"));
    fs::write(&path,&bytes).unwrap();
    #[cfg(unix)] {
        fs::remove_file(&csv).unwrap();std::os::unix::fs::symlink(root.join("manifest.csv"),&csv).unwrap();
        assert!(!command(&root,&["history","--attempt",&id]).status.success());
        fs::remove_file(&csv).unwrap();fs::write(&csv,&csv_bytes).unwrap();
        fs::remove_file(&path).unwrap();std::os::unix::fs::symlink(root.join("manifest.csv"),&path).unwrap();
        assert!(!command(&root,&["history"]).status.success());
        fs::remove_file(&path).unwrap();fs::write(&path,&bytes).unwrap();
    }
    assert_eq!(fs::read(&path).unwrap(),bytes);assert_eq!(fs::read(&csv).unwrap(),csv_bytes);
    assert!(!root.join(".proj/build.lock").exists());
}
#[test]
fn old_fingerprints_only_reconstruct_matching_rows_and_pending_is_unresolved() {
    let (_temp,root)=setup();build(&root);
    let id=history(&root,None)["entries"][0]["attempt_id"].as_str().unwrap().to_owned();
    let path=record_path(&root,&id);let mut record:Value=serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record["final_outputs"].as_object_mut().unwrap().remove("rows");
    fs::write(&path,serde_json::to_vec(&record).unwrap()).unwrap();
    assert_eq!(history(&root,Some(&id))["entries"][0]["final_files"]["source"],"project_manifest");
    let manifest=fs::read(root.join("manifest.csv")).unwrap();
    fs::write(root.join("manifest.csv"),proj::manifest_csv::encode(&[]).unwrap()).unwrap();
    assert_eq!(history(&root,Some(&id))["entries"][0]["final_files"],Value::Null);
    fs::write(root.join("manifest.csv"),manifest).unwrap();
    record["status"]=json!("pre_git_snapshot");record["stage"]=json!("git");record["git"]["commit"]=json!("pending");
    record["final_outputs"]=Value::Null;
    fs::write(&path,serde_json::to_vec(&record).unwrap()).unwrap();
    assert_eq!(history(&root,Some(&id))["entries"][0]["status"],"unresolved");
    assert_eq!(history(&root,Some(&id))["entries"][0]["final_files"],Value::Null);
}
#[test]
fn history_limits_and_unknown_project_are_read_only() {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
    assert_eq!(history(&root,None),json!({"entries":[]}));
    for limit in ["0","101"]{assert!(!command(&root,&["history","--limit",limit]).status.success());}
    assert_eq!(fs::read_dir(root).unwrap().count(),0);
}
