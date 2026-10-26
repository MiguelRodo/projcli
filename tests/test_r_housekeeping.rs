use std::{fs, process::Command};

#[test]
fn r_actions_are_strict_independent_booleans_and_merge_per_action() {
    for text in ["r: {document: 1}", "r: {readme: 'yes'}",
        "r: {renv-snapshot: null}", "r: {unknown: true}"] {
        assert!(serde_yaml::from_str::<proj::yml::BuildConfig>(text).is_err(), "{text}");
    }
    let config: proj::yml::BuildConfig = serde_yaml::from_str("r: {document: true}").unwrap();
    assert!(config.r.document);
    assert!(!config.r.readme && !config.r.renv_snapshot);
    let temp = tempfile::tempdir().unwrap();
    let root = camino::Utf8Path::from_path(temp.path()).unwrap();
    fs::write(root.join("_proj.yml"), "build:\n  r: {document: true, readme: true}\n").unwrap();
    fs::write(root.join("_projr-test.yml"), "build:\n  r: {document: false, renv-snapshot: true}\n").unwrap();
    let combined = proj::yml::get_combined_yml(Some("test"), root).unwrap();
    let config: proj::yml::ProjConfig = serde_json::from_value(combined).unwrap();
    assert!(!config.build.r.document && config.build.r.readme && config.build.r.renv_snapshot);
    for file in ["_projr-test.yml", "_projr-local.yml"] {
        for invalid in ["r: null", "r: {document: null}", "r: {readme: null}",
            "r: {renv-snapshot: null}", "r: {document: 'true'}"] {
            fs::write(root.join(file), format!("build:\n  {invalid}\n")).unwrap();
            assert!(proj::yml::get_combined_yml(Some("test"), root).is_err(),
                "explicit invalid action disappeared in {file}: {invalid}");
        }
        fs::remove_file(root.join(file)).unwrap();
    }
}

#[test]
fn rmd_initialization_preserves_markdown_and_existing_rmd_and_quotes_title() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("README.md"), "keep plain readme\n").unwrap();
    let initialize = || Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["--non-interactive", "--project"]).arg(root)
        .args(["init", "readme", "--rmd", "--title", "Title: 'quoted'\nnext"])
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .output().unwrap();
    let output = initialize();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let bytes = fs::read(root.join("README.Rmd")).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    let header = text.strip_prefix("---\n").unwrap().split("---\n").next().unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(header).unwrap();
    assert_eq!(yaml["title"].as_str(), Some("Title: 'quoted'\nnext"));
    assert_eq!(yaml["output"].as_str(), Some("md_document"));
    assert!(initialize().status.success());
    assert_eq!(fs::read(root.join("README.Rmd")).unwrap(), bytes);
    assert_eq!(fs::read(root.join("README.md")).unwrap(), b"keep plain readme\n");
}

#[test]
fn renv_init_preserves_existing_project_state_without_running_r() {
    for name in ["renv.lock", ".Rprofile", "renv"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        if name == "renv" { fs::create_dir(root.join(name)).unwrap(); }
        else { fs::write(root.join(name), "existing").unwrap(); }
        fs::write(root.join("_proj.yml"), "build: {}\n").unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_proj"))
            .args(["--non-interactive", "--project"]).arg(root).args(["init", "renv"])
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
            .output().unwrap();
        assert!(!output.status.success());
        if name != "renv" { assert_eq!(fs::read(root.join(name)).unwrap(), b"existing"); }
        assert!(!root.join(".gitignore").exists());
    }
}

#[cfg(unix)]
mod native_fixture {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    fn fixture() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir(root.join("R")).unwrap();
        fs::create_dir(root.join("man")).unwrap();
        fs::write(root.join("man/old.Rd"), "old documentation").unwrap();
        fs::write(root.join("DESCRIPTION"), "Package: fixture\nVersion: 1.2.3\n").unwrap();
        fs::write(root.join("README.Rmd"), "---\noutput: md_document\n---\n").unwrap();
        fs::write(root.join("README.md"), "old readme").unwrap();
        fs::write(root.join("renv.lock"), "old lock").unwrap();
        fs::write(root.join("VERSION"), "v1.2.3.7\n").unwrap();
        fs::write(root.join("_proj.yml"), "config:\n  r:\n    rscript: 'tools/R script'\nbuild:\n  scripts: []\n  git: false\n  r: {document: true, readme: true, renv-snapshot: true}\n").unwrap();
        fs::create_dir(root.join("tools")).unwrap();
        // Native executable stub validates actual argv and completion handling.
        fs::write(root.join("tools/R script"), r#"#!/bin/sh
set -eu
if test "$1" = "--version"; then printf 'R fixture\n'; exit 0; fi
if test "$#" -eq 1; then
    case "$1" in
        */pre-hook.R)
            test "$(cat renv.lock)" = "pre"
            printf 'hook\n' >> tools/events
            exit 0
            ;;
    esac
fi
test "$1" = "--no-save"
test "$2" = "--no-restore"
test -f "$3"
stage="$4"
test "$5" = "$PWD"
printf '%s\n' "$stage" >> tools/events
if test "$stage" = "check" && test -f fail-check; then exit 17; fi
if test "$stage" = "pre"; then
    if test "$8" = "1"; then printf pre > renv.lock; fi
    if test -f fail-pre; then exit 17; fi
fi
if test "$stage" = "post"; then
    if test "$8" = "1"; then printf post > renv.lock; fi
    if test "$6" = "1"; then printf generated > man/new.Rd; fi
    if test "$7" = "1"; then printf generated > README.md; fi
    if test -f fail-post; then exit 17; fi
    if test -f early-exit; then exit 0; fi
fi
if test "$stage" = "init"; then
    mkdir renv
    printf generated > renv/activate.R
    printf generated > renv.lock
    printf generated > .Rprofile
    printf generated > .gitignore
    printf generated > .Rbuildignore
    if test -f fail-init; then exit 17; fi
fi
printf 'complete\n' > "$9"
"#).unwrap();
        fs::set_permissions(root.join("tools/R script"), fs::Permissions::from_mode(0o755)).unwrap();
        temp
    }
    fn build(root: &std::path::Path, preview: bool) -> std::process::Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_proj"));
        command.arg(if preview {"build-dev"} else {"build"});
        if !preview { command.arg("--patch"); }
        command.args(["--non-interactive", "--project"]).arg(root).current_dir(root)
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
            .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap()
    }
    #[test]
    fn configured_spaced_executable_batches_pre_post_and_preview_skips_r_actions() {
        let temp = fixture(); let root = temp.path();
        assert!(build(root, true).status.success());
        assert!(!root.join("tools/events").exists());
        let output = build(root, false);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(fs::read_to_string(root.join("tools/events")).unwrap(), "check\npre\npost\n");
        assert_eq!(fs::read(root.join("README.md")).unwrap(), b"generated");
        assert_eq!(fs::read(root.join("man/new.Rd")).unwrap(), b"generated");
        assert_eq!(fs::read(root.join("renv.lock")).unwrap(), b"post");
    }
    #[test]
    fn pre_snapshot_precedes_start_hook_like_original_projr() {
        let temp = fixture(); let root = temp.path();
        let mut config = fs::read_to_string(root.join("_proj.yml")).unwrap();
        config.push_str("  hooks:\n    pre: [pre-hook.R]\n");
        fs::write(root.join("_proj.yml"), config).unwrap();
        fs::write(root.join("pre-hook.R"), "# native fixture checks the prior snapshot\n").unwrap();
        let output = build(root, false);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(fs::read_to_string(root.join("tools/events")).unwrap(),
            "check\npre\nhook\npost\n");
    }

    #[test]
    fn post_failure_or_early_success_exit_preserves_version_docs_readme_and_lock() {
        for marker in ["fail-pre", "fail-post", "early-exit"] {
            let temp = fixture(); let root = temp.path();
            fs::write(root.join(marker), "").unwrap();
            let output = build(root, false);
            assert!(!output.status.success(), "{marker}");
            let expected = if marker == "fail-pre" { "check\npre\n" } else { "check\npre\npost\n" };
            assert_eq!(fs::read_to_string(root.join("tools/events")).unwrap(), expected,
                "negative witness must reach the intended action boundary");
            assert_eq!(fs::read(root.join("VERSION")).unwrap(), b"v1.2.3.7\n");
            assert_eq!(fs::read(root.join("README.md")).unwrap(), b"old readme");
            assert_eq!(fs::read(root.join("renv.lock")).unwrap(), b"old lock");
            assert_eq!(fs::read(root.join("man/old.Rd")).unwrap(), b"old documentation");
            assert!(!root.join("man/new.Rd").exists());
        }
    }
    #[test]
    fn preflight_failure_does_not_enter_build_and_individual_actions_stay_independent() {
        let temp = fixture(); let root = temp.path();
        fs::write(root.join("fail-check"), "").unwrap();
        assert!(!build(root, false).status.success());
        assert_eq!(fs::read_to_string(root.join("tools/events")).unwrap(), "check\n");
        assert_eq!(fs::read(root.join("VERSION")).unwrap(), b"v1.2.3.7\n");
        assert_eq!(fs::read(root.join("renv.lock")).unwrap(), b"old lock");
        assert_eq!(fs::read(root.join("README.md")).unwrap(), b"old readme");
        for action in ["document", "readme", "renv-snapshot", "none"] {
            let temp = fixture(); let root = temp.path();
            let setting = if action == "none" { "{}".to_owned() } else { format!("{{{action}: true}}") };
            fs::write(root.join("_proj.yml"), format!(
                "config:\n  r:\n    rscript: 'tools/R script'\nbuild:\n  scripts: []\n  git: false\n  r: {setting}\n")).unwrap();
            let output = build(root, false);
            assert!(output.status.success(), "{action}: {}", String::from_utf8_lossy(&output.stderr));
            assert_eq!(root.join("man/new.Rd").exists(), action == "document");
            assert_eq!(fs::read(root.join("README.md")).unwrap(),
                if action == "readme" { b"generated".as_slice() } else { b"old readme".as_slice() });
            assert_eq!(fs::read(root.join("renv.lock")).unwrap(),
                if action == "renv-snapshot" { b"post".as_slice() } else { b"old lock".as_slice() });
            if action == "none" { assert!(!root.join("tools/events").exists()); }
        }
    }

    #[test]
    fn explicit_renv_init_creates_state_on_success_and_restores_ignore_bytes_on_failure() {
        for fail in [false, true] {
            let temp = fixture(); let root = temp.path();
            fs::remove_file(root.join("renv.lock")).unwrap();
            fs::write(root.join(".gitignore"), "old git ignores").unwrap();
            fs::write(root.join(".Rbuildignore"), "old R ignores").unwrap();
            if fail { fs::write(root.join("fail-init"), "").unwrap(); }
            let output = Command::new(env!("CARGO_BIN_EXE_proj"))
                .args(["--non-interactive", "--project"]).arg(root).args(["init", "renv"])
                .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
                .output().unwrap();
            assert_eq!(output.status.success(), !fail, "{}", String::from_utf8_lossy(&output.stderr));
            assert_eq!(fs::read_to_string(root.join("tools/events")).unwrap(), "init\n");
            for name in ["renv", "renv.lock", ".Rprofile"] { assert_eq!(root.join(name).exists(), !fail, "{name}"); }
            if fail {
                assert_eq!(fs::read(root.join(".gitignore")).unwrap(), b"old git ignores");
                assert_eq!(fs::read(root.join(".Rbuildignore")).unwrap(), b"old R ignores");
            }
        }
    }

    #[test]
    fn init_rejects_hard_linked_ignore_files_before_launch() {
        for name in [".gitignore", ".Rbuildignore"] {
            let temp = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            fs::write(outside.path().join("external"), "external").unwrap();
            fs::hard_link(outside.path().join("external"), temp.path().join(name)).unwrap();
            // An absent executable makes a launched driver fail differently.
            let output = Command::new(env!("CARGO_BIN_EXE_proj"))
                .args(["--non-interactive", "--project"]).arg(temp.path()).args(["init", "renv"])
                .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
                .output().unwrap();
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("hard-linked"));
            assert_eq!(fs::read(outside.path().join("external")).unwrap(), b"external");
            for created in ["renv", "renv.lock", ".Rprofile"] { assert!(!temp.path().join(created).exists()); }
        }
    }

    #[test]
    fn linked_outputs_fail_before_r_or_hooks_and_preserve_external_bytes() {
        for hard in [false, true] {
            let temp = fixture(); let root = temp.path();
            let outside = tempfile::tempdir().unwrap();
            fs::write(outside.path().join("external"), "external").unwrap();
            fs::remove_file(root.join("README.md")).unwrap();
            if hard {fs::hard_link(outside.path().join("external"), root.join("README.md")).unwrap();}
            else {symlink(outside.path().join("external"), root.join("README.md")).unwrap();}
            assert!(!build(root, false).status.success());
            assert!(!root.join("tools/events").exists());
            assert_eq!(fs::read(outside.path().join("external")).unwrap(), b"external");
        }
    }
}
