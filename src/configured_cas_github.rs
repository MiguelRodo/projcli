//! Configured CAS over complete GitHub ZIPs; Git metadata selects the trusted base.
//! Immutable exports preserve prior bytes. No distributed mutable head or GC.
use crate::{
    cas_github::Reference,
    cas_remote::{Placement, Style},
    catalogue::Format,
    configured_cas::SnapshotVerification,
    github_release::GithubReleaseClient,
    snapshot_format::{DirectoryRoot, FlatSnapshot, TreeSnapshot},
};
use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024 - 1;
const MAX_EXPANDED: u64 = 8 * 1024 * 1024 * 1024;
/// Validate transport-specific definition without credentials, I/O or helper execution.
pub(crate) fn definition(value: &Value) -> Result<crate::cas_remote::Definition> {
    anyhow::ensure!(
        value.get("structure").and_then(Value::as_str) == Some("cas"),
        "Expected structure: cas"
    );
    anyhow::ensure!(
        value.get("path").is_none(),
        "GitHub CAS uses immutable store asset names, not path prefixes"
    );
    anyhow::ensure!(
        value.get("manifest-style").is_some(),
        "GitHub CAS currently requires explicit manifest-style: tree or flat"
    );
    let mut wire = value.clone();
    wire["path"] = Value::String("unused-github-store".into());
    let policy = crate::cas_remote::Definition::parse(&wire)?;
    anyhow::ensure!(
        matches!(policy.manifest, Placement::Auto | Placement::Remote),
        "GitHub CAS currently requires remote full metadata"
    );
    Ok(policy)
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    schema: String,
    repository: String,
    api: String,
    reference: Reference,
}
impl Binding {
    fn check(&self, repository: &str, api: &str, tag: &str) -> Result<()> {
        self.reference.validate()?;
        anyhow::ensure!(
            self.schema == "projcli.cas-github-project.v1"
                && self.repository == repository
                && self.api == api
                && self.reference.tag == tag,
            "Trusted CAS reference disagrees with captured repository/API/tag; explicit migration required"
        );
        Ok(())
    }
}
/// Frozen destination; no Debug implementation because the client contains credentials.
pub(crate) struct Target {
    title: String,
    namespace: Utf8PathBuf,
    sources: Vec<(String, Utf8PathBuf)>,
    frozen: Option<Vec<u8>>,
    binding: Option<Binding>,
    client: GithubReleaseClient,
    style: Style,
    format: Format,
    repository: String,
    api: String,
    tag: String,
}
fn namespace(root: &Utf8Path, title: &str) -> Result<Utf8PathBuf> {
    let namespace = crate::cas_tree::native(
        &crate::cas_remote::project_metadata(root, title),
        "cas-github/v1",
    )?;
    crate::cas_tree::project_target(root, &namespace, true)?;
    crate::cas::checked_directory(&namespace)?;
    for relative in ["current.json", "snapshots"] {
        let path = crate::cas_tree::native(&namespace, relative)?;
        crate::cas_tree::project_target(root, &path, true)?;
        if relative == "snapshots" {
            crate::cas::checked_directory(&path)?;
        }
    }
    Ok(namespace)
}
fn optional(path: &Utf8Path) -> Result<Option<Vec<u8>>> {
    if !crate::yml::configuration_entry_exists(path)? {
        return Ok(None);
    }
    Ok(Some(crate::fs_utils::read_managed_bounded(
        path,
        crate::snapshot_format::MAX_METADATA_BYTES as u64,
    )?))
}
fn tag(title: &str) -> Result<String> {
    anyhow::ensure!(
        !title.trim().is_empty()
            && !title.contains("@version")
            && !title.chars().any(char::is_control),
        "GitHub CAS requires a named long-running storage Release"
    );
    Ok(title.trim_matches(' ').replace(' ', "-"))
}
/// Validate namespaces/selected labels and freeze credentials/current ref before hooks.
/// Preview does not request credentials, download or create a store.
pub(crate) fn capture(
    root: &Utf8Path,
    raw: &Map<String, Value>,
    config: &crate::yml::ValidatedConfig,
    environment: &crate::run_environment::RunEnvironment,
    preview: bool,
    ordinary: Option<&Map<String, Value>>,
    version: &str,
) -> Result<Vec<Target>> {
    let mut result = Vec::new();
    let mut namespaces = std::collections::BTreeSet::new();
    let mut tags = std::collections::BTreeSet::new();
    for title in raw.keys() {
        let resolved = tag(title)?;
        anyhow::ensure!(
            tags.insert(resolved.clone()),
            "Configured CAS titles resolve to the same Release tag"
        );
        for other in ordinary.into_iter().flat_map(|values| values.keys()) {
            let ordinary_tag = if other == "@version" {
                version.to_owned()
            } else {
                other.trim_matches(' ').replace(' ', "-")
            };
            anyhow::ensure!(
                ordinary_tag != resolved,
                "GitHub ordinary/CAS titles resolve to the same Release tag"
            );
        }
    }
    for (title, value) in raw {
        let policy = definition(value)?;
        let labels = crate::cas_remote::selected_labels(config, &policy.content)?;
        if labels.is_empty() {
            continue;
        }
        let namespace = namespace(root, title)?;
        let tag = tag(title)?;
        anyhow::ensure!(
            namespaces.insert(namespace.clone()),
            "Configured store project metadata namespaces collide"
        );
        let sources = labels
            .into_iter()
            .map(|label| Ok((label.clone(), config.get_path(root, &label)?)))
            .collect::<Result<Vec<_>>>()?;
        for source in config
            .directories
            .keys()
            .map(|label| config.get_path(root, label))
        {
            anyhow::ensure!(
                !crate::path_safety::portable_paths_overlap(&namespace, &source?),
                "GitHub CAS metadata overlaps a declared resource"
            );
        }
        let frozen = optional(&namespace.join("current.json"))?;
        let binding = frozen
            .as_ref()
            .map(|bytes| serde_json::from_slice::<Binding>(bytes))
            .transpose()?;
        if let Some(binding) = &binding {
            binding.reference.validate()?;
        }
        if preview {
            continue;
        }
        let (repository, client) = crate::github_publication::connection(root, environment, None)?;
        let api = environment
            .text("GITHUB_API_URL")
            .unwrap_or("https://api.github.com")
            .to_owned();
        if let Some(binding) = &binding {
            binding.check(&repository, &api, &tag)?;
        }
        result.push(Target {
            title: title.clone(),
            namespace,
            sources,
            frozen,
            binding,
            client,
            style: policy.manifest_style,
            format: policy
                .manifest_format
                .unwrap_or(if policy.manifest_style == Style::Tree {
                    Format::Json
                } else {
                    Format::Csv
                }),
            repository,
            api,
            tag,
        });
    }
    Ok(result)
}
/// Check frozen project selection and final source types before entering distribution.
pub(crate) fn prepare(root: &Utf8Path, target: &Target) -> Result<()> {
    anyhow::ensure!(
        optional(&target.namespace.join("current.json"))? == target.frozen,
        "Trusted CAS base reference changed after capture"
    );
    namespace(root, &target.title)?;
    for (_, source) in &target.sources {
        crate::path_safety::resolve_path(source.as_std_path(), true)?;
        anyhow::ensure!(
            fs::symlink_metadata(source)?.is_dir(),
            "CAS publication source must be a real directory"
        );
    }
    Ok(())
}
/// Download trusted base, operate with the same local engine, then publish only
/// a verified new export and guarded Git references. Old remote bytes never deleted.
/// Missing local authority with existing remote exports fails, never guesses latest.
pub(crate) fn publish(root: &Utf8Path, target: &Target, version: &str) -> Result<String> {
    prepare(root, target)?;
    let temporary = tempfile::tempdir()?;
    let fresh = Utf8PathBuf::try_from(temporary.path().join("store"))?;
    let imported = target
        .binding
        .as_ref()
        .map(|binding| {
            crate::cas_github::download(
                root,
                &target.client,
                &binding.reference,
                MAX_BYTES,
                MAX_EXPANDED,
            )
        })
        .transpose()?;
    let store = if let Some(imported) = &imported {
        imported.path()
    } else {
        let assets = target
            .client
            .assets_if_exists(&target.tag)?
            .unwrap_or_default();
        anyhow::ensure!(
            !assets
                .iter()
                .any(|asset| asset.name.starts_with("proj-cas-store-")),
            "Remote CAS exports exist but trusted project current.json is missing; recover the project reference instead of guessing"
        );
        fs::create_dir(&fresh)?;
        &fresh
    };
    let id = if target.style == Style::Tree {
        let directories = target
            .sources
            .iter()
            .map(|(label, source)| {
                Ok(DirectoryRoot {
                    label: label.clone(),
                    tree: crate::cas_tree::ingest_directory(root, store, store, source)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let snapshot = TreeSnapshot::new(version.into(), None, directories)?;
        crate::cas_tree::publish_snapshot(root, store, store, &snapshot)?
    } else {
        let directories = target
            .sources
            .iter()
            .map(|(label, source)| {
                crate::cas_tree::ingest_flat_directory(root, store, source, label)
            })
            .collect::<Result<Vec<_>>>()?;
        let snapshot = FlatSnapshot::new(version.into(), None, directories)?;
        crate::cas_flat::publish_snapshot(
            root,
            store,
            &[store.to_owned()],
            None,
            &snapshot,
            target.format,
        )?
    };
    let reference =
        crate::cas_github::upload(root, store, &target.client, &target.tag, true, MAX_EXPANDED)?;
    let binding = Binding {
        schema: "projcli.cas-github-project.v1".into(),
        repository: target.repository.clone(),
        api: target.api.clone(),
        reference,
    };
    let bytes = serde_json::to_vec_pretty(&binding)?;
    let path = crate::cas_tree::native(
        &target.namespace,
        &format!("snapshots/{}/{}.json", &id[..2], &id[2..]),
    )?;
    crate::cas_tree::project_target(root, &path, true)?;
    if let Some(existing) = optional(&path)? {
        let previous: Binding = serde_json::from_slice(&existing)?;
        previous.check(&target.repository, &target.api, &target.tag)?;
        anyhow::ensure!(
            previous.reference.snapshots.contains_key(&id),
            "Existing CAS snapshot locator conflicts"
        );
    } else {
        crate::cas::write_metadata(&path, &bytes, true)?;
    }
    anyhow::ensure!(
        optional(&target.namespace.join("current.json"))? == target.frozen,
        "CAS base changed during publication; retain both exports and inspect"
    );
    crate::cas::write_metadata(&target.namespace.join("current.json"), &bytes, false)
        .context("Verified CAS export exists but project reference failed; retain remote export")?;
    Ok(id)
}
fn raw(
    root: &Utf8Path,
    title: &str,
    profile: Option<&str>,
) -> Result<(
    crate::yml::ValidatedConfig,
    crate::run_environment::RunEnvironment,
    String,
)> {
    let profiles = crate::run_context::select_profiles(profile, root)?;
    crate::yml::require_active_configuration_for_profiles(root, &profiles)?;
    let value = crate::yml::get_combined_yml(Some(&profiles.join(",")), root)?;
    let config = crate::restore::projected_config(&value, root)?;
    let remote = value
        .get("remotes")
        .and_then(|v| v.get("github"))
        .and_then(|v| v.get(title))
        .context("Named GitHub CAS remote not found")?;
    definition(remote)?;
    let environment =
        crate::run_environment::RunEnvironment::resolve(Some(&profiles.join(",")), root)?;
    Ok((config, environment, tag(title)?))
}
/// Whether an explicit named snapshot selects the GitHub CAS transport.
pub fn selected(root: &Utf8Path, title: &str, profile: Option<&str>) -> Result<bool> {
    let profiles = crate::run_context::select_profiles(profile, root)?;
    let value = crate::yml::get_combined_yml(Some(&profiles.join(",")), root)?;
    Ok(value
        .get("remotes")
        .and_then(|v| v.get("github"))
        .and_then(|v| v.get(title))
        .and_then(|v| v.get("structure"))
        .and_then(Value::as_str)
        == Some("cas"))
}
fn recover(
    root: &Utf8Path,
    title: &str,
    id: &str,
    profile: Option<&str>,
    max_bytes: u64,
    max_expanded: u64,
) -> Result<(
    crate::yml::ValidatedConfig,
    crate::cas_archive::Import,
    SnapshotVerification,
)> {
    crate::cas::validate_digest(id)?;
    let (config, environment, tag) = raw(root, title, profile)?;
    let path = crate::cas_tree::native(
        &namespace(root, title)?,
        &format!("snapshots/{}/{}.json", &id[..2], &id[2..]),
    )?;
    let binding: Binding = serde_json::from_slice(&optional(&path)?.context(
        "Trusted project snapshot locator is missing; recover it from project Git history",
    )?)?;
    let (repository, client) =
        crate::github_publication::connection(root, &environment, Some(&binding.repository))?;
    let api = environment
        .text("GITHUB_API_URL")
        .unwrap_or("https://api.github.com");
    binding.check(&repository, api, &tag)?;
    anyhow::ensure!(
        binding.reference.snapshots.contains_key(id),
        "Snapshot locator does not contain requested binding"
    );
    let imported =
        crate::cas_github::download(root, &client, &binding.reference, max_bytes, max_expanded)?;
    let kind = imported
        .snapshots()
        .get(id)
        .context("CAS snapshot locator is missing")?;
    let snapshot = match kind {
        crate::cas_archive::Kind::Tree => SnapshotVerification::Tree {
            observation: crate::cas_tree::verify_snapshot(imported.path(), imported.path(), id)?,
        },
        crate::cas_archive::Kind::Flat => SnapshotVerification::FlatCatalogue {
            observation: crate::cas_flat::verify_snapshot(imported.path(), imported.path(), id)?,
        },
        crate::cas_archive::Kind::Legacy => SnapshotVerification::LegacyFlat {
            observation: crate::cas_tree::verify_legacy_snapshot(imported.path(), id)?,
        },
    };
    Ok((config, imported, snapshot))
}
/// Deeply verify one explicit Git-backed snapshot locator and every store payload.
pub fn verify(
    root: &Utf8Path,
    title: &str,
    id: &str,
    profile: Option<&str>,
    max_bytes: u64,
    max_expanded: u64,
) -> Result<SnapshotVerification> {
    recover(root, title, id, profile, max_bytes, max_expanded).map(|(_, _, snapshot)| snapshot)
}
/// Restore from trusted project Git metadata independently of build.dest.
/// Keep the imported-store owner alive until verified installation completes.
pub fn restore(
    root: &Utf8Path,
    title: &str,
    id: &str,
    label: &str,
    profile: Option<&str>,
    remote_version: Option<&str>,
    clear: bool,
    max_bytes: u64,
    max_expanded: u64,
) -> Result<crate::restore::RestoreResult> {
    let (config, imported, snapshot) = recover(root, title, id, profile, max_bytes, max_expanded)?;
    let label = config
        .directories
        .keys()
        .find(|key| key.eq_ignore_ascii_case(label))
        .context("Restore label is undeclared")?;
    if let Some(version) = remote_version {
        anyhow::ensure!(
            version == snapshot.version(),
            "Requested version differs from explicit CAS snapshot"
        );
    }
    let destination = config.get_path(root, label)?;
    match &snapshot {
        SnapshotVerification::Tree { .. } => {
            crate::cas_tree::restore_label(
                root,
                imported.path(),
                imported.path(),
                id,
                label,
                &destination,
                clear,
            )?;
        }
        SnapshotVerification::FlatCatalogue { .. } => {
            crate::cas_flat::restore_label(
                root,
                imported.path(),
                imported.path(),
                id,
                label,
                &destination,
                clear,
            )?;
        }
        SnapshotVerification::LegacyFlat { .. } => {
            crate::cas_tree::restore_legacy_label(
                root,
                imported.path(),
                id,
                label,
                &destination,
                clear,
            )?;
        }
        SnapshotVerification::Xet { .. } => {
            anyhow::bail!("CAS snapshot cannot be reinterpreted as Xet")
        }
    }
    Ok(crate::restore::RestoreResult {
        label: label.clone(),
        destination,
        provider: Some("github".into()),
        title: Some(title.into()),
        version: Some(snapshot.version().into()),
        restored: true,
        skipped: None,
        snapshot_id: Some(id.into()),
        snapshot_kind: Some(snapshot.kind().into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn github_cas_style_and_initial_metadata_placement_are_explicit() {
        assert!(definition(&serde_json::json!({"structure":"cas"})).is_err());
        assert!(
            definition(&serde_json::json!({"structure":"cas","manifest-style":"tree"})).is_ok()
        );
        for format in ["csv", "json", "csv-dedupe-file", "json-dedupe-file"] {
            assert!(definition(&serde_json::json!({"structure":"cas","manifest-style":"flat","manifest-format":format})).is_ok());
        }
        for placement in ["project", "both"] {
            assert!(definition(&serde_json::json!({"structure":"cas","manifest-style":"tree","manifest":placement})).is_err());
        }
    }
}
