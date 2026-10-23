use std::{collections::BTreeMap, fs, path::Path, process::Command};
use camino::Utf8Path;
use proj::local_remote::{download_version, select_version, LocalStructure};

fn snapshot(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn walk(root: &Path, at: &Path, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
        for entry in fs::read_dir(at).unwrap() {
            let entry = entry.unwrap(); let path = entry.path();
            let key = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            if path.is_dir() { out.insert(key, None); walk(root, &path, out); }
            else { out.insert(key, Some(fs::read(path).unwrap())); }
        }
    }
    let mut result = BTreeMap::new(); walk(root, root, &mut result); result
}
fn invoke(root: &Path, structure: &str, version: Option<&str>, clear: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_proj"));
    command.args(["--machine", "--project"]).arg(root)
        .args(["remote", "download", "--path", "remote", "--label", "raw-data",
            "--structure", structure, "--destination", "downloaded"]);
    if let Some(version) = version { command.args(["--remote-version", version]); }
    if clear { command.arg("--clear"); }
    command.current_dir(root).env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap()
}
fn assert_success(output: &std::process::Output) {
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success(), "{json}\n{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(json["ok"], true);
    assert_eq!(json["result"]["completed"], true);
}

#[test]
fn archive_download_stages_nested_unicode_and_hidden_bytes_and_preserves_extras() {
    let temp = tempfile::tempdir().unwrap(); let root = temp.path().canonicalize().unwrap();
    let source = root.join("remote/raw-data/v1.2.3-4");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("nested/donn\u{00e9}es,\u{6570}.csv"), b"new exact bytes\n").unwrap();
    fs::write(source.join(".hidden"), b"hidden").unwrap();
    fs::write(source.join("shared"), b"updated").unwrap();
    fs::create_dir(root.join("downloaded")).unwrap();
    fs::write(root.join("downloaded/shared"), b"old").unwrap();
    fs::write(root.join("downloaded/only-local"), b"retained").unwrap();
    let remote_before = snapshot(&root.join("remote"));
    assert_success(&invoke(&root, "archive", Some("v1.2.3-4"), false));
    assert_eq!(fs::read(root.join("downloaded/shared")).unwrap(), b"updated");
    assert_eq!(fs::read(root.join("downloaded/only-local")).unwrap(), b"retained");
    assert_eq!(fs::read(root.join("downloaded/.hidden")).unwrap(), b"hidden");
    assert_eq!(fs::read(root.join("downloaded/nested/donn\u{00e9}es,\u{6570}.csv")).unwrap(), b"new exact bytes\n");
    assert_eq!(snapshot(&root.join("remote")), remote_before);
    assert!(!root.join("_proj.yml").exists());
    assert!(!root.join("VERSION").exists());
    assert!(fs::read_dir(&root).unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().starts_with(".proj-download-")));
}

#[test]
fn clear_is_success_only_and_empty_markers_do_not_select_a_false_data_source() {
    let temp = tempfile::tempdir().unwrap(); let root = temp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("remote/raw-data/v1.2.3")).unwrap();
    fs::write(root.join("remote/raw-data/v1.2.3/new"), b"replacement").unwrap();
    fs::create_dir(root.join("downloaded")).unwrap();
    fs::write(root.join("downloaded/previous"), b"keep until success").unwrap();
    let before = snapshot(&root);
    assert!(!invoke(&root, "archive", Some("v9.9.9"), true).status.success());
    assert_eq!(snapshot(&root), before);
    assert_success(&invoke(&root, "archive", Some("v1.2.3"), true));
    assert_eq!(snapshot(&root.join("downloaded")), BTreeMap::from([("new".into(), Some(b"replacement".to_vec()))]));
    fs::create_dir(root.join("remote/raw-data/v2.0.0-empty")).unwrap();
    assert_success(&invoke(&root, "archive", Some("v2.0.0"), false));
    assert!(root.join("downloaded/new").exists());
    assert_success(&invoke(&root, "archive", Some("v2.0.0"), true));
    assert!(snapshot(&root.join("downloaded")).is_empty());
    fs::write(root.join("remote/raw-data/v2.0.0-empty/not-empty"), b"do not discard").unwrap();
    let before = snapshot(&root);
    assert!(!invoke(&root, "archive", Some("v2.0.0"), true).status.success());
    assert_eq!(snapshot(&root), before);
}

#[test]
fn latest_uses_exact_advertised_source_and_missing_or_invalid_requests_preserve_target() {
    let temp = tempfile::tempdir().unwrap(); let root = temp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("remote/raw-data")).unwrap();
    fs::write(root.join("remote/VERSION"), b"raw-data: v1.2.3*\n").unwrap();
    fs::write(root.join("remote/raw-data/result"), b"latest bytes").unwrap();
    assert_success(&invoke(&root, "latest", None, false));
    assert_eq!(fs::read(root.join("downloaded/result")).unwrap(), b"latest bytes");
    let before = snapshot(&root);
    assert!(!invoke(&root, "latest", Some("v1.2.3"), true).status.success());
    assert!(!invoke(&root, "archive", None, true).status.success());
    assert!(!invoke(&root, "archive", Some("../../escape"), true).status.success());
    assert_eq!(snapshot(&root), before);
    fs::remove_dir_all(root.join("remote/raw-data")).unwrap();
    let before = snapshot(&root);
    assert!(!invoke(&root, "latest", None, true).status.success());
    assert_eq!(snapshot(&root), before);
}

#[test]
fn staging_conflict_fails_before_touching_existing_destination() {
    let temp = tempfile::tempdir().unwrap(); let root = temp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("remote/raw-data/v1.2.3")).unwrap();
    fs::write(root.join("remote/raw-data/v1.2.3/conflict"), b"file conflicts with old directory").unwrap();
    fs::create_dir_all(root.join("downloaded/conflict")).unwrap();
    fs::write(root.join("downloaded/conflict/previous"), b"intact").unwrap();
    let before = snapshot(&root);
    assert!(!invoke(&root, "archive", Some("v1.2.3"), false).status.success());
    assert_eq!(snapshot(&root), before);
}

#[test]
fn protected_and_overlapping_targets_fail_before_mutation() {
    let temp = tempfile::tempdir().unwrap(); let root_path = temp.path().canonicalize().unwrap();
    let root = Utf8Path::from_path(&root_path).unwrap();
    fs::create_dir_all(root.join("remote/raw-data/v1.2.3")).unwrap();
    fs::write(root.join("remote/raw-data/v1.2.3/data"), b"source").unwrap();
    let version = select_version(&root.join("remote"), "raw-data", LocalStructure::Archive, Some("v1.2.3"), true).unwrap();
    let before = snapshot(&root_path);
    for destination in [root.to_owned(), root.join(".git/data"), root.join(".projr/data"),
        root.join("remote"), version.path.join("nested"), root.join("absent-parent/data")] {
        assert!(download_version(root, &version, &destination, true).is_err(), "{destination}");
    }
    assert_eq!(snapshot(&root_path), before);
}

#[cfg(unix)]
#[test]
fn links_special_entries_and_external_hardlink_aliases_are_preserved() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap(); let root = temp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("remote/raw-data/v1.2.3")).unwrap();
    fs::write(root.join("remote/raw-data/v1.2.3/data"), b"new bytes").unwrap();
    fs::create_dir(root.join("downloaded")).unwrap();
    fs::write(root.join("external"), b"external original").unwrap();
    fs::hard_link(root.join("external"), root.join("downloaded/data")).unwrap();
    assert_success(&invoke(&root, "archive", Some("v1.2.3"), false));
    assert_eq!(fs::read(root.join("external")).unwrap(), b"external original");
    assert_eq!(fs::read(root.join("downloaded/data")).unwrap(), b"new bytes");
    symlink(root.join("external"), root.join("remote/raw-data/v1.2.3/link")).unwrap();
    let previous = snapshot(&root.join("downloaded"));
    assert!(!invoke(&root, "archive", Some("v1.2.3"), true).status.success());
    assert_eq!(snapshot(&root.join("downloaded")), previous);
    fs::remove_file(root.join("remote/raw-data/v1.2.3/link")).unwrap();
    assert!(Command::new("mkfifo").arg(root.join("remote/raw-data/v1.2.3/pipe")).status().unwrap().success());
    assert!(!invoke(&root, "archive", Some("v1.2.3"), true).status.success());
    assert_eq!(snapshot(&root.join("downloaded")), previous);
    fs::remove_file(root.join("remote/raw-data/v1.2.3/pipe")).unwrap();
    symlink(root.join("external"), root.join("downloaded/link")).unwrap();
    assert!(!invoke(&root, "archive", Some("v1.2.3"), false).status.success());
    assert_eq!(fs::read(root.join("external")).unwrap(), b"external original");
}
