//! `proj restore` against projr-written legacy history and registered local
//! remotes (a documented projcli superset of projr's GitHub-only restore).
use camino::Utf8PathBuf;
use proj::restore::{RestoreRequest, restore};
use std::{fs, process::Command};

fn setup(history: &str) -> (tempfile::TempDir, Utf8PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = Utf8PathBuf::from_path_buf(temp.path().canonicalize().unwrap()).unwrap();
    fs::create_dir(root.join("data")).unwrap();
    fs::write(root.join("data/old.txt"), "previous").unwrap();
    fs::write(root.join("manifest.csv"), history).unwrap();
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    (temp, root)
}
fn request(destination: &str, clear: bool) -> RestoreRequest {
    RestoreRequest {
        destination: Some(destination.into()),
        clear,
        profile: Some("".into()),
        ..Default::default()
    }
}
fn remote(root: &camino::Utf8Path, base: &str, version: &str, bytes: &str) {
    fs::create_dir_all(root.join(base).join("raw").join(version)).unwrap();
    fs::write(
        root.join(base).join("raw").join(version).join("data.csv"),
        bytes,
    )
    .unwrap();
}
const RECORDED: &str =
    "label,fn,version,hash\nraw,data.csv,v1.2.3,900150983cd24fb0d6963f7d28e17f72\n";

#[test]
fn legacy_history_verifies_bytes_and_clear_replaces_only_after_staging() {
    let (_temp, root) = setup(RECORDED);
    remote(&root, "remote-A", "v1.2.3", "abc");
    remote(&root, "remote-B", "v1.2.3", "not the recorded bytes");
    fs::write(
        root.join("_projr.yml"),
        "directories:\n  raw:\n    path: data\nremotes:\n  local:\n    A:\n      path: remote-A\n    B:\n      path: remote-B\n",
    )
    .unwrap();
    // The extra local file is a local change.
    let error = restore(&root, &request("local:A", false)).unwrap_err();
    assert!(format!("{error:#}").contains("--clear"), "{error:#}");
    assert!(root.join("data/old.txt").exists());
    // Remote bytes that differ from the recorded MD5 never replace anything.
    let error = restore(&root, &request("local:B", true)).unwrap_err();
    assert!(
        format!("{error:#}").contains("No matching files for raw at v1.2.3"),
        "{error:#}"
    );
    assert!(root.join("data/old.txt").exists());
    let outcome = restore(&root, &request("local:A", true)).unwrap();
    assert_eq!(outcome.version.as_deref(), Some("v1.2.3"));
    assert_eq!(outcome.labels[0].status, "installed");
    assert!(!root.join("data/old.txt").exists());
    assert_eq!(fs::read(root.join("data/data.csv")).unwrap(), b"abc");
    // The projr-written legacy CSV is read, never rewritten.
    assert_eq!(
        fs::read(root.join("manifest.csv")).unwrap(),
        RECORDED.as_bytes()
    );
    // Unregistered local names are errors once a remote is needed.
    fs::remove_file(root.join("data/data.csv")).unwrap();
    let error = restore(&root, &request("local:missing", true)).unwrap_err();
    assert!(
        error.to_string().contains("Unknown local remote"),
        "{error}"
    );
}

#[test]
fn recorded_empty_directories_need_no_remote_and_missing_history_fails() {
    let (_temp, root) = setup("label,fn,version,hash\nraw,,v1.2.3,\n");
    fs::write(
        root.join("_projr.yml"),
        "directories:\n  raw:\n    path: data\n",
    )
    .unwrap();
    assert!(restore(&root, &request("local:unregistered", false)).is_err());
    assert!(root.join("data/old.txt").exists());
    let outcome = restore(&root, &request("local:unregistered", true)).unwrap();
    assert_eq!(outcome.labels[0].status, "installed");
    assert!(outcome.source.is_none());
    assert_eq!(fs::read_dir(root.join("data")).unwrap().count(), 0);

    // Without a recorded snapshot, no remote file list can be trusted.
    fs::write(root.join("manifest.csv"), "label,fn,version,hash\n").unwrap();
    fs::write(
        root.join("_projr.yml"),
        "directories:\n  raw:\n    path: data\nremotes:\n  local:\n    A:\n      path: remote\n",
    )
    .unwrap();
    remote(&root, "remote", "v1.2.3", "abc");
    let error = restore(&root, &request("local:A", true)).unwrap_err();
    assert!(error.to_string().contains("No matching files"), "{error}");
    fs::write(root.join("manifest.csv"), "bad").unwrap();
    assert!(restore(&root, &request("local:A", true)).is_err());
}

#[test]
fn non_raw_labels_are_restored_only_when_named_and_no_raw_label_is_a_no_op() {
    let (_temp, root) =
        setup("label,fn,version,hash\noutput,data.csv,v1.2.3,900150983cd24fb0d6963f7d28e17f72\n");
    fs::write(
        root.join("_projr.yml"),
        "directories:\n  output:\n    path: data\nremotes:\n  local:\n    A:\n      path: remote\n",
    )
    .unwrap();
    let outcome = restore(&root, &request("local:A", true)).unwrap();
    assert!(outcome.version.is_none() && outcome.labels.is_empty());
    fs::create_dir_all(root.join("remote/output/v1.2.3")).unwrap();
    fs::write(root.join("remote/output/v1.2.3/data.csv"), "abc").unwrap();
    let mut named = request("local:A", true);
    named.labels = vec!["output".into()];
    restore(&root, &named).unwrap();
    assert_eq!(fs::read(root.join("data/data.csv")).unwrap(), b"abc");
}

#[test]
fn child_profile_selection_matches_projr_key_and_explicit_precedence() {
    // QUARTO_PROFILE never selects projr profiles (projr R/profile.R:223-252).
    for (projr, quarto, explicit, expected) in [
        ("", "", None, "analysis"),
        ("", "quarto", None, "analysis"),
        ("quarto", "", None, "quarto"),
        ("first", "quarto", None, "first"),
        ("", "quarto", Some("analysis"), "analysis"),
        ("", "", Some("first,second"), "first"),
    ] {
        let (_temp, root) = setup(RECORDED);
        for name in ["base", "analysis", "quarto", "first", "second"] {
            fs::create_dir(root.join(format!("{name}-target"))).unwrap();
            fs::write(root.join(format!("{name}-target/old")), "intact").unwrap();
        }
        remote(&root, "store", "v1.2.3", "abc");
        fs::write(
            root.join("_projr.yml"),
            "directories:\n  raw:\n    path: base-target\nremotes:\n  local:\n    A:\n      path: store\nprofile: analysis\n",
        )
        .unwrap();
        for name in ["analysis", "quarto", "first", "second"] {
            fs::write(
                root.join(format!("_projr-{name}.yml")),
                format!("directories:\n  raw:\n    path: {name}-target\n"),
            )
            .unwrap();
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_proj"));
        command
            .env("PROJR_PROFILE", projr)
            .env("QUARTO_PROFILE", quarto)
            .env_remove(proj::PROJECT_ROOT_ENV)
            .env_remove(proj::RUN_CONTEXT_ENV)
            .args([
                "--project",
                root.as_str(),
                "--machine",
                "restore",
                "--destination",
                "local:A",
                "--clear",
            ]);
        if let Some(profile) = explicit {
            command.args(["--profile", profile]);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            value["result"]["restore"]["labels"][0]["path"],
            root.join(format!("{expected}-target")).as_str()
        );
        for name in ["base", "analysis", "quarto", "first", "second"] {
            if name == expected {
                assert_eq!(
                    fs::read(root.join(format!("{name}-target/data.csv"))).unwrap(),
                    b"abc"
                );
                assert!(!root.join(format!("{name}-target/old")).exists());
            } else {
                assert_eq!(
                    fs::read(root.join(format!("{name}-target/old"))).unwrap(),
                    b"intact"
                );
            }
        }
    }
}

#[test]
fn ordinary_remote_restore_requires_an_explicit_snapshot() {
    let (_temp, root) = setup(RECORDED);
    fs::write(
        root.join("_projr.yml"),
        "directories:\n  raw: {path: data}\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .env_remove(proj::PROJECT_ROOT_ENV)
        .env_remove(proj::RUN_CONTEXT_ENV)
        .args([
            "--project",
            root.as_str(),
            "remote",
            "restore",
            "--label",
            "raw",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--snapshot"));
}
