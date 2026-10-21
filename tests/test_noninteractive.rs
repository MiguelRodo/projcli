use std::{fs, io::Write, process::{Command, Stdio}};

#[test]
fn noninteractive_runner_gets_eof_even_when_cli_stdin_has_data() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::write(root.join("_proj.yml"), "directories:\n  cache:\n    path: _tmp\nbuild:\n  scripts: [analysis.py]\n").unwrap();
    fs::write(root.join("analysis.py"), "import sys\nfrom pathlib import Path\nvalue = sys.stdin.read(1)\nPath('_tmp').mkdir(exist_ok=True)\nPath('_tmp/stdin.txt').write_text('eof' if value == '' else 'input')\n").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["--non-interactive", "build-dev", "--project"]).arg(root).current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::NON_INTERACTIVE_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    let _ = input.write_all(b"A");
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "stdout={} stderr={}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert_eq!(fs::read_to_string(root.join("_tmp/stdin.txt")).unwrap(), "eof");
}

#[cfg(unix)]
#[test]
fn noninteractive_dependency_failure_never_auto_installs() {
    use std::os::unix::fs::PermissionsExt;
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    fs::write(root.join("_proj.yml"), "directories:\n  docs:\n    path: docs\nbuild:\n  scripts: [analysis.Rmd]\n").unwrap();
    fs::write(root.join("analysis.Rmd"), "fixture").unwrap();
    fs::write(root.join("_environment.local"), "PROJ_NON_INTERACTIVE=false\n").unwrap();
    fs::write(bin.join("Rscript"), r#"#!/bin/sh
if [ "$1" = --version ]; then exit 0; fi
case "$2" in
  *install.packages*) echo attempted > install-attempted; exit 0 ;;
esac
exit 1
"#).unwrap();
    fs::set_permissions(bin.join("Rscript"), fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build-dev", "--non-interactive", "--project"]).arg(root)
        .current_dir(root).env("PATH", &bin).env("PROJR_AUTO_INSTALL", "true")
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::NON_INTERACTIVE_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Required R package"));
    assert!(!root.join("install-attempted").exists());
    assert!(!root.join("VERSION").exists());
}


#[cfg(unix)]
#[test]
fn noninteractive_r_probe_gets_eof_before_dependency_checks() {
    use std::os::unix::fs::PermissionsExt;
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    fs::write(root.join("_proj.yml"), "directories:\n  docs:\n    path: docs\nbuild:\n  scripts: [analysis.Rmd]\n").unwrap();
    fs::write(root.join("analysis.Rmd"), "fixture").unwrap();
    fs::write(bin.join("Rscript"), r#"#!/bin/sh
if [ "$1" = --version ]; then exit 0; fi
case "$2" in
  *install.packages*) echo attempted > install-attempted; exit 0 ;;
esac
if IFS= read -r value; then
  echo input > probe-stdin
else
  echo eof > probe-stdin
fi
exit 1
"#).unwrap();
    fs::set_permissions(bin.join("Rscript"), fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build-dev", "--non-interactive", "--project"]).arg(root)
        .current_dir(root).env("PATH", &bin).env("PROJR_AUTO_INSTALL", "true")
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::NON_INTERACTIVE_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    let _ = input.write_all(b"A\n");
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(root.join("probe-stdin")).unwrap().trim(), "eof");
    assert!(!root.join("install-attempted").exists());
}
