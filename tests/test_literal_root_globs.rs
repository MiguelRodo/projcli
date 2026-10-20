use std::{fs, process::Command};

#[test]
fn root_metacharacters_are_literal_while_include_and_exclude_globs_keep_working() {
    for selection in ["[analysis.py]", "['*.py', '!skip*.py']"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("demo [1] with spaces");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("_proj.yml"), format!("directories:\n  cache:\n    path: _tmp\nbuild:\n  scripts: {selection}\n")).unwrap();
        for (script, marker) in [("analysis.py", "analysis"), ("skip-this.py", "excluded")] {
            fs::write(root.join(script), format!("from pathlib import Path\nPath('_tmp').mkdir(exist_ok=True)\nPath('_tmp/{marker}').write_text('ran')\n")).unwrap();
        }
        let output = Command::new(env!("CARGO_BIN_EXE_proj"))
            .args(["build-dev", "--non-interactive", "--project"]).arg(&root)
            .current_dir(&root).env_remove(proj::PROJECT_ROOT_ENV)
            .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
        assert!(output.status.success(), "{} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert_eq!(fs::read_to_string(root.join("_tmp/analysis")).unwrap(), "ran");
        assert!(!root.join("_tmp/excluded").exists());
    }
}


#[test]
fn explicit_failure_executes_under_native_and_canonical_project_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project [2] with spaces");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("_proj.yml"), "directories:\n  cache:\n    path: _tmp\nbuild:\n  scripts: [analysis.py]\n").unwrap();
    fs::write(root.join("analysis.py"), "from pathlib import Path\nPath('_tmp').mkdir(exist_ok=True)\nPath('_tmp/executed').write_text('yes')\nraise SystemExit(17)\n").unwrap();
    let canonical = root.canonicalize().unwrap();
    for project_argument in [&root, &canonical] {
        let output = Command::new(env!("CARGO_BIN_EXE_proj"))
            .args(["build-dev", "--non-interactive", "--project"]).arg(project_argument)
            .current_dir(&root).env_remove(proj::PROJECT_ROOT_ENV)
            .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
        assert!(!output.status.success(), "explicit failing script skipped");
        assert_eq!(fs::read_to_string(root.join("_tmp/executed")).unwrap(), "yes");
        fs::remove_file(root.join("_tmp/executed")).unwrap();
    }
}
