//! Ordered analysis selection from explicit files, source directories and globs.
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};

pub(crate) fn is_scripts(label: &str) -> bool {
    label.eq_ignore_ascii_case("scripts")
        || label
            .to_ascii_lowercase()
            .strip_prefix("scripts-")
            .is_some_and(|name| !name.is_empty())
}

fn metadata_component(value: &str) -> bool {
    [".git", ".proj", ".projr"].iter().any(|name| {
        if cfg!(windows) {
            value.eq_ignore_ascii_case(name)
        } else {
            value == *name
        }
    })
}

enum Exclusion {
    File(Utf8PathBuf),
    Pattern(glob::Pattern),
}

fn supported(path: &Utf8Path) -> bool {
    matches!(
        path.extension(),
        Some("R" | "r" | "py" | "qmd" | "Rmd" | "rmd")
    )
}

fn portable(value: &str) -> anyhow::Result<String> {
    let value = if cfg!(windows) {
        value.replace('\\', "/")
    } else {
        value.to_owned()
    };
    anyhow::ensure!(
        !value.is_empty()
            && !value.contains('\\')
            && !value.contains(':')
            && !value.chars().any(char::is_control)
            && Utf8Path::new(&value).components().all(|part| matches!(
                part,
                camino::Utf8Component::Normal(_) | camino::Utf8Component::CurDir
            )),
        "Script patterns must be nonempty project-relative paths without parent traversal"
    );
    let normalized = Utf8Path::new(&value)
        .components()
        .filter_map(|part| match part {
            camino::Utf8Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    Ok(if normalized.is_empty() {
        ".".to_owned()
    } else {
        normalized
    })
}

fn walk(
    directory: &Utf8Path,
    depth: Option<usize>,
    blocked: &[Utf8PathBuf],
    files: &mut Vec<Utf8PathBuf>,
) -> anyhow::Result<()> {
    let mut entries = std::fs::read_dir(directory)
        .with_context(|| format!("Cannot enumerate script directory {directory}"))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("Cannot read script entry at {directory}"))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if entry.file_name().to_str().is_some_and(metadata_component) {
            continue;
        }
        let path = Utf8PathBuf::from_path_buf(entry.path())
            .map_err(|_| anyhow::anyhow!("Script paths must be UTF-8"))?;
        if blocked.iter().any(|base| path.starts_with(base)) {
            continue;
        }
        let kind = entry
            .file_type()
            .with_context(|| format!("Cannot inspect script entry {path}"))?;
        anyhow::ensure!(
            !kind.is_symlink(),
            "Script discovery does not follow symbolic links: {path}"
        );
        if kind.is_file() && supported(&path) {
            files.push(path);
        } else if kind.is_dir() && depth != Some(0) {
            walk(&path, depth.map(|value| value - 1), blocked, files)?;
        }
    }
    Ok(())
}

/// Frozen directory declarations resolve names; ordinary relative patterns use the project root.
pub(crate) fn resolve(
    root: &Utf8Path,
    selection: &[String],
    config: &crate::yml::ValidatedConfig,
) -> anyhow::Result<Vec<Utf8PathBuf>> {
    let metadata_directory = |path: &Utf8Path| {
        path.strip_prefix(root).is_ok_and(|relative| {
            relative
                .components()
                .any(|part| metadata_component(part.as_str()))
        })
    };
    let mut blocked = vec![root.join(".git"), root.join(".proj"), root.join(".projr")];
    for label in config.directories.keys() {
        if label.to_ascii_lowercase().starts_with("cache") || crate::paths::is_managed_output(label)
        {
            blocked.push(
                Utf8PathBuf::from_path_buf(crate::path_safety::resolve_path(
                    config.get_path(root, label)?.as_std_path(),
                    true,
                )?)
                .map_err(|_| anyhow::anyhow!("Directory path must be UTF-8"))?,
            );
        }
    }
    let text = |value: &str| -> anyhow::Result<String> {
        if let Some(label) = config
            .directories
            .keys()
            .find(|label| is_scripts(label) && label.eq_ignore_ascii_case(value))
        {
            let path = Utf8PathBuf::from_path_buf(crate::path_safety::resolve_path(
                config.get_path(root, label)?.as_std_path(),
                true,
            )?)
            .map_err(|_| anyhow::anyhow!("Scripts path must be UTF-8"))?;
            anyhow::ensure!(
                path.starts_with(root) && path != root,
                "Scripts directory must be inside the project: {label}"
            );
            return Ok(path.strip_prefix(root)?.as_str().replace('\\', "/"));
        }
        if root.join(value).is_file() {
            let path = Utf8PathBuf::from_path_buf(crate::path_safety::resolve_path(
                root.join(value).as_std_path(),
                true,
            )?)
            .map_err(|_| anyhow::anyhow!("Script path must be UTF-8"))?;
            return Ok(path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_owned()
                .into_string());
        }
        portable(value)
    };
    let options = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: true,
    };
    // Parse every exclusion first, even an exclusion-only selection.
    let mut exclusions = Vec::new();
    for value in selection.iter().filter_map(|value| value.strip_prefix('!')) {
        let mut value = text(value)?;
        let target = root.join(&value);
        if target.is_file() {
            exclusions.push(Exclusion::File(
                Utf8PathBuf::from_path_buf(crate::path_safety::resolve_path(
                    target.as_std_path(),
                    true,
                )?)
                .map_err(|_| anyhow::anyhow!("Script path must be UTF-8"))?,
            ));
        } else {
            if target.is_dir() {
                value = format!("{}/**", glob::Pattern::escape(value.trim_end_matches('/')));
            }
            exclusions.push(Exclusion::Pattern(
                glob::Pattern::new(&value)
                    .with_context(|| format!("Invalid script/hook exclusion pattern '!{value}'"))?,
            ));
        }
    }
    let mut selected = Vec::new();
    for value in selection.iter().filter(|value| !value.starts_with('!')) {
        let value = text(value)?;
        let target = root.join(&value);
        let mut files = Vec::new();
        if target.exists() {
            crate::path_safety::resolve_path(target.as_std_path(), true)?;
            if target.is_dir() {
                anyhow::ensure!(
                    !metadata_directory(&target),
                    "Script directory selection enters project metadata: {value}"
                );
                anyhow::ensure!(
                    !blocked.iter().any(|base| target.starts_with(base)),
                    "Script directory selection overlaps generated output or project state: {value}"
                );
                walk(&target, None, &blocked, &mut files)?;
            } else {
                anyhow::ensure!(
                    target.is_file() && supported(&target),
                    "Unsupported selected script: {value}"
                );
                files.push(target);
            }
        } else {
            let pattern = glob::Pattern::new(&value)
                .with_context(|| format!("Invalid script pattern '{value}'"))?;
            let parts: Vec<_> = value.split('/').collect();
            let first = parts
                .iter()
                .position(|part| part.chars().any(|c| matches!(c, '*' | '?' | '[')))
                .unwrap_or(parts.len().saturating_sub(1));
            let mut directory = root.to_owned();
            for part in &parts[..first] {
                directory.push(part);
            }
            crate::path_safety::resolve_path(directory.as_std_path(), true)?;
            anyhow::ensure!(
                !metadata_directory(&directory),
                "Script glob selection enters project metadata: {value}"
            );
            if directory.is_dir() && !blocked.iter().any(|base| directory.starts_with(base)) {
                let remainder = &parts[first..];
                let depth = if remainder.contains(&"**") {
                    None
                } else {
                    Some(remainder.len().saturating_sub(1))
                };
                walk(&directory, depth, &blocked, &mut files)?;
                files.retain(|file| {
                    pattern
                        .matches_path_with(file.strip_prefix(root).unwrap().as_std_path(), options)
                });
            }
        }
        anyhow::ensure!(
            !files.is_empty(),
            "Explicit script/hook pattern matched no files (no supported sources): {value}"
        );
        // Each include expands lexically; explicit include order remains authoritative.
        files = files
            .into_iter()
            .map(|file| {
                Utf8PathBuf::from_path_buf(crate::path_safety::resolve_path(
                    file.as_std_path(),
                    true,
                )?)
                .map_err(|_| anyhow::anyhow!("Script path must be UTF-8"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        files.sort();
        for file in files {
            let relative = file.strip_prefix(root).unwrap_or(&file);
            if !exclusions.iter().any(|exclude| match exclude {
                Exclusion::File(path) => path == &file,
                Exclusion::Pattern(pattern) => {
                    pattern.matches_path_with(relative.as_std_path(), options)
                }
            }) && !selected.contains(&file)
            {
                selected.push(file);
            }
        }
    }
    Ok(selected)
}

/// Routes independent documents within the one configured docs destination.
pub(crate) fn document_directory(
    root: &Utf8Path,
    file: &Utf8Path,
    docs: &Utf8Path,
    config: &crate::yml::ValidatedConfig,
) -> anyhow::Result<Utf8PathBuf> {
    let mut groups = Vec::new();
    for label in config.directories.keys() {
        if !is_scripts(label) {
            continue;
        }
        let source = Utf8PathBuf::from_path_buf(crate::path_safety::resolve_path(
            config.get_path(root, label)?.as_std_path(),
            true,
        )?)
        .map_err(|_| anyhow::anyhow!("Scripts path must be UTF-8"))?;
        if let Ok(relative) = file.strip_prefix(&source) {
            let mut directory = docs.to_owned();
            if !label.eq_ignore_ascii_case("scripts") {
                directory.push(&label["scripts-".len()..]);
            }
            if let Some(parent) = relative.parent() {
                directory.push(parent);
            }
            groups.push(directory);
        }
    }
    anyhow::ensure!(
        groups.len() <= 1,
        "Selected document belongs to overlapping scripts directories: {file}"
    );
    Ok(groups.pop().unwrap_or_else(|| docs.to_owned()))
}
