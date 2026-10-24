#[cfg(unix)]
use std::path::Path;
use std::{fs, path::PathBuf, process::Command, sync::{Arc, Barrier}, time::{Duration, Instant}};

#[test]
fn overlapping_library_builds_keep_profiles_values_and_parent_environment_independent() {
    let container = tempfile::tempdir().unwrap();
    let roots = [container.path().join("alpha"), container.path().join("beta")];
    let keys = ["PROJCLI_CONTEXT_TEST", "PROJR_PROFILE", proj::PROJECT_ROOT_ENV, proj::RUN_CONTEXT_ENV];
    let before: Vec<_> = keys.iter().map(std::env::var_os).collect();
    for (index, name) in ["alpha", "beta"].iter().enumerate() {
        let root = &roots[index];
        fs::create_dir(root).unwrap();
        fs::write(root.join("_proj.yml"), "build:\n  scripts: [wrong-profile.py]\n").unwrap();
        fs::write(root.join(format!("_projr-{name}.yml")), "dev:\n  scripts: [analysis.py]\n").unwrap();
        fs::write(root.join(format!("_environment-{name}")), format!("PROJCLI_CONTEXT_TEST={name}\n")).unwrap();
        let other = serde_json::to_string(&roots[1-index].join("ready").to_string_lossy()).unwrap();
        let release = serde_json::to_string(&container.path().join("release").to_string_lossy()).unwrap();
        fs::write(root.join("analysis.py"), format!("import os, time, json\nfrom pathlib import Path\nPath('ready').touch()\ndeadline = time.monotonic() + 10\nwhile not Path({other}).exists():\n if time.monotonic() > deadline: raise RuntimeError('other context never started')\n time.sleep(0.01)\nwhile not Path({release}).exists():\n if time.monotonic() > deadline: raise RuntimeError('parent never released contexts')\n time.sleep(0.01)\nPath('observed.json').write_text(json.dumps([os.environ['PROJCLI_CONTEXT_TEST'], os.environ['PROJR_PROFILE'], os.environ['PROJ_PROJECT_ROOT']]))\n")).unwrap();
    }
    let barrier = Arc::new(Barrier::new(3));
    let handles: Vec<_> = roots.iter().zip(["alpha", "beta"]).map(|(root, name)| {
        let root = root.clone(); let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            proj::build_project_with_execution(camino::Utf8Path::from_path(&root).unwrap(), proj::BuildMode::Dev, Some(name), None, proj::ExecutionDirectory::Project)
        })
    }).collect();
    barrier.wait();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !roots.iter().all(|root| root.join("ready").exists()) {
        assert!(Instant::now() < deadline, "contexts did not overlap");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(keys.iter().map(std::env::var_os).collect::<Vec<_>>(), before);
    let python = proj::find_python_command().unwrap();
    let unrelated = Command::new(python).args(["-c", "import os,json; print(json.dumps([os.environ.get(k) for k in ['PROJCLI_CONTEXT_TEST','PROJR_PROFILE','PROJ_PROJECT_ROOT','PROJ_RUN_CONTEXT']]))"]).output().unwrap();
    assert!(unrelated.status.success());
    let inherited: Vec<Option<String>> = serde_json::from_slice(&unrelated.stdout).unwrap();
    let expected: Vec<_> = before.iter().map(|value| value.as_ref().map(|value| value.to_string_lossy().into_owned())).collect();
    assert_eq!(inherited, expected);
    fs::write(container.path().join("release"), "continue").unwrap();
    for handle in handles { handle.join().unwrap().unwrap(); }
    assert_eq!(keys.iter().map(std::env::var_os).collect::<Vec<_>>(), before);
    for (root, name) in roots.iter().zip(["alpha", "beta"]) {
        let observed: Vec<String> = serde_json::from_slice(&fs::read(root.join("observed.json")).unwrap()).unwrap();
        assert_eq!(observed[0], name);
        assert_eq!(observed[1], name);
        assert_eq!(PathBuf::from(&observed[2]), root.canonicalize().unwrap());
    }
}

#[cfg(unix)]
fn fake_path(root: &Path, name: &str, body: &str) -> std::ffi::OsString {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(root.join("bin")).unwrap();
    let path = root.join("bin").join(name);
    fs::write(&path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    let mut paths = vec![root.join("bin")];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    std::env::join_paths(paths).unwrap()
}

#[cfg(unix)]
#[test]
fn r_version_probe_package_probe_and_render_share_the_resolved_values() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::create_dir(root.join("_tmp")).unwrap();
    let path = fake_path(root, "Rscript", "#!/bin/sh\nmkdir -p \"$PROJ_PROJECT_ROOT/_tmp\"\nprintf '%s\\n' \"$PROJCLI_PROBE_CONTEXT\" >> \"$PROJ_PROJECT_ROOT/_tmp/seen\"\ncase \"$2\" in *render*) printf fresh > \"$PROJ_PROJECT_ROOT/report.html\" ;; esac\nexit 0\n");
    fs::write(root.join("_environment"), "PROJCLI_PROBE_CONTEXT=probe-fixture\n").unwrap();
    fs::write(root.join("_proj.yml"), "directories:\n  cache:\n    path: _tmp\n  docs:\n    path: published\ndev:\n  scripts: [report.Rmd]\n").unwrap();
    fs::write(root.join("report.Rmd"), "Report\n").unwrap();
    let before = std::env::var_os("PROJCLI_PROBE_CONTEXT");
    let result = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build-dev", "--non-interactive", "--project"]).arg(root).current_dir(root)
        .env("PATH", path).env_remove("PROJCLI_PROBE_CONTEXT").env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .env_remove(proj::PROJECT_ROOT_ENV).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(std::env::var_os("PROJCLI_PROBE_CONTEXT"), before);
    let seen = fs::read_to_string(root.join("_tmp/seen")).unwrap();
    assert!(seen.lines().count() >= 3);
    assert!(seen.lines().all(|value| value == "probe-fixture"));
}

#[cfg(unix)]
#[test]
fn configured_git_children_share_the_run_environment_without_mutating_the_parent() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    let path = fake_path(root, "git", "#!/bin/sh\nprintf '%s\\n' \"$PROJCLI_GIT_CONTEXT\" >> \"$PROJ_PROJECT_ROOT/git-seen\"\ncase \"$1\" in\nls-files) exit 0 ;;\nrev-parse) if [ \"$2\" = --absolute-git-dir ]; then printf \"%s/.git\\n\" \"$PROJ_PROJECT_ROOT\"; fi ;;\nconfig) echo fixture ;;\nrev-list) echo 0 ;;\nsymbolic-ref) echo main ;;\nesac\nexit 0\n");
    fs::write(root.join("_environment"), "PROJCLI_GIT_CONTEXT=git-fixture\n").unwrap();
    fs::write(root.join("_proj.yml"), "config:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: []\n  git: true\n  restrictions:\n    not_behind: false\n").unwrap();
    fs::create_dir(root.join(".git")).unwrap();
    let before = std::env::var_os("PROJCLI_GIT_CONTEXT");
    let result = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build", "--patch", "--non-interactive", "--project"]).arg(root)
        .env("PATH", path).env_remove("PROJCLI_GIT_CONTEXT").env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .env_remove(proj::PROJECT_ROOT_ENV).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(std::env::var_os("PROJCLI_GIT_CONTEXT"), before);
    let seen = fs::read_to_string(root.join("git-seen")).unwrap();
    assert!(seen.lines().count() >= 5);
    assert!(seen.lines().all(|value| value == "git-fixture"));
}


#[cfg(unix)]
#[test]
fn inherited_non_unicode_values_remain_present_and_override_file_values() {
    use std::os::unix::ffi::OsStringExt;
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::write(root.join("_proj.yml"), "dev:\n  scripts: [analysis.py]\n").unwrap();
    fs::write(root.join("_environment"), "PROJCLI_NATIVE_VALUE=from-file\n").unwrap();
    fs::write(root.join("_environment.required"), "PROJCLI_NATIVE_VALUE\n").unwrap();
    fs::write(root.join("analysis.py"), "import os\nfrom pathlib import Path\nPath('native-value').write_text(os.environb[b'PROJCLI_NATIVE_VALUE'].hex())\n").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build-dev", "--non-interactive", "--project"]).arg(root).current_dir(root)
        .env("PROJCLI_NATIVE_VALUE", std::ffi::OsString::from_vec(vec![255, 97, 128]))
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(fs::read_to_string(root.join("native-value")).unwrap(), "ff6180");
}

#[cfg(windows)]
#[test]
fn inherited_ascii_key_case_remains_equivalent_on_windows() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::write(root.join("_proj.yml"), "dev:\n  scripts: [analysis.py]\n").unwrap();
    fs::write(root.join("_environment"), "PROJCLI_CASE_TEST=from-file\n").unwrap();
    fs::write(root.join("analysis.py"), "import os\nfrom pathlib import Path\nPath('case-value').write_text(os.environ['PROJCLI_CASE_TEST'])\n").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build-dev", "--non-interactive", "--project"]).arg(root).current_dir(root)
        .env("projcli_case_test", "inherited")
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(fs::read_to_string(root.join("case-value")).unwrap(), "inherited");
}


#[cfg(windows)]
#[test]
fn unicode_case_aliases_cannot_override_inherited_windows_environment_keys() {
    for filename in ["_environment", "_environment.required"] {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        fs::write(root.join("_proj.yml"), "dev:\n  scripts: [analysis.py]\n").unwrap();
        fs::write(root.join(filename), "PROJCLI_é=file\n").unwrap();
        fs::write(root.join("analysis.py"), "from pathlib import Path\nPath('analysis-ran').touch()\n").unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_proj"))
            .args(["build-dev", "--non-interactive", "--project"]).arg(root).current_dir(root)
            .env("PROJCLI_É", "parent")
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
            .output().unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("Non-ASCII environment assignment keys"));
        assert!(!root.join("analysis-ran").exists());
        for absent in ["VERSION", ".gitignore", ".Rbuildignore"] { assert!(!root.join(absent).exists()); }
    }
}
