//! Logical safe targets are separate from configured final destinations.
use camino::{Utf8Path, Utf8PathBuf};

/// Resolves a declared resource label and optional file/subdirectory fragments.
/// Safe output/data/docs targets use the first declared cache-class mapping and
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
