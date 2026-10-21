use std::{fs, process::{Command, Output}};

const PRESET: &str = "directories:\n  raw-data:\n    path: _raw_data\n  cache:\n    path: _tmp\n  output:\n    path: _output\n";

fn cli(root: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_proj")).args(args)
        .arg("--project").arg(root).arg("--non-interactive")
        .current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .output().unwrap()
}

#[test]
fn local_init_writes_explicit_preset_without_disabling_analysis_or_running_setup() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    let result = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["--non-interactive", "--project"]).arg(root).args(["init", "directories"])
        .env("PATH", "").env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(fs::read_to_string(root.join("_proj.yml")).unwrap(), PRESET);
    let config: proj::ProjConfig = serde_yaml::from_str(PRESET).unwrap();
    assert!(config.build.scripts.is_none());
    for name in ["_raw_data", "_tmp", "_output"] { assert!(root.join(name).is_dir()); }
    for name in [".git", "VERSION", "R", "docs"] { assert!(!root.join(name).exists(), "unexpected {name}"); }
    let repeat = cli(root, &["init", "directories"]);
    assert!(repeat.status.success());
    assert_eq!(fs::read_to_string(root.join("_proj.yml")).unwrap(), PRESET);
    fs::write(root.join("analysis.py"), "from pathlib import Path\nPath('_tmp/result').write_text('discovered')\n").unwrap();
    let preview = cli(root, &["build-dev"]);
    assert!(preview.status.success(), "{}", String::from_utf8_lossy(&preview.stderr));
    assert_eq!(fs::read_to_string(root.join("_tmp/result")).unwrap(), "discovered");
}

#[test]
fn existing_primary_and_legacy_configs_keep_bytes_and_empty_script_selection() {
    for name in ["_proj.yml", "_projr.yml"] {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let content = "# preserve this comment\ndirectories:\n  output-special:\n    path: custom-results\nbuild:\n  scripts: []\n";
        fs::write(root.join(name), content).unwrap();
        assert!(cli(root, &["init", "directories"]).status.success());
        assert_eq!(fs::read_to_string(root.join(name)).unwrap(), content);
        assert!(root.join("custom-results").is_dir());
        for absent in ["_tmp", "_output", "_raw_data", "R", "docs"] { assert!(!root.join(absent).exists()); }
        if name == "_projr.yml" { assert!(!root.join("_proj.yml").exists()); }
    }
}

#[test]
fn active_profile_alone_is_preserved_without_inventing_a_base_file() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    let content = "directories:\n  cache:\n    path: scratch\n";
    fs::write(root.join("_projr-fixture.yml"), content).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["init", "directories", "--project"]).arg(root)
        .env("PROJR_PROFILE", "fixture").env_remove("QUARTO_PROFILE").env_remove(proj::PROJECT_ROOT_ENV)
        .output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(!root.join("_proj.yml").exists());
    assert!(root.join("scratch").is_dir());
    assert_eq!(fs::read_to_string(root.join("_projr-fixture.yml")).unwrap(), content);
}

#[test]
fn paths_require_declared_nonempty_mappings_without_suffix_fallback() {
    let project = tempfile::tempdir().unwrap();
    let root = camino::Utf8Path::from_path(project.path()).unwrap();
    for text in ["directories:\n  output: {}\n", "directories:\n  output:\n    path: ''\n", "directories:\n  output:\n    path: '   '\n", "directories:\n  output:\n    path: first\n  OUTPUT:\n    path: second\n"] {
        let raw: proj::ProjConfig = serde_yaml::from_str(text).unwrap();
        assert!(raw.validate_and_resolve(root, false).is_err(), "accepted {text}");
    }
    let raw: proj::ProjConfig = serde_yaml::from_str("directories:\n  raw-data:\n    path: input\n").unwrap();
    let config = raw.validate_and_resolve(root, false).unwrap();
    assert_eq!(config.directories.len(), 1);
    assert_eq!(config.get_path(root, "RAW-DATA").unwrap(), root.join("input"));
    for label in ["raw", "raw-data-extra", "cache", "output", "docs"] { assert!(config.get_path(root, label).is_err()); }
}

#[test]
fn invalid_mappings_and_storage_labels_fail_before_managed_writes() {
    for content in [
        "directories:\n  output: {}\nbuild:\n  scripts: []\n",
        "directories:\n  cache:\n    path: scratch\nremotes:\n  local:\n    archive:\n      path: archive\n      content: [output-missing]\n      structure: cas\n      inspect: manifest\nbuild:\n  scripts: []\n  git: false\n",
    ] {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        fs::write(root.join("_proj.yml"), content).unwrap();
        fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
        fs::write(root.join(".gitignore"), "manual\n").unwrap();
        let result = cli(root, &["build", "--patch"]);
        assert!(!result.status.success());
        assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
        assert_eq!(fs::read_to_string(root.join(".gitignore")).unwrap(), "manual\n");
        assert_eq!(fs::read_to_string(root.join("_proj.yml")).unwrap(), content);
        for absent in ["_tmp", "scratch", "archive", ".Rbuildignore"] { assert!(!root.join(absent).exists()); }
    }
}

#[test]
fn production_uses_declared_cache_without_touching_unconfigured_legacy_staging() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::create_dir_all(root.join("_tmp/projr/1.2.3")).unwrap();
    fs::write(root.join("_tmp/projr/1.2.3/keep"), "unmanaged").unwrap();
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    fs::create_dir_all(root.join("scratch/projr/1.2.3")).unwrap();
    fs::write(root.join("scratch/projr/1.2.3/remove"), "old staging").unwrap();
    fs::write(root.join("analysis.py"), "from pathlib import Path\nassert not Path('scratch/projr/1.2.3/remove').exists()\nassert Path('_tmp/projr/1.2.3/keep').read_text() == 'unmanaged'\nPath('scratch/analysis-ran').write_text('custom cache')\n").unwrap();
    fs::write(root.join("_proj.yml"), "directories:\n  cache:\n    path: scratch\nconfig:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: [analysis.py]\n  git: false\n  restrictions:\n    not_behind: false\n").unwrap();
    let result = cli(root, &["build", "--patch"]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(fs::read_to_string(root.join("_tmp/projr/1.2.3/keep")).unwrap(), "unmanaged");
    assert_eq!(fs::read_to_string(root.join("scratch/analysis-ran")).unwrap(), "custom cache");
    assert!(!root.join("scratch/projr/1.2.3/remove").exists());
}


#[test]
fn storage_paths_are_root_relative_and_undeclared_content_is_rejected() {
    let project = tempfile::tempdir().unwrap();
    let root = camino::Utf8Path::from_path(project.path()).unwrap();
    let text = "directories:\n  output:\n    path: results\nremotes:\n  local:\n    archive:\n      path: archives\n      content: [output]\n      structure: cas\n      inspect: manifest\n";
    let raw: proj::ProjConfig = serde_yaml::from_str(text).unwrap();
    let resolved = raw.validate_and_resolve(root, false).unwrap();
    assert_eq!(resolved.remotes.local.unwrap()["archive"].path, root.join("archives"));
    let invalid: proj::ProjConfig = serde_yaml::from_str(&text.replace("[output]", "[output-unknown]")).unwrap();
    assert!(invalid.validate_and_resolve(root, false).is_err());
}

#[test]
fn project_renderer_missing_docs_fails_before_state_changes() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::write(root.join("_quarto.yml"), "project:\n  type: default\n").unwrap();
    fs::write(root.join("_proj.yml"), "directories:\n  cache:\n    path: scratch\nbuild:\n  git: false\n").unwrap();
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    let result = cli(root, &["build", "--patch"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("docs"));
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
    for absent in ["scratch", ".gitignore", ".Rbuildignore"] { assert!(!root.join(absent).exists()); }
}


#[test]
fn upload_only_selection_needs_no_unused_cache_or_renderer_mapping() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::create_dir(root.join("results")).unwrap();
    fs::write(root.join("results/result.csv"), "measured,42\n").unwrap();
    fs::write(root.join("_quarto.yml"), "project:\n  type: default\n").unwrap();
    fs::write(root.join("_proj.yml"), "directories:\n  output:\n    path: results\nremotes:\n  local:\n    archive:\n      path: archives\n      content: [output]\n      structure: cas\n      inspect: manifest\nconfig:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: []\n  git: false\n  dest: [archive]\n  restrictions:\n    not_behind: false\n").unwrap();
    let result = cli(root, &["build", "--patch"]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(root.join("archives").is_dir());
    assert_eq!(fs::read_to_string(root.join("results/result.csv")).unwrap(), "measured,42\n");
    for absent in ["_tmp", "cache", "docs"] { assert!(!root.join(absent).exists()); }
}

#[test]
fn explicit_python_preview_does_not_activate_an_unused_project_renderer() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::write(root.join("_quarto.yml"), "project:\n  type: default\n").unwrap();
    fs::write(root.join("_proj.yml"), "build:\n  scripts: [analysis.py]\n").unwrap();
    fs::write(root.join("analysis.py"), "from pathlib import Path\nPath('analysis-ran').write_text('python')\n").unwrap();
    let result = cli(root, &["build-dev"]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(fs::read_to_string(root.join("analysis-ran")).unwrap(), "python");
    assert_eq!(fs::read_to_string(root.join("_quarto.yml")).unwrap(), "project:\n  type: default\n");
}
