use crate::ignore::root;
use crate::profile::get_active_profiles;
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;

/// Represents the complete structure of a `_proj.yml` configuration file.
///
/// Contains dynamically user-defined directory labels mapping to specific system paths.
/// These paths adhere to required prefixes like `raw`, `output`, `cache`, or `docs`.
///
/// # Errors
///
/// Deserialization will fail if the provided YAML is structurally invalid or map
/// types are mismatched.
///
/// ```rust
/// use proj::yml::ProjConfig;
/// let config = ProjConfig::default();
/// assert!(config.directories.is_empty());
/// ```

#[derive(Deserialize, Debug, Default, Clone)]
pub struct RemotesConfig {
    pub local: Option<HashMap<String, LocalRemoteConfig>>,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StorageStructure {
    Cas,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InspectStrategy {
    Manifest,
    File,
}

#[derive(Deserialize, Debug, Clone)]
pub struct LocalRemoteConfig {
    pub path: Utf8PathBuf,
    pub structure: StorageStructure,
    pub content: Vec<String>,
    pub inspect: InspectStrategy,
}

#[derive(Deserialize, Debug, Default, Clone)]
pub struct ProjConfig {
    #[serde(default)]
    pub remotes: RemotesConfig,
    #[serde(default)]
    pub config: GlobalConfig,
    #[serde(default)]
    pub directories: HashMap<String, DirConfig>,
    #[serde(default)]
    pub build: BuildConfig,
    #[serde(default)]
    pub dev: DevConfig,
    /// Captures arbitrary parameters maps under any matching alias keys.
    /// Evaluates aliases in order: "parameters", "parameter", "params", "param".
    #[serde(
        alias = "parameter",
        alias = "params",
        alias = "param",
        default = "default_parameters"
    )]
    pub parameters: serde_yaml::Value,
    #[serde(default)]
    pub metadata: Option<Value>,
}

fn default_parameters() -> serde_yaml::Value {
    serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GitEngine {
    Auto,
    System,
}

/// Represents the top-level configuration key `config` in `_proj.yml`.
#[derive(Deserialize, Debug, Default, Clone)]
pub struct GlobalConfig {
    #[serde(default)]
    pub git: GeneralGitConfig,
}

fn default_git_engine() -> GitEngine {
    GitEngine::Auto
}

/// Represents general git settings inside `config.git`.
#[derive(Deserialize, Debug, Clone)]
pub struct GeneralGitConfig {
    #[serde(default = "default_use_proj_cred_helper")]
    pub use_proj_cred_helper: bool,
    #[serde(default = "default_git_engine")]
    pub engine: GitEngine,
}

impl Default for GeneralGitConfig {
    fn default() -> Self {
        Self {
            use_proj_cred_helper: true,
            engine: GitEngine::Auto,
        }
    }
}

fn default_use_proj_cred_helper() -> bool {
    true
}

/// Represents the build configuration options within `_proj.yml`.
#[derive(Deserialize, Debug, Default, Clone)]
pub struct BuildConfig {
    pub dest: Option<Vec<String>>,
    pub scripts: Option<Vec<String>>,
    #[serde(default)]
    pub hooks: HooksConfig,
    pub profile: Option<String>,
    #[serde(default)]
    pub git: GitConfigOpt,
    #[serde(default)]
    pub restrictions: RestrictionsConfig,
    pub clear_output: Option<String>,
    pub output_run: Option<bool>,
}

#[derive(Deserialize, Debug, Default, Clone, PartialEq)]
pub struct HooksConfig {
    pub pre: Option<Vec<String>>,
    pub post: Option<Vec<String>>,
    pub both: Option<Vec<String>>,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct RestrictionsConfig {
    pub only_branches: Option<Vec<String>>,
    pub not_branches: Option<Vec<String>>,
    pub not_behind: Option<bool>,
}

impl Default for RestrictionsConfig {
    fn default() -> Self {
        RestrictionsConfig {
            only_branches: None,
            not_branches: None,
            not_behind: None, // Omitted means implicitly true
        }
    }
}

#[derive(Deserialize, Debug, Default, Clone, PartialEq)]
pub struct DevConfig {
    pub scripts: Option<Vec<String>>,
    pub hooks: Option<HooksConfig>,
    pub old_dev_remove: Option<bool>,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(untagged)]
pub enum GitConfigOpt {
    Boolean(bool),
    Detailed(GitConfig),
}

impl Default for GitConfigOpt {
    fn default() -> Self {
        // By default, acts as a detailed block with empty entries to evaluate run-time fallbacks
        GitConfigOpt::Detailed(GitConfig::default())
    }
}

#[derive(Deserialize, Debug, Default, Clone, PartialEq)]
pub struct GitConfig {
    pub commit: Option<bool>,
    pub push: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedGitConfig {
    pub commit: bool,
    pub push: bool,
}

/// Represents the configuration for a single directory entry within `_proj.yml`.
///
/// Each custom label specifies an optional specific physical path and a set of
/// ignore rules to be synced automatically with `.gitignore` and `.Rbuildignore`.
///
/// # Errors
///
/// Fails to parse if the `path` key holds a non-string format.
///
/// ```rust
/// use proj::yml::{DirConfig, IgnoreConfig};
/// let dir = DirConfig { path: None, ignore: IgnoreConfig::Single("all".to_string()) };
/// ```
#[derive(Deserialize, Debug, Clone)]
pub struct DirConfig {
    pub path: Option<Utf8PathBuf>,
    #[serde(default = "default_ignore")]
    pub ignore: IgnoreConfig,
}

/// Defines auto-generated ignore rules applied to tracked directories.
///
/// Can either be a single string rule or a list of multiple target platforms
/// (e.g. `["git", "rbuild"]`).
///
/// # Errors
///
/// Standard fallback occurs dynamically if untagged keys don't match list or string.
///
/// ```rust
/// use proj::yml::IgnoreConfig;
/// let config1 = IgnoreConfig::Single("all".to_string());
/// let config2 = IgnoreConfig::Multiple(vec!["git".to_string(), "rbuild".to_string()]);
/// ```
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(untagged)]
pub enum IgnoreConfig {
    Single(String),
    Multiple(Vec<String>),
}

fn default_ignore() -> IgnoreConfig {
    IgnoreConfig::Single("all".to_string())
}

/// The fully resolved and validated project configuration.
///
/// Once parsed and checked, this structure guarantees that all referenced directories
/// conform to naming rules, and default unreferenced base targets are injected.
///
/// # Panics
///
/// None expected as this acts purely as an internal verified state container.
///
/// ```rust
/// use std::collections::HashMap;
/// use proj::yml::{ValidatedConfig, ResolvedGitConfig, RestrictionsConfig, GlobalConfig};
/// let config = ValidatedConfig {
///     remotes: Default::default(),
///     dest: vec![],
///     config: GlobalConfig::default(),
///     directories: HashMap::new(),
///     git: ResolvedGitConfig { commit: false, push: false },
///     restrictions: RestrictionsConfig::default(),
///     clear_output: None,
///     output_run: None,
///     old_dev_remove: None,
///     parameters: Default::default()
/// };
/// ```
#[derive(Debug)]
pub struct ValidatedConfig {
    pub remotes: RemotesConfig,
    pub dest: Vec<String>,
    pub config: GlobalConfig,
    pub directories: HashMap<String, ResolvedDir>,
    pub git: ResolvedGitConfig,
    pub restrictions: RestrictionsConfig,
    pub clear_output: Option<String>,
    pub output_run: Option<bool>,
    pub old_dev_remove: Option<bool>,
    pub parameters: serde_yaml::Value,
}

/// Represents a validated directory with an absolute or properly referenced system path.
///
/// Holds the final calculated path alongside its resolved `IgnoreConfig` rule set.
///
/// ```rust
/// use camino::{Utf8Path, Utf8PathBuf};
/// use proj::yml::{ResolvedDir, IgnoreConfig};
/// let resolved = ResolvedDir { path: Utf8PathBuf::from("_raw"), ignore: IgnoreConfig::Single("all".to_string()) };
/// ```
#[derive(Debug)]
pub struct ResolvedDir {
    pub path: Utf8PathBuf,
    pub ignore: IgnoreConfig,
}

impl ProjConfig {
    /// Validates the raw parsed YAML layout and injects missing base default targets.
    ///
    /// It iterates through all specified configuration blocks to ensure custom label names
    /// start with approved identifiers (`cache`, `raw`, `output`, `docs`).
    ///
    /// # Errors
    ///
    /// Returns a validation error `String` if a defined directory label uses an
    /// unsupported prefix layout.
    ///
    /// ```rust
    /// use proj::yml::ProjConfig;
    /// use camino::{Utf8Path, Utf8PathBuf};
    /// let config = ProjConfig::default();
    /// let validated = config.validate_and_resolve(&Utf8PathBuf::from("."), false).unwrap();
    /// assert!(validated.directories.contains_key("raw"));
    /// ```
    pub fn validate_and_resolve(
        &self,
        project_root: &Utf8Path,
        is_dev: bool,
    ) -> anyhow::Result<ValidatedConfig> {
        let mut resolved = HashMap::new();

        // 0. Validate Structural Constraints
        if let Some(dest) = &self.build.dest {
            if let Some(remotes) = &self.remotes.local {
                for dest_tag in dest {
                    if !remotes.contains_key(dest_tag) {
                        anyhow::bail!(
                            "Build destination target '{}' is not registered in remotes.local inventory.",
                            dest_tag
                        );
                    }
                }
            } else {
                anyhow::bail!(
                    "build.dest contains targets but remotes.local is completely undefined."
                );
            }
        }

        // 1. Process explicit user configurations
        for (label, config) in &self.directories {
            let lower_label = label.to_lowercase();

            // Validate prefix rules
            if !lower_label.starts_with("cache")
                && !lower_label.starts_with("raw")
                && !lower_label.starts_with("output")
                && !lower_label.starts_with("docs")
            {
                anyhow::bail!(
                    "Invalid directory label '{}' found in _proj.yml. \
                    All custom labels must begin with 'raw', 'output', 'docs', or 'cache'.",
                    label
                );
            }

            // Fallback to defaults if 'path' key is omitted under a validated label
            let path = match &config.path {
                Some(p) => p.clone(),
                None => match () {
                    _ if lower_label.starts_with("cache") => Utf8PathBuf::from("_tmp"),
                    _ if lower_label.starts_with("raw") => Utf8PathBuf::from("_raw"),
                    _ if lower_label.starts_with("output") => Utf8PathBuf::from("_output"),
                    _ if lower_label.starts_with("docs") => Utf8PathBuf::from("docs"),
                    _ => unreachable!(),
                },
            };

            resolved.insert(
                label.clone(),
                ResolvedDir {
                    path,
                    ignore: config.ignore.clone(),
                },
            );
        }

        // 2. Inject missing default base targets if completely omitted from the YAML file
        let base_defaults = [
            ("cache", "_tmp"),
            ("raw", "_raw"),
            ("output", "_output"),
            ("docs", "docs"),
        ];

        for (base_key, default_path) in base_defaults {
            // Check if any existing key satisfies this base (case-insensitive check)
            let exists = resolved.keys().any(|k| k.to_lowercase() == base_key);
            if !exists {
                resolved.insert(
                    base_key.to_string(),
                    ResolvedDir {
                        path: Utf8PathBuf::from(default_path),
                        ignore: default_ignore(),
                    },
                );
            }
        }

        // 3. Resolve Git configuration
        let has_git = project_root.join(".git").exists();

        let mut resolved_git = ResolvedGitConfig {
            commit: false,
            push: false,
        };

        if is_dev {
            // Dev builds bypass git
            resolved_git.commit = false;
            resolved_git.push = false;
        } else {
            match &self.build.git {
                GitConfigOpt::Boolean(false) => {
                    resolved_git.commit = false;
                    resolved_git.push = false;
                }
                GitConfigOpt::Boolean(true) => {
                    resolved_git.commit = true;
                    resolved_git.push = false;
                }
                GitConfigOpt::Detailed(detail) => {
                    resolved_git.commit = detail.commit.unwrap_or(has_git);
                    resolved_git.push = detail.push.unwrap_or(false);
                }
            }
        }

        if resolved_git.push && !resolved_git.commit {
            anyhow::bail!("Configuration error: 'push' cannot be true if 'commit' is false.");
        }

        Ok(ValidatedConfig {
            remotes: self.remotes.clone(),
            dest: self.build.dest.clone().unwrap_or_default(),
            config: self.config.clone(),
            directories: resolved,
            git: resolved_git,
            restrictions: self.build.restrictions.clone(),
            clear_output: self.build.clear_output.clone(),
            output_run: self.build.output_run,
            old_dev_remove: self.dev.old_dev_remove,
            parameters: self.parameters.clone(),
        })
    }
}

impl ValidatedConfig {
    /// Resolves an exact or dynamically prefixed directory label against the physical file system.
    ///
    /// Takes the base project root and constructs absolute paths based on mapped configuration definitions.
    /// If an exact match is missing, it intelligently applies suffix nesting rules based on prefix mappings.
    ///
    /// # Errors
    ///
    /// Returns an error if the requested label completely fails prefix structural checks.
    ///
    /// ```rust
    /// use camino::{Utf8Path, Utf8PathBuf};
    /// use std::collections::HashMap;
    /// use proj::yml::{ValidatedConfig, ResolvedDir, IgnoreConfig, ResolvedGitConfig, RestrictionsConfig, GlobalConfig};
    ///
    /// let mut dirs = HashMap::new();
    /// dirs.insert("raw".to_string(), ResolvedDir {
    ///     path: Utf8PathBuf::from("_raw"),
    ///     ignore: IgnoreConfig::Single("all".to_string())
    /// });
    ///
    /// let config = ValidatedConfig {
    ///     remotes: Default::default(),
    ///     dest: vec![],
    ///     config: GlobalConfig::default(),
    ///     directories: dirs,
    ///     git: ResolvedGitConfig { commit: false, push: false },
    ///     restrictions: RestrictionsConfig::default(),
    ///     clear_output: None,
    ///     output_run: None,
    ///     old_dev_remove: None,
    ///     parameters: Default::default()
    /// };
    /// let path = config.get_path(&Utf8PathBuf::from("/mock/root"), "raw-data").unwrap();
    /// assert_eq!(path, Utf8PathBuf::from("/mock/root/_raw/data"));
    /// ```
    pub fn get_path(&self, project_root: &Utf8Path, label: &str) -> anyhow::Result<Utf8PathBuf> {
        // Rule A: Check for an exact matching key in the map
        if let Some(dir) = self.directories.get(label) {
            return Ok(make_absolute(project_root, &dir.path));
        }

        // Rule B: Dynamic prefix parsing fallback for unlisted sub-labels
        let lower_label = label.to_lowercase();
        let base_prefix = if lower_label.starts_with("cache") {
            Some("cache")
        } else if lower_label.starts_with("raw") {
            Some("raw")
        } else if lower_label.starts_with("output") {
            Some("output")
        } else if lower_label.starts_with("docs") {
            Some("docs")
        } else {
            None
        };

        if let Some(prefix) = base_prefix {
            // Find the base directory configuration (which is guaranteed to exist due to our injector)
            // Perform case-insensitive lookup to avoid panic if base was provided in different casing
            let base_dir = &self
                .directories
                .iter()
                .find(|(k, _)| k.to_lowercase() == prefix)
                .unwrap()
                .1
                .path;

            // Extract the suffix part
            let suffix = if label.len() > prefix.len() {
                if label.as_bytes()[prefix.len()] == b'-' {
                    &label[prefix.len() + 1..]
                } else {
                    &label[prefix.len()..]
                }
            } else {
                ""
            };

            let resolved_path = if suffix.is_empty() {
                base_dir.clone()
            } else {
                base_dir.join(suffix)
            };

            Ok(make_absolute(project_root, &resolved_path))
        } else {
            anyhow::bail!(
                "Requested label '{}' does not match any valid structural prefix.",
                label
            )
        }
    }
}

fn make_absolute(root: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

/// Deep merges `source` into `target`.
pub fn deep_merge(target: Value, source: Value) -> Value {
    match (target.clone(), source.clone()) {
        (_, Value::Null) => target,
        (Value::Object(mut target_map), Value::Object(source_map)) => {
            for (k, v) in source_map {
                let target_val = target_map.remove(&k).unwrap_or(Value::Null);
                target_map.insert(k, deep_merge(target_val, v));
            }
            Value::Object(target_map)
        }
        (_, _) => source,
    }
}

/// Strictly filters out any top-level namespace not in permitted keys.
pub fn yml_get_filter_top_level(value: Value) -> Value {
    if let Value::Object(map) = value {
        let mut new_map = serde_json::Map::new();
        let allowed_keys = [
            "directories",
            "build",
            "dev",
            "metadata",
            "remotes",
            "config",
        ];
        for (k, v) in map {
            if allowed_keys.contains(&k.as_str()) {
                new_map.insert(k, v);
            }
        }
        Value::Object(new_map)
    } else {
        value
    }
}

/// Selects the base filename without silently merging canonical and legacy files.
pub(crate) fn base_configuration_path(base_dir: &Utf8Path) -> anyhow::Result<Option<Utf8PathBuf>> {
    let primary = base_dir.join("_proj.yml");
    let legacy = base_dir.join("_projr.yml");
    if primary.exists() && legacy.exists() {
        anyhow::bail!("Ambiguous configuration: both _proj.yml and _projr.yml exist; keep one base file.");
    }
    Ok(if primary.exists() { Some(primary) } else if legacy.exists() { Some(legacy) } else { None })
}

fn read_configuration_file(path: &Utf8Path) -> anyhow::Result<Value> {
    let content = fs::read_to_string(path).with_context(|| format!("Failed to read configuration {path}"))?;
    let value: Value = serde_yaml::from_str(&content).with_context(|| format!("Failed to parse configuration {path}"))?;
    let aliases = ["parameters", "parameter", "params", "param"];
    if aliases.iter().filter(|key| value.get(**key).is_some()).count() > 1 {
        anyhow::bail!("Configuration error: multiple 'parameters' aliases found in {path}.");
    }
    Ok(value)
}

/// Merges canonical or legacy base configuration, active profiles, and local overrides.
///
/// Explicit profiles replace the environment list. Local overrides win over profiles;
/// earlier-listed profiles win over later profiles, and profiles win over the base.
/// Both base filenames together are an error. Reading never modifies project files.
pub fn get_combined_yml(
    explicit_profile: Option<&str>,
    base_dir: &Utf8Path,
) -> anyhow::Result<Value> {
    let mut base_val = match base_configuration_path(base_dir)? {
        Some(path) => read_configuration_file(&path)?,
        None => Value::Object(serde_json::Map::new()),
    };
    let profiles = if let Some(explicit) = explicit_profile {
        explicit.split(|c| c == ',' || c == ';')
            .map(str::trim).filter(|s| !s.is_empty() && *s != "default" && *s != "local")
            .map(str::to_string).collect()
    } else {
        get_active_profiles()
    };
    if profiles.iter().any(|name| !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')) {
        anyhow::bail!("Invalid active profile name.");
    }
    for profile in profiles.into_iter().rev() {
        let path = base_dir.join(format!("_projr-{profile}.yml"));
        if path.exists() {
            base_val = deep_merge(base_val, read_configuration_file(&path)?);
        }
    }
    let local = base_dir.join("_projr-local.yml");
    if local.exists() {
        base_val = deep_merge(base_val, read_configuration_file(&local)?);
    }
    Ok(yml_get_filter_top_level(base_val))
}

/// Reads, merges and validates configuration without modifying the filesystem.
///
/// Missing configuration uses defaults; malformed or unsupported values return
/// errors from parsing or validation. `is_dev` selects development Git settings.
/// Ignore synchronization is an explicit responsibility of mutating operations.
///
/// ```rust
/// let project = tempfile::tempdir().unwrap();
/// let root = camino::Utf8Path::from_path(project.path()).unwrap();
/// let config = proj::yml_read_from(root, false).unwrap();
/// assert!(config.directories.contains_key("output"));
/// assert!(!root.join(".gitignore").exists());
/// ```
pub fn yml_read_from(project_root: &Utf8Path, is_dev: bool) -> anyhow::Result<ValidatedConfig> {
    let combined_val = get_combined_yml(None, project_root)?;
    let config: ProjConfig = serde_json::from_value(combined_val)
        .context("Failed to deserialize combined configuration.")?;
    let validated = config.validate_and_resolve(project_root, is_dev)?;
    Ok(validated)
}

/// Extracts a deeply nested parameter value from the loaded project configuration.
pub fn get_parameter(validated: &ValidatedConfig, keys: &[&str]) -> Option<serde_yaml::Value> {
    let mut current = &validated.parameters;

    if keys.is_empty() {
        return Some(current.clone());
    }

    for key in keys {
        if let serde_yaml::Value::Mapping(map) = current {
            let yml_key = serde_yaml::Value::String((*key).to_string());
            if let Some(next) = map.get(&yml_key) {
                current = next;
            } else {
                return None;
            }
        } else {
            return None;
        }
    }
    Some(current.clone())
}

/// Scans the target configuration file and injects an empty parameters block if missing.
pub fn add_empty_parameters_block(project_root: &Utf8Path) -> anyhow::Result<bool> {
    let Some(yml_path) = base_configuration_path(project_root)? else {
        return Ok(false);
    };

    let content = fs::read_to_string(&yml_path).context("Failed to read project configuration.")?;
    let raw_value: serde_yaml::Value =
        serde_yaml::from_str(&content).context("Failed to parse project configuration.")?;

    if let serde_yaml::Value::Mapping(mut map) = raw_value {
        let aliases = ["parameters", "parameter", "params", "param"];
        let found = aliases
            .iter()
            .any(|&a| map.contains_key(&serde_yaml::Value::String(a.to_string())));

        if found {
            return Ok(false);
        }

        map.insert(
            serde_yaml::Value::String("parameters".to_string()),
            serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
        );

        let new_content =
            serde_yaml::to_string(&map).context("Failed to serialize configuration.")?;
        fs::write(&yml_path, new_content).context("Failed to write configuration.")?;
        Ok(true)
    } else {
        let mut map = serde_yaml::Mapping::new();
        map.insert(
            serde_yaml::Value::String("parameters".to_string()),
            serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
        );
        let new_content =
            serde_yaml::to_string(&map).context("Failed to serialize new configuration.")?;
        fs::write(&yml_path, new_content).context("Failed to write updated configuration.")?;
        Ok(true)
    }
}

pub fn yml_read(is_dev: bool) -> anyhow::Result<ValidatedConfig> {
    let project_root = root().context("Could not find project root.")?;
    let project_root =
        Utf8PathBuf::try_from(project_root).context("Project root is not valid UTF-8.")?;
    yml_read_from(&project_root, is_dev)
}

pub fn yml_get() -> String {
    "projr yml content".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_yml_get() {
        assert_eq!(yml_get(), "projr yml content");
    }

    #[test]
    fn test_git_config_opt_parsing() {
        let yaml_bool_true = "
directories: {}
build:
  git: true
";
        let config_true: ProjConfig = serde_yaml::from_str(yaml_bool_true).unwrap();
        assert_eq!(config_true.build.git, GitConfigOpt::Boolean(true));

        let yaml_bool_false = "
directories: {}
build:
  git: false
";
        let config_false: ProjConfig = serde_yaml::from_str(yaml_bool_false).unwrap();
        assert_eq!(config_false.build.git, GitConfigOpt::Boolean(false));

        let yaml_detailed = "
directories: {}
build:
  git:
    commit: true
    push: false
";
        let config_detailed: ProjConfig = serde_yaml::from_str(yaml_detailed).unwrap();
        assert_eq!(
            config_detailed.build.git,
            GitConfigOpt::Detailed(GitConfig {
                commit: Some(true),
                push: Some(false)
            })
        );
    }
}

#[cfg(test)]
mod parameter_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_parameter_aliases() {
        let yaml_params = "
params:
  key1: val1
";
        let config: ProjConfig = serde_yaml::from_str(yaml_params).unwrap();
        assert_eq!(config.parameters["key1"], "val1");
    }

    #[test]
    fn test_multiple_aliases_conflict() {
        let temp = TempDir::new().unwrap();
        let root_path = temp.path();
        fs::write(root_path.join("VERSION"), "v1.0.0").unwrap();
        let yaml_conflict = "
parameters:
  key1: val1
params:
  key2: val2
";
        fs::write(root_path.join("_proj.yml"), yaml_conflict).unwrap();
        let result = yml_read_from(camino::Utf8Path::from_path(root_path).unwrap(), false);
        assert!(result.is_err());
    }
}
