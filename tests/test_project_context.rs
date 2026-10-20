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
