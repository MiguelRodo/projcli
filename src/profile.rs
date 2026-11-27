//! projr configuration-file layout and profile selection.
//!
//! The base configuration is `_projr.yml` at the project root or in
//! `.projr/config/` (also `_projr/config/`), where the short names
//! `default.yml`, `<profile>.yml` and `local.yml` are accepted beside the
//! `_projr-<profile>.yml` spelling. Root files take precedence
//! (SATVILab/projr@5f915ef `R/yml.R:308-338`).
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use serde_json::Value;
use std::env;
use std::fs;

/// Configuration directories searched after the project root, in order.
pub const CONFIG_DIRS: [&str; 2] = [".projr/config", "_projr/config"];

/// Extracts and normalizes profile tags from PROJR_PROFILE environment variable.
///
/// Splits by ',' or ';', strips whitespace, and rejects 'default' and 'local'.
pub fn get_active_profiles() -> Vec<String> {
    parse_active_profiles(&env::var("PROJR_PROFILE").unwrap_or_default())
}

pub(crate) fn parse_active_profiles(value: &str) -> Vec<String> {
    value
        .split([',', ';'])
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && *value != "default"
                && *value != "local"
                && is_valid_profile_name(value)
        })
        .map(str::to_owned)
        .collect()
}

fn is_valid_profile_name(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Root-level file name of a profile: `_projr.yml` for `default`, else `_projr-<profile>.yml`.
fn root_file_name(profile: &str) -> String {
    if profile == "default" {
        "_projr.yml".to_owned()
    } else {
        format!("_projr-{profile}.yml")
    }
}

/// Locates the existing configuration file of a profile (`default`, `local` or a name).
///
/// The root file wins over `.projr/config/` and then `_projr/config/`. Inside a
/// configuration directory, `<profile>.yml` (`default.yml` for the base file) is
/// accepted beside `_projr-<profile>.yml`; both present in one directory is an
/// error, as in projr. Returns `None` when no file exists.
///
/// ```rust
/// let project = tempfile::tempdir().unwrap();
/// let root = camino::Utf8Path::from_path(project.path()).unwrap();
/// std::fs::create_dir_all(root.join(".projr/config")).unwrap();
/// std::fs::write(root.join(".projr/config/default.yml"), "").unwrap();
/// let found = proj::profile::existing_config(root, "default").unwrap().unwrap();
/// assert!(found.ends_with(".projr/config/default.yml"));
/// std::fs::write(root.join("_projr.yml"), "").unwrap();
/// assert_eq!(proj::profile::existing_config(root, "default").unwrap().unwrap(), root.join("_projr.yml"));
/// ```
pub fn existing_config(root: &Utf8Path, profile: &str) -> anyhow::Result<Option<Utf8PathBuf>> {
    let filename = root_file_name(profile);
    let mut paths = vec![root.join(&filename)];
    for dir in CONFIG_DIRS {
        let mut present = Vec::new();
        for name in [filename.clone(), format!("{profile}.yml")] {
            let path = root.join(dir).join(name);
            if crate::yml::configuration_entry_exists(&path)? {
                present.push(path);
            }
        }
        if let [first, second] = present.as_slice() {
            anyhow::bail!(
                "Both {} and {} exist in {dir}/. Keep one.",
                first.file_name().unwrap_or_default(),
                second.file_name().unwrap_or_default()
            );
        }
        paths.extend(present);
    }
    for path in paths {
        if crate::yml::configuration_entry_exists(&path)? {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// The file a profile is read from, or the root file new configuration is created at.
pub fn config_path(root: &Utf8Path, profile: &str) -> anyhow::Result<Utf8PathBuf> {
    Ok(existing_config(root, profile)?.unwrap_or_else(|| root.join(root_file_name(profile))))
}

/// Project-relative configuration files: root `_projr*.yml` and every
/// `[A-Za-z0-9_-]+.yml` in the configuration directories (projr `.yml_config_files`).
pub fn config_files(root: &Utf8Path) -> anyhow::Result<Vec<String>> {
    let mut files = Vec::new();
    for (dir, root_level) in [("", true), (CONFIG_DIRS[0], false), (CONFIG_DIRS[1], false)] {
        let directory = if root_level {
            root.to_owned()
        } else {
            root.join(dir)
        };
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| format!("Cannot list {directory}")),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.with_context(|| format!("Cannot list {directory}"))?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let matches = if root_level {
                name == "_projr.yml"
                    || name
                        .strip_prefix("_projr-")
                        .and_then(|stem| stem.strip_suffix(".yml"))
                        .is_some_and(|stem| !stem.is_empty() && is_valid_profile_name(stem))
            } else {
                name.strip_suffix(".yml")
                    .is_some_and(|stem| !stem.is_empty() && is_valid_profile_name(stem))
            };
            if matches {
                names.push(if root_level {
                    name
                } else {
                    format!("{dir}/{name}")
                });
            }
        }
        names.sort();
        files.extend(names);
    }
    Ok(files)
}

/// The profile a configuration file belongs to: `default`, `local` or a profile name.
///
/// ```rust
/// use proj::profile::config_file_profile;
/// assert_eq!(config_file_profile("_projr.yml"), "default");
/// assert_eq!(config_file_profile(".projr/config/local.yml"), "local");
/// assert_eq!(config_file_profile("_projr-local.yml"), "local");
/// ```
pub fn config_file_profile(file: &str) -> String {
    let name = file.rsplit('/').next().unwrap_or(file);
    let stem = name.strip_suffix(".yml").unwrap_or(name);
    match stem {
        "_projr" => "default".to_owned(),
        _ => stem.strip_prefix("_projr-").unwrap_or(stem).to_owned(),
    }
}

/// Whether a directory holds a projr base configuration (projr's root criterion).
///
/// `_projr.yml` counts unless the directory is itself a configuration directory;
/// `.projr/config/` or `_projr/config/` holding `_projr.yml` or `default.yml` also counts.
pub(crate) fn has_base_config(path: &Utf8Path) -> bool {
    let is_config_dir = path.file_name() == Some("config")
        && path
            .parent()
            .and_then(Utf8Path::file_name)
            .is_some_and(|name| name == ".projr" || name == "_projr");
    (!is_config_dir && path.join("_projr.yml").is_file())
        || CONFIG_DIRS.iter().any(|dir| {
            ["_projr.yml", "default.yml"]
                .iter()
                .any(|name| path.join(dir).join(name).is_file())
        })
}

/// Profiles named by the `profile` key, split by kind.
///
/// projr accepts a string, a list of strings, or a mapping with `projr` and
/// `quarto` entries (`R/profile.R:238-270`). `profile.renv` is an error.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProfileKey {
    /// projr configuration profiles.
    pub projr: Vec<String>,
    /// Quarto render profiles, applied when `QUARTO_PROFILE` is unset.
    pub quarto: Vec<String>,
}

/// Parses and validates a `profile` key value.
///
/// # Errors
///
/// Fails for `profile.renv`, unknown mapping keys and non-string entries.
///
/// ```rust
/// use proj::profile::parse_profile_key;
/// let key = parse_profile_key(&serde_json::json!({"projr": "a", "quarto": ["print"]})).unwrap();
/// assert_eq!((key.projr, key.quarto), (vec!["a".to_owned()], vec!["print".to_owned()]));
/// assert!(parse_profile_key(&serde_json::json!({"renv": "x"})).is_err());
/// ```
pub fn parse_profile_key(value: &Value) -> anyhow::Result<ProfileKey> {
    fn strings(value: &Value, name: &str) -> anyhow::Result<Vec<String>> {
        let invalid = || anyhow::anyhow!("{name} must be a string or a list of strings");
        match value {
            Value::Null => Ok(Vec::new()),
            Value::String(text) => Ok(vec![text.clone()]),
            Value::Array(items) => items
                .iter()
                .map(|item| item.as_str().map(str::to_owned).ok_or_else(invalid))
                .collect(),
            _ => Err(invalid()),
        }
    }
    let Value::Object(map) = value else {
        return Ok(ProfileKey {
            projr: strings(value, "profile")?,
            quarto: Vec::new(),
        });
    };
    anyhow::ensure!(
        !map.contains_key("renv"),
        "profile.renv is not supported yet."
    );
    if let Some(key) = map
        .keys()
        .find(|key| !["projr", "quarto"].contains(&key.as_str()))
    {
        anyhow::bail!("Unknown profile key '{key}'; use projr or quarto");
    }
    let get = |kind: &str| {
        map.get(kind).map_or(Ok(Vec::new()), |value| {
            strings(value, &format!("profile.{kind}"))
        })
    };
    Ok(ProfileKey {
        projr: get("projr")?,
        quarto: get("quarto")?,
    })
}

/// Profiles from the `profile` key of the local file, else the default file.
///
/// Each kind is taken from the first of the two files that names one, as projr's
/// `.profile_config` does. Named profile files cannot set the key.
pub(crate) fn configured_profiles(root: &Utf8Path) -> anyhow::Result<ProfileKey> {
    let mut selected = ProfileKey::default();
    for file in ["default", "local"] {
        let Some(path) = existing_config(root, file)? else {
            continue;
        };
        let value = crate::yml::read_configuration_file(&path)?;
        if let Some(key) = value.get("profile") {
            let key =
                parse_profile_key(key).with_context(|| format!("Invalid profile key in {path}"))?;
            if !key.projr.is_empty() {
                selected.projr = key.projr;
            }
            if !key.quarto.is_empty() {
                selected.quarto = key.quarto;
            }
        }
    }
    Ok(selected)
}

/// Spawns a clean `_projr-<name>.yml` workspace node.
pub fn create_profile(name: &str, project_root: &Utf8Path) -> anyhow::Result<Utf8PathBuf> {
    if name == "default" || name == "local" || !is_valid_profile_name(name) {
        anyhow::bail!("Invalid profile name: {}", name);
    }
    let path = config_path(project_root, name)?;
    if path.exists() {
        anyhow::bail!("Profile {} already exists.", name);
    }
    fs::write(&path, "").with_context(|| format!("Failed to write empty profile to {}", path))?;
    Ok(path)
}

/// Recursively sets all leaf values to null
fn nullify_values(val: &mut Value) {
    match val {
        Value::Object(map) => {
            for (_, v) in map.iter_mut() {
                nullify_values(v);
            }
        }
        _ => {
            *val = Value::Null;
        }
    }
}

/// Scans the base `_projr.yml` structure and clones its layout into `_projr-local.yml` with all assignments mapped to null.
pub fn create_local_profile(project_root: &Utf8Path) -> anyhow::Result<Utf8PathBuf> {
    let baseline_path = crate::yml::base_configuration_path(project_root)?;
    let local_path = config_path(project_root, "local")?;

    if local_path.exists() {
        anyhow::bail!("Local profile {local_path} already exists.");
    }

    if let Some(baseline_path) = baseline_path {
        let content = fs::read_to_string(&baseline_path).with_context(|| {
            format!(
                "Failed to read baseline configuration from {}",
                baseline_path
            )
        })?;
        let yaml_value: serde_yaml::Value = serde_yaml::from_str(&content)
            .context("Failed to parse baseline YAML configuration")?;

        // Convert to serde_json::Value for processing
        let mut json_value: serde_json::Value = serde_json::to_value(yaml_value)
            .context("Failed to convert YAML configuration to JSON for nullification")?;
        nullify_values(&mut json_value);

        let out_yaml: serde_yaml::Value = serde_json::from_value(json_value)
            .context("Failed to convert nullified JSON back to YAML")?;
        let out_str =
            serde_yaml::to_string(&out_yaml).context("Failed to serialize local profile YAML")?;
        fs::write(&local_path, out_str)
            .with_context(|| format!("Failed to write local profile to {}", local_path))?;
    } else {
        fs::write(&local_path, "")
            .with_context(|| format!("Failed to write empty local profile to {}", local_path))?;
    }

    Ok(local_path)
}

/// Permanently deletes the matching config node.
pub fn delete_profile(name: &str, project_root: &Utf8Path) -> anyhow::Result<()> {
    match existing_config(project_root, name)? {
        Some(path) => {
            fs::remove_file(&path)
                .with_context(|| format!("Failed to delete profile file at {}", path))?;
            Ok(())
        }
        None => anyhow::bail!("Profile {} does not exist.", name),
    }
}

/// projr's starter configuration (`inst/project_structure/_projr.yml`).
pub(crate) const STARTER: &str = "directories:\n  raw-data:\n    path: _raw_data\n  cache:\n    path: _tmp\n  output:\n    path: _output\n  docs:\n    path: docs\n";

/// Writes projr's starter `_projr.yml` when no base configuration exists.
///
/// As in projr (`R/build.R:289-310`), Git is disabled outside a repository and
/// pushing is disabled without a tracking upstream. An existing file is never
/// overwritten, even one created concurrently. Returns whether it was written.
pub(crate) fn write_starter(
    root: &Utf8Path,
    environment: &crate::run_environment::RunEnvironment,
) -> anyhow::Result<bool> {
    if crate::yml::base_configuration_path(root)?.is_some() {
        return Ok(false);
    }
    // projcli looks for `.git` in ancestors too, as its Git publication does.
    let git = if !root
        .ancestors()
        .any(|directory| directory.join(".git").exists())
    {
        "build:\n  git: false\n"
    } else if crate::git::tracking_target_in(root, environment).is_err() {
        "build:\n  git:\n    push: false\n"
    } else {
        ""
    };
    let mut file = tempfile::NamedTempFile::new_in(root.as_std_path())
        .context("Cannot stage the starter configuration")?;
    std::io::Write::write_all(&mut file, format!("{STARTER}{git}").as_bytes())?;
    file.persist_noclobber(root.join("_projr.yml"))
        .map_err(|error| error.error)
        .context("Failed to create _projr.yml without overwriting existing configuration")?;
    // projr seeds the ignore starter with the configuration, previews included.
    crate::ignore::seed_ignore_starter(root)?;
    eprintln!("Created a starter _projr.yml; edit it to declare this project's directories.");
    Ok(true)
}
