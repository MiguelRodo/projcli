
use std::env;

use std::fs;
use crate::yml::{ValidatedConfig, IgnoreConfig};

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
/// `PROJ MANAGED` markers. It resolves the specific rules outlined in the `ValidatedConfig`.
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
pub fn update_ignores_for(project_root: &camino::Utf8Path, validated: &ValidatedConfig) -> anyhow::Result<()> {
    let mut git_ignores = Vec::new();
    let mut rbuild_ignores = Vec::new();

    for (_, dir) in &validated.directories {
        crate::yml::validate_ignore(&dir.ignore)?;
        // Collect ignores based on the ignore config
        let (should_git, should_rbuild) = match &dir.ignore {
            IgnoreConfig::Single(s) => {
                let s = s.to_lowercase();
                (s == "all" || s == "git", s == "all" || s == "rbuild")
            }
            IgnoreConfig::Multiple(vec) => {
                let lower_vec: Vec<String> = vec.iter().map(|s| s.to_lowercase()).collect();
                (lower_vec.contains(&"all".to_string()) || lower_vec.contains(&"git".to_string()),
                 lower_vec.contains(&"all".to_string()) || lower_vec.contains(&"rbuild".to_string()))
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
                let path_str = if cfg!(windows) {path_str.replace('\\',"/")}else{path_str};
                if should_git {
                    let literal=path_str.chars().flat_map(|character| {
                        if matches!(character,'\\'|'#'|'!'|'*'|'?'|'['|']') {vec!['\\',character]}else{vec![character]}
                    }).collect::<String>();
                    let git_pattern=format!("{}/**",literal.trim_end_matches('/'));
                    git_ignores.push(git_pattern);
                }
                if should_rbuild {
                    let rbuild_pattern = format!("^{}/$", regex::escape(path_str.trim_end_matches('/')));
                    rbuild_ignores.push(rbuild_pattern);
                    let rbuild_pattern_2 = format!("^{}$", regex::escape(path_str.trim_end_matches('/')));
                    rbuild_ignores.push(rbuild_pattern_2);
                }
            }
        }
    }

    if project_root.join("DESCRIPTION").exists() {
        rbuild_ignores.extend([
            r"^\.git$", r"^\.proj$", r"^\.projr$", r"^_proj(r)?(-[^/]+)?\.ya?ml$",
            r"^VERSION$", r"^manifest\.csv$", r"^CITATION\.cff$", r"^_quarto\.ya?ml$",
            r"^_bookdown\.ya?ml$", r"^README\.Rmd$", r"^README_files$", r"^.*\.Rproj$", r"^\.Rproj\.user$"
        ].into_iter().map(str::to_owned));
    }
    // Sort and deduplicate
    git_ignores.sort();
    git_ignores.dedup();
    rbuild_ignores.sort();
    rbuild_ignores.dedup();

    let gitignore_path = project_root.join(".gitignore");
    update_ignore_file(&gitignore_path, &git_ignores)?;

    let rbuildignore_path = project_root.join(".Rbuildignore");
    if project_root.join("DESCRIPTION").exists() || rbuildignore_path.exists() {
        update_ignore_file(&rbuildignore_path, &rbuild_ignores)?;
    }

    Ok(())
}

fn update_ignore_file(path: &camino::Utf8Path, ignores: &[String]) -> anyhow::Result<()> {
    let start_marker = "# --- PROJ MANAGED ---";
    let end_marker = "# --- END PROJ MANAGED ---";

    let content_lines: Vec<String> = if path.exists() {
        crate::fs_utils::read_managed_text(path)?
            .lines()
            .map(|s| s.to_string())
            .collect()
    } else {
        if ignores.is_empty() {
            return Ok(()); // Don't create file if nothing to ignore
        }
        Vec::new()
    };

    let starts=content_lines.iter().filter(|line|line.as_str()==start_marker).count();
    let ends=content_lines.iter().filter(|line|line.as_str()==end_marker).count();
    anyhow::ensure!((starts==0&&ends==0)||(starts==1&&ends==1),
        "Malformed or repeated managed ignore markers in {path}");
    let start_idx = content_lines.iter().position(|line| line == start_marker);
    let end_idx = content_lines.iter().position(|line| line == end_marker);

    anyhow::ensure!(start_idx.zip(end_idx).is_none_or(|(start,end)|start<end),
        "Reversed managed ignore markers in {path}");
    let mut new_lines = Vec::new();

    match (start_idx, end_idx) {
        (Some(start), Some(end)) if start < end => {
            // Found block, replace it
            new_lines.extend_from_slice(&content_lines[..=start]);
            for ignore in ignores {
                new_lines.push(ignore.clone());
            }
            new_lines.extend_from_slice(&content_lines[end..]);
        }
        _ => {
            // Block not found or malformed, append to end
            new_lines.extend(content_lines);
            if !new_lines.is_empty() && new_lines.last().unwrap() != "" {
                new_lines.push("".to_string());
            }
            new_lines.push(start_marker.to_string());
            for ignore in ignores {
                new_lines.push(ignore.clone());
            }
            new_lines.push(end_marker.to_string());
        }
    }

    let mut final_content = new_lines.join("\n");
    if !final_content.ends_with('\n') {
        final_content.push('\n');
    }

    // Only write if the content actually changed
    if path.exists() {
        if let Ok(existing_content) = crate::fs_utils::read_managed_text(path) {
            if existing_content == final_content {
                return Ok(());
            }
        }
    }

    if path.exists() {crate::fs_utils::replace_managed_file(path, final_content)?;}
    else {crate::fs_utils::write_managed_file(path, final_content)?;}
    Ok(())
}

/// Finds the nearest recognized project root at or above a starting directory.
///
/// Recognizes projr-style VCS, VERSION, README, R-package, RStudio, renv and
/// Quarto markers, plus proj configuration and Python/Julia project metadata.
/// It never searches child directories. With no marker, the starting directory
/// is selected, matching projr's cwd fallback. Invalid paths return `None`.
/// Discovery alone does not validate or waive the build configuration contract.
///
/// ```rust
/// let project = tempfile::tempdir().unwrap();
/// std::fs::write(project.path().join("README.md"), "research").unwrap();
/// let path = camino::Utf8Path::from_path(project.path()).unwrap();
/// assert_eq!(proj::root_from(path), Some(path.canonicalize_utf8().unwrap()));
/// ```
pub fn root_from(start_path: &camino::Utf8Path) -> Option<camino::Utf8PathBuf> {
    let start = camino::Utf8PathBuf::from_path_buf(fs::canonicalize(start_path).ok()?).ok()?;
    if !start.is_dir() {
        return None;
    }
    let found = start.ancestors().find(|path| has_project_marker(path)).map(|path| path.to_owned());
    found.or(Some(start))
}

fn has_line_prefix(path: &camino::Utf8Path, prefix: &str) -> bool {
    fs::read_to_string(path).map(|text| text.lines().any(|line| line.starts_with(prefix))).unwrap_or(false)
}

fn has_project_marker(path: &camino::Utf8Path) -> bool {
    if ["VERSION", "README.md", "_quarto.yml", "_proj.yml", "_projr.yml",
        "pyproject.toml", "Project.toml", "JuliaProject.toml"]
        .iter().any(|name| path.join(name).is_file())
        || path.join(".git").is_dir() || path.join(".svn").is_dir()
        || has_line_prefix(&path.join(".git"), "gitdir: ")
        || has_line_prefix(&path.join("DESCRIPTION"), "Package: ")
    {
        return true;
    }
    if let Ok(text) = fs::read_to_string(path.join("renv.lock")) {
        let packages = regex::Regex::new(r#""Packages":\s*\{"#).expect("constant marker pattern");
        if text.lines().any(|line| packages.is_match(line)) {
            return true;
        }
    }
    if let Ok(entries) = fs::read_dir(path) {
        return entries.flatten().any(|entry| {
            let file = entry.path();
            file.is_file() && file.extension().and_then(|extension| extension.to_str()) == Some("Rproj")
                && fs::read_to_string(file).map(|text| text.lines().next().is_some_and(|line| line.starts_with("Version: "))).unwrap_or(false)
        });
    }
    false
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
pub fn update_ignores(project_root: &camino::Utf8Path, validated: &ValidatedConfig) -> anyhow::Result<()> {
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
        vec![
            format!("^{}/", trimmed),
            format!("^{}$", trimmed),
        ]
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
        vec![
            format!("^!{}/", trimmed),
            format!("^!{}$", trimmed),
        ]
    } else {
        vec![format!("^!{}$", trimmed)]
    }
}

/// Appends entries to an ignore file outside the managed block.
///
/// Modifies the ignore file by appending user-specified manual exclusions
/// strictly above the `# --- PROJ MANAGED ---` block, ensuring separating whitespace.
fn append_manual_ignores(path: &camino::Utf8Path, ignores: &[String]) -> anyhow::Result<()> {
    if ignores.is_empty() {
        return Ok(());
    }

    let start_marker = "# --- PROJ MANAGED ---";

    let content_lines: Vec<String> = if path.exists() {
        crate::fs_utils::read_managed_text(path)?
            .lines()
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    };

    let start_idx = content_lines.iter().position(|line| line == start_marker);

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
/// strictly below the `# --- END PROJ MANAGED ---` block.
fn append_unignores(path: &camino::Utf8Path, ignores: &[String]) -> anyhow::Result<()> {
    if ignores.is_empty() {
        return Ok(());
    }

    let start_marker = "# --- PROJ MANAGED ---";
    let end_marker = "# --- END PROJ MANAGED ---";

    let content_lines: Vec<String> = if path.exists() {
        crate::fs_utils::read_managed_text(path)?
            .lines()
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    };

    let mut new_lines = Vec::new();
    let end_idx = content_lines.iter().position(|line| line == end_marker);

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

            if !lower_lines.is_empty() && new_lines.last().map(|s| s.as_str()) != Some("") && lower_lines.first().map(|s| s.as_str()) != Some("") {
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

            new_lines.push(start_marker.to_string());
            new_lines.push(end_marker.to_string());

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
/// fs::write(root.join(".gitignore"), "# --- PROJ MANAGED ---\n").unwrap();
///
/// add_manual_ignores(root, &["temp.log".to_string()], true, IgnoreType::Git).unwrap();
///
/// let contents = fs::read_to_string(root.join(".gitignore")).unwrap();
/// assert!(contents.contains("temp.log"));
/// assert!(contents.contains("# --- PROJ MANAGED ---"));
/// ```
pub fn add_manual_ignores(
    project_root: &camino::Utf8Path,
    paths: &[String],
    force_create: bool,
    ignore_type: IgnoreType
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
/// `.gitignore` or `.Rbuildignore` below the `# --- END PROJ MANAGED ---` block.
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
/// fs::write(root.join(".gitignore"), "# --- PROJ MANAGED ---\n# --- END PROJ MANAGED ---\n").unwrap();
///
/// remove_manual_ignores(root, &["!!temp.log".to_string()], IgnoreType::Git).unwrap();
///
/// let contents = fs::read_to_string(root.join(".gitignore")).unwrap();
/// assert!(contents.contains("!temp.log"));
/// assert!(contents.contains("# --- END PROJ MANAGED ---"));
/// assert!(contents.find("# --- END PROJ MANAGED ---").unwrap() < contents.find("!temp.log").unwrap());
/// ```
pub fn remove_manual_ignores(
    project_root: &camino::Utf8Path,
    paths: &[String],
    ignore_type: IgnoreType
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
