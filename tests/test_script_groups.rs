use std::{fs,process::Command};
fn run(root:&std::path::Path,args:&[&str])->std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_proj")).args(["--non-interactive","--project"]).arg(root)
        .args(args).current_dir(root).env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap()
}
fn fixture(selection:&str)->tempfile::TempDir {
    let temp=tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("source with spaces/nested")).unwrap();
    fs::write(temp.path().join("_proj.yml"),format!(
        "directories:\n  scripts-l1: {{path: 'source with spaces'}}\n  cache: {{path: cache}}\nbuild:\n  git: false\n  scripts: {selection}\n")).unwrap();
    for name in ["a","skip","nested/b"] {
        fs::write(temp.path().join(format!("source with spaces/{name}.py")),format!(
            "from pathlib import Path\nPath('cache').mkdir(exist_ok=True)\nwith Path('cache/order').open('a') as f: f.write('{name}\\n')\n")).unwrap();
    }
    fs::write(temp.path().join("source with spaces/manual.html"),"hand-authored").unwrap();
    fs::write(temp.path().join("source with spaces/data.csv"),"data").unwrap();
    temp
}
#[test]
fn directory_references_and_globs_keep_order_filter_assets_and_use_zero_depth_double_star() {
    for (selection,expected) in [
        ("[scripts-l1]",vec!["a","nested/b","skip"]),
        ("['source with spaces/*.py']",vec!["a","skip"]),
        ("['source with spaces/**/*.py','!source with spaces/skip*']",vec!["a","nested/b"]),
        ("['source with spaces/nested/b.py','source with spaces/**/*','!**/skip*']",vec!["nested/b","a"]),
        ("['source with spaces/**/*.py','!source with spaces/*.py']",vec!["nested/b"]),
    ] {
        let temp=fixture(selection);
        let output=run(temp.path(),&["build-dev"]);
        assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
        assert_eq!(fs::read_to_string(temp.path().join("cache/order")).unwrap().lines().collect::<Vec<_>>(),expected);
        assert!(String::from_utf8_lossy(&output.stderr).contains("Selected analysis scripts"));
        assert_eq!(fs::read_to_string(temp.path().join("source with spaces/manual.html")).unwrap(),"hand-authored");
    }
}
#[test]
fn unmatched_or_unsafe_patterns_fail_before_creating_cache() {
    for selection in ["['source with spaces/**/*.R']","['source with spaces/../*.py']","[scripts-l1,'![[']","['source with spaces/manual.html']"] {
        let temp=fixture(selection);
        let output=run(temp.path(),&["build-dev"]);
        assert!(!output.status.success(),"selection unexpectedly passed: {selection}");
        assert!(!temp.path().join("cache").exists());
    }
}
#[test]
fn script_defaults_preserve_sources_and_user_rules_and_only_rbuild_excludes_the_group() {
    let temp=fixture("[]");
    fs::write(temp.path().join("DESCRIPTION"),"Title: Generic project\nVersion: 1.2.3\n").unwrap();
    fs::write(temp.path().join(".gitignore"),"my-manual-rule\n").unwrap();
    let output=run(temp.path(),&["init","directories"]);
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    let git=fs::read_to_string(temp.path().join(".gitignore")).unwrap();
    assert!(git.contains("my-manual-rule"));
    assert!(!git.contains("source with spaces"));
    let rbuild=fs::read_to_string(temp.path().join(".Rbuildignore")).unwrap();
    assert!(rbuild.contains("^source with spaces$"));
    assert!(!rbuild.contains("^R$"));
    let before=fs::read(temp.path().join(".Rbuildignore")).unwrap();
    assert!(run(temp.path(),&["init","directories"]).status.success());
    assert_eq!(fs::read(temp.path().join(".Rbuildignore")).unwrap(),before);
}
#[cfg(unix)]
#[test]
fn discovered_symlink_is_rejected_without_following_it() {
    let temp=fixture("[scripts-l1]");
    let outside=tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(),temp.path().join("source with spaces/linked")).unwrap();
    let output=run(temp.path(),&["build-dev"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("symbolic links"));
    assert!(!temp.path().join("cache").exists());
}

#[test]
fn literal_absolute_includes_and_excludes_are_symmetric_and_canonical() {
    let temp=fixture("[]");
    let file=temp.path().join("source with spaces/a.py").canonicalize().unwrap();
    let selections=vec![file.to_str().unwrap().to_owned(),format!("!{}",file.display())];
    fs::write(temp.path().join("_proj.yml"),format!(
        "directories:\n  scripts-l1: {{path: 'source with spaces'}}\n  cache: {{path: cache}}\nbuild:\n  git: false\n  scripts: {}\n",serde_json::to_string(&selections).unwrap())).unwrap();
    let output=run(temp.path(),&["build-dev"]);
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    assert!(!temp.path().join("cache/order").exists());
}
#[test]
fn grouped_file_and_asset_ancestor_collisions_fail_before_start_hooks() {
    for group in ["x.html","report_files"] {
        let temp=tempfile::tempdir().unwrap();
        let root=temp.path();
        fs::create_dir_all(root.join("first")).unwrap();
        fs::create_dir_all(root.join("second")).unwrap();
        fs::write(root.join("first/report.qmd"),"---\nformat: html\noutput-file: x.html\n---\nFirst\n").unwrap();
        fs::write(root.join("second/other.qmd"),"---\nformat: html\n---\nSecond\n").unwrap();
        fs::write(root.join("hook.py"),"from pathlib import Path\nPath('hook-ran').write_text('unexpected')\n").unwrap();
        fs::write(root.join("_proj.yml"),format!(
            "directories:\n  cache: {{path: cache}}\n  docs: {{path: docs}}\n  scripts: {{path: first}}\n  scripts-{group}: {{path: second}}\nbuild:\n  git: false\n  scripts: [scripts, scripts-{group}]\n  hooks:\n    pre: [hook.py]\n")).unwrap();
        let output=run(root,&["build-dev"]);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("collide"),"{}",String::from_utf8_lossy(&output.stderr));
        assert!(!root.join("hook-ran").exists());
        assert!(!root.join("cache").exists());
    }
}
