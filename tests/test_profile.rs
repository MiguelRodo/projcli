use proj::profile::create_local_profile;
use proj::yml::{deep_merge, yml_get_filter_top_level, get_combined_yml};
use serde_json::json;
use std::fs;
use tempfile::tempdir;

// #[test]
// fn test_get_active_profiles() {
//     unsafe {
//         std::env::set_var("PROJR_PROFILE", "stage, theme ; custom , default , local");
//     }
//     let profiles = get_active_profiles();
//     assert_eq!(profiles, vec!["stage", "theme", "custom"]);
//     unsafe {
//         std::env::remove_var("PROJR_PROFILE");
//     }
// }

#[test]
fn test_deep_merge() {
    let base = json!({
        "build": {
            "dest": ["a"],
            "scripts": ["run.sh"]
        },
        "dev": {
            "old_dev_remove": true
        }
    });

    let profile = json!({
        "build": {
            "dest": ["b"],
            "scripts": null
        },
        "dev": {
            "old_dev_remove": false
        }
    });

    let merged = deep_merge(base, profile);

    assert_eq!(merged, json!({
        "build": {
            "dest": ["b"],
            "scripts": ["run.sh"] // null in profile backed off to base
        },
        "dev": {
            "old_dev_remove": false
        }
    }));
}

#[test]
fn test_filter_top_level() {
    let raw = json!({
        "build": {},
        "dev": {},
        "directories": {},
        "metadata": {},
        "remotes": {},
        "config": {},
        "unknown": "should be dropped",
        "another": 123
    });

    let filtered = yml_get_filter_top_level(raw);

    assert!(filtered.get("build").is_some());
    assert!(filtered.get("dev").is_some());
    assert!(filtered.get("directories").is_some());
    assert!(filtered.get("metadata").is_some());
    assert!(filtered.get("remotes").is_some());
    assert!(filtered.get("config").is_some());
    assert!(filtered.get("unknown").is_none());
    assert!(filtered.get("another").is_none());
}

#[test]
fn test_cascading_merge() {
    let root = tempdir().unwrap();
    let base_dir = root.path();

    // 1. Base _proj.yml
    fs::write(base_dir.join("_proj.yml"), "
build:
  dest: [base_remote]
  scripts: [base.sh]
").unwrap();

    // 2. Profile 1 (stage)
    fs::write(base_dir.join("_projr-stage.yml"), "
build:
  dest: [stage_remote]
").unwrap();

    // 3. Profile 2 (theme)
    fs::write(base_dir.join("_projr-theme.yml"), "
build:
  scripts: [theme.sh]
").unwrap();

    // 4. Local override
    fs::write(base_dir.join("_projr-local.yml"), "
build:
  scripts: [local.sh]
").unwrap();

    let base_dir_utf8 = camino::Utf8PathBuf::try_from(base_dir.to_path_buf()).unwrap();

    // Passing explicit profile to avoid env var race conditions during parallel tests
    let combined = get_combined_yml(Some("stage, theme"), &base_dir_utf8).unwrap();

    // Precedence: Local > Profile > Base
    // dest: stage_remote (from stage)
    // scripts: local.sh (from local)

    assert_eq!(combined["build"]["dest"], json!(["stage_remote"]));
    assert_eq!(combined["build"]["scripts"], json!(["local.sh"]));
}

#[test]
fn test_create_local_profile() {
    let root = tempdir().unwrap();
    let base_dir = root.path();

    fs::write(base_dir.join("_proj.yml"), "
build:
  dest: [base_remote]
").unwrap();

    let base_dir_utf8 = camino::Utf8PathBuf::try_from(base_dir.to_path_buf()).unwrap();
    create_local_profile(&base_dir_utf8).unwrap();

    let local_content = fs::read_to_string(base_dir_utf8.join("_projr-local.yml")).unwrap();
    let local_val: serde_json::Value = serde_yaml::from_str(&local_content).unwrap();

    // It should have the same structure but null values
    assert_eq!(local_val["build"]["dest"], serde_json::Value::Null);
}
