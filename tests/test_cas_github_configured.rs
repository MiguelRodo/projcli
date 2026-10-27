//! Configured GitHub CAS uses actual engine with loopback REST only.
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
fn command(root: &Path, engine: Option<&Path>) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_proj"));
    command.current_dir(root).env_clear();
    for key in [
        "SYSTEMROOT",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
        "TEMP",
        "TMP",
        "TMPDIR",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let name = if cfg!(windows) {
        "proj-xet-engine.exe"
    } else {
        "proj-xet-engine"
    };
    let mut paths: Vec<_> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter(|directory| !directory.join(name).is_file())
        .collect();
    if let Some(engine) = engine {
        paths.insert(0, engine.parent().unwrap().to_owned());
    }
    command
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("HOME", root.join("home"))
        .env("USERPROFILE", root.join("home"))
        .args(["--machine", "--non-interactive", "--project"])
        .arg(root);
    command
}
fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], true);
    value
}
fn failed(output: Output) {
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout)
        .expect("failure must be structured, not a CLI panic");
    assert_eq!(value["ok"], false);
}
fn setup() -> (tempfile::TempDir, PathBuf, Value) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp
        .path()
        .canonicalize()
        .unwrap()
        .join("project with spaces");
    for name in ["input/empty", "results", "cache", "home"] {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    fs::write(root.join("input/data.csv"), b"name,value\none,1\ntwo,2\n").unwrap();
    fs::write(root.join("results/keep.txt"), b"unselected output\n").unwrap();
    fs::write(root.join("VERSION"), b"v1.2.3\n").unwrap();
    let config = json!({"directories":{"raw-data":{"path":"input"},"output":{"path":"results"},
        "cache":{"path":"cache","safe":true}},
        "remotes":{"local":{"backup":{"path":"../store","structure":"xet","content":true}}},
        "build":{"scripts":[],"git":false,"dest":{"backup":{"content":["raw-data"]}}}});
    save(&root, &config);
    (temp, root, config)
}
fn save(root: &Path, value: &Value) {
    fs::write(
        root.join("_proj.yml"),
        serde_json::to_vec_pretty(value).unwrap(),
    )
    .unwrap();
}

#[path = "common/xet_github_mock.rs"]
mod mock;
use mock::Server;
fn invoke(root: &Path, engine: Option<&Path>, server: &Server) -> Command {
    let mut command = command(root, engine);
    command
        .env("GITHUB_API_URL", &server.base)
        .env("GITHUB_PAT", "fixture-token");
    command
}
fn github_setup() -> (tempfile::TempDir, PathBuf, Value) {
    let (temp, root, mut config) = setup();
    config["remotes"] = json!({"github":{"data":{"structure":"cas","manifest-style":"tree","content":["raw-data"]}}});
    config["build"]["dest"] = json!({"data":{}});
    save(&root, &config);
    for args in [
        vec!["init"],
        vec![
            "remote",
            "add",
            "origin",
            "https://127.0.0.1/owner/repo.git",
        ],
    ] {
        let mut git = Command::new("git");
        git.current_dir(&root).env_clear();
        for key in [
            "PATH",
            "SYSTEMROOT",
            "WINDIR",
            "COMSPEC",
            "PATHEXT",
            "TEMP",
            "TMP",
            "TMPDIR",
        ] {
            if let Some(value) = std::env::var_os(key) {
                git.env(key, value);
            }
        }
        git.env("HOME", root.join("home"))
            .env("USERPROFILE", root.join("home"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", root.join("home/no-config"))
            .env("GIT_TERMINAL_PROMPT", "0");
        assert!(git.args(args).output().unwrap().status.success());
    }
    (temp, root, config)
}
fn namespace(root: &Path) -> PathBuf {
    let mut hash = blake3::Hasher::new_derive_key("projcli.remote-name.v1");
    hash.update(b"DATA");
    let key = hash.finalize().to_hex().to_string();
    root.join(".proj/remotes")
        .join(&key[..2])
        .join(&key[2..])
        .join("cas-github/v1")
}
fn copy_tree(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &destination)
        } else {
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}
#[test]
fn github_cas_preview_and_empty_selection_need_no_helper_or_network() {
    let server = Server::start();
    let (_temp, root, mut config) = github_setup();
    success(
        invoke(&root, None, &server)
            .args(["yml", "read"])
            .output()
            .unwrap(),
    );
    let version = fs::read(root.join("VERSION")).unwrap();
    success(
        invoke(&root, None, &server)
            .arg("build-dev")
            .output()
            .unwrap(),
    );
    assert_eq!(fs::read(root.join("VERSION")).unwrap(), version);
    assert!(!namespace(&root).exists());
    assert!(!server.state.lock().unwrap().exists);
    config["build"]["dest"] = json!({});
    save(&root, &config);
    success(invoke(&root, None, &server).arg("build").output().unwrap());
    assert!(!server.state.lock().unwrap().exists);
    server.finish();
}
#[test]
fn github_cas_tag_collision_fails_before_hooks_and_credentials() {
    let server = Server::start();
    let (_temp, root, mut config) = github_setup();
    config["remotes"]["github"] = json!({"data store":{"structure":"cas","manifest-style":"tree"},"data-store":{"structure":"cas","manifest-style":"tree"}});
    config["build"]["dest"] = json!({"data store":{},"data-store":{}});
    fs::write(
        root.join("start.py"),
        "from pathlib import Path\nPath('hook-ran').write_text('bad')\n",
    )
    .unwrap();
    config["build"]["hooks"] = json!({"pre":["start.py"]});
    save(&root, &config);
    let output = invoke(&root, None, &server).arg("build").output().unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("same Release tag"));
    failed(output);
    assert!(!root.join("hook-ran").exists());
    assert!(!server.state.lock().unwrap().exists);
    server.finish();
}
#[test]
fn actual_configured_github_cas_refs_fresh_restore_and_failure_preservation() {
    let server = Server::start();
    let (temp, root, mut config) = github_setup();
    let expected = fs::read(root.join("input/data.csv")).unwrap();
    success(invoke(&root, None, &server).arg("build").output().unwrap());
    let namespace = namespace(&root);
    let current = fs::read(namespace.join("current.json")).unwrap();
    let binding: Value = serde_json::from_slice(&current).unwrap();
    assert_eq!(binding["repository"], "owner/repo");
    assert_eq!(binding["reference"]["structure"], "cas");
    let first = binding["reference"]["snapshots"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .to_owned();
    let old_assets = server.state.lock().unwrap().assets.clone();
    let mut corrupt = binding.clone();
    corrupt["reference"]["hash"] = json!("bad");
    fs::write(
        namespace.join("current.json"),
        serde_json::to_vec(&corrupt).unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("spy.py"),
        "from pathlib import Path\nPath('hook-ran').write_text('bad')\n",
    )
    .unwrap();
    config["build"]["hooks"] = json!({"pre":["spy.py"]});
    save(&root, &config);
    let malformed = invoke(&root, None, &server).arg("build").output().unwrap();
    assert!(String::from_utf8_lossy(&malformed.stderr).contains("CAS digest must be 64"));
    failed(malformed);
    assert!(!root.join("hook-ran").exists());
    assert_eq!(server.state.lock().unwrap().assets, old_assets);
    config["build"].as_object_mut().unwrap().remove("hooks");
    save(&root, &config);
    fs::write(namespace.join("current.json"), &current).unwrap();
    fs::write(root.join("input/data.csv"), b"second configured version\n").unwrap();
    server.state.lock().unwrap().fail_cas = true;
    failed(invoke(&root, None, &server).arg("build").output().unwrap());
    assert_eq!(fs::read(namespace.join("current.json")).unwrap(), current);
    assert_eq!(server.state.lock().unwrap().assets, old_assets);
    server.state.lock().unwrap().fail_cas = false;
    success(invoke(&root, None, &server).arg("build").output().unwrap());
    let updated: Value =
        serde_json::from_slice(&fs::read(namespace.join("current.json")).unwrap()).unwrap();
    assert_eq!(
        updated["reference"]["snapshots"].as_object().unwrap().len(),
        2
    );
    let fresh = temp.path().join("fresh project");
    fs::create_dir_all(fresh.join("home")).unwrap();
    copy_tree(&root.join(".proj/remotes"), &fresh.join(".proj/remotes"));
    config["build"]["dest"] = json!({});
    save(&fresh, &config);
    fs::write(fresh.join("VERSION"), b"v9.8.7\n").unwrap();
    success(
        invoke(&fresh, None, &server)
            .args([
                "remote",
                "restore",
                "--provider",
                "github",
                "--title",
                "data",
                "--snapshot",
                &first,
                "--label",
                "raw-data",
                "--clear",
                "--max-bytes",
                "67108864",
                "--max-expanded-bytes",
                "67108864",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(fs::read(fresh.join("input/data.csv")).unwrap(), expected);
    assert!(fresh.join("input/empty").is_dir());
    success(
        invoke(&fresh, None, &server)
            .args([
                "remote",
                "verify",
                "--title",
                "data",
                "--snapshot",
                &first,
                "--max-bytes",
                "67108864",
                "--max-expanded-bytes",
                "67108864",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(fs::read(fresh.join("VERSION")).unwrap(), b"v9.8.7\n");
    fs::remove_file(namespace.join("current.json")).unwrap();
    let missing = invoke(&root, None, &server).arg("build").output().unwrap();
    assert!(
        String::from_utf8_lossy(&missing.stderr)
            .contains("trusted project current.json is missing")
    );
    failed(missing);
    assert!(server.state.lock().unwrap().deleted.is_empty());
    println!(
        "Configured GitHub structure:cas build, trusted Git refs, failed-upload preservation and fresh explicit restore passed."
    );
    server.finish();
}

#[test]
fn configured_github_cas_flat_modes_keep_version_references_and_support_fresh_restore() {
    use proj::catalogue::{Catalogue, Format, Inventory};
    for (format, mode, filename) in [
        (Format::Csv, "csv", "manifest.csv"),
        (Format::Json, "json", "manifest.json"),
        (
            Format::CsvDedupeFile,
            "csv-dedupe-file",
            "manifest-dedupe-file.csv",
        ),
        (
            Format::JsonDedupeFile,
            "json-dedupe-file",
            "manifest-dedupe-file.json",
        ),
    ] {
        let server = Server::start();
        let (temp, root, mut config) = github_setup();
        config["remotes"]["github"]["data"]["manifest-style"] = json!("flat");
        config["remotes"]["github"]["data"]["manifest-format"] = json!(mode);
        save(&root, &config);
        let expected = fs::read(root.join("input/data.csv")).unwrap();
        success(invoke(&root, None, &server).arg("build").output().unwrap());
        success(invoke(&root, None, &server).arg("build").output().unwrap());
        let current = namespace(&root).join("current.json");
        let binding: Value = serde_json::from_slice(&fs::read(&current).unwrap()).unwrap();
        let snapshots = binding["reference"]["snapshots"].as_object().unwrap();
        assert_eq!(snapshots.len(), 2);
        assert!(snapshots.values().all(|value| value == "flat"));
        let asset = binding["reference"]["asset"].as_str().unwrap();
        let archive = temp.path().join("store.zip");
        fs::write(&archive, &server.state.lock().unwrap().assets[asset].1).unwrap();
        let imported = proj::cas_archive::import(
            &camino::Utf8PathBuf::from_path_buf(archive).unwrap(),
            binding["reference"]["hash"].as_str().unwrap(),
            binding["reference"]["length"].as_u64().unwrap(),
            4 * 1024 * 1024,
        )
        .unwrap();
        let history = Catalogue::parse(
            &fs::read(imported.path().join("catalogues/v1").join(filename)).unwrap(),
            format,
        )
        .unwrap();
        assert!(matches!(
            &history.records[1].inventory,
            Inventory::Reference { .. }
        ));
        let id = snapshots.keys().next().unwrap().clone();
        let fresh = temp.path().join("fresh flat project");
        fs::create_dir_all(fresh.join("home")).unwrap();
        copy_tree(&root.join(".proj/remotes"), &fresh.join(".proj/remotes"));
        config["build"]["dest"] = json!({});
        save(&fresh, &config);
        fs::write(fresh.join("VERSION"), b"v8.0.0\n").unwrap();
        success(
            invoke(&fresh, None, &server)
                .args([
                    "remote",
                    "restore",
                    "--provider",
                    "github",
                    "--title",
                    "data",
                    "--snapshot",
                    &id,
                    "--label",
                    "raw-data",
                    "--clear",
                    "--max-bytes",
                    "4194304",
                    "--max-expanded-bytes",
                    "4194304",
                ])
                .output()
                .unwrap(),
        );
        assert_eq!(fs::read(fresh.join("input/data.csv")).unwrap(), expected);
        assert!(server.state.lock().unwrap().deleted.is_empty());
        server.finish();
    }
}

#[test]
fn ordinary_literal_version_text_title_remains_valid_beside_shared_store() {
    let server = Server::start();
    let (_temp, root, mut config) = github_setup();
    config["remotes"]["github"]["results@version"] =
        json!({"structure":"archive","content":["raw-data"]});
    config["build"]["dest"]["results@version"] = json!({});
    save(&root, &config);
    success(
        invoke(&root, None, &server)
            .arg("build-dev")
            .output()
            .unwrap(),
    );
    assert!(!server.state.lock().unwrap().exists);
    server.finish();
}

#[test]
fn cas_xet_titles_cannot_share_one_normalized_storage_release() {
    let server = Server::start();
    let (_temp, root, mut config) = github_setup();
    config["remotes"]["github"]["data "] = json!({"structure":"xet","content":["raw-data"]});
    config["build"]["dest"]["data "] = json!({});
    save(&root, &config);
    let output = invoke(&root, None, &server).arg("build").output().unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("same Release tag"));
    failed(output);
    assert!(!server.state.lock().unwrap().exists);
    server.finish();
}
