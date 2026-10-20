use crate::yml::ValidatedConfig;
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use std::fs;

/// Clears old development folders if configured.
pub fn clear_old(
    project_root: &Utf8Path,
    current_version: &str,
    is_dev: bool,
    old_dev_remove: Option<bool>,
) -> anyhow::Result<()> {
    if old_dev_remove != Some(true) {
        return Ok(());
    }

    let base_path = project_root.join("_tmp").join("projr");
    crate::path_safety::resolve_path(base_path.as_std_path(), true)?;
    if !base_path.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(base_path.as_std_path()).context("Failed to read base directory")? {
        let entry = entry.context("Failed to read base directory entry")?;
        let entry_path = Utf8PathBuf::try_from(entry.path())
            .context("Non-UTF-8 path encountered during base clear iteration")?;

        let is_dir = entry.metadata().context("Failed to read entry metadata")?.is_dir();
        if is_dir {
            let dir_name_str = entry_path.file_name().context("Failed to get file name from path")?;

            if is_dev {
                if dir_name_str != current_version {
                    fs::remove_dir_all(entry_path.as_std_path())
                        .context("Failed to remove directory")?;
                }
            } else {
                if dir_name_str != "log" {
                    fs::remove_dir_all(entry_path.as_std_path())
                        .context("Failed to remove directory")?;
                }
            }
        }
    }

    Ok(())
}

/// Clears pre-build caches and output folders.
pub fn clear_pre(
    project_root: &Utf8Path,
    current_version: &str,
    config: &ValidatedConfig,
    clear_output: Option<&str>,
) -> anyhow::Result<()> {
    crate::path_safety::validate_build_paths(project_root, config, &[])?;
    if current_version.contains('/') || current_version.contains('\\') || current_version == "." || current_version == ".." {
        anyhow::bail!("Invalid cache version component");
    }
    // 1. Versioned cache path clearance
    let versioned_cache_path = project_root
        .join("_tmp")
        .join("projr")
        .join(current_version);

    if versioned_cache_path.exists() {
        for entry in fs::read_dir(versioned_cache_path.as_std_path()).context("Failed to read versioned cache directory")? {
            let entry = entry.context("Failed to read versioned cache directory entry")?;
            let entry_path = Utf8PathBuf::try_from(entry.path())
                .context("Non-UTF-8 path encountered during versioned cache clear")?;
            let name_str = entry_path.file_name().context("Failed to get file name from path")?;

            // Exclude 'old' and allow 'docs' to be cleared out during pre-build
            if name_str != "old" {
                let is_dir = entry.metadata().context("Failed to read entry metadata")?.is_dir();
                if is_dir {
                    fs::remove_dir_all(entry_path.as_std_path())
                        .context("Failed to remove directory")?;
                } else {
                    fs::remove_file(entry_path.as_std_path())
                        .context("Failed to remove file")?;
                }
            }
        }
    }

    // 2. For output directory clearing
    if clear_output == Some("never") {
        return Ok(());
    }

    let is_pre = clear_output == Some("pre");
    let cache_base = match config.get_path(project_root, "cache") {
        Ok(p) => p,
        Err(_) => project_root.join("_tmp"), // default fallback
    };

    for (label, _) in &config.directories {
        let lower_label = label.to_lowercase();
        if !(lower_label.starts_with("output") || lower_label == "data") {
            continue;
        }
        if lower_label == "docs" {
            continue;
        }

        // Construct the safe cache path
        let safe_cache_path = cache_base.join("projr").join(current_version).join(label);
        if safe_cache_path.exists() {
            for entry in fs::read_dir(safe_cache_path.as_std_path()).context("Failed to read safe cache directory")? {
                let entry = entry.context("Failed to read safe cache directory entry")?;
                let entry_path = Utf8PathBuf::try_from(entry.path())
                    .context("Non-UTF-8 path encountered during safe cache clear")?;

                let is_dir = entry.metadata().context("Failed to read entry metadata")?.is_dir();
                if is_dir {
                    fs::remove_dir_all(entry_path.as_std_path())
                        .context("Failed to remove directory")?;
                } else {
                    fs::remove_file(entry_path.as_std_path())
                        .context("Failed to remove file")?;
                }
            }
        }

        // If 'pre', clear the public unsafe path
        if is_pre {
            if let Ok(unsafe_path) = config.get_path(project_root, label) {
                if unsafe_path.exists() {
                    for entry in fs::read_dir(unsafe_path.as_std_path()).context("Failed to read unsafe path directory")? {
                        let entry = entry.context("Failed to read unsafe path directory entry")?;
                        let entry_path = Utf8PathBuf::try_from(entry.path())
                            .context("Non-UTF-8 path encountered during unsafe path clear")?;

                        let is_dir = entry.metadata().context("Failed to read entry metadata")?.is_dir();
                        if is_dir {
                            fs::remove_dir_all(entry_path.as_std_path())
                                .context("Failed to remove directory")?;
                        } else {
                            fs::remove_file(entry_path.as_std_path())
                                .context("Failed to remove file")?;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Clears post-build deployment folders.
pub fn clear_post(
    project_root: &Utf8Path,
    is_dev: bool,
    config: &ValidatedConfig,
    clear_output: Option<&str>,
    is_single_doc_engine: bool,
) -> anyhow::Result<()> {
    if !is_dev {
        crate::path_safety::validate_build_paths(project_root, config, &[])?;
    }
    if !is_dev && is_single_doc_engine {
        if let Ok(docs_path) = config.get_path(project_root, "docs") {
            if docs_path.exists() {
                for entry in fs::read_dir(docs_path.as_std_path()).context("Failed to read docs directory")? {
                    let entry = entry.context("Failed to read docs directory entry")?;
                    let entry_path = Utf8PathBuf::try_from(entry.path())
                        .context("Non-UTF-8 path encountered during docs clear")?;

                    let is_dir = entry.metadata().context("Failed to read entry metadata")?.is_dir();
                    if is_dir {
                        fs::remove_dir_all(entry_path.as_std_path())
                            .context("Failed to remove directory")?;
                    } else {
                        fs::remove_file(entry_path.as_std_path())
                            .context("Failed to remove file")?;
                    }
                }
            }
        }
    }

    if clear_output == Some("post") {
        for (label, _) in &config.directories {
            let lower_label = label.to_lowercase();
            if lower_label.starts_with("output") || lower_label == "data" {
                if let Ok(unsafe_path) = config.get_path(project_root, label) {
                    if unsafe_path.exists() {
                        for entry in fs::read_dir(unsafe_path.as_std_path()).context("Failed to read unsafe path directory")? {
                            let entry = entry.context("Failed to read unsafe path directory entry")?;
                            let entry_path = Utf8PathBuf::try_from(entry.path())
                                .context("Non-UTF-8 path encountered during post output clear")?;

                            let is_dir = entry.metadata().context("Failed to read entry metadata")?.is_dir();
                            if is_dir {
                                fs::remove_dir_all(entry_path.as_std_path())
                                    .context("Failed to remove directory")?;
                            } else {
                                fs::remove_file(entry_path.as_std_path())
                                    .context("Failed to remove file")?;
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
