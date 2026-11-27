use std::env;

use crate::yml::{IgnoreConfig, ValidatedConfig};
use std::fs;

/// Filter selection for ignore targeting.
///
/// Controls whether manual ignores apply to all tracking surfaces or only specific ones.
#[derive(Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum IgnoreType {
    /// Target both tracking surfaces.
    All,
    /// Target `.gitignore` only.
    Git,
    /// Target `.Rbuildignore` only.
    Rbuild,
}

impl Default for IgnoreType {
    fn default() -> Self {
        IgnoreType::All
    }
}

/// Synchronizes ignore rules dynamically across `.gitignore` and `.Rbuildignore`.
///
/// Modifies the ignore files in place within the demarcated regions defined by
/// projr's `# Start of projr section` / `# End of projr section` markers. It
/// resolves the specific rules outlined in the `ValidatedConfig`. A file
/// without that block first receives projr's editable starter defaults (such
/// as `_projr-local.yml`) outside the block.
///
/// # Errors
///
/// Returns an error if an ignore file cannot be created, written to, or if
/// the target physical directory doesn't have required permission layers.
///
/// ```rust
///
/// use std::collections::HashMap;
/// use tempfile::TempDir;
/// use proj::yml::{ValidatedConfig, ResolvedDir, IgnoreConfig};
/// use proj::ignore::update_ignores_for;
///
/// let temp = TempDir::new().unwrap();
/// let canonical_root = temp.path().canonicalize().unwrap();
/// let root = camino::Utf8Path::from_path(&canonical_root).unwrap();
///
/// use proj::yml::{ResolvedGitConfig, RestrictionsConfig, GlobalConfig};
/// let mut dirs = HashMap::new();
/// dirs.insert("raw".to_string(), ResolvedDir {
///     safe: None, hash: None,
///     path: root.join("_raw"),
///     ignore: IgnoreConfig::Single("all".to_string())
/// });
///
/// let validated = ValidatedConfig {
///     remotes: Default::default(),
///     dest: vec![],
///     config: GlobalConfig::default(),
///     settings: Default::default(),
///     directories: dirs,
///     git: ResolvedGitConfig { commit: false, push: false },
///     restrictions: RestrictionsConfig::default(),
///     clear_output: None,
///     output_run: None,
///     old_dev_remove: None,
///     parameters: Default::default()
/// };
/// update_ignores_for(&root, &validated).unwrap();
///
/// assert!(root.join(".gitignore").exists());
/// ```
/// The supplied project root must use its resolved identity; controlled writes
/// reject symlinks in the root or metadata path rather than following them.
pub fn update_ignores_for(
    project_root: &camino::Utf8Path,
    validated: &ValidatedConfig,
) -> anyhow::Result<()> {
    update_ignores_for_generated(project_root, validated, &[])
}

pub(crate) fn update_ignores_for_generated(
    project_root: &camino::Utf8Path,
    validated: &ValidatedConfig,
    generated: &[std::path::PathBuf],
) -> anyhow::Result<()> {
    let mut git_dirs = Vec::new();
    let mut rbuild_dirs = Vec::new();

    for (label, dir) in &validated.directories {
        crate::yml::validate_ignore(&dir.ignore)?;
        // Collect ignores based on the ignore config
        let (should_git, should_rbuild) = match &dir.ignore {
            IgnoreConfig::Unspecified => {
                let scripts = crate::script_selection::is_scripts(label);
                let path = validated.get_path(project_root, label)?;
                let package_source = path
                    .strip_prefix(project_root)
                    .ok()
                    .and_then(|path| path.components().next())
                    .is_some_and(|part| {
                        ["R", "man", "inst", "src", "data", "tests", "vignettes"]
                            .iter()
                            .any(|name| part.as_str().eq_ignore_ascii_case(name))
                    });
                (!scripts, !(scripts && package_source))
            }
            IgnoreConfig::Single(s) => {
                let s = s.to_lowercase();
                (s == "all" || s == "git", s == "all" || s == "rbuild")
            }
            IgnoreConfig::Multiple(vec) => {
                let lower_vec: Vec<String> = vec.iter().map(|s| s.to_lowercase()).collect();
                (
                    lower_vec.contains(&"all".to_string())
                        || lower_vec.contains(&"git".to_string()),
                    lower_vec.contains(&"all".to_string())
                        || lower_vec.contains(&"rbuild".to_string()),
                )
            }
        };

        if should_git || should_rbuild {
            let path_str_opt = if dir.path.is_absolute() {
                // Only process if it is within the project root
                if let Ok(rel_path) = dir.path.strip_prefix(project_root) {
                    Some(rel_path.as_str().to_string())
                } else {
                    None
                }
            } else {
                Some(dir.path.as_str().to_string())
            };

            if let Some(path_str) = path_str_opt {
                let path_str = if cfg!(windows) {
                    path_str.replace('\\', "/")
                } else {
                    path_str
                };
                if should_git {
                    let literal = path_str
                        .chars()
                        .flat_map(|character| {
                            if matches!(character, '\\' | '#' | '!' | '*' | '?' | '[' | ']') {
                                vec!['\\', character]
                            } else {
                                vec![character]
                            }
                        })
                        .collect::<String>();
                    let git_pattern = format!("{}/**", literal.trim_end_matches('/'));
                    git_dirs.push(git_pattern);
                }
                if should_rbuild {
                    let rbuild_pattern =
                        format!("^{}/$", regex::escape(path_str.trim_end_matches('/')));
                    rbuild_dirs.push(rbuild_pattern);
                    let rbuild_pattern_2 =
                        format!("^{}$", regex::escape(path_str.trim_end_matches('/')));
                    rbuild_dirs.push(rbuild_pattern_2);
                }
            }
        }
    }

    // Automatic metadata ignores share the same conditional Rbuild policy.
    let mut git_ignores = Vec::new();
    let mut rbuild_ignores = Vec::new();
    if project_root.join("_environment.local").exists() {
        git_ignores.push("/_environment.local".to_owned());
        rbuild_ignores.push(r"^_environment\.local$".to_owned());
    }
    // projr's project-state rules (`R/ignore-auto.R:125-132`).
    git_ignores.extend(
        [
            "/.projr/manifests/.versions.lock",
            "/.projr/logs/**",
            "/.projr/source/**",
        ]
        .map(str::to_owned),
    );
    // Local configuration files, matched by profile (`R/ignore-auto.R:134-142`).
    for file in crate::profile::config_files(project_root)? {
        if crate::profile::config_file_profile(&file) == "local" {
            git_ignores.push(file);
        }
    }
    // projr's Quarto directory rules (`R/ignore-auto.R:59-80`).
    for directory in [".quarto", "index_files"] {
        if project_root.join(directory).is_dir() {
            git_ignores.push(format!("{directory}/**"));
        }
    }
    if project_root.join("DESCRIPTION").exists() {
        // projr's conditional R-build rules (`R/ignore-auto.R:59-160`).
        for directory in [
            "_extensions",
            "index_files",
            ".quarto",
            ".devcontainer",
            ".github",
            ".vscode",
        ] {
            if project_root.join(directory).is_dir() {
                let escaped = regex::escape(directory);
                rbuild_ignores.extend([format!("^{escaped}/"), format!("^{escaped}$")]);
            }
        }
        let entries = fs::read_dir(project_root).map_err(|error| {
            anyhow::anyhow!("Cannot list {project_root} for ignore rules: {error}")
        })?;
        for entry in entries {
            let name = entry?.file_name();
            let Some(name) = name.to_str() else { continue };
            let lower = name.to_ascii_lowercase();
            if name.ends_with(".code-workspace")
                || name == "_environment"
                || name == "_environment.required"
                || name.starts_with("_environment-")
                || lower.ends_with(".qmd")
                || lower.ends_with(".rmd")
            {
                rbuild_ignores.push(format!("^{}$", regex::escape(name)));
            }
        }
        rbuild_ignores.extend(
            [
                r"^\.git$",
                r"^\.projr$",
                r"^_projr$",
                r"^_projr(-[^/]+)?\.yml$",
                r"^VERSION$",
                r"^manifest\.csv$",
                r"^CITATION\.cff$",
                r"^_quarto\.ya?ml$",
                r"^_bookdown\.ya?ml$",
                r"^README\.Rmd$",
                r"^README_files$",
                r"^.*\.Rproj$",
                r"^\.Rproj\.user$",
            ]
            .into_iter()
            .map(str::to_owned),
        );
    }
    // Frozen native outputs only: never all HTML/assets or final docs.
    for (label, directory) in &validated.directories {
        if !crate::script_selection::is_scripts(label) {
            continue;
        }
        let auto_git = match &directory.ignore {
            IgnoreConfig::Unspecified => true,
            IgnoreConfig::Single(value) => ["all", "git"]
                .iter()
                .any(|allowed| value.eq_ignore_ascii_case(allowed)),
            IgnoreConfig::Multiple(values) => values.iter().any(|value| {
                ["all", "git"]
                    .iter()
                    .any(|allowed| value.eq_ignore_ascii_case(allowed))
            }),
        };
        if !auto_git {
            continue;
        }
        let source = validated.get_path(project_root, label)?;
        for path in generated {
            let path = camino::Utf8Path::from_path(path)
                .ok_or_else(|| anyhow::anyhow!("Generated path must be UTF-8"))?;
            if path.starts_with(&source) {
                let relative = path.strip_prefix(project_root)?;
                let value = if cfg!(windows) {
                    relative.as_str().replace('\\', "/")
                } else {
                    relative.to_string()
                };
                let escaped = value
                    .chars()
                    .flat_map(|c| {
                        if matches!(c, '\\' | '#' | '!' | '*' | '?' | '[' | ']') {
                            vec!['\\', c]
                        } else {
                            vec![c]
                        }
                    })
                    .collect::<String>();
                git_ignores.push(format!("/{escaped}"));
            }
        }
    }
    let gitignore_path = project_root.join(".gitignore");
    update_ignore_file(&gitignore_path, git_dirs, git_ignores, GIT_STARTER)?;

    let rbuildignore_path = project_root.join(".Rbuildignore");
    if project_root.join("DESCRIPTION").exists() || rbuildignore_path.exists() {
        update_ignore_file(
            &rbuildignore_path,
            rbuild_dirs,
            rbuild_ignores,
            RBUILD_STARTER,
        )?;
    }

    Ok(())
}

/// Start line written for a new managed block, byte-identical to projr's.
const START_MARKER: &str =
    "# Start of projr section: do not edit by hand (update with projr_ignore_auto())";
/// End line of the managed block, byte-identical to projr's.
const END_MARKER: &str = "# End of projr section";

/// Editable `.gitignore` defaults projr seeds once, outside the managed block.
const GIT_STARTER: &[&str] = &[
    ".vdoc.*",
    ".DS_Store",
    "Thumbs.db",
    ".Rproj.user",
    ".Rhistory",
    ".RData",
    ".Ruserdata",
    "_projr-local.yml",
    "_environment.local",
];

/// Editable `.Rbuildignore` defaults projr seeds once, outside the managed block.
const RBUILD_STARTER: &[&str] = &[
    r"(^|/)\.vdoc\.",
    r"(^|/)(\.DS_Store|Thumbs\.db)$",
    r"^.*\.Rproj$",
    r"^\.Rproj\.user$",
    r"^_projr-local\.yml$",
];

/// Matches projr's `^# Start of projr section(:.*)?[[:blank:]]*$`.
fn is_start_marker(line: &str) -> bool {
    line.strip_prefix("# Start of projr section")
        .is_some_and(|rest| rest.starts_with(':') || rest.trim_matches([' ', '\t']).is_empty())
}

/// Matches projr's `^# End of projr section[[:blank:]]*$`.
fn is_end_marker(line: &str) -> bool {
    line.strip_prefix("# End of projr section")
        .is_some_and(|rest| rest.trim_matches([' ', '\t']).is_empty())
}

/// Locates the single managed block, rejecting repeated, unpaired or reversed
/// markers instead of guessing.
fn managed_block(
    lines: &[String],
    path: &camino::Utf8Path,
) -> anyhow::Result<Option<(usize, usize)>> {
    let find = |matches: fn(&str) -> bool| -> Vec<usize> {
        let marked = lines.iter().enumerate().filter(|(_, line)| matches(line));
        marked.map(|(index, _)| index).collect()
    };
    match (
        find(is_start_marker).as_slice(),
        find(is_end_marker).as_slice(),
    ) {
        ([], []) => Ok(None),
        ([start], [end]) if start < end => Ok(Some((*start, *end))),
        ([_], [_]) => anyhow::bail!("Reversed managed ignore markers in {path}"),
        _ => anyhow::bail!("Malformed or repeated managed ignore markers in {path}"),
    }
}

/// Rewrites the managed block in projr's layout: configured directory rules, a
/// blank separator, then the other automatic rules. A file without a block first
/// receives the starter defaults it lacks, then the block, then its existing
/// lines, so later user exceptions still win (projr's `.init_ignore_starter`).
fn update_ignore_file(
    path: &camino::Utf8Path,
    mut directories: Vec<String>,
    mut others: Vec<String>,
    starter: &[&str],
) -> anyhow::Result<()> {
    for rules in [&mut directories, &mut others] {
        rules.sort();
        rules.dedup();
    }
    let existing = if path.exists() {
        Some(crate::fs_utils::read_managed_text(path)?)
    } else {
        None
    };
    let lines: Vec<String> = existing
        .as_deref()
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    let content = directories.into_iter().chain([String::new()]).chain(others);
    let new_lines: Vec<String> = match managed_block(&lines, path)? {
        Some((start, end)) => lines[..=start]
            .iter()
            .cloned()
            .chain(content)
            .chain(lines[end..].iter().cloned())
            .collect(),
        None => starter
            .iter()
            .filter(|rule| !lines.iter().any(|line| line.as_str() == **rule))
            .map(|rule| rule.to_string())
            .chain([START_MARKER.to_owned()])
            .chain(content)
            .chain([END_MARKER.to_owned()])
            .chain(lines.iter().cloned())
            .collect(),
    };
    let final_content = new_lines.join("\n") + "\n";
    match existing {
        Some(existing) if existing == final_content => Ok(()),
        Some(_) => crate::fs_utils::replace_managed_file(path, final_content),
        None => crate::fs_utils::write_managed_file(path, final_content),
    }
}

/// Finds the project root: the nearest folder at or above a starting directory
/// holding a projr configuration (`_projr.yml`, or `_projr.yml`/`default.yml` in
/// its `.projr/config/` or `_projr/config/`), as projr's root criterion does.
///
/// Other markers (VERSION, Git, DESCRIPTION, ...) are not consulted: when no
/// configuration exists anywhere above, the starting directory is the root, so a
/// first build or init writes the starter there. Child directories are never
/// searched. Invalid paths return `None`.
///
/// ```rust
/// let project = tempfile::tempdir().unwrap();
/// std::fs::write(project.path().join("_projr.yml"), "directories: {}").unwrap();
/// let nested = project.path().join("analysis");
/// std::fs::create_dir(&nested).unwrap();
/// std::fs::write(nested.join("VERSION"), "v1.0.0").unwrap();
/// let path = camino::Utf8Path::from_path(&nested).unwrap();
/// let root = camino::Utf8Path::from_path(project.path()).unwrap();
/// assert_eq!(proj::root_from(path), Some(root.canonicalize_utf8().unwrap()));
/// ```
pub fn root_from(start_path: &camino::Utf8Path) -> Option<camino::Utf8PathBuf> {
    let start = camino::Utf8PathBuf::from_path_buf(fs::canonicalize(start_path).ok()?).ok()?;
    if !start.is_dir() {
        return None;
    }
    let found = start
        .ancestors()
        .find(|path| crate::profile::has_base_config(path))
        .map(|path| path.to_owned());
    found.or(Some(start))
}

/// Seeds projr's editable starter lines and an empty managed block into
/// `.gitignore` (and `.Rbuildignore` in a package) when no block exists yet,
/// as projr's `.init_ignore_starter` does. Files with a block are untouched.
pub(crate) fn seed_ignore_starter(project_root: &camino::Utf8Path) -> anyhow::Result<()> {
    let mut targets = vec![(project_root.join(".gitignore"), GIT_STARTER)];
    if project_root.join("DESCRIPTION").exists() || project_root.join(".Rbuildignore").exists() {
        targets.push((project_root.join(".Rbuildignore"), RBUILD_STARTER));
    }
    for (path, starter) in targets {
        let existing = if path.exists() {
            Some(crate::fs_utils::read_managed_text(&path)?)
        } else {
            None
        };
        let lines: Vec<String> = existing
            .as_deref()
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect();
        if managed_block(&lines, &path)?.is_some() {
            continue;
        }
        let new_lines: Vec<String> = starter
            .iter()
            .filter(|rule| !lines.iter().any(|line| line.as_str() == **rule))
            .map(|rule| rule.to_string())
            .chain([START_MARKER.to_owned(), END_MARKER.to_owned()])
            .chain(lines.iter().cloned())
            .collect();
        let content = new_lines.join("\n") + "\n";
        match existing {
            Some(_) => crate::fs_utils::replace_managed_file(&path, content)?,
            None => crate::fs_utils::write_managed_file(&path, content)?,
        }
    }
    Ok(())
}

/// Thin production wrapper: discovers the conceptual project root directory via heuristic path traversal.
///
/// An inherited absolute `PROJ_PROJECT_ROOT` pins the root across cwd changes.
/// Otherwise this uses the current directory as the starting search context.
/// An invalid inherited root fails instead of silently rediscovering a different project.
///
/// # Errors
///
/// Returns `None` if the root cannot be located or if the environment's current working directory is invalid.
///
/// ```rust,ignore
/// use proj::ignore::root;
/// let result = root();
/// ```
pub fn root() -> Option<camino::Utf8PathBuf> {
    if let Some(inherited) = env::var_os(crate::project::PROJECT_ROOT_ENV) {
        let path = std::path::PathBuf::from(inherited);
        if !path.is_absolute() || !path.is_dir() {
            return None;
        }
        return camino::Utf8PathBuf::from_path_buf(std::fs::canonicalize(path).ok()?).ok();
    }
    let current_dir = env::current_dir().ok()?;
    root_from(camino::Utf8Path::from_path(&current_dir)?)
}

/// Thin production wrapper: synchronizes ignore rules dynamically across `.gitignore` and `.Rbuildignore`.
///
/// # Errors
///
/// Returns an error if the files cannot be updated or if required permissions are lacking.
///
/// ```rust,ignore
/// use proj::ignore::update_ignores;
/// update_ignores(project_root, validated_config).unwrap();
/// ```
pub fn update_ignores(
    project_root: &camino::Utf8Path,
    validated: &ValidatedConfig,
) -> anyhow::Result<()> {
    update_ignores_for(project_root, validated)
}

/// Classifies a path string based on disk status and trailing slash.
fn is_directory(path_str: &str, project_root: &camino::Utf8Path) -> bool {
    if path_str.ends_with('/') {
        return true;
    }
    let full_path = project_root.join(path_str);
    full_path.is_dir()
}

fn format_gitignore_path(path_str: &str, is_dir: bool) -> String {
    let mut formatted = path_str.to_string();
    if is_dir && !formatted.ends_with('/') {
        formatted.push('/');
    }
    formatted
}

fn format_unignore_gitignore_path(path_str: &str, is_dir: bool) -> String {
    let stripped = path_str.trim_start_matches('!');
    let mut formatted = stripped.to_string();
    if is_dir && !formatted.ends_with('/') {
        formatted.push('/');
    }
    format!("!{}", formatted)
}

fn format_rbuildignore_path(path_str: &str, is_dir: bool) -> Vec<String> {
    let mut trimmed = path_str.trim_end_matches('/').trim().to_string();
    trimmed = trimmed.replace(".", "\\.");
    trimmed = trimmed.replace("*", ".*");
    trimmed = trimmed.replace("?", ".");

    if is_dir {
        vec![format!("^{}/", trimmed), format!("^{}$", trimmed)]
    } else {
        vec![format!("^{}$", trimmed)]
    }
}

fn format_unignore_rbuildignore_path(path_str: &str, is_dir: bool) -> Vec<String> {
    let stripped = path_str.trim_start_matches('!');
    let mut trimmed = stripped.trim_end_matches('/').trim().to_string();
    trimmed = trimmed.replace(".", "\\.");
    trimmed = trimmed.replace("*", ".*");
    trimmed = trimmed.replace("?", ".");

    if is_dir {
        vec![format!("^!{}/", trimmed), format!("^!{}$", trimmed)]
    } else {
        vec![format!("^!{}$", trimmed)]
    }
}

/// Appends entries to an ignore file outside the managed block.
///
/// Modifies the ignore file by appending user-specified manual exclusions
/// strictly above the `# Start of projr section` block, ensuring separating whitespace.
fn append_manual_ignores(path: &camino::Utf8Path, ignores: &[String]) -> anyhow::Result<()> {
    if ignores.is_empty() {
        return Ok(());
    }

    let content_lines: Vec<String> = if path.exists() {
        crate::fs_utils::read_managed_text(path)?
            .lines()
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    };

    let start_idx = content_lines.iter().position(|line| is_start_marker(line));

    let mut new_lines = Vec::new();
    let mut added_any = false;

    match start_idx {
        Some(idx) => {
            new_lines.extend_from_slice(&content_lines[..idx]);
            // Remove trailing empty lines before the block
            while let Some(last) = new_lines.last() {
                if last.trim().is_empty() {
                    new_lines.pop();
                } else {
                    break;
                }
            }

            for ignore in ignores {
                if !new_lines.contains(ignore) && !content_lines[idx..].contains(ignore) {
                    new_lines.push(ignore.clone());
                    added_any = true;
                }
            }
            if added_any && !new_lines.is_empty() {
                new_lines.push("".to_string());
            }
            new_lines.extend_from_slice(&content_lines[idx..]);
        }
        None => {
            new_lines.extend(content_lines.clone());
            // Remove trailing empty lines at end of file
            while let Some(last) = new_lines.last() {
                if last.trim().is_empty() {
                    new_lines.pop();
                } else {
                    break;
                }
            }

            for ignore in ignores {
                if !new_lines.contains(ignore) {
                    new_lines.push(ignore.clone());
                    added_any = true;
                }
            }
        }
    }

    if !added_any {
        return Ok(());
    }

    let mut final_content = new_lines.join("\n");
    if !final_content.ends_with('\n') {
        final_content.push('\n');
    }

    crate::fs_utils::write_managed_file(path, final_content)?;
    Ok(())
}

/// Appends unignore (negated) entries to an ignore file strictly below the managed block.
///
/// Modifies the ignore file by appending user-specified manual negations
/// strictly below the `# End of projr section` block.
fn append_unignores(path: &camino::Utf8Path, ignores: &[String]) -> anyhow::Result<()> {
    if ignores.is_empty() {
        return Ok(());
    }

    let content_lines: Vec<String> = if path.exists() {
        crate::fs_utils::read_managed_text(path)?
            .lines()
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    };

    let mut new_lines = Vec::new();
    let end_idx = content_lines.iter().position(|line| is_end_marker(line));

    let mut added_any = false;

    match end_idx {
        Some(idx) => {
            new_lines.extend_from_slice(&content_lines[..=idx]);

            // Check what lines exist below the end marker
            let mut lower_lines = content_lines[idx + 1..].to_vec();

            // Deduplicate new ignores against lines already present below the marker
            for ignore in ignores {
                if !lower_lines.contains(ignore) {
                    lower_lines.push(ignore.clone());
                    added_any = true;
                }
            }

            if !lower_lines.is_empty()
                && new_lines.last().map(|s| s.as_str()) != Some("")
                && lower_lines.first().map(|s| s.as_str()) != Some("")
            {
                // Connection formatting lines can be wired here if needed
            }

            new_lines.extend(lower_lines);
        }
        None => {
            // No managed block exists at all
            new_lines.extend(content_lines.clone());

            // Remove trailing empty lines at end of file
            while let Some(last) = new_lines.last() {
                if last.trim().is_empty() {
                    new_lines.pop();
                } else {
                    break;
                }
            }

            if !new_lines.is_empty() {
                new_lines.push("".to_string());
            }

            new_lines.push(START_MARKER.to_string());
            new_lines.push(END_MARKER.to_string());

            for ignore in ignores {
                if !new_lines.contains(ignore) {
                    new_lines.push(ignore.clone());
                    added_any = true;
                }
            }
        }
    }

    if !added_any {
        return Ok(());
    }

    let mut final_content = new_lines.join("\n");
    if !final_content.ends_with('\n') {
        final_content.push('\n');
    }

    crate::fs_utils::write_managed_file(path, final_content)?;
    Ok(())
}

/// Adds raw paths to tracking ignores manually outside the managed block.
///
/// Converts a list of raw string paths into properly formatted regular expressions
/// or pattern rules, and injects them into `.gitignore` or `.Rbuildignore` accordingly.
///
/// If `force_create` is `false`, it skips creating `.gitignore` if no `.git` context exists,
/// and skips `.Rbuildignore` if no `DESCRIPTION` file exists in the project root.
///
/// # Errors
///
/// Returns an error if an I/O exception occurs while attempting to read or write
/// to the configuration files.
///
/// ```rust
/// use std::fs;
/// use tempfile::TempDir;
/// use proj::ignore::{add_manual_ignores, IgnoreType};
///
/// let temp = TempDir::new().unwrap();
/// let canonical_root = temp.path().canonicalize().unwrap();
/// let root = camino::Utf8Path::from_path(&canonical_root).unwrap();
/// fs::write(root.join(".gitignore"), "# Start of projr section\n# End of projr section\n").unwrap();
///
/// add_manual_ignores(root, &["temp.log".to_string()], true, IgnoreType::Git).unwrap();
///
/// let contents = fs::read_to_string(root.join(".gitignore")).unwrap();
/// assert!(contents.contains("temp.log"));
/// assert!(contents.find("temp.log").unwrap() < contents.find("# Start of projr section").unwrap());
/// ```
pub fn add_manual_ignores(
    project_root: &camino::Utf8Path,
    paths: &[String],
    force_create: bool,
    ignore_type: IgnoreType,
) -> anyhow::Result<()> {
    let mut git_ignores = Vec::new();
    let mut rbuild_ignores = Vec::new();

    for path_str in paths {
        let is_dir = is_directory(path_str, project_root);

        if ignore_type == IgnoreType::All || ignore_type == IgnoreType::Git {
            git_ignores.push(format_gitignore_path(path_str, is_dir));
        }

        if ignore_type == IgnoreType::All || ignore_type == IgnoreType::Rbuild {
            rbuild_ignores.extend(format_rbuildignore_path(path_str, is_dir));
        }
    }

    let should_git = ignore_type == IgnoreType::All || ignore_type == IgnoreType::Git;
    let should_rbuild = ignore_type == IgnoreType::All || ignore_type == IgnoreType::Rbuild;

    if should_git {
        let gitignore_path = project_root.join(".gitignore");
        let valid_env = project_root.join(".git").exists();
        if gitignore_path.exists() || force_create || (!force_create && valid_env) {
            append_manual_ignores(&gitignore_path, &git_ignores)?;
        }
    }

    if should_rbuild {
        let rbuildignore_path = project_root.join(".Rbuildignore");
        let valid_env = project_root.join("DESCRIPTION").exists();
        if rbuildignore_path.exists() || force_create || (!force_create && valid_env) {
            append_manual_ignores(&rbuildignore_path, &rbuild_ignores)?;
        }
    }

    Ok(())
}

/// Manually forces paths to be tracked by appending negated patterns.
///
/// Converts a list of raw string paths into properly formatted negated regular expressions
/// or pattern rules (stripping duplicate negation tokens), and injects them into
/// `.gitignore` or `.Rbuildignore` below the `# End of projr section` block.
///
/// Skips `.Rbuildignore` if no `DESCRIPTION` file exists in the project root.
/// If the target ignore file does not exist, it will be created and the negated paths
/// appended below the empty block position.
///
/// # Validation & Errors
///
/// Paths will be pre-processed to remove duplicate negation `!` operators.
/// Returns an error if an I/O exception occurs while attempting to read or write
/// to the configuration files.
///
/// ```rust
/// use std::fs;
/// use tempfile::TempDir;
/// use proj::ignore::{remove_manual_ignores, IgnoreType};
///
/// let temp = TempDir::new().unwrap();
/// let canonical_root = temp.path().canonicalize().unwrap();
/// let root = camino::Utf8Path::from_path(&canonical_root).unwrap();
/// fs::write(root.join(".gitignore"), "# Start of projr section\n# End of projr section\n").unwrap();
///
/// remove_manual_ignores(root, &["!!temp.log".to_string()], IgnoreType::Git).unwrap();
///
/// let contents = fs::read_to_string(root.join(".gitignore")).unwrap();
/// assert!(contents.contains("!temp.log"));
/// assert!(contents.contains("# End of projr section"));
/// assert!(contents.find("# End of projr section").unwrap() < contents.find("!temp.log").unwrap());
/// ```
pub fn remove_manual_ignores(
    project_root: &camino::Utf8Path,
    paths: &[String],
    ignore_type: IgnoreType,
) -> anyhow::Result<()> {
    let mut git_ignores = Vec::new();
    let mut rbuild_ignores = Vec::new();

    let mut clean_paths = Vec::new();
    for p in paths {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            clean_paths.push(trimmed.to_string());
        }
    }
    clean_paths.sort();
    clean_paths.dedup();

    for path_str in clean_paths {
        let is_dir = is_directory(&path_str, project_root);

        if ignore_type == IgnoreType::All || ignore_type == IgnoreType::Git {
            git_ignores.push(format_unignore_gitignore_path(&path_str, is_dir));
        }

        if ignore_type == IgnoreType::All || ignore_type == IgnoreType::Rbuild {
            rbuild_ignores.extend(format_unignore_rbuildignore_path(&path_str, is_dir));
        }
    }

    let should_git = ignore_type == IgnoreType::All || ignore_type == IgnoreType::Git;
    let should_rbuild = ignore_type == IgnoreType::All || ignore_type == IgnoreType::Rbuild;

    if should_git {
        let gitignore_path = project_root.join(".gitignore");
        append_unignores(&gitignore_path, &git_ignores)?;
    }

    if should_rbuild {
        let valid_env = project_root.join("DESCRIPTION").exists();
        if valid_env {
            let rbuildignore_path = project_root.join(".Rbuildignore");
            append_unignores(&rbuildignore_path, &rbuild_ignores)?;
        }
    }

    Ok(())
}
