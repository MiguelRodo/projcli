use std::{collections::BTreeMap, fs, path::{Path, PathBuf}, process::Command};

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
    for configuration in [None, Some("directories: {}\n"), Some("directories: [malformed\n")] {
        for existing_ignores in [false, true] {
            let project = tempfile::tempdir().unwrap();
            let root = project.path();
            fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
            if let Some(content) = configuration { fs::write(root.join("_proj.yml"), content).unwrap(); }
            if existing_ignores {
                fs::write(root.join(".gitignore"), "# preserve git rules\n").unwrap();
                fs::write(root.join(".Rbuildignore"), "# preserve R rules\n").unwrap();
            }
            fs::create_dir(root.join("existing directory")).unwrap();
            let before = snapshot(root);
            for args in [vec!["yml", "read"], vec!["path", "get", "--label", "output"], vec!["version", "get"]] {
                let output = Command::new(env!("CARGO_BIN_EXE_proj"))
                    .args(&args).arg("--project").arg(root)
                    .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
                    .output().unwrap();
                if configuration == Some("directories: [malformed\n") && args[0] != "version" {
                    assert!(!output.stderr.is_empty());
                } else {
                    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
                    assert!(!output.stdout.is_empty());
                }
                assert_eq!(snapshot(root), before, "query {args:?} modified the project");
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
        assert!(config.get_path(root, "output").unwrap().is_absolute());
        assert_eq!(snapshot(project.path()), before);
    }
}

#[test]
fn production_build_still_requests_ignore_synchronization() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    fs::write(root.join("_proj.yml"), "config:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: [analysis.py]\n  git: false\n  restrictions:\n    not_behind: false\n").unwrap();
    fs::write(root.join("analysis.py"), "from pathlib import Path\nPath('_tmp').mkdir(exist_ok=True)\nPath('_tmp/production.txt').write_text('done')\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build", "--patch", "--description", "fixture", "--project"]).arg(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .output().unwrap();
    assert!(output.status.success(), "stdout={} stderr={}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(root.join(".gitignore").exists());
    assert!(root.join(".Rbuildignore").exists());
    assert_eq!(fs::read_to_string(root.join("_tmp/production.txt")).unwrap(), "done");
}
