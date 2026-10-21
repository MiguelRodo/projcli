use proj::cas::{ingest_directory, verify_remote_integrity};
use proj::yml::{ValidatedConfig, RemotesConfig, LocalRemoteConfig, StorageStructure, InspectStrategy};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;

#[test]
fn test_cas_integration_deduplication() {
    let root = tempdir().unwrap();
    let physical_root = root.path().canonicalize().unwrap();
    let project_root = physical_root.join("proj");
    let cas_remote_root = physical_root.join("cas");

    fs::create_dir_all(camino::Utf8Path::from_path(&project_root).unwrap()).unwrap();
    fs::create_dir_all(camino::Utf8Path::from_path(&cas_remote_root).unwrap()).unwrap();

    let source_dir = project_root.join("data").join("raw");
    fs::create_dir_all(camino::Utf8Path::from_path(&source_dir).unwrap()).unwrap();

    // Create initial files
    fs::write(source_dir.join("file1.txt"), "content A").unwrap();
    fs::write(source_dir.join("file2.txt"), "content B").unwrap();

    // Pass 1: Ingest version 1.0.0
    ingest_directory(
        camino::Utf8Path::from_path(&project_root).unwrap(),
        camino::Utf8Path::from_path(&cas_remote_root).unwrap(),
        "raw",
        camino::Utf8Path::from_path(&source_dir).unwrap(),
        "1.0.0"
    ).unwrap();

    // Check objects were created
    let objects_dir = cas_remote_root.join("objects");
    let mut object_count_v1 = 0;
    for entry in walkdir(&objects_dir) {
        if entry.is_file() {
            object_count_v1 += 1;
        }
    }
    assert_eq!(object_count_v1, 2, "Should have 2 objects after first ingestion");

    // Pass 2: Ingest version 2.0.0 (duplicate files)
    ingest_directory(
        camino::Utf8Path::from_path(&project_root).unwrap(),
        camino::Utf8Path::from_path(&cas_remote_root).unwrap(),
        "raw",
        camino::Utf8Path::from_path(&source_dir).unwrap(),
        "2.0.0"
    ).unwrap();

    // Check object count hasn't changed
    let mut object_count_v2 = 0;
    for entry in walkdir(&objects_dir) {
        if entry.is_file() {
            object_count_v2 += 1;
        }
    }
    assert_eq!(object_count_v2, 2, "Duplicate files should be skipped, still 2 objects");

    // Pass 3: Modify a file and ingest version 3.0.0
    fs::write(source_dir.join("file2.txt"), "content B modified").unwrap();

    ingest_directory(
        camino::Utf8Path::from_path(&project_root).unwrap(),
        camino::Utf8Path::from_path(&cas_remote_root).unwrap(),
        "raw",
        camino::Utf8Path::from_path(&source_dir).unwrap(),
        "3.0.0"
    ).unwrap();

    let mut object_count_v3 = 0;
    for entry in walkdir(&objects_dir) {
        if entry.is_file() {
            object_count_v3 += 1;
        }
    }
    assert_eq!(object_count_v3, 3, "Modified file should result in 1 new object");

    // Verify manifests were created
    let manifests_dir = cas_remote_root.join("manifests");
    let mut manifest_count = 0;
    for entry in walkdir(&manifests_dir) {
        if entry.is_file() && entry.extension().unwrap_or_default() == "json" {
            manifest_count += 1;
        }
    }
    // versions 1.0.0 and 2.0.0 have the SAME file contents, BUT the version string in the DirectoryManifest differs.
    // Therefore, the JSON payload differs, the BLAKE3 hash differs, so 3 manifest files are created!
    assert_eq!(manifest_count, 3, "Should have 3 unique manifest files");

    // Verify ledger has 3 entries
    let ledger_path = project_root.join(".projr").join("manifests.csv");
    let ledger_content = fs::read_to_string(&ledger_path).unwrap();
    let lines: Vec<&str> = ledger_content.lines().collect();
    assert_eq!(lines.len(), 3, "Ledger should have 3 lines appended");
}

fn walkdir(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if dir.is_dir() {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                paths.extend(walkdir(&path));
            } else {
                paths.push(path);
            }
        }
    }
    paths
}

#[test]
fn test_verify_remote_integrity() {
    let root = tempdir().unwrap();
    let physical_root = root.path().canonicalize().unwrap();
    let cas_remote_root = physical_root.join("cas");
    let project_root = physical_root.join("proj");
    fs::create_dir_all(camino::Utf8Path::from_path(&project_root).unwrap()).unwrap();
    fs::create_dir_all(camino::Utf8Path::from_path(&cas_remote_root).unwrap()).unwrap();
    let source_dir = project_root.join("data").join("raw");
    fs::create_dir_all(camino::Utf8Path::from_path(&source_dir).unwrap()).unwrap();
    fs::write(source_dir.join("file1.txt"), "content A").unwrap();

    ingest_directory(
        camino::Utf8Path::from_path(&project_root).unwrap(),
        camino::Utf8Path::from_path(&cas_remote_root).unwrap(),
        "raw",
        camino::Utf8Path::from_path(&source_dir).unwrap(),
        "1.0.0"
    ).unwrap();

    let mut remotes = HashMap::new();
    remotes.insert("my_remote".to_string(), LocalRemoteConfig {
        path: camino::Utf8PathBuf::from_path_buf(cas_remote_root.clone()).unwrap(),
        structure: StorageStructure::Cas,
        content: vec!["raw".to_string()],
        inspect: InspectStrategy::Manifest,
    });

    let config = ValidatedConfig {
        remotes: RemotesConfig { local: Some(remotes.clone()) },
        dest: vec![],
        config: Default::default(), settings: Default::default(),
        directories: HashMap::new(),
        git: proj::yml::ResolvedGitConfig { commit: false, push: false },
        restrictions: Default::default(),
        clear_output: None,
        output_run: None,
        old_dev_remove: None,
        parameters: serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
    };

    assert!(verify_remote_integrity("my_remote", &config).is_ok());

    let mut config_file = config;
    remotes.get_mut("my_remote").unwrap().inspect = InspectStrategy::File;
    config_file.remotes.local = Some(remotes);
    assert!(verify_remote_integrity("my_remote", &config_file).is_ok());
}
