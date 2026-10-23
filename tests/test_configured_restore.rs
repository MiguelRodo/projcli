use std::{fs,process::Command};
use camino::Utf8PathBuf;
use proj::restore::{restore_label,RestoreRequest};
fn root(temp:&tempfile::TempDir)->Utf8PathBuf {Utf8PathBuf::from_path_buf(temp.path().canonicalize().unwrap()).unwrap()}
fn setup()->(tempfile::TempDir,Utf8PathBuf) {
    let temp=tempfile::tempdir().unwrap();let root=root(&temp);
    fs::create_dir(root.join("data")).unwrap();fs::write(root.join("data/old.txt"),"previous").unwrap();
    fs::write(root.join("manifest.csv"),"label,fn,version,hash\nraw,data.csv,v1.2.3,900150983cd24fb0d6963f7d28e17f72\n").unwrap();
    fs::write(root.join("VERSION"),"v1.2.3\n").unwrap();(temp,root)
}
fn request()->RestoreRequest {RestoreRequest {label:"raw".into(),max_bytes:1024*1024,max_expanded_bytes:1024*1024,..Default::default()}}
fn remote(root:&camino::Utf8Path,base:&str,version:&str,bytes:&str) {
    fs::create_dir_all(root.join(base).join("raw").join(version)).unwrap();
    fs::write(root.join(base).join("raw").join(version).join("data.csv"),bytes).unwrap();
}
#[test]
fn source_reference_precedes_preferred_destination_and_clear_is_staged() {
    let (_temp,root)=setup();remote(&root,"restore-A","v1.9","chosen-A");remote(&root,"upload-B","v1.10","not-B");
    fs::write(root.join("_proj.yml"),"directories:\n  raw:\n    path: data\n    source:\n      local: A\nbuild:\n  local:\n    A:\n      path: restore-A\n    B:\n      path: upload-B\n      content: [raw]\n      source: true\n").unwrap();
    let result=restore_label(&root,&request()).unwrap();assert!(result.restored);assert_eq!(result.title.as_deref(),Some("A"));
    assert_eq!(fs::read(root.join("data/data.csv")).unwrap(),b"chosen-A");assert!(root.join("data/old.txt").exists());
    let mut clear=request();clear.clear=true;restore_label(&root,&clear).unwrap();assert!(!root.join("data/old.txt").exists());
    // Ordinary publication fields are not silently accepted by builds.
    assert!(proj::yml::yml_read_from(&root,false).is_err());
}
#[test]
fn explicit_inline_source_and_profile_local_precedence_use_physical_final_path() {
    let (_temp,root)=setup();remote(&root,"base","v1.0","base");remote(&root,"profile","v1.0","profile");remote(&root,"local","v1.0","local");
    fs::write(root.join("_proj.yml"),"directories:\n  raw:\n    path: data\n    source:\n      local:\n        A:\n          path: base\n").unwrap();
    fs::write(root.join("_projr-analysis.yml"),"directories:\n  raw:\n    source:\n      local:\n        A:\n          path: profile\n").unwrap();
    fs::write(root.join("_projr-local.yml"),"directories:\n  raw:\n    source:\n      local:\n        A:\n          path: local\n").unwrap();
    let mut req=request();req.profile=Some("analysis".into());
    let result=restore_label(&root,&req).unwrap();assert!(result.restored);assert_eq!(fs::read(root.join("data/data.csv")).unwrap(),b"local");
}
#[test]
fn destination_preference_false_exclusion_filters_and_numeric_archive_selection() {
    let (_temp,root)=setup();for (base,bytes) in [("first","first"),("preferred","preferred"),("excluded","excluded")] {
        remote(&root,base,"v1.9",bytes);remote(&root,base,"v1.10",bytes);
    }
    fs::write(root.join("_proj.yml"),"directories:\n  raw:\n    path: data\nbuild:\n  local:\n    excluded:\n      path: excluded\n      content: [raw]\n      source: false\n    first:\n      path: first\n      content: [raw]\n    preferred:\n      path: preferred\n      content: [raw]\n      source: [raw]\n").unwrap();
    let result=restore_label(&root,&request()).unwrap();assert_eq!(result.title.as_deref(),Some("preferred"));assert_eq!(result.version.as_deref(),Some("v1.10"));
    let mut req=request();req.title=Some("first".into());req.remote_version=Some("v1.9".into());
    let result=restore_label(&root,&req).unwrap();assert_eq!(result.version.as_deref(),Some("v1.9"));assert_eq!(fs::read(root.join("data/data.csv")).unwrap(),b"first");
    req.title=Some("excluded".into());req.clear=true;assert!(restore_label(&root,&req).is_err());assert!(root.join("data/old.txt").exists());
}
#[test]
fn missing_corrupt_empty_unavailable_and_protected_restore_never_clears_first() {
    let (_temp,root)=setup();
    fs::write(root.join("_proj.yml"),"directories:\n  raw:\n    path: data\n    source:\n      local:\n        A:\n          path: missing\n").unwrap();
    let mut req=request();req.clear=true;assert!(restore_label(&root,&req).is_err());assert_eq!(fs::read(root.join("data/old.txt")).unwrap(),b"previous");
    fs::write(root.join("manifest.csv"),"label,fn,version,hash\nraw,,v1.2.3,\n").unwrap();
    assert_eq!(restore_label(&root,&req).unwrap().skipped.as_deref(),Some("no-recorded-files"));assert!(root.join("data/old.txt").exists());
    fs::write(root.join("manifest.csv"),"bad").unwrap();assert!(restore_label(&root,&req).is_err());
    fs::remove_file(root.join("manifest.csv")).unwrap();assert!(restore_label(&root,&req).is_err());
}
#[test]
fn empty_latest_skips_without_clearing_and_cli_uses_project_root() {
    let (_temp,root)=setup();fs::create_dir(root.join("remote")).unwrap();
    fs::create_dir(root.join("remote/raw-empty")).unwrap();fs::write(root.join("remote/VERSION"),"raw: v1.2.3*\n").unwrap();
    fs::write(root.join("_proj.yml"),"directories:\n  raw:\n    path: data\n    source:\n      local:\n        source:\n          path: remote\n          structure: latest\n").unwrap();
    let output=Command::new(env!("CARGO_BIN_EXE_proj")).current_dir(root.parent().unwrap())
        .args(["--project",root.as_str(),"--machine","remote","restore","--label","raw","--clear","--max-bytes","10000","--max-expanded-bytes","10000"]).output().unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    let value:serde_json::Value=serde_json::from_slice(&output.stdout).unwrap();assert_eq!(value["result"]["restored"],false);assert_eq!(value["result"]["skipped"],"empty-remote");
    assert!(root.join("data/old.txt").exists());
}
