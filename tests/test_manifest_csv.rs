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

fn record(root:&Path,version:&str)->serde_json::Value {
    let records:Vec<serde_json::Value>=fs::read_dir(root.join(".proj/log")).unwrap()
        .map(|entry|entry.unwrap().path()).filter(|path|path.extension().is_some_and(|value|value=="json"))
        .map(|path|serde_json::from_slice(&fs::read(path).unwrap()).unwrap())
        .filter(|record:&serde_json::Value|record["release_version"]==version).collect();
    assert_eq!(records.len(),1);
    records.into_iter().next().unwrap()
}
fn assert_fingerprint(fingerprint:&serde_json::Value,rows:&[ManifestRow],version:&str) {
    assert_eq!(fingerprint["csv_version"],version);
    assert_eq!(fingerprint["rows"],rows.len());
    assert_eq!(fingerprint["files"],rows.iter().filter(|row|!row.r#fn.is_empty()).count());
    let digest=blake3::hash(&proj::manifest_csv::encode(rows).unwrap()).to_hex().to_string();
    assert_eq!(fingerprint["canonical_csv_blake3"],digest);
}

#[test]
fn production_records_md5_pre_inputs_final_outputs_and_exact_history_without_git_or_remotes() {
    let temporary = tempfile::tempdir().unwrap(); let physical_root = temporary.path().canonicalize().unwrap(); let root = physical_root.as_path();
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
    let first_record=record(root,"v1.2.4");
    assert_eq!(first_record["status"],"completed");
    assert_eq!(first_record["git"]["commit"],"not_requested");
    assert_eq!(first_record["distribution_may_have_started"],false);
    assert_eq!(first_record["version_before"],"v1.2.3.4");
    assert_eq!(first_record["version_after"],"v1.2.4.1");
    assert_fingerprint(&first_record["inventory"]["inputs"],
        &rows.iter().filter(|row|row.label=="raw").cloned().collect::<Vec<_>>(),"v1.2.4");
    assert_fingerprint(&first_record["inventory"]["release_outputs"],
        &rows.iter().filter(|row|row.label!="raw").cloned().collect::<Vec<_>>(),"v1.2.4");
    assert_eq!(first_record["final_outputs"]["status"],"completed");
    assert_eq!(first_record["final_outputs"]["inventory"],first_record["inventory"]["release_outputs"]);
    let pre = manifest::read(path(&root.join("cache/projr/v1.2.4/manifest.csv"))).unwrap();
    assert_eq!(pre, rows.iter().filter(|r| r.label == "raw").cloned().collect::<Vec<_>>());
    assert_eq!(fs::read(root.join("released/result.csv")).unwrap(), b"hello");
    fs::write(root.join("_proj.yml"), config).unwrap();
    success(invoke(root, "build"));
    let history = manifest::read(path(&root.join("manifest.csv"))).unwrap();
    assert_eq!(history.len(), 6);
    assert!(rows.iter().all(|r| history.contains(r)));
    assert!(history.iter().any(|r| r.label == "raw" && r.version == "v1.2.5" && r.hash != "900150983cd24fb0d6963f7d28e17f72"));
    assert_eq!(record(root,"v1.2.4"),first_record,"previous attempt evidence changed");
    assert_fingerprint(&record(root,"v1.2.5")["inventory"]["inputs"],
        &history.iter().filter(|row|row.label=="raw"&&row.version=="v1.2.5").cloned().collect::<Vec<_>>(),"v1.2.5");
    assert_eq!(fs::read_dir(root.join(".proj/log")).unwrap().count(),4);
}

#[test]
fn configured_hash_flags_and_cache_framework_exclusion_match_projr() {
    let temporary = tempfile::tempdir().unwrap(); let physical_root = temporary.path().canonicalize().unwrap(); let root = physical_root.as_path();
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
    let log=record(root,"v1.2.4");
    assert_eq!(log["inventory"]["inputs"]["labels"],serde_json::json!(["cache"]));
    assert_fingerprint(&log["inventory"]["inputs"],&cache.into_iter().cloned().collect::<Vec<_>>(),"v1.2.4");
}

#[test]
fn preview_disabled_output_and_failed_end_hook_preserve_prior_csv_and_version() {
    for mode in ["build-dev", "disabled", "failed"] {
        let temporary = tempfile::tempdir().unwrap(); let physical_root = temporary.path().canonicalize().unwrap(); let root = physical_root.as_path();
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
        if mode=="disabled" {
            let log=record(root,"v1.2.4");
            assert!(log.get("inventory").is_none());
            assert_eq!(log["final_outputs"]["status"],"not_requested");
            assert!(log["final_outputs"]["inventory"].is_null());
        }else{
            assert!(!root.join(".proj/log").exists(),"pre-distribution failure/preview retention changed");
        }
        if mode != "disabled"  {
            assert_eq!(fs::read(root.join("VERSION")).unwrap(), b"v1.2.3-4\n");
        }
    }
}

#[test]
fn bad_history_and_invalid_layer_hash_fail_before_start_hooks_and_missing_manifest_rolls_back() {
    for kind in ["history", "hash", "absent"] {
        let temporary = tempfile::tempdir().unwrap(); let physical_root = temporary.path().canonicalize().unwrap(); let root = physical_root.as_path();
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
    let temporary = tempfile::tempdir().unwrap(); let physical_root = temporary.path().canonicalize().unwrap(); let root = physical_root.as_path();
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
    let temporary = tempfile::tempdir().unwrap(); let physical_root = temporary.path().canonicalize().unwrap(); let root = physical_root.as_path();
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
    let temporary = tempfile::tempdir().unwrap(); let physical_root = temporary.path().canonicalize().unwrap(); let root = physical_root.as_path();
    fs::write(root.join("_proj.yml"), "directories:\n  raw: {path: input}\nbuild:\n  scripts: []\n  git: false\n").unwrap();
    fs::write(root.join("VERSION"), "v1.2.3-4\n").unwrap();
    fs::create_dir(root.join("input")).unwrap();
    success(invoke(root, "build"));
    let rows = manifest::read(path(&root.join("manifest.csv"))).unwrap();
    assert_eq!(rows, vec![ManifestRow { label: "raw".into(), r#fn: "".into(), version: "v1.2.4".into(), hash: "".into() }]);
    assert!(!root.join("cache").exists());
}

#[test]
fn end_hook_output_changes_are_observed_without_enforcement_or_rewriting_publication_inventory() {
    for publish in [false,true] {
        let temporary=tempfile::tempdir().unwrap();let root=temporary.path().canonicalize().unwrap();
        fixture(&root,"");
        fs::write(root.join("released/obsolete.csv"),b"delete").unwrap();
        let config=fs::read_to_string(root.join("_proj.yml")).unwrap();
        let destination=if publish {"  local:\n    fixture:\n      path: remote\n      structure: latest\n      content: [output]\n      send: {inspect: file}\n"}else{""};
        fs::write(root.join("_proj.yml"),format!("{config}  hooks:\n    post: [end.py]\n{destination}")).unwrap();
        fs::write(root.join("end.py"),"from pathlib import Path\nPath('released/result.csv').rename('released/renamed.csv')\nPath('released/obsolete.csv').unlink()\nPath('released/données.csv').write_bytes(b'abc')\nPath('docs').mkdir(exist_ok=True)\nPath('docs/report.html').write_bytes(b'<html>final</html>')\n").unwrap();
        success(invoke(&root,"build"));
        let log=record(&root,"v1.2.4");
        assert_eq!(log["status"],"completed");
        assert_eq!(log["distribution_may_have_started"],publish);
        let rows=manifest::read(path(&root.join("manifest.csv"))).unwrap();
        let release:Vec<_>=rows.iter().filter(|row|row.label!="raw").cloned().collect();
        assert_fingerprint(&log["inventory"]["release_outputs"],&release,"v1.2.4");
        assert!(release.iter().any(|row|row.r#fn=="result.csv"&&row.hash=="5d41402abc4b2a76b9719d911017c592"));
        assert!(release.iter().any(|row|row.r#fn=="obsolete.csv"));
        assert!(!release.iter().any(|row|row.r#fn=="renamed.csv"||row.r#fn=="données.csv"));
        let mut final_rows=manifest::capture_directory("docs",path(&root.join("docs")),"v1.2.4",false).unwrap();
        final_rows.extend(manifest::capture_directory("output",path(&root.join("released")),"v1.2.4",false).unwrap());
        assert_eq!(final_rows.iter().filter(|row|!row.r#fn.is_empty()).count(),3);
        assert!(final_rows.iter().any(|row|row.r#fn=="renamed.csv"&&row.hash=="5d41402abc4b2a76b9719d911017c592"));
        assert!(final_rows.iter().any(|row|row.r#fn=="données.csv"&&row.hash=="900150983cd24fb0d6963f7d28e17f72"));
        assert_fingerprint(&log["final_outputs"]["inventory"],&final_rows,"v1.2.4");
        assert_ne!(log["final_outputs"]["inventory"],log["inventory"]["release_outputs"]);
        let reference=&log["final_outputs"]["rows"];
        let bytes=fs::read(root.join(".proj/log").join(reference["file"].as_str().unwrap())).unwrap();
        assert_eq!(reference["bytes"],bytes.len());
        assert_eq!(reference["blake3"],blake3::hash(&bytes).to_hex().as_str());
        assert_eq!(proj::manifest_csv::decode(&bytes).unwrap(),final_rows);
        let id=log["attempt_id"].as_str().unwrap();
        let history=proj::history::read(path(&root),Some(id),20).unwrap();
        let detail=serde_json::to_value(&history).unwrap();
        assert_eq!(detail["entries"][0]["final_files"]["rows"],serde_json::to_value(&final_rows).unwrap());
        let human=history.markdown();
        assert!(human.contains("renamed&#46;csv")&&human.contains("données&#46;csv"));
        assert!(!human.contains("obsolete&#46;csv")&&!human.contains("result&#46;csv"));
        assert_eq!(fs::read(root.join("released/renamed.csv")).unwrap(),b"hello");
        assert!(!root.join("released/result.csv").exists());
        if publish {
            assert_eq!(fs::read(root.join("remote/output/result.csv")).unwrap(),b"hello");
            assert!(!root.join("remote/output/renamed.csv").exists());
        }
    }
}

#[test]
fn plain_record_destination_failure_restores_exact_state_without_fabricating_success() {
    let temporary=tempfile::tempdir().unwrap();let root=temporary.path().canonicalize().unwrap();
    fixture(&root,"");
    fs::write(root.join("VERSION"),b"v1.2.3-19\r\n").unwrap();
    fs::create_dir(root.join(".proj")).unwrap();
    fs::write(root.join(".proj/log"),b"preserved non-directory").unwrap();
    let version=fs::read(root.join("VERSION")).unwrap();
    let output=invoke(&root,"build");
    assert!(!output.status.success());
    assert_eq!(fs::read(root.join("VERSION")).unwrap(),version);
    assert_eq!(fs::read(root.join("released/result.csv")).unwrap(),b"hello");
    assert!(!root.join("manifest.csv").exists());
    assert_eq!(fs::read(root.join(".proj/log")).unwrap(),b"preserved non-directory");
    assert!(!root.join(".proj/build.lock").exists());
}

#[test]
fn plain_record_install_failure_removes_only_its_new_csv_and_restores_exact_version() {
    let temporary=tempfile::tempdir().unwrap();let root=temporary.path().canonicalize().unwrap();
    fixture(&root,"");
    let config=fs::read_to_string(root.join("_proj.yml")).unwrap();
    fs::write(root.join("_proj.yml"),format!("{config}  hooks:\n    post: [block.py]\n")).unwrap();
    fs::write(root.join("block.py"),"import json\nfrom pathlib import Path\nidentity=json.loads(Path('.proj/build.lock').read_bytes())['attempt_id']\nlogs=Path('.proj/log')\nlogs.mkdir(parents=True,exist_ok=True)\n(logs / (identity+'.json')).mkdir()\n(logs / 'unrelated.txt').write_bytes(b'preserved')\n").unwrap();
    let before=fs::read(root.join("VERSION")).unwrap();
    let result=invoke(&root,"build");assert!(!result.status.success());
    assert_eq!(fs::read(root.join("VERSION")).unwrap(),before);
    assert_eq!(fs::read(root.join("released/result.csv")).unwrap(),b"hello");
    assert!(!root.join("manifest.csv").exists());
    let paths:Vec<_>=fs::read_dir(root.join(".proj/log")).unwrap().map(|entry|entry.unwrap().path()).collect();
    assert_eq!(paths.len(),2);assert!(paths.iter().all(|path|path.is_dir()||path.file_name().unwrap()=="unrelated.txt"));
    assert_eq!(fs::read(root.join(".proj/log/unrelated.txt")).unwrap(),b"preserved");
    assert!(!root.join(".proj/build.lock").exists());
}
