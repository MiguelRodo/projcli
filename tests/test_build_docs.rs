use std::fs;
use tempfile::tempdir;

use proj::build::{copy_individual_rmd, copy_individual_quarto};
use proj::fs_utils::dir_move_exact;

#[test]
fn test_dir_move_exact_protects_files() {
    let root = tempdir().unwrap();
    let src = root.path().canonicalize().unwrap().join("src");
    let dest = root.path().canonicalize().unwrap().join("dest");

    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dest).unwrap();

    fs::write(src.join("new_file.txt"), "new").unwrap();
    fs::write(dest.join("old_file.txt"), "old").unwrap();
    fs::write(dest.join("CHANGELOG.md"), "changelog").unwrap();
    fs::write(dest.join(".gitignore"), "ignore").unwrap();

    dir_move_exact(&camino::Utf8PathBuf::try_from(src.clone()).unwrap(), &camino::Utf8PathBuf::try_from(dest.clone()).unwrap()).unwrap();

    assert!(dest.join("new_file.txt").exists());
    assert!(!dest.join("old_file.txt").exists());
    assert!(dest.join("CHANGELOG.md").exists());
    assert!(dest.join(".gitignore").exists());
}

#[test]
fn test_mixed_engine_project() {
    let root = tempdir().unwrap();
    let canonical_root = root.path().canonicalize().unwrap();
    let project_root = canonical_root.as_path();
    let docs_path = project_root.join("docs");

    fs::create_dir_all(&docs_path).unwrap();

    // Setup an Rmd file
    let rmd_content = "---\ntitle: abc\nformat: word_document\n---\nHello";
    let rmd_path = project_root.join("test_rmd.Rmd");
    fs::write(&rmd_path, rmd_content).unwrap();

    // The build process generates these files
    fs::write(project_root.join("test_rmd.docx"), "mock docx").unwrap();
    let rmd_files_dir = project_root.join("test_rmd_files");
    fs::create_dir_all(&rmd_files_dir).unwrap();
    fs::write(rmd_files_dir.join("asset.png"), "mock png").unwrap();

    // Setup a Qmd file
    let qmd_content = "---\ntitle: xyz\noutput-file: custom.html\n---\nHello";
    let qmd_path = project_root.join("test_qmd.qmd");
    fs::write(&qmd_path, qmd_content).unwrap();

    // The build process generates these files
    fs::write(project_root.join("custom.html"), "mock html").unwrap();
    let qmd_files_dir = project_root.join("test_qmd_files");
    fs::create_dir_all(&qmd_files_dir).unwrap();
    fs::write(qmd_files_dir.join("script.js"), "mock js").unwrap();

    // Now run copy routines
    copy_individual_rmd(&camino::Utf8PathBuf::try_from(rmd_path).unwrap(), &camino::Utf8PathBuf::try_from(docs_path.clone()).unwrap(), camino::Utf8Path::from_path(project_root).unwrap()).unwrap();
    copy_individual_quarto(&camino::Utf8PathBuf::try_from(qmd_path).unwrap(), &camino::Utf8PathBuf::try_from(docs_path.clone()).unwrap(), camino::Utf8Path::from_path(project_root).unwrap()).unwrap();

    // Assertions
    assert!(docs_path.join("test_rmd.docx").exists());
    assert!(docs_path.join("test_rmd_files").join("asset.png").exists());
    assert!(docs_path.join("custom.html").exists());
    assert!(docs_path.join("test_qmd_files").join("script.js").exists());

    // Original generated files should be moved (or deleted based on dir_move_exact / rename)
    assert!(!project_root.join("test_rmd.docx").exists());
    assert!(!project_root.join("test_rmd_files").exists());
    assert!(!project_root.join("custom.html").exists());
    assert!(!project_root.join("test_qmd_files").exists());
}
