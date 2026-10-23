use std::{collections::BTreeMap, fs, path::Path, process::Command};
fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, at: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(at).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() { walk(root, &path, out); }
            else { out.insert(path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"), fs::read(path).unwrap()); }
        }
    }
    let mut result = BTreeMap::new(); walk(root, root, &mut result); result
}
fn invoke(root: &Path, strategy: &str, dry: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_proj"));
    command.args(["--machine", "--project"]).arg(root)
        .args(["remote", "store", "--source", "source", "--destination", "destination",
            "--inspect", "file", "--strategy", strategy]);
    if dry { command.arg("--dry-run"); }
    command.current_dir(root).env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV).output().unwrap()
}
fn json(output: &std::process::Output) -> serde_json::Value {
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success(), "{value}\n{}", String::from_utf8_lossy(&output.stderr));
    value
}
fn fixture(root: &Path) {
    fs::create_dir(root.join("source")).unwrap(); fs::create_dir(root.join("destination")).unwrap();
    for (name, bytes) in [("new", "new"), ("changed", "new"), ("same", "same")] {
        fs::write(root.join("source").join(name), bytes).unwrap();
    }
    for (name, bytes) in [("old", "old"), ("changed", "old"), ("same", "same")] {
        fs::write(root.join("destination").join(name), bytes).unwrap();
    }
    // Changed bytes have the same length and mtime, so MD5 must drive comparison.
    let time = std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000));
    for path in ["source/changed", "destination/changed"] {
        fs::File::options().write(true).open(root.join(path)).unwrap().set_times(time).unwrap();
    }
    fs::write(root.join("VERSION"), b"v7.8.9").unwrap();
    fs::write(root.join("manifest.csv"), b"unrelated history must remain untouched").unwrap();
}
#[test]
fn four_real_cli_policies_match_dry_run_and_exact_destination_bytes() {
    for (strategy, uploads, removals) in [
        ("upload-all", vec!["changed","new","same"], vec![]),
        ("upload-missing", vec!["new"], vec![]),
        ("sync-diff", vec!["changed","new"], vec!["old"]),
        ("sync-purge", vec!["changed","new","same"], vec!["changed","old","same"]),
    ] {
        let temp = tempfile::tempdir().unwrap(); let root = temp.path().canonicalize().unwrap();
        fixture(&root);
        let before = files(&root);
        let plan = json(&invoke(&root, strategy, true));
        assert_eq!(plan["result"]["plan"]["applied"], false);
        assert_eq!(plan["result"]["plan"]["uploads"], serde_json::json!(uploads));
        assert_eq!(plan["result"]["plan"]["removals"], serde_json::json!(removals));
        assert_eq!(plan["result"]["plan"]["same_bytes"], serde_json::json!(["same"]));
        assert_eq!(files(&root), before);
        let source_before = files(&root.join("source"));
        let result = json(&invoke(&root, strategy, false));
        assert_eq!(result["result"]["completed"], true);
        let mut expected = BTreeMap::from([
            ("changed".into(), if strategy == "upload-missing" { b"old".to_vec() } else { b"new".to_vec() }),
            ("new".into(), b"new".to_vec()), ("same".into(), b"same".to_vec())]);
        if strategy.starts_with("upload-") { expected.insert("old".into(), b"old".to_vec()); }
        assert_eq!(files(&root.join("destination")), expected);
        assert_eq!(files(&root.join("source")), source_before);
        assert_eq!(fs::read(root.join("VERSION")).unwrap(), b"v7.8.9");
        assert_eq!(fs::read(root.join("manifest.csv")).unwrap(), b"unrelated history must remain untouched");
        assert!(fs::read_dir(&root).unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().starts_with(".proj-")));
    }
}
#[test]
fn dry_run_missing_target_unicode_and_empty_source_create_no_state() {
    let temp = tempfile::tempdir().unwrap(); let root = temp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("source/nested")).unwrap();
    fs::write(root.join("source/nested/donn\u{00e9}es,\u{6570}.csv"), b"exact").unwrap();
    let before = files(&root);
    let plan = json(&invoke(&root, "sync-diff", true));
    assert_eq!(plan["result"]["plan"]["uploads"], serde_json::json!(["nested/donn\u{00e9}es,\u{6570}.csv"]));
    assert!(!root.join("destination").exists());
    assert_eq!(files(&root), before);
    json(&invoke(&root, "sync-diff", false));
    assert_eq!(files(&root.join("destination")), files(&root.join("source")));
    fs::remove_dir_all(root.join("source")).unwrap(); fs::create_dir(root.join("source")).unwrap();
    json(&invoke(&root, "upload-missing", false));
    assert!(!files(&root.join("destination")).is_empty());
    json(&invoke(&root, "sync-diff", false));
    assert!(files(&root.join("destination")).is_empty());
}
#[test]
fn file_directory_conflict_in_merge_is_preflight_error_and_mirror_can_replace_it() {
    let temp = tempfile::tempdir().unwrap(); let root = temp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("source/conflict")).unwrap();
    fs::write(root.join("source/conflict/new"), b"new").unwrap();
    fs::create_dir(root.join("destination")).unwrap();
    fs::write(root.join("destination/conflict"), b"old file").unwrap();
    let before = files(&root);
    for strategy in ["upload-all", "upload-missing"] {
        assert!(!invoke(&root, strategy, true).status.success());
        assert!(!invoke(&root, strategy, false).status.success());
        assert_eq!(files(&root), before);
    }
    json(&invoke(&root, "sync-diff", false));
    assert_eq!(fs::read(root.join("destination/conflict/new")).unwrap(), b"new");
}
#[test]
fn unsafe_targets_and_unsupported_inspection_fail_without_mutation() {
    let temp = tempfile::tempdir().unwrap(); let root = temp.path().canonicalize().unwrap(); fixture(&root);
    let before = files(&root);
    for target in [".", ".git/data", ".projr/data", "source/nested"] {
        let output = Command::new(env!("CARGO_BIN_EXE_proj")).arg("--project").arg(&root)
            .args(["remote","store","--source","source","--destination",target,"--inspect","file","--dry-run"])
            .current_dir(&root).output().unwrap();
        assert!(!output.status.success());
    }
    for inspection in [None, Some("manifest")] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_proj"));
        command.arg("--project").arg(&root).args(["remote","store","--source","source","--destination","destination"]);
        if let Some(inspection) = inspection { command.args(["--inspect",inspection]); }
        assert!(!command.current_dir(&root).output().unwrap().status.success());
    }
    assert_eq!(files(&root), before);
}
#[cfg(unix)]
#[test]
fn mirror_keeps_private_modes_and_hardlink_alias_bytes_and_rejects_links() {
    use std::os::unix::{fs::PermissionsExt, fs::symlink};
    let temp = tempfile::tempdir().unwrap(); let root = temp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("source/private")).unwrap(); fs::create_dir_all(root.join("destination/private")).unwrap();
    fs::write(root.join("source/private/data"), b"new").unwrap();
    fs::write(root.join("outside"), b"old").unwrap();
    fs::hard_link(root.join("outside"),root.join("destination/private/data")).unwrap();
    fs::set_permissions(root.join("destination"),fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(root.join("destination/private"),fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(root.join("destination/private/data"),fs::Permissions::from_mode(0o400)).unwrap();
    json(&invoke(&root, "sync-diff", false));
    for name in ["destination","destination/private"] {
        assert_eq!(fs::metadata(root.join(name)).unwrap().permissions().mode() & 0o777,0o700);
    }
    assert_eq!(fs::metadata(root.join("destination/private/data")).unwrap().permissions().mode() & 0o777,0o400);
    assert_eq!(fs::read(root.join("outside")).unwrap(),b"old");
    assert_eq!(fs::read(root.join("destination/private/data")).unwrap(),b"new");
    symlink(root.join("outside"),root.join("source/link")).unwrap();
    let before = files(&root.join("destination"));
    assert!(!invoke(&root,"sync-purge",false).status.success());
    assert_eq!(files(&root.join("destination")),before);
}

#[test]
fn native_filename_aliases_cannot_overwrite_missing_uploads_or_hide_overwrites() {
    for strategy in ["upload-missing", "upload-all", "sync-diff", "sync-purge"] {
        let temp = tempfile::tempdir().unwrap(); let root = temp.path().canonicalize().unwrap();
        fs::create_dir(root.join("source")).unwrap(); fs::create_dir(root.join("destination")).unwrap();
        fs::write(root.join("source/A.csv"), b"new bytes").unwrap();
        fs::write(root.join("destination/a.csv"), b"existing bytes").unwrap();
        let alias = root.join("destination/A.csv").exists();
        let before = files(&root);
        let dry = invoke(&root,strategy,true);
        if alias {
            assert!(!dry.status.success());
            assert!(!invoke(&root,strategy,false).status.success());
            assert_eq!(files(&root),before);
        } else {
            let plan = json(&dry);
            assert_eq!(plan["result"]["plan"]["uploads"],serde_json::json!(["A.csv"]));
            json(&invoke(&root,strategy,false));
            assert_eq!(fs::read(root.join("destination/A.csv")).unwrap(),b"new bytes");
            if strategy.starts_with("upload-") {
                assert_eq!(fs::read(root.join("destination/a.csv")).unwrap(),b"existing bytes");
            }
        }
    }
}
#[test]
fn native_parent_directory_alias_is_rejected_without_loosening_existing_tree() {
    let temp = tempfile::tempdir().unwrap(); let root = temp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("source/Private")).unwrap();
    fs::create_dir_all(root.join("destination/private")).unwrap();
    fs::write(root.join("source/Private/new"),b"new").unwrap();
    fs::write(root.join("destination/private/old"),b"old").unwrap();
    let before = files(&root);
    if root.join("destination/Private").exists() {
        assert!(!invoke(&root,"upload-all",true).status.success());
        assert!(!invoke(&root,"upload-all",false).status.success());
        assert_eq!(files(&root),before);
    } else {
        json(&invoke(&root,"upload-all",false));
        assert_eq!(fs::read(root.join("destination/private/old")).unwrap(),b"old");
        assert_eq!(fs::read(root.join("destination/Private/new")).unwrap(),b"new");
    }
}
