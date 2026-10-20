use std::env;
use std::fs;
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use serde_json::Value;

/// Extracts and normalizes profile tags from PROJR_PROFILE environment variable.
///
/// Splits by ',' or ';', strips whitespace, and rejects 'default' and 'local'.
pub fn get_active_profiles() -> Vec<String> {
    if let Ok(val) = env::var("PROJR_PROFILE") {
        val.split(|c| c == ',' || c == ';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != "default" && s != "local" && is_valid_profile_name(s))
            .collect()
    } else {
        Vec::new()
    }
}

fn is_valid_profile_name(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Spawns a clean `_projr-<name>.yml` workspace node.
pub fn create_profile(name: &str, project_root: &Utf8Path) -> anyhow::Result<Utf8PathBuf> {
    if name == "default" || name == "local" || !is_valid_profile_name(name) {
        anyhow::bail!("Invalid profile name: {}", name);
    }
    let filename = format!("_projr-{}.yml", name);
    let path = project_root.join(filename);
    if path.exists() {
        anyhow::bail!("Profile {} already exists.", name);
    }
    fs::write(&path, "")
        .with_context(|| format!("Failed to write empty profile to {}", path))?;
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
        Value::Array(_arr) => {
            // Arrays are typically replaced entirely, but if we need structural clone, we just nullify elements
            // Actually, for local profile it might be easier to just make it an empty array or null
            *val = Value::Null;
        }
        _ => {
            *val = Value::Null;
        }
    }
}

/// Scans the baseline `_proj.yml` structure and clones its layout into `_projr-local.yml` with all assignments mapped to null.
pub fn create_local_profile(project_root: &Utf8Path) -> anyhow::Result<Utf8PathBuf> {
    let baseline_path = crate::yml::base_configuration_path(project_root)?;
    let local_path = project_root.join("_projr-local.yml");

    if local_path.exists() {
        anyhow::bail!("Local profile _projr-local.yml already exists.");
    }

    if let Some(baseline_path) = baseline_path {
        let content = fs::read_to_string(&baseline_path)
            .with_context(|| format!("Failed to read baseline configuration from {}", baseline_path))?;
        let yaml_value: serde_yaml::Value = serde_yaml::from_str(&content)
            .context("Failed to parse baseline YAML configuration")?;

        // Convert to serde_json::Value for processing
        let mut json_value: serde_json::Value = serde_json::to_value(yaml_value)
            .context("Failed to convert YAML configuration to JSON for nullification")?;
        nullify_values(&mut json_value);

        let out_yaml: serde_yaml::Value = serde_json::from_value(json_value)
            .context("Failed to convert nullified JSON back to YAML")?;
        let out_str = serde_yaml::to_string(&out_yaml)
            .context("Failed to serialize local profile YAML")?;
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
    let filename = format!("_projr-{}.yml", name);
    let path = project_root.join(filename);
    if path.exists() {
        fs::remove_file(&path)
            .with_context(|| format!("Failed to delete profile file at {}", path))?;
        Ok(())
    } else {
        anyhow::bail!("Profile {} does not exist.", name);
    }
}
