use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, directory: &Path, entries: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_owned();
            if path.is_dir() {
                entries.insert(relative, None);
                visit(root, &path, entries);
            } else {
                entries.insert(relative, Some(fs::read(path).unwrap()));
            }
        }
    }
    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}

#[test]
fn cli_queries_preserve_files_and_directories_with_or_without_configuration() {
    for configuration in [
        None,
        Some("directories: {}\n"),
        Some("directories: [malformed\n"),
        Some("directories:\n  output:\n    path: _output\n"),
    ] {
        for existing_ignores in [false, true] {
            let project = tempfile::tempdir().unwrap();
            let root = project.path();
            fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
            if let Some(content) = configuration {
                fs::write(root.join("_proj.yml"), content).unwrap();
            }
            if existing_ignores {
                fs::write(root.join(".gitignore"), "# preserve git rules\n").unwrap();
                fs::write(root.join(".Rbuildignore"), "# preserve R rules\n").unwrap();
            }
            fs::create_dir(root.join("existing directory")).unwrap();
            let before = snapshot(root);
            for args in [
                vec!["yml", "read"],
                vec![
                    "path", "get", "--label", "output", "--target", "final", "--format", "absolute",
                ],
                vec!["version", "get"],
            ] {
                let output = Command::new(env!("CARGO_BIN_EXE_proj"))
                    .args(&args)
                    .arg("--project")
                    .arg(root)
                    .env_remove(proj::PROJECT_ROOT_ENV)
                    .env_remove("PROJR_PROFILE")
                    .env_remove("QUARTO_PROFILE")
                    .output()
                    .unwrap();
                if (configuration == Some("directories: [malformed\n") && args[0] != "version")
                    || (args[0] == "path"
                        && configuration != Some("directories:\n  output:\n    path: _output\n"))
                {
                    assert!(!output.stderr.is_empty());
                } else {
                    assert!(
                        output.status.success(),
                        "{}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    assert!(!output.stdout.is_empty());
                }
                assert_eq!(
                    snapshot(root),
                    before,
                    "query {args:?} modified the project"
                );
            }
        }
    }
}

#[test]
fn library_config_reads_are_nonmutating() {
    let project = tempfile::tempdir().unwrap();
    let root = camino::Utf8Path::from_path(project.path()).unwrap();
    let before = snapshot(project.path());
    for is_dev in [false, true] {
        let config = proj::yml_read_from(root, is_dev).unwrap();
        assert!(config.directories.is_empty());
        assert!(config.get_path(root, "output").is_err());
        assert_eq!(snapshot(project.path()), before);
    }
}

#[test]
fn production_build_still_requests_ignore_synchronization() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    fs::write(root.join("_proj.yml"), "directories:\n  cache:\n    path: _tmp\nconfig:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: [analysis.py]\n  git: false\n  restrictions:\n    not_behind: false\n").unwrap();
    fs::write(root.join("analysis.py"), "from pathlib import Path\nPath('_tmp').mkdir(exist_ok=True)\nPath('_tmp/production.txt').write_text('done')\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build", "--patch", "--description", "fixture", "--project"])
        .arg(root)
        .current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV)
        .env_remove("PROJR_PROFILE")
        .env_remove("QUARTO_PROFILE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.join(".gitignore").exists());
    assert!(root.join(".Rbuildignore").exists());
    assert_eq!(
        fs::read_to_string(root.join("_tmp/production.txt")).unwrap(),
        "done"
    );
}

fn project_with_custom_ignores() -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    let canonical_root = project.path().canonicalize().unwrap();
    let root = camino::Utf8Path::from_path(&canonical_root).unwrap();
    fs::write(
        root.join("_proj.yml"),
        "directories:\n  raw-private:\n    path: private-data\n    ignore: all\n",
    )
    .unwrap();
    fs::write(root.join(".gitignore"), "manual-rule\n").unwrap();
    fs::write(root.join(".Rbuildignore"), "^manual-rule$\n").unwrap();
    let config = proj::yml_read_from(root, false).unwrap();
    proj::update_ignores(root, &config).unwrap();
    project
}

fn init_directories(root: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["init", "directories", "--project"])
        .arg(root)
        .env_remove(proj::PROJECT_ROOT_ENV)
        .env_remove("PROJR_PROFILE")
        .env_remove("QUARTO_PROFILE")
        .output()
        .unwrap()
}

#[test]
fn init_preserves_custom_ignore_rules_on_malformed_or_unreadable_config() {
    for unreadable in [false, true] {
        let project = project_with_custom_ignores();
        let root = project.path();
        let config = root.join("_proj.yml");
        if unreadable {
            fs::remove_file(&config).unwrap();
            fs::create_dir(&config).unwrap(); // A directory cannot be read as a YAML file.
        } else {
            fs::write(&config, "directories: [malformed\n").unwrap();
        }
        let before = snapshot(root);
        let output = init_directories(root);
        assert!(!output.status.success());
        assert!(!output.stderr.is_empty());
        assert_eq!(
            snapshot(root),
            before,
            "failed init replaced custom ignores or created directories"
        );
    }
}

#[cfg(unix)]
#[test]
fn init_preserves_custom_ignores_when_config_permission_is_denied() {
    use std::os::unix::fs::PermissionsExt;
    let project = project_with_custom_ignores();
    let root = project.path();
    let before = snapshot(root);
    let config = root.join("_proj.yml");
    let original = fs::metadata(&config).unwrap().permissions();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o000)).unwrap();
    let output = init_directories(root);
    fs::set_permissions(&config, original).unwrap();
    assert!(!output.status.success());
    assert_eq!(snapshot(root), before);
}
