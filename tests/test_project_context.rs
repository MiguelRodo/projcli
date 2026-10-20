use std::{fs, process::{Command, Output}};

fn query(project: &std::path::Path, inherited: Option<&std::path::Path>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_proj"));
    command.args(["path", "get", "--label", "output", "--project"]).arg(project)
        .env_remove(proj::PROJECT_ROOT_ENV);
    if let Some(root) = inherited { command.env(proj::PROJECT_ROOT_ENV, root); }
    command.output().unwrap()
}

#[test]
fn explicit_root_overrides_ancestor_and_inherited_context_without_markers() {
    let outer = tempfile::tempdir().unwrap();
    fs::write(outer.path().join("VERSION"), "v1.0.0").unwrap();
    fs::write(outer.path().join("_proj.yml"), "directories: {}\n").unwrap();
    let selected = outer.path().join("selected project");
    fs::create_dir(&selected).unwrap();
    let output = query(&selected, Some(outer.path()));
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let actual = String::from_utf8(output.stdout).unwrap();
    assert_eq!(std::path::Path::new(actual.trim()), selected.canonicalize().unwrap().join("_output"));
    assert!(!outer.path().join(".gitignore").exists());
}

#[test]
fn missing_file_and_invalid_inherited_roots_fail_without_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("file");
    fs::write(&file, "data").unwrap();
    for path in [&file, &temp.path().join("missing")] {
        assert!(!query(path, None).status.success());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["path", "get", "--label", "output"])
        .env(proj::PROJECT_ROOT_ENV, "invalid-relative-root")
        .current_dir(temp.path()).output().unwrap();
    assert!(!output.status.success());
}

#[test]
fn child_path_query_keeps_selected_root_after_cwd_changes() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::create_dir(root.join("scripts")).unwrap();
    fs::create_dir(root.join("elsewhere")).unwrap();
    fs::write(root.join("_proj.yml"), "build:\n  scripts: [scripts/analysis.py]\n").unwrap();
    fs::write(root.join("scripts/analysis.py"), r#"import os, subprocess
from pathlib import Path
root = Path(os.environ['PROJ_PROJECT_ROOT'])
os.chdir(root / 'elsewhere')
path = subprocess.check_output([os.environ['PROJCLI_FIXTURE_BINARY'], 'path', 'get', '--label', 'output'], text=True).strip()
assert Path(path) == root / '_output', (path, root)
(root / '_tmp').mkdir(exist_ok=True)
(root / '_tmp/observed-root.txt').write_text(str(root))
"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build-dev", "--project"]).arg(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .env("PROJCLI_FIXTURE_BINARY", env!("CARGO_BIN_EXE_proj"))
        .output().unwrap();
    assert!(output.status.success(), "stdout={} stderr={}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let observed = fs::read_to_string(root.join("_tmp/observed-root.txt")).unwrap();
    assert_eq!(std::path::Path::new(&observed), root.canonicalize().unwrap());
    assert!(!root.join("VERSION").exists());
}


#[test]
fn direct_build_supplies_child_root_without_replacing_callers_root() {
    let project = tempfile::tempdir().unwrap();
    let root = camino::Utf8Path::from_path(project.path()).unwrap();
    let expected = serde_json::to_string(root.canonicalize_utf8().unwrap().as_str()).unwrap();
    fs::write(root.join("_proj.yml"), "build:\n  scripts: [analysis.py]\n").unwrap();
    let script = format!("import os\nfrom pathlib import Path\nassert Path(os.environ['PROJ_PROJECT_ROOT']) == Path({expected})\nPath('_tmp').mkdir(exist_ok=True)\nPath('_tmp/direct.txt').write_text('done')\n");
    fs::write(root.join("analysis.py"), script).unwrap();
    let before = std::env::var_os(proj::PROJECT_ROOT_ENV);
    proj::build_project(root, proj::BuildMode::Dev, None, None).unwrap();
    assert_eq!(std::env::var_os(proj::PROJECT_ROOT_ENV), before);
    assert_eq!(fs::read_to_string(root.join("_tmp/direct.txt")).unwrap(), "done");
}


#[test]
fn simultaneous_direct_builds_keep_independent_roots() {
    use std::{sync::{Arc, Barrier}, time::{Duration, Instant}};
    let workspace = tempfile::tempdir().unwrap();
    let roots = [workspace.path().join("first project"), workspace.path().join("second project")];
    let release = workspace.path().join("release");
    let binary = serde_json::to_string(env!("CARGO_BIN_EXE_proj")).unwrap();
    let release_literal = serde_json::to_string(release.to_str().unwrap()).unwrap();
    for (index, root) in roots.iter().enumerate() {
        fs::create_dir(root).unwrap();
        let own = serde_json::to_string(root.canonicalize().unwrap().to_str().unwrap()).unwrap();
        let other = serde_json::to_string(roots[1 - index].to_str().unwrap()).unwrap();
        fs::write(root.join("_proj.yml"), "build:\n  scripts: [analysis.py]\n").unwrap();
        let script = format!(r#"import os, subprocess, time
from pathlib import Path
root = Path({own})
(root / 'ready').write_text('ready')
deadline = time.monotonic() + 15
while not Path({release_literal}).exists():
    if time.monotonic() > deadline:
        raise RuntimeError('parent did not release simultaneous builds')
    time.sleep(0.01)
assert Path(os.environ['PROJ_PROJECT_ROOT']) == root
os.chdir({other})
result = subprocess.check_output([{binary}, 'path', 'get', '--label', 'output'], text=True).strip()
assert Path(result) == root / '_output', (result, root)
(root / '_tmp').mkdir(exist_ok=True)
(root / '_tmp/independent.txt').write_text('done')
"#);
        fs::write(root.join("analysis.py"), script).unwrap();
    }
    let before = std::env::var_os(proj::PROJECT_ROOT_ENV);
    let start = Arc::new(Barrier::new(2));
    std::thread::scope(|scope| {
        let handles: Vec<_> = roots.iter().map(|root| {
            let start = start.clone();
            scope.spawn(move || {
                start.wait();
                proj::build_project(camino::Utf8Path::from_path(root).unwrap(), proj::BuildMode::Dev, None, None)
            })
        }).collect();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !roots.iter().all(|root| root.join("ready").exists()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let both_running = roots.iter().all(|root| root.join("ready").exists());
        let during = std::env::var_os(proj::PROJECT_ROOT_ENV);
        fs::write(&release, "continue").unwrap();
        for handle in handles { handle.join().unwrap().unwrap(); }
        assert!(both_running, "both child processes must overlap for this regression");
        assert_eq!(during, before, "a direct build must not replace the caller's root");
    });
    for root in roots {
        assert_eq!(fs::read_to_string(root.join("_tmp/independent.txt")).unwrap(), "done");
    }
}
