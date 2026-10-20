use std::{fs, path::Path, process::{Command, Output}};

fn invoke(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(args).arg("--project").arg(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .output().unwrap()
}

fn assert_failure(output: Output) {
    assert!(!output.status.success(), "false success: {}", String::from_utf8_lossy(&output.stdout));
    assert!(!output.stderr.is_empty());
}

#[test]
fn query_and_version_errors_are_nonzero() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    assert_failure(invoke(root, &["version", "get"]));
    assert_failure(invoke(root, &["version", "set", "invalid-version"]));
    assert!(!root.join("VERSION").exists());
    assert_failure(invoke(root, &["path", "get", "--label", "unsupported-label"]));
    fs::write(root.join("_proj.yml"), "directories: [malformed\n").unwrap();
    assert_failure(invoke(root, &["yml", "read"]));
    assert_failure(invoke(root, &["path", "get", "--label", "output"]));
}

#[test]
fn ignore_and_initialization_errors_are_nonzero() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::create_dir(root.join(".gitignore")).unwrap();
    assert_failure(invoke(root, &["ignore", "add", "some-file.csv"]));
    fs::write(root.join("DESCRIPTION"), "Package: fixture\nVersion: invalid-version\n").unwrap();
    assert_failure(invoke(root, &["init", "version"]));
    assert_failure(invoke(root, &["init"]));
    assert!(!root.join("VERSION").exists());
    assert!(!root.join(".git").exists());
}

#[test]
fn missing_executable_and_child_failures_remain_nonzero() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::write(root.join("_proj.yml"), "build:\n  scripts: [analysis.py]\n").unwrap();
    fs::write(root.join("analysis.py"), "raise SystemExit(17)\n").unwrap();
    assert_failure(invoke(root, &["build-dev"]));
    let empty_path = root.join("empty-path");
    fs::create_dir(&empty_path).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build-dev", "--project"]).arg(root).env("PATH", &empty_path)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .output().unwrap();
    assert_failure(output);
}

#[test]
fn successful_queries_still_return_zero() {
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("_proj.yml"), "directories:\n  output:\n    path: _output\n").unwrap();
    fs::write(project.path().join("VERSION"), "v1.2.3\n").unwrap();
    for args in [&["version", "get"][..], &["path", "get", "--label", "output"], &["yml", "read"]] {
        let output = invoke(project.path(), args);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(!output.stdout.is_empty());
    }
}

#[cfg(unix)]
#[test]
fn git_and_github_initialization_failures_propagate_without_network() {
    use std::os::unix::fs::PermissionsExt;
    for mode in ["add", "commit", "remote", "github"] {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        fs::create_dir(root.join(".git")).unwrap();
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        fs::write(bin.join("git"), r#"#!/bin/sh
echo "$1" >> calls.txt
if [ "$1" = "$PROJCLI_GIT_FAILURE" ]; then
  echo fixture-git-failure >&2
  exit 23
fi
exit 0
"#).unwrap();
        fs::write(bin.join("gh"), "#!/bin/sh\necho called > gh.called\necho fixture-gh-failure >&2\nexit 24\n").unwrap();
        for executable in ["git", "gh"] {
            fs::set_permissions(bin.join(executable), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let args: &[&str] = if mode == "add" || mode == "commit" {
            &["init", "git", "--commit", "true"]
        } else { &["init", "github", "--public", "false"] };
        let output = Command::new(env!("CARGO_BIN_EXE_proj"))
            .args(args).arg("--project").arg(root).env("PATH", &bin)
            .env("PROJCLI_GIT_FAILURE", mode)
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
            .output().unwrap();
        assert_failure(output);
        if mode == "add" { assert!(!fs::read_to_string(root.join("calls.txt")).unwrap().contains("commit")); }
        if mode == "remote" { assert!(!root.join("gh.called").exists()); }
        if mode == "github" { assert!(root.join("gh.called").exists()); }
    }
}
