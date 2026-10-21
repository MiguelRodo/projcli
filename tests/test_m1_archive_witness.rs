//! Validates actual remote M1 archive witnesses with the existing CAS checker.
use std::{collections::{BTreeMap, BTreeSet}, fs};
use anyhow::{Context, Result, ensure};
use camino::{Utf8Path, Utf8PathBuf};

fn validate_case(root: &Utf8Path) -> Result<()> {
    let csv = fs::read(root.join("results.csv"))?;
    ensure!(csv == b"result\n84\n", "Unexpected computed CSV");
    ensure!(fs::read(root.join("final-results.csv"))? == csv, "Final CSV differs from cache");
    ensure!(fs::read(root.join("final-report.html"))? == fs::read(root.join("report.html"))?, "Final report differs from cache");
    ensure!(fs::read(root.join("final-version.txt"))? == b"Version: v1.2.4.1", "Wrong next-dev version");
    let config: proj::ProjConfig = serde_yaml::from_str("directories:\n  output: {path: final-output}\n  docs: {path: final-docs}\nremotes:\n  local:\n    archive:\n      path: archive\n      structure: cas\n      inspect: file\n      content: [output, docs]\n")?;
    let validated = config.validate_and_resolve(root, false)?;
    proj::verify_remote_integrity("archive", &validated)?;
    let remote = root.join("archive");
    let mut manifests = BTreeMap::new();
    for entry in fs::read_dir(remote.join("manifests"))? {
        let path = Utf8PathBuf::try_from(entry?.path())?;
        if path.extension() != Some("json") { continue; }
        let manifest: proj::DirectoryManifest = serde_json::from_slice(&fs::read(&path)?)?;
        ensure!(manifest.version == "1.2.4", "Wrong archive release version");
        let digest = path.file_stem().context("Missing manifest digest")?.to_owned();
        ensure!(manifests.insert(manifest.label.clone(), (digest, manifest)).is_none(), "Duplicate role manifest");
    }
    ensure!(manifests.keys().map(String::as_str).collect::<Vec<_>>() == ["docs","output"], "Expected exactly docs/output manifests");
    for (label, logical, witness) in [("output","results.csv","results.csv"),("docs","03-report.html","report.html")] {
        let manifest = &manifests[label].1;
        let entry = manifest.files.iter().find(|entry| entry.path == logical).context("Missing expected archived result")?;
        ensure!(entry.hash == proj::hash_file(&root.join(witness))?, "Archived result hash differs from witness");
        let object = remote.join("objects").join(&entry.hash[..2]).join(&entry.hash[2..]);
        ensure!(fs::read(object)? == fs::read(root.join(witness))?, "Archived object bytes differ from witness");
    }
    let index = fs::read_to_string(root.join("manifests.csv"))?;
    let mut indexed = BTreeSet::new();
    for line in index.lines() {
        // Fixture labels/timestamps contain no CSV quoting characters.
        let fields: Vec<_> = line.split(',').collect();
        ensure!(fields.len() == 4 && fields[0] == "1.2.4" && !fields[3].is_empty(), "Wrong archive index record");
        let expected = manifests.get(fields[1]).context("Unknown index role")?;
        ensure!(fields[2] == expected.0 && indexed.insert(fields[1]), "Wrong or duplicate indexed manifest");
    }
    ensure!(indexed == BTreeSet::from(["docs","output"]), "Missing archive index role");
    Ok(())
}

#[test]
#[ignore = "requires artifacts from the remote actual M1 CLI driver"]
fn actual_final_and_archive_witnesses_reject_corruption() -> Result<()> {
    let evidence = Utf8PathBuf::try_from(std::env::var_os("PROJCLI_M1_EVIDENCE").context("Missing remote evidence path")?)?;
    let evidence = Utf8PathBuf::try_from(evidence.canonicalize()?)?;
    for name in ["manual-no-init-build","initialized-build"] {
        let source = evidence.join(name);
        validate_case(&source)?;
        let temporary = tempfile::tempdir()?;
        let physical = Utf8PathBuf::try_from(temporary.path().canonicalize()?)?;
        let copied = physical.join("case");
        proj::fs_utils::copy_dir_recursive(&source, &copied)?;
        for bad in [&b""[..], &b"result\n999\n"[..]] {
            fs::write(copied.join("final-results.csv"), bad)?;
            ensure!(validate_case(&copied).is_err(), "Corrupt final CSV passed");
        }
        fs::copy(source.join("final-results.csv"), copied.join("final-results.csv"))?;
        let manifest_path = fs::read_dir(copied.join("archive/manifests"))?.next().context("Missing manifest")??.path();
        let original = fs::read(&manifest_path)?;
        let mut manifest: proj::DirectoryManifest = serde_json::from_slice(&original)?;
        let entry = manifest.files.first().context("Expected archived file")?;
        let object = copied.join("archive/objects").join(&entry.hash[..2]).join(&entry.hash[2..]);
        let object_bytes = fs::read(&object)?;
        fs::remove_file(&object)?;
        ensure!(validate_case(&copied).is_err(), "Missing archived object passed");
        fs::write(&object, b"corrupt object")?;
        ensure!(validate_case(&copied).is_err(), "Corrupt archived object passed");
        fs::write(&object, object_bytes)?;
        manifest.label = "wrong-role".into();
        let wrong_bytes = serde_json::to_vec(&manifest)?;
        let wrong_path = copied.join("archive/manifests").join(format!("{}.json", blake3::hash(&wrong_bytes).to_hex()));
        fs::remove_file(&manifest_path)?;
        fs::write(&wrong_path, wrong_bytes)?;
        ensure!(validate_case(&copied).is_err(), "Wrong-role manifest passed");
        fs::remove_file(&wrong_path)?;
        fs::write(&manifest_path, original)?;
        fs::write(copied.join("final-version.txt"), b"Version: v9.9.9")?;
        ensure!(validate_case(&copied).is_err(), "Wrong next-dev version passed");
        fs::copy(source.join("final-version.txt"), copied.join("final-version.txt"))?;
        fs::write(copied.join("manifests.csv"), "9.9.9,output,wrong,now\n")?;
        ensure!(validate_case(&copied).is_err(), "Wrong release index passed");
    }
    fs::write(evidence.join("archive-validation.json"),
        br#"{"complete":true,"validated_production_cases":2,"negative_probes":["empty_final_csv","corrupt_final_csv","missing_object","corrupt_object","wrong_role_manifest","wrong_next_dev_version","wrong_release_index"]}"#)?;
    Ok(())
}
