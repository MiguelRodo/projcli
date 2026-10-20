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
    fs::write(root.join("_proj.yml"), format!(r#"directories:
  cache:
    path: _tmp
  output:
    path: _output
  docs:
    path: docs
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


fn mutating_preview(root: &Path, empty: bool, fail: bool) {
    fs::write(root.join("VERSION"), b"Version: v1.2.3\r\n").unwrap();
    fs::write(root.join(".gitignore"), b"# original ignores\n").unwrap();
    fs::write(root.join("README.md"), b"original README\n").unwrap();
    fs::write(root.join("_environment.local"), b"PROJCLI_PREVIEW_STATE=ready\n").unwrap();
    for name in ["_output", "docs", "_archive", ".git"] {
        fs::create_dir(root.join(name)).unwrap();
        fs::write(root.join(name).join("released.bin"), b"original\0release\n").unwrap();
    }
    fs::create_dir(root.join("nested")).unwrap();
    let scripts = if empty { "[]" } else { "[nested/analysis.py]" };
    let hooks = if empty { "dev:\n  hooks:\n    pre: [nested/pre.py]\n    post: [nested/post.py]\n" } else { "" };
    fs::write(root.join("_proj.yml"), format!("directories:\n  cache:\n    path: _tmp\n  output:\n    path: _output\n  docs:\n    path: docs\nremotes:\n  local:\n    archive:\n      path: _archive\n      structure: cas\n      inspect: manifest\n      content: [output]\nbuild:\n  scripts: {scripts}\n  git:\n    commit: true\n    push: true\n  dest: [archive]\n{hooks}")).unwrap();
    let mutation = r#"import os
from pathlib import Path
root = Path(os.environ['PROJ_PROJECT_ROOT'])
assert os.environ['PROJCLI_PREVIEW_STATE'] == 'ready'
for name in ['VERSION', '.gitignore', 'README.md', '_environment.local', '_proj.yml']:
    (root / name).write_bytes(b'changed managed state')
for name in ['_output', 'docs', '_archive', '.git']:
    (root / name / 'released.bin').unlink()
    (root / name / 'new.bin').write_bytes(b'unrequested replacement')
Path(__file__).unlink()
preview = root / '_tmp/preview'
preview.mkdir(parents=True, exist_ok=True)
(preview / 'result.csv').write_bytes(b'x,y\n1,2\n')
"#;
    if empty {
        fs::write(root.join("nested/pre.py"), mutation).unwrap();
        fs::write(root.join("nested/post.py"), format!("import os\nfrom pathlib import Path\nroot = Path(os.environ['PROJ_PROJECT_ROOT'])\n(root / '_tmp/preview/post-ran').write_bytes(b'yes')\n(root / 'VERSION').write_bytes(b'post mutation')\nPath(__file__).write_text('changed hook')\nraise SystemExit({})\n", if fail { 17 } else { 0 })).unwrap();
    } else {
        fs::write(root.join("nested/analysis.py"), format!("{mutation}\nraise SystemExit({})\n", if fail { 17 } else { 0 })).unwrap();
    }
}

#[test]
fn successful_and_failed_previews_restore_mutated_release_source_config_and_publication_state() {
    for empty in [false, true] {
        for fail in [false, true] {
            let project = tempfile::tempdir().unwrap();
            let root = project.path().canonicalize().unwrap();
            mutating_preview(&root, empty, fail);
            let before = files_under(&root);
            let output = Command::new(env!("CARGO_BIN_EXE_proj"))
                .args(["build-dev", "--non-interactive", "--project"]).arg(&root)
                .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE")
                .env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT")
                .output().unwrap();
            assert_eq!(output.status.success(), !fail, "empty={empty}, fail={fail}: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            assert_eq!(files_under(&root), before, "empty={empty}, fail={fail}");
            assert_eq!(fs::read(root.join("_tmp/preview/result.csv")).unwrap(), b"x,y\n1,2\n");
            assert_eq!(root.join("_tmp/preview/post-ran").exists(), empty);
        }
    }
}

#[test]
fn unsafe_preview_recovery_paths_fail_before_analysis() {
    for unsafe_path in [".", "..", "_tmp/archive"] {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
        fs::write(root.join("_proj.yml"), format!("directories:\n  cache:\n    path: _tmp\nremotes:\n  local:\n    archive:\n      path: {unsafe_path}\n      structure: cas\n      inspect: manifest\n      content: []\nbuild:\n  scripts: [analysis.py]\n")).unwrap();
        fs::write(root.join("analysis.py"), "open('analysis-called', 'w').write('bad')\n").unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_proj"))
            .args(["build-dev", "--non-interactive", "--project"]).arg(&root)
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE")
            .env_remove("QUARTO_PROFILE").output().unwrap();
        assert!(!output.status.success(), "{unsafe_path}");
        assert!(!root.join("analysis-called").exists());
        assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
    }
}
