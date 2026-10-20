use std::{fs, process::Command};

fn configuration(docs: &str) -> String {
    format!("directories:\n  cache:\n    path: _tmp\n  raw:\n    path: _raw_data\n  output:\n    path: _output\n  docs:\n    path: '{docs}'\nconfig:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: [analysis.py]\n  git: false\n  clear_output: post\n  restrictions:\n    not_behind: false\n")
}

fn failed_build_preserves(root: &std::path::Path, configuration: &str) {
    fs::write(root.join("_proj.yml"), configuration).unwrap();
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    fs::write(root.join("analysis.py"), "from pathlib import Path\nPath('analysis-ran').touch()\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build", "--patch", "--non-interactive", "--project"]).arg(root).current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
    assert!(!output.status.success(), "unsafe destination accepted");
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
    assert_eq!(fs::read_to_string(root.join("_proj.yml")).unwrap(), configuration);
    assert!(root.join("analysis.py").is_file());
    for name in ["analysis-ran", ".gitignore", ".Rbuildignore", "_tmp"] { assert!(!root.join(name).exists(), "unexpected {name}"); }
}

#[test]
fn destructive_root_ancestor_staging_overlap_and_traversal_fail_before_mutation() {
    for docs in [".", "..", "_tmp/projr", "_output", "_raw_data/derived", ".git/results"] {
        let temp = tempfile::tempdir().unwrap();
        failed_build_preserves(temp.path(), &configuration(docs));
    }
    let temp = tempfile::tempdir().unwrap();
    failed_build_preserves(temp.path(), &configuration("docs").replace("  output:", "  output/../../escape:"));
}

#[cfg(unix)]
#[test]
fn symlinked_destination_preserves_project_and_external_files() {
    let temp = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    fs::write(external.path().join("keep"), "external").unwrap();
    std::os::unix::fs::symlink(external.path(), temp.path().join("linked")).unwrap();
    failed_build_preserves(temp.path(), &configuration("linked/docs"));
    assert_eq!(fs::read_to_string(external.path().join("keep")).unwrap(), "external");
}

#[test]
fn invalid_directory_moves_do_not_delete_existing_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let canonical = temp.path().canonicalize().unwrap();
    let root = camino::Utf8Path::from_path(&canonical).unwrap();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("keep"), "source").unwrap();
    for destination in [source.clone(), root.to_owned(), source.join("nested")] {
        assert!(proj::dir_move_exact(&source, &destination).is_err());
        assert_eq!(fs::read_to_string(source.join("keep")).unwrap(), "source");
    }
    assert!(proj::dir_move_exact(&root.join("missing"), &source).is_err());
    assert_eq!(fs::read_to_string(source.join("keep")).unwrap(), "source");
}


#[test]
fn disjoint_external_resource_destination_is_allowed() {
    let temp = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let root = temp.path();
    let docs = external.path().canonicalize().unwrap().join("release-docs");
    fs::create_dir(&docs).unwrap();
    fs::write(docs.join("keep.html"), "previous documentation").unwrap();
    let quoted = docs.to_string_lossy().replace('\\', "/").replace('\'', "''");
    fs::write(root.join("_proj.yml"), configuration(&quoted).replace("clear_output: post", "clear_output: never")).unwrap();
    fs::write(root.join("analysis.py"), "from pathlib import Path\nPath('_output').mkdir(exist_ok=True)\nPath('_output/result').write_text('completed')\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build", "--patch", "--non-interactive", "--project"]).arg(root).current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
    assert!(output.status.success(), "{} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert_eq!(fs::read_to_string(root.join("_output/result")).unwrap(), "completed");
    assert_eq!(fs::read_to_string(docs.join("keep.html")).unwrap(), "previous documentation");
}


#[test]
fn direct_cleanup_entrypoints_reject_project_root_destination() {
    let temp = tempfile::tempdir().unwrap();
    let canonical = temp.path().canonicalize().unwrap();
    let root = camino::Utf8Path::from_path(&canonical).unwrap();
    fs::write(root.join("keep"), "project").unwrap();
    let config: proj::ProjConfig = serde_yaml::from_str(&configuration(".")).unwrap();
    let validated = config.validate_and_resolve(root, false).unwrap();
    assert!(proj::clear_pre(root, "1.2.3", &validated, Some("pre")).is_err());
    assert!(proj::clear_post(root, false, &validated, Some("post"), true).is_err());
    assert_eq!(fs::read_to_string(root.join("keep")).unwrap(), "project");
}


#[test]
fn empty_analysis_still_validates_managed_destinations_and_files_are_not_directories() {
    let temp = tempfile::tempdir().unwrap();
    failed_build_preserves(temp.path(), &configuration(".").replace("scripts: [analysis.py]", "scripts: []"));
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("data-file"), "keep").unwrap();
    failed_build_preserves(temp.path(), &configuration("data-file"));
    assert_eq!(fs::read_to_string(temp.path().join("data-file")).unwrap(), "keep");
}


#[test]
fn dev_cleanup_is_a_noop_even_for_unsafe_managed_paths() {
    let temp = tempfile::tempdir().unwrap();
    let canonical = temp.path().canonicalize().unwrap();
    let root = camino::Utf8Path::from_path(&canonical).unwrap();
    fs::write(root.join("keep"), "project").unwrap();
    let raw: proj::ProjConfig = serde_yaml::from_str(&configuration("docs").replace("path: _output", "path: .")).unwrap();
    let validated = raw.validate_and_resolve(root, true).unwrap();
    proj::clear_post(root, true, &validated, Some("post"), false).unwrap();
    proj::clear_old(root, "1.2.3", true, Some(true)).unwrap();
    assert_eq!(fs::read_to_string(root.join("keep")).unwrap(), "project");
}

#[test]
fn copy_rejects_nested_destination_before_creating_empty_subdirectories() {
    let temp = tempfile::tempdir().unwrap();
    let canonical = temp.path().canonicalize().unwrap();
    let root = camino::Utf8Path::from_path(&canonical).unwrap();
    let source = root.join("source");
    fs::create_dir_all(source.join("empty")).unwrap();
    let destination = source.join("new-destination");
    assert!(proj::copy_dir_recursive(&source, &destination).is_err());
    assert!(!destination.exists());
    assert!(source.join("empty").is_dir());
}

#[test]
fn direct_individual_promotions_reject_traversal_and_promote_to_external_docs() {
    for extension in ["qmd", "Rmd"] {
        let temp = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let canonical = temp.path().canonicalize().unwrap();
        let outside = external.path().canonicalize().unwrap();
        let root = camino::Utf8Path::from_path(&canonical).unwrap();
        let docs = camino::Utf8Path::from_path(&outside).unwrap().join("docs");
        let input = root.join(format!("report.{extension}"));
        for name in ["../outside.html", "/absolute.html", "C:\\outside.html", "VERSION", "_proj.yml"] {
            fs::write(&input, format!("---\nformat: html\noutput-file: '{name}'\n---\n")).unwrap();
            let result = if extension == "qmd" { proj::copy_individual_quarto(&input, &docs, root) } else { proj::copy_individual_rmd(&input, &docs, root) };
            assert!(result.is_err());
            assert!(!docs.exists());
        }
        fs::write(&input, "---\nformat: html\n---\n").unwrap();
        fs::write(root.join("report.html"), "rendered").unwrap();
        if extension == "qmd" { proj::copy_individual_quarto(&input, &docs, root).unwrap(); }
        else { proj::copy_individual_rmd(&input, &docs, root).unwrap(); }
        assert_eq!(fs::read_to_string(docs.join("report.html")).unwrap(), "rendered");
        assert!(input.is_file());
    }
}

#[test]
fn project_renderers_reject_source_documents_in_cleanup_trees_before_mutation() {
    for engine in ["_quarto.yml", "_bookdown.yml"] {
        for path in ["docs/nested/source.qmd", "_output/source.ipynb", "_tmp/source.md"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let source = root.join(path);
            fs::create_dir_all(source.parent().unwrap()).unwrap();
            fs::write(&source, "source document").unwrap();
            fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
            fs::write(root.join(engine), "project: {}\n").unwrap();
            fs::write(root.join("_proj.yml"), configuration("docs").replace("  scripts: [analysis.py]\n", "")).unwrap();
            let output = Command::new(env!("CARGO_BIN_EXE_proj"))
                .args(["build", "--patch", "--non-interactive", "--project"]).arg(root).current_dir(root)
                .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("overlaps managed cleanup"), "{}", String::from_utf8_lossy(&output.stderr));
            assert_eq!(fs::read_to_string(&source).unwrap(), "source document");
            assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
            assert!(!root.join(".gitignore").exists());
            assert_eq!(fs::read_to_string(root.join(engine)).unwrap(), "project: {}\n");
        }
    }
}

#[cfg(unix)]
#[test]
fn derived_symlink_leaves_fail_before_any_cleanup_even_when_never_or_dangling() {
    for leaf in ["legacy-version", "configured-version", "configured-output", "legacy-docs", "dangling"] {
        let temp = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let canonical = temp.path().canonicalize().unwrap();
        let root = camino::Utf8Path::from_path(&canonical).unwrap();
        fs::write(external.path().join("keep"), "external").unwrap();
        fs::create_dir_all(root.join("_tmp/projr")).unwrap();
        fs::create_dir_all(root.join("_cache/projr")).unwrap();
        let link = match leaf {
            "legacy-version" | "dangling" => root.join("_tmp/projr/1.2.3"),
            "configured-version" => root.join("_cache/projr/1.2.3"),
            "configured-output" => root.join("_cache/projr/1.2.3/output"),
            _ => root.join("_tmp/projr/1.2.3/docs"),
        };
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        let destination = if leaf == "dangling" { external.path().join("absent") } else { external.path().to_owned() };
        let legacy_version = root.join("_tmp/projr/1.2.3");
        if leaf != "legacy-version" && leaf != "dangling" {
            fs::create_dir_all(&legacy_version).unwrap();
            fs::write(legacy_version.join("keep-old-cache"), "old cache").unwrap();
        }
        std::os::unix::fs::symlink(&destination, &link).unwrap();
        let contents = configuration("docs").replace("path: _tmp", "path: _cache");
        let raw: proj::ProjConfig = serde_yaml::from_str(&contents).unwrap();
        let validated = raw.validate_and_resolve(root, false).unwrap();
        assert!(proj::clear_pre(root, "1.2.3", &validated, Some("never")).is_err());
        fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
        fs::write(root.join("_proj.yml"), &contents).unwrap();
        fs::write(root.join("analysis.py"), "raise RuntimeError('must not run')\n").unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_proj"))
            .args(["build", "--patch", "--non-interactive", "--project", root.as_str()]).current_dir(root)
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
        assert!(!output.status.success());
        assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
        assert!(!root.join(".gitignore").exists());
        assert_eq!(fs::read_to_string(external.path().join("keep")).unwrap(), "external");
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        if leaf != "legacy-version" && leaf != "dangling" {
            assert_eq!(fs::read_to_string(legacy_version.join("keep-old-cache")).unwrap(), "old cache");
        }
    }
}

#[cfg(unix)]
#[test]
fn recursive_copy_and_individual_promotions_reject_symlinked_destinations_before_writes() {
    let temp = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let canonical = temp.path().canonicalize().unwrap();
    let root = camino::Utf8Path::from_path(&canonical).unwrap();
    fs::write(external.path().join("report.html"), "external").unwrap();
    std::os::unix::fs::symlink(external.path(), root.join("linked")).unwrap();
    let source = root.join("source");
    fs::create_dir_all(source.join("empty")).unwrap();
    assert!(proj::copy_dir_recursive(&source, &root.join("linked/new-directory")).is_err());
    assert!(!external.path().join("new-directory").exists());
    for extension in ["qmd", "Rmd"] {
        let input = root.join(format!("report.{extension}"));
        fs::write(&input, "---\nformat: html\n---\n").unwrap();
        fs::write(root.join("report.html"), "rendered").unwrap();
        let result = if extension == "qmd" { proj::copy_individual_quarto(&input, &root.join("linked"), root) } else { proj::copy_individual_rmd(&input, &root.join("linked"), root) };
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(root.join("report.html")).unwrap(), "rendered");
        assert_eq!(fs::read_to_string(external.path().join("report.html")).unwrap(), "external");
    }
}


#[test]
fn literal_root_globs_do_not_hide_selected_inputs_from_cleanup_guards() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project [1]");
    fs::create_dir_all(root.join("_output")).unwrap();
    fs::write(root.join("_output/analysis.py"), "raise RuntimeError('must not run')\n").unwrap();
    fs::write(root.join("_proj.yml"), configuration("docs").replace("scripts: [analysis.py]", "scripts: [_output/analysis.py]")).unwrap();
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build", "--patch", "--non-interactive", "--project"]).arg(&root).current_dir(&root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("contains selected script/hook"), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
    assert!(root.join("_output/analysis.py").exists());
    assert!(!root.join(".gitignore").exists());
}

#[cfg(unix)]
#[test]
fn individual_asset_destination_links_are_checked_before_moving_the_main_output() {
    let temp = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let canonical = temp.path().canonicalize().unwrap();
    let root = camino::Utf8Path::from_path(&canonical).unwrap();
    let docs = root.join("published");
    fs::create_dir(&docs).unwrap();
    fs::create_dir(root.join("report_files")).unwrap();
    fs::write(root.join("report_files/asset"), "generated asset").unwrap();
    fs::write(external.path().join("keep"), "external").unwrap();
    std::os::unix::fs::symlink(external.path(), docs.join("report_files")).unwrap();
    for extension in ["qmd", "Rmd"] {
        let input = root.join(format!("report.{extension}"));
        fs::write(&input, "---\nformat: html\n---\n").unwrap();
        fs::write(root.join("report.html"), "rendered").unwrap();
        let result = if extension == "qmd" { proj::copy_individual_quarto(&input, &docs, root) } else { proj::copy_individual_rmd(&input, &docs, root) };
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(root.join("report.html")).unwrap(), "rendered");
        assert_eq!(fs::read_to_string(external.path().join("keep")).unwrap(), "external");
        assert!(!docs.join("report.html").exists());
    }
}
