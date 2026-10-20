use std::{fs, process::Command};

fn configuration(docs: &str) -> String {
    format!("directories:\n  cache:\n    path: _tmp\n  raw:\n    path: _raw_data\n  output:\n    path: _output\n  docs:\n    path: '{docs}'\nconfig:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: [analysis.py]\n  git: false\n  clear_output: post\n  restrictions:\n    not_behind: false\n")
}

fn failed_build_preserves(root: &std::path::Path, configuration: &str) {
    fs::write(root.join("_proj.yml"), configuration).unwrap();
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    fs::write(root.join("analysis.py"), "from pathlib import Path\nPath('analysis-ran').touch()\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build", "--patch", "--non-interactive", "--project"]).arg(root).current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
    assert!(!output.status.success(), "unsafe destination accepted");
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
    assert_eq!(fs::read_to_string(root.join("_proj.yml")).unwrap(), configuration);
    assert!(root.join("analysis.py").is_file());
    for name in ["analysis-ran", ".gitignore", ".Rbuildignore", "_tmp"] { assert!(!root.join(name).exists(), "unexpected {name}"); }
}

#[test]
fn destructive_root_ancestor_staging_overlap_and_traversal_fail_before_mutation() {
    for docs in [".", "..", "_tmp/projr", "_output", "_raw_data/derived", ".git/results"] {
        let temp = tempfile::tempdir().unwrap();
        failed_build_preserves(temp.path(), &configuration(docs));
    }
    let temp = tempfile::tempdir().unwrap();
    failed_build_preserves(temp.path(), &configuration("docs").replace("  output:", "  output/../../escape:"));
}

#[cfg(unix)]
#[test]
fn symlinked_destination_preserves_project_and_external_files() {
    let temp = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    fs::write(external.path().join("keep"), "external").unwrap();
    std::os::unix::fs::symlink(external.path(), temp.path().join("linked")).unwrap();
    failed_build_preserves(temp.path(), &configuration("linked/docs"));
    assert_eq!(fs::read_to_string(external.path().join("keep")).unwrap(), "external");
}

#[test]
fn invalid_directory_moves_do_not_delete_existing_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let canonical = temp.path().canonicalize().unwrap();
    let root = camino::Utf8Path::from_path(&canonical).unwrap();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("keep"), "source").unwrap();
    for destination in [source.clone(), root.to_owned(), source.join("nested")] {
        assert!(proj::dir_move_exact(&source, &destination).is_err());
        assert_eq!(fs::read_to_string(source.join("keep")).unwrap(), "source");
    }
    assert!(proj::dir_move_exact(&root.join("missing"), &source).is_err());
    assert_eq!(fs::read_to_string(source.join("keep")).unwrap(), "source");
}


#[test]
fn disjoint_external_resource_destination_is_allowed() {
    let temp = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let root = temp.path();
    let docs = external.path().canonicalize().unwrap().join("release-docs");
    let quoted = docs.to_string_lossy().replace('\\', "/").replace('\'', "''");
    fs::write(root.join("_proj.yml"), configuration(&quoted).replace("clear_output: post", "clear_output: never")).unwrap();
    fs::write(root.join("analysis.py"), "from pathlib import Path\nPath('_output').mkdir(exist_ok=True)\nPath('_output/result').write_text('completed')\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build", "--patch", "--non-interactive", "--project"]).arg(root).current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
    assert!(output.status.success(), "{} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert_eq!(fs::read_to_string(root.join("_output/result")).unwrap(), "completed");
}


#[test]
fn direct_cleanup_entrypoints_reject_project_root_destination() {
    let temp = tempfile::tempdir().unwrap();
    let canonical = temp.path().canonicalize().unwrap();
    let root = camino::Utf8Path::from_path(&canonical).unwrap();
    fs::write(root.join("keep"), "project").unwrap();
    let config: proj::ProjConfig = serde_yaml::from_str(&configuration(".")).unwrap();
    let validated = config.validate_and_resolve(root, false).unwrap();
    assert!(proj::clear_pre(root, "1.2.3", &validated, Some("pre")).is_err());
    assert!(proj::clear_post(root, false, &validated, Some("post"), true).is_err());
    assert_eq!(fs::read_to_string(root.join("keep")).unwrap(), "project");
}
