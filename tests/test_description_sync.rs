//! VERSION-authoritative metadata updates with exact failure preservation.
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
fn cli(root: &Path, args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_proj"));
    c.args(["--non-interactive", "--project"])
        .arg(root)
        .args(args)
        .current_dir(root);
    for k in [
        proj::PROJECT_ROOT_ENV,
        proj::RUN_CONTEXT_ENV,
        "PROJR_PROFILE",
        "QUARTO_PROFILE",
        "PROJR_CLEAR_OUTPUT",
    ] {
        c.env_remove(k);
    }
    c.output().unwrap()
}
fn good(o: Output) {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}
fn project() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    fs::write(
        t.path().join("_proj.yml"),
        "build: {git: false, scripts: []}\n",
    )
    .unwrap();
    fs::write(t.path().join("VERSION"), b"v1.2.3-7\n").unwrap();
    t
}
#[test]
fn default_sync_preserves_other_fields_and_crlf_without_creating_description() {
    let t = project();
    let r = t.path();
    good(cli(r, &["version", "set", "2.3.4-8"]));
    assert!(!r.join("DESCRIPTION").exists());
    let desc = "Package: example\r\nTitle: caf\u{e9}\r\nAuthors@R: person(\"A\", \"B\")\r\nDescription: line one\r\n  line two\r\nVersion: 9.9.9\r\nLicense: MIT\r\nConfig/roxygen2/version: 8.1.0\r\n";
    fs::write(r.join("DESCRIPTION"), desc).unwrap();
    let o = cli(r, &["version", "set", "2.3.4-9"]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("VERSION is authoritative"));
    good(o);
    assert_eq!(
        fs::read_to_string(r.join("DESCRIPTION")).unwrap(),
        desc.replace("9.9.9", "2.3.4.9")
    );
}
#[test]
fn opt_out_and_layer_validation_are_explicit() {
    let t = project();
    let r = t.path();
    let desc = "Version: malformed\nVersion: duplicate\n";
    fs::write(r.join("DESCRIPTION"), desc).unwrap();
    fs::write(
        r.join("_projr-local.yml"),
        "settings: {sync-description-version: false}\n",
    )
    .unwrap();
    good(cli(r, &["version", "set", "2.3.4"]));
    assert_eq!(fs::read_to_string(r.join("DESCRIPTION")).unwrap(), desc);
    for marker in ["null", "'false'", "1", "[]"] {
        fs::write(
            r.join("_projr-local.yml"),
            format!("settings: {{sync-description-version: {marker}}}\n"),
        )
        .unwrap();
        let before = fs::read(r.join("VERSION")).unwrap();
        assert!(!cli(r, &["version", "set", "3.0.0"]).status.success());
        assert_eq!(fs::read(r.join("VERSION")).unwrap(), before);
    }
}
#[test]
fn ambiguous_or_invalid_description_fails_before_writes() {
    for desc in [
        "  orphan\nVersion: 1.0.0\n",
        "Package: fixture\n\nTitle: another record\n",
        "Package: fixture\nVersion: 1.0.0\nVersion: 2.0.0\n",
        "Package: fixture\nVersion: 1.0.0\n  continued\n",
        "invalid DCF\n",
    ] {
        let t = project();
        let r = t.path();
        fs::write(r.join("DESCRIPTION"), desc).unwrap();
        let before = fs::read(r.join("VERSION")).unwrap();
        assert!(!cli(r, &["version", "set", "3.0.0"]).status.success());
        assert_eq!(fs::read(r.join("VERSION")).unwrap(), before);
        assert_eq!(fs::read_to_string(r.join("DESCRIPTION")).unwrap(), desc);
    }
}
#[test]
fn preview_preserves_mismatch_and_production_synchronizes_next_dev() {
    let t = project();
    let r = t.path();
    let desc = b"Package: fixture\nVersion: 9.9.9\nTitle: keep\n";
    fs::write(r.join("DESCRIPTION"), desc).unwrap();
    let before = fs::read(r.join("VERSION")).unwrap();
    good(cli(r, &["build-dev"]));
    assert_eq!(fs::read(r.join("VERSION")).unwrap(), before);
    assert_eq!(fs::read(r.join("DESCRIPTION")).unwrap(), desc);
    good(cli(r, &["build", "--patch"]));
    assert_eq!(
        fs::read_to_string(r.join("DESCRIPTION")).unwrap(),
        "Package: fixture\nVersion: 1.2.4.1\nTitle: keep\n"
    );
    assert!(
        fs::read_to_string(r.join("VERSION"))
            .unwrap()
            .contains("1.2.4.1")
    );
}
#[test]
fn post_hook_failure_restores_exact_counter_and_description() {
    let t = project();
    let r = t.path();
    fs::write(
        r.join("DESCRIPTION"),
        b"Package: fixture\r\nVersion: 1.2.3.7\r\n",
    )
    .unwrap();
    fs::write(
        r.join("_proj.yml"),
        "build:\n  git: false\n  scripts: []\n  hooks: {post: [fail.py]}\n",
    )
    .unwrap();
    fs::write(r.join("fail.py"), "raise SystemExit(17)\n").unwrap();
    let version = fs::read(r.join("VERSION")).unwrap();
    let desc = fs::read(r.join("DESCRIPTION")).unwrap();
    assert!(!cli(r, &["build", "--patch"]).status.success());
    assert_eq!(fs::read(r.join("VERSION")).unwrap(), version);
    assert_eq!(fs::read(r.join("DESCRIPTION")).unwrap(), desc);
}
#[cfg(unix)]
#[test]
fn symlink_is_rejected_and_hard_link_alias_keeps_bytes() {
    use std::os::unix::fs::symlink;
    let t = project();
    let r = t.path();
    let alias = r.join("outside-metadata");
    let desc = b"Package: fixture\nVersion: 1.2.3\n";
    fs::write(&alias, desc).unwrap();
    symlink(&alias, r.join("DESCRIPTION")).unwrap();
    assert!(!cli(r, &["version", "set", "3.0.0"]).status.success());
    fs::remove_file(r.join("DESCRIPTION")).unwrap();
    fs::hard_link(&alias, r.join("DESCRIPTION")).unwrap();
    good(cli(r, &["version", "set", "3.0.0"]));
    assert_eq!(fs::read(&alias).unwrap(), desc);
    assert!(
        fs::read_to_string(r.join("DESCRIPTION"))
            .unwrap()
            .contains("Version: 3.0.0\n")
    );
}

#[test]
fn missing_version_is_inserted_before_trailing_blank_lines() {
    let t = project();
    let r = t.path();
    fs::write(r.join("DESCRIPTION"), "Package: fixture\r\n\r\n\r\n").unwrap();
    good(cli(r, &["version", "set", "2.0.0"]));
    assert_eq!(
        fs::read_to_string(r.join("DESCRIPTION")).unwrap(),
        "Package: fixture\r\nVersion: 2.0.0\r\n\r\n\r\n"
    );
}

#[test]
fn overflowing_dev_counter_is_not_preserved_as_an_equal_release_version() {
    assert!(proj::ProjVersion::parse("1.2.3-4294967296").is_none());
    let t = project();
    let r = t.path();
    fs::write(
        r.join("DESCRIPTION"),
        "Package: fixture\nVersion: 1.2.3-4294967296\n",
    )
    .unwrap();
    good(cli(r, &["version", "set", "1.2.3"]));
    assert_eq!(
        fs::read_to_string(r.join("DESCRIPTION")).unwrap(),
        "Package: fixture\nVersion: 1.2.3\n"
    );
}
