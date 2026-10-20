use std::{collections::HashMap, fs};
use camino::{Utf8Path, Utf8PathBuf};
use proj::{DirectoryManifest, FileEntry, LocalRemoteConfig, RemotesConfig, StorageStructure, InspectStrategy, ValidatedConfig};

fn root(temp: &tempfile::TempDir) -> Utf8PathBuf { Utf8PathBuf::try_from(temp.path().canonicalize().unwrap()).unwrap() }
fn configuration(remote: &Utf8Path, inspect: InspectStrategy) -> ValidatedConfig {
    ValidatedConfig {
        remotes: RemotesConfig { local: Some(HashMap::from([("archive".into(), LocalRemoteConfig { path: remote.to_owned(), structure: StorageStructure::Cas, content: vec!["output".into()], inspect })])) },
        dest: vec![], config: Default::default(), directories: HashMap::new(), git: proj::ResolvedGitConfig { commit: false, push: false },
        restrictions: Default::default(), clear_output: None, output_run: None, old_dev_remove: None,
        parameters: serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
    }
}
fn write_manifest(remote: &Utf8Path, manifest: &DirectoryManifest) -> Utf8PathBuf {
    fs::create_dir_all(remote.join("manifests")).unwrap();
    let bytes = serde_json::to_vec(manifest).unwrap();
    let path = remote.join("manifests").join(format!("{}.json", blake3::hash(&bytes).to_hex()));
    fs::write(&path, bytes).unwrap(); path
}

#[test]
fn missing_file_and_overlapping_sources_fail_without_publication_or_source_changes() {
    let directory = tempfile::tempdir().unwrap(); let project = root(&directory);
    fs::write(project.join("source-file"), "source bytes").unwrap();
    for source in [project.join("missing"), project.join("source-file")] {
        let remote = project.join("archive");
        assert!(proj::ingest_directory(&project, &remote, "output", &source, "1.0.0").is_err());
        assert!(!remote.exists()); assert!(!project.join(".projr").exists());
    }
    let source = project.join("source"); fs::create_dir(&source).unwrap(); fs::write(source.join("keep"), "original").unwrap();
    assert!(proj::ingest_directory(&project, &source.join("archive"), "output", &source, "1.0.0").is_err());
    assert_eq!(fs::read(source.join("keep")).unwrap(), b"original");
    assert!(!source.join("archive").exists());
}

#[test]
fn malformed_digests_paths_duplicates_and_manifest_bytes_are_rejected_by_both_modes() {
    for digest in ["".to_owned(), "a".into(), "é".repeat(32), "../outside".into(), "G".repeat(64), "a".repeat(63)] {
        let directory = tempfile::tempdir().unwrap(); let remote = root(&directory);
        let manifest = DirectoryManifest { label: "output".into(), version: "1.0.0".into(), files: vec![FileEntry { path: "file.txt".into(), hash: digest }] };
        write_manifest(&remote, &manifest);
        for inspect in [InspectStrategy::Manifest, InspectStrategy::File] { assert!(proj::verify_remote_integrity("archive", &configuration(&remote, inspect)).is_err()); }
    }
    for path in ["../outside", "/absolute", "a\\b", "a//b", "C:drive", "./file", ""] {
        let directory = tempfile::tempdir().unwrap(); let remote = root(&directory);
        let manifest = DirectoryManifest { label: "output".into(), version: "1.0.0".into(), files: vec![FileEntry { path: path.into(), hash: "a".repeat(64) }] };
        write_manifest(&remote, &manifest);
        assert!(proj::verify_remote_integrity("archive", &configuration(&remote, InspectStrategy::Manifest)).is_err());
    }
    let directory = tempfile::tempdir().unwrap(); let remote = root(&directory);
    let entry = FileEntry { path: "same".into(), hash: "a".repeat(64) };
    let path = write_manifest(&remote, &DirectoryManifest { label: "output".into(), version: "1.0.0".into(), files: vec![entry.clone(), entry] });
    assert!(proj::verify_remote_integrity("archive", &configuration(&remote, InspectStrategy::Manifest)).is_err());
    fs::write(path, b"{}").unwrap();
    assert!(proj::verify_remote_integrity("archive", &configuration(&remote, InspectStrategy::Manifest)).is_err());
}

#[test]
fn existing_corrupt_object_is_neither_trusted_nor_overwritten_and_file_inspection_hashes_bytes() {
    let directory = tempfile::tempdir().unwrap(); let project = root(&directory);
    let source = project.join("source"); let remote = project.join("archive");
    fs::create_dir(&source).unwrap(); fs::write(source.join("result"), b"verified-source").unwrap();
    proj::ingest_directory(&project, &remote, "output", &source, "1.0.0").unwrap();
    let digest = blake3::hash(b"verified-source").to_hex().to_string();
    let object = remote.join("objects").join(&digest[..2]).join(&digest[2..]);
    fs::write(&object, "corrupt existing object").unwrap();
    let index = fs::read(project.join(".projr/manifests.csv")).unwrap();
    let manifest_count = fs::read_dir(remote.join("manifests")).unwrap().count();
    assert!(proj::ingest_directory(&project, &remote, "output", &source, "2.0.0").is_err());
    assert_eq!(fs::read(&object).unwrap(), b"corrupt existing object");
    assert_eq!(fs::read(source.join("result")).unwrap(), b"verified-source");
    assert_eq!(fs::read(project.join(".projr/manifests.csv")).unwrap(), index);
    assert_eq!(fs::read_dir(remote.join("manifests")).unwrap().count(), manifest_count);
    assert!(proj::verify_remote_integrity("archive", &configuration(&remote, InspectStrategy::File)).is_err());
}

#[cfg(unix)]
#[test]
fn literal_backslash_cannot_alias_a_nested_path_and_linked_sources_or_objects_are_rejected() {
    let directory = tempfile::tempdir().unwrap(); let project = root(&directory);
    let source = project.join("source"); let remote = project.join("archive");
    fs::create_dir_all(source.join("a")).unwrap();
    fs::write(source.join("a/b"), b"nested").unwrap(); fs::write(source.join("a\\b"), b"literal").unwrap();
    assert!(proj::ingest_directory(&project, &remote, "output", &source, "1.0.0").is_err());
    assert!(!remote.exists()); assert_eq!(fs::read(source.join("a\\b")).unwrap(), b"literal");
    fs::remove_file(source.join("a\\b")).unwrap();
    let external = tempfile::tempdir().unwrap(); fs::write(external.path().join("keep"), "external").unwrap();
    std::os::unix::fs::symlink(external.path().join("keep"), source.join("linked")).unwrap();
    assert!(proj::ingest_directory(&project, &remote, "output", &source, "1.0.0").is_err());
    assert!(!remote.exists()); assert_eq!(fs::read(external.path().join("keep")).unwrap(), b"external");
}

#[test]
fn conflicting_manifest_and_unwritable_index_targets_preserve_old_state() {
    let directory = tempfile::tempdir().unwrap(); let project = root(&directory);
    let source = project.join("source"); let remote = project.join("archive");
    fs::create_dir(&source).unwrap(); fs::write(source.join("result"), b"actual").unwrap();
    fs::create_dir(project.join(".projr")).unwrap(); fs::write(project.join(".projr/manifests.csv"), "old-index\n").unwrap();
    let manifest = DirectoryManifest { label: "output".into(), version: "1.0.0".into(), files: vec![FileEntry { path: "result".into(), hash: blake3::hash(b"actual").to_hex().to_string() }] };
    let target = write_manifest(&remote, &manifest); fs::write(&target, "old-conflicting-bytes").unwrap();
    assert!(proj::ingest_directory(&project, &remote, "output", &source, "1.0.0").is_err());
    assert_eq!(fs::read(&target).unwrap(), b"old-conflicting-bytes");
    assert_eq!(fs::read(project.join(".projr/manifests.csv")).unwrap(), b"old-index\n");
    fs::remove_file(project.join(".projr/manifests.csv")).unwrap(); fs::create_dir(project.join(".projr/manifests.csv")).unwrap();
    fs::write(project.join(".projr/manifests.csv/keep"), "old-index-directory").unwrap();
    assert!(proj::ingest_directory(&project, &remote, "output", &source, "2.0.0").is_err());
    assert_eq!(fs::read(project.join(".projr/manifests.csv/keep")).unwrap(), b"old-index-directory");
    assert_eq!(fs::read(source.join("result")).unwrap(), b"actual");
}

#[test]
fn index_rows_escape_metadata_and_every_published_object_matches_its_hash() {
    let directory = tempfile::tempdir().unwrap(); let project = root(&directory);
    let source = project.join("source"); let remote = project.join("archive");
    fs::create_dir(&source).unwrap(); fs::write(source.join("result"), b"actual-content").unwrap();
    proj::ingest_directory(&project, &remote, "raw\"quoted", &source, "1,2").unwrap();
    let index = fs::read_to_string(project.join(".projr/manifests.csv")).unwrap();
    assert!(index.starts_with("\"1,2\",\"raw\"\"quoted\","));
    proj::verify_remote_integrity("archive", &configuration(&remote, InspectStrategy::File)).unwrap();
    assert_eq!(fs::read(source.join("result")).unwrap(), b"actual-content");
}


#[cfg(unix)]
#[test]
fn index_write_failure_keeps_old_index_and_only_complete_remote_metadata() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap(); let project = root(&directory);
    let source = project.join("source"); let remote = project.join("archive");
    fs::create_dir(&source).unwrap(); fs::write(source.join("result"), "unchanged source").unwrap();
    proj::ingest_directory(&project, &remote, "output", &source, "1.0.0").unwrap();
    let index = project.join(".projr/manifests.csv"); let before = fs::read(&index).unwrap();
    let directory = project.join(".projr"); let permissions = fs::metadata(&directory).unwrap().permissions();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o555)).unwrap();
    let result = proj::ingest_directory(&project, &remote, "output", &source, "2.0.0");
    fs::set_permissions(&directory, permissions).unwrap();
    assert!(result.is_err());
    assert_eq!(fs::read(&index).unwrap(), before);
    assert_eq!(fs::read(source.join("result")).unwrap(), b"unchanged source");
    // A failed index may leave a complete unindexed manifest, never truncated JSON.
    proj::verify_remote_integrity("archive", &configuration(&remote, InspectStrategy::File)).unwrap();
}


#[test]
fn impossible_file_directory_layouts_fail_in_both_inspection_modes_regardless_of_order() {
    for paths in [vec!["a", "a/b"], vec!["a/b", "a"], vec!["a", "a-b", "a/b"]] {
        let directory = tempfile::tempdir().unwrap(); let remote = root(&directory);
        let digest = blake3::hash(b"actual object").to_hex().to_string();
        let object = remote.join("objects").join(&digest[..2]).join(&digest[2..]);
        fs::create_dir_all(object.parent().unwrap()).unwrap(); fs::write(object, b"actual object").unwrap();
        let manifest = DirectoryManifest { label: "output".into(), version: "1.0.0".into(), files: paths.into_iter().map(|path| FileEntry { path: path.to_owned(), hash: digest.clone() }).collect() };
        write_manifest(&remote, &manifest);
        for inspect in [InspectStrategy::Manifest, InspectStrategy::File] {
            let error = proj::verify_remote_integrity("archive", &configuration(&remote, inspect)).unwrap_err();
            assert!(format!("{error:#}").contains("file/directory path collision"));
        }
    }
}
