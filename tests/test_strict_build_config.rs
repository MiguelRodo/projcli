use std::{fs, process::{Command, Output}};

fn build(root: &std::path::Path, profile: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_proj"));
    cmd.args(["build-dev", "--non-interactive", "--project"]).arg(root).current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE");
    if let Some(profile) = profile { cmd.args(["--profile", profile]); }
    cmd.output().unwrap()
}

#[test]
fn only_active_root_configuration_allows_a_build() {
    for mode in ["none", "inactive-profile", "local-only", "active-profile", "base", "legacy"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let name = match mode {
            "inactive-profile" | "active-profile" => Some("_projr-test.yml"),
            "local-only" => Some("_projr-local.yml"), "base" => Some("_proj.yml"),
            "legacy" => Some("_projr.yml"), _ => None,
        };
        if let Some(name) = name { fs::write(root.join(name), "build:\n  scripts: []\n").unwrap(); }
        fs::write(root.join("unexpected.py"), "from pathlib import Path\nPath('analysis-ran').touch()\n").unwrap();
        let output = build(root, if mode == "active-profile" { Some("test") } else { None });
        assert_eq!(output.status.success(), matches!(mode, "active-profile" | "base" | "legacy"), "{mode}: {}", String::from_utf8_lossy(&output.stderr));
        assert!(!root.join("analysis-ran").exists());
        assert!(!root.join("VERSION").exists());
        assert!(!root.join(".gitignore").exists());
    }
}

#[test]
fn unsupported_configuration_fails_before_build_writes() {
    for contents in [
        "unsupported: true\n", "build:\n  unsupported: true\n",
        "dev:\n  unsupported: true\n", "config:\n  unsupported: true\n",
        "directories:\n  cache:\n    path: _tmp\n    unsupported: true\n",
        "build:\n  hooks:\n    unsupported: [hook.py]\n",
        "config:\n  git:\n    unsupported: true\n",
        "remotes:\n  unsupported: {}\n",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(root.join("_proj.yml"), contents).unwrap();
        fs::write(root.join("_environment.local"), "FIXTURE_VALUE=local\n").unwrap();
        let output = build(root, None);
        assert!(!output.status.success(), "accepted {contents}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("unknown field"), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(fs::read_to_string(root.join("_proj.yml")).unwrap(), contents);
        for name in ["VERSION", ".gitignore", ".Rbuildignore", "_tmp"] { assert!(!root.join(name).exists(), "unexpected {name}"); }
    }
}

#[test]
fn arbitrary_parameters_and_metadata_remain_supported() {
    let temp = tempfile::tempdir().unwrap();
    let root = camino::Utf8Path::from_path(temp.path()).unwrap();
    fs::write(root.join("_proj.yml"), "parameters:\n  study:\n    arbitrary: 42\nmetadata:\n  arbitrary: true\nbuild:\n  scripts: []\n").unwrap();
    assert!(build(root.as_std_path(), None).status.success());
    let validated = proj::yml_read_from(root, true).unwrap();
    assert_eq!(proj::yml::get_parameter(&validated, &["study", "arbitrary"]), Some(serde_yaml::Value::Number(42.into())));
}
