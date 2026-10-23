use std::{collections::BTreeMap,fs,path::Path,process::Command};
fn invoke(root:&Path,mode:&str)->std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_proj")).args([mode,"--non-interactive","--project"]).arg(root)
        .current_dir(root).env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT").output().unwrap()
}

fn restore(root:&Path)->serde_json::Value {
    let output=Command::new(env!("CARGO_BIN_EXE_proj")).current_dir(root)
        .args(["--machine","--project"]).arg(root)
        .args(["remote","restore","--label","output","--clear","--max-bytes","100000","--max-expanded-bytes","100000"])
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}
fn setup(strategy:&str,cue:&str,content:&str)->(tempfile::TempDir,std::path::PathBuf) {
    let temporary=tempfile::tempdir().unwrap();let root=temporary.path().canonicalize().unwrap();
    fs::create_dir(root.join("input")).unwrap();fs::create_dir(root.join("results")).unwrap();
    fs::write(root.join("VERSION"),"v1.2.3\n").unwrap();fs::write(root.join("CHANGELOG.md"),"curated changelog\n").unwrap();
    fs::write(root.join("results/same"),"same").unwrap();fs::write(root.join("results/changed"),"new").unwrap();fs::write(root.join("results/new"),"new").unwrap();
    fs::write(root.join("input/input.csv"),"abc").unwrap();
    fs::write(root.join("_proj.yml"),format!("directories:\n  raw: {{path: input}}\n  output: {{path: results}}\nbuild:\n  scripts: []\n  git: false\n  local:\n    delivery:\n      path: remote\n      structure: latest\n      content: {content}\n      send:\n        inspect: file\n        strategy: {strategy}\n        cue: {cue}\n")).unwrap();
    (temporary,root)
}
fn files(root:&Path)->BTreeMap<String,Vec<u8>> {
    let mut result=BTreeMap::new();if !root.exists(){return result;}
    for entry in fs::read_dir(root).unwrap() {
        let path=entry.unwrap().path();assert!(path.is_file());result.insert(path.file_name().unwrap().to_string_lossy().into(),fs::read(path).unwrap());
    }result
}
fn success(output:std::process::Output) {assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));}
#[test]
fn configured_build_four_strategies_and_both_cues_publish_exact_bytes_and_history() {
    for strategy in ["upload-all","upload-missing","sync-diff","sync-purge"] {
        for cue in ["always","if-change"] {
            let (_temporary,root)=setup(strategy,cue,"[output]");
            fs::create_dir_all(root.join("remote/output")).unwrap();
            for (name,bytes) in [("same","same"),("changed","old"),("old","old")] {fs::write(root.join("remote/output").join(name),bytes).unwrap();}
            let old=b"label,fn,version,hash\nraw,input.csv,v0.1.0,900150983cd24fb0d6963f7d28e17f72\n";
            fs::write(root.join("remote/manifest.csv"),old).unwrap();fs::write(root.join("remote/VERSION"),"Project: v0.1.0\nraw: v0.1.0\n").unwrap();
            success(invoke(&root,"build"));
            let mut expected=BTreeMap::from([("changed".into(),if strategy=="upload-missing" {b"old".to_vec()}else{b"new".to_vec()}),("new".into(),b"new".to_vec()),("same".into(),b"same".to_vec())]);
            if strategy.starts_with("upload-"){expected.insert("old".into(),b"old".to_vec());}
            assert_eq!(files(&root.join("remote/output")),expected);
            let versions=fs::read_to_string(root.join("remote/VERSION")).unwrap();
            let star=if strategy.starts_with("upload-"){"*"}else{""};
            assert!(versions.contains(&format!("output: v1.2.4{star}\n")),"{versions}");
            assert!(versions.contains("raw: v0.1.0\n"));
            assert_eq!(fs::read(root.join("remote/CHANGELOG.md")).unwrap(),b"curated changelog\n");
            let remote=proj::manifest::read(camino::Utf8Path::from_path(&root.join("remote/manifest.csv")).unwrap()).unwrap();
            assert!(remote.iter().any(|row|row.label=="raw"&&row.version=="v0.1.0"));
            assert_eq!(remote.iter().filter(|row|row.label=="output"&&row.version=="v1.2.4").count(),3);
            assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap().trim(),"Version: v1.2.4.1");
            let history=fs::read(root.join("manifest.csv")).unwrap();
            let version=fs::read(root.join("VERSION")).unwrap();
            fs::write(root.join("results/changed"),"damaged local").unwrap();
            fs::write(root.join("results/local-only"),"discard by explicit clear").unwrap();
            let restored=restore(&root);
            assert_eq!(restored["result"]["restored"],true);
            assert_eq!(restored["result"]["provider"],"local");
            assert_eq!(restored["result"]["title"],"delivery");
            assert_eq!(restored["result"]["version"],"v1.2.4");
            assert_eq!(files(&root.join("results")),expected);
            assert_eq!(fs::read(root.join("manifest.csv")).unwrap(),history);
            assert_eq!(fs::read(root.join("VERSION")).unwrap(),version);
        }
    }
}
#[test]
fn unchanged_sync_diff_cue_controls_label_version_while_project_and_history_advance() {
    for cue in ["always","if-change"] {
        let (_temporary,root)=setup("sync-diff",cue,"[output]");
        success(invoke(&root,"build"));success(invoke(&root,"build"));
        let text=fs::read_to_string(root.join("remote/VERSION")).unwrap();
        assert!(text.contains("Project: v1.2.5\n"));
        assert!(text.contains(if cue=="always" {"output: v1.2.5\n"}else{"output: v1.2.4\n"}),"{text}");
        let rows=proj::manifest::read(camino::Utf8Path::from_path(&root.join("remote/manifest.csv")).unwrap()).unwrap();
        for version in ["v1.2.4","v1.2.5"] {assert_eq!(rows.iter().filter(|row|row.label=="output"&&row.version==version).count(),3);}
    }
}
#[test]
fn content_true_false_null_and_explicit_list_select_only_the_declared_targets() {
    for content in ["true","false","null","[raw]"] {
        let (_temporary,root)=setup("sync-diff","if-change",content);
        let config=fs::read_to_string(root.join("_proj.yml")).unwrap();
        fs::write(root.join("_proj.yml"),config.replace("directories:\n","directories:\n  cache: {path: cache, hash: true}\n")).unwrap();
        fs::create_dir(root.join("cache")).unwrap();fs::write(root.join("cache/private"),"cache bytes").unwrap();
        success(invoke(&root,"build"));
        assert!(!root.join("remote/cache").exists());
        assert_eq!(root.join("remote/output").exists(),content=="true");
        assert_eq!(root.join("remote/raw").exists(),content=="true"||content=="[raw]");
    }
}
#[test]
fn empty_latest_round_trip_and_nonempty_transition_reconcile_variants() {
    let (_temporary,root)=setup("sync-diff","if-change","[output]");
    for name in ["same","changed","new"] {fs::remove_file(root.join("results").join(name)).unwrap();}
    success(invoke(&root,"build"));
    assert!(root.join("remote/output-empty").is_dir());assert!(!root.join("remote/output").exists());
    assert!(fs::read_to_string(root.join("remote/VERSION")).unwrap().contains("output: v1.2.4\n"));
    fs::write(root.join("results/restored"),"new release").unwrap();
    success(invoke(&root,"build"));
    assert_eq!(fs::read(root.join("remote/output/restored")).unwrap(),b"new release");assert!(!root.join("remote/output-empty").exists());
    fs::remove_file(root.join("results/restored")).unwrap();success(invoke(&root,"build"));
    assert!(root.join("remote/output-empty").is_dir());assert!(!root.join("remote/output").exists());
    fs::write(root.join("results/local-only"),"must survive empty restore").unwrap();
    let restored=restore(&root);
    assert_eq!(restored["result"]["restored"],false);
    assert_eq!(restored["result"]["skipped"],"empty-remote");
    assert_eq!(fs::read(root.join("results/local-only")).unwrap(),b"must survive empty restore");
}
#[test]
fn preview_and_output_disabled_do_not_publish() {
    for mode in ["build-dev","disabled"] {
        let (_temporary,root)=setup("sync-diff","always","true");
        if mode=="disabled" {
            let config=fs::read_to_string(root.join("_proj.yml")).unwrap();
            fs::write(root.join("_proj.yml"),config.replace("build:\n","build:\n  output_run: false\n")).unwrap();
        }
        success(invoke(&root,if mode=="disabled"{"build"}else{"build-dev"}));
        assert!(!root.join("remote").exists());
    }
}
#[test]
fn collisions_and_unsupported_inspection_fail_before_hooks_or_any_remote_write() {
    for edit in ["collision","append-false","overlap","invalid-inspect","invalid-layout","conflict"] {
        let (_temporary,root)=setup("sync-diff","always","[output]");
        fs::write(root.join("start.py"),"from pathlib import Path\nPath('hook-ran').write_text('bad')\n").unwrap();
        let mut config=fs::read_to_string(root.join("_proj.yml")).unwrap().replace("build:\n","build:\n  hooks: {pre: [start.py]}\n");
        config=match edit {
            "collision"=>config.replace("  raw: {path: input}","  raw: {path: input}\n  output-empty: {path: other}").replace("content: [output]","content: [output, output-empty]"),
            "append-false"=>config.replace("structure: latest","structure: latest\n      path-append-label: false"),
            "overlap"=>config.replace("path: remote","path: results"),
            "invalid-inspect"=>config.replace("inspect: file","inspect: invalid"),
            "invalid-layout"=>config.replace("structure: latest","structure: invalid"),
            _=>config.replace("inspect: file","inspect: file\n        conflict: overwrite"),
        };
        fs::write(root.join("_proj.yml"),config).unwrap();let before=fs::read(root.join("VERSION")).unwrap();
        let output=invoke(&root,"build");assert!(!output.status.success(),"{edit}");
        assert!(!root.join("hook-ran").exists(),"{edit}");assert!(!root.join("remote").exists(),"{edit}");
        assert_eq!(fs::read(root.join("VERSION")).unwrap(),before);assert!(!root.join("manifest.csv").exists());
    }
}
#[test]
fn malformed_remote_history_preserves_remote_bytes_and_successful_local_release() {
    let (_temporary,root)=setup("sync-diff","always","[output]");
    fs::create_dir_all(root.join("remote/output")).unwrap();fs::write(root.join("remote/output/old"),"old remote").unwrap();
    fs::write(root.join("remote/manifest.csv"),"invalid metadata").unwrap();
    let output=invoke(&root,"build");assert!(!output.status.success());
    assert_eq!(fs::read(root.join("remote/output/old")).unwrap(),b"old remote");assert!(!root.join("remote/output/new").exists());
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap().trim(),"Version: v1.2.4");
    assert!(String::from_utf8_lossy(&output.stderr).contains("local release preserved"));
}

#[test]
fn equal_bytes_missing_label_or_untrusted_metadata_are_repaired_after_file_inspection() {
    for prior in [None,Some("Project: v0.1.0\n"),Some("Project: v0.1.0\noutput: v0.1.0*\n")] {
        let (_temporary,root)=setup("sync-diff","if-change","[output]");
        fs::create_dir_all(root.join("remote/output")).unwrap();
        for entry in fs::read_dir(root.join("results")).unwrap() {
            let path=entry.unwrap().path();fs::copy(&path,root.join("remote/output").join(path.file_name().unwrap())).unwrap();
        }
        if let Some(prior)=prior {fs::write(root.join("remote/VERSION"),prior).unwrap();}
        success(invoke(&root,"build"));
        let text=fs::read_to_string(root.join("remote/VERSION")).unwrap();
        assert!(text.contains("output: v1.2.4\n"),"{text}");assert!(!text.contains('*'));
    }
}
#[test]
fn planned_case_alias_destinations_fail_before_creating_either_namespace() {
    let (_temporary,root)=setup("sync-diff","always","[output]");
    let config=fs::read_to_string(root.join("_proj.yml")).unwrap();
    let second="\n    second:\n      path: REMOTE\n      structure: latest\n      content: [raw]\n      send: {inspect: file}\n";
    fs::write(root.join("_proj.yml"),format!("{config}{second}")).unwrap();
    let output=invoke(&root,"build");assert!(!output.status.success());
    assert!(!root.join("remote").exists());assert!(!root.join("REMOTE").exists());
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(),"v1.2.3\n");
}
#[cfg(unix)]
#[test]
fn new_destinations_retain_selected_source_root_and_nested_directory_privacy() {
    use std::os::unix::fs::PermissionsExt;
    let (_temporary,root)=setup("sync-diff","always","[output]");
    fs::create_dir(root.join("results/private")).unwrap();fs::write(root.join("results/private/secret"),"private bytes").unwrap();
    for (path,mode) in [("results",0o700),("results/private",0o750)] {fs::set_permissions(root.join(path),fs::Permissions::from_mode(mode)).unwrap();}
    success(invoke(&root,"build"));
    assert_eq!(fs::metadata(root.join("remote/output")).unwrap().permissions().mode()&0o777,0o700);
    assert_eq!(fs::metadata(root.join("remote/output/private")).unwrap().permissions().mode()&0o777,0o750);
}
#[test]
fn all_destination_history_is_validated_before_first_destination_changes() {
    let (_temporary,root)=setup("sync-diff","always","[output]");
    let config=fs::read_to_string(root.join("_proj.yml")).unwrap();
    fs::write(root.join("_proj.yml"),format!("{config}\n    second:\n      path: broken-remote\n      structure: latest\n      content: [raw]\n      send: {{inspect: file}}\n")).unwrap();
    fs::create_dir(root.join("broken-remote")).unwrap();fs::write(root.join("broken-remote/manifest.csv"),"bad").unwrap();
    let output=invoke(&root,"build");assert!(!output.status.success());
    assert!(!root.join("remote").exists());
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap().trim(),"Version: v1.2.4");
}

#[test]
fn planned_unicode_normalization_aliases_fail_before_hooks_or_namespace_creation() {
    let (_temporary,root)=setup("sync-diff","always","[output]");
    let composed="remote-\u{e9}";let decomposed="remote-e\u{301}";
    let config=fs::read_to_string(root.join("_proj.yml")).unwrap().replace("path: remote",&format!("path: {composed}"))
        .replace("build:\n","build:\n  hooks: {pre: [start.py]}\n");
    fs::write(root.join("start.py"),"from pathlib import Path\nPath('hook-ran').write_text('bad')\n").unwrap();
    fs::write(root.join("_proj.yml"),format!("{config}\n    second:\n      path: {decomposed}\n      structure: latest\n      content: [raw]\n      send: {{inspect: file}}\n")).unwrap();
    let output=invoke(&root,"build");assert!(!output.status.success());
    assert!(!root.join("hook-ran").exists());assert!(!root.join(composed).exists());assert!(!root.join(decomposed).exists());
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(),"v1.2.3\n");
}

#[test]
fn later_preparation_failure_cleans_readonly_snapshots_on_every_platform() {
    let (_temporary,root)=setup("sync-diff","always","[output]");
    let config=fs::read_to_string(root.join("_proj.yml")).unwrap();
    fs::write(root.join("_proj.yml"),format!("{config}\n    second:\n      path: broken-remote\n      structure: latest\n      content: [raw]\n      send: {{inspect: file}}\n")).unwrap();
    fs::create_dir(root.join("broken-remote")).unwrap();fs::write(root.join("broken-remote/manifest.csv"),"bad").unwrap();
    let source=root.join("results/same");let original=fs::metadata(&source).unwrap().permissions();
    let mut readonly=original.clone();readonly.set_readonly(true);fs::set_permissions(&source,readonly).unwrap();
    let private=root.join("private-temp");fs::create_dir(&private).unwrap();
    let output=Command::new(env!("CARGO_BIN_EXE_proj")).current_dir(&root).args(["build","--non-interactive","--project"]).arg(&root)
        .env("TMPDIR",&private).env("TMP",&private).env("TEMP",&private)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT").output().unwrap();
    fs::set_permissions(&source,original).unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Cannot read manifest history"));
    assert!(!root.join("remote").exists());
    let staging:Vec<_>=fs::read_dir(&private).unwrap().filter_map(|entry| {
        let entry=entry.unwrap();entry.file_name().to_string_lossy().starts_with("proj-publication-").then_some(entry.path())
    }).collect();
    assert!(staging.is_empty(),"retained publication snapshots: {staging:?}");
}

#[test]
fn legacy_data_remote_namespaces_fail_before_hooks_and_preserve_history() {
    for state in ["directory","empty","advertisement","combined"] {
        let (_temporary,root)=setup("sync-diff","always","[output-r-data]");
        let config=fs::read_to_string(root.join("_proj.yml")).unwrap()
            .replace("directories:\n","directories:\n  output-r-data: {}\n")
            .replace("build:\n","build:\n  hooks: {pre: [start.py]}\n");
        fs::write(root.join("_proj.yml"),config).unwrap();
        fs::write(root.join("start.py"),"from pathlib import Path\nPath('hook-ran').write_text('bad')\n").unwrap();
        fs::create_dir(root.join("remote")).unwrap();
        if state=="directory"||state=="combined" {
            fs::create_dir(root.join("remote/data")).unwrap();
            fs::write(root.join("remote/data/old"),"historical bytes").unwrap();
        }
        if state=="empty" {fs::create_dir(root.join("remote/data-empty")).unwrap();}
        let version=if state=="advertisement"||state=="combined" {"Project: v0.1.0\ndata: v0.1.0\n"}else{"Project: v0.1.0\n"};
        fs::write(root.join("remote/VERSION"),version).unwrap();
        let before=fs::read(root.join("VERSION")).unwrap();
        let output=invoke(&root,"build");
        assert!(!output.status.success(),"{state}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("Legacy data"),"{}",String::from_utf8_lossy(&output.stderr));
        assert!(!root.join("hook-ran").exists());
        assert_eq!(fs::read(root.join("VERSION")).unwrap(),before);
        assert_eq!(fs::read_to_string(root.join("remote/VERSION")).unwrap(),version);
        assert!(!root.join("manifest.csv").exists());
        assert!(!root.join("remote/output-r-data").exists());
        if state=="directory"||state=="combined" {assert_eq!(fs::read(root.join("remote/data/old")).unwrap(),b"historical bytes");}
    }
}

#[test]
fn late_legacy_advertisement_aborts_all_targets_before_publication() {
    let (_temporary,root)=setup("sync-diff","always","[output]");
    let config=fs::read_to_string(root.join("_proj.yml")).unwrap()
        .replace("directories:\n","directories:\n  output-r-data: {}\n")
        .replace("build:\n","build:\n  hooks: {pre: [start.py]}\n")
        + "    later:\n      path: later-remote\n      structure: latest\n      content: [output-r-data]\n      send: {inspect: file}\n";
    fs::write(root.join("_proj.yml"),config).unwrap();
    fs::write(root.join("start.py"),"from pathlib import Path\np=Path('later-remote')\np.mkdir()\n(p/'VERSION').write_bytes(b'Project: v0.1.0\\ndata: v0.1.0\\n')\n").unwrap();
    let output=invoke(&root,"build");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Legacy data"),"{}",String::from_utf8_lossy(&output.stderr));
    assert!(!root.join("remote").exists(),"first destination must not publish");
    assert!(!root.join("later-remote/output-r-data").exists());
    assert_eq!(fs::read_to_string(root.join("later-remote/VERSION")).unwrap(),"Project: v0.1.0\ndata: v0.1.0\n");
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap().trim(),"Version: v1.2.4");
    assert!(root.join("manifest.csv").is_file(),"successful local release is preserved");
}

#[test]
fn no_inspection_all_strategies_and_cues_preserve_source_planning_semantics() {
    for strategy in ["upload-all","upload-missing","sync-diff","sync-purge","default"] {
        for cue in ["always","if-change"] {
            let (_temporary,root)=setup(if strategy=="default" {"upload-all"}else{strategy},cue,"[output]");
            let mut config=fs::read_to_string(root.join("_proj.yml")).unwrap().replace("inspect: file","inspect: none");
            if strategy=="default" {config=config.replace("        strategy: upload-all\n","");}
            fs::write(root.join("_proj.yml"),config).unwrap();
            fs::create_dir_all(root.join("remote/output")).unwrap();
            fs::write(root.join("remote/output/changed"),"old").unwrap();
            fs::write(root.join("remote/output/extra"),"keep unless nonempty purge").unwrap();
            fs::write(root.join("remote/VERSION"),"Project: v0.1.0\noutput: v0.1.0\n").unwrap();
            success(invoke(&root,"build"));
            let mut expected=files(&root.join("results"));
            if strategy!="sync-purge" {expected.insert("extra".into(),b"keep unless nonempty purge".to_vec());}
            assert_eq!(files(&root.join("remote/output")),expected);
            let star=if strategy=="sync-purge" {""}else{"*"};
            assert!(fs::read_to_string(root.join("remote/VERSION")).unwrap().contains(&format!("output: v1.2.4{star}\n")));
            // Equal source bytes are still selected blindly on the next build.
            success(invoke(&root,"build"));
            assert!(fs::read_to_string(root.join("remote/VERSION")).unwrap().contains(&format!("output: v1.2.5{star}\n")));
            fs::write(root.join("results/changed"),"damaged").unwrap();
            fs::write(root.join("results/local-only"),"remove").unwrap();
            let restored=restore(&root);
            assert_eq!(restored["result"]["restored"],true);
            assert_eq!(files(&root.join("results")),expected);
        }
    }
}
#[test]
fn no_inspection_empty_source_retains_unknown_extras_even_for_purge() {
    for strategy in ["upload-all","upload-missing","sync-diff","sync-purge"] {
        for existing in [false,true] {
            let (_temporary,root)=setup(strategy,"if-change","[output]");
            let config=fs::read_to_string(root.join("_proj.yml")).unwrap().replace("inspect: file","inspect: none");
            fs::write(root.join("_proj.yml"),config).unwrap();
            for name in ["same","changed","new"] {fs::remove_file(root.join("results").join(name)).unwrap();}
            if existing {
                fs::create_dir_all(root.join("remote/output")).unwrap();
                fs::write(root.join("remote/output/unknown"),"preserve without comparison").unwrap();
                fs::write(root.join("remote/VERSION"),"Project: v0.1.0\noutput: v0.1.0\n").unwrap();
            }
            success(invoke(&root,"build"));
            let version=fs::read_to_string(root.join("remote/VERSION")).unwrap();
            if existing {
                assert_eq!(fs::read(root.join("remote/output/unknown")).unwrap(),b"preserve without comparison");
                assert!(!root.join("remote/output-empty").exists());
                assert!(version.contains(if strategy=="sync-diff" {"output: v1.2.4*\n"}else{"output: v0.1.0\n"}),"{version}");
            } else {
                assert!(root.join("remote/output-empty").is_dir());
                assert!(version.contains("output: v1.2.4\n"),"{version}");
            }
        }
    }
}

#[test]
fn manifest_default_trusts_matching_history_and_preserves_unselected_actual_bytes() {
    for explicit in [false,true] {
        let (_temporary,root)=setup("sync-diff","if-change","[output]");
        let mut config=fs::read_to_string(root.join("_proj.yml")).unwrap();
        config=if explicit {config.replace("inspect: file","inspect: manifest")}else{config.replace("        inspect: file\n","")};
        fs::write(root.join("_proj.yml"),config).unwrap();
        success(invoke(&root,"build"));
        // History agrees, so a same-record file is intentionally not rehashed
        // for the decision even if actual bytes have independently drifted.
        fs::write(root.join("remote/output/same"),"remote drift").unwrap();
        fs::write(root.join("results/changed"),"changed again").unwrap();
        success(invoke(&root,"build"));
        assert_eq!(fs::read(root.join("remote/output/same")).unwrap(),b"remote drift");
        assert_eq!(fs::read(root.join("remote/output/changed")).unwrap(),b"changed again");
        assert!(fs::read_to_string(root.join("remote/VERSION")).unwrap().contains("output: v1.2.5\n"));
        // No source change keeps the prior label version with if-change.
        success(invoke(&root,"build"));
        assert!(fs::read_to_string(root.join("remote/VERSION")).unwrap().contains("output: v1.2.5\n"));
    }
}
#[test]
fn manifest_untrusted_missing_or_mismatched_history_falls_back_to_physical_bytes() {
    for state in ["star","missing-local","missing-remote","mismatch","duplicate"] {
        let (_temporary,root)=setup("sync-diff","if-change","[output]");
        let config=fs::read_to_string(root.join("_proj.yml")).unwrap().replace("inspect: file","inspect: manifest");
        fs::write(root.join("_proj.yml"),config).unwrap();
        success(invoke(&root,"build"));
        fs::write(root.join("remote/output/same"),"remote drift").unwrap();
        match state {
            "star"=>{fs::write(root.join("remote/VERSION"),"Project: v1.2.4\noutput: v1.2.4*\n").unwrap();},
            "missing-local"=>{fs::remove_file(root.join("manifest.csv")).unwrap();},
            "missing-remote"=>{fs::remove_file(root.join("remote/manifest.csv")).unwrap();},
            "mismatch"|"duplicate"=>{
                let path=root.join("remote/manifest.csv");
                let text=fs::read_to_string(&path).unwrap();
                let text=if state=="mismatch" {text.replace("51037a4a37730f52c8732586d3aaa316","900150983cd24fb0d6963f7d28e17f72")}else{
                    format!("{text}{}",text.lines().nth(1).unwrap().to_owned()+"\n")
                };
                fs::write(path,text).unwrap();
            },
            _=>unreachable!(),
        }
        success(invoke(&root,"build"));
        assert_eq!(fs::read(root.join("remote/output/same")).unwrap(),b"same","{state}");
        assert!(fs::read_to_string(root.join("remote/VERSION")).unwrap().contains("output: v1.2.5\n"),"{state}");
    }
}

#[test]
fn trusted_manifest_all_strategies_and_cues_apply_only_their_selected_actions() {
    for strategy in ["upload-all","upload-missing","sync-diff","sync-purge"] {
        for cue in ["always","if-change"] {
            let (_temporary,root)=setup(strategy,cue,"[output]");
            let config=fs::read_to_string(root.join("_proj.yml")).unwrap().replace("inspect: file","inspect: manifest");
            fs::write(root.join("_proj.yml"),config).unwrap();
            success(invoke(&root,"build"));
            fs::write(root.join("remote/output/same"),"drift").unwrap();
            fs::write(root.join("remote/output/unknown"),"unrecorded").unwrap();
            fs::write(root.join("results/changed"),"changed again").unwrap();
            fs::write(root.join("results/additional"),"added").unwrap();
            fs::remove_file(root.join("results/new")).unwrap();
            success(invoke(&root,"build"));
            let mut expected=BTreeMap::from([
                ("additional".into(),b"added".to_vec()),
                ("changed".into(),if strategy=="upload-missing" {b"new".to_vec()}else{b"changed again".to_vec()}),
                ("same".into(),if strategy=="upload-missing"||strategy=="sync-diff" {b"drift".to_vec()}else{b"same".to_vec()}),
            ]);
            if strategy!="sync-purge" {expected.insert("unknown".into(),b"unrecorded".to_vec());}
            if strategy.starts_with("upload-") {expected.insert("new".into(),b"new".to_vec());}
            assert_eq!(files(&root.join("remote/output")),expected,"{strategy}/{cue}");
            let star=if strategy.starts_with("upload-") {"*"}else{""};
            assert!(fs::read_to_string(root.join("remote/VERSION")).unwrap().contains(&format!("output: v1.2.5{star}\n")));
            let result=restore(&root);
            assert_eq!(result["result"]["restored"],true);
            assert_eq!(files(&root.join("results")),expected);
        }
    }
}
#[test]
fn trusted_manifest_unknown_shape_conflict_preserves_remote_and_local_release() {
    let (_temporary,root)=setup("sync-diff","if-change","[output]");
    let config=fs::read_to_string(root.join("_proj.yml")).unwrap().replace("inspect: file","inspect: manifest");
    fs::write(root.join("_proj.yml"),config).unwrap();
    success(invoke(&root,"build"));
    fs::create_dir(root.join("remote/output/folder")).unwrap();
    fs::write(root.join("remote/output/folder/nested"),"unknown remote").unwrap();
    fs::write(root.join("results/folder"),"incoming file").unwrap();
    let history=fs::read(root.join("remote/manifest.csv")).unwrap();
    let version=fs::read(root.join("remote/VERSION")).unwrap();
    let output=invoke(&root,"build");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("actual destination shape"),"{}",String::from_utf8_lossy(&output.stderr));
    assert_eq!(fs::read(root.join("remote/output/folder/nested")).unwrap(),b"unknown remote");
    assert_eq!(fs::read(root.join("remote/output/changed")).unwrap(),b"new");
    assert_eq!(fs::read(root.join("remote/manifest.csv")).unwrap(),history);
    assert_eq!(fs::read(root.join("remote/VERSION")).unwrap(),version);
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap().trim(),"Version: v1.2.5");
}

#[test]
fn archive_all_inspections_strategies_and_cues_preserve_snapshots_and_restore() {
    for inspection in ["file","manifest","none"] {
        for strategy in ["upload-all","upload-missing","sync-diff","sync-purge"] {
            for cue in ["always","if-change"] {
                let (_temporary,root)=setup(strategy,cue,"[output]");
                let config=fs::read_to_string(root.join("_proj.yml")).unwrap()
                    .replace("structure: latest","structure: archive").replace("inspect: file",&format!("inspect: {inspection}"));
                fs::write(root.join("_proj.yml"),config).unwrap();
                success(invoke(&root,"build"));
                let original=files(&root.join("remote/output/v1.2.4"));
                assert_eq!(original,files(&root.join("results")));
                success(invoke(&root,"build"));
                let reuse=inspection!="none"&&cue=="if-change"&&(strategy=="sync-diff"||strategy=="upload-missing");
                assert_eq!(root.join("remote/output/v1.2.5").exists(),!reuse,"{inspection}/{strategy}/{cue}");
                assert_eq!(files(&root.join("remote/output/v1.2.4")),original);
                fs::write(root.join("results/changed"),"next release").unwrap();
                success(invoke(&root,"build"));
                let expected=files(&root.join("results"));
                assert_eq!(files(&root.join("remote/output/v1.2.6")),expected);
                assert_eq!(files(&root.join("remote/output/v1.2.4")),original);
                fs::write(root.join("results/changed"),"damaged").unwrap();
                let restored=restore(&root);
                assert_eq!(restored["result"]["version"],"v1.2.6");
                assert_eq!(files(&root.join("results")),expected);
            }
        }
    }
}
#[test]
fn default_archive_layout_records_empty_and_nonempty_versions_without_erasing_history() {
    let (_temporary,root)=setup("sync-diff","if-change","[output]");
    let config=fs::read_to_string(root.join("_proj.yml")).unwrap()
        .replace("      structure: latest\n","").replace("        inspect: file\n","");
    fs::write(root.join("_proj.yml"),config).unwrap();
    for name in ["same","changed","new"] {fs::remove_file(root.join("results").join(name)).unwrap();}
    success(invoke(&root,"build"));
    assert!(root.join("remote/output/v1.2.4-empty").is_dir());
    success(invoke(&root,"build"));
    assert!(!root.join("remote/output/v1.2.5-empty").exists());
    fs::write(root.join("results/now"),"nonempty").unwrap();
    success(invoke(&root,"build"));
    assert!(root.join("remote/output/v1.2.4-empty").is_dir());
    assert_eq!(fs::read(root.join("remote/output/v1.2.6/now")).unwrap(),b"nonempty");
}

#[test]
fn changed_then_reverted_archive_history_requires_a_new_complete_snapshot() {
    let (_temporary,root)=setup("sync-diff","if-change","[output]");
    let config=fs::read_to_string(root.join("_proj.yml")).unwrap().replace("structure: latest","structure: archive");
    fs::write(root.join("_proj.yml"),config).unwrap();
    success(invoke(&root,"build"));
    let original=files(&root.join("remote/output/v1.2.4"));
    fs::write(root.join("results/changed"),"intermediate").unwrap();success(invoke(&root,"build"));
    fs::write(root.join("results/changed"),"new").unwrap();success(invoke(&root,"build"));
    assert_eq!(files(&root.join("remote/output/v1.2.6")),original);
    assert_eq!(fs::read(root.join("remote/output/v1.2.5/changed")).unwrap(),b"intermediate");
}
#[test]
fn archive_untrusted_or_drifted_old_comparison_never_produces_a_partial_snapshot() {
    for state in ["star","drift","mismatch","future"] {
        let (_temporary,root)=setup("sync-diff","if-change","[output]");
        let config=fs::read_to_string(root.join("_proj.yml")).unwrap().replace("structure: latest","structure: archive");
        fs::write(root.join("_proj.yml"),config).unwrap();success(invoke(&root,"build"));
        match state {
            "star"=>fs::write(root.join("remote/VERSION"),"Project: v1.2.4\noutput: v1.2.4*\n").unwrap(),
            "drift"=>fs::write(root.join("remote/output/v1.2.4/same"),"remote drift").unwrap(),
            "mismatch"=>fs::write(root.join("remote/VERSION"),"Project: v1.2.4\noutput: v1.2.3\n").unwrap(),
            _=>{
                fs::create_dir(root.join("remote/output/v9.0.0")).unwrap();
                fs::write(root.join("remote/output/v9.0.0/untouched"),"future").unwrap();
            },
        }
        success(invoke(&root,"build"));
        assert_eq!(files(&root.join("remote/output/v1.2.5")),files(&root.join("results")),"{state}");
        if state=="drift" {assert_eq!(fs::read(root.join("remote/output/v1.2.4/same")).unwrap(),b"remote drift");}
        if state=="future" {assert_eq!(fs::read(root.join("remote/output/v9.0.0/untouched")).unwrap(),b"future");}
    }
}
