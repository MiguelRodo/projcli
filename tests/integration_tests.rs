use proj::ignore::root;

#[test]
fn test_root_detection() { assert!(root().is_some()); }

#[test]
fn missing_version_uses_description_or_default_for_a_local_only_build() {
    for (description, expected) in [(Some("Package: fixture\nVersion: 1.2.3.4\n"), "1.2.4.1"), (None, "0.0.2.1")] {
        let project = tempfile::tempdir().unwrap();
        let root = camino::Utf8Path::from_path(project.path()).unwrap();
        if let Some(description) = description { std::fs::write(root.join("DESCRIPTION"), description).unwrap(); }
        std::fs::write(root.join("_proj.yml"), "directories:\n  cache:\n    path: _tmp\nconfig:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: []\n  git: false\n  restrictions:\n    not_behind: false\n").unwrap();
        proj::build_project(root, proj::BuildMode::ProdPatch, None, None).unwrap();
        assert_eq!(proj::version_get_from(root).unwrap().to_string(false), expected);
        assert!(!root.join(".git").exists());
    }
}
