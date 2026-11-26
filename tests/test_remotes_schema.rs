//! projr 0.9 remotes schema (SATVILab/projr@5f915ef): `build.dest` selection, destination placeholders, GitHub `repo`/`branch`/`tag` and the
//! pre-render archive version check. Local fixtures and a loopback GitHub mock only.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

fn command(root: &Path, program: &str) -> Command {
    let mut command = Command::new(program);
    command.current_dir(root);
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy().to_ascii_uppercase();
        if ["GIT_", "GH_", "GITHUB_", "PROJR_", "PROJCLI_"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
            || matches!(name.as_str(), "QUARTO_PROFILE" | "SSH_AUTH_SOCK")
        {
            command.env_remove(key);
        }
    }
    command
        .env_remove(proj::PROJECT_ROOT_ENV)
        .env_remove(proj::RUN_CONTEXT_ENV)
        .env("HOME", root.join("home"))
        .env("USERPROFILE", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("home"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", root.join("home/gitconfig"))
        .env("GIT_TERMINAL_PROMPT", "0");
    command
}

fn build(root: &Path, args: &[&str], api: Option<&str>) -> Output {
    let mut command = command(root, env!("CARGO_BIN_EXE_proj"));
    if let Some(api) = api {
        command
            .env("GITHUB_API_URL", api)
            .env("GITHUB_PAT", "fixture-token");
    }
    command
        .args(["--non-interactive", "--project"])
        .arg(root)
        .arg("build")
        .args(args)
        .output()
        .unwrap()
}

fn ok(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn failed(output: Output) -> String {
    assert!(!output.status.success());
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A project with one output file, a pre-hook marker and a Git checkout on `feature/foo`.
fn project(config: Value) -> (tempfile::TempDir, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap().join("project");
    for name in ["results", "home"] {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    fs::write(root.join("results/out.csv"), "result\n").unwrap();
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    fs::write(
        root.join("pre.py"),
        "from pathlib import Path\nPath('hook-ran').write_text('yes')\n",
    )
    .unwrap();
    write_config(&root, config);
    for args in [
        vec!["init", "-q"],
        vec!["symbolic-ref", "HEAD", "refs/heads/feature/foo"],
    ] {
        assert!(
            command(&root, "git")
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    (temporary, root)
}

fn write_config(root: &Path, mut config: Value) {
    config["directories"] = json!({"output":{"path":"results"}});
    config["build"]["scripts"] = json!([]);
    config["build"]["git"] = json!(false);
    config["build"]["hooks"] = json!({"pre":["pre.py"]});
    fs::write(
        root.join("_proj.yml"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .unwrap();
}

fn attempt_record(root: &Path) -> Value {
    let mut records: Vec<_> = fs::read_dir(root.join(".proj/log"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "json")
                && !path.to_string_lossy().ends_with(".git.json")
        })
        .collect();
    records.sort();
    serde_json::from_slice(&fs::read(records.last().unwrap()).unwrap()).unwrap()
}

#[test]
fn local_placeholders_resolve_once_and_are_recorded() {
    let (_temporary, root) = project(json!({
        "remotes":{"local":{"store":{"path":"_archive/@branch/@version-@uuid"},"unused":{"path":"../unused"}}},
        "build":{"dest":"store"}
    }));
    ok(build(&root, &[], None));
    let branch = root.join("_archive/feature_2ffoo");
    let builds: Vec<_> = fs::read_dir(&branch)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(builds.len(), 1);
    let uuid = builds[0].strip_prefix("v1.2.4-").unwrap();
    assert_eq!(uuid.len(), 36);
    assert_eq!(
        fs::read(branch.join(&builds[0]).join("output/v1.2.4/out.csv")).unwrap(),
        b"result\n"
    );
    assert!(!root.parent().unwrap().join("unused").exists());
    let record = attempt_record(&root);
    assert_eq!(record["destinations"]["uuid"], uuid);
    assert_eq!(
        record["destinations"]["targets"],
        json!([{"provider":"local","title":"store","name":"store",
            "path":format!("_archive/feature_2ffoo/v1.2.4-{uuid}")}])
    );
    // Each build gets its own UUID and therefore its own empty archive.
    ok(build(&root, &[], None));
    assert_eq!(fs::read_dir(&branch).unwrap().count(), 2);
}

#[test]
fn archive_version_check_refuses_before_hooks_and_honours_rule_and_opt_out() {
    let (_temporary, root) = project(json!({
        "remotes":{"local":{"store":{"path":"../archive"}}},
        "build":{"dest":"store"}
    }));
    let archive = root.parent().unwrap().join("archive");
    fs::create_dir_all(&archive).unwrap();
    fs::write(archive.join("VERSION"), "Project: v1.2.4\noutput: v1.2.4\n").unwrap();
    let error = failed(build(&root, &[], None));
    assert!(
        error.contains("local.store: v1.2.4, proposed version must be newer"),
        "{error}"
    );
    assert!(!root.join("hook-ran").exists());
    assert_eq!(
        fs::read_to_string(root.join("VERSION")).unwrap(),
        "v1.2.3\n"
    );

    // A development checkout without a completed build is behind the archive.
    fs::write(archive.join("VERSION"), "Project: v1.5.0\n").unwrap();
    fs::write(root.join("VERSION"), "v1.2.3.1\n").unwrap();
    let error = failed(build(&root, &["--major"], None));
    assert!(error.contains("v1.5.0, local base is behind"), "{error}");
    assert!(error.contains("rule: target"), "{error}");
    assert!(!root.join("hook-ran").exists());

    let mut config: Value =
        serde_json::from_slice(&fs::read(root.join("_proj.yml")).unwrap()).unwrap();
    config["build"]["restrictions"] = json!({"rule":"target"});
    write_config(&root, config.clone());
    let report = ok(build(&root, &["--major"], None));
    assert!(
        report.contains("local.store: v1.5.0, check passed"),
        "{report}"
    );
    assert!(archive.join("output/v2.0.0/out.csv").exists());

    // Equal versions are refused under both rules; false skips the check, so
    // the build reaches its hooks (publication may then still refuse the version).
    fs::remove_file(root.join("hook-ran")).unwrap();
    fs::write(root.join("VERSION"), "v1.9.9\n").unwrap();
    assert!(failed(build(&root, &["--major"], None)).contains("proposed version must be newer"));
    assert!(!root.join("hook-ran").exists());
    config["build"]["restrictions"] = json!({"not_behind_remote":false});
    write_config(&root, config);
    let _ = build(&root, &["--major"], None);
    assert!(root.join("hook-ran").exists());
}

#[test]
fn populated_archives_without_a_version_and_named_titles_are_checked() {
    let (_temporary, root) = project(json!({
        "remotes":{"local":{"store":{"path":"../archive"},"other":{"path":"../other"}}},
        "build":{"dest":"store"}
    }));
    let parent = root.parent().unwrap().to_owned();
    fs::create_dir_all(parent.join("archive/stray")).unwrap();
    let error = failed(build(&root, &[], None));
    assert!(error.contains("could not verify"), "{error}");
    fs::remove_dir(parent.join("archive/stray")).unwrap();

    // A listed title is checked even when the build does not publish to it.
    fs::create_dir_all(parent.join("other")).unwrap();
    fs::write(parent.join("other/VERSION"), "v3.0.0\n").unwrap();
    let mut config: Value =
        serde_json::from_slice(&fs::read(root.join("_proj.yml")).unwrap()).unwrap();
    config["build"]["restrictions"] = json!({"not_behind_remote":["other"]});
    write_config(&root, config.clone());
    assert!(
        failed(build(&root, &[], None))
            .contains("local.other: v3.0.0, proposed version must be newer")
    );
    config["build"]["restrictions"] = json!({"not_behind_remote":["local:missing"]});
    write_config(&root, config.clone());
    assert!(failed(build(&root, &[], None)).contains("not declared under remotes"));
    // projr 0.9 rejects the type-keyed mapping; names are qualified instead.
    config["build"]["restrictions"] = json!({"not_behind_remote":{"local":["other"]}});
    write_config(&root, config.clone());
    assert!(failed(build(&root, &[], None)).contains("<type>:<title>"));
    config["remotes"]["github"] = json!({"other":{}});
    config["build"]["restrictions"] = json!({"not_behind_remote":"other"});
    write_config(&root, config.clone());
    assert!(failed(build(&root, &[], None)).contains("Use local:other or github:other"));
    config["build"]["restrictions"] = json!({"not_behind_remote":["local:other"]});
    write_config(&root, config);
    assert!(
        failed(build(&root, &[], None))
            .contains("local.other: v3.0.0, proposed version must be newer")
    );
    assert!(!root.join("hook-ran").exists());
}

#[test]
fn build_dest_forms_and_inline_archives_follow_projr_09() {
    let (_temporary, root) = project(json!({
        "remotes":{"local":{"store":{"path":"../archive"}}},
        "build":{"local":{"store":{"path":"../elsewhere"}}}
    }));
    assert!(failed(build(&root, &[], None)).contains(
        "build.local is no longer supported. Define each archive under remotes.local.<title> and select it with build.dest"
    ));
    let mut config = json!({
        "remotes":{"local":{"store":{"path":"../archive"}}},
        "build":{"dest":{"store":{"path":"../elsewhere"}}}
    });
    write_config(&root, config.clone());
    assert!(
        failed(build(&root, &[], None)).contains("only accepts content, cue, inspect and strategy")
    );
    config["build"]["dest"] = json!(["undefined"]);
    write_config(&root, config.clone());
    assert!(failed(build(&root, &[], None)).contains("not declared under remotes"));
    // The same title can name a GitHub and a local archive; qualify it.
    config["remotes"]["github"] = json!({"store":{}});
    config["build"]["dest"] = json!(["store"]);
    write_config(&root, config.clone());
    assert!(failed(build(&root, &[], None)).contains("Use local:store or github:store"));
    assert!(!root.join("hook-ran").exists());
    config["build"]["dest"] = json!({"content":["output"],"local:store":{}});
    write_config(&root, config);
    ok(build(&root, &[], None));
    assert!(
        root.parent()
            .unwrap()
            .join("archive/output/v1.2.4/out.csv")
            .exists()
    );
}

#[test]
fn profile_layers_replace_build_dest_whole() {
    let (_temporary, root) = project(json!({
        "remotes":{"local":{"base":{"path":"../base"},"profiled":{"path":"../profiled"}}},
        "build":{"dest":{"base":{}}}
    }));
    fs::write(
        root.join("_projr-local.yml"),
        "build:\n  dest:\n    profiled: {}\n",
    )
    .unwrap();
    ok(build(&root, &[], None));
    let parent = root.parent().unwrap();
    assert!(parent.join("profiled/output/v1.2.4/out.csv").exists());
    assert!(!parent.join("base").exists());
}

#[derive(Default)]
struct State {
    release: bool,
    branch: Option<String>,
    tag: Option<String>,
    assets: BTreeMap<String, (u64, Vec<u8>)>,
    created: Vec<Value>,
    notes: Vec<String>,
    paths: Vec<String>,
}
struct Server {
    base: String,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    /// Serves one repository, team/results, holding the Release `figures-v1.2.4`.
    fn start(state: State) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(state));
        let stop = Arc::new(AtomicBool::new(false));
        let (captured, halt, origin) = (state.clone(), stop.clone(), base.clone());
        let release = |origin: &str, note: Option<&String>| {
            json!({"id":7,"tag_name":"figures-v1.2.4","body":note,
                "upload_url":format!("{origin}/upload{{?name,label}}")})
            .to_string()
            .into_bytes()
        };
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
                let (method, path) = (first.next().unwrap(), first.next().unwrap().to_owned());
                let length = text
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                let mut data = captured.lock().unwrap();
                data.paths.push(format!("{method} {path}"));
                let mut status = 200;
                let response = if path == "/repos/team/results/releases/tags/figures-v1.2.4" {
                    if data.release {
                        release(&origin, data.notes.last())
                    } else {
                        status = 404;
                        b"{}".to_vec()
                    }
                } else if path == "/repos/team/results/git/ref/heads/feature%2Ffoo" {
                    match &data.branch {
                        Some(sha) => json!({"object":{"type":"commit","sha":sha}})
                            .to_string()
                            .into_bytes(),
                        None => {
                            status = 404;
                            b"{}".to_vec()
                        }
                    }
                } else if path == "/repos/team/results/git/ref/tags/figures-v1.2.4" {
                    match &data.tag {
                        Some(sha) => json!({"object":{"type":"commit","sha":sha}})
                            .to_string()
                            .into_bytes(),
                        None => {
                            status = 404;
                            b"{}".to_vec()
                        }
                    }
                } else if method == "POST" && path == "/repos/team/results/releases" {
                    let value: Value = serde_json::from_slice(&body).unwrap();
                    data.notes
                        .push(value["body"].as_str().unwrap_or_default().to_owned());
                    data.created.push(value);
                    data.release = true;
                    status = 201;
                    release(&origin, data.notes.last())
                } else if method == "PATCH" && path == "/repos/team/results/releases/7" {
                    let value: Value = serde_json::from_slice(&body).unwrap();
                    data.notes.push(value["body"].as_str().unwrap().to_owned());
                    release(&origin, data.notes.last())
                } else if path.starts_with("/repos/team/results/releases/7/assets?") {
                    serde_json::to_vec(
                        &data
                            .assets
                            .iter()
                            .map(|(name, (id, bytes))| json!({"id":id,"name":name,"size":bytes.len()}))
                            .collect::<Vec<_>>(),
                    )
                    .unwrap()
                } else if method == "POST" && path.starts_with("/upload?name=") {
                    let name = path.strip_prefix("/upload?name=").unwrap().to_owned();
                    let id = 100 + data.assets.len() as u64;
                    let size = body.len();
                    data.assets.insert(name.clone(), (id, body));
                    status = 201;
                    json!({"id":id,"name":name,"size":size})
                        .to_string()
                        .into_bytes()
                } else if let Some(id) = path.strip_prefix("/repos/team/results/releases/assets/") {
                    let id: u64 = id.parse().unwrap();
                    let name = data
                        .assets
                        .iter()
                        .find(|(_, value)| value.0 == id)
                        .map(|(name, _)| name.clone())
                        .unwrap();
                    if method == "DELETE" {
                        data.assets.remove(&name);
                        status = 204;
                        Vec::new()
                    } else {
                        data.assets[&name].1.clone()
                    }
                } else {
                    status = 500;
                    format!("unexpected {method} {path}").into_bytes()
                };
                write!(
                    stream,
                    "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                )
                .unwrap();
                stream.write_all(&response).unwrap();
            }
        });
        Self {
            base,
            state,
            stop,
            thread: Some(worker),
        }
    }
    fn finish(mut self) -> State {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
        std::mem::take(&mut *self.state.lock().unwrap())
    }
}

fn github_project() -> (tempfile::TempDir, PathBuf) {
    project(json!({
        "remotes":{"github":{"reports":{"repo":"team/results","tag":"figures-@version","branch":"@branch",
            "send":{"inspect":"none"}}}},
        "build":{"dest":{"reports":{"content":["output"]}}}
    }))
}

fn version_zip() -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file("VERSION", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer
        .write_all(b"Project: v1.2.4\noutput: v1.2.4\n")
        .unwrap();
    writer.finish().unwrap().into_inner()
}

#[test]
fn github_repo_branch_and_templated_tag_create_the_release_at_the_branch_head() {
    let sha = "a".repeat(40);
    let server = Server::start(State {
        branch: Some(sha.clone()),
        ..State::default()
    });
    let (_temporary, root) = github_project();
    ok(build(&root, &[], Some(&server.base)));
    let state = server.finish();
    assert_eq!(state.created.len(), 1);
    assert_eq!(state.created[0]["tag_name"], "figures-v1.2.4");
    assert_eq!(state.created[0]["target_commitish"], sha.as_str());
    // The Release is created before the build with a pending note, which a
    // successful publication replaces.
    assert_eq!(state.notes.len(), 2);
    assert!(state.notes[0].contains("has not finished publishing"));
    assert_eq!(state.notes[1], "Published by projcli build v1.2.4.");
    assert!(state.assets.contains_key("output-v1.2.4.zip"));
    assert!(
        state
            .paths
            .iter()
            .all(|path| !path.contains("/repos/owner/"))
    );
    let record = attempt_record(&root);
    assert_eq!(
        record["destinations"]["targets"],
        json!([{"provider":"github","title":"reports","repository":"team/results","tag":"figures-v1.2.4"}])
    );
    assert!(
        !serde_json::to_string(&record)
            .unwrap()
            .contains("127.0.0.1")
    );
}

#[test]
fn missing_branch_or_conflicting_tag_stops_before_hooks_without_writes() {
    for (branch, tag, message) in [
        (None, None, "does not identify a commit"),
        (
            Some("a".repeat(40)),
            Some("b".repeat(40)),
            "does not point to branch",
        ),
    ] {
        let server = Server::start(State {
            branch,
            tag,
            ..State::default()
        });
        let (_temporary, root) = github_project();
        let error = failed(build(&root, &[], Some(&server.base)));
        assert!(error.contains(message), "{error}");
        assert!(!root.join("hook-ran").exists());
        let state = server.finish();
        assert!(state.created.is_empty() && state.assets.is_empty());
        assert!(state.paths.iter().all(|path| path.starts_with("GET ")));
    }
}

#[test]
fn github_archive_version_check_reads_version_zip_read_only() {
    let mut assets = BTreeMap::new();
    assets.insert("VERSION.zip".to_owned(), (1, version_zip()));
    let server = Server::start(State {
        release: true,
        assets,
        ..State::default()
    });
    let (_temporary, root) = github_project();
    let error = failed(build(&root, &[], Some(&server.base)));
    assert!(
        error.contains("github.reports: v1.2.4, proposed version must be newer"),
        "{error}"
    );
    assert!(!root.join("hook-ran").exists());
    let state = server.finish();
    assert!(state.paths.iter().all(|path| path.starts_with("GET ")));
    assert_eq!(state.assets.len(), 1);
}

#[test]
fn base_rule_uses_the_last_completed_build_and_previews_skip_placeholders() {
    let (_temporary, root) = project(json!({
        "remotes":{"local":{"store":{"path":"../archive"}}},
        "build":{"dest":"store"}
    }));
    ok(build(&root, &[], None));
    fs::write(root.join("results/out.csv"), "second\n").unwrap();
    // VERSION is now a development version; the completed v1.2.4 attempt is the base.
    let report = ok(build(&root, &[], None));
    assert!(
        report.contains("Local base: v1.2.4, proposed: v1.2.5"),
        "{report}"
    );
    assert!(
        report.contains("local.store: v1.2.4, check passed"),
        "{report}"
    );

    // A preview publishes nothing, so @branch outside a Git checkout is not resolved.
    fs::remove_dir_all(root.join(".git")).unwrap();
    let config = json!({
        "remotes":{"local":{"store":{"path":"../archive-@branch"}}},
        "build":{"dest":"store"}
    });
    write_config(&root, config);
    let mut preview = command(&root, env!("CARGO_BIN_EXE_proj"));
    ok(preview
        .args(["--non-interactive", "--project"])
        .arg(&root)
        .arg("build-dev")
        .output()
        .unwrap());
    assert!(failed(build(&root, &[], None)).contains("@branch needs a named Git branch"));
}

#[test]
fn existing_release_needs_no_branch_resolution() {
    let server = Server::start(State {
        release: true,
        ..State::default()
    });
    let (_temporary, root) = github_project();
    // No checkout: `branch: "@branch"` matters only when a Release must be created.
    fs::remove_dir_all(root.join(".git")).unwrap();
    ok(build(&root, &[], Some(&server.base)));
    let state = server.finish();
    assert!(state.created.is_empty());
    assert!(state.assets.contains_key("output-v1.2.4.zip"));
    assert!(state.paths.iter().all(|path| !path.contains("/git/ref/")));
}

#[test]
fn release_is_created_before_hooks_and_kept_when_the_build_fails() {
    let server = Server::start(State {
        branch: Some("a".repeat(40)),
        ..State::default()
    });
    let (_temporary, root) = github_project();
    // The hook announces itself to the mock, so request order shows hook timing.
    let hook = format!(
        "import urllib.request\ntry:\n    urllib.request.urlopen('{}/hook-ran')\nexcept Exception:\n    pass\nraise SystemExit(3)\n",
        server.base
    );
    fs::write(root.join("pre.py"), hook).unwrap();
    let error = failed(build(&root, &[], Some(&server.base)));
    assert!(
        error.contains("figures-v1.2.4 were created before the failure"),
        "{error}"
    );
    let state = server.finish();
    let created = state
        .paths
        .iter()
        .position(|path| path == "POST /repos/team/results/releases")
        .expect("Release created");
    let hook = state
        .paths
        .iter()
        .position(|path| path == "GET /hook-ran")
        .expect("hook ran");
    assert!(created < hook);
    assert_eq!(state.created.len(), 1);
    // projr: the empty Release stays, with its pending note; nothing is deleted.
    assert_eq!(state.notes.len(), 1);
    assert!(state.notes[0].contains("has not finished publishing"));
    assert!(state.assets.is_empty());
    assert_eq!(state.paths.len(), hook + 1);
}

#[test]
fn a_later_successful_build_replaces_a_stale_pending_note() {
    let server = Server::start(State {
        branch: Some("a".repeat(40)),
        ..State::default()
    });
    let (_temporary, root) = github_project();
    fs::write(root.join("pre.py"), "raise SystemExit(3)\n").unwrap();
    failed(build(&root, &[], Some(&server.base)));
    fs::write(root.join("pre.py"), "pass\n").unwrap();
    ok(build(&root, &[], Some(&server.base)));
    let state = server.finish();
    assert_eq!(state.created.len(), 1);
    assert_eq!(state.notes.len(), 2);
    assert_eq!(state.notes[1], "Published by projcli build v1.2.4.");
    assert!(state.assets.contains_key("output-v1.2.4.zip"));
}

#[test]
fn base_rule_counts_a_committed_snapshot_whose_receipt_is_not_in_the_clone() {
    let (temporary, root) = project(json!({
        "remotes":{"local":{"store":{"path":"../archive"}}},
        "build":{"dest":"store"}
    }));
    fs::write(root.join("pre.py"), "pass\n").unwrap();
    let mut config: Value =
        serde_json::from_slice(&fs::read(root.join("_proj.yml")).unwrap()).unwrap();
    config["build"]["git"] = json!({"commit": true});
    fs::write(
        root.join("_proj.yml"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .unwrap();
    let identity = "[user]\n\tname = Fixture\n\temail = fixture@example.invalid\n";
    fs::write(root.join("home/gitconfig"), identity).unwrap();
    for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "fixture"]] {
        assert!(
            command(&root, "git")
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    ok(build(&root, &[], None));
    // The terminal receipt is untracked until the next build commits it.
    let clone = temporary.path().canonicalize().unwrap().join("clone");
    assert!(
        command(&root, "git")
            .args(["clone", "-q"])
            .arg(&root)
            .arg(&clone)
            .output()
            .unwrap()
            .status
            .success()
    );
    fs::create_dir_all(clone.join("home")).unwrap();
    fs::write(clone.join("home/gitconfig"), identity).unwrap();
    assert!(
        !fs::read_dir(clone.join(".proj/log")).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".git.json")
        })
    );
    config["build"]["git"] = json!(false);
    fs::write(
        clone.join("_proj.yml"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .unwrap();
    let report = ok(build(&clone, &[], None));
    assert!(
        report.contains("Local base: v1.2.4, proposed: v1.2.5"),
        "{report}"
    );
}
