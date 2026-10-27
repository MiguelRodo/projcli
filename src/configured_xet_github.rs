//! Configured Xet over complete GitHub ZIPs; Git metadata selects the trusted base.
//! Immutable exports preserve prior bytes. No distributed mutable head or GC.
use crate::{
    github_release::GithubReleaseClient,
    xet::{Engine, Snapshot},
    xet_github::Reference,
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
        value.get("path").is_none(),
        "GitHub Xet uses immutable store asset names, not path prefixes"
    );
    let mut wire = value.clone();
    wire["path"] = Value::String("unused-github-store".into());
    crate::xet_remote::definition(&wire)
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
            self.schema == "projcli.xet-github-project.v1"
                && self.repository == repository
                && self.api == api
                && self.reference.tag == tag,
            "Trusted Xet reference disagrees with captured repository/API/tag; explicit migration required"
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
    engine: Engine,
    repository: String,
    api: String,
    tag: String,
}
fn namespace(root: &Utf8Path, title: &str) -> Result<Utf8PathBuf> {
    let namespace = crate::cas_tree::native(
        &crate::cas_remote::project_metadata(root, title),
        "xet-github/v1",
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
        "GitHub Xet requires a named long-running storage Release"
    );
    Ok(title.trim_matches(' ').replace(' ', "-"))
}
fn discover(environment: &crate::run_environment::RunEnvironment) -> Result<Engine> {
    #[cfg(not(unix))]
    {
        let _ = environment;
        anyhow::bail!("Experimental Xet store access currently requires Unix hard-link validation");
    }
    #[cfg(unix)]
    {
        Engine::discover(environment)
    }
}
/// Validate namespaces/selected labels and freeze credentials/helper/current ref before hooks.
/// Preview does not request credentials, start helper, download or create a store.
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
            "Configured Xet titles resolve to the same Release tag"
        );
        for other in ordinary.into_iter().flat_map(|values| values.keys()) {
            let ordinary_tag = if other == "@version" {
                version.to_owned()
            } else {
                other.trim_matches(' ').replace(' ', "-")
            };
            anyhow::ensure!(
                ordinary_tag != resolved,
                "GitHub ordinary/Xet titles resolve to the same Release tag"
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
                "GitHub Xet metadata overlaps a declared resource"
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
        let engine = discover(environment)?;
        result.push(Target {
            title: title.clone(),
            namespace,
            sources,
            frozen,
            binding,
            client,
            engine,
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
        "Trusted Xet base reference changed after capture"
    );
    namespace(root, &target.title)?;
    for (_, source) in &target.sources {
        crate::path_safety::resolve_path(source.as_std_path(), true)?;
        anyhow::ensure!(
            fs::symlink_metadata(source)?.is_dir(),
            "Xet publication source must be a real directory"
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
            crate::xet_github::download(
                root,
                &target.client,
                &binding.reference,
                MAX_BYTES,
                MAX_EXPANDED,
                &target.engine,
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
                .any(|asset| asset.name.starts_with("proj-xet-store-")),
            "Remote Xet exports exist but trusted project current.json is missing; recover the project reference instead of guessing"
        );
        fs::create_dir(&fresh)?;
        &fresh
    };
    let id = crate::xet::publish(root, store, &target.sources, version, &target.engine)?;
    let reference = crate::xet_github::upload(
        root,
        store,
        &target.client,
        &target.tag,
        true,
        MAX_EXPANDED,
        &target.engine,
    )?;
    let binding = Binding {
        schema: "projcli.xet-github-project.v1".into(),
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
            previous.reference.snapshots.contains(&id),
            "Existing Xet snapshot locator conflicts"
        );
    } else {
        crate::cas::write_metadata(&path, &bytes, true)?;
    }
    anyhow::ensure!(
        optional(&target.namespace.join("current.json"))? == target.frozen,
        "Xet base changed during publication; retain both exports and inspect"
    );
    crate::cas::write_metadata(&target.namespace.join("current.json"), &bytes, false)
        .context("Verified Xet export exists but project reference failed; retain remote export")?;
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
        .context("Named GitHub Xet remote not found")?;
    definition(remote)?;
    let environment =
        crate::run_environment::RunEnvironment::resolve(Some(&profiles.join(",")), root)?;
    Ok((config, environment, tag(title)?))
}
/// Whether an explicit named snapshot selects the GitHub Xet transport.
pub fn selected(root: &Utf8Path, title: &str, profile: Option<&str>) -> Result<bool> {
    let profiles = crate::run_context::select_profiles(profile, root)?;
    let value = crate::yml::get_combined_yml(Some(&profiles.join(",")), root)?;
    Ok(value
        .get("remotes")
        .and_then(|v| v.get("github"))
        .and_then(|v| v.get(title))
        .and_then(|v| v.get("structure"))
        .and_then(Value::as_str)
        == Some("xet"))
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
    Engine,
    crate::xet_archive::Import,
    Snapshot,
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
        binding
            .reference
            .snapshots
            .iter()
            .any(|candidate| candidate == id),
        "Snapshot locator does not contain requested binding"
    );
    let engine = discover(&environment)?;
    let imported = crate::xet_github::download(
        root,
        &client,
        &binding.reference,
        max_bytes,
        max_expanded,
        &engine,
    )?;
    let snapshot = crate::xet::read_snapshot(root, imported.path(), id)?;
    Ok((config, engine, imported, snapshot))
}
/// Deeply verify one explicit Git-backed snapshot locator and every store payload.
pub fn verify(
    root: &Utf8Path,
    title: &str,
    id: &str,
    profile: Option<&str>,
    max_bytes: u64,
    max_expanded: u64,
) -> Result<Snapshot> {
    recover(root, title, id, profile, max_bytes, max_expanded).map(|(_, _, _, snapshot)| snapshot)
}
/// Restore from trusted project Git metadata independently of build.dest.
/// Keep the imported-store owner alive until verified installation completes.
pub fn restore(
    root: &Utf8Path,
    title: &str,
    id: &str,
    label: &str,
    options: crate::restore::SnapshotRestoreOptions<'_>,
) -> Result<crate::restore::RestoreResult> {
    let crate::restore::SnapshotRestoreOptions {
        profile, remote_version, clear, max_bytes, max_expanded,
    } = options;
    let (config, engine, imported, snapshot) =
        recover(root, title, id, profile, max_bytes, max_expanded)?;
    let label = config
        .directories
        .keys()
        .find(|key| key.eq_ignore_ascii_case(label))
        .context("Restore label is undeclared")?;
    if let Some(version) = remote_version {
        anyhow::ensure!(
            version == snapshot.inventory.version,
            "Requested version differs from explicit Xet snapshot"
        );
    }
    let destination = config.get_path(root, label)?;
    crate::xet::restore(
        root,
        imported.path(),
        id,
        label,
        &destination,
        clear,
        &engine,
    )?;
    Ok(crate::restore::RestoreResult {
        label: label.clone(),
        destination,
        provider: Some("github".into()),
        title: Some(title.into()),
        version: Some(snapshot.inventory.version),
        restored: true,
        skipped: None,
        snapshot_id: Some(id.into()),
        snapshot_kind: Some("xet-local-v1".into()),
    })
}
