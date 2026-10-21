use std::{fs, path::Path, process::Command};

fn nearest_root_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let parent = tempfile::tempdir().unwrap();
    fs::write(parent.path().join("VERSION"), "v9.0.0").unwrap();
    fs::create_dir(parent.path().join(".git")).unwrap();
    fs::write(parent.path().join("_proj.yml"), "directories:\n  output:\n    path: _output\n").unwrap();
    let project = parent.path().join("inner project");
    fs::create_dir(&project).unwrap();
    (parent, project)
}

fn discovered(path: &Path) -> std::path::PathBuf {
    proj::root_from(camino::Utf8Path::from_path(path).unwrap()).unwrap().into_std_path_buf()
}

#[test]
fn nearest_file_marker_wins_without_a_version_prerequisite() {
    for (name, content) in [
        ("VERSION", "v1.0.0"), ("README.md", "research"), ("_quarto.yml", "project: {}"),
        ("_proj.yml", "directories: {}"), ("_projr.yml", "directories: {}"),
        ("pyproject.toml", "[project]"), ("Project.toml", "[deps]"), ("JuliaProject.toml", "[deps]"),
        ("DESCRIPTION", "Title: Research\nPackage: demo\nVersion: 1.0.0\n"),
        ("analysis.Rproj", "Version: 1.0\n"), ("renv.lock", "{\n  \"Packages\": {}\n}"),
        (".git", "gitdir: /fixture/worktree-metadata\n"),
    ] {
        let (_parent, project) = nearest_root_fixture();
        fs::write(project.join(name), content).unwrap();
        let nested = project.join("a/b/c/d/e/f/g/h/i");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(discovered(&nested), project.canonicalize().unwrap(), "marker {name}");
    }
    for name in [".git", ".svn"] {
        let (_parent, project) = nearest_root_fixture();
        fs::create_dir(project.join(name)).unwrap();
        assert_eq!(discovered(&project), project.canonicalize().unwrap());
    }
}

#[test]
fn invalid_structured_markers_and_dependency_files_alone_do_not_select_roots() {
    for (name, content) in [
        ("DESCRIPTION", "Version: 1.0.0\n"), ("analysis.Rproj", "Title: fixture\nVersion: 1.0\n"),
        ("renv.lock", "{\"Packages\": null}"), (".git", "unrelated file\n"),
        ("requirements.txt", "some-package"), ("Manifest.toml", "manifest_format = \"2.0\""),
    ] {
        let (parent, project) = nearest_root_fixture();
        fs::write(project.join(name), content).unwrap();
        assert_eq!(discovered(&project), parent.path().canonicalize().unwrap(), "marker {name}");
    }
}

#[test]
fn discovery_walks_up_and_explicit_selection_still_overrides_a_nearer_marker() {
    let (parent, project) = nearest_root_fixture();
    let buried = project.join("buried/five/levels/down/project");
    fs::create_dir_all(&buried).unwrap();
    fs::write(buried.join("pyproject.toml"), "[project]").unwrap();
    assert_eq!(discovered(&project), parent.path().canonicalize().unwrap());
    fs::write(project.join("README.md"), "nearest root").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["path", "get", "--label", "output", "--target","final","--format","absolute","--project"]).arg(parent.path())
        .current_dir(&project).env_remove(proj::PROJECT_ROOT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
    assert!(output.status.success());
    assert_eq!(Path::new(String::from_utf8(output.stdout).unwrap().trim()), parent.path().canonicalize().unwrap().join("_output"));
}

#[test]
fn configured_python_preview_keeps_shallow_script_discovery() {
    let (_parent, project) = nearest_root_fixture();
    fs::write(project.join("pyproject.toml"), "[project]").unwrap();
    fs::write(project.join("_proj.yml"), "directories:\n  cache:\n    path: _tmp\n").unwrap();
    fs::write(project.join("analysis.py"), "from pathlib import Path\nPath('_tmp').mkdir(exist_ok=True)\nPath('_tmp/result.txt').write_text('ran')\n").unwrap();
    fs::create_dir(project.join("nested")).unwrap();
    fs::write(project.join("nested/should-not-run.py"), "from pathlib import Path\nPath('unexpected').write_text('bad')\nraise SystemExit(17)\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build-dev", "--non-interactive"]).current_dir(&project)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .output().unwrap();
    assert!(output.status.success(), "stdout={} stderr={}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert_eq!(fs::read_to_string(project.join("_tmp/result.txt")).unwrap(), "ran");
    assert!(!project.join("VERSION").exists());
    assert!(!project.join("nested/unexpected").exists());
}


#[test]
fn markerless_discovery_falls_back_to_the_starting_directory() {
    let project = tempfile::tempdir().unwrap();
    let path = camino::Utf8Path::from_path(project.path()).unwrap();
    assert_eq!(proj::root_from(path), Some(path.canonicalize_utf8().unwrap()));
}
