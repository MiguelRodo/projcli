use std::{collections::BTreeMap, fs, path::Path, process::Command};

fn files_under(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &Path, path: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap();
            if relative.starts_with("_tmp/preview") { continue; }
            if path.is_dir() { visit(root, &path, files); }
            else { files.insert(relative.to_string_lossy().into_owned(), fs::read(path).unwrap()); }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

fn fixture(root: &Path, clear: &str, fail: bool) {
    fs::write(root.join("VERSION"), "Version: v1.2.3\n").unwrap();
    fs::write(root.join("_proj.yml"), format!(r#"directories: {{}}
remotes:
  local:
    archive:
      path: _archive
      structure: cas
      inspect: manifest
      content: [output]
build:
  scripts: [analysis.py]
  git:
    commit: true
    push: true
  clear_output: {clear}
  dest: [archive]
dev:
  old_dev_remove: true
"#)).unwrap();
    for directory in ["_output", "docs", "_archive", "_tmp/projr/previous"] {
        fs::create_dir_all(root.join(directory)).unwrap();
        fs::write(root.join(directory).join("released.txt"), "keep this evidence").unwrap();
    }
    fs::write(root.join(".gitignore"), "# existing git ignore\n").unwrap();
    fs::write(root.join(".Rbuildignore"), "# existing R ignore\n").unwrap();
    fs::write(root.join("_environment.local"), "PROJCLI_PREVIEW_FIXTURE=ready\n").unwrap();
    let script = format!(r#"import os
from pathlib import Path
assert os.environ['PROJCLI_PREVIEW_FIXTURE'] == 'ready'
target = Path('_tmp/preview')
target.mkdir(parents=True, exist_ok=True)
(target / 'result.csv').write_bytes(b'x,y\n1,2\n')
(target / 'report.txt').write_text('preview complete\n')
raise SystemExit({})
"#, if fail { 7 } else { 0 });
    fs::write(root.join("analysis.py"), script).unwrap();
}

#[test]
fn preview_preserves_release_archive_version_ignores_and_previous_cache() {
    for clear in ["pre", "post"] {
        for fail in [false, true] {
            let project = tempfile::tempdir().unwrap();
            fixture(project.path(), clear, fail);
            let before = files_under(project.path());
            let output = Command::new(env!("CARGO_BIN_EXE_proj"))
                .arg("build-dev").current_dir(project.path())
                .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
                .env_remove("PROJR_CLEAR_OUTPUT")
                .output().unwrap();
            assert_eq!(output.status.success(), !fail,
                "stdout={} stderr={}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            assert_eq!(files_under(project.path()), before, "clear={clear}, fail={fail}");
            assert_eq!(fs::read_to_string(project.path().join("_tmp/preview/result.csv")).unwrap(), "x,y\n1,2\n");
        }
    }
}

#[test]
fn preview_without_version_does_not_create_one() {
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("_proj.yml"), "directories:\n  cache:\n    path: _tmp\n").unwrap();
    fs::write(project.path().join("analysis.py"), "from pathlib import Path\nPath('_tmp').mkdir(exist_ok=True)\nPath('_tmp/result.txt').write_text('done')\n").unwrap();
    let root = camino::Utf8Path::from_path(project.path()).unwrap();
    proj::build_project(root, proj::BuildMode::Dev, None, None).unwrap();
    assert!(!root.join("VERSION").exists());
    assert!(!root.join(".gitignore").exists());
    assert!(!root.join(".Rbuildignore").exists());
    assert_eq!(fs::read_to_string(root.join("_tmp/result.txt")).unwrap(), "done");
}
