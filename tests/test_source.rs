//! `build.source`, `proj source` and `proj restore-repo` (projr 0.9
//! `R/source.R`, `R/restore-repo.R`): real local builds as producers, consumers
//! that fetch from their local archive, and a loopback GitHub fixture.
use camino::{Utf8Path, Utf8PathBuf};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        Arc,
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
        .args(["--non-interactive", "--project"])
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
/// YAML-safe absolute path text.
/// Also drops the Windows verbatim prefix, which Git cannot read.
fn yaml_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\")
        .unwrap_or(&text)
        .replace('\\', "/")
}

/// A producer analysis that archives `raw-data` to `<temporary>/store`, built
/// once per entry of `versions` (data.csv holds each entry), giving v1.2.4, v1.2.5, ...
fn producer(temporary: &Path, versions: &[&str]) -> (PathBuf, PathBuf) {
    let root = temporary.join("producer");
    let store = temporary.join("store");
    fs::create_dir_all(root.join("input/sub")).unwrap();
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    fs::write(root.join("input/sub/notes.txt"), "nested").unwrap();
    fs::write(
        root.join("_projr.yml"),
        format!(
            "directories:\n  raw-data: {{path: input}}\n  output: {{path: results}}\nbuild:\n  scripts: []\n  git: false\n  restrictions: {{not_behind_remote: false}}\n  dest: [arch]\nremotes:\n  local:\n    arch:\n      path: {}\n      content: [raw-data]\n      send: {{inspect: file}}\n",
            yaml_path(&store)
        ),
    )
    .unwrap();
    for data in versions {
        fs::write(root.join("input/data.csv"), data).unwrap();
        success(proj(&root, &["build"]));
    }
    (root, store)
}

/// A consumer whose pre hook records whether the fetched input was present.
fn consumer(temporary: &Path, store: &Path, extra: &str) -> PathBuf {
    let root = temporary.join("consumer");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("hook.py"),
        "import os\nwith open('seen.txt', 'w') as f:\n    f.write(str(os.path.exists('input/data.csv')))\n",
    )
    .unwrap();
    fs::write(
        root.join("_projr.yml"),
        format!(
            "directories:\n  raw-data: {{path: input}}\n  output: {{path: results}}\nbuild:\n  scripts: []\n  git: false\n  hooks: {{pre: [hook.py]}}\n{extra}remotes:\n  local:\n    arch: {{path: {}}}\n",
            yaml_path(store)
        ),
    )
    .unwrap();
    root
}
fn record(root: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(root.join(".projr/source/raw-data.json")).unwrap()).unwrap()
}

#[test]
fn production_builds_fetch_before_hooks_and_keep_inputs_after_a_failure() {
    let temporary = tempfile::tempdir().unwrap();
    let base = temporary.path().canonicalize().unwrap();
    let (producer, store) = producer(&base, &["abc"]);
    let root = consumer(&base, &store, "  source: arch\n");
    success(proj(&root, &["build"]));
    assert_eq!(files(&root.join("input")), files(&producer.join("input")));
    assert_eq!(fs::read_to_string(root.join("seen.txt")).unwrap(), "True");
    let fetched = record(&root);
    assert_eq!(fetched["remote"], "local:arch");
    assert_eq!(fetched["version"], "v1.2.4");
    assert_eq!(
        fetched["files"]["data.csv"],
        format!("{:x}", md5::compute(b"abc"))
    );
    // The fetched inputs are this build's recorded inputs.
    assert!(
        root.join(".projr/manifests/raw-data/versions.csv")
            .is_file()
    );

    // build cue defaults to if-empty: a folder with files is left alone, offline.
    fs::write(root.join("input/data.csv"), "edited").unwrap();
    let moved = base.join("store-away");
    fs::rename(&store, &moved).unwrap();
    success(proj(&root, &["build"]));
    assert_eq!(fs::read(root.join("input/data.csv")).unwrap(), b"edited");
    fs::rename(&moved, &store).unwrap();

    // With if-change, a local edit since the last fetch stops the build before hooks.
    fs::remove_file(root.join("seen.txt")).unwrap();
    let config = fs::read_to_string(root.join("_projr.yml")).unwrap();
    fs::write(
        root.join("_projr.yml"),
        config.replace("  source: arch\n", "  source: {arch: {cue: if-change}}\n"),
    )
    .unwrap();
    let error = failure(proj(&root, &["build"]));
    assert!(
        error.contains("local changes would be overwritten or deleted: data.csv"),
        "{error}"
    );
    assert!(!root.join("seen.txt").exists(), "hooks did not run");
    assert_eq!(fs::read(root.join("input/data.csv")).unwrap(), b"edited");

    // A failing build keeps the inputs it fetched (projr), with their record.
    fs::write(root.join("_projr.yml"), &config).unwrap();
    fs::write(root.join("hook.py"), "raise SystemExit(3)\n").unwrap();
    fs::remove_dir_all(root.join("input")).unwrap();
    fs::remove_dir_all(root.join(".projr/source")).unwrap();
    failure(proj(&root, &["build"]));
    assert_eq!(files(&root.join("input")), files(&producer.join("input")));
    assert_eq!(record(&root)["version"], "v1.2.4");
}

#[test]
fn development_builds_fetch_only_with_dev_source() {
    let temporary = tempfile::tempdir().unwrap();
    let base = temporary.path().canonicalize().unwrap();
    let (_producer, store) = producer(&base, &["abc"]);
    let root = consumer(&base, &store, "  source: arch\n");
    success(proj(&root, &["build-dev"]));
    assert!(
        !root.join("input").exists(),
        "decision D3: no fetch by default"
    );
    let config = fs::read_to_string(root.join("_projr.yml")).unwrap();
    fs::write(
        root.join("_projr.yml"),
        format!("{config}dev:\n  source: true\n"),
    )
    .unwrap();
    success(proj(&root, &["build-dev"]));
    // Previews restore root files such as the hook's note, but not fetched inputs.
    assert_eq!(fs::read(root.join("input/data.csv")).unwrap(), b"abc");
    assert_eq!(record(&root)["version"], "v1.2.4");
}

#[test]
fn versions_pin_or_follow_and_proj_source_resolves_conflicts() {
    let temporary = tempfile::tempdir().unwrap();
    let base = temporary.path().canonicalize().unwrap();
    let (_producer, store) = producer(&base, &["abc", "xyz"]);
    let root = consumer(
        &base,
        &store,
        "  source:\n    arch: {version: \"1.2.4\", cue: if-change}\n",
    );
    success(proj(&root, &["build"]));
    assert_eq!(fs::read(root.join("input/data.csv")).unwrap(), b"abc");
    // A pinned build already in place needs no network access.
    let moved = base.join("store-away");
    fs::rename(&store, &moved).unwrap();
    success(proj(&root, &["build"]));
    fs::rename(&moved, &store).unwrap();

    // A major version follows the newest completed build within it.
    let value = success(proj(&root, &["--machine", "source", "--version", "1"]));
    let label = &value["result"]["source"]["labels"][0];
    assert_eq!(label["status"], "update");
    assert_eq!(label["version"], "v1.2.5");
    assert_eq!(fs::read(root.join("input/data.csv")).unwrap(), b"xyz");
    let error = failure(proj(&root, &["source", "--version", "1.3"]));
    assert!(
        error.contains("has no completed build matching version 1.3"),
        "{error}"
    );
    let error = failure(proj(&root, &["source", "--version", "1.2.3.4"]));
    assert!(error.contains("--version must be latest"), "{error}");

    // Local changes since the last fetch: error, keep or replace. Nothing
    // changes until every label is checked.
    fs::write(root.join("input/data.csv"), "mine").unwrap();
    fs::write(root.join("input/extra.txt"), "local only").unwrap();
    let error = failure(proj(&root, &["source", "--version", "1.2.4"]));
    assert!(
        error.contains("local changes would be overwritten"),
        "{error}"
    );
    assert!(error.contains("data.csv, extra.txt"), "{error}");
    assert_eq!(fs::read(root.join("input/data.csv")).unwrap(), b"mine");
    let value = success(proj(
        &root,
        &[
            "--machine",
            "source",
            "--version",
            "1.2.4",
            "--conflict",
            "local",
        ],
    ));
    assert_eq!(value["result"]["source"]["labels"][0]["kept"], 2);
    assert_eq!(fs::read(root.join("input/data.csv")).unwrap(), b"mine");
    assert_eq!(
        record(&root)["kept"],
        serde_json::json!(["data.csv", "extra.txt"])
    );
    success(proj(
        &root,
        &["source", "--version", "1.2.4", "--conflict", "remote"],
    ));
    assert_eq!(fs::read(root.join("input/data.csv")).unwrap(), b"abc");
    assert!(
        !root.join("input/extra.txt").exists(),
        "sync-diff removes extras"
    );
    // download-missing adds files without touching existing ones.
    fs::remove_file(root.join("input/sub/notes.txt")).unwrap();
    fs::write(root.join("input/data.csv"), "mine").unwrap();
    success(proj(
        &root,
        &[
            "source",
            "--strategy",
            "download-missing",
            "--conflict",
            "error",
            "--version",
            "1.2.4",
        ],
    ));
    assert_eq!(
        fs::read(root.join("input/sub/notes.txt")).unwrap(),
        b"nested"
    );
    assert_eq!(fs::read(root.join("input/data.csv")).unwrap(), b"mine");

    // proj source holds the build writer claim.
    fs::write(root.join(".projr/build.lock"), "held").unwrap();
    let error = failure(proj(&root, &["source", "--conflict", "remote"]));
    assert!(error.contains("another build or restore"), "{error}");
    fs::remove_file(root.join(".projr/build.lock")).unwrap();
}

#[test]
fn discovery_names_the_single_publisher_and_propagates_listing_errors() {
    let temporary = tempfile::tempdir().unwrap();
    let base = temporary.path().canonicalize().unwrap();
    let (producer, store) = producer(&base, &["abc"]);
    let root = consumer(&base, &store, "");
    let config = fs::read_to_string(root.join("_projr.yml")).unwrap();
    let missing = base.join("never-published");
    fs::write(
        root.join("_projr.yml"),
        format!("{config}    other: {{path: {}}}\n", yaml_path(&missing)),
    )
    .unwrap();
    // An unpublished remote is skipped; the one that published raw-data supplies it.
    let value = success(proj(&root, &["--machine", "source"]));
    assert_eq!(
        value["result"]["source"]["labels"][0]["remote"],
        "local:arch"
    );
    assert_eq!(files(&root.join("input")), files(&producer.join("input")));

    // Decision D5: a remote that cannot be listed is an error, not "not published".
    fs::create_dir_all(&missing).unwrap();
    fs::write(missing.join("manifest.csv"), "not,a,manifest\n\"broken").unwrap();
    fs::write(missing.join("VERSION"), "v1.0.0\n").unwrap();
    let error = failure(proj(&root, &["source"]));
    assert!(error.contains("Cannot list local:other"), "{error}");

    // Two publishers need --remote.
    fs::remove_dir_all(&missing).unwrap();
    copy(&store, &missing);
    let error = failure(proj(&root, &["source"]));
    assert!(
        error.contains("raw-data is published by local:arch and local:other"),
        "{error}"
    );
    success(proj(&root, &["source", "--remote", "local:other"]));
    assert_eq!(record(&root)["remote"], "local:other");
}

fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy(&path, &target);
        } else {
            fs::copy(&path, &target).unwrap();
        }
    }
}

#[test]
fn configuration_is_validated_and_profile_layers_replace_build_source() {
    let temporary = tempfile::tempdir().unwrap();
    let base = temporary.path().canonicalize().unwrap();
    let (_producer, store) = producer(&base, &["abc"]);
    let root = consumer(&base, &store, "  source: arch\n");
    let config = fs::read_to_string(root.join("_projr.yml")).unwrap();
    for (source, expected) in [
        ("{arch: {version: 1.2}}", "must be a string"),
        (
            "{arch: {version: \"1.2.3.4\"}}",
            "must be latest or an x, x.y or x.y.z",
        ),
        ("{arch: {cue: sometimes}}", "cue must be one of"),
        ("{arch: {path: elsewhere}}", "only accepts content, version"),
        ("[cue]", "options belong in a mapping"),
        (
            "{arch: {content: [output]}}",
            "must name declared raw directories",
        ),
        ("missing", "not declared under remotes"),
        ("[arch, local:arch]", "more than once"),
    ] {
        fs::write(
            root.join("_projr.yml"),
            config.replace("  source: arch\n", &format!("  source: {source}\n")),
        )
        .unwrap();
        let error = failure(proj(&root, &["build"]));
        assert!(error.contains(expected), "{source}: {error}");
    }
    fs::write(
        root.join("_projr.yml"),
        config.replace("arch: {path:", "arch: {get: {cue: sometimes}, path:"),
    )
    .unwrap();
    let error = failure(proj(&root, &["build"]));
    assert!(error.contains("get.cue must be one of"), "{error}");

    // A local layer's build.source replaces the base value whole (projr R/yml.R).
    let missing = base.join("never-published");
    fs::write(
        root.join("_projr.yml"),
        config.replace(
            "  source: arch\n",
            "  source:\n    arch: {content: [raw-data]}\n",
        ) + &format!("    other: {{path: {}}}\n", yaml_path(&missing)),
    )
    .unwrap();
    copy(&store, &missing);
    fs::write(
        root.join("_projr-local.yml"),
        "build:\n  source:\n    other: {}\n",
    )
    .unwrap();
    fs::rename(&store, base.join("store-away")).unwrap();
    success(proj(&root, &["build"]));
    assert_eq!(record(&root)["remote"], "local:other");
}

#[test]
fn version_current_restores_the_recorded_inputs() {
    let temporary = tempfile::tempdir().unwrap();
    let base = temporary.path().canonicalize().unwrap();
    let (producer, _store) = producer(&base, &["abc"]);
    let expected = files(&producer.join("input"));
    fs::remove_dir_all(producer.join("input")).unwrap();
    let error = failure(proj(
        &producer,
        &[
            "source",
            "--version",
            "current",
            "--remote",
            "arch",
            "--conflict",
            "local",
        ],
    ));
    assert!(error.contains("restores the recorded files"), "{error}");
    let value = success(proj(
        &producer,
        &[
            "--machine",
            "source",
            "--version",
            "current",
            "--remote",
            "arch",
        ],
    ));
    assert_eq!(value["result"]["source"]["labels"][0]["version"], "v1.2.4");
    assert_eq!(files(&producer.join("input")), expected);
}

/// Fixture Git with no inherited GIT_* overrides or global configuration.
fn git(directory: &Path, args: &[&str]) {
    let mut git = Command::new("git");
    git.current_dir(directory);
    for (key, _) in std::env::vars_os() {
        if key
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            git.env_remove(key);
        }
    }
    let status = git
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", directory.join(".no-global-gitconfig"))
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[test]
fn restore_repo_clones_then_fetches_every_input() {
    let temporary = tempfile::tempdir().unwrap();
    let base = temporary.path().canonicalize().unwrap();
    let (producer, store) = producer(&base, &["abc"]);
    let root = consumer(&base, &store, "");
    // A bare "server" repository holding the consumer's committed configuration.
    let server = base.join("server");
    let bare = server.join("owner/analysis.git");
    fs::create_dir_all(&bare).unwrap();
    git(&base, &["init", "--quiet", "--bare", &yaml_path(&bare)]);
    git(&bare, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    let work = |args: &[&str]| {
        let mut full = vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
        ];
        full.extend_from_slice(args);
        git(&root, &full);
    };
    work(&["init", "--quiet"]);
    work(&["add", "_projr.yml", "hook.py"]);
    work(&["commit", "--quiet", "-m", "fixture"]);
    work(&["push", "--quiet", &yaml_path(&bare), "HEAD:refs/heads/main"]);

    let clones = base.join("clones");
    fs::create_dir(&clones).unwrap();
    let output = command(&clones)
        .env("GITHUB_SERVER_URL", yaml_path(&server))
        .args([
            "--machine",
            "restore-repo",
            "owner/analysis",
            "copy",
            "--remote",
            "local:arch",
        ])
        .output()
        .unwrap();
    let value = success(output);
    assert_eq!(
        value["result"]["restore_repo"]["source"]["labels"][0]["status"],
        "update"
    );
    assert_eq!(
        files(&clones.join("copy/input")),
        files(&producer.join("input"))
    );
    // An existing destination is an honest failure.
    let output = command(&clones)
        .env("GITHUB_SERVER_URL", yaml_path(&server))
        .args(["restore-repo", "owner/analysis", "copy"])
        .output()
        .unwrap();
    let error = failure(output);
    assert!(
        error.contains("git clone of owner/analysis failed"),
        "{error}"
    );
    let output = command(&clones)
        .args(["restore-repo", "../x/y"])
        .output()
        .unwrap();
    assert!(failure(output).contains("owner/repo"));
}

struct Server {
    base: String,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn start(routes: BTreeMap<String, (u16, Vec<u8>)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let halt = stop.clone();
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
                assert_eq!(first.next().unwrap(), "GET", "sources are read-only");
                let path = first.next().unwrap().to_owned();
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
            stop,
            thread: Some(worker),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

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

fn github_assets() -> Vec<(&'static str, Vec<u8>)> {
    let payload = tempfile::tempdir().unwrap();
    let payload_root = Utf8PathBuf::from_path_buf(payload.path().canonicalize().unwrap()).unwrap();
    fs::write(payload_root.join("data.csv"), "abc").unwrap();
    let (zip, _) = proj::github_zip::directory_zip(&payload_root).unwrap();
    let payload_zip = fs::read(zip.path()).unwrap();
    let metadata = tempfile::tempdir().unwrap();
    let meta = Utf8PathBuf::from_path_buf(metadata.path().canonicalize().unwrap()).unwrap();
    proj::manifest_store::write(
        &meta.join("manifests"),
        &[proj::manifest::ManifestRow {
            label: "raw-data".into(),
            r#fn: "data.csv".into(),
            version: "v1.2.4".into(),
            hash: format!("{:x}", md5::compute(b"abc")),
        }],
        None,
        false,
    )
    .unwrap();
    fs::write(meta.join("VERSION"), "Project: v1.2.4\nraw-data: v1.2.4\n").unwrap();
    let bundle = |kind| {
        let zip = proj::github_zip::metadata_zip(Utf8Path::new(&meta), kind).unwrap();
        fs::read(zip.path()).unwrap()
    };
    vec![
        (
            "manifest.zip",
            bundle(proj::github_zip::MetadataKind::Manifest),
        ),
        (
            "VERSION.zip",
            bundle(proj::github_zip::MetadataKind::Version),
        ),
        ("raw-data-v1.2.4.zip", payload_zip),
    ]
}

fn github_consumer(temporary: &Path) -> PathBuf {
    let root = temporary.join("consumer");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("_projr.yml"),
        "directories:\n  raw-data: {path: input}\nremotes:\n  github:\n    archive: {repo: owner/repo}\n",
    )
    .unwrap();
    root
}

#[test]
fn github_sources_are_verified_and_listing_failures_propagate() {
    let temporary = tempfile::tempdir().unwrap();
    let base = temporary.path().canonicalize().unwrap();
    let root = github_consumer(&base);
    let server = Server::start(release("archive", &github_assets()));
    let value = success(
        command(&root)
            .env("GITHUB_API_URL", &server.base)
            .env("GITHUB_PAT", "fixture-token")
            .args(["--machine", "--project"])
            .arg(&root)
            .args(["source", "--remote", "github:archive"])
            .output()
            .unwrap(),
    );
    let label = &value["result"]["source"]["labels"][0];
    assert_eq!(label["remote"], "github:archive");
    assert_eq!(label["version"], "v1.2.4");
    assert_eq!(fs::read(root.join("input/data.csv")).unwrap(), b"abc");
    drop(server);

    // Discovery: a Release listing that fails is an error (decision D5).
    fs::remove_dir_all(root.join("input")).unwrap();
    let mut routes = BTreeMap::new();
    routes.insert(
        "/repos/owner/repo/releases/tags/archive".to_owned(),
        (500, b"{}".to_vec()),
    );
    let server = Server::start(routes);
    let error = failure(
        command(&root)
            .env("GITHUB_API_URL", &server.base)
            .env("GITHUB_PAT", "fixture-token")
            .args(["--non-interactive", "--project"])
            .arg(&root)
            .arg("source")
            .output()
            .unwrap(),
    );
    assert!(error.contains("Cannot list github:archive"), "{error}");
    assert!(!error.contains("fixture-token"), "{error}");
    assert!(!root.join("input").exists());
}
