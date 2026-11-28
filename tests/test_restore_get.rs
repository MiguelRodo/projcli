//! `proj restore` and `proj get` (projr 0.9 `projr_restore`, `projr_get_local`,
//! `projr_get_github`): real local builds, and a loopback GitHub fixture.
use camino::{Utf8Path, Utf8PathBuf};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Output},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_proj"));
    command.current_dir(root);
    for (key, _) in std::env::vars_os() {
        let text = key.to_string_lossy().to_ascii_uppercase();
        if text.starts_with("GIT_")
            || text.starts_with("GH_")
            || text.starts_with("GITHUB_")
            || text.starts_with("PROJR_")
            || text.starts_with("PROJCLI_")
            || text == "QUARTO_PROFILE"
        {
            command.env_remove(key);
        }
    }
    command
        .env_remove(proj::PROJECT_ROOT_ENV)
        .env_remove(proj::RUN_CONTEXT_ENV)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", root.join(".no-global-gitconfig"))
        .env("GIT_TERMINAL_PROMPT", "0");
    command
}
fn proj(root: &Path, args: &[&str]) -> Output {
    command(root)
        .args(["--project"])
        .arg(root)
        .args(args)
        .output()
        .unwrap()
}
#[track_caller]
fn success(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null)
}
#[track_caller]
fn failure(output: Output) -> String {
    assert!(!output.status.success(), "unexpected success");
    String::from_utf8_lossy(&output.stderr).into_owned()
}
fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut result = BTreeMap::new();
    if !root.exists() {
        return result;
    }
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if path.is_dir() {
            for (child, bytes) in files(&path) {
                result.insert(format!("{name}/{child}"), bytes);
            }
        } else {
            result.insert(name, fs::read(path).unwrap());
        }
    }
    result
}
fn project(dest_path: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap().join("analysis");
    fs::create_dir_all(root.join("input/sub")).unwrap();
    fs::create_dir(root.join("results")).unwrap();
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    fs::write(root.join("input/data.csv"), "abc").unwrap();
    fs::write(root.join("input/sub/notes.txt"), "nested").unwrap();
    fs::write(root.join("results/result.txt"), "first").unwrap();
    fs::write(
        root.join("_projr.yml"),
        format!(
            "directories:\n  raw-data: {{path: input}}\n  output: {{path: results}}\nbuild:\n  scripts: []\n  git: false\n  restrictions: {{not_behind_remote: false}}\n  dest: [arch]\nremotes:\n  local:\n    arch:\n      path: {dest_path}\n      content: [raw-data, output]\n      send: {{inspect: file}}\n"
        ),
    )
    .unwrap();
    (temporary, root)
}
fn build(root: &Path) {
    success(proj(root, &["build", "--non-interactive"]));
}

#[test]
fn restore_reads_recorded_uuid_destination_verifies_md5_and_stages_before_replacing() {
    let (_temporary, root) = project("store/@uuid");
    build(&root);
    let store: Vec<_> = fs::read_dir(root.join("store")).unwrap().collect();
    assert_eq!(store.len(), 1, "one @uuid destination");
    let uuid_dir = store[0].as_ref().unwrap().path();
    let expected = files(&root.join("input"));

    // Default labels are every raw directory; the recorded @uuid path is used.
    fs::remove_dir_all(root.join("input")).unwrap();
    let value = success(proj(
        &root,
        &["--machine", "restore", "--destination", "local:arch"],
    ));
    let result = &value["result"]["restore"];
    assert_eq!(result["version"], "v1.2.4");
    assert_eq!(result["source"], "local:arch");
    assert_eq!(result["labels"][0]["label"], "raw-data");
    assert_eq!(result["labels"][0]["status"], "installed");
    assert_eq!(result["labels"].as_array().unwrap().len(), 1);
    assert_eq!(files(&root.join("input")), expected);

    // Local changes and extra files are errors without --clear.
    fs::write(root.join("input/data.csv"), "edited").unwrap();
    let error = failure(proj(&root, &["restore", "--destination", "local:arch"]));
    assert!(error.contains("differ from v1.2.4"), "{error}");
    assert!(error.contains("--clear"), "{error}");
    assert_eq!(fs::read(root.join("input/data.csv")).unwrap(), b"edited");
    fs::write(root.join("input/data.csv"), "abc").unwrap();
    fs::write(root.join("input/extra.txt"), "extra").unwrap();
    failure(proj(&root, &["restore", "--destination", "local:arch"]));
    assert!(root.join("input/extra.txt").exists());
    success(proj(
        &root,
        &["restore", "--destination", "local:arch", "--clear"],
    ));
    assert_eq!(files(&root.join("input")), expected);

    // A corrupt archive file never replaces the local directory.
    fs::write(root.join("input/data.csv"), "edited").unwrap();
    let archived = uuid_dir.join("raw-data/v1.2.4/data.csv");
    fs::write(&archived, "corrupt").unwrap();
    let error = failure(proj(
        &root,
        &["restore", "--destination", "local:arch", "--clear"],
    ));
    assert!(error.contains("No matching files for raw-data"), "{error}");
    assert_eq!(fs::read(root.join("input/data.csv")).unwrap(), b"edited");
    assert!(
        root.parent()
            .unwrap()
            .read_dir()
            .unwrap()
            .chain(root.read_dir().unwrap())
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".projr-get-")),
        "staging is removed after a failure"
    );
    fs::write(&archived, "abc").unwrap();
    fs::write(root.join("input/data.csv"), "abc").unwrap();

    // A matching directory needs no remote at all.
    fs::remove_dir_all(root.join("store")).unwrap();
    let value = success(proj(
        &root,
        &["--machine", "restore", "--destination", "local:arch"],
    ));
    assert_eq!(
        value["result"]["restore"]["labels"][0]["status"],
        "verified"
    );
    assert!(value["result"]["restore"]["source"].is_null());

    // The destination is only consulted when a label needs files.
    fs::remove_file(root.join("input/data.csv")).unwrap();
    let error = failure(proj(&root, &["restore", "--destination", "local:none"]));
    assert!(error.contains("Unknown local remote: none"), "{error}");
    let error = failure(proj(&root, &["restore", "--label", "missing"]));
    assert!(error.contains("not declared"), "{error}");
}

#[test]
fn restore_falls_back_to_an_identical_snapshot_and_restores_several_labels() {
    let (_temporary, root) = project("store");
    build(&root);
    fs::write(root.join("results/result.txt"), "second").unwrap();
    build(&root);
    // Raw data did not change; remove its newer archive, if one was written, so
    // only the identical v1.2.4 snapshot remains.
    let newer = root.join("store/raw-data/v1.2.5");
    if newer.exists() {
        fs::remove_dir_all(&newer).unwrap();
    }
    let raw = files(&root.join("input"));
    let output = files(&root.join("results"));
    fs::remove_dir_all(root.join("input")).unwrap();
    fs::remove_dir_all(root.join("results")).unwrap();
    let value = success(proj(
        &root,
        &[
            "--machine",
            "restore",
            "--destination",
            "local:arch",
            "--label",
            "raw-data,output",
        ],
    ));
    assert_eq!(value["result"]["restore"]["version"], "v1.2.5");
    assert_eq!(files(&root.join("input")), raw);
    assert_eq!(files(&root.join("results")), output);
    // An explicit older version restores that build's files.
    let value = success(proj(
        &root,
        &[
            "--machine",
            "restore",
            "--destination",
            "local:arch",
            "--label",
            "output",
            "--version",
            "1.2.4",
            "--clear",
        ],
    ));
    assert_eq!(value["result"]["restore"]["version"], "v1.2.4");
    assert_eq!(fs::read(root.join("results/result.txt")).unwrap(), b"first");
}

#[test]
fn get_local_copies_verified_directories_from_another_analysis() {
    let (temporary, root) = project("store");
    build(&root);
    let consumer = temporary.path().canonicalize().unwrap().join("consumer");
    fs::create_dir(&consumer).unwrap();
    let out = consumer.join("downloads");
    let value = success(
        command(&consumer)
            .args(["--machine", "--project"])
            .arg(&consumer)
            .args(["get", "local", "--path"])
            .arg(&root)
            .arg("--out")
            .arg(&out)
            .output()
            .unwrap(),
    );
    let result = &value["result"]["get"];
    assert_eq!(result["version"], "v1.2.4");
    assert_eq!(files(&out.join("raw-data")), files(&root.join("input")));
    assert_eq!(files(&out.join("output")), files(&root.join("results")));

    // Changed output requires --overwrite; matching files are reused either way.
    fs::write(out.join("output/result.txt"), "local edit").unwrap();
    let get = |extra: &[&str]| {
        command(&consumer)
            .args(["--project"])
            .arg(&consumer)
            .args(["get", "local", "--label", "output", "--path"])
            .arg(&root)
            .arg("--out")
            .arg(&out)
            .args(extra)
            .output()
            .unwrap()
    };
    let error = failure(get(&[]));
    assert!(error.contains("--overwrite"), "{error}");
    assert_eq!(
        fs::read(out.join("output/result.txt")).unwrap(),
        b"local edit"
    );
    success(get(&["--overwrite"]));
    assert_eq!(fs::read(out.join("output/result.txt")).unwrap(), b"first");

    // The producer's own directories cannot be a destination.
    let error = failure(
        command(&consumer)
            .args(["--project"])
            .arg(&consumer)
            .args(["get", "local", "--path"])
            .arg(&root)
            .arg("--out")
            .arg(root.join("input"))
            .output()
            .unwrap(),
    );
    assert!(error.contains("overlaps the source"), "{error}");
    // A non-root path is not a producing analysis.
    let error = failure(
        command(&consumer)
            .args(["--project"])
            .arg(&consumer)
            .args(["get", "local", "--path"])
            .arg(root.join("input"))
            .output()
            .unwrap(),
    );
    assert!(error.contains("root of the producing"), "{error}");

    // A registered remote of the current project, with an explicit version.
    let remote_out = temporary.path().canonicalize().unwrap().join("remote-out");
    let value = success(
        command(&root)
            .args(["--machine", "--project"])
            .arg(&root)
            .args([
                "get",
                "local",
                "--remote",
                "arch",
                "--label",
                "raw-data",
                "--version",
                "v1.2.4",
                "--out",
            ])
            .arg(&remote_out)
            .output()
            .unwrap(),
    );
    assert_eq!(value["result"]["get"]["version"], "v1.2.4");
    assert_eq!(
        files(&remote_out.join("raw-data")),
        files(&root.join("input"))
    );
}

#[test]
fn directory_source_and_position_are_removed() {
    let (_temporary, root) = project("store");
    let config = fs::read_to_string(root.join("_projr.yml")).unwrap();
    fs::write(
        root.join("_projr.yml"),
        config.replace(
            "raw-data: {path: input}",
            "raw-data: {path: input, source: {local: arch}}",
        ),
    )
    .unwrap();
    let error = failure(proj(&root, &["restore"]));
    assert!(
        error.contains("directories.raw-data.source is no longer supported")
            && error.contains("build.source"),
        "{error}"
    );
    let help = proj(&root, &["remote", "restore", "--help"]);
    assert!(!String::from_utf8_lossy(&help.stdout).contains("--position"));
}

struct Server {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    /// Serves exact `path?query` routes; asset downloads by numeric ID.
    fn start(routes: BTreeMap<String, (u16, Vec<u8>)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, halt) = (requests.clone(), stop.clone());
        let worker = thread::spawn(move || {
            while !halt.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream.set_nonblocking(false).unwrap();
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    head.push(byte[0]);
                }
                let text = String::from_utf8(head).unwrap();
                let mut first = text.lines().next().unwrap().split_whitespace();
                let method = first.next().unwrap().to_owned();
                let path = first.next().unwrap().to_owned();
                assert_eq!(method, "GET", "restore and get are read-only");
                seen.lock().unwrap().push(path.clone());
                let (status, body) = routes.get(&path).cloned().unwrap_or((404, b"{}".to_vec()));
                write!(
                    stream,
                    "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        Self {
            base,
            requests,
            stop,
            thread: Some(worker),
        }
    }
    fn finish(mut self) -> Vec<String> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
        self.requests.lock().unwrap().clone()
    }
}

/// A Release fixture: assets keyed by name, with IDs from 1.
fn release(tag: &str, assets: &[(&str, Vec<u8>)]) -> BTreeMap<String, (u16, Vec<u8>)> {
    let mut routes = BTreeMap::new();
    routes.insert(
        format!("/repos/owner/repo/releases/tags/{tag}"),
        (
            200,
            serde_json::json!({"id": 7, "tag_name": tag})
                .to_string()
                .into_bytes(),
        ),
    );
    let listing: Vec<_> = assets
        .iter()
        .enumerate()
        .map(|(i, (name, bytes))| serde_json::json!({"id": i + 1, "name": name, "size": bytes.len()}))
        .collect();
    routes.insert(
        "/repos/owner/repo/releases/7/assets?per_page=100&page=1".into(),
        (200, serde_json::to_vec(&listing).unwrap()),
    );
    for (i, (_, bytes)) in assets.iter().enumerate() {
        routes.insert(
            format!("/repos/owner/repo/releases/assets/{}", i + 1),
            (200, bytes.clone()),
        );
    }
    routes
}
fn zip_of(directory: &Path) -> Vec<u8> {
    let (zip, _) =
        proj::github_zip::directory_zip(Utf8Path::from_path(directory).unwrap()).unwrap();
    fs::read(zip.path()).unwrap()
}
/// A checkout at v1.2.4 whose manifest store records `raw-data` as `files`.
fn github_project(files: &[(&str, &str)]) -> (tempfile::TempDir, Utf8PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let root =
        Utf8PathBuf::from_path_buf(temporary.path().canonicalize().unwrap().join("checkout"))
            .unwrap();
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("VERSION"), "v1.2.4\n").unwrap();
    fs::write(
        root.join("_projr.yml"),
        "directories:\n  raw-data: {path: input}\n  output: {path: results}\nbuild:\n  scripts: []\n  git: false\n",
    )
    .unwrap();
    record(&root.join(".projr/manifests"), files);
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .current_dir(&root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", root.join(".no-global-gitconfig"))
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    };
    git(&["init", "--quiet"]);
    git(&[
        "remote",
        "add",
        "origin",
        "https://127.0.0.1/owner/repo.git",
    ]);
    (temporary, root)
}
fn record(store: &Utf8Path, files: &[(&str, &str)]) {
    let rows: Vec<_> = files
        .iter()
        .map(|(name, bytes)| proj::manifest::ManifestRow {
            label: "raw-data".into(),
            r#fn: (*name).into(),
            version: "v1.2.4".into(),
            hash: format!("{:x}", md5::compute(bytes)),
        })
        .collect();
    proj::manifest_store::write(store, &rows, None, false).unwrap();
}
fn manifest_zip(files: &[(&str, &str)]) -> Vec<u8> {
    let temporary = tempfile::tempdir().unwrap();
    let base = Utf8PathBuf::from_path_buf(temporary.path().canonicalize().unwrap()).unwrap();
    record(&base.join("manifests"), files);
    let zip =
        proj::github_zip::metadata_zip(&base, proj::github_zip::MetadataKind::Manifest).unwrap();
    fs::read(zip.path()).unwrap()
}
fn github(server: &Server, root: &Path, args: &[&str]) -> Output {
    command(root)
        .env("GITHUB_API_URL", &server.base)
        .env("GITHUB_PAT", "fixture-token")
        .args(["--project"])
        .arg(root)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn restore_from_github_archive_checks_manifests_and_disagreement() {
    let wanted = [("data.csv", "abc"), ("sub/notes.txt", "nested")];
    let payload = tempfile::tempdir().unwrap();
    fs::create_dir_all(payload.path().join("sub")).unwrap();
    fs::write(payload.path().join("data.csv"), "abc").unwrap();
    fs::write(payload.path().join("sub/notes.txt"), "nested").unwrap();
    let payload_zip = zip_of(&payload.path().canonicalize().unwrap());

    let (_temporary, root) = github_project(&wanted);
    let server = Server::start(release(
        "archive",
        &[
            ("manifest.zip", manifest_zip(&wanted)),
            ("raw-data-v1.2.4.zip", payload_zip.clone()),
        ],
    ));
    let value = success(github(
        &server,
        root.as_std_path(),
        &["--machine", "restore"],
    ));
    assert_eq!(
        value["result"]["restore"]["source"],
        "github:owner/repo@archive"
    );
    assert_eq!(
        fs::read(root.join("input/sub/notes.txt")).unwrap(),
        b"nested"
    );
    let requests = server.finish();
    assert!(
        requests
            .iter()
            .all(|path| path.starts_with("/repos/owner/repo/releases/"))
    );

    // The checkout and the Release record different files for the same build.
    let (_temporary, root) = github_project(&[("data.csv", "other")]);
    let server = Server::start(release(
        "archive",
        &[
            ("manifest.zip", manifest_zip(&wanted)),
            ("raw-data-v1.2.4.zip", payload_zip.clone()),
        ],
    ));
    let error = failure(github(&server, root.as_std_path(), &["restore"]));
    assert!(
        error.contains("Sources disagree about raw-data at v1.2.4"),
        "{error}"
    );
    assert!(!root.join("input").exists());
    server.finish();

    // A Release without a manifest relies on the checkout's snapshot; a payload
    // that does not match it is rejected before installation.
    let (_temporary, root) = github_project(&[("data.csv", "different")]);
    let server = Server::start(release("v9", &[("raw-data-v1.2.4.zip", payload_zip)]));
    let error = failure(github(
        &server,
        root.as_std_path(),
        &["restore", "--destination", "v9"],
    ));
    assert!(
        error.contains("does not match the selected build snapshot"),
        "{error}"
    );
    assert!(!root.join("input").exists());
    server.finish();
}

#[test]
fn get_github_reads_a_tag_and_a_branch_without_a_local_project() {
    let wanted = [("data.csv", "abc"), ("sub/notes.txt", "nested")];
    let payload = tempfile::tempdir().unwrap();
    fs::create_dir_all(payload.path().join("sub")).unwrap();
    fs::write(payload.path().join("data.csv"), "abc").unwrap();
    fs::write(payload.path().join("sub/notes.txt"), "nested").unwrap();
    let payload_zip = zip_of(&payload.path().canonicalize().unwrap());
    let mut routes = release(
        "archive",
        &[
            ("manifest.zip", manifest_zip(&wanted)),
            ("raw-data-v1.2.4.zip", payload_zip),
            ("VERSION.zip", {
                let directory = tempfile::tempdir().unwrap();
                let base =
                    Utf8PathBuf::from_path_buf(directory.path().canonicalize().unwrap()).unwrap();
                fs::write(base.join("VERSION"), "Project: v1.2.4\nraw-data: v1.2.4\n").unwrap();
                let zip =
                    proj::github_zip::metadata_zip(&base, proj::github_zip::MetadataKind::Version)
                        .unwrap();
                fs::read(zip.path()).unwrap()
            }),
        ],
    );
    let sha = "0123456789abcdef0123456789abcdef01234567";
    routes.insert(
        "/repos/owner/repo".into(),
        (200, br#"{"default_branch":"main"}"#.to_vec()),
    );
    routes.insert(
        "/repos/owner/repo/git/ref/heads/main".into(),
        (
            200,
            format!(r#"{{"object":{{"type":"commit","sha":"{sha}"}}}}"#).into_bytes(),
        ),
    );
    routes.insert(
        format!("/repos/owner/repo/contents/_projr.yml?ref={sha}"),
        (200, b"directories:\n  raw-data: {path: input}\n".to_vec()),
    );
    routes.insert(
        format!("/repos/owner/repo/contents/VERSION?ref={sha}"),
        (200, b"v1.2.4\n".to_vec()),
    );
    let server = Server::start(routes);
    let temporary = tempfile::tempdir().unwrap();
    let consumer = temporary.path().canonicalize().unwrap();

    // A concrete tag: version from the Release's VERSION.zip, labels from its manifest.
    let value = success(github(
        &server,
        &consumer,
        &[
            "--machine",
            "get",
            "github",
            "--repo",
            "owner/repo",
            "--tag",
            "archive",
            "--out",
            "by-tag",
        ],
    ));
    assert_eq!(value["result"]["get"]["version"], "v1.2.4");
    assert_eq!(
        fs::read(consumer.join("by-tag/raw-data/data.csv")).unwrap(),
        b"abc"
    );
    // The default branch's configuration and VERSION select the archive Release.
    let value = success(github(
        &server,
        &consumer,
        &[
            "--machine",
            "get",
            "github",
            "--repo",
            "owner/repo",
            "--out",
            "by-branch",
        ],
    ));
    assert_eq!(value["result"]["get"]["labels"][0]["label"], "raw-data");
    assert_eq!(
        fs::read(consumer.join("by-branch/raw-data/sub/notes.txt")).unwrap(),
        b"nested"
    );
    let error = failure(github(
        &server,
        &consumer,
        &["get", "github", "--repo", "owner/repo", "--remote", "x"],
    ));
    assert!(error.contains("cannot be used with"), "{error}");
    let error = failure(github(
        &server,
        &consumer,
        &["get", "github", "--repo", "../x"],
    ));
    assert!(error.contains("owner/repo"), "{error}");
    server.finish();
}
