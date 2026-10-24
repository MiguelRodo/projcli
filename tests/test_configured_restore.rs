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
fn request()->RestoreRequest {RestoreRequest {label:"raw".into(),profile:Some("".into()),max_bytes:1024*1024,max_expanded_bytes:1024*1024,..Default::default()}}
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
    // Restore annotations can now coexist with normal build configuration;
    // normal builds validate the reference without automatically restoring.
    assert!(proj::yml::yml_read_from(&root,false).is_ok());
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
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .args(["--project",root.as_str(),"--machine","remote","restore","--label","raw","--clear","--max-bytes","10000","--max-expanded-bytes","10000"]).output().unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    let value:serde_json::Value=serde_json::from_slice(&output.stdout).unwrap();assert_eq!(value["result"]["restored"],false);assert_eq!(value["result"]["skipped"],"empty-remote");
    assert!(root.join("data/old.txt").exists());
}

#[test]
fn child_profile_selection_matches_build_quarto_projr_and_explicit_precedence() {
    for (projr,quarto,explicit,expected) in [
        ("","",None,"analysis"),("","quarto",None,"quarto"),
        ("analysis","quarto",None,"analysis"),("","quarto",Some("analysis"),"analysis"),
        ("","",Some("first,second"),"first"),
    ] {
        let (_temp,root)=setup();
        for name in ["base","analysis","quarto","first","second"] {
            fs::create_dir(root.join(format!("{name}-target"))).unwrap();fs::write(root.join(format!("{name}-target/old")),"intact").unwrap();
            remote(&root,name,"v1.0",name);
        }
        fs::write(root.join("_proj.yml"),"directories:\n  raw:\n    path: base-target\n    source:\n      local:\n        A:\n          path: base\nbuild:\n  profile: analysis\n").unwrap();
        for name in ["analysis","quarto","first","second"] {
            fs::write(root.join(format!("_projr-{name}.yml")),format!("directories:\n  raw:\n    path: {name}-target\n    source:\n      local:\n        A:\n          path: {name}\n")).unwrap();
        }
        let mut command=Command::new(env!("CARGO_BIN_EXE_proj"));
        command.env("PROJR_PROFILE",projr).env("QUARTO_PROFILE",quarto)
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
            .args(["--project",root.as_str(),"--machine","remote","restore","--label","raw","--clear","--max-bytes","1000","--max-expanded-bytes","1000"]);
        if let Some(profile)=explicit {command.args(["--profile",profile]);}
        let output=command.output().unwrap();assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
        let value:serde_json::Value=serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["result"]["destination"],root.join(format!("{expected}-target")).as_str());
        for name in ["base","analysis","quarto","first","second"] {
            if name==expected {assert_eq!(fs::read(root.join(format!("{name}-target/data.csv"))).unwrap(),name.as_bytes());assert!(!root.join(format!("{name}-target/old")).exists());}
            else {assert_eq!(fs::read(root.join(format!("{name}-target/old"))).unwrap(),b"intact");assert!(!root.join(format!("{name}-target/data.csv")).exists());}
        }
    }
}
#[test]
fn undefined_reference_selected_source_failure_and_unsupported_get_do_not_fallback_or_clear() {
    let (_temp,root)=setup();remote(&root,"valid","v1.0","fallback");
    for sources in [
        "    source:\n      local: undefined\n",
        "    source:\n      local:\n        first:\n          path: missing\n        second:\n          path: valid\n",
        "    source:\n      local:\n        first:\n          path: valid\n          get:\n            strategy: sync-purge\n",
    ] {
        fs::write(root.join("_proj.yml"),format!("directories:\n  raw:\n    path: data\n{sources}build:\n  local:\n    fallback:\n      path: valid\n      content: [raw]\n")).unwrap();
        let mut req=request();req.profile=Some("".into());req.clear=true;
        assert!(restore_label(&root,&req).is_err());assert_eq!(fs::read(root.join("data/old.txt")).unwrap(),b"previous");assert!(!root.join("data/data.csv").exists());
    }
}
#[test]
fn legacy_data_aliases_are_read_only_and_conflicting_namespaces_fail() {
    let (_temp,root)=setup();
    fs::write(root.join("_projr.yml"),"directories:\n  data: {}\nbuild:\n  local:\n    archive:\n      path: remote\n      content: [data]\n      source: [data]\n").unwrap();
    let history=b"label,fn,version,hash\ndata,data.csv,v1.2.3,900150983cd24fb0d6963f7d28e17f72\n";
    fs::write(root.join("manifest.csv"),history).unwrap();
    fs::create_dir_all(root.join("remote/data/v1.2.3")).unwrap();fs::write(root.join("remote/data/v1.2.3/restored"),"legacy bytes").unwrap();
    let config=fs::read(root.join("_projr.yml")).unwrap();
    let mut req=request();req.label="output-r-data".into();req.profile=Some("".into());req.clear=true;
    let result=restore_label(&root,&req).unwrap();assert!(result.restored);
    assert_eq!(fs::read(root.join("data/restored")).unwrap(),b"legacy bytes");
    assert_eq!(fs::read(root.join("manifest.csv")).unwrap(),history);assert_eq!(fs::read(root.join("_projr.yml")).unwrap(),config);
    fs::create_dir_all(root.join("remote/output-r-data/v1.2.3")).unwrap();fs::write(root.join("remote/output-r-data/v1.2.3/new"),"conflict").unwrap();
    assert!(restore_label(&root,&req).unwrap_err().to_string().contains("namespaces conflict"));
    assert_eq!(fs::read(root.join("data/restored")).unwrap(),b"legacy bytes");
}


#[test]
fn registry_sources_preserve_legacy_priority_unless_explicitly_preferred() {
    for (legacy_preferred,registry_preferred,directory_reference,expected) in [
        (false,false,false,"legacy"),(true,true,false,"legacy"),
        (false,true,false,"registered"),(true,false,true,"registered")] {
        let (_temp,root)=setup();remote(&root,"legacy","v1.0","legacy");remote(&root,"registered","v1.0","registered");
        let mut directory=serde_json::json!({"path":"data"});
        if directory_reference {directory["source"]=serde_json::json!({"local":"registered"});}
        let mut old=serde_json::json!({"path":"legacy","content":["raw"]});
        let mut new=serde_json::json!({"path":"registered","content":["raw"]});
        if legacy_preferred {old["source"]=serde_json::json!(true);}
        if registry_preferred {new["source"]=serde_json::json!(true);}
        let config=serde_json::json!({"directories":{"raw":directory},"remotes":{"local":{"registered":new}},
            "build":{"local":{"legacy":old}}});
        fs::write(root.join("_proj.yml"),serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        let result=restore_label(&root,&request()).unwrap();assert_eq!(result.title.as_deref(),Some(expected));
        assert_eq!(fs::read(root.join("data/data.csv")).unwrap(),expected.as_bytes());
    }
}
