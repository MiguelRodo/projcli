use std::{fs, path::Path, process::Command};

fn archive_contains(directory: &Path, expected: &[u8]) -> bool {
    if !directory.exists() { return false; }
    fs::read_dir(directory).unwrap().any(|entry| {
        let path = entry.unwrap().path();
        if path.is_dir() { archive_contains(&path, expected) }
        else { fs::read(path).unwrap() == expected }
    })
}

#[test]
fn explicit_empty_selection_runs_hooks_without_analysis_cleanup_or_rendering() {
    for selection in ["production", "dev-build-empty", "dev-override-empty"] {
        for clear in ["pre", "post"] {
            for renderer_configs in [false, true] {
                let project = tempfile::tempdir().unwrap();
                let root = project.path();
                fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
                for (directory, content) in [("_output", "released-output"), ("docs", "released-docs")] {
                    fs::create_dir(root.join(directory)).unwrap();
                    fs::write(root.join(directory).join("keep.txt"), content).unwrap();
                }
                fs::write(root.join("analysis.py"), "from pathlib import Path\nPath('analysis-ran').write_text('unexpected')\n").unwrap();
                for (file, label) in [("pre.py", "pre"), ("post.py", "post")] {
                    fs::write(root.join(file), format!("from pathlib import Path\nwith Path('hooks.txt').open('a') as out:\n    out.write('{label}\\n')\n")).unwrap();
                }
                let scripts = if selection == "dev-override-empty" { "[analysis.py]" } else { "[]" };
                let dev = if selection == "dev-override-empty" { "dev:\n  scripts: []\n" } else { "" };
                let config = format!(r#"directories: {{}}
config:
  git:
    use_proj_cred_helper: false
remotes:
  local:
    archive:
      path: _archive
      structure: cas
      inspect: manifest
      content: [output, docs]
build:
  scripts: {scripts}
  hooks:
    pre: [pre.py]
    post: [post.py]
  clear_output: {clear}
  git: false
  restrictions:
    not_behind: false
  dest: [archive]
{dev}"#);
                fs::write(root.join("_proj.yml"), config).unwrap();
                if renderer_configs {
                    // Neither engine configuration should be parsed, rewritten or run.
                    fs::write(root.join("_quarto.yml"), "invalid fixture: [\n").unwrap();
                    fs::write(root.join("_bookdown.yml"), "invalid fixture: [\n").unwrap();
                }
                let is_prod = selection == "production";
                let mut command = Command::new(env!("CARGO_BIN_EXE_proj"));
                if is_prod { command.args(["build", "--patch", "--description", "upload-only fixture"]); }
                else { command.arg("build-dev"); }
                let output = command.arg("--project").arg(root).arg("--non-interactive")
                    .current_dir(root).env_remove(proj::PROJECT_ROOT_ENV)
                    .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT")
                    .output().unwrap();
                assert!(output.status.success(), "{selection} clear={clear}: stdout={} stderr={}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
                assert!(!root.join("analysis-ran").exists());
                assert_eq!(fs::read_to_string(root.join("hooks.txt")).unwrap().replace("\r\n", "\n"), "pre\npost\n");
                assert_eq!(fs::read_to_string(root.join("_output/keep.txt")).unwrap(), "released-output");
                assert_eq!(fs::read_to_string(root.join("docs/keep.txt")).unwrap(), "released-docs");
                if renderer_configs {
                    assert_eq!(fs::read_to_string(root.join("_quarto.yml")).unwrap(), "invalid fixture: [\n");
                    assert_eq!(fs::read_to_string(root.join("_bookdown.yml")).unwrap(), "invalid fixture: [\n");
                }
                if is_prod {
                    assert!(archive_contains(&root.join("_archive"), b"released-output"));
                    assert!(archive_contains(&root.join("_archive"), b"released-docs"));
                    assert_eq!(proj::ProjVersion::parse(&fs::read_to_string(root.join("VERSION")).unwrap()).unwrap().patch, 4);
                } else {
                    assert!(!root.join("_archive").exists());
                    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
                }
            }
        }
    }
}

#[test]
fn absent_dev_scripts_still_fall_back_to_build_scripts() {
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("_proj.yml"), "build:\n  scripts: [analysis.py]\n").unwrap();
    fs::write(project.path().join("analysis.py"), "from pathlib import Path\nPath('_tmp').mkdir(exist_ok=True)\nPath('_tmp/result.txt').write_text('ran')\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build-dev", "--project"]).arg(project.path()).arg("--non-interactive")
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(fs::read_to_string(project.path().join("_tmp/result.txt")).unwrap(), "ran");
}
