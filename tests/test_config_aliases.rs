use std::{fs, process::Command};
use proj::yml::{add_empty_parameters_block, get_combined_yml};
use serde_json::json;

#[test]
fn either_base_filename_supports_queries_and_local_profile_creation() {
    for name in ["_proj.yml", "_projr.yml"] {
        let temp = tempfile::tempdir().unwrap();
        let root = camino::Utf8Path::from_path(temp.path()).unwrap();
        fs::write(root.join(name), "directories:\n  output:\n    path: _results\n").unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_proj"))
            .args(["--project", root.as_str(), "path", "get", "--label", "output"])
            .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(std::path::Path::new(String::from_utf8(output.stdout).unwrap().trim()), root.canonicalize_utf8().unwrap().join("_results").as_std_path());
        proj::profile::create_local_profile(root).unwrap();
        assert!(fs::read_to_string(root.join("_projr-local.yml")).unwrap().contains("output"));
        assert!(add_empty_parameters_block(root).unwrap());
        assert!(fs::read_to_string(root.join(name)).unwrap().contains("parameters:"));
        let other = if name == "_proj.yml" { "_projr.yml" } else { "_proj.yml" };
        assert!(!root.join(other).exists());
    }
}

#[test]
fn ambiguous_bases_fail_without_changing_either_file_or_creating_local_profile() {
    let temp = tempfile::tempdir().unwrap();
    let root = camino::Utf8Path::from_path(temp.path()).unwrap();
    for name in ["_proj.yml", "_projr.yml"] { fs::write(root.join(name), "build: {}\n").unwrap(); }
    assert!(get_combined_yml(Some(""), root).unwrap_err().to_string().contains("both _proj.yml and _projr.yml"));
    assert!(add_empty_parameters_block(root).is_err());
    assert!(proj::profile::create_local_profile(root).is_err());
    for name in ["_proj.yml", "_projr.yml"] { assert_eq!(fs::read_to_string(root.join(name)).unwrap(), "build: {}\n"); }
    assert!(!root.join("_projr-local.yml").exists());
}

#[test]
fn earlier_profiles_win_and_local_overrides_both_base_names() {
    for name in ["_proj.yml", "_projr.yml"] {
        let temp = tempfile::tempdir().unwrap();
        let root = camino::Utf8Path::from_path(temp.path()).unwrap();
        fs::write(root.join(name), "build:\n  scripts: [base.py]\n  dest: [base]\n").unwrap();
        fs::write(root.join("_projr-first.yml"), "build:\n  scripts: [first.py]\n").unwrap();
        fs::write(root.join("_projr-second.yml"), "build:\n  scripts: [second.py]\n  dest: [second]\n").unwrap();
        let merged = get_combined_yml(Some("first;second"), root).unwrap();
        assert_eq!(merged["build"]["scripts"], json!(["first.py"]));
        assert_eq!(merged["build"]["dest"], json!(["second"]));
        fs::write(root.join("_projr-local.yml"), "build:\n  scripts: []\n").unwrap();
        assert_eq!(get_combined_yml(Some("first,second"), root).unwrap()["build"]["scripts"], json!([]));
        assert!(get_combined_yml(Some("../outside"), root).is_err());
    }
}


#[test]
fn duplicate_keys_are_rejected_in_every_active_configuration_layer() {
    for layer in ["_proj.yml", "_projr.yml", "_projr-test.yml", "_projr-local.yml"] {
        for invalid in ["build: {}\nbuild: {scripts: []}\n", "build:\n  scripts: []\n  scripts: [unexpected.py]\n"] {
            let temp = tempfile::tempdir().unwrap();
            let root = camino::Utf8Path::from_path(temp.path()).unwrap();
            if layer != "_proj.yml" && layer != "_projr.yml" {
                fs::write(root.join("_proj.yml"), "build: {}\n").unwrap();
            }
            fs::write(root.join(layer), invalid).unwrap();
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
