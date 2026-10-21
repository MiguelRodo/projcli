//! Selection of a stable project root for a CLI invocation and its children.

use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};

/// Absolute project root inherited by child CLI invocations during a managed run.
///
/// Explicit `--project` selection overrides this value. The variable pins root
/// identity, not the child's working directory or renderer configuration.
///
/// ```rust
/// assert_eq!(proj::PROJECT_ROOT_ENV, "PROJ_PROJECT_ROOT");
/// ```
pub const PROJECT_ROOT_ENV: &str = "PROJ_PROJECT_ROOT";

/// Selects an explicit directory or resolves the inherited/discovered project root.
///
/// An explicit directory needs no VERSION or configuration marker. Otherwise
/// the existing discovery policy is used. Missing directories, files used as
/// directories and non-UTF-8 paths return errors. The result is absolute.
///
/// ```rust
/// let folder = tempfile::tempdir().unwrap();
/// let path = camino::Utf8Path::from_path(folder.path()).unwrap();
/// let root = proj::resolve_project_root(Some(path)).unwrap();
/// assert!(root.is_absolute());
/// ```
pub fn resolve_project_root(explicit: Option<&Utf8Path>) -> anyhow::Result<Utf8PathBuf> {
    let candidate = match explicit {
        Some(path) => path.to_owned(),
        None => crate::run_context::inherited_root()?.or_else(crate::ignore::root).context("Could not find project root")?,
    };
    let absolute = std::fs::canonicalize(&candidate)
        .with_context(|| format!("Cannot select project directory '{}'", candidate))?;
    anyhow::ensure!(absolute.is_dir(), "Project path '{}' is not a directory", candidate);
    Utf8PathBuf::from_path_buf(absolute)
        .map_err(|_| anyhow::anyhow!("Project root is not valid UTF-8"))
}
