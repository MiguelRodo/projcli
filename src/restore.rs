//! Configured single-label restore from an ordinary named registry or legacy inline definitions.
//! This is an explicit operation; current upload selection does not remove registered sources.
use crate::github_zip::{GithubStructure, directory_asset_name};
use crate::local_remote::{LocalStructure, compare_versions, version_numbers};
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;

/// Explicit configured restore controls. Directory references retain first priority;
/// legacy preferred sources precede registry preferred sources, then legacy/registry
/// fallbacks in their merged declaration order. No automatic source failover is implied.
#[derive(Default)]
pub struct RestoreRequest {
    /// Declared label to restore (required; no implicit role or path).
    pub label: String,
    /// Filter legacy directory-source/build-destination sections; registered definitions
    /// remain candidates for either choice. None searches both legacy sections.
    pub position: Option<String>,
    /// Restrict the provider to local or github.
    pub provider: Option<String>,
    /// Restrict a registered name or legacy inline title/reference.
    pub title: Option<String>,
    /// Exact archive version override; None selects newest remote version.
    pub remote_version: Option<String>,
    /// Explicit GitHub owner/repository, avoiding any Git dependency.
    pub repository: Option<String>,
    /// Explicit active profiles; local overrides remain highest.
    pub profile: Option<String>,
    /// Clear destination-only files only after valid complete staging.
    pub clear: bool,
    /// Maximum compressed GitHub asset bytes.
    pub max_bytes: u64,
    /// Maximum expanded GitHub ZIP bytes.
    pub max_expanded_bytes: u64,
}
/// One result. A skipped empty/unrecorded label never clears local files.
#[derive(Debug, Serialize)]
pub struct RestoreResult {
    /// Canonical configured label.
    pub label: String,
    /// Explicit final configured target.
    pub destination: Utf8PathBuf,
    /// Selected provider, absent when local history says nothing to restore.
    pub provider: Option<String>,
    /// Selected title, distinct from the transport's normalized tag.
    pub title: Option<String>,
    /// Selected version; not a CSV-integrity/trust guarantee.
    pub version: Option<String>,
    /// Whether transfer and installation completed.
    pub restored: bool,
    /// no-recorded-files or empty-remote, otherwise None.
    pub skipped: Option<String>,
    /// Exact content-addressed snapshot for explicit CAS recovery; absent for ordinary layouts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    /// Stored CAS format; absent for ordinary latest/archive restores.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_kind: Option<String>,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Layout {
    Latest,
    Archive,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct Definition {
    pub(crate) path: Option<String>,
    pub(crate) structure: Option<Layout>,
    pub(crate) path_append_label: Option<bool>,
    pub(crate) content: Option<Value>,
    pub(crate) send: Option<Value>,
    pub(crate) get: Option<Value>,
    pub(crate) cue: Option<Value>,
    pub(crate) id: Option<Value>,
    pub(crate) manifest: Option<crate::cas_remote::Placement>,
    pub(crate) manifest_style: Option<crate::cas_remote::Style>,
    pub(crate) manifest_format: Option<crate::catalogue::Format>,
    /// GitHub destination repository (`owner/name`); defaults to the producing repository.
    pub(crate) repo: Option<String>,
    /// GitHub release name (tag and title); defaults to `title`, then the entry name.
    pub(crate) tag: Option<String>,
    /// GitHub branch whose head a newly created release targets (`@branch` = checkout).
    pub(crate) branch: Option<String>,
}
/// Reject GitHub-only fields on a local destination definition.
pub(crate) fn local_only_fields(value: &Value) -> anyhow::Result<()> {
    for key in ["repo", "tag", "branch"] {
        anyhow::ensure!(
            value.get(key).is_none(),
            "{key} applies only to GitHub destinations"
        );
    }
    Ok(())
}
struct Source {
    provider: String,
    title: String,
    definition: Definition,
}
/// Validate projr 0.9 `get` fetch settings (SATVILab/projr@5f915ef
/// `R/misc.R:787-797`, `R/yml-check.R:437-456`). They are accepted for a later
/// `build.source` slice; nothing fetches with them yet.
fn fetch_settings(get: &Value) -> anyhow::Result<()> {
    let get = get.as_object().context("get must be a mapping")?;
    for (key, value) in get {
        let text = value
            .as_str()
            .with_context(|| format!("get.{key} must be a string"))?;
        let allowed: &[&str] =
            match key.as_str() {
                "version" => {
                    anyhow::ensure!(
                        text == "latest"
                            || (text.split('.').count() <= 3
                                && text.split('.').all(|part| !part.is_empty()
                                    && part.bytes().all(|b| b.is_ascii_digit()))),
                        "get.version must be latest or an x, x.y or x.y.z version"
                    );
                    continue;
                }
                "cue" => &["if-empty", "never", "if-change", "always"],
                "inspect" => &["manifest", "file", "none"],
                "strategy" => &[
                    "sync-diff",
                    "sync-purge",
                    "download-all",
                    "download-missing",
                ],
                "conflict" => &["error", "local", "remote"],
                _ => anyhow::bail!("get accepts version, cue, inspect, strategy and conflict"),
            };
        anyhow::ensure!(
            allowed.contains(&text),
            "get.{key} must be one of {}",
            allowed.join(", ")
        );
    }
    Ok(())
}
pub(crate) fn definition(value: &Value) -> anyhow::Result<Definition> {
    for key in ["manifest", "manifest-style", "manifest-format"] {
        anyhow::ensure!(
            !value.get(key).is_some_and(Value::is_null),
            "{key} must be supported or omitted"
        );
    }
    anyhow::ensure!(
        value.get("source").is_none(),
        "A remote's source field is no longer supported (projr 0.9); select sources with build.source"
    );
    let parsed: Definition =
        serde_json::from_value(value.clone()).context("Invalid ordinary remote definition")?;
    crate::ordinary_manifest::Policy::parse(&parsed)?;
    if let Some(repo) = &parsed.repo {
        let parts: Vec<_> = repo.split('/').collect();
        anyhow::ensure!(
            parts.len() == 2
                && parts.iter().all(|part| !part.is_empty()
                    && *part != "."
                    && *part != ".."
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))),
            "repo must be a GitHub owner/repository name"
        );
    }
    for (key, text) in [("tag", &parsed.tag), ("branch", &parsed.branch)] {
        if let Some(text) = text {
            anyhow::ensure!(
                !text.trim().is_empty() && !text.chars().any(char::is_control),
                "{key} must be nonempty printable text"
            );
        }
    }
    if let Some(get) = &parsed.get {
        fetch_settings(get)?;
    }
    // Restore does not execute upload policy. Keep those nodes typed but do not
    // silently enable them in builds (publication validates its executable subset).
    if let Some(send) = &parsed.send {
        anyhow::ensure!(send.is_object(), "send must be a mapping");
    }
    if let Some(cue) = &parsed.cue {
        anyhow::ensure!(cue.is_string(), "cue must be a string");
    }
    if let Some(id) = &parsed.id {
        anyhow::ensure!(id.is_string(), "id must be a string");
    }
    Ok(parsed)
}
fn referenced(
    raw: &Value,
    provider: &str,
    title: &str,
    inline: Option<&Value>,
) -> anyhow::Result<Definition> {
    anyhow::ensure!(
        !title.is_empty() && !title.chars().any(char::is_control),
        "Invalid remote title"
    );
    let registered = raw
        .get("remotes")
        .and_then(|v| v.get(provider))
        .and_then(|v| v.get(title));
    let value = match (registered, inline) {
        (Some(shared), Some(inline)) => crate::yml::deep_merge(shared.clone(), inline.clone()),
        (Some(shared), None) => shared.clone(),
        (None, Some(inline)) => inline.clone(),
        (None, None) => anyhow::bail!("Directory source references an undefined remote"),
    };
    anyhow::ensure!(
        !matches!(
            value.get("structure").and_then(Value::as_str),
            Some("cas" | "xet")
        ),
        "CAS/Xet restore requires an explicit --snapshot and --title"
    );
    definition(&value)
}
/// Accept projr's `<type>:<title>` remote names; an unqualified title registered
/// for both types is ambiguous (SATVILab/projr@5f915ef `R/yml-dest.R:661-703`).
fn qualified(raw: &Value, request: &RestoreRequest) -> anyhow::Result<RestoreRequest> {
    let mut provider = request.provider.clone();
    let mut title = request.title.clone();
    if let Some((kind, rest)) = request.title.as_deref().and_then(|t| t.split_once(':'))
        && matches!(kind, "local" | "github")
        && !rest.is_empty()
    {
        anyhow::ensure!(
            provider.as_deref().is_none_or(|p| p == kind),
            "--title {kind}:{rest} conflicts with --provider"
        );
        provider = Some(kind.into());
        title = Some(rest.into());
    }
    if let (None, Some(title)) = (&provider, &title) {
        let registered = |kind: &str| raw["remotes"][kind].get(title.as_str()).is_some();
        anyhow::ensure!(
            !(registered("local") && registered("github")),
            "Remote {title} is declared under both remotes.local and remotes.github. Use local:{title} or github:{title}"
        );
    }
    Ok(RestoreRequest {
        label: request.label.clone(),
        position: request.position.clone(),
        provider,
        title,
        remote_version: request.remote_version.clone(),
        repository: request.repository.clone(),
        profile: request.profile.clone(),
        clear: request.clear,
        max_bytes: request.max_bytes,
        max_expanded_bytes: request.max_expanded_bytes,
    })
}
fn provider_allowed(provider: &str, request: &RestoreRequest) -> bool {
    request
        .provider
        .as_deref()
        .is_none_or(|filter| filter == provider)
}
fn title_allowed(title: &str, request: &RestoreRequest) -> bool {
    request
        .title
        .as_deref()
        .is_none_or(|filter| filter == title)
}
pub(crate) fn content_has(value: Option<&Value>, label: &str, raw: &Value) -> anyhow::Result<bool> {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Ok(false),
        Some(Value::Bool(true)) => Ok(!label.to_ascii_lowercase().starts_with("cache")),
        Some(Value::Array(labels)) => {
            let declared = raw["directories"]
                .as_object()
                .context("Directories must be a mapping")?;
            for label in labels {
                let label = label
                    .as_str()
                    .context("Remote content must contain label strings")?;
                anyhow::ensure!(
                    declared.contains_key(label),
                    "Remote content contains an undeclared label"
                );
            }
            Ok(labels.iter().any(|item| item.as_str() == Some(label)))
        }
        _ => anyhow::bail!("Remote content must be boolean or a label list"),
    }
}
/// Explicit immutable-store recovery settings; transport budgets are mandatory.
pub struct SnapshotRestoreOptions<'a> {
    /// Selected configuration profile, if any.
    pub profile: Option<&'a str>,
    /// Optional expected version; it must match the selected snapshot.
    pub remote_version: Option<&'a str>,
    /// Remove destination members absent from the verified snapshot.
    pub clear: bool,
    /// Maximum compressed store transfer length.
    pub max_bytes: u64,
    /// Maximum aggregate expanded archive length.
    pub max_expanded: u64,
}

fn select_source(raw: &Value, label: &str, request: &RestoreRequest) -> anyhow::Result<Source> {
    // Registry lookup is independent of upload selection and legacy section filters.
    // Directory-specific references retain their existing priority.
    if request.position.as_deref() != Some("dest")
        && let Some(sources) = raw["directories"][label]
            .get("source")
            .filter(|v| !v.is_null())
    {
        for (provider, entries) in sources
            .as_object()
            .context("Directory source must be a provider mapping")?
        {
            anyhow::ensure!(
                matches!(provider.as_str(), "local" | "github"),
                "Unsupported source provider"
            );
            if !provider_allowed(provider, request) {
                continue;
            }
            let candidates: Vec<(String, Option<&Value>)> = match entries {
                Value::String(title) => vec![(title.clone(), None)],
                Value::Array(titles) => titles
                    .iter()
                    .map(|v| {
                        Ok((
                            v.as_str()
                                .context("Source references must be strings")?
                                .into(),
                            None,
                        ))
                    })
                    .collect::<anyhow::Result<_>>()?,
                Value::Object(titles) => titles
                    .iter()
                    .map(|(title, value)| (title.clone(), Some(value)))
                    .collect(),
                Value::Null => vec![],
                _ => anyhow::bail!(
                    "Directory source must contain a title, title list or inline definitions"
                ),
            };
            for (title, inline) in candidates {
                if title_allowed(&title, request) {
                    return Ok(Source {
                        definition: referenced(raw, provider, &title, inline)?,
                        provider: provider.clone(),
                        title,
                    });
                }
            }
        }
    }
    let mut registry_fallback = None;
    if let Some(registry) = raw.get("remotes").and_then(Value::as_object) {
        for (provider, entries) in registry {
            if !matches!(provider.as_str(), "local" | "github")
                || !provider_allowed(provider, request)
            {
                continue;
            }
            for (title, value) in entries
                .as_object()
                .context("Remote provider must contain named definitions")?
            {
                if !title_allowed(title, request) {
                    continue;
                }
                // Legacy flat CAS is not an ordinary restore source. Tree-aware configured
                // recovery remains a separately tracked implementation step.
                if matches!(
                    value.get("structure").and_then(Value::as_str),
                    Some("cas" | "xet")
                ) {
                    continue;
                }
                let definition = definition(value)?;
                if value.get("content").is_some() && !content_has(value.get("content"), label, raw)?
                {
                    continue;
                }
                let source = Source {
                    provider: provider.clone(),
                    title: title.clone(),
                    definition,
                };
                if registry_fallback.is_none() {
                    registry_fallback = Some(source)
                }
            }
        }
    }
    if let Some(source) = registry_fallback {
        return Ok(source);
    }
    anyhow::bail!("No configured restore source matches the requested label and filters")
}
pub(crate) fn projected_config(
    raw: &Value,
    root: &Utf8Path,
) -> anyhow::Result<crate::yml::ValidatedConfig> {
    let mut projection = raw.clone();
    if let Some(directories) = projection
        .get_mut("directories")
        .and_then(Value::as_object_mut)
    {
        for directory in directories.values_mut() {
            if let Some(directory) = directory.as_object_mut() {
                directory.remove("source");
            }
        }
    }
    let mut config: crate::yml::ProjConfig =
        serde_json::from_value(projection).context("Invalid restore project configuration")?;
    crate::remote_registry::validate_names(&config)?;
    crate::remote_registry::reject_inline(&config)?;
    // Upload selection never limits restore sources.
    config.build.dest = None;
    config.validate_and_resolve(root, false)
}

/// Placeholder values for a lookup outside a build: the current run version and
/// checkout. `@uuid` names a single build and fails until recorded destinations are read.
fn lookup_templates(root: &Utf8Path) -> crate::template::Templates {
    crate::template::Templates::for_lookup(
        root,
        crate::run_version_from(root)
            .ok()
            .map(|version| version.to_string(true)),
    )
}
fn repository(root: &Utf8Path, explicit: Option<&str>, api: &str) -> anyhow::Result<String> {
    if let Some(repository) = explicit {
        return Ok(repository.into());
    }
    repository_in(
        root,
        explicit,
        api,
        &crate::run_environment::RunEnvironment::inherited(),
    )
}
/// Resolve Git origin using an already captured environment and API host.
pub(crate) fn repository_in(
    root: &Utf8Path,
    explicit: Option<&str>,
    api: &str,
    environment: &crate::run_environment::RunEnvironment,
) -> anyhow::Result<String> {
    if let Some(repository) = explicit {
        return Ok(repository.into());
    }
    let mut command = crate::git::git_command_in(Some(root), environment)?;
    let output = command
        .current_dir(root)
        .args(["remote", "get-url", "origin"])
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "Cannot resolve Git origin; supply --repository owner/repo"
    );
    let url = std::str::from_utf8(&output.stdout)
        .context("Git origin is not UTF-8")?
        .trim();
    let (host, path) = if let Some(url) = url.strip_prefix("https://") {
        url.split_once('/')
            .context("Git origin is not a repository URL")?
    } else if let Some(url) = url.strip_prefix("git@") {
        url.split_once(':')
            .context("Git origin is not an SSH repository URL")?
    } else {
        anyhow::bail!("Unsupported Git origin; supply --repository owner/repo");
    };
    let api: ureq::http::Uri = api.parse().context("Invalid API base")?;
    let expected = if api
        .host()
        .is_some_and(|host| host.eq_ignore_ascii_case("api.github.com"))
    {
        "github.com"
    } else {
        api.host().context("API base has no host")?
    };
    anyhow::ensure!(
        host.eq_ignore_ascii_case(expected),
        "Git origin does not match the API host; supply --repository explicitly"
    );
    Ok(path.strip_suffix(".git").unwrap_or(path).into())
}
fn github_version(
    client: &crate::github_release::GithubReleaseClient,
    root: &Utf8Path,
    source: &Source,
    label: &str,
    request: &RestoreRequest,
    tag: &str,
) -> anyhow::Result<(String, String, bool)> {
    let definition = &source.definition;
    let prefix = definition.path.as_deref();
    let append = definition.path_append_label.unwrap_or(true);
    let assets = client.assets(tag)?;
    if matches!(
        definition.structure.unwrap_or(Layout::Archive),
        Layout::Archive
    ) {
        let mut stems = Vec::new();
        for remote_label in if label == "output-r-data" {
            vec![label, "data"]
        } else {
            vec![label]
        } {
            let stem = directory_asset_name(
                prefix,
                append,
                remote_label,
                GithubStructure::Latest,
                None,
                false,
            )?;
            let stem = format!("{}-v", stem.strip_suffix(".zip").unwrap());
            if !stems.contains(&stem) {
                stems.push(stem);
            }
        }
        let mut namespaces = std::collections::BTreeSet::new();
        let mut found = Vec::new();
        for asset in assets {
            let Some((namespace, version)) = stems.iter().find_map(|stem| {
                asset
                    .name
                    .strip_prefix(stem)
                    .and_then(|v| v.strip_suffix(".zip"))
                    .map(|v| (stem, v))
            }) else {
                continue;
            };
            let empty = version.ends_with("-empty");
            let version = version.strip_suffix("-empty").unwrap_or(version);
            if !version.as_bytes().first().is_some_and(u8::is_ascii_digit) {
                continue;
            }
            let numbers = version_numbers(version)
                .context("Invalid numeric-looking GitHub archive asset version")?;
            namespaces.insert(namespace.clone());
            if request
                .remote_version
                .as_deref()
                .is_some_and(|wanted| wanted.strip_prefix('v').unwrap_or(wanted) != version)
            {
                continue;
            }
            found.push((numbers, format!("v{version}"), asset.name, empty));
        }
        anyhow::ensure!(
            namespaces.len() <= 1,
            "Canonical and legacy data remote namespaces conflict"
        );
        found.sort_by(|a, b| {
            compare_versions(&b.0, &a.0)
                .then(a.3.cmp(&b.3))
                .then(a.2.cmp(&b.2))
        });
        let (_, version, name, empty) = found
            .into_iter()
            .next()
            .context("Requested GitHub archive version is unavailable")?;
        Ok((version, name, empty))
    } else {
        let transfer = tempfile::Builder::new()
            .prefix("proj-restore-meta-")
            .tempdir_in(fs::canonicalize(std::env::temp_dir())?)?;
        let downloaded = (|| {
            let path = Utf8Path::from_path(transfer.path())
                .context("Metadata path must be UTF-8")?
                .join("VERSION.zip");
            client.download_asset(
                root,
                tag,
                "VERSION.zip",
                &path,
                request.max_bytes.min(4 * 1024 * 1024),
            )?;
            let extracted = crate::github_zip::extract_zip(
                &path,
                request.max_expanded_bytes.min(16 * 1024 * 1024),
            )?;
            let result = (|| {
                let directory =
                    Utf8Path::from_path(extracted.path()).context("Metadata path must be UTF-8")?;
                let entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
                anyhow::ensure!(
                    entries.len() == 1 && entries[0].file_name() == "VERSION",
                    "VERSION.zip must contain exactly VERSION"
                );
                let text = crate::fs_utils::read_managed_text(&directory.join("VERSION"))?;
                let matches: Vec<_> = text
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .filter(|(key, _)| {
                        key.trim() == label
                            || (label == "output-r-data" && key.trim().eq_ignore_ascii_case("data"))
                    })
                    .collect();
                anyhow::ensure!(
                    matches.len() == 1,
                    "VERSION.zip label is missing or ambiguous"
                );
                let value = matches[0].1.trim();
                let version = value.strip_suffix('*').unwrap_or(value);
                anyhow::ensure!(
                    version_numbers(version).is_some(),
                    "Invalid advertised GitHub version"
                );
                let version = format!("v{}", version.strip_prefix('v').unwrap_or(version));
                if let Some(wanted) = &request.remote_version {
                    anyhow::ensure!(
                        wanted.trim_start_matches('v') == version.trim_start_matches('v'),
                        "Requested version is not the advertised latest"
                    );
                }
                let remote_label = matches[0].0.trim();
                let full = directory_asset_name(
                    prefix,
                    append,
                    remote_label,
                    GithubStructure::Latest,
                    None,
                    false,
                )?;
                let empty = directory_asset_name(
                    prefix,
                    append,
                    remote_label,
                    GithubStructure::Latest,
                    None,
                    true,
                )?;
                if assets.iter().any(|asset| asset.name == full) {
                    Ok((version, full, false))
                } else if assets.iter().any(|asset| asset.name == empty) {
                    Ok((version, empty, true))
                } else {
                    anyhow::bail!("Advertised GitHub latest asset is unavailable");
                }
            })();
            let cleanup = extracted.close();
            match (result, cleanup) {
                (Ok(result), Ok(())) => Ok(result),
                (Err(error), Ok(())) => Err(error),
                (Ok(_), Err(error)) => Err(error),
                (Err(error), Err(cleanup)) => Err(error.context(format!(
                    "Metadata extraction cleanup also failed: {cleanup:#}"
                ))),
            }
        })();
        let cleanup_path = transfer.path().to_owned();
        let cleanup = transfer.close().with_context(|| {
            format!(
                "Metadata staging cleanup failed at {}",
                cleanup_path.display()
            )
        });
        match (downloaded, cleanup) {
            (Ok(result), Ok(())) => Ok(result),
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(cleanup)) => {
                Err(error.context(format!("Metadata staging cleanup also failed: {cleanup:#}")))
            }
        }
    }
}
/// Restores one declared final directory using existing merged inline config.
/// Requires active project configuration and valid manifest history (the
/// `.projr/manifests` store, or a projr-written legacy root manifest.csv), matching
/// projr's history gate. Source entries precede eligible build destinations;
/// source:true/label preferences beat fallback, source:false excludes. There is
/// no retry against another source after a selected source fails. Empty/unrecorded
/// labels are skipped without clearing. Local and GitHub latest/archive downloads
/// use the existing staged installers. The CSV is not an integrity/trust check.
/// Existing ordinary provider definitions are reused. Normal builds validate source
/// references without auto-restoring and reject unsupported publication fields.
pub fn restore_label(root: &Utf8Path, request: &RestoreRequest) -> anyhow::Result<RestoreResult> {
    anyhow::ensure!(
        request
            .position
            .as_deref()
            .is_none_or(|v| matches!(v, "source" | "dest")),
        "Restore position must be source or dest"
    );
    anyhow::ensure!(
        request
            .provider
            .as_deref()
            .is_none_or(|v| matches!(v, "local" | "github")),
        "Restore provider must be local or github"
    );
    let profiles = crate::run_context::select_profiles(request.profile.as_deref(), root)?;
    crate::yml::require_active_configuration_for_profiles(root, &profiles)?;
    let selected_profiles = profiles.join(",");
    let raw = crate::yml::get_combined_yml(Some(&selected_profiles), root)?;
    let config = projected_config(&raw, root)?;
    let label = config
        .directories
        .keys()
        .find(|label| label.eq_ignore_ascii_case(&request.label))
        .context("Restore label is not declared")?
        .clone();
    let destination = config.get_path(root, &label)?;
    anyhow::ensure!(
        crate::manifest_store::history_exists(
            &crate::manifest_store::project_store(root),
            &root.join("manifest.csv")
        )?,
        "Restore requires recorded manifest history (.projr/manifests or a legacy manifest.csv)"
    );
    let rows = crate::manifest_store::read_project(root)?;
    let mut result = RestoreResult {
        label: label.clone(),
        destination: destination.clone(),
        provider: None,
        title: None,
        version: None,
        restored: false,
        skipped: None,
        snapshot_id: None,
        snapshot_kind: None,
    };
    if !rows.iter().any(|row| {
        (row.label == label || (label == "output-r-data" && row.label.eq_ignore_ascii_case("data")))
            && !row.r#fn.is_empty()
    }) {
        result.skipped = Some("no-recorded-files".into());
        return Ok(result);
    }
    let request = &qualified(&raw, request)?;
    let source = select_source(&raw, &label, request)?;
    result.provider = Some(source.provider.clone());
    result.title = Some(source.title.clone());
    if source.provider == "local" {
        let path = source
            .definition
            .path
            .as_deref()
            .context("Local restore source requires an explicit path")?;
        anyhow::ensure!(!path.is_empty(), "Local restore path must not be empty");
        let path = &lookup_templates(root).expand(path)?;
        let base = if Utf8Path::new(path).is_absolute() {
            Utf8PathBuf::from(path)
        } else {
            root.join(path)
        };
        let layout = match source.definition.structure.unwrap_or(Layout::Archive) {
            Layout::Latest => LocalStructure::Latest,
            Layout::Archive => LocalStructure::Archive,
        };
        let append = source.definition.path_append_label.unwrap_or(true);
        let mut catalogs = Vec::new();
        for remote_label in if label == "output-r-data" {
            vec![label.as_str(), "data"]
        } else {
            vec![label.as_str()]
        } {
            let versions = crate::local_remote::list_versions(&base, remote_label, layout, append)?;
            if !versions.is_empty()
                && !catalogs.iter().any(
                    |(_, previous): &(&str, Vec<crate::local_remote::LocalVersion>)| {
                        previous
                            .iter()
                            .map(|v| {
                                (
                                    &v.path,
                                    &v.version,
                                    v.empty,
                                    v.present,
                                    v.metadata_untrusted,
                                )
                            })
                            .eq(versions.iter().map(|v| {
                                (
                                    &v.path,
                                    &v.version,
                                    v.empty,
                                    v.present,
                                    v.metadata_untrusted,
                                )
                            }))
                    },
                )
            {
                catalogs.push((remote_label, versions));
            }
        }
        anyhow::ensure!(
            catalogs.len() <= 1,
            "Canonical and legacy data remote namespaces conflict"
        );
        let (remote_label, versions) = catalogs
            .into_iter()
            .next()
            .context("Local source has no available version")?;
        let selected =
            if matches!(layout, LocalStructure::Archive) && request.remote_version.is_some() {
                crate::local_remote::select_version(
                    &base,
                    remote_label,
                    layout,
                    request.remote_version.as_deref(),
                    append,
                )?
            } else {
                let selected = versions.into_iter().next().unwrap();
                if let Some(wanted) = &request.remote_version {
                    anyhow::ensure!(
                        version_numbers(wanted) == version_numbers(&selected.version),
                        "Requested version is not the advertised latest"
                    );
                }
                selected
            };
        anyhow::ensure!(
            selected.present,
            "Advertised local restore source is unavailable"
        );
        result.version = Some(selected.version.clone());
        if selected.empty {
            anyhow::ensure!(
                fs::read_dir(&selected.path)?.next().is_none(),
                "Empty local marker contains data"
            );
            result.skipped = Some("empty-remote".into());
            return Ok(result);
        }
        crate::local_remote::download_version(root, &selected, &destination, request.clear)?;
    } else {
        let api =
            std::env::var("GITHUB_API_URL").unwrap_or_else(|_| "https://api.github.com".into());
        let repository = repository(
            root,
            request
                .repository
                .as_deref()
                .or(source.definition.repo.as_deref()),
            &api,
        )?;
        let token = ["GITHUB_PAT", "GH_TOKEN", "GITHUB_TOKEN"]
            .iter()
            .find_map(|key| std::env::var(key).ok().filter(|v| !v.trim().is_empty()));
        let client = crate::github_release::GithubReleaseClient::new(&repository, &api, token)?;
        let tag = crate::github_publication::release_tag(
            &source.title,
            &source.definition,
            &lookup_templates(root),
        )?;
        let (version, name, empty) = github_version(&client, root, &source, &label, request, &tag)?;
        result.version = Some(version);
        if empty {
            result.skipped = Some("empty-remote".into());
            return Ok(result);
        }
        crate::github_zip::download_directory(
            &client,
            root,
            &tag,
            &name,
            &destination,
            request.max_bytes,
            request.max_expanded_bytes,
            request.clear,
            false,
        )?;
    }
    result.restored = true;
    Ok(result)
}
