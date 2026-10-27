use std::{fs, path::Path, process::{Command, Output}};

fn setup(root: &Path) {
    fs::write(root.join("_proj.yml"), "build:\n  scripts: [analysis.py]\n").unwrap();
    fs::write(root.join("analysis.py"), "from pathlib import Path\nPath('analysis-ran').touch()\n").unwrap();
}
fn run(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build-dev", "--non-interactive", "--project"]).arg(root).current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .output().unwrap()
}

#[test]
fn invalid_assignments_fail_without_panic_secret_output_or_analysis() {
    for (name, contents) in [
        ("_environment", b"=secret-sentinel\n".as_slice()),
        ("_environment.local", b"BAD\0KEY=secret-sentinel\n".as_slice()),
        ("_environment", b"PROJ_ENV_VALUE=secret\0-sentinel\n".as_slice()),
        ("_environment.required", b"=secret-sentinel\n".as_slice()),
        ("_environment.required", b"BAD\0KEY\n".as_slice()),
    ] {
        let project = tempfile::tempdir().unwrap();
        let root = project.path(); setup(root);
        fs::write(root.join(name), contents).unwrap();
        let output = run(root);
        assert!(!output.status.success());
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(diagnostic.contains("Invalid"), "{diagnostic}");
        assert!(!diagnostic.contains("secret"));
        assert!(!diagnostic.contains("panicked"));
        for absent in ["analysis-ran", "VERSION", ".gitignore", ".Rbuildignore"] { assert!(!root.join(absent).exists()); }
    }
}

#[test]
fn environment_and_required_directory_inputs_return_read_errors() {
    for name in ["_environment", "_environment.local", "_environment.required", "_environment-profile"] {
        let project = tempfile::tempdir().unwrap();
        let root = project.path(); setup(root);
        fs::create_dir(root.join(name)).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_proj"));
        command.args(["build-dev", "--non-interactive", "--project"]).arg(root)
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove("QUARTO_PROFILE");
        if name == "_environment-profile" { command.env("PROJR_PROFILE", "profile"); }
        else { command.env_remove("PROJR_PROFILE"); }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("Cannot read"));
        assert!(!root.join("analysis-ran").exists());
    }
}

#[test]
fn inherited_then_local_then_earlier_profile_then_base_precedence_is_preserved() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path(); setup(root);
    fs::write(root.join("_environment"), "PROJ_ENV_INHERITED=base\nPROJ_ENV_LOCAL=base\nPROJ_ENV_PROFILE=base\nPROJ_ENV_BASE=base\n").unwrap();
    fs::write(root.join("_environment-first"), "PROJ_ENV_PROFILE=first\n").unwrap();
    fs::write(root.join("_environment-second"), "PROJ_ENV_PROFILE=second\n").unwrap();
    fs::write(root.join("_environment.local"), "PROJ_ENV_INHERITED=local\nPROJ_ENV_LOCAL=local\n").unwrap();
    fs::write(root.join("_environment.required"), "PROJ_ENV_INHERITED\nPROJ_ENV_LOCAL\nPROJ_ENV_PROFILE\nPROJ_ENV_BASE\n").unwrap();
    fs::write(root.join("analysis.py"), "import os\nfrom pathlib import Path\nPath('values').write_text('|'.join(os.environ[k] for k in ['PROJ_ENV_INHERITED','PROJ_ENV_LOCAL','PROJ_ENV_PROFILE','PROJ_ENV_BASE']))\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build-dev", "--non-interactive", "--project"]).arg(root).current_dir(root)
        .env("PROJ_ENV_INHERITED", "inherited").env("PROJR_PROFILE", "first,second")
        .env_remove("PROJ_ENV_LOCAL").env_remove("PROJ_ENV_PROFILE").env_remove("PROJ_ENV_BASE")
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("QUARTO_PROFILE").output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(fs::read_to_string(root.join("values")).unwrap(), "inherited|local|first|base");
}

#[cfg(unix)]
#[test]
fn unreadable_required_file_does_not_silently_disable_validation() {
    use std::os::unix::fs::PermissionsExt;
    let project = tempfile::tempdir().unwrap();
    let root = project.path(); setup(root);
    let path = root.join("_environment.required");
    fs::write(&path, "PROJ_ENV_REQUIRED\n").unwrap();
    let permissions = fs::metadata(&path).unwrap().permissions();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let output = run(root);
    fs::set_permissions(&path, permissions).unwrap();
    assert!(!output.status.success());
    assert!(!root.join("analysis-ran").exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Cannot read"));
}

#[test]
fn public_assignment_validation_rejects_invalid_os_inputs_without_mutation() {
    let mut guard = proj::EnvGuard::new();
    for (key, value) in [("", "secret"), ("BAD=KEY", "secret"), ("BAD\0KEY", "secret"), ("PROJ_ENV_SAFE", "secret\0value")] {
        assert!(guard.track_and_set(key, value).is_err());
    }
}
