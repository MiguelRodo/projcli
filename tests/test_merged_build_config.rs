use std::{fs, process::Command};

#[test]
fn build_uses_the_same_merged_configuration_for_scripts_and_hooks() {
    for base_name in ["_proj.yml", "_projr.yml"] {
        for local_override in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            fs::write(root.join(base_name), "directories:\n  cache:\n    path: _tmp\nbuild:\n  scripts: [absent.py]\n  hooks:\n    pre: [absent-hook.py]\n").unwrap();
            fs::write(root.join("_projr-first.yml"), "build:\n  scripts: [first.py]\n  hooks:\n    pre: [first-hook.py]\n").unwrap();
            fs::write(root.join("_projr-second.yml"), "build:\n  scripts: [absent-second.py]\n").unwrap();
            if local_override {
                fs::write(root.join("_projr-local.yml"), "dev:\n  scripts: []\n  hooks:\n    pre: [local-hook.py]\n").unwrap();
            }
            for (file, marker) in [("first.py", "analysis"), ("first-hook.py", "first-hook"), ("local-hook.py", "local-hook")] {
                fs::write(root.join(file), format!("from pathlib import Path\nPath('_tmp').mkdir(exist_ok=True)\nPath('_tmp/{marker}').write_text('ran')\n")).unwrap();
            }
            let output = Command::new(env!("CARGO_BIN_EXE_proj"))
                .args(["build-dev", "--non-interactive", "--profile", "first,second", "--project"])
                .arg(root).current_dir(root).env_remove(proj::PROJECT_ROOT_ENV)
                .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
            assert!(output.status.success(), "{} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            assert_eq!(root.join("_tmp/analysis").exists(), !local_override);
            assert_eq!(root.join("_tmp/first-hook").exists(), !local_override);
            assert_eq!(root.join("_tmp/local-hook").exists(), local_override);
            assert!(!root.join("VERSION").exists());
        }
    }
}

#[test]
fn invalid_merged_config_does_not_write_environment_ignores_or_version() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("_proj.yml"), "build: {}\n").unwrap();
    fs::write(root.join("_projr-broken.yml"), "build:\n  scripts: {invalid: shape}\n").unwrap();
    fs::write(root.join("_environment.local"), "FIXTURE_VALUE=local\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build", "--patch", "--non-interactive", "--profile", "broken", "--project"])
        .arg(root).current_dir(root).env_remove(proj::PROJECT_ROOT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
    assert!(!output.status.success());
    for name in [".gitignore", ".Rbuildignore", "VERSION", "_tmp"] { assert!(!root.join(name).exists(), "unexpected {name}"); }
    assert_eq!(fs::read_to_string(root.join("_environment.local")).unwrap(), "FIXTURE_VALUE=local\n");
}


#[test]
fn missing_analysis_target_does_not_write_environment_ignores_or_version() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("_proj.yml"), "build:\n  scripts: [missing.py]\n  git: false\n").unwrap();
    fs::write(root.join("_environment.local"), "FIXTURE_VALUE=local\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build", "--patch", "--non-interactive", "--project"])
        .arg(root).current_dir(root).env_remove(proj::PROJECT_ROOT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
    assert!(!output.status.success());
    for name in [".gitignore", ".Rbuildignore", "VERSION", "_tmp"] { assert!(!root.join(name).exists(), "unexpected {name}"); }
}
