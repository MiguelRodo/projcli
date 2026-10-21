//! Logical safe targets are separate from configured final destinations.
use camino::{Utf8Path, Utf8PathBuf};

/// Resolves a resource label and optional file/subdirectory fragments.
/// The built-in output-r-data has final target project/data, even without YAML;
/// safe lookup still requires a declared eligible cache and valid run version.
/// Safe output/data/docs targets use the selected eligible cache-class mapping and
/// captured active run version (or standalone VERSION). Other roles and safe=false
/// preserve configured paths, including absolute external mappings.
///
/// Queries never create paths. Absolute/parent-traversing fragments and undeclared
/// labels are errors; a fragment cannot replace or escape the selected role root.
pub fn resource_path_from(root: &Utf8Path, label: &str, fragments: &[Utf8PathBuf], safe: bool) -> anyhow::Result<Utf8PathBuf> {
    for fragment in fragments {
        anyhow::ensure!((cfg!(windows) || !fragment.as_str().contains('\\')) && !fragment.as_str().contains(':')
            && !fragment.as_str().chars().any(char::is_control), "Path fragments must be portable relative paths");
        anyhow::ensure!(fragment.components().all(|part| matches!(part, camino::Utf8Component::Normal(_) | camino::Utf8Component::CurDir)),
            "Path fragments must not be absolute or traverse parents");
    }
    let root = crate::resolve_project_root(Some(root))?;
    let mut path = crate::run_context::role_target_from(&root, label, safe)?;
    for fragment in fragments { path.push(fragment); }
    Ok(path)
}


pub(crate) fn is_generated_output(label: &str) -> bool {
    let label = label.to_ascii_lowercase();
    label.starts_with("output")
}

pub(crate) fn is_managed_output(label: &str) -> bool {
    is_generated_output(label) || label.to_ascii_lowercase().starts_with("docs")
}


pub(crate) fn select_safe_cache<'a>(directories: impl IntoIterator<Item = (&'a str, Option<bool>)>, order: &[String]) -> anyhow::Result<Option<String>> {
    let mut candidates = Vec::new();
    for (label, safe) in directories {
        if label.to_ascii_lowercase().starts_with("cache") { candidates.push((label, safe)); }
        else { anyhow::ensure!(safe.is_none(), "Directory safe selection is only supported for cache-class mappings"); }
    }
    if candidates.is_empty() { return Ok(None); }
    let selected: Vec<_> = candidates.iter().filter(|(_, safe)| *safe == Some(true)).collect();
    anyhow::ensure!(selected.len() <= 1, "Multiple cache directories explicitly select safe: true");
    if let Some((label, _)) = selected.first() { return Ok(Some((*label).to_string())); }
    let eligible: Vec<_> = candidates.iter().filter(|(_, safe)| *safe != Some(false)).collect();
    anyhow::ensure!(!eligible.is_empty(), "No eligible safe cache directory: every candidate has safe: false");
    if let Some((label, _)) = eligible.iter().find(|(label, _)| label.eq_ignore_ascii_case("cache")) { return Ok(Some((*label).to_string())); }
    if let Some(label) = order.iter().find(|label| eligible.iter().any(|(candidate, _)| *candidate == label.as_str())) { return Ok(Some(label.clone())); }
    // Typed HashMaps have no declaration order; this fallback is used only for validation.
    Ok(eligible.iter().map(|(label, _)| (*label).to_string()).min())
}

/// Representation is independent of the resource target and execution directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PathFormat {
    /// Caller-relative ordinarily, absolute for a known renderer; cross-root fallback is absolute.
    Auto,
    /// Always return an absolute filesystem path.
    Absolute,
    /// Strictly relative to the selected base; fail when a relative path is impossible.
    Relative,
}

/// Explicit base for relative filesystem paths, not a Markdown URL base override.
#[derive(Clone, Debug)]
pub enum RelativeBase {
    /// Effective calling process cwd at query time.
    Caller,
    /// Selected project root.
    Project,
    /// Parent of a document explicitly supplied by the renderer invocation.
    Document,
    /// A user-supplied absolute directory.
    Directory(Utf8PathBuf),
}

/// Presents an absolute resource target without changing what it identifies.
/// Auto observes only explicitly passed render context, never suffixes or cwd markers.
/// An explicit relative base is accepted only with Relative. Cross-drive/namespace
/// strict relative requests fail; Auto falls back to absolute when necessary.
pub fn present_resource_path(root: &Utf8Path, target: &Utf8Path, format: PathFormat, base: Option<&RelativeBase>) -> anyhow::Result<Utf8PathBuf> {
    use anyhow::Context;
    anyhow::ensure!(target.is_absolute(), "Resource target must be absolute");
    anyhow::ensure!(base.is_none() || format == PathFormat::Relative, "An explicit relative base requires --format relative");
    let target = normalize(target)?;
    let render = crate::run_context::render_context()?;
    if format == PathFormat::Absolute || (format == PathFormat::Auto && render.is_some()) { return Ok(target); }
    let cwd = std::env::current_dir()?;
    let cwd = Utf8Path::from_path(&cwd).context("Caller working directory must be UTF-8")?;
    let base = match base.unwrap_or(&RelativeBase::Caller) {
        RelativeBase::Caller => normalize(cwd)?,
        RelativeBase::Project => normalize(root)?,
        RelativeBase::Document => {
            let document = render.and_then(|context| context.document).context("Document-relative paths require an explicitly known document")?;
            normalize(document.parent().context("Render document has no parent")?)?
        }
        RelativeBase::Directory(path) => {
            anyhow::ensure!(path.is_absolute(), "Custom relative base must be absolute");
            anyhow::ensure!(!path.exists() || path.is_dir(), "Relative base must be a directory");
            normalize(path)?
        }
    };
    match relative_path(&target, &base) {
        Some(path) => Ok(path),
        None if format == PathFormat::Auto => Ok(target),
        None => anyhow::bail!("Cannot express a strict relative path across different roots/drives"),
    }
}

fn normalize(path: &Utf8Path) -> anyhow::Result<Utf8PathBuf> {
    let path = crate::path_safety::resolve_path(path.as_std_path(), false)?;
    Utf8PathBuf::from_path_buf(path).map_err(|_| anyhow::anyhow!("Resolved path must be UTF-8"))
}

fn relative_path(target: &Utf8Path, base: &Utf8Path) -> Option<Utf8PathBuf> {
    let target: Vec<_> = target.components().collect();
    let base: Vec<_> = base.components().collect();
    let common = target.iter().zip(&base).take_while(|(left, right)| left == right).count();
    if common == 0 { return None; }
    if base[common..].iter().any(|part| !matches!(part, camino::Utf8Component::Normal(_))) { return None; }
    let mut path = Utf8PathBuf::new();
    for _ in &base[common..] { path.push(".."); }
    for part in &target[common..] { path.push(part.as_str()); }
    if path.as_str().is_empty() { path.push("."); }
    Some(path)
}

#[cfg(test)]
mod representation_tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn different_windows_drives_have_no_strict_relative_path() {
        assert!(relative_path(Utf8Path::new("C:/target/file"),Utf8Path::new("D:/base")).is_none());
    }
    #[cfg(windows)]
    #[test]
    fn normalized_unc_share_roots_keep_share_identity() {
        assert_eq!(relative_path(Utf8Path::new(r"\\?\UNC\server\share\folder\file"),Utf8Path::new(r"\\?\UNC\server\share\")).unwrap(),Utf8PathBuf::from(r"folder\file"));
        assert!(relative_path(Utf8Path::new(r"\\?\UNC\server\share\folder"),Utf8Path::new(r"\\?\UNC\server\other\")).is_none());
    }
    #[cfg(unix)]
    #[test]
    fn ordinary_relative_paths_keep_parent_steps_and_identity() {
        assert_eq!(relative_path(Utf8Path::new("/project/_raw/image"),Utf8Path::new("/project/analysis")).unwrap(),Utf8PathBuf::from("../_raw/image"));
        assert_eq!(relative_path(Utf8Path::new("/project"),Utf8Path::new("/project")).unwrap(),Utf8PathBuf::from("."));
    }
}
