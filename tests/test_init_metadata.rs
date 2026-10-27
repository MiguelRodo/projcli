//! Explicit project init: own temporary files only, no hosting-account mutations.
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
struct Fixture {
    temp: tempfile::TempDir,
    root: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project with spaces");
        fs::create_dir(&root).unwrap();
        fs::create_dir(temp.path().join("home")).unwrap();
        fs::write(temp.path().join("empty-git-config"), "").unwrap();
        Self { temp, root }
    }
    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut c = Command::new(program);
        c.current_dir(&self.root);
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy().to_ascii_uppercase();
            if name.starts_with("GIT_")
                || name.starts_with("GH_")
                || name.starts_with("GITHUB_")
                || name.starts_with("PROJR_")
            {
                c.env_remove(key);
            }
        }
        c.env_remove(proj::PROJECT_ROOT_ENV)
            .env_remove(proj::RUN_CONTEXT_ENV)
            .env_remove("QUARTO_PROFILE")
            .env("HOME", self.temp.path().join("home"))
            .env("USERPROFILE", self.temp.path().join("home"))
            .env("GH_CONFIG_DIR", self.temp.path().join("home/gh"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                self.temp.path().join("empty-git-config"),
            )
            .env("GIT_TERMINAL_PROMPT", "0");
        c
    }
    fn cli(&self, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_proj"))
            .args(["--non-interactive", "--project"])
            .arg(&self.root)
            .args(args)
            .output()
            .unwrap()
    }
    fn git(&self, args: &[&str]) -> Output {
        self.command("git").args(args).output().unwrap()
    }
}
fn good(output: Output) -> Output {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
fn text(root: &Path, name: &str) -> String {
    fs::read_to_string(root.join(name)).unwrap()
}
#[test]
fn noninteractive_defaults_are_local_markdown_only_and_repeatable() {
    let f = Fixture::new();
    good(f.cli(&["init"]));
    for name in ["VERSION", "_proj.yml", "README.md", ".gitignore"] {
        assert!(f.root.join(name).is_file(), "{name}");
    }
    for name in [
        "DESCRIPTION",
        "NAMESPACE",
        "R",
        "README.Rmd",
        "renv",
        "renv.lock",
        ".Rprofile",
        "CITATION.cff",
        "LICENSE",
        ".git",
        ".Rbuildignore",
    ] {
        assert!(!f.root.join(name).exists(), "{name}");
    }
    let before = text(&f.root, "README.md");
    good(f.cli(&["init", "--title", "Do not replace"]));
    assert_eq!(text(&f.root, "README.md"), before);
}
#[test]
fn explicit_description_contact_and_rmd_are_metadata_not_a_package_scaffold() {
    let f = Fixture::new();
    good(f.cli(&[
        "init",
        "--description-file",
        "--readme-rmd",
        "--title",
        "A: quoted 'title'",
        "--purpose",
        "Compare two models.",
        "--contact-name",
        "Project Contact",
        "--contact-email",
        "contact@example.invalid",
    ]));
    let desc = text(&f.root, "DESCRIPTION");
    assert!(desc.contains("Title: A: quoted 'title'\n"));
    assert!(desc.contains("Maintainer: Project Contact <contact@example.invalid>\n"));
    assert!(desc.contains("Version: 0.0.1\n"));
    assert!(!f.root.join("R").exists());
    assert!(!f.root.join("NAMESPACE").exists());
    let readme = text(&f.root, "README.Rmd");
    let header = readme
        .strip_prefix("---\n")
        .unwrap()
        .split("---\n")
        .next()
        .unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(header).unwrap();
    assert_eq!(yaml["title"].as_str(), Some("A: quoted 'title'"));
    assert!(readme.find("Compare two models.").unwrap() < readme.find("## Contact").unwrap());
    assert!(f.root.join(".Rbuildignore").is_file());
    let before = fs::read(f.root.join("DESCRIPTION")).unwrap();
    good(f.cli(&["init", "--description-file", "--readme-rmd"]));
    assert_eq!(fs::read(f.root.join("DESCRIPTION")).unwrap(), before);
}
#[test]
fn citation_requires_explicit_authors_and_never_uses_contact_as_author() {
    let f = Fixture::new();
    assert!(
        !f.cli(&["init", "--citation", "--contact-name", "Not an author"])
            .status
            .success()
    );
    assert!(!f.root.join("VERSION").exists());
    assert!(!f.root.join("CITATION.cff").exists());
    good(f.cli(&[
        "init",
        "--citation",
        "--title",
        "Real project",
        "--contact-name",
        "Not an author",
        "--author-family",
        "Example",
        "--author-given",
        "Ada",
        "--author-name",
        "Research Team",
    ]));
    let value: serde_yaml::Value = serde_yaml::from_str(&text(&f.root, "CITATION.cff")).unwrap();
    assert_eq!(value["cff-version"].as_str(), Some("1.2.0"));
    assert_eq!(value["title"].as_str(), Some("Real project"));
    assert_eq!(
        value["authors"][0]["family-names"].as_str(),
        Some("Example")
    );
    assert_eq!(value["authors"][1]["name"].as_str(), Some("Research Team"));
    for name in ["doi", "license", "references"] {
        assert!(value.get(name).is_none());
    }
    assert!(!text(&f.root, "CITATION.cff").contains("Not an author"));
    let original = fs::read(f.root.join("CITATION.cff")).unwrap();
    good(f.cli(&["init", "--citation"]));
    assert_eq!(fs::read(f.root.join("CITATION.cff")).unwrap(), original);
}
#[test]
fn all_four_code_licences_are_complete_and_unknown_or_missing_holder_fails() {
    for (choice, needle, min) in [
        ("ccby", "Section 8", 15000),
        ("apache", "END OF TERMS AND CONDITIONS", 10000),
        ("cc0", "Public License Fallback", 6000),
    ] {
        let f = Fixture::new();
        good(f.cli(&["init", "--code-license", choice]));
        let license = text(&f.root, "LICENSE");
        assert!(license.contains(needle), "{choice}");
        assert!(license.len() > min);
        assert!(license.starts_with("Scope:"));
        assert!(!license.contains("[Apache License Text]"));
        assert!(!f.root.join("DESCRIPTION").exists());
    }
    let f = Fixture::new();
    assert!(
        !f.cli(&["init", "--code-license", "proprietary"])
            .status
            .success()
    );
    assert!(!f.root.join("LICENSE").exists());
    good(f.cli(&[
        "init",
        "--code-license",
        "proprietary",
        "--copyright-holder",
        "Explicit Owner",
    ]));
    assert!(text(&f.root, "LICENSE").contains("Explicit Owner. All rights reserved."));
    let f = Fixture::new();
    assert!(
        !f.cli(&["init", "--code-license", "unknown"])
            .status
            .success()
    );
    assert!(!f.root.join("VERSION").exists());
}
#[test]
fn plan_is_read_only_and_any_remote_suppresses_remote_setup() {
    let f = Fixture::new();
    good(f.git(&["init"]));
    good(f.git(&[
        "remote",
        "add",
        "elsewhere",
        "https://example.invalid/fixture.git",
    ]));
    let output = good(f.cli(&["--machine", "init", "plan"]));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["result"]["initialization"]["git_repository"], true);
    assert_eq!(value["result"]["initialization"]["remote"], true);
    assert!(!f.root.join("VERSION").exists());
    good(f.cli(&["init", "--git=true", "--github=true"]));
    assert_eq!(
        String::from_utf8(good(f.git(&["remote", "get-url", "elsewhere"])).stdout)
            .unwrap()
            .trim(),
        "https://example.invalid/fixture.git"
    );
}
#[test]
fn enclosing_repository_and_linked_worktree_do_not_get_nested_git_repositories() {
    let f = Fixture::new();
    good(f.git(&["init"]));
    good(f.git(&["config", "user.name", "Fixture"]));
    good(f.git(&["config", "user.email", "fixture@example.invalid"]));
    good(f.git(&["config", "commit.gpgsign", "false"]));
    fs::write(f.root.join("seed"), "seed").unwrap();
    good(f.git(&["add", "seed"]));
    good(f.git(&["commit", "-m", "seed"]));
    let nested = f.root.join("nested");
    fs::create_dir(&nested).unwrap();
    let output = f
        .command(env!("CARGO_BIN_EXE_proj"))
        .args(["--non-interactive", "--project"])
        .arg(&nested)
        .args(["init", "--git=true"])
        .output()
        .unwrap();
    good(output);
    assert!(!nested.join(".git").exists());
    let linked = f.temp.path().join("linked worktree");
    good(f.git(&["worktree", "add", "--detach", linked.to_str().unwrap()]));
    let output = f
        .command(env!("CARGO_BIN_EXE_proj"))
        .args(["--non-interactive", "--project"])
        .arg(&linked)
        .args(["init", "--git=true"])
        .output()
        .unwrap();
    good(output);
    assert!(linked.join(".git").is_file());
}
#[test]
fn malformed_managed_markers_preserve_user_rules_instead_of_accumulating_blocks() {
    let f = Fixture::new();
    fs::write(
        f.root.join(".gitignore"),
        "custom\n# --- PROJ MANAGED ---\n",
    )
    .unwrap();
    assert!(!f.cli(&["init", "directories"]).status.success());
    assert_eq!(
        text(&f.root, ".gitignore"),
        "custom\n# --- PROJ MANAGED ---\n"
    );
}
#[test]
fn package_ignore_uses_escaped_paths_without_excluding_legitimate_package_sources() {
    let f = Fixture::new();
    fs::write(
        f.root.join("DESCRIPTION"),
        "Package: fixture\nVersion: 1.0.0\n",
    )
    .unwrap();
    fs::write(f.root.join("_proj.yml"),"directories:\n  raw-data: {path: 'raw [v1].data'}\n  cache: {path: 'cache+space dir'}\n  raw-functions: {path: R, ignore: []}\n  output: {path: results}\nbuild: {git: false}\n").unwrap();
    fs::write(f.root.join(".Rbuildignore"), "^custom-rule$\n").unwrap();
    good(f.cli(&["init", "directories"]));
    let before = text(&f.root, ".Rbuildignore");
    assert!(before.contains("^raw \\[v1\\]\\.data$"));
    assert!(before.contains("^cache\\+space dir$"));
    assert!(!before.lines().any(|line| line == "^R$" || line == "^R/$"));
    assert!(before.starts_with("^custom-rule$\n"));
    good(f.cli(&["init", "directories"]));
    assert_eq!(text(&f.root, ".Rbuildignore"), before);
    assert!(text(&f.root, ".gitignore").contains(r"raw \[v1\].data/**"));
}
#[cfg(unix)]
#[test]
fn optional_github_uses_only_the_fixture_shim_and_never_pushes() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let bin = f.temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let shim = bin.join("gh");
    fs::write(&shim,"#!/bin/sh\nprintf '%s\\n' \"$@\" > gh-called\ngit remote add fixture https://example.invalid/fixture.git\n").unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let output = f
        .command(env!("CARGO_BIN_EXE_proj"))
        .env("PATH", &path)
        .args(["--non-interactive", "--project"])
        .arg(&f.root)
        .args(["init", "--git=true", "--github=true"])
        .output()
        .unwrap();
    good(output);
    let args = text(&f.root, "gh-called");
    assert!(args.contains("--private"));
    assert!(!args.contains("--push"));
    fs::remove_file(f.root.join("gh-called")).unwrap();
    let output = f
        .command(env!("CARGO_BIN_EXE_proj"))
        .env("PATH", &path)
        .args(["--non-interactive", "--project"])
        .arg(&f.root)
        .args(["init", "--github=true"])
        .output()
        .unwrap();
    good(output);
    assert!(!f.root.join("gh-called").exists());
}

#[test]
fn existing_description_seeds_version_without_normalizing_other_bytes() {
    let f = Fixture::new();
    let desc = "Package: fixture\r\nVersion:   1.2.3-4\r\nTitle: Keep\r\n";
    fs::write(f.root.join("DESCRIPTION"), desc).unwrap();
    good(f.cli(&["init"]));
    assert_eq!(text(&f.root, "DESCRIPTION"), desc);
    assert!(text(&f.root, "VERSION").contains("1.2.3.4"));
}
#[test]
fn existing_description_mismatch_follows_version_unless_opted_out() {
    for disabled in [false, true] {
        let f = Fixture::new();
        fs::write(f.root.join("VERSION"), "v2.3.4-9\n").unwrap();
        let desc = "Package: fixture\r\nVersion: 1.0.0\r\nTitle: Keep\r\n";
        fs::write(f.root.join("DESCRIPTION"), desc).unwrap();
        if disabled {
            fs::write(
                f.root.join("_proj.yml"),
                "settings: {sync-description-version: false}\n",
            )
            .unwrap();
        }
        good(f.cli(&["init"]));
        assert_eq!(
            text(&f.root, "DESCRIPTION"),
            if disabled {
                desc.to_owned()
            } else {
                desc.replace("1.0.0", "2.3.4.9")
            }
        );
        assert_eq!(text(&f.root, "VERSION"), "v2.3.4-9\n");
    }
}
#[test]
fn explicit_rbuild_ignore_policy_is_not_silently_overridden_for_package_directories() {
    let f = Fixture::new();
    fs::write(
        f.root.join("DESCRIPTION"),
        "Package: fixture\nVersion: 1.0.0\n",
    )
    .unwrap();
    fs::write(
        f.root.join("_proj.yml"),
        "directories:\n  raw-source: {path: R, ignore: rbuild}\n",
    )
    .unwrap();
    good(f.cli(&["init", "directories"]));
    assert!(
        text(&f.root, ".Rbuildignore")
            .lines()
            .any(|line| line == "^R$")
    );
}
#[cfg(unix)]
#[test]
fn linked_or_special_metadata_fails_before_new_files_and_keeps_external_bytes() {
    use std::os::unix::fs::symlink;
    for name in [
        "VERSION",
        "README.md",
        "DESCRIPTION",
        "CITATION.cff",
        "LICENSE",
    ] {
        let f = Fixture::new();
        let external = f.temp.path().join("external");
        fs::write(&external, "outside bytes").unwrap();
        symlink(&external, f.root.join(name)).unwrap();
        let args = match name {
            "DESCRIPTION" => vec!["init", "--description-file"],
            "CITATION.cff" => vec!["init", "--citation"],
            "LICENSE" => vec!["init", "--code-license", "ccby"],
            _ => vec!["init"],
        };
        assert!(!f.cli(&args).status.success(), "{name}");
        assert_eq!(fs::read_to_string(&external).unwrap(), "outside bytes");
        assert!(!f.root.join("_proj.yml").exists(), "{name}");
    }
    let f = Fixture::new();
    fs::create_dir(f.root.join("README.md")).unwrap();
    assert!(!f.cli(&["init"]).status.success());
    assert!(!f.root.join("VERSION").exists());
}

#[test]
fn folder_licensing_is_separate_explicit_and_create_only() {
    let f = Fixture::new();
    good(f.cli(&["init", "directories"]));
    good(f.cli(&[
        "init",
        "license",
        "--label",
        "raw-data",
        "--license",
        "ccby",
    ]));
    let path = f.root.join("_raw_data/LICENSE");
    let bytes = fs::read(&path).unwrap();
    assert!(
        String::from_utf8_lossy(&bytes)
            .starts_with("Scope: this licence applies only to material in this directory.")
    );
    assert!(!f.root.join("LICENSE").exists());
    assert!(!f.root.join("_output/LICENSE").exists());
    good(f.cli(&[
        "init",
        "license",
        "--label",
        "raw-data",
        "--license",
        "apache",
    ]));
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert!(
        !f.cli(&[
            "init",
            "license",
            "--label",
            "undeclared",
            "--license",
            "ccby"
        ])
        .status
        .success()
    );
}

#[test]
fn markdown_licence_is_preserved_without_adding_a_conflicting_plain_licence() {
    let f = Fixture::new();
    good(f.cli(&["init", "directories"]));
    for target in [f.root.clone(), f.root.join("_raw_data")] {
        fs::write(target.join("LICENSE.md"), "existing chosen licence\n").unwrap();
    }
    good(f.cli(&["init", "license", "--license", "apache"]));
    good(f.cli(&["init", "license", "--label", "raw-data", "--license", "cc0"]));
    for target in [f.root.clone(), f.root.join("_raw_data")] {
        assert_eq!(text(&target, "LICENSE.md"), "existing chosen licence\n");
        assert!(!target.join("LICENSE").exists());
    }
}

#[test]
fn standalone_readme_preserves_existing_bytes_and_rejects_special_targets() {
    let f = Fixture::new();
    fs::write(f.root.join("README.md"), "Existing source bytes\n").unwrap();
    good(f.cli(&["init", "readme", "--title", "Replacement"]));
    assert_eq!(text(&f.root, "README.md"), "Existing source bytes\n");
    fs::remove_file(f.root.join("README.md")).unwrap();
    fs::create_dir(f.root.join("README.md")).unwrap();
    assert!(!f.cli(&["init", "readme"]).status.success());
}
#[cfg(unix)]
#[test]
fn standalone_readme_rejects_dangling_link_without_external_write() {
    let f = Fixture::new();
    let target = f.temp.path().join("external-readme");
    std::os::unix::fs::symlink(&target, f.root.join("README.md")).unwrap();
    assert!(
        !f.cli(&["init", "readme", "--title", "Do not write"])
            .status
            .success()
    );
    assert!(!target.exists());
    assert!(
        fs::symlink_metadata(f.root.join("README.md"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
