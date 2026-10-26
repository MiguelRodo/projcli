use serde::{Serialize, Deserialize};
use regex::Regex;
use std::fs;
use crate::ignore::root;
use camino::Utf8Path;
use anyhow::Context;

/// Represents the project version separated into standard semantic components.
///
/// This structure holds the `major`, `minor`, `patch`, and an optional `dev` field.
/// The `dev` field accommodates build cycles or pre-release versioning natively supported
/// by certain runtime dependencies.
///
/// # Errors
///
/// Serialization and deserialization operations might fail if the input JSON
/// doesn't contain valid numbers for the corresponding fields.
///
/// ```rust
/// use proj::version::ProjVersion;
/// let version = ProjVersion { major: 1, minor: 2, patch: 3, dev: 0 };
/// assert_eq!(version.major, 1);
/// ```
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ProjVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    #[serde(default)]
    pub dev: u32,
}

impl ProjVersion {
    /// Formats the version as a standard string identifier.
    ///
    /// It can optionally prefix the string with a "v" character. The `dev` component
    /// is included only if it is strictly greater than 0.
    ///
    /// # Errors
    ///
    /// This method is infallible and should not produce an error under normal memory conditions.
    ///
    /// ```rust
    /// use proj::version::ProjVersion;
    /// let version = ProjVersion { major: 1, minor: 2, patch: 3, dev: 4 };
    /// assert_eq!(version.to_string(true), "v1.2.3.4");
    /// assert_eq!(version.to_string(false), "1.2.3.4");
    /// ```
    pub fn to_string(&self, include_v: bool) -> String {
        let prefix = if include_v { "v" } else { "" };
        if self.dev > 0 {
            format!("{}{}.{}.{}.{}", prefix, self.major, self.minor, self.patch, self.dev)
        } else {
            format!("{}{}.{}.{}", prefix, self.major, self.minor, self.patch)
        }
    }

    /// Canonical versioned-cache key: leading v and a hyphenated dev component.
    /// Existing version text APIs retain their accepted dotted spelling.
    /// ```rust
    /// let version = proj::ProjVersion { major: 1, minor: 2, patch: 3, dev: 4 };
    /// assert_eq!(version.cache_key(), "v1.2.3-4");
    /// ```
    pub fn cache_key(&self) -> String {
        let release = format!("v{}.{}.{}", self.major, self.minor, self.patch);
        if self.dev > 0 { format!("{release}-{}", self.dev) } else { release }
    }

    /// Parses a raw string into a structured `ProjVersion`.
    ///
    /// This handles multiple variations of string formatting, including JSON formatted
    /// structures and various text formats like "v1.2.3.4", "v1.2.3-4", and "1.2.3".
    ///
    /// # Errors
    ///
    /// If the input string cannot be recognized as a valid JSON object or a regex
    /// formatted string, it returns `None`.
    ///
    /// ```rust
    /// use proj::version::ProjVersion;
    /// let version = ProjVersion::parse("v1.2.3.4").unwrap();
    /// assert_eq!(version.major, 1);
    /// assert_eq!(version.dev, 4);
    /// ```
    pub fn parse(s: &str) -> Option<Self> {
        // Try JSON first
        if s.trim().starts_with('{') {
            if let Ok(version) = serde_json::from_str::<ProjVersion>(s) {
                return Some(version);
            }
        }

        // Try regex for string format (e.g. "Version: v1.2.3.4", "v1.2.3-4", "1.2.3")
        let re = Regex::new(r"(?i)(?:Version:\s*)?v?(\d+)\.(\d+)\.(\d+)(?:[.\-](\d+))?").ok()?;
        let caps = re.captures(s.trim())?;

        let major: u32 = caps.get(1)?.as_str().parse().ok()?;
        let minor: u32 = caps.get(2)?.as_str().parse().ok()?;
        let patch: u32 = caps.get(3)?.as_str().parse().ok()?;
        let dev: u32 = caps.get(4).map_or(0, |m| m.as_str().parse().unwrap_or(0));

        Some(ProjVersion {
            major,
            minor,
            patch,
            dev,
        })
    }
}

/// Retrieves the current project version from the `VERSION` file.
///
/// Scans upward from the current working directory to locate the project root containing
/// the `VERSION` file. If found, it reads the content and parses it into a `ProjVersion`.
///
/// # Errors
///
/// Returns `None` if the project root cannot be detected, the `VERSION` file is unreadable,
/// or the file content cannot be parsed successfully.
///
/// ```rust
/// use std::fs;
/// use tempfile::TempDir;
/// use camino::Utf8Path;
/// use proj::version::{ProjVersion, version_get_from};
///
/// let temp = TempDir::new().unwrap();
/// let temp_utf8 = Utf8Path::from_path(temp.path()).unwrap();
/// let version_path = temp_utf8.join("VERSION");
/// fs::write(&version_path, "Version: v1.0.0").unwrap();
///
/// let version = version_get_from(&temp_utf8).unwrap();
/// assert_eq!(version.major, 1);
/// assert_eq!(version.minor, 0);
/// assert_eq!(version.patch, 0);
/// ```
pub fn version_get_from(root: &Utf8Path) -> Option<ProjVersion> {
    let version_file_path = root.join("VERSION");
    let content = fs::read_to_string(version_file_path).ok()?;
    ProjVersion::parse(&content)
}

/// Thin production wrapper: Retrieves the current project version from the `VERSION` file using implicit system environment to find root.
///
/// # Errors
///
/// Returns `None` if the root cannot be detected or if the `VERSION` file is invalid.
///
/// ```rust,ignore
/// use proj::version::version_get;
/// let version = version_get();
/// ```
pub fn version_get() -> Option<ProjVersion> {
    let root = root()?;
    version_get_from(&root)
}

/// Updates the current project version globally using a specified root.
///
/// Writes the new version into the `VERSION` file situated at the resolved project root.
/// The input format can be structured JSON or a standard literal version string.
///
/// # Errors
///
/// Returns an `anyhow::Result` if the input format is invalid,
/// or the `VERSION` file cannot be written to.
///
/// ```rust
/// use std::fs;
/// use tempfile::TempDir;
/// use camino::Utf8Path;
/// use proj::version::{ProjVersion, version_set_at};
///
/// let temp = TempDir::new().unwrap();
/// let canonical_root = temp.path().canonicalize().unwrap();
/// let temp_utf8 = Utf8Path::from_path(&canonical_root).unwrap();
/// let version_path = temp_utf8.join("VERSION");
/// fs::write(&version_path, "v1.0.0").unwrap();
///
/// version_set_at(&temp_utf8, "v1.2.0").unwrap();
/// let new_version = fs::read_to_string(&version_path).unwrap();
/// assert_eq!(new_version, "Version: v1.2.0");
/// ```
pub fn version_set_at(root: &Utf8Path, version_str: &str) -> anyhow::Result<()> {
    let combined = crate::yml::merged_configuration_from(root, None, false)?;
    let settings: crate::yml::SettingsConfig = combined.get("settings")
        .cloned().map(serde_json::from_value).transpose()?.unwrap_or_default();
    let version = ProjVersion::parse(version_str)
        .with_context(|| format!("Could not parse version: {version_str}"))?;
    diagnose_description(root, settings.sync_description_version.unwrap_or(true))?;
    version_set_at_with_sync(root, &version, settings.sync_description_version.unwrap_or(true))

}

/// Validate the existing DESCRIPTION and prepare only its Version field.
/// Other bytes (including line endings, field order and continuation text) survive.
pub(crate) fn description_bytes(root: &Utf8Path, version: &ProjVersion) -> anyhow::Result<Option<Vec<u8>>> {
    let path = root.join("DESCRIPTION");
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let text = crate::fs_utils::read_managed_text(&path)?;
    let mut output = String::new();
    let mut found = false;
    let mut previous_version = false;
    let mut has_field = false;
    let mut separator = None;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        if body.starts_with([' ', '\t']) {
            anyhow::ensure!(has_field && separator.is_none(), "DESCRIPTION has an orphan continuation line");
            anyhow::ensure!(!previous_version, "DESCRIPTION Version must be a single-line field");
        } else if !body.is_empty() {
            anyhow::ensure!(separator.is_none(), "DESCRIPTION must contain one DCF record");
            has_field = true;
            let (key, _) = body.split_once(':').context("Invalid DESCRIPTION field: expected name: value")?;
            anyhow::ensure!(!key.is_empty() && key.chars().all(|c| !c.is_whitespace() && !c.is_control()),
                "Invalid DESCRIPTION field name");
            previous_version = key == "Version";
            if previous_version {
                anyhow::ensure!(!found, "DESCRIPTION has duplicate Version fields");
                found = true;
                output.push_str("Version: ");
                output.push_str(&version.to_string(false));
                output.push_str(if line.ends_with("\r\n") { "\r\n" } else if line.ends_with('\n') { "\n" } else { "" });
                continue;
            }
        } else {
            if has_field && separator.is_none() { separator = Some(output.len()); }
            previous_version = false;
        }
        output.push_str(line);
    }
    anyhow::ensure!(has_field, "DESCRIPTION contains no fields");
    if !found {
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let at = separator.unwrap_or(output.len());
        let prefix = if at>0 && !output[..at].ends_with('\n') { newline } else { "" };
        output.insert_str(at, &format!("{prefix}Version: {}{newline}", version.to_string(false)));
    }
    Ok(Some(output.into_bytes()))
}

/// Check description structure before hooks; VERSION wins when two valid versions differ.
pub(crate) fn diagnose_description(root: &Utf8Path, sync: bool) -> anyhow::Result<()> {
    if !sync { return Ok(()); }
    let expected = version_get_from(root).unwrap_or(ProjVersion {major:0,minor:0,patch:1,dev:0});
    if description_bytes(root, &expected)?.is_some() {
        let text = crate::fs_utils::read_managed_text(root.join("DESCRIPTION"))?;
        if let Some(current) = text.lines().find_map(|line| line.strip_prefix("Version:")) {
            if let Some(authority) = version_get_from(root) {
                if ProjVersion::parse(current) != Some(authority) {
                    eprintln!("DESCRIPTION Version differs from VERSION; VERSION is authoritative and production writes will synchronize it (settings.sync-description-version=false disables this).");
                }
            }
        }
    }
    Ok(())
}

/// Write VERSION and optionally an existing DESCRIPTION using frozen build policy.
/// Two guarded replacements have exact local failure recovery, not crash atomicity.
pub(crate) fn version_set_at_with_sync(root: &Utf8Path, version: &ProjVersion, sync: bool) -> anyhow::Result<()> {
    let description = if sync { description_bytes(root, version)? } else { None };
    let path = root.join("VERSION");
    let mut targets = vec![path.clone().into_std_path_buf()];
    if description.is_some() { targets.push(root.join("DESCRIPTION").into_std_path_buf()); }
    let backup = crate::build_state::BuildState::capture(&[], &targets)?;
    let result = (|| {
        if let Some(bytes) = description {
            crate::fs_utils::replace_managed_file(&root.join("DESCRIPTION"), bytes)?;
        }
        if path.exists() {
            crate::fs_utils::replace_managed_file(&path, version_file_content(version))?;
        } else {
            use std::io::Write;
            crate::path_safety::resolve_path(path.as_std_path(), true)?;
            let mut staged = tempfile::NamedTempFile::new_in(root)?;
            staged.write_all(version_file_content(version).as_bytes())?;
            staged.persist_noclobber(&path).map_err(|error| error.error)?;
        }
        Ok(())
    })();
    match result { Ok(()) => Ok(()), Err(error) => Err(backup.restore(error)) }
}

/// Thin production wrapper: Updates the current project version globally using implicit system environment to find root.
///
/// # Errors
///
/// Returns an error if the project root cannot be resolved, or if the write fails.
///
/// ```rust,ignore
/// use proj::version::version_set;
/// version_set("1.0.0").unwrap();
/// ```
pub fn version_set(version_str: &str) -> anyhow::Result<()> {
    let root = root().context("Could not find project root containing VERSION file")?;
    version_set_at(&root, version_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_projversion_parse() {
        let v1 = ProjVersion::parse("Version: v1.2.3.4").unwrap();
        assert_eq!(v1.major, 1);
        assert_eq!(v1.minor, 2);
        assert_eq!(v1.patch, 3);
        assert_eq!(v1.dev, 4);

        let v2 = ProjVersion::parse("v1.2.3-4").unwrap();
        assert_eq!(v2.major, 1);
        assert_eq!(v2.minor, 2);
        assert_eq!(v2.patch, 3);
        assert_eq!(v2.dev, 4);

        let v3 = ProjVersion::parse("1.2.3").unwrap();
        assert_eq!(v3.major, 1);
        assert_eq!(v3.minor, 2);
        assert_eq!(v3.patch, 3);
        assert_eq!(v3.dev, 0);

        let v4 = ProjVersion::parse(r#"{"major":1,"minor":2,"patch":3,"dev":5}"#).unwrap();
        assert_eq!(v4.major, 1);
        assert_eq!(v4.minor, 2);
        assert_eq!(v4.patch, 3);
        assert_eq!(v4.dev, 5);
    }

    #[test]
    fn test_projversion_to_string() {
        let v = ProjVersion { major: 1, minor: 2, patch: 3, dev: 4 };
        assert_eq!(v.to_string(true), "v1.2.3.4");
        assert_eq!(v.to_string(false), "1.2.3.4");

        let v2 = ProjVersion { major: 1, minor: 2, patch: 3, dev: 0 };
        assert_eq!(v2.to_string(true), "v1.2.3");
        assert_eq!(v2.to_string(false), "1.2.3");
    }
}

pub(crate) fn version_file_content(version: &ProjVersion) -> String {
    format!("Version: {}", version.to_string(true))
}
