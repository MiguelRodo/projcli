use std::{
    fs,
    process::{Command, Output},
};

fn build(root: &std::path::Path, profile: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_proj"));
    cmd.args(["build-dev", "--non-interactive", "--project"])
        .arg(root)
        .current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV)
        .env_remove("PROJR_PROFILE")
        .env_remove("QUARTO_PROFILE");
    if let Some(profile) = profile {
        cmd.args(["--profile", profile]);
    }
    cmd.output().unwrap()
}

#[test]
fn a_first_build_writes_the_projr_starter_configuration() {
    const STARTER: &str = "directories:\n  raw-data:\n    path: _raw_data\n  cache:\n    path: _tmp\n  output:\n    path: _output\n  docs:\n    path: docs\nbuild:\n  git: false\n";
    for mode in ["none", "local-only", "active-profile", "base", "config-dir"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let name = match mode {
            "active-profile" => Some("_projr-test.yml"),
            "local-only" => Some("_projr-local.yml"),
            "base" => Some("_projr.yml"),
            "config-dir" => Some(".projr/config/default.yml"),
            _ => None,
        };
        if let Some(name) = name {
            fs::create_dir_all(root.join(name).parent().unwrap()).unwrap();
            fs::write(root.join(name), "build:\n  scripts: []\n").unwrap();
        }
        let output = build(
            root,
            if mode == "active-profile" {
                Some("test")
            } else {
                None
            },
        );
        assert!(
            output.status.success(),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let starter = matches!(mode, "none" | "local-only" | "active-profile");
        assert_eq!(
            fs::read_to_string(root.join("_projr.yml")).ok().as_deref() == Some(STARTER),
            starter,
            "{mode}"
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stderr).contains("Created a starter _projr.yml"),
            starter,
            "{mode}"
        );
        if mode == "config-dir" {
            assert!(!root.join("_projr.yml").exists());
        }
    }
}

#[test]
fn unsupported_configuration_fails_before_build_writes() {
    for contents in [
        "unsupported: true\n",
        "build:\n  unsupported: true\n",
        "dev:\n  unsupported: true\n",
        "config:\n  unsupported: true\n",
        "directories:\n  cache:\n    path: _tmp\n    unsupported: true\n",
        "build:\n  hooks:\n    unsupported: [hook.py]\n",
        "config:\n  git:\n    unsupported: true\n",
        "remotes:\n  unsupported: {}\n",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(root.join("_projr.yml"), contents).unwrap();
        fs::write(root.join("_environment.local"), "FIXTURE_VALUE=local\n").unwrap();
        let output = build(root, None);
        assert!(!output.status.success(), "accepted {contents}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("unknown field"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(root.join("_projr.yml")).unwrap(),
            contents
        );
        for name in ["VERSION", ".gitignore", ".Rbuildignore", "_tmp"] {
            assert!(!root.join(name).exists(), "unexpected {name}");
        }
    }
}

#[test]
fn arbitrary_parameters_and_metadata_remain_supported() {
    let temp = tempfile::tempdir().unwrap();
    let root = camino::Utf8Path::from_path(temp.path()).unwrap();
    fs::write(root.join("_projr.yml"), "parameters:\n  study:\n    arbitrary: 42\nmetadata:\n  arbitrary: true\nbuild:\n  scripts: []\n").unwrap();
    assert!(build(root.as_std_path(), None).status.success());
    let validated = proj::yml_read_from(root, true).unwrap();
    assert_eq!(
        proj::yml::get_parameter(&validated, &["study", "arbitrary"]),
        Some(serde_yaml::Value::Number(42.into()))
    );
}

#[test]
fn parameter_aliases_are_canonicalized_before_merging_every_layer() {
    for base_alias in ["parameters", "parameter", "params", "param"] {
        for profile_alias in ["parameters", "parameter", "params", "param"] {
            for local_alias in ["parameters", "parameter", "params", "param"] {
                let temp = tempfile::tempdir().unwrap();
                let root = camino::Utf8Path::from_path(temp.path()).unwrap();
                fs::write(root.join("_projr.yml"), format!("{base_alias}:\n  study:\n    x: 1\n    base_only: true\nbuild:\n  scripts: []\n")).unwrap();
                fs::write(
                    root.join("_projr-first.yml"),
                    format!("{profile_alias}:\n  study:\n    x: 2\n    profile_only: true\n"),
                )
                .unwrap();
                fs::write(
                    root.join("_projr-second.yml"),
                    "param:\n  study:\n    x: 3\n    second_only: true\n",
                )
                .unwrap();
                let merged = proj::yml::get_combined_yml(Some("first,second"), root).unwrap();
                assert_eq!(merged["parameters"]["study"]["x"], serde_json::json!(2));
                fs::write(
                    root.join("_projr-local.yml"),
                    format!("{local_alias}:\n  study:\n    x: 4\n"),
                )
                .unwrap();
                let merged = proj::yml::get_combined_yml(Some("first,second"), root).unwrap();
                assert_eq!(merged["parameters"]["study"]["x"], serde_json::json!(4));
                for key in ["base_only", "profile_only", "second_only"] {
                    assert_eq!(merged["parameters"]["study"][key], serde_json::json!(true));
                }
                let typed: proj::yml::ProjConfig = serde_json::from_value(merged).unwrap();
                assert_eq!(
                    typed.parameters["study"]["x"],
                    serde_yaml::Value::Number(4.into())
                );
            }
        }
    }
}

#[test]
fn multiple_parameter_aliases_in_one_file_still_fail_in_every_layer() {
    for layer in [
        "_projr.yml",
        ".projr/config/default.yml",
        "_projr-test.yml",
        "_projr-local.yml",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = camino::Utf8Path::from_path(temp.path()).unwrap();
        fs::create_dir_all(root.join(layer).parent().unwrap()).unwrap();
        fs::write(root.join(layer), "parameters: {}\nparams: {}\n").unwrap();
        let error = proj::yml::get_combined_yml(Some("test"), root).unwrap_err();
        assert!(error.to_string().contains("multiple 'parameters' aliases"));
    }
}
