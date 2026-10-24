use std::{collections::BTreeMap, fs, path::{Path, PathBuf}, process::Command};

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(root: &Path, here: &Path, map: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(here).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_owned();
            if relative.starts_with("cache") { continue; }
            if path.is_dir() { map.insert(relative, None); walk(root, &path, map); }
            else { map.insert(relative, Some(fs::read(path).unwrap())); }
        }
    }
    let mut map = BTreeMap::new(); walk(root, root, &mut map); map
}

// Compare every prior byte/presence exactly; permit only the one new approved local diagnostic.
fn recovered_snapshot(root:&Path,before:&BTreeMap<PathBuf,Option<Vec<u8>>>)->BTreeMap<PathBuf,Option<Vec<u8>>> {
    let mut after=snapshot(root);
    let directory=Path::new(".proj/log/local-failures");
    let added:Vec<_>=after.keys().filter(|p|p.starts_with(directory)&&!before.contains_key(*p)).cloned().collect();
    let mut diagnostics=0;
    for path in added {
        let bytes=after.remove(&path).unwrap();
        if path==directory {assert!(bytes.is_none());continue;}
        let file=path.strip_prefix(directory).unwrap();
        assert_eq!(file.components().count(),1,"unexpected diagnostic subtree");
        let bytes=bytes.expect("unexpected diagnostic subdirectory");
        if file==Path::new(".gitignore") {assert_eq!(bytes,b"*\n");continue;}
        assert!(bytes.len()<=1_048_576);
        let value:serde_json::Value=serde_json::from_slice(&bytes).unwrap();
        let id=value["attempt_id"].as_str().unwrap();
        assert!(id.starts_with("attempt-")&&id.len()==32&&id.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-'));
        assert_eq!(file,Path::new(&format!("{id}.json")));
        assert_eq!(value["schema_version"],1);
        assert_eq!(value["status"],"failed");
        assert_eq!(value["distribution_may_have_started"],false);
        diagnostics+=1;
    }
    assert_eq!(diagnostics,1);
    for parent in [".proj",".proj/log"] {
        let parent=Path::new(parent);
        if !before.contains_key(parent){assert_eq!(after.remove(parent),Some(None));}
    }
    assert!(!root.join(".proj/build.lock").exists());
    after
}

#[test]
fn failure_at_each_hook_and_analysis_restores_exact_version_authorities_and_dev_counters() {
    let authorities: [(&str, Option<&[u8]>, Option<&[u8]>); 5] = [
        ("absent", None, None),
        ("release-version", Some(b"v1.2.3\n"), None),
        ("dev-seven", Some(b"Version: v1.2.3-7\r\n"), None),
        ("both-authorities", Some(b"Version: v1.2.3.7\n"),
         Some(b"Package: fixture\nVersion: 1.2.3.42\n")),
        ("description-only", None, Some(b"Package: fixture\nVersion: 1.2.3.19\n")),
    ];
    for phase in ["pre", "analysis", "post", "both"] {
        for (authority, version_bytes, description_bytes) in authorities {
            let project = tempfile::tempdir().unwrap();
            let root = project.path();
            let hooks = if phase == "analysis" { String::new() } else { format!("  hooks:\n    {phase}: [fail.py]\n") };
            let scripts = if phase == "analysis" { "[fail.py]" } else { "[analysis.py]" };
            fs::write(root.join("_proj.yml"), format!("directories:\n  cache:\n    path: cache\n  output:\n    path: output\n  docs:\n    path: published\nconfig:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: {scripts}\n{hooks}  git: false\n  clear_output: pre\n  restrictions:\n    not_behind: false\n")).unwrap();
            fs::write(root.join(".gitignore"), "# exact prior ignore\nmanual\n").unwrap();
            fs::write(root.join(".Rbuildignore"), "^manual$\n").unwrap();
            fs::write(root.join("analysis.py"), "from pathlib import Path\nPath('output/new').write_text('new')\n").unwrap();
            fs::write(root.join("fail.py"), "from pathlib import Path\nPath('output').mkdir(exist_ok=True)\nPath('output/partial').write_text('partial')\nPath('published/old.html').write_text('changed')\nPath('VERSION').write_text('Version: v9.9.9.1')\nPath('DESCRIPTION').write_text('Package: fixture\\nVersion: 9.9.9.1\\n')\nraise SystemExit(17)\n").unwrap();
            fs::create_dir_all(root.join("output/empty")).unwrap();
            fs::create_dir(root.join("published")).unwrap();
            fs::write(root.join("output/previous"), b"old\0output").unwrap();
            fs::write(root.join("published/old.html"), "old docs").unwrap();
            if let Some(bytes) = version_bytes { fs::write(root.join("VERSION"), bytes).unwrap(); }
            if let Some(bytes) = description_bytes { fs::write(root.join("DESCRIPTION"), bytes).unwrap(); }
            fs::create_dir_all(root.join(".proj/log")).unwrap();
            fs::write(root.join(".proj/log/keep-prior-evidence"),b"preserve unrelated evidence").unwrap();
            let before = snapshot(root);
            let result = Command::new(env!("CARGO_BIN_EXE_proj"))
                .args(["build", "--patch", "--non-interactive", "--project"]).arg(root).current_dir(root)
                .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
                .env_remove("PROJR_CLEAR_OUTPUT").output().unwrap();
            assert!(!result.status.success(), "{phase}");
            assert!(String::from_utf8_lossy(&result.stderr).contains("17"), "{}", String::from_utf8_lossy(&result.stderr));
            assert_eq!(recovered_snapshot(root,&before), before, "{phase}, authority={authority}");
        }
    }
}

#[test]
fn publication_failure_reports_local_success_and_keeps_the_new_release() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::write(root.join("_proj.yml"), "directories:\n  cache:\n    path: cache\n  output:\n    path: output\nremotes:\n  local:\n    archive:\n      path: not-a-directory\n      structure: cas\n      inspect: manifest\n      content: [output]\nconfig:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: [analysis.py]\n  git: false\n  dest: [archive]\n  restrictions:\n    not_behind: false\n").unwrap();
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    fs::write(root.join("not-a-directory"), "publication blocker").unwrap();
    fs::write(root.join("analysis.py"), "from pathlib import Path\nPath('output').mkdir(exist_ok=True)\nPath('output/result').write_text('successful analysis')\n").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build", "--patch", "--non-interactive", "--project"]).arg(root).current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .env_remove("PROJR_CLEAR_OUTPUT").output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Local build succeeded"));
    assert_eq!(fs::read_to_string(root.join("output/result")).unwrap(), "successful analysis");
    assert_eq!(proj::version_get_from(camino::Utf8Path::from_path(root).unwrap()).unwrap().to_string(false), "1.2.4");
}


#[cfg(unix)]
#[test]
fn missing_renderer_promotion_is_a_failure_and_restores_exact_project_config() {
    use std::os::unix::fs::PermissionsExt;
    for mode in ["missing", "empty", "failed"] {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let original = "# keep comments and render selection\nproject:\n  type: default\n  render: [report.qmd]\nformat: html\n";
        fs::write(root.join("_quarto.yml"), original).unwrap();
        fs::write(root.join("_proj.yml"), "directories:\n  cache:\n    path: cache\n  docs:\n    path: published\nconfig:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: [report.qmd]\n  git: false\n  restrictions:\n    not_behind: false\n").unwrap();
        fs::write(root.join("report.qmd"), "---\nformat: html\n---\nReport\n").unwrap();
        fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
        fs::create_dir(root.join("published")).unwrap();
        fs::write(root.join("published/old.html"), "previous release").unwrap();
        fs::create_dir(root.join("bin")).unwrap();
        let fake = format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo fake; exit 0; fi\n{}\nexit {}\n", if mode == "empty" { "mkdir -p cache/projr/1.2.3/docs/empty" } else { "" }, if mode == "failed" { 17 } else { 0 });
        for name in ["quarto", "Rscript"] {
            let path = root.join("bin").join(name);
            fs::write(&path, &fake).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let before = snapshot(&root);
        let mut paths = vec![root.join("bin")];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
        let result = Command::new(env!("CARGO_BIN_EXE_proj"))
            .args(["build", "--patch", "--non-interactive", "--project"]).arg(&root).current_dir(&root)
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
            .env_remove("PROJR_CLEAR_OUTPUT").output().unwrap();
        assert!(!result.status.success(), "missing render output must not report success");
        assert_eq!(recovered_snapshot(&root,&before), before, "mode={mode}: {}", String::from_utf8_lossy(&result.stderr));
        assert_eq!(fs::read_to_string(root.join("_quarto.yml")).unwrap(), original);
    }
}


#[test]
fn version_write_failure_restores_files_after_successful_analysis_and_post_hook() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::create_dir(root.join("output")).unwrap();
    fs::write(root.join("output/previous"), "old release").unwrap();
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    fs::write(root.join("_proj.yml"), "directories:\n  cache:\n    path: cache\n  output:\n    path: output\nconfig:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: [analysis.py]\n  hooks:\n    post: [block-version.py]\n  git: false\n  restrictions:\n    not_behind: false\n").unwrap();
    fs::write(root.join("analysis.py"), "from pathlib import Path\nPath('output/new').write_text('new release')\n").unwrap();
    fs::write(root.join("block-version.py"), "from pathlib import Path\np = Path('VERSION')\np.unlink()\np.mkdir()\n").unwrap();
    let before = snapshot(root);
    let result = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build", "--patch", "--non-interactive", "--project"]).arg(root).current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .env_remove("PROJR_CLEAR_OUTPUT").output().unwrap();
    assert!(!result.status.success());
    assert_eq!(recovered_snapshot(root,&before), before);
}

#[cfg(unix)]
#[test]
fn backup_failure_precedes_authoritative_writes_and_preserves_external_bytes() {
    let project = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::create_dir(root.join("output")).unwrap();
    fs::write(external.path().join("keep"), "external sentinel").unwrap();
    std::os::unix::fs::symlink(external.path().join("keep"), root.join("output/linked")).unwrap();
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    fs::write(root.join(".gitignore"), "manual\n").unwrap();
    fs::write(root.join("_proj.yml"), "directories:\n  cache:\n    path: cache\n  output:\n    path: output\nconfig:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: [analysis.py]\n  git: false\n  restrictions:\n    not_behind: false\n").unwrap();
    fs::write(root.join("analysis.py"), "from pathlib import Path\nPath('output/analysis-ran').touch()\n").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build", "--patch", "--non-interactive", "--project"]).arg(root).current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .env_remove("PROJR_CLEAR_OUTPUT").output().unwrap();
    assert!(!result.status.success());
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
    assert_eq!(fs::read_to_string(root.join(".gitignore")).unwrap(), "manual\n");
    assert!(!root.join("output/analysis-ran").exists());
    assert_eq!(fs::read_to_string(external.path().join("keep")).unwrap(), "external sentinel");
}

fn contains_bytes(directory: &Path, expected: &[u8]) -> bool {
    fs::read_dir(directory).unwrap().any(|entry| {
        let path = entry.unwrap().path();
        if path.is_dir() { contains_bytes(&path, expected) }
        else { fs::read(path).unwrap() == expected }
    })
}

#[test]
fn post_clear_precedes_promotion_and_archive_contains_actual_analysis_bytes() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::write(root.join("_proj.yml"), "directories:\n  cache:\n    path: cache\n  output:\n    path: output\nremotes:\n  local:\n    archive:\n      path: archives\n      structure: cas\n      inspect: manifest\n      content: [output]\nconfig:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: [analysis.py]\n  git: false\n  clear_output: post\n  dest: [archive]\n  restrictions:\n    not_behind: false\n").unwrap();
    fs::write(root.join("analysis.py"), format!(r#"from pathlib import Path
import json,subprocess
value=json.loads(subprocess.check_output([{binary},'--machine','path','get','--label','output','--target','safe','result.csv','--format','absolute'],text=True))
target=Path(value['result']['path']);target.parent.mkdir(parents=True,exist_ok=True);target.write_text('actual-analysis-bytes')
"#,binary=serde_json::to_string(env!("CARGO_BIN_EXE_proj")).unwrap())).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build", "--patch", "--non-interactive", "--project"]).arg(root).current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .env_remove("PROJR_CLEAR_OUTPUT").output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(contains_bytes(&root.join("archives"), b"actual-analysis-bytes"));
    assert_eq!(fs::read(root.join("output/result.csv")).unwrap(), b"actual-analysis-bytes");
}


#[test]
fn nested_selected_scripts_and_hooks_restore_themselves_after_failure() {
    for hook in [false, true] {
        for deleted in [false, true] {
            let project = tempfile::tempdir().unwrap();
            let root = project.path();
            fs::create_dir(root.join("nested")).unwrap();
            let script = if deleted { "from pathlib import Path\nPath(__file__).unlink()\nraise SystemExit(17)\n" }
                else { "from pathlib import Path\nPath(__file__).write_text('changed')\nraise SystemExit(17)\n" };
            fs::write(root.join("nested/fail.py"), script).unwrap();
            let selection = if hook { "  scripts: []\n  hooks:\n    pre: [nested/fail.py]\n" }
                else { "  scripts: [nested/fail.py]\n" };
            fs::write(root.join("_proj.yml"), format!("directories:\n  cache:\n    path: cache\nconfig:\n  git:\n    use_proj_cred_helper: false\nbuild:\n{selection}  git: false\n  restrictions:\n    not_behind: false\n")).unwrap();
            let result = Command::new(env!("CARGO_BIN_EXE_proj"))
                .args(["build", "--patch", "--non-interactive", "--project"]).arg(root).current_dir(root)
                .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
                .env_remove("PROJR_CLEAR_OUTPUT").output().unwrap();
            assert!(!result.status.success());
            assert!(String::from_utf8_lossy(&result.stderr).contains("17"));
            assert_eq!(fs::read_to_string(root.join("nested/fail.py")).unwrap(), script, "hook={hook}, deleted={deleted}");
            assert!(!root.join("VERSION").exists());
        }
    }
}


#[test]
fn purging_disposable_cache_cannot_destroy_recovery_or_the_previous_release() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::create_dir(root.join("output")).unwrap();
    fs::write(root.join("output/previous"), "old release").unwrap();
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    fs::write(root.join("_proj.yml"), "directories:\n  cache:\n    path: cache\n  output:\n    path: output\nconfig:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: [analysis.py]\n  git: false\n  restrictions:\n    not_behind: false\n").unwrap();
    fs::write(root.join("analysis.py"), "import shutil\nshutil.rmtree('cache', ignore_errors=True)\nraise SystemExit(17)\n").unwrap();
    let before = snapshot(root);
    let result = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build", "--patch", "--non-interactive", "--project"]).arg(root).current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .env_remove("PROJR_CLEAR_OUTPUT").output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("17"));
    assert_eq!(recovered_snapshot(root,&before), before);
}

#[cfg(unix)]
#[test]
fn no_op_renderer_cannot_promote_a_stale_individual_document() {
    use std::os::unix::fs::PermissionsExt;
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    fs::create_dir(root.join("published")).unwrap();
    fs::write(root.join("published/old.html"), "old released docs").unwrap();
    fs::write(root.join("report.html"), "stale native output").unwrap();
    fs::write(root.join("report.qmd"), "---\nformat: html\n---\nReport\n").unwrap();
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    fs::write(root.join("_proj.yml"), "directories:\n  cache:\n    path: cache\n  docs:\n    path: published\nconfig:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: [report.qmd]\n  git: false\n  restrictions:\n    not_behind: false\n").unwrap();
    fs::create_dir(root.join("bin")).unwrap();
    for name in ["quarto", "Rscript"] {
        let path = root.join("bin").join(name);
        fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let before = snapshot(&root);
    let mut paths = vec![root.join("bin")];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    let result = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build", "--patch", "--non-interactive", "--project"]).arg(&root).current_dir(&root)
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .env_remove("PROJR_CLEAR_OUTPUT").output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("missing"));
    assert_eq!(recovered_snapshot(&root,&before), before);
}


#[test]
fn direct_project_promotion_rejects_empty_and_asset_only_trees_before_clearing_docs() {
    for mode in ["empty", "assets", "zero-document", "document"] {
        let project = tempfile::tempdir().unwrap();
        let root_path = project.path().canonicalize().unwrap();
        let root = camino::Utf8Path::from_path(&root_path).unwrap();
        let staging = root.join("staging");
        let docs = root.join("docs");
        fs::create_dir_all(staging.join("empty")).unwrap();
        fs::create_dir(&docs).unwrap();
        fs::write(docs.join("old.html"), "old document").unwrap();
        match mode {
            "assets" => fs::write(staging.join("style.css"), "body {}").unwrap(),
            "zero-document" => fs::write(staging.join("report.html"), "").unwrap(),
            "document" => fs::write(staging.join("report.html"), "new document").unwrap(),
            _ => {}
        }
        let result = proj::copy_global_quarto_project(&staging, &docs);
        assert_eq!(result.is_ok(), mode == "document");
        if mode == "document" { assert_eq!(fs::read_to_string(docs.join("report.html")).unwrap(), "new document"); }
        else { assert_eq!(fs::read_to_string(docs.join("old.html")).unwrap(), "old document"); }
    }
}


#[cfg(unix)]
#[test]
fn selected_link_is_rejected_before_mutation_instead_of_backing_only_its_referent() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::create_dir(root.join("nested")).unwrap();
    fs::write(root.join("nested/actual.py"), "from pathlib import Path\nPath(__file__).unlink()\nraise SystemExit(17)\n").unwrap();
    std::os::unix::fs::symlink("actual.py", root.join("nested/runner.py")).unwrap();
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    fs::write(root.join("_proj.yml"), "directories:\n  cache:\n    path: cache\nconfig:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: [nested/runner.py]\n  git: false\n  restrictions:\n    not_behind: false\n").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build", "--patch", "--non-interactive", "--project"]).arg(root).current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .env_remove("PROJR_CLEAR_OUTPUT").output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("symlink"));
    assert!(root.join("nested/runner.py").symlink_metadata().unwrap().file_type().is_symlink());
    assert!(root.join("nested/actual.py").is_file());
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
    for absent in [".gitignore", ".Rbuildignore", "cache"] { assert!(!root.join(absent).exists()); }
}

#[cfg(unix)]
#[test]
fn project_pdf_artifacts_succeed_with_document_or_inherited_project_format() {
    use std::os::unix::fs::PermissionsExt;
    for inherited in [false, true] {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let project_yaml = if inherited { "# inherited format\nproject:\n  type: default\nformat: pdf\n" }
            else { "# document format\nproject:\n  type: default\n" };
        fs::write(root.join("_quarto.yml"), project_yaml).unwrap();
        fs::write(root.join("report.qmd"), if inherited { "Report\n" } else { "---\nformat: pdf\n---\nReport\n" }).unwrap();
        fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
        fs::write(root.join("_proj.yml"), "directories:\n  cache:\n    path: cache\n  docs:\n    path: published\nconfig:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: [report.qmd]\n  git: false\n  restrictions:\n    not_behind: false\n").unwrap();
        fs::create_dir(root.join("bin")).unwrap();
        fs::write(root.join("bin/Rscript"), "#!/bin/sh\nexit 0\n").unwrap();
        fs::write(root.join("bin/quarto"), "#!/usr/bin/env python3\nimport os, sys\nfrom pathlib import Path\nif sys.argv[1:2]==['inspect']: print('{\"files\":{\"input\":[]}}'); raise SystemExit(0)\nif sys.argv[1:2] == ['render']:\n root=Path(os.environ['PROJ_PROJECT_ROOT'])\n line=next(line for line in (root/'_quarto.yml').read_text().splitlines() if line.strip().startswith('output-dir:'))\n p=Path(line.split(':',1)[1].strip().strip(chr(34)).strip(chr(39)))\n p.mkdir(parents=True,exist_ok=True)\n (p/'report.pdf').write_bytes(b'%PDF-fresh-document')\n").unwrap();
        for name in ["quarto", "Rscript"] { fs::set_permissions(root.join("bin").join(name), fs::Permissions::from_mode(0o755)).unwrap(); }
        let mut paths = vec![root.join("bin")];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
        let result = Command::new(env!("CARGO_BIN_EXE_proj"))
            .args(["build", "--patch", "--non-interactive", "--project"]).arg(&root).current_dir(&root)
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
            .env_remove("PROJR_CLEAR_OUTPUT").output().unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        assert_eq!(fs::read(root.join("published/report.pdf")).unwrap(), b"%PDF-fresh-document");
        assert_eq!(fs::read_to_string(root.join("_quarto.yml")).unwrap(), project_yaml);
    }
}


#[cfg(unix)]
#[test]
fn bookdown_native_assets_restore_after_post_promotion_hook_failure() {
    use std::os::unix::fs::PermissionsExt;
    for name in ["_main", "custom-book"] {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let yaml = if name == "_main" { "# preserve default\n{}\n".to_string() }
            else { format!("# preserve custom\nbook_filename: {name}\n") };
        fs::write(root.join("_bookdown.yml"), &yaml).unwrap();
        fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
        fs::write(root.join("report.Rmd"), "Report\n").unwrap();
        fs::write(root.join("post.py"), "raise SystemExit(17)\n").unwrap();
        fs::write(root.join("_proj.yml"), "directories:\n  cache:\n    path: cache\n  docs:\n    path: published\nconfig:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: [report.Rmd]\n  hooks:\n    post: [post.py]\n  git: false\n  restrictions:\n    not_behind: false\n").unwrap();
        let assets = root.join(format!("{name}_files"));
        fs::create_dir_all(assets.join("empty")).unwrap();
        fs::write(assets.join("old.png"), b"old-native-asset").unwrap();
        fs::create_dir(root.join("published")).unwrap();
        fs::write(root.join("published/old.html"), "previous docs").unwrap();
        fs::create_dir(root.join("bin")).unwrap();
        let fake = format!("#!/usr/bin/env python3\nimport os, sys\nfrom pathlib import Path\nif any('render_site' in arg for arg in sys.argv):\n root = Path(os.environ['PROJ_PROJECT_ROOT'])\n line=next(line for line in (root/'_bookdown.yml').read_text().splitlines() if line.strip().startswith('output_dir:'))\n staging=Path(line.split(':',1)[1].strip().strip(chr(34)).strip(chr(39)))\n staging.mkdir(parents=True, exist_ok=True)\n (staging / 'index.html').write_text('new document')\n (root / '{name}_files').mkdir(parents=True, exist_ok=True)
 (root / '{name}_files' / 'new.png').write_text('new asset')\n");
        fs::write(root.join("bin/Rscript"), fake).unwrap();
        fs::set_permissions(root.join("bin/Rscript"), fs::Permissions::from_mode(0o755)).unwrap();
        let before = snapshot(&root);
        let mut paths = vec![root.join("bin")];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
        let result = Command::new(env!("CARGO_BIN_EXE_proj"))
            .args(["build", "--patch", "--non-interactive", "--project"]).arg(&root).current_dir(&root)
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
            .env_remove("PROJR_CLEAR_OUTPUT").output().unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("17"), "{}", String::from_utf8_lossy(&result.stderr));
        assert_eq!(recovered_snapshot(&root,&before), before, "book_filename={name}");
    }
}


#[test]
fn no_distribution_cas_preparation_failures_retain_one_local_diagnostic() {
    for case in ["missing-source","end-hook","observation"] {
        let empty_content=case!="missing-source";
        let temporary=tempfile::tempdir().unwrap();
        let root=temporary.path().canonicalize().unwrap();
        fs::write(root.join("VERSION"),b"v1.2.3-7\n").unwrap();
        fs::write(root.join("fail.py"),if case=="observation" {
            "from pathlib import Path\nPath('missing-output').write_bytes(b'not a directory')\n"
        } else {"raise SystemExit(17)\n"}).unwrap();
        let content=if empty_content {"[]"}else{"[output]"};
        let hook=if empty_content {"  hooks: {post: [fail.py]}\n"}else{""};
        fs::write(root.join("_proj.yml"),format!("directories:\n  output: {{path: missing-output}}\nremotes:\n  local:\n    archive:\n      path: archives\n      structure: cas\n      inspect: manifest\n      content: {content}\nbuild:\n  scripts: []\n  git: false\n  dest: [archive]\n{hook}  restrictions:\n    not_behind: false\n")).unwrap();
        let result=std::process::Command::new(env!("CARGO_BIN_EXE_proj"))
            .current_dir(&root).args(["--machine","--non-interactive","--project"]).arg(&root).arg("build")
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
            .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT")
            .output().unwrap();
        assert!(!result.status.success(),"case {empty_content}");
        let root_json=fs::read_dir(root.join(".proj/log")).unwrap()
            .filter(|entry|entry.as_ref().unwrap().path().extension().is_some_and(|e|e=="json")).count();
        assert_eq!(root_json,0,"duplicate ordinary record in case {empty_content}");
        let diagnostics:Vec<_>=fs::read_dir(root.join(".proj/log/local-failures")).unwrap()
            .map(|e|e.unwrap().path()).filter(|p|p.extension().is_some_and(|e|e=="json")).collect();
        assert_eq!(diagnostics.len(),1);
        let history=std::process::Command::new(env!("CARGO_BIN_EXE_proj"))
            .args(["--machine","--project"]).arg(&root).arg("history").output().unwrap();
        assert!(history.status.success(),"{}",String::from_utf8_lossy(&history.stderr));
        let value:serde_json::Value=serde_json::from_slice(&history.stdout).unwrap();
        assert_eq!(value["result"]["history"]["entries"].as_array().unwrap().len(),1);
        assert_eq!(value["result"]["history"]["entries"][0]["local_only"],true);
        assert!(!root.join("archives").exists());
    }
}
