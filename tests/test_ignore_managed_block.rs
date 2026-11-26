//! projr-compatible managed ignore blocks and starter defaults (projr 0.8.0).
use std::{fs, path::Path, process::Command};

const START: &str =
    "# Start of projr section: do not edit by hand (update with projr_ignore_auto())";
const END: &str = "# End of projr section";
const GIT_STARTER: &str = ".vdoc.*\n.DS_Store\nThumbs.db\n.Rproj.user\n.Rhistory\n.RData\n\
.Ruserdata\n_projr-local.yml\n_environment.local\n";

fn init(root: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["--non-interactive", "--project"])
        .arg(root)
        .args(["init", "directories"])
        .current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV)
        .env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE")
        .env_remove("QUARTO_PROFILE")
        .output()
        .unwrap()
}

fn project(gitignore: Option<&str>) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let config = "directories:\n  cache: {path: _tmp}\nbuild:\n  git: false\n";
    fs::write(temp.path().join("_proj.yml"), config).unwrap();
    if let Some(text) = gitignore {
        fs::write(temp.path().join(".gitignore"), text).unwrap();
    }
    temp
}

fn refresh(root: &Path) -> String {
    let output = init(root);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    fs::read_to_string(root.join(".gitignore")).unwrap()
}

#[test]
fn fresh_gitignore_gets_starter_defaults_then_projr_block_and_is_stable() {
    let temp = project(None);
    let git = refresh(temp.path());
    assert_eq!(git, format!("{GIT_STARTER}{START}\n_tmp/**\n\n{END}\n"));
    assert_eq!(refresh(temp.path()), git);
}

#[test]
fn existing_rules_stay_after_the_block_and_defaults_are_added_once() {
    let temp = project(Some("# my rules\n.DS_Store\n!.vdoc.keep\n"));
    let git = refresh(temp.path());
    let lines: Vec<_> = git.lines().collect();
    assert_eq!(lines.iter().filter(|line| **line == ".DS_Store").count(), 1);
    assert!(git.ends_with(&format!("{END}\n# my rules\n.DS_Store\n!.vdoc.keep\n")));
    assert!(git.starts_with(".vdoc.*\nThumbs.db\n"));
    // Seeding happens once: a removed default is not restored, and manual
    // rules on both sides of the block survive later refreshes.
    let edited = git.replace(".Rhistory\n", "my-scratch/\n") + "late-rule\n";
    fs::write(temp.path().join(".gitignore"), &edited).unwrap();
    assert_eq!(refresh(temp.path()), edited);
}

#[test]
fn projr_written_gitignore_is_refreshed_in_place_as_a_single_block() {
    let before = format!("{GIT_STARTER}# Start of projr section: kept note\n");
    let after = format!("{END}\n# my own\n!keep.txt\n");
    let projr = format!("{before}/_raw_data/**\n\n/.projr/logs/**\n{after}");
    let temp = project(Some(&projr));
    let git = refresh(temp.path());
    assert_eq!(git, format!("{before}_tmp/**\n\n{after}"));
    let starts = git
        .lines()
        .filter(|line| line.starts_with("# Start of projr section"));
    assert_eq!(starts.count(), 1);
    assert_eq!(refresh(temp.path()), git);
}

#[test]
fn malformed_markers_fail_without_rewriting_and_lookalike_comments_are_rules() {
    for text in [
        format!("{START}\n{START}\n{END}\n"),
        format!("{END}\nrule\n{START}\n"),
        format!("rule\n{END}\n"),
        format!("{START}  \nrule\n"),
    ] {
        let temp = project(Some(&text));
        assert!(!init(temp.path()).status.success(), "{text}");
        assert_eq!(
            fs::read_to_string(temp.path().join(".gitignore")).unwrap(),
            text
        );
    }
    let comments = "# Start of projr section example\n# End of projr section example\n";
    let temp = project(Some(comments));
    let git = refresh(temp.path());
    assert!(git.ends_with(&format!("{END}\n{comments}")));
    assert_eq!(git.matches(START).count(), 1);
}

#[test]
fn package_rbuildignore_gets_projr_starter_regexes_once() {
    let temp = project(None);
    fs::write(
        temp.path().join("DESCRIPTION"),
        "Package: study\nVersion: 0.1.0\n",
    )
    .unwrap();
    fs::write(temp.path().join(".Rbuildignore"), "^custom$\n").unwrap();
    assert!(init(temp.path()).status.success());
    let rbuild = fs::read_to_string(temp.path().join(".Rbuildignore")).unwrap();
    let starter = "(^|/)\\.vdoc\\.\n(^|/)(\\.DS_Store|Thumbs\\.db)$\n^.*\\.Rproj$\n\
^\\.Rproj\\.user$\n^_projr-local\\.yml$\n";
    assert!(rbuild.starts_with(&format!("{starter}{START}\n^_tmp$\n^_tmp/$\n\n")));
    assert!(rbuild.ends_with(&format!("{END}\n^custom$\n")));
    assert!(init(temp.path()).status.success());
    assert_eq!(
        fs::read_to_string(temp.path().join(".Rbuildignore")).unwrap(),
        rbuild
    );
}

#[test]
fn git_ignores_projr_local_configuration() {
    let temp = project(None);
    refresh(temp.path());
    let empty = temp.path().join("empty-git-config");
    fs::write(&empty, "").unwrap();
    let git = |args: &[&str]| {
        let mut command = Command::new("git");
        for (key, _) in std::env::vars_os() {
            if key
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GIT_")
            {
                command.env_remove(key);
            }
        }
        command
            .args(args)
            .current_dir(temp.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &empty)
            .status()
    };
    let Ok(status) = git(&["init", "--quiet"]) else {
        return; // Git is optional for this check.
    };
    assert!(status.success());
    for ignored in ["_projr-local.yml", "nested/.vdoc.session", "_tmp/x"] {
        let status = git(&["check-ignore", "--quiet", ignored]).unwrap();
        assert!(status.success(), "{ignored}");
    }
}
