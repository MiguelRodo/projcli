use std::{fs, process::Command};

fn build(root: &std::path::Path, mode: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["--machine", "--non-interactive", "--project"])
        .arg(root).arg(mode).current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .env_remove("PROJR_CLEAR_OUTPUT").output().unwrap()
}

#[test]
fn malformed_script_and_hook_exclusions_fail_before_execution_or_managed_writes() {
    let selections = [
        ("['analysis.py', '![[']", "    pre: [hook.py]\n"),
        ("['![[']", "    pre: [hook.py]\n"),
        ("[analysis.py]", "    pre: [hook.py, '![[']\n"),
        ("[analysis.py]", "    pre: [hook.py]\n    post: [hook.py, '![[']\n"),
        ("[analysis.py]", "    both: [hook.py, '![[']\n"),
        ("[]", "    post: ['![[']\n"),
    ];
    for mode in ["build-dev", "build"] {
        for (scripts, hooks) in selections {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path().canonicalize().unwrap().join("project [1] with spaces");
            fs::create_dir(&root).unwrap();
            for name in ["released", "archive"] { fs::create_dir(root.join(name)).unwrap(); }
            fs::write(root.join("released/previous.csv"), b"old,release\n1,2\n").unwrap();
            fs::write(root.join("archive/previous.bin"), b"prior archive").unwrap();
            fs::write(root.join("VERSION"), b"v1.2.3-4\n").unwrap();
            fs::write(root.join(".gitignore"), b"manual-rule\n").unwrap();
            let config = format!("directories:\n  cache: {{path: cache}}\n  output: {{path: released}}\nsettings:\n  output-clear-stage: pre\nbuild:\n  git: false\n  scripts: {scripts}\n  hooks:\n{hooks}");
            fs::write(root.join("_proj.yml"), &config).unwrap();
            for name in ["analysis", "hook"] {
                fs::write(root.join(format!("{name}.py")), format!(
                    "from pathlib import Path\nPath('{name}-ran').write_text('must not run')\n"
                )).unwrap();
            }

            let output = build(&root, mode);
            let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            let diagnostic = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success(), "{mode}/{scripts}/{hooks}: {diagnostic}");
            assert_eq!(envelope["ok"], false);
            assert_eq!(envelope["protocol_version"], 1);
            assert_eq!(envelope["error"]["code"], "command_failed");
            assert!(diagnostic.contains("Invalid script/hook exclusion pattern '![["),
                "{diagnostic}");
            for name in ["analysis-ran", "hook-ran", "cache", ".Rbuildignore", ".git", ".projr"] {
                assert!(!root.join(name).exists(), "{mode}/{scripts}/{hooks}: unexpected {name}");
            }
            assert_eq!(fs::read(root.join("released/previous.csv")).unwrap(), b"old,release\n1,2\n");
            assert_eq!(fs::read(root.join("archive/previous.bin")).unwrap(), b"prior archive");
            assert_eq!(fs::read(root.join("VERSION")).unwrap(), b"v1.2.3-4\n");
            assert_eq!(fs::read(root.join(".gitignore")).unwrap(), b"manual-rule\n");
            assert_eq!(fs::read_to_string(root.join("_proj.yml")).unwrap(), config);
        }
    }
}

#[test]
fn valid_exclusions_keep_first_match_order_deduplication_and_ignored_explicit_targets() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap().join("project [2] with spaces");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("_proj.yml"),
        "directories:\n  cache: {path: cache}\nbuild:\n  git: false\n  scripts: [second.py, '*.py', '!skip*.py', second.py]\n"
    ).unwrap();
    fs::write(root.join(".gitignore"), "first.py\n").unwrap();
    for name in ["first", "second", "skip"] {
        fs::write(root.join(format!("{name}.py")), format!(
            "from pathlib import Path\nPath('cache').mkdir(exist_ok=True)\nwith Path('cache/order.txt').open('a') as stream: stream.write('{name}\\n')\n"
        )).unwrap();
    }
    let output = build(&root, "build-dev");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(fs::read_to_string(root.join("cache/order.txt")).unwrap().lines().collect::<Vec<_>>(),
        ["second", "first"]);
    assert_eq!(fs::read_to_string(root.join(".gitignore")).unwrap(), "first.py\n");
    assert!(!root.join("VERSION").exists());
}
