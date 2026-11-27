use std::{fs, path::Path, process::Command};

fn nearest_root_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let parent = tempfile::tempdir().unwrap();
    fs::write(parent.path().join("VERSION"), "v9.0.0").unwrap();
    fs::create_dir(parent.path().join(".git")).unwrap();
    fs::write(
        parent.path().join("_projr.yml"),
        "directories:\n  output:\n    path: _output\n",
    )
    .unwrap();
    let project = parent.path().join("inner project");
    fs::create_dir(&project).unwrap();
    (parent, project)
}

fn discovered(path: &Path) -> std::path::PathBuf {
    proj::root_from(camino::Utf8Path::from_path(path).unwrap())
        .unwrap()
        .into_std_path_buf()
}

/// Generic markers that projr's build fallback recognises but discovery ignores.
const GENERIC: [(&str, &str); 12] = [
    ("VERSION", "v1.0.0"),
    ("README.md", "research"),
    ("_quarto.yml", "project: {}"),
    ("pyproject.toml", "[project]"),
    ("Project.toml", "[deps]"),
    ("JuliaProject.toml", "[deps]"),
    (
        "DESCRIPTION",
        "Title: Research\nPackage: demo\nVersion: 1.0.0\n",
    ),
    ("analysis.Rproj", "Version: 1.0\n"),
    ("renv.lock", "{\n  \"Packages\": {}\n}"),
    (".git", "gitdir: /fixture/worktree-metadata\n"),
    ("requirements.txt", "some-package"),
    ("Manifest.toml", "manifest_format = \"2.0\""),
];

#[test]
fn every_projr_configuration_form_selects_the_nearest_configured_folder() {
    for name in [
        "_projr.yml",
        ".projr/config/default.yml",
        ".projr/config/_projr.yml",
        "_projr/config/default.yml",
        "_projr/config/_projr.yml",
    ] {
        let (_parent, project) = nearest_root_fixture();
        fs::create_dir_all(project.join(name).parent().unwrap()).unwrap();
        fs::write(project.join(name), "directories: {}").unwrap();
        let nested = project.join("a/b/c/d/e/f/g/h/i");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(
            discovered(&nested),
            project.canonicalize().unwrap(),
            "config {name}"
        );
    }
}

#[test]
fn configuration_beats_nearer_generic_markers() {
    // projr's root criterion (R/path.R:260-288): only configuration identifies a root.
    for (name, content) in GENERIC {
        let (parent, project) = nearest_root_fixture();
        fs::write(project.join(name), content).unwrap();
        assert_eq!(
            discovered(&project),
            parent.path().canonicalize().unwrap(),
            "marker {name}"
        );
    }
    for name in [".git", ".svn"] {
        let (parent, project) = nearest_root_fixture();
        fs::create_dir(project.join(name)).unwrap();
        assert_eq!(discovered(&project), parent.path().canonicalize().unwrap());
    }
}

#[test]
fn without_any_configuration_the_starting_directory_is_the_root() {
    let parent = tempfile::tempdir().unwrap();
    for (name, content) in GENERIC {
        fs::write(parent.path().join(name), content).unwrap();
    }
    fs::create_dir(parent.path().join(".svn")).unwrap();
    let start = parent.path().join("analysis");
    fs::create_dir(&start).unwrap();
    assert_eq!(discovered(&start), start.canonicalize().unwrap());
}

#[test]
fn a_first_preview_writes_the_starter_in_the_working_directory() {
    let parent = tempfile::tempdir().unwrap();
    fs::write(parent.path().join("VERSION"), "v1.0.0").unwrap();
    fs::write(parent.path().join("README.md"), "research").unwrap();
    let start = parent.path().join("analysis");
    fs::create_dir(&start).unwrap();
    fs::write(start.join("_projr-local.yml"), "build:\n  scripts: []\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build-dev", "--non-interactive"])
        .current_dir(&start)
        .env_remove(proj::PROJECT_ROOT_ENV)
        .env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE")
        .env_remove("QUARTO_PROFILE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(start.join("_projr.yml").is_file());
    assert!(!parent.path().join("_projr.yml").exists());
    // projr seeds the ignore starter with the configuration, previews included.
    let gitignore = fs::read_to_string(start.join(".gitignore")).unwrap();
    assert!(
        gitignore.lines().any(|line| line == "_projr-local.yml"),
        "{gitignore}"
    );
    assert!(
        gitignore.contains("# Start of projr section"),
        "{gitignore}"
    );
}

#[test]
fn explicit_selection_overrides_discovery() {
    let (parent, project) = nearest_root_fixture();
    fs::write(
        project.join("_projr.yml"),
        "directories:\n  output:\n    path: _inner\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args([
            "path",
            "get",
            "--label",
            "output",
            "--target",
            "final",
            "--format",
            "absolute",
            "--project",
        ])
        .arg(parent.path())
        .current_dir(&project)
        .env_remove(proj::PROJECT_ROOT_ENV)
        .env_remove("PROJR_PROFILE")
        .env_remove("QUARTO_PROFILE")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        Path::new(String::from_utf8(output.stdout).unwrap().trim()),
        parent.path().canonicalize().unwrap().join("_output")
    );
}

#[test]
fn configured_python_preview_keeps_shallow_script_discovery() {
    let (_parent, project) = nearest_root_fixture();
    fs::write(project.join("pyproject.toml"), "[project]").unwrap();
    fs::write(
        project.join("_projr.yml"),
        "directories:\n  cache:\n    path: _tmp\n",
    )
    .unwrap();
    fs::write(project.join("analysis.py"), "from pathlib import Path\nPath('_tmp').mkdir(exist_ok=True)\nPath('_tmp/result.txt').write_text('ran')\n").unwrap();
    fs::create_dir(project.join("nested")).unwrap();
    fs::write(
        project.join("nested/should-not-run.py"),
        "from pathlib import Path\nPath('unexpected').write_text('bad')\nraise SystemExit(17)\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build-dev", "--non-interactive"])
        .current_dir(&project)
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
    assert_eq!(
        fs::read_to_string(project.join("_tmp/result.txt")).unwrap(),
        "ran"
    );
    assert!(!project.join("VERSION").exists());
    assert!(!project.join("nested/unexpected").exists());
}

#[test]
fn a_config_directory_belongs_to_the_surrounding_project() {
    let (_parent, project) = nearest_root_fixture();
    let config = project.join(".projr/config");
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("_projr.yml"), "directories: {}").unwrap();
    // projr's criterion: _projr.yml inside .projr/config does not make that folder a root.
    assert_eq!(discovered(&config), project.canonicalize().unwrap());
    let other = project.join("_projr/config");
    fs::create_dir_all(&other).unwrap();
    assert_eq!(discovered(&other), project.canonicalize().unwrap());
}

#[test]
fn unconfigured_discovery_falls_back_to_the_starting_directory() {
    let project = tempfile::tempdir().unwrap();
    let path = camino::Utf8Path::from_path(project.path()).unwrap();
    assert_eq!(
        proj::root_from(path),
        Some(path.canonicalize_utf8().unwrap())
    );
}
