use std::{fs, path::Path, process::Command};
use camino::Utf8Path;
use proj::manifest::{self, ManifestRow};

fn path(path: &Path) -> &Utf8Path { Utf8Path::from_path(path).unwrap() }
fn invoke(root: &Path, mode: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_proj"))
        .args([mode, "--non-interactive", "--project"]).arg(root).current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT")
        .output().unwrap()
}
fn fixture(root: &Path, extra: &str) {
    fs::write(root.join("VERSION"), b"v1.2.3-4\n").unwrap();
    fs::write(root.join("_proj.yml"), format!("directories:\n  raw: {{path: input}}\n  cache: {{path: cache}}\n  output: {{path: released}}\n  docs: {{path: docs}}\nbuild:\n  scripts: []\n  git: false\n{extra}")).unwrap();
    fs::create_dir(root.join("input")).unwrap();
    fs::write(root.join("input/a.csv"), b"abc").unwrap();
    fs::create_dir(root.join("released")).unwrap();
    fs::write(root.join("released/result.csv"), b"hello").unwrap();
}
fn success(output: std::process::Output) {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn production_records_md5_pre_inputs_final_outputs_and_exact_history_without_git_or_remotes() {
    let temporary = tempfile::tempdir().unwrap(); let root = temporary.path();
    fixture(root, "");
    fs::write(root.join("analysis.py"), "import os\nfrom pathlib import Path\nroot = Path(os.environ['PROJ_PROJECT_ROOT'])\n(root / 'input/a.csv').write_bytes(b'changed')\ntarget = root / 'cache/projr/v1.2.4/output/result.csv'\ntarget.parent.mkdir(parents=True, exist_ok=True)\ntarget.write_bytes(b'hello')\n").unwrap();
    let config = fs::read_to_string(root.join("_proj.yml")).unwrap();
    fs::write(root.join("_proj.yml"), config.replace("scripts: []", "scripts: [analysis.py]")).unwrap();
    fs::write(root.join("released/result.csv"), b"prior").unwrap();
    success(invoke(root, "build"));
    let rows = manifest::read(path(&root.join("manifest.csv"))).unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().any(|r| r.label == "raw" && r.r#fn == "a.csv" && r.version == "v1.2.4" && r.hash == "900150983cd24fb0d6963f7d28e17f72"));
    assert!(rows.iter().any(|r| r.label == "output" && r.r#fn == "result.csv" && r.hash == "5d41402abc4b2a76b9719d911017c592"));
    assert!(rows.iter().any(|r| r.label == "docs" && r.r#fn.is_empty() && r.hash.is_empty()));
    assert!(!rows.iter().any(|r| r.label == "cache"));
    let pre = manifest::read(path(&root.join("cache/projr/v1.2.4/manifest.csv"))).unwrap();
    assert_eq!(pre, rows.iter().filter(|r| r.label == "raw").cloned().collect::<Vec<_>>());
    assert_eq!(fs::read(root.join("released/result.csv")).unwrap(), b"hello");
    fs::write(root.join("_proj.yml"), config).unwrap();
    success(invoke(root, "build"));
    let history = manifest::read(path(&root.join("manifest.csv"))).unwrap();
    assert_eq!(history.len(), 6);
    assert!(rows.iter().all(|r| history.contains(r)));
    assert!(history.iter().any(|r| r.label == "raw" && r.version == "v1.2.5" && r.hash != "900150983cd24fb0d6963f7d28e17f72"));
}

#[test]
fn configured_hash_flags_and_cache_framework_exclusion_match_projr() {
    let temporary = tempfile::tempdir().unwrap(); let root = temporary.path();
    fixture(root, "");
    let config = fs::read_to_string(root.join("_proj.yml")).unwrap()
        .replace("raw: {path: input}", "raw: {path: input, hash: false}")
        .replace("cache: {path: cache}", "cache: {path: cache, hash: true}");
    fs::write(root.join("_proj.yml"), config).unwrap();
    fs::create_dir_all(root.join("cache/projr/v0.1.0")).unwrap();
    fs::write(root.join("cache/projr/v0.1.0/private"), b"framework").unwrap();
    fs::write(root.join("cache/retained.csv"), b"abc").unwrap();
    success(invoke(root, "build"));
    let rows = manifest::read(path(&root.join("manifest.csv"))).unwrap();
    assert!(!rows.iter().any(|r| r.label == "raw"));
    let cache: Vec<_> = rows.iter().filter(|r| r.label == "cache").collect();
    assert_eq!(cache.len(), 1);
    assert_eq!(cache[0].r#fn, "retained.csv");
}

#[test]
fn preview_disabled_output_and_failed_end_hook_preserve_prior_csv_and_version() {
    for mode in ["build-dev", "disabled", "failed"] {
        let temporary = tempfile::tempdir().unwrap(); let root = temporary.path();
        fixture(root, "");
        let prior = b"\"label\",\"fn\",\"version\",\"hash\"\n\"raw\",\"\",\"v0.1.0\",\"\"\n";
        fs::write(root.join("manifest.csv"), prior).unwrap();
        let config = fs::read_to_string(root.join("_proj.yml")).unwrap();
        if mode == "disabled" {
            fs::write(root.join("_proj.yml"), config.replace("build:\n", "build:\n  output_run: false\n")).unwrap();
        } else if mode == "failed" {
            fs::write(root.join("end.py"), "import csv, os\nfrom pathlib import Path\nroot = Path(os.environ['PROJ_PROJECT_ROOT'])\nwith (root / 'manifest.csv').open(encoding='utf-8', newline='') as stream:\n    rows = list(csv.DictReader(stream))\nassert any(row['version'] == 'v1.2.4' and row['label'] == 'raw' and row['fn'] == 'a.csv' for row in rows)\n(root / 'cache/end-ran').write_bytes(b'current release CSV observed')\nraise SystemExit(17)\n").unwrap();
            fs::write(root.join("_proj.yml"), config.replace("build:\n", "build:\n  hooks:\n    post: [end.py]\n")).unwrap();
        }
        let output = invoke(root, if mode == "build-dev" { "build-dev" } else { "build" });
        if mode == "failed" {
            assert!(!output.status.success());
            assert_eq!(fs::read(root.join("cache/end-ran")).unwrap(), b"current release CSV observed");
        }
        else { success(output); }
        assert_eq!(fs::read(root.join("manifest.csv")).unwrap(), prior);
        if mode != "disabled" {
            assert_eq!(fs::read(root.join("VERSION")).unwrap(), b"v1.2.3-4\n");
        }
    }
}

#[test]
fn bad_history_and_invalid_layer_hash_fail_before_start_hooks_and_missing_manifest_rolls_back() {
    for kind in ["history", "hash", "absent"] {
        let temporary = tempfile::tempdir().unwrap(); let root = temporary.path();
        fixture(root, "");
        fs::write(root.join("start.py"), "from pathlib import Path\nPath('start-ran').write_text('yes')\n").unwrap();
        fs::write(root.join("end.py"), "import csv, os\nfrom pathlib import Path\nroot = Path(os.environ['PROJ_PROJECT_ROOT'])\nwith (root / 'manifest.csv').open(encoding='utf-8', newline='') as stream:\n    rows = list(csv.DictReader(stream))\nassert any(row['version'] == 'v1.2.4' and row['label'] == 'raw' and row['fn'] == 'a.csv' for row in rows)\n(root / 'cache/end-ran').write_bytes(b'current release CSV observed')\nraise SystemExit(17)\n").unwrap();
        let config = fs::read_to_string(root.join("_proj.yml")).unwrap()
            .replace("build:\n", if kind == "absent" { "build:\n  hooks:\n    post: [end.py]\n" } else { "build:\n  hooks:\n    pre: [start.py]\n" });
        fs::write(root.join("_proj.yml"), config).unwrap();
        if kind == "history" { fs::write(root.join("manifest.csv"), b"not,a,manifest\n").unwrap(); }
        if kind == "hash" { fs::write(root.join("_projr-local.yml"), "directories:\n  raw: {hash: null}\n").unwrap(); }
        let output = invoke(root, "build");
        assert!(!output.status.success());
        if kind == "absent" {
            assert_eq!(fs::read(root.join("cache/end-ran")).unwrap(), b"current release CSV observed");
        }
        assert!(!root.join("start-ran").exists());
        if kind == "history" { assert_eq!(fs::read(root.join("manifest.csv")).unwrap(), b"not,a,manifest\n"); }
        else { assert!(!root.join("manifest.csv").exists()); }
        assert_eq!(fs::read(root.join("VERSION")).unwrap(), b"v1.2.3-4\n");
    }
}

#[test]
fn standalone_csv_preserves_quoted_names_exact_rows_and_hardlink_aliases() {
    let temporary = tempfile::tempdir().unwrap(); let root = temporary.path();
    let source = root.join("input"); fs::create_dir(&source).unwrap();
    fs::write(source.join("donn\u{00e9}es,\u{6570}\u{636e}.csv"), b"abc").unwrap();
    fs::create_dir(source.join("empty")).unwrap();
    let rows = manifest::capture_directory("raw", path(&source), "v1.0.0", false).unwrap();
    let csv = root.join("manifest.csv");
    manifest::write(path(&csv), &rows).unwrap();
    assert_eq!(manifest::read(path(&csv)).unwrap(), rows);
    let alias = root.join("alias.csv");
    fs::hard_link(&csv, &alias).unwrap();
    let prior = fs::read(&alias).unwrap();
    manifest::write(path(&csv), &[]).unwrap();
    assert_eq!(fs::read(&alias).unwrap(), prior);
    assert!(manifest::read(path(&csv)).unwrap().is_empty());
    // These different tuples have identical concatenated strings (legacy key bug).
    let row = |label: &str, name: &str| ManifestRow { label: label.into(), r#fn: name.into(), version: "v1.0.0".into(), hash: "900150983cd24fb0d6963f7d28e17f72".into() };
    assert_eq!(manifest::deduplicate([row("raw", "ab"), row("rawa", "b"), row("raw", "ab")]).len(), 2);
}

#[cfg(unix)]
#[test]
fn links_and_special_files_fail_without_modifying_history() {
    use std::os::unix::fs::symlink;
    let temporary = tempfile::tempdir().unwrap(); let root = temporary.path();
    let source = root.join("input"); fs::create_dir(&source).unwrap();
    fs::write(root.join("outside"), b"unchanged").unwrap();
    symlink(root.join("outside"), source.join("link")).unwrap();
    assert!(manifest::capture_directory("raw", path(&source), "v1.0.0", false).is_err());
    let csv = root.join("manifest.csv");
    symlink(root.join("outside"), &csv).unwrap();
    assert!(manifest::read(path(&csv)).is_err());
    assert!(manifest::write(path(&csv), &[]).is_err());
    assert_eq!(fs::read(root.join("outside")).unwrap(), b"unchanged");
    fs::remove_file(source.join("link")).unwrap();
    let status = Command::new("mkfifo").arg(source.join("pipe")).status().unwrap();
    assert!(status.success());
    assert!(manifest::capture_directory("raw", path(&source), "v1.0.0", false).is_err());
}

#[test]
fn cache_free_empty_selection_still_records_explicit_inputs() {
    let temporary = tempfile::tempdir().unwrap(); let root = temporary.path();
    fs::write(root.join("_proj.yml"), "directories:\n  raw: {path: input}\nbuild:\n  scripts: []\n  git: false\n").unwrap();
    fs::write(root.join("VERSION"), "v1.2.3-4\n").unwrap();
    fs::create_dir(root.join("input")).unwrap();
    success(invoke(root, "build"));
    let rows = manifest::read(path(&root.join("manifest.csv"))).unwrap();
    assert_eq!(rows, vec![ManifestRow { label: "raw".into(), r#fn: "".into(), version: "v1.2.4".into(), hash: "".into() }]);
    assert!(!root.join("cache").exists());
}
