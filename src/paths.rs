//! Logical safe targets are separate from configured final destinations.
use camino::{Utf8Path, Utf8PathBuf};

/// Resolves a declared resource label and optional file/subdirectory fragments.
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


pub(crate) fn is_output_or_data(label: &str) -> bool {
    let label = label.to_ascii_lowercase();
    label.starts_with("output") || label.starts_with("data")
}

pub(crate) fn is_managed_output(label: &str) -> bool {
    is_output_or_data(label) || label.to_ascii_lowercase().starts_with("docs")
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
