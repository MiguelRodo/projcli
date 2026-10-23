use std::{collections::BTreeMap, fs, path::Path, process::Command};
use camino::Utf8Path;
use proj::local_remote::{list_versions, LocalStructure};

fn snapshot(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn walk(root: &Path, at: &Path, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
        for entry in fs::read_dir(at).unwrap() {
            let entry = entry.unwrap(); let path = entry.path();
            let key = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            if path.is_dir() { out.insert(key, None); walk(root, &path, out); }
            else { out.insert(key, Some(fs::read(path).unwrap())); }
        }
    }
    let mut out = BTreeMap::new(); walk(root, root, &mut out); out
}
fn invoke(root: &Path, structure: &str, append: bool) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_proj")).args(["--machine", "--project"]).arg(root)
        .args(["remote", "versions", "--path", "remote", "--label", "raw-data",
            "--structure", structure, "--path-append-label", if append { "true" } else { "false" }])
        .current_dir(root).env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap()
}
fn query(output: &std::process::Output) -> serde_json::Value {
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], output.status.success());
    value
}

#[test]
fn archive_cli_orders_numeric_versions_empty_variants_and_unicode_without_mutation() {
    let temp = tempfile::tempdir().unwrap(); let physical = temp.path().canonicalize().unwrap(); let root = physical.as_path();
    let base = root.join("remote/raw-data");
    for name in ["v1.9.0-empty", "v1.10.0", "v1.10.0-1", "v1.10.0.1-empty", "notes"] {
        fs::create_dir_all(base.join(name)).unwrap();
    }
    fs::write(base.join("v1.10.0/donn\u{00e9}es,\u{6570}.csv"), b"abc").unwrap();
    fs::write(root.join("remote/VERSION"), b"raw-data: v0.0.1*\n").unwrap();
    let before = snapshot(root);
    let output = invoke(root, "archive", true); assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let value = query(&output); let versions = value["result"]["versions"].as_array().unwrap();
    assert_eq!(versions.iter().map(|v| v["version"].as_str().unwrap()).collect::<Vec<_>>(),
        ["v1.10.0.1", "v1.10.0.1", "v1.10.0", "v1.9.0"]);
    assert_eq!(versions[0]["empty"], false); assert_eq!(versions[1]["empty"], true);
    assert!(versions.iter().all(|v| v["present"] == true && v["metadata_untrusted"].is_null()));
    assert!(versions[0]["path"].as_str().unwrap().ends_with("v1.10.0-1"));
    assert_eq!(snapshot(root), before);
}

#[test]
fn latest_cli_discloses_metadata_marker_empty_and_missing_targets() {
    let temp = tempfile::tempdir().unwrap(); let physical = temp.path().canonicalize().unwrap(); let root = physical.as_path();
    fs::create_dir_all(root.join("remote/raw-data-empty")).unwrap();
    fs::write(root.join("remote/VERSION"), b"raw: v9.0.0\nraw-data: v1.2.3-4*\n").unwrap();
    let before = snapshot(root); let output = invoke(root, "latest", true);
    assert!(output.status.success()); let value = query(&output); let entry = &value["result"]["versions"][0];
    assert_eq!(entry["version"], "v1.2.3.4"); assert_eq!(entry["empty"], true);
    assert_eq!(entry["metadata_untrusted"], true); assert_eq!(entry["present"], true);
    assert_eq!(snapshot(root), before);
    fs::remove_dir(root.join("remote/raw-data-empty")).unwrap();
    let value = query(&invoke(root, "latest", true));
    assert_eq!(value["result"]["versions"][0]["present"], false);
    assert_eq!(value["result"]["versions"][0]["empty"], false);
    fs::write(root.join("remote/VERSION"), b"raw-data: v1.2.3\nraw-data: v2.0.0\n").unwrap();
    assert!(!invoke(root, "latest", true).status.success());
    fs::write(root.join("remote/VERSION"), b"raw-data: ../../elsewhere\n").unwrap();
    assert!(!invoke(root, "latest", true).status.success());
}

#[test]
fn absent_remote_is_empty_not_created_and_no_append_reads_explicit_archive_root() {
    let temp = tempfile::tempdir().unwrap(); let physical = temp.path().canonicalize().unwrap(); let root = physical.as_path();
    for structure in ["latest", "archive"] {
        let output = invoke(root, structure, true); assert!(output.status.success());
        assert_eq!(query(&output)["result"]["versions"], serde_json::json!([]));
        assert!(snapshot(root).is_empty());
    }
    fs::create_dir_all(root.join("remote/v1.2")).unwrap();
    let output = invoke(root, "archive", false); assert!(output.status.success());
    assert_eq!(query(&output)["result"]["versions"][0]["version"], "v1.2");
    fs::create_dir(root.join("remote/v1.2-invalid")).unwrap();
    assert!(!invoke(root, "archive", false).status.success());
}

#[test]
fn traversal_labels_fail_without_mutation() {
    let temp = tempfile::tempdir().unwrap(); let physical = temp.path().canonicalize().unwrap();
    let base = Utf8Path::from_path(&physical).unwrap();
    for label in ["../escape", "/absolute", "a/b"] {
        assert!(list_versions(base, label, LocalStructure::Archive, true).is_err());
    }
    assert!(snapshot(&physical).is_empty());
}

#[cfg(unix)]
#[test]
fn linked_candidates_and_version_metadata_are_rejected_without_following() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap(); let physical = temp.path().canonicalize().unwrap();
    let base_path = physical.join("remote"); let base = Utf8Path::from_path(&base_path).unwrap();
    fs::create_dir_all(base_path.join("raw-data")).unwrap();
    fs::create_dir(physical.join("outside")).unwrap();
    fs::write(physical.join("outside/sentinel"), b"untouched").unwrap();
    symlink(physical.join("outside"), base_path.join("raw-data/v1.2.3")).unwrap();
    assert!(list_versions(base, "raw-data", LocalStructure::Archive, true).is_err());
    symlink(physical.join("outside/sentinel"), base_path.join("VERSION")).unwrap();
    assert!(list_versions(base, "raw-data", LocalStructure::Latest, true).is_err());
    assert_eq!(fs::read(physical.join("outside/sentinel")).unwrap(), b"untouched");
    fs::remove_file(base_path.join("raw-data/v1.2.3")).unwrap();
    assert!(Command::new("mkfifo").arg(base_path.join("raw-data/v1.2.3")).status().unwrap().success());
    assert!(list_versions(base, "raw-data", LocalStructure::Archive, true).is_err());
}

#[test]
fn human_help_and_absolute_path_query_agree_with_machine_command() {
    let temp = tempfile::tempdir().unwrap();
    let physical = temp.path().canonicalize().unwrap();
    fs::create_dir_all(physical.join("remote/raw-data/v1.2.3")).unwrap();
    let help = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["remote", "versions", "--help"]).output().unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    for option in ["--path", "--label", "--structure", "--path-append-label"] {
        assert!(help.contains(option), "{help}");
    }
    let before = snapshot(&physical);
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .arg("--project").arg(&physical)
        .args(["remote", "versions", "--path"]).arg(physical.join("remote"))
        .args(["--label", "raw-data"]).current_dir(&physical)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("v1.2.3 "));
    assert!(text.contains("raw-data"));
    assert_eq!(snapshot(&physical), before);
}

#[test]
fn labels_ending_empty_remain_valid_explicit_label_names() {
    let temp = tempfile::tempdir().unwrap();
    let physical = temp.path().canonicalize().unwrap();
    fs::create_dir_all(physical.join("raw-empty/v1.2.3")).unwrap();
    let base = Utf8Path::from_path(&physical).unwrap();
    let entries = list_versions(base, "raw-empty", LocalStructure::Archive, true).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(!entries[0].empty);
    fs::write(physical.join("VERSION"), b"raw-empty: v1.2.3\n").unwrap();
    let entries = list_versions(base, "raw-empty", LocalStructure::Latest, true).unwrap();
    assert!(entries[0].present);
    assert!(!entries[0].empty);
}

#[test]
fn latest_without_label_appending_reports_base_presence_not_sibling_marker() {
    let temp = tempfile::tempdir().unwrap();
    let physical = temp.path().canonicalize().unwrap();
    fs::create_dir(physical.join("remote")).unwrap();
    fs::create_dir(physical.join("remote-empty")).unwrap();
    fs::write(physical.join("remote/VERSION"), b"raw-data: v1.2.3*\n").unwrap();
    let before = snapshot(&physical);
    let output = invoke(&physical, "latest", false);
    assert!(output.status.success());
    let result = query(&output);
    let entry = &result["result"]["versions"][0];
    assert_eq!(entry["present"], true);
    assert_eq!(entry["empty"], false);
    assert_eq!(entry["metadata_untrusted"], true);
    assert!(entry["path"].as_str().unwrap().ends_with("remote"));
    assert_eq!(snapshot(&physical), before);
}
