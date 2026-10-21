//! Captured path/version inputs shared by a build and its child CLI queries.
use std::{collections::BTreeMap, fs, io::{Read, Write}};
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use crate::{BuildMode, ProjVersion, yml::ValidatedConfig};

/// Per-command pointer to a private, versioned run-context snapshot.
/// Values contain resource paths and version inputs, never the full environment.
pub const RUN_CONTEXT_ENV: &str = "PROJ_RUN_CONTEXT";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunContext {
    schema_version: u32,
    root: Utf8PathBuf,
    profiles: Vec<String>,
    directories: BTreeMap<String, Utf8PathBuf>,
    active_version: ProjVersion,
    development: bool,
}

pub(crate) struct ContextFile(tempfile::NamedTempFile);

impl ContextFile {
    pub(crate) fn capture(root: &Utf8Path, config: &ValidatedConfig, profiles: Vec<String>, active_version: ProjVersion, development: bool) -> anyhow::Result<Self> {
        let directories = config.directories.keys().map(|label| Ok((label.clone(), config.get_path(root, label)?)))
            .collect::<anyhow::Result<_>>()?;
        let context = RunContext { schema_version: 1, root: root.to_owned(), profiles, directories, active_version, development };
        let temporary = std::env::temp_dir().canonicalize().context("Cannot resolve run-context temporary directory")?;
        let mut file = tempfile::NamedTempFile::new_in(temporary).context("Cannot create private run context")?;
        if let Ok(cache) = config.get_path(root, "cache") {
            let cache = crate::path_safety::resolve_path(cache.as_std_path(), true)?;
            anyhow::ensure!(!file.path().starts_with(cache), "Run context cannot be stored in disposable cache");
        }
        serde_json::to_writer(file.as_file_mut(), &context).context("Cannot write run context")?;
        file.flush().context("Cannot flush run context")?;
        Ok(Self(file))
    }

    pub(crate) fn path(&self) -> &std::path::Path { self.0.path() }
}

fn inherited() -> anyhow::Result<Option<RunContext>> {
    let Some(path) = std::env::var_os(RUN_CONTEXT_ENV) else { return Ok(None); };
    let path = std::path::PathBuf::from(path);
    anyhow::ensure!(path.is_absolute(), "Run context path must be absolute");
    let metadata = fs::symlink_metadata(&path).context("Cannot inspect inherited run context")?;
    anyhow::ensure!(metadata.is_file() && !metadata.file_type().is_symlink(), "Run context must be a regular file");
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(1_048_577).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 1_048_576, "Run context exceeds the supported size");
    let context: RunContext = serde_json::from_slice(&bytes).context("Invalid inherited run context")?;
    anyhow::ensure!(context.schema_version == 1, "Unsupported run-context schema");
    anyhow::ensure!(context.root.is_absolute() && context.root.is_dir(), "Invalid captured project root");
    for (label, path) in &context.directories {
        crate::path_safety::validate_directory_label(label)?;
        anyhow::ensure!(path.is_absolute(), "Captured directory paths must be absolute");
    }
    for profile in &context.profiles {
        anyhow::ensure!(normalized_profiles(profile)? == vec![profile.clone()], "Invalid captured profile list");
    }
    Ok(Some(context))
}

pub(crate) fn inherited_root() -> anyhow::Result<Option<Utf8PathBuf>> {
    Ok(inherited()?.map(|context| context.root))
}

fn inherited_for(root: &Utf8Path) -> anyhow::Result<Option<RunContext>> {
    let Some(context) = inherited()? else { return Ok(None); };
    // Explicit selection of another project discards this run's snapshot.
    let same = fs::canonicalize(root)? == fs::canonicalize(&context.root)?;
    Ok(if same { Some(context) } else { None })
}

/// Reads a configured final directory from the active run snapshot or standalone config.
/// This is not the logical safe-output resolver or a directory-creation operation.
/// Errors include invalid context, unknown labels and invalid configuration.
pub fn configured_path_from(root: &Utf8Path, label: &str) -> anyhow::Result<Utf8PathBuf> {
    crate::path_safety::validate_directory_label(label)?;
    if let Some(context) = inherited_for(root)? {
        return context.directories.into_iter().find(|(key, _)| key.eq_ignore_ascii_case(label))
            .map(|(_, path)| path).with_context(|| format!("Directory label '{label}' is not captured"));
    }
    let profiles = select_profiles(None, root)?;
    let combined = crate::yml::get_combined_yml(Some(&profiles.join(",")), root)?;
    let config: crate::yml::ProjConfig = serde_json::from_value(combined)?;
    config.validate_and_resolve(root, false)?.get_path(root, label)
}

/// Reads the active managed-run version, otherwise the selected root's VERSION.
/// Child queries do not reread mutable VERSION during an inherited run.
pub fn run_version_from(root: &Utf8Path) -> anyhow::Result<ProjVersion> {
    if let Some(context) = inherited_for(root)? { return Ok(context.active_version); }
    crate::version_get_from(root).context("Cannot read a valid project version")
}

pub(crate) fn select_profiles(explicit: Option<&str>, root: &Utf8Path) -> anyhow::Result<Vec<String>> {
    if let Some(value) = explicit { return normalized_profiles(value); }
    for key in ["PROJR_PROFILE", "QUARTO_PROFILE"] {
        if let Some(value) = std::env::var_os(key) {
            let value = value.to_str().context("Profile selection must be UTF-8")?;
            if !value.trim().is_empty() { return normalized_profiles(value); }
        }
    }
    let base = crate::yml::get_combined_yml(Some(""), root)?;
    match base.get("build").and_then(|build| build.get("profile")) {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::String(value)) => normalized_profiles(value),
        Some(_) => anyhow::bail!("build.profile must be a string"),
    }
}

fn normalized_profiles(value: &str) -> anyhow::Result<Vec<String>> {
    let mut profiles = Vec::new();
    for profile in value.split([',', ';']).map(str::trim).filter(|value| !value.is_empty()) {
        anyhow::ensure!(profile.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "Invalid active profile name");
        if ["default", "local", "required"].contains(&profile) { continue; }
        if !profiles.iter().any(|value| value == profile) { profiles.push(profile.to_owned()); }
    }
    Ok(profiles)
}

pub(crate) fn version_plan(mut version: ProjVersion, mode: BuildMode) -> anyhow::Result<(ProjVersion, ProjVersion)> {
    if mode == BuildMode::Dev {
        if version.dev == 0 { version.dev = 1; }
        return Ok((version.clone(), version));
    }
    match mode {
        BuildMode::ProdMajor => { version.major = version.major.checked_add(1).context("Major version overflow")?; version.minor = 0; version.patch = 0; }
        BuildMode::ProdMinor => { version.minor = version.minor.checked_add(1).context("Minor version overflow")?; version.patch = 0; }
        BuildMode::ProdPatch => { version.patch = version.patch.checked_add(1).context("Patch version overflow")?; }
        BuildMode::Dev => unreachable!(),
    }
    version.dev = 0;
    let mut next = version.clone(); next.dev = 1;
    Ok((version, next))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn active_and_next_versions_are_deliberate_and_checked() {
        let before = ProjVersion { major: 1, minor: 2, patch: 3, dev: 4 };
        let (active, next) = version_plan(before.clone(), BuildMode::ProdPatch).unwrap();
        assert_eq!((active.patch, active.dev, next.patch, next.dev), (4, 0, 4, 1));
        assert_eq!(version_plan(before.clone(), BuildMode::Dev).unwrap().0, before);
        assert_eq!(version_plan(ProjVersion { dev: 0, ..before }, BuildMode::Dev).unwrap().0.dev, 1);
        assert!(version_plan(ProjVersion { patch: u32::MAX, major: 0, minor: 0, dev: 0 }, BuildMode::ProdPatch).is_err());
    }
}
