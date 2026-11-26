//! projr 0.6+ manifest store fixtures, following SATVILab/projr@ab3ce8a
//! tests/testthat/test-manifest-store.R with valid MD5 file hashes.
use camino::{Utf8Path, Utf8PathBuf};
use proj::manifest::ManifestRow;
use proj::manifest_store as store;
use std::fs;

const A: &str = "900150983cd24fb0d6963f7d28e17f72";
const B: &str = "d41d8cd98f00b204e9800998ecf8427e";
const C: &str = "0cc175b9c0f1b6a831c399e269772661";

fn row(label: &str, name: &str, version: &str, hash: &str) -> ManifestRow {
    ManifestRow {
        label: label.into(),
        r#fn: name.into(),
        version: version.into(),
        hash: hash.into(),
    }
}

fn temp() -> (tempfile::TempDir, Utf8PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = Utf8PathBuf::from_path_buf(dir.path().canonicalize().unwrap()).unwrap();
    (dir, path)
}

fn versions(store_dir: &Utf8Path, label: &str) -> (Vec<String>, Vec<String>) {
    let index = store::read_index(store_dir, Some(label)).unwrap();
    (
        index.iter().map(|e| e.version.clone()).collect(),
        index.iter().map(|e| e.directory_version.clone()).collect(),
    )
}

#[test]
fn version_map_reuses_snapshots_and_records_first_versions() {
    let (_dir, root) = temp();
    let s = root.join("manifests");
    let rows = |version: &str, second: &str| {
        vec![
            row("output", "a", version, A),
            row("output", "b", version, second),
        ]
    };
    store::write(&s, &rows("v1.9", B), None, true).unwrap();
    store::write(&s, &rows("v1.10", C), None, true).unwrap();
    let mut reversed = rows("v1.11", B);
    reversed.reverse();
    store::write(&s, &reversed, None, true).unwrap();
    store::write(
        &s,
        &[
            row("raw-data", "a", "v1.11", A),
            row("raw-data", "b", "v1.11", B),
        ],
        None,
        true,
    )
    .unwrap();
    assert_eq!(
        versions(&s, "output"),
        (
            vec!["v1.9".into(), "v1.10".into(), "v1.11".into()],
            vec!["v1.9".into(), "v1.10".into(), "v1.9".into()]
        )
    );
    let snapshots = fs::read_dir(s.join("output"))
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("files-")
        })
        .count();
    assert_eq!(snapshots, 2);
    // Exact projr bytes and SHA-256 identity of the v1.9 snapshot.
    let index = store::read_index(&s, Some("output")).unwrap();
    let bytes = fs::read(s.join(format!("output/files-{}.csv", index[0].snapshot))).unwrap();
    assert_eq!(
        bytes,
        format!("\"fn\",\"hash\"\n\"a\",\"{A}\"\n\"b\",\"{B}\"\n").as_bytes()
    );
    assert_eq!(store::sha256_hex(&bytes), index[0].snapshot);
    assert_eq!(
        store::read(&s, Some("output"), Some("v1.11")).unwrap(),
        rows("v1.11", B)
    );
    assert_eq!(fs::read_to_string(s.join("FORMAT")).unwrap(), "1\n");
    assert_eq!(
        fs::read_to_string(s.join(".gitattributes")).unwrap(),
        "files-*.csv -text\nversions.csv -text\n"
    );
    assert!(s.join(store::LOCK).is_file());
    let index_bytes = fs::read_to_string(s.join("output/versions.csv")).unwrap();
    assert!(index_bytes.starts_with(
        "\"label\",\"version\",\"directory_version\",\"snapshot\"\n\"output\",\"v1.9\",\"v1.9\",\""
    ));

    // A backfill moves the first version; replacing it moves the reference back.
    store::write(&s, &rows("v1.2", B), None, true).unwrap();
    assert_eq!(versions(&s, "output").1, ["v1.2", "v1.2", "v1.10", "v1.2"]);
    store::write(&s, &[row("output", "a", "v1.2", A)], None, true).unwrap();
    assert_eq!(versions(&s, "output").1, ["v1.2", "v1.9", "v1.10", "v1.9"]);
    let fns: Vec<_> = store::read(&s, Some("output"), Some("v1.2"))
        .unwrap()
        .into_iter()
        .map(|r| r.r#fn)
        .collect();
    assert_eq!(fns, ["a"]);
}

#[test]
fn empty_directories_are_header_only_and_labels_are_path_encoded() {
    let (_dir, root) = temp();
    let s = root.join("manifests");
    store::write(
        &s,
        &[row("docs", "", "v2", ""), row("aux", "x", "v2", A)],
        None,
        false,
    )
    .unwrap();
    let index = store::read_index(&s, Some("docs")).unwrap();
    assert_eq!(
        index[0].snapshot,
        "4fd72e2a5d3cc6a18060cae51819f4908d8cbddfe7eb06174728787dacf895eb"
    );
    assert_eq!(
        fs::read(s.join(store::snapshot_path(&index[0]))).unwrap(),
        b"\"fn\",\"hash\"\n"
    );
    assert_eq!(
        store::read(&s, Some("docs"), None).unwrap(),
        [row("docs", "", "v2", "")]
    );
    assert!(s.join("_61ux/versions.csv").is_file());
    assert!(store::read(&s, Some("undefined"), None).unwrap().is_empty());
    assert!(store::read(&s, None, Some("v404")).unwrap().is_empty());
    assert!(
        !s.join(store::LOCK).exists(),
        "unlocked writes create no lock file"
    );
    assert_eq!(
        store::published_files(&s).unwrap()[..2],
        ["FORMAT".to_owned(), "_61ux/versions.csv".to_owned()]
    );
}

#[test]
fn project_imports_legacy_csv_once_and_leaves_it_untouched() {
    let (_dir, root) = temp();
    let legacy = format!(
        "\"label\",\"fn\",\"version\",\"hash\"\n\"raw-data\",\"NA\",\"v1.0\",\"{A}\"\n\"output\",\"001\",\"v1.0\",\"{B}\"\n"
    );
    fs::write(root.join("manifest.csv"), &legacy).unwrap();
    assert_eq!(store::read_project(&root).unwrap().len(), 2);
    store::write_project(&root, &[row("output", "001", "v1.1", B)]).unwrap();
    assert_eq!(
        fs::read_to_string(root.join("manifest.csv")).unwrap(),
        legacy
    );
    let s = store::project_store(&root);
    assert_eq!(versions(&s, "output").0, ["v1.0", "v1.1"]);
    // Once ready, a changed legacy CSV is not imported again.
    fs::write(root.join("manifest.csv"), legacy.replace("v1.0", "v9.9")).unwrap();
    store::write_project(&root, &[row("output", "001", "v1.2", B)]).unwrap();
    assert!(
        !store::read_index(&s, None)
            .unwrap()
            .iter()
            .any(|e| e.version == "v9.9")
    );
    // A missing or foreign snapshot fails closed.
    let raw = store::read_index(&s, Some("raw-data")).unwrap();
    fs::remove_file(s.join(store::snapshot_path(&raw[0]))).unwrap();
    assert_eq!(
        store::read(&s, Some("output"), Some("v1.1")).unwrap()[0].hash,
        B
    );
    let error = store::read(&s, Some("raw-data"), None).unwrap_err();
    assert!(
        format!("{error:#}").contains("Missing manifest snapshot"),
        "{error:#}"
    );
    let foreign = format!(
        "\"label\",\"version\",\"directory_version\",\"snapshot\"\n\"raw-data\",\"v1.0\",\"v1.0\",\"../foreign\"\n"
    );
    fs::write(s.join("raw-data/versions.csv"), foreign).unwrap();
    assert!(store::read_index(&s, None).is_err());
}

#[test]
fn failed_first_import_is_retried_from_the_legacy_csv() {
    let (_dir, root) = temp();
    let prior = format!(
        "\"label\",\"fn\",\"version\",\"hash\"\n\"output\",\"report\",\"v1\",\"{A}\"\n\"abandoned\",\"report\",\"v7\",\"{A}\"\n"
    );
    fs::write(root.join("manifest.csv"), &prior).unwrap();
    let s = store::project_store(&root);
    assert!(store::write_project(&root, &[row("output", "report", "vinvalid", A)]).is_err());
    assert!(!store::ready(&s).unwrap());
    assert_eq!(fs::read_to_string(s.join("FORMAT")).unwrap(), "importing\n");
    assert!(s.join("abandoned/versions.csv").is_file());
    assert_eq!(
        store::read_project(&root).unwrap().len(),
        2,
        "unfinished stores read the legacy CSV"
    );
    fs::write(
        root.join("manifest.csv"),
        format!("\"label\",\"fn\",\"version\",\"hash\"\n\"output\",\"report\",\"v1\",\"{A}\"\n"),
    )
    .unwrap();
    store::write_project(&root, &[row("output", "report", "v2", A)]).unwrap();
    assert_eq!(fs::read_to_string(s.join("FORMAT")).unwrap(), "1\n");
    assert!(!s.join("abandoned/versions.csv").exists());
    let versions: Vec<_> = store::read_project(&root)
        .unwrap()
        .into_iter()
        .map(|r| r.version)
        .collect();
    assert_eq!(versions, ["v1", "v2"]);
}

#[test]
fn existing_indexes_without_format_and_unknown_formats_fail_closed() {
    let (_dir, root) = temp();
    let s = root.join("manifests");
    fs::create_dir_all(s.join("docs")).unwrap();
    let original = b"version,description\r\n1,local history\r\n";
    fs::write(s.join("docs/versions.csv"), original).unwrap();
    let error = store::write(&s, &[row("docs", "report", "v1", A)], None, false).unwrap_err();
    assert!(error.to_string().contains("existing index"), "{error:#}");
    assert_eq!(fs::read(s.join("docs/versions.csv")).unwrap(), original);
    assert!(!s.join("FORMAT").exists());
    fs::write(s.join("FORMAT"), "2\n").unwrap();
    assert!(store::ready(&s).is_err());
}

#[test]
fn existing_gitattributes_lines_are_kept_and_missing_rules_appended() {
    let (_dir, root) = temp();
    let s = root.join("manifests");
    fs::create_dir_all(&s).unwrap();
    fs::write(s.join(".gitattributes"), "*.md text\nfiles-*.csv -text\n").unwrap();
    store::write(&s, &[row("docs", "report.pdf", "v1", A)], None, false).unwrap();
    assert_eq!(
        fs::read_to_string(s.join(".gitattributes")).unwrap(),
        "*.md text\nfiles-*.csv -text\nversions.csv -text\n"
    );
}

#[cfg(unix)]
#[test]
fn linked_store_entries_are_rejected() {
    let (_dir, root) = temp();
    let s = root.join("manifests");
    store::write(&s, &[row("docs", "report", "v1", A)], None, false).unwrap();
    fs::rename(s.join("docs"), root.join("elsewhere")).unwrap();
    std::os::unix::fs::symlink(root.join("elsewhere"), s.join("docs")).unwrap();
    assert!(store::read(&s, None, None).is_err());
    assert!(store::read(&s, Some("docs"), None).is_err());
}
