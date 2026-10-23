use std::{fs, path::Path, process::{Command, Output}};

fn build(root: &Path, mode: &str) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["--machine", "--non-interactive", "--project"]).arg(root).arg(mode)
        .current_dir(root).env_remove(proj::PROJECT_ROOT_ENV)
        .env_remove(proj::RUN_CONTEXT_ENV).env_remove("PROJR_PROFILE")
        .env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT").output()
}

#[cfg(unix)]
#[test]
fn unreadable_root_keeps_ownership_phase_and_cause_before_hooks_or_managed_writes() {
    use std::os::unix::fs::PermissionsExt;
    for mode in ["build-dev", "build"] {
        for explicit_empty in [false, true] {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path().canonicalize().unwrap().join("project [1] with spaces");
            fs::create_dir(&root).unwrap();
            for directory in ["cache", "released", "archive"] { fs::create_dir(root.join(directory)).unwrap(); }
            fs::write(root.join("cache/previous"), b"prior cache").unwrap();
            fs::write(root.join("released/previous.csv"), b"prior output").unwrap();
            fs::write(root.join("archive/previous.bin"), b"prior archive").unwrap();
            fs::write(root.join("VERSION"), b"v1.2.3-4\n").unwrap();
            fs::write(root.join(".gitignore"), b"manual-rule\n").unwrap();
            let selection = if explicit_empty { "  scripts: []\n" } else { "" };
            let config = format!("directories:\n  cache: {{path: cache}}\n  output: {{path: released}}\nsettings:\n  output-clear-stage: pre\nbuild:\n  git: false\n{selection}");
            fs::write(root.join("_proj.yml"), &config).unwrap();
            fs::write(root.join("analysis.py"), "from pathlib import Path\nPath('cache/analysis-ran').write_text('unexpected')\n").unwrap();

            let original_permissions = fs::metadata(&root).unwrap().permissions();
            // Known config files are readable, but enumerating the directory is denied.
            fs::set_permissions(&root, fs::Permissions::from_mode(0o300)).unwrap();
            let config_readable = fs::read_to_string(root.join("_proj.yml")).is_ok();
            let enumeration_denied = fs::read_dir(&root).is_err();
            let output = if config_readable && enumeration_denied { Some(build(&root, mode)) } else { None };
            // Restore before any assertions or unwrapping child failures.
            fs::set_permissions(&root, original_permissions).unwrap();
            assert!(config_readable && enumeration_denied, "fixture requires an unprivileged Unix user");
            let output = output.unwrap().unwrap();
            let diagnostic = String::from_utf8_lossy(&output.stderr);
            let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert!(!output.status.success(), "{mode}/{explicit_empty}: {diagnostic}");
            assert_eq!(envelope["ok"], false);
            assert_eq!(envelope["protocol_version"], 1);
            assert_eq!(envelope["error"]["code"], "command_failed");
            // Ownership is now validated before any mutable input capture or scan.
            let phase = "Cannot acquire build writer ownership at";
            assert!(diagnostic.contains(&format!("{phase} {}", root.display())), "{diagnostic}");
            assert!(diagnostic.contains("Cannot inspect native store names"), "{diagnostic}");
            assert!(diagnostic.to_lowercase().contains("permission denied"), "{diagnostic}");
            assert_eq!(fs::read(root.join("cache/previous")).unwrap(), b"prior cache");
            assert_eq!(fs::read_dir(root.join("cache")).unwrap().count(), 1);
            assert_eq!(fs::read(root.join("released/previous.csv")).unwrap(), b"prior output");
            assert_eq!(fs::read(root.join("archive/previous.bin")).unwrap(), b"prior archive");
            assert_eq!(fs::read(root.join("VERSION")).unwrap(), b"v1.2.3-4\n");
            assert_eq!(fs::read(root.join(".gitignore")).unwrap(), b"manual-rule\n");
            assert_eq!(fs::read_to_string(root.join("_proj.yml")).unwrap(), config);
            for name in [".Rbuildignore", ".projr", ".proj", ".git"] { assert!(!root.join(name).exists()); }
        }
    }
}

#[test]
fn readable_fallback_keeps_hidden_ignored_and_alphabetical_shallow_selection() {
    let temporary = tempfile::tempdir().unwrap();
    let parent = temporary.path().canonicalize().unwrap();
    let root = parent.join("project [2] with spaces");fs::create_dir(&root).unwrap();
    fs::write(root.join("_proj.yml"), "directories:\n  cache: {path: cache}\nbuild:\n  git: false\n").unwrap();
    fs::write(root.join(".gitignore"), "01-first.py\n").unwrap();
    for (name, label) in [("02-second.py", "second"), (".hidden.py", "hidden"), ("01-first.py", "first")] {
        fs::write(root.join(name), format!("from pathlib import Path\nPath('cache').mkdir(exist_ok=True)\nwith Path('cache/order').open('a') as stream: stream.write('{label}\\n')\n")).unwrap();
    }
    fs::create_dir(root.join("nested")).unwrap();
    fs::write(root.join("nested/must-not-run.py"), "raise SystemExit(17)\n").unwrap();
    fs::write(root.join("ignored.pyc"), b"unsupported sidecar").unwrap();
    #[cfg(unix)] {
        let external = parent.join("external.py");
        fs::write(&external, "raise SystemExit(17)\n").unwrap();
        std::os::unix::fs::symlink(&external, root.join("linked.py")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("loop.py")).unwrap();
    }
    let output = build(&root, "build-dev").unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(fs::read_to_string(root.join("cache/order")).unwrap().lines().collect::<Vec<_>>(),
        ["hidden", "first", "second"]);
    assert_eq!(fs::read_to_string(root.join(".gitignore")).unwrap(), "01-first.py\n");
    assert!(!root.join("VERSION").exists());
}
