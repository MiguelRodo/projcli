use std::{fs, process::Command};
use proj::yml::{add_empty_parameters_block, get_combined_yml};
use serde_json::json;

/// projr's base configuration locations (`R/yml.R:308-338`).
const BASES: [&str; 4] = [
    "_projr.yml",
    ".projr/config/default.yml",
    ".projr/config/_projr.yml",
    "_projr/config/default.yml",
];

fn put(root: &camino::Utf8Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

#[test]
fn every_base_location_supports_queries_and_local_profile_creation() {
    for name in BASES {
        let temp = tempfile::tempdir().unwrap();
        let root = camino::Utf8Path::from_path(temp.path()).unwrap();
        put(root, name, "directories:\n  output:\n    path: _results\n");
        let output = Command::new(env!("CARGO_BIN_EXE_proj"))
            .args(["--project", root.as_str(), "path", "get", "--label", "output","--target","final","--format","absolute"])
            .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
        assert!(output.status.success(), "{name}: {}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(std::path::Path::new(String::from_utf8(output.stdout).unwrap().trim()), root.canonicalize_utf8().unwrap().join("_results").as_std_path());
        proj::profile::create_local_profile(root).unwrap();
        assert!(fs::read_to_string(root.join("_projr-local.yml")).unwrap().contains("output"));
        assert!(add_empty_parameters_block(root).unwrap());
        assert!(fs::read_to_string(root.join(name)).unwrap().contains("parameters:"), "{name}");
        if name != "_projr.yml" {
            assert!(!root.join("_projr.yml").exists(), "{name}");
        }
    }
}

#[test]
fn both_spellings_in_one_config_directory_fail_without_changes() {
    for (directory, long, short) in [
        (".projr/config", "_projr.yml", "default.yml"),
        ("_projr/config", "_projr.yml", "default.yml"),
        (".projr/config", "_projr-local.yml", "local.yml"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = camino::Utf8Path::from_path(temp.path()).unwrap();
        for name in [long, short] { put(root, &format!("{directory}/{name}"), "build: {}\n"); }
        let error = get_combined_yml(Some(""), root).unwrap_err().to_string();
        assert_eq!(error, format!("Both {long} and {short} exist in {directory}/. Keep one."));
        if long == "_projr.yml" {
            assert!(add_empty_parameters_block(root).is_err());
            assert!(proj::profile::create_local_profile(root).is_err());
        }
        for name in [long, short] {
            assert_eq!(fs::read_to_string(root.join(directory).join(name)).unwrap(), "build: {}\n");
        }
        assert!(!root.join("_projr-local.yml").exists());
    }
}

#[test]
fn root_files_take_precedence_over_config_directories() {
    let temp = tempfile::tempdir().unwrap();
    let root = camino::Utf8Path::from_path(temp.path()).unwrap();
    put(root, "_projr.yml", "build:\n  scripts: [root.py]\n");
    put(root, ".projr/config/default.yml", "build:\n  scripts: [nested.py]\n");
    put(root, "_projr/config/default.yml", "build:\n  scripts: [legacy.py]\n");
    put(root, "_projr-local.yml", "dev:\n  scripts: [root-local.py]\n");
    put(root, ".projr/config/local.yml", "dev:\n  scripts: [nested-local.py]\n");
    let merged = get_combined_yml(Some(""), root).unwrap();
    assert_eq!(merged["build"]["scripts"], json!(["root.py"]));
    assert_eq!(merged["dev"]["scripts"], json!(["root-local.py"]));
    fs::remove_file(root.join("_projr.yml")).unwrap();
    fs::remove_file(root.join("_projr-local.yml")).unwrap();
    let merged = get_combined_yml(Some(""), root).unwrap();
    assert_eq!(merged["build"]["scripts"], json!(["nested.py"]));
    assert_eq!(merged["dev"]["scripts"], json!(["nested-local.py"]));
    fs::remove_file(root.join(".projr/config/default.yml")).unwrap();
    assert_eq!(get_combined_yml(Some(""), root).unwrap()["build"]["scripts"], json!(["legacy.py"]));
}

#[test]
fn earlier_profiles_win_and_local_overrides_with_short_names() {
    for (first, second, local) in [
        ("_projr-first.yml", "_projr-second.yml", "_projr-local.yml"),
        (".projr/config/first.yml", ".projr/config/_projr-second.yml", ".projr/config/local.yml"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = camino::Utf8Path::from_path(temp.path()).unwrap();
        put(root, "_projr.yml", "build:\n  scripts: [base.py]\n  dest: [base]\n");
        put(root, first, "build:\n  scripts: [first.py]\n");
        put(root, second, "build:\n  scripts: [second.py]\n  dest: [second]\n");
        let merged = get_combined_yml(Some("first;second"), root).unwrap();
        assert_eq!(merged["build"]["scripts"], json!(["first.py"]));
        assert_eq!(merged["build"]["dest"], json!(["second"]));
        put(root, local, "build:\n  scripts: []\n");
        assert_eq!(get_combined_yml(Some("first,second"), root).unwrap()["build"]["scripts"], json!([]));
        assert!(get_combined_yml(Some("../outside"), root).is_err());
    }
}


#[test]
fn duplicate_keys_are_rejected_in_every_active_configuration_layer() {
    for layer in ["_projr.yml", ".projr/config/default.yml", "_projr-test.yml", ".projr/config/test.yml", "_projr-local.yml", ".projr/config/local.yml"] {
        for invalid in ["build: {}\nbuild: {scripts: []}\n", "build:\n  scripts: []\n  scripts: [unexpected.py]\n"] {
            let temp = tempfile::tempdir().unwrap();
            let root = camino::Utf8Path::from_path(temp.path()).unwrap();
            if !layer.ends_with("_projr.yml") && !layer.ends_with("default.yml") {
                put(root, "_projr.yml", "build: {}\n");
            }
            put(root, layer, invalid);
            let error = get_combined_yml(Some("test"), root).unwrap_err();
            assert!(format!("{error:#}").contains("duplicate"), "{error:#}");
            let output = Command::new(env!("CARGO_BIN_EXE_proj"))
                .args(["build-dev", "--non-interactive", "--profile", "test", "--project", root.as_str()])
                .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
            assert!(!output.status.success(), "duplicate keys accepted in {layer}");
            assert_eq!(fs::read_to_string(root.join(layer)).unwrap(), invalid);
            for name in ["VERSION", ".gitignore", ".Rbuildignore", "_tmp"] {
                assert!(!root.join(name).exists(), "unexpected {name} after invalid {layer}");
            }
        }
    }
}
