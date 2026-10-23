//! Real CLI cooperative ownership: fixtures never alter global environment.
use std::{fs, path::Path, process::Command, thread, time::{Duration, Instant}};

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_proj"));
    command.current_dir(root).args(["build", "--patch", "--non-interactive", "--project"]).arg(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT");
    command
}

fn fixture(root: &Path, scripts: &str) {
    fs::write(root.join("_proj.yml"), format!("directories:\n  cache: {{path: cache}}\nbuild:\n  scripts: {scripts}\n  git: false\n  restrictions:\n    not_behind: false\n")).unwrap();
    fs::write(root.join("VERSION"), b"v1.2.3-7\n").unwrap();
}

#[test]
fn concurrent_attempt_is_rejected_then_a_fresh_identity_can_succeed() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    fixture(&root, "[analysis.py]");
    fs::write(root.join("analysis.py"), r#"from pathlib import Path
import time
Path('started').write_text('ready')
deadline=time.monotonic()+20
while not Path('continue').exists():
    if time.monotonic()>deadline:
        raise RuntimeError('fixture release timeout')
    time.sleep(0.02)
"#).unwrap();
    let mut first = command(&root).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !root.join("started").exists() {
        assert!(Instant::now() < deadline, "first build did not reach analysis");
        assert!(first.try_wait().unwrap().is_none(), "first build exited before analysis");
        thread::sleep(Duration::from_millis(20));
    }
    let claim = fs::read(root.join(".proj/build.lock")).unwrap();
    let identity: serde_json::Value = serde_json::from_slice(&claim).unwrap();
    let id = identity["attempt_id"].as_str().unwrap();
    assert!(id.starts_with("attempt-") && id.len() >= 24);
    let during = fs::read(root.join("VERSION")).unwrap();
    let second = command(&root).output().unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("project build writer"));
    assert_eq!(fs::read(root.join(".proj/build.lock")).unwrap(), claim);
    assert_eq!(fs::read_dir(root.join(".proj")).unwrap().count(), 1);
    assert_eq!(fs::read(root.join("VERSION")).unwrap(), during);
    fs::write(root.join("continue"), b"release").unwrap();
    assert!(first.wait().unwrap().success());
    assert!(!root.join(".proj/build.lock").exists());
    assert_eq!(fs::read_dir(root.join(".proj/log")).unwrap().count(),1);

    // A second successful run gets a fresh identity, including with .proj retained.
    fs::create_dir_all(root.join(".proj")).unwrap();
    fs::write(root.join(".proj/keep"), b"unrelated existing state").unwrap();
    fs::write(root.join("analysis.py"), "from pathlib import Path\nPath('identity.json').write_bytes(Path('.proj/build.lock').read_bytes())\n").unwrap();
    let third = command(&root).output().unwrap();
    assert!(third.status.success(), "{}", String::from_utf8_lossy(&third.stderr));
    let next: serde_json::Value = serde_json::from_slice(&fs::read(root.join("identity.json")).unwrap()).unwrap();
    assert_ne!(identity["attempt_id"], next["attempt_id"]);
    assert!(!root.join(".proj/build.lock").exists());
    assert_eq!(fs::read(root.join(".proj/keep")).unwrap(), b"unrelated existing state");
}

#[test]
fn stale_claim_and_invalid_configuration_preserve_exact_prior_state() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    fixture(&root, "[]");
    fs::create_dir(root.join(".proj")).unwrap();
    fs::write(root.join(".proj/build.lock"), b"opaque prior owner").unwrap();
    let before = fs::read(root.join("VERSION")).unwrap();
    let result = command(&root).output().unwrap();
    assert!(!result.status.success());
    assert_eq!(fs::read(root.join("VERSION")).unwrap(), before);
    assert_eq!(fs::read(root.join(".proj/build.lock")).unwrap(), b"opaque prior owner");
    fs::remove_file(root.join(".proj/build.lock")).unwrap();
    fs::remove_dir(root.join(".proj")).unwrap();
    fs::write(root.join("_proj.yml"), b"broken: [").unwrap();
    assert!(!command(&root).output().unwrap().status.success());
    assert!(!root.join(".proj").exists());
    assert_eq!(fs::read(root.join("VERSION")).unwrap(), before);
}

#[test]
fn managed_outputs_cannot_replace_coordination_state() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    fixture(&root, "[]");
    let config = fs::read_to_string(root.join("_proj.yml")).unwrap()
        .replace("  cache:", "  output: {path: .proj}\n  cache:");
    fs::write(root.join("_proj.yml"), config).unwrap();
    let result = command(&root).output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("protected project metadata"));
    assert_eq!(fs::read(root.join("VERSION")).unwrap(), b"v1.2.3-7\n");
    assert!(!root.join(".proj").exists());
}

#[cfg(unix)]
#[test]
fn linked_coordination_directory_is_rejected_without_touching_external_state() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    let external = temporary.path().join("external");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&external).unwrap();
    fixture(&root, "[]");
    fs::write(external.join("sentinel"), b"preserve").unwrap();
    std::os::unix::fs::symlink(&external, root.join(".proj")).unwrap();
    assert!(!command(&root).output().unwrap().status.success());
    assert_eq!(fs::read(external.join("sentinel")).unwrap(), b"preserve");
    assert_eq!(fs::read_dir(&external).unwrap().count(), 1);
}

#[test]
fn preview_cas_recovery_cannot_replace_coordination_state() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    fixture(&root, "[]");
    let config = fs::read_to_string(root.join("_proj.yml")).unwrap()
        .replace("  cache:", "  output: {path: output}\n  cache:");
    fs::write(root.join("_proj.yml"), config + "\nremotes:\n  local:\n    archive:\n      path: .proj\n      structure: cas\n      inspect: manifest\n      content: [output]\n").unwrap();
    fs::create_dir(root.join(".proj")).unwrap();
    fs::write(root.join(".proj/keep"), b"preserve coordination").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_proj"))
        .current_dir(&root).args(["build-dev", "--non-interactive", "--project"]).arg(&root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("coordination"), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(fs::read(root.join(".proj/keep")).unwrap(), b"preserve coordination");
    assert!(!root.join(".proj/build.lock").exists());
    assert_eq!(fs::read(root.join("VERSION")).unwrap(), b"v1.2.3-7\n");
}

#[test]
fn casefolded_coordination_names_fail_without_modifying_the_alias() {
    for (directory, claim) in [(".PROJ", "keep"), (".proj", "BUILD.LOCK")] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        fixture(&root, "[]");
        fs::create_dir(root.join(directory)).unwrap();
        fs::write(root.join(directory).join(claim), b"preserve alias").unwrap();
        if !root.join(".proj").exists() || (claim == "BUILD.LOCK" && !root.join(".proj/build.lock").exists()) {
            continue; // Native case-sensitive filesystem: these are distinct names.
        }
        let before = fs::read(root.join(directory).join(claim)).unwrap();
        let result = command(&root).output().unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("native filesystem alias"));
        assert_eq!(fs::read(root.join(directory).join(claim)).unwrap(), before);
        assert_eq!(fs::read_dir(root.join(directory)).unwrap().count(), 1);
        assert_eq!(fs::read(root.join("VERSION")).unwrap(), b"v1.2.3-7\n");
    }
}
