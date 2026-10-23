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

#[cfg(unix)]
#[test]
fn download_preserves_private_and_readonly_permission_flags() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap(); let root = temp.path().canonicalize().unwrap();
    let source = root.join("remote/raw-data/v1.2.3");
    fs::create_dir_all(source.join("new-private")).unwrap();
    fs::create_dir(source.join("retained-private")).unwrap();
    fs::write(source.join("shared"), b"new").unwrap();
    fs::write(source.join("new-private/data"), b"private source").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o750)).unwrap();
    fs::set_permissions(source.join("new-private"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(source.join("shared"), fs::Permissions::from_mode(0o644)).unwrap();
    fs::create_dir_all(root.join("downloaded/retained-private")).unwrap();
    fs::write(root.join("downloaded/shared"), b"old").unwrap();
    fs::write(root.join("downloaded/retained-private/local"), b"retained").unwrap();
    fs::set_permissions(root.join("downloaded"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(root.join("downloaded/retained-private"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(root.join("downloaded/shared"), fs::Permissions::from_mode(0o400)).unwrap();
    assert_success(&invoke(&root, "archive", Some("v1.2.3"), false));
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&root.join("downloaded")), 0o700);
    assert_eq!(mode(&root.join("downloaded/retained-private")), 0o700);
    assert_eq!(mode(&root.join("downloaded/new-private")), 0o700);
    assert_eq!(mode(&root.join("downloaded/shared")), 0o400);
    assert_eq!(fs::read(root.join("downloaded/shared")).unwrap(), b"new");
    assert_success(&invoke(&root, "archive", Some("v1.2.3"), true));
    assert_eq!(mode(&root.join("downloaded")), 0o700);
    assert!(!root.join("downloaded/retained-private/local").exists());
    let root_utf8 = Utf8Path::from_path(&root).unwrap();
    let selected = select_version(&root_utf8.join("remote"), "raw-data", LocalStructure::Archive, Some("v1.2.3"), true).unwrap();
    download_version(root_utf8, &selected, &root_utf8.join("new-download"), false).unwrap();
    assert_eq!(mode(&root.join("new-download")), 0o750);
    assert_eq!(mode(&root.join("new-download/new-private")), 0o700);
}

// This fixture interposes only rename calls inside its own temporary project.
// It adds no production test hooks, process-global environment mutation or API.
#[cfg(target_os = "linux")]
#[test]
fn install_failure_restores_original_and_rollback_failure_retains_original_with_path() {
    let temp = tempfile::tempdir().unwrap(); let physical = temp.path().canonicalize().unwrap();
    let shim = rename_fault(&physical);
    for operation in ["download", "store", "restore"] {
    for fail_rollback in [false, true] {
        let root = physical.join(format!("{operation}-{}", if fail_rollback { "rollback-error" } else { "install-error" }));
        fs::create_dir_all(root.join("remote/raw-data/v1.2.3")).unwrap();
        fs::create_dir(root.join("downloaded")).unwrap();
        fs::write(root.join("remote/raw-data/v1.2.3/new"), b"new source bytes").unwrap();
        fs::write(root.join("downloaded/original"), b"original bytes").unwrap();
        if operation=="restore" {
            fs::write(root.join("_proj.yml"),"directories:\n  raw-data:\n    path: downloaded\n    source:\n      local:\n        A:\n          path: remote\n").unwrap();
            fs::write(root.join("manifest.csv"),"label,fn,version,hash\nraw-data,new,v1.2.3,900150983cd24fb0d6963f7d28e17f72\n").unwrap();
        }
        let before = snapshot(&root);
        let mut command = Command::new(env!("CARGO_BIN_EXE_proj"));
        command.args(["--project"]).arg(&root)
            .current_dir(&root).env("LD_PRELOAD", &shim).env("PROJ_TEST_RENAME_ROOT", &root)
            .env_remove("PROJ_TEST_FAIL_ROLLBACK").env_remove(proj::PROJECT_ROOT_ENV)
            .env_remove(proj::RUN_CONTEXT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE");
        if operation=="restore" {
            command.args(["--machine","remote","restore","--label","raw-data","--clear","--max-bytes","10000","--max-expanded-bytes","10000"]);
        } else if operation == "download" {
            command.args(["remote","download","--path","remote","--label","raw-data",
                "--remote-version","v1.2.3","--destination","downloaded","--clear"]);
        } else {
            command.args(["remote","store","--source","remote/raw-data/v1.2.3",
                "--destination","downloaded","--inspect","file","--strategy","sync-diff"]);
        }
        if fail_rollback { command.env("PROJ_TEST_FAIL_ROLLBACK", "1"); }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        if operation=="restore" {
            let json:serde_json::Value=serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(json["ok"],false);assert_eq!(json["error"]["code"],"command_failed");
            assert!(!String::from_utf8_lossy(&output.stdout).contains(".proj-download-"));
        }
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(diagnostic.contains("Cannot install"), "{diagnostic}");
        if fail_rollback {
            assert!(!root.join("downloaded").exists());
            let staging: Vec<_> = fs::read_dir(&root).unwrap().map(|e| e.unwrap().path())
                .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(".proj-download-")).collect();
            assert_eq!(staging.len(), 1);
            let previous = staging[0].join("previous");
            assert_eq!(fs::read(previous.join("original")).unwrap(), b"original bytes");
            assert!(diagnostic.contains(previous.to_str().unwrap()), "{diagnostic}");
            assert!(diagnostic.contains("rollback failed"), "{diagnostic}");
            assert_eq!(fs::read(root.join("remote/raw-data/v1.2.3/new")).unwrap(), b"new source bytes");
        } else {
            assert_eq!(snapshot(&root), before);
            assert!(diagnostic.contains("Cannot install staged local download"), "{diagnostic}");
        }
    }
    }
}

#[cfg(target_os = "linux")]
fn rename_fault(physical:&Path)->std::path::PathBuf {
    let shim_source = physical.join("rename-fault.c");
    let shim = physical.join("rename-fault.so");
    fs::write(&shim_source, r#"
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdlib.h>
#include <string.h>
static int ends(const char *text, const char *suffix) {
    size_t n = strlen(text), m = strlen(suffix);
    return n >= m && strcmp(text + n - m, suffix) == 0;
}
int rename(const char *from, const char *to) {
    const char *root = getenv("PROJ_TEST_RENAME_ROOT");
    if (root && strncmp(from, root, strlen(root)) == 0 &&
        from[strlen(root)] == '/' && strstr(from, "/.proj-download-") &&
        (ends(from, "/next") ||
         (getenv("PROJ_TEST_FAIL_ROLLBACK") && ends(from, "/previous")))) {
        errno = EIO;
        return -1;
    }
    int (*real_rename)(const char *, const char *) = dlsym(RTLD_NEXT, "rename");
    return real_rename(from, to);
}
"#).unwrap();
    let compiled = Command::new("cc").args(["-shared", "-fPIC"]).arg(&shim_source)
        .args(["-ldl", "-o"]).arg(&shim).output().unwrap();
    assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));
    shim
}

#[cfg(target_os = "linux")]
#[test]
fn configured_publication_cleans_all_private_readonly_snapshots_on_prepare_or_apply_failure() {
    use std::os::unix::fs::PermissionsExt;
    let temp=tempfile::tempdir().unwrap();let physical=temp.path().canonicalize().unwrap();let shim=rename_fault(&physical);
    for phase in ["prepare","apply"] {
        let root=physical.join(phase);fs::create_dir(&root).unwrap();
        for label in ["raw-a","raw-b"] {
            fs::create_dir(root.join(label)).unwrap();fs::write(root.join(label).join("input"),"original").unwrap();
        }
        fs::write(root.join("VERSION"),"v1.2.3\n").unwrap();
        fs::write(root.join("mutate.py"),"from pathlib import Path\nPath('raw-b/input').write_text('changed')\n").unwrap();
        let scripts=if phase=="prepare" {"[mutate.py]"}else{"[]"};
        fs::write(root.join("_proj.yml"),format!("directories:\n  raw-a: {{path: raw-a}}\n  raw-b: {{path: raw-b}}\n  cache: {{path: cache}}\nbuild:\n  scripts: {scripts}\n  git: false\n  local:\n    first:\n      path: delivered-a\n      structure: latest\n      content: [raw-a]\n      send: {{inspect: file}}\n    second:\n      path: delivered-b\n      structure: latest\n      content: [raw-b]\n      send: {{inspect: file}}\n")).unwrap();
        fs::set_permissions(root.join("raw-a/input"),fs::Permissions::from_mode(0o400)).unwrap();
        fs::set_permissions(root.join("raw-a"),fs::Permissions::from_mode(0o500)).unwrap();
        fs::set_permissions(root.join("raw-b"),fs::Permissions::from_mode(0o500)).unwrap();
        if phase=="apply" {fs::set_permissions(root.join("raw-b/input"),fs::Permissions::from_mode(0o400)).unwrap();}
        let staging=root.join("private-staging");fs::create_dir(&staging).unwrap();
        let mut command=Command::new(env!("CARGO_BIN_EXE_proj"));
        command.current_dir(&root).args(["build","--non-interactive","--project"]).arg(&root)
            .env("TMPDIR",&staging).env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
            .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT");
        if phase=="apply" {command.env("LD_PRELOAD",&shim).env("PROJ_TEST_RENAME_ROOT",&root).env_remove("PROJ_TEST_FAIL_ROLLBACK");}
        let output=command.output().unwrap();assert!(!output.status.success());
        let diagnostic=String::from_utf8_lossy(&output.stderr);
        assert!(diagnostic.contains(if phase=="prepare" {"disagree with current CSV"}else{"Cannot install staged local download"}),"{diagnostic}");
        assert!(diagnostic.contains("local release preserved"),"{diagnostic}");
        assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(),"Version: v1.2.4");
        assert_eq!(fs::read_dir(&staging).unwrap().count(),0,"{diagnostic}");
        assert!(!root.join("delivered-b").exists());assert!(!root.join("delivered-a/raw-a").exists());
        for label in ["raw-a","raw-b"] {fs::set_permissions(root.join(label),fs::Permissions::from_mode(0o700)).unwrap();}
    }
}
