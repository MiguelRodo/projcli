//! `proj restore`: restore a checkout's raw inputs from the build that produced
//! it (SATVILab/projr@5f915ef `R/restore-build.R`), plus the shared remote
//! definition parsing used by publication and the explicit CAS/Xet snapshot
//! restores. Registered remotes are lookups; upload selection never limits them.
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One explicit CAS/Xet snapshot restore result.
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
        // projr 0.9 fetch settings, used by build.source and proj source.
        let get = get.as_object().context("get must be a mapping")?;
        anyhow::ensure!(
            !get.contains_key("content"),
            "get accepts version, cue, inspect, strategy and conflict"
        );
        crate::source::check_fetch(get, "get")?;
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

pub(crate) fn projected_config(
    raw: &Value,
    root: &Utf8Path,
) -> anyhow::Result<crate::yml::ValidatedConfig> {
    let mut config: crate::yml::ProjConfig =
        serde_json::from_value(raw.clone()).context("Invalid restore project configuration")?;
    crate::remote_registry::validate_names(&config)?;
    crate::remote_registry::reject_inline(&config)?;
    // Upload selection never limits restore sources.
    config.build.dest = None;
    config.validate_and_resolve(root, false)
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
        "Cannot resolve Git origin; set the remote's repo field"
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
        anyhow::bail!("Unsupported Git origin; set the remote's repo field");
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
        "Git origin does not match the API host; set the remote's repo field"
    );
    Ok(path.strip_suffix(".git").unwrap_or(path).into())
}

/// `proj restore` arguments (projr `projr_restore`).
#[derive(Default)]
pub struct RestoreRequest {
    /// Labels to restore; empty restores every declared raw directory.
    pub labels: Vec<String>,
    /// Exact build version; default is the checkout's completed build.
    pub version: Option<String>,
    /// A registered GitHub remote title, a concrete Release tag in the checkout's
    /// repository, or (projcli) `local:<title>` for a registered local remote.
    /// Defaults to `archive`.
    pub destination: Option<String>,
    /// Replace local files that differ from the snapshot, including extra files.
    pub clear: bool,
    /// Explicit active profiles.
    pub profile: Option<String>,
}

/// The verified outcome of `proj restore`.
#[derive(Debug, Serialize)]
pub struct RestoreOutcome {
    /// The build version restored; None when no raw directory is declared.
    pub version: Option<String>,
    /// The remote consulted (`github:owner/repo@tag` or `local:<title>`), if any.
    pub source: Option<String>,
    /// Each label's final directory.
    pub labels: Vec<crate::get::Installed>,
}

/// The version restore targets: the requested one, else the checkout's completed
/// VERSION, else the newest completed production build recorded in the checkout
/// (projr reads `.projr/BUILDLOG.md`; projcli reads `.projr/attempts`, then a
/// projr BUILDLOG).
fn restore_version(root: &Utf8Path, version: Option<&str>) -> anyhow::Result<String> {
    if let Some(version) = version {
        return crate::get::check_version(version);
    }
    if let Some(version) = crate::get::completed_version_file(root) {
        return Ok(version);
    }
    crate::get::project_record(root, None)?
        .version
        .context("No completed build recorded in this checkout. Supply --version.")
}

/// Restores the raw inputs a checked-out build used (projr `projr_restore`,
/// SATVILab/projr@5f915ef `R/restore-build.R:27-170`).
///
/// The checkout's `.projr/manifests` (or a projr-written legacy `manifest.csv`)
/// names each label's files and MD5s at the build version. Directories that
/// already match are left alone and need no network access; recorded empty
/// directories need none either. Other labels come from the selected remote:
/// GitHub `archive` by default, a registered GitHub title (its destination as
/// recorded by that build, so `@version`, `@uuid` and `@branch` resolve), a
/// concrete tag in the checkout's repository, or a registered `local:<title>`.
/// An identical snapshot recorded under an earlier version is used when the exact
/// version's archive is missing. Every label is staged and verified before any
/// destination is replaced. Without `clear`, local changes are an error.
/// A remote snapshot missing from the checkout is recorded in its manifest store.
pub fn restore(root: &Utf8Path, request: &RestoreRequest) -> anyhow::Result<RestoreOutcome> {
    let profiles = crate::run_context::select_profiles(request.profile.as_deref(), root)?;
    crate::yml::require_active_configuration_for_profiles(root, &profiles)?;
    let raw = crate::yml::get_combined_yml(Some(&profiles.join(",")), root)?;
    let config = projected_config(&raw, root)?;
    let declared: Vec<String> = raw
        .get("directories")
        .and_then(Value::as_object)
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    let mut labels: Vec<String> = Vec::new();
    if request.labels.is_empty() {
        labels.extend(
            declared
                .iter()
                .filter(|label| crate::get::is_raw(label))
                .cloned(),
        );
    } else {
        for label in &request.labels {
            anyhow::ensure!(
                declared.contains(label),
                "Restore label is not declared: {label}"
            );
            if !labels.contains(label) {
                labels.push(label.clone());
            }
        }
    }
    crate::get::check_labels(&labels)?;
    if labels.is_empty() {
        return Ok(RestoreOutcome {
            version: None,
            source: None,
            labels: Vec::new(),
        });
    }
    // The build writer claim keeps restore from racing a build or another
    // restore (projr locks the output directory, `R/get-install.R:69-75`). It
    // fails closed on an existing claim, including inside a running build.
    let _claim = crate::attempt::Attempt::acquire(root)
        .context("Cannot restore while another build or restore owns this project")?;
    let version = restore_version(root, request.version.as_deref())?;
    let paths = labels
        .iter()
        .map(|label| config.get_path(root, label))
        .collect::<anyhow::Result<Vec<_>>>()?;
    crate::get::check_destinations(Some(root), &paths, &[])?;
    let project = crate::get::metadata_source(root);
    let mut snapshots = Vec::new();
    for label in &labels {
        snapshots.push(crate::get::snapshot(
            &project.store,
            &project.csv,
            label,
            &version,
        )?);
    }
    let mut todo = Vec::new();
    let mut release = Vec::new();
    for (index, snapshot) in snapshots.iter().enumerate() {
        let files = snapshot.as_ref().and_then(|s| s.files.as_ref());
        let matches = match files {
            Some(files) => {
                paths[index].is_dir()
                    && crate::get::hash_dir(&paths[index], &labels[index])? == *files
            }
            None => false,
        };
        todo.push(!matches);
        // Recorded empty directories need no remote access.
        release.push(!matches && files.is_none_or(|files| !files.is_empty()));
    }
    let (_guard, workspace) = crate::get::workspace()?;
    let transfer = crate::get::Transfer {
        workspace: &workspace,
    };
    let mut sources = vec![project];
    let mut consulted = None;
    if release.iter().any(|needed| *needed)
        && let Some(source) = remote_source(root, &raw, &version, request, &transfer)?
    {
        consulted = Some(source.name.clone());
        sources.push(source);
    }
    let selected: Vec<usize> = (0..labels.len()).filter(|i| todo[*i]).collect();
    let mut installed = Vec::new();
    if !selected.is_empty() {
        let chosen_labels: Vec<String> = selected.iter().map(|i| labels[*i].clone()).collect();
        let chosen_paths: Vec<Utf8PathBuf> = selected.iter().map(|i| paths[*i].clone()).collect();
        installed = crate::get::install(
            crate::get::Install {
                sources: &sources,
                labels: &chosen_labels,
                paths: &chosen_paths,
                version: &version,
                overwrite: request.clear,
                option: "--clear",
            },
            &transfer,
        )?;
    }
    // Record snapshots recovered from the remote in the checkout's manifest store.
    let mut recovered = Vec::new();
    if sources.len() > 1 {
        let remote = &sources[sources.len() - 1];
        let project = &sources[0];
        for index in 0..labels.len() {
            let known = snapshots[index]
                .as_ref()
                .is_some_and(|snapshot| snapshot.files.is_some());
            if !release[index] || known {
                continue;
            }
            let Some(snapshot) =
                crate::get::snapshot(&remote.store, &remote.csv, &labels[index], &version)?
            else {
                continue;
            };
            if let Some(first) = snapshot.versions.first()
                && *first != version
                && crate::get::snapshot(&project.store, &project.csv, &labels[index], first)?
                    .is_none()
            {
                recovered.extend(snapshot.rows.iter().cloned().map(|mut row| {
                    row.version = first.clone();
                    row
                }));
            }
            recovered.extend(snapshot.rows);
        }
    }
    if !recovered.is_empty() {
        crate::manifest_store::write_project(root, &recovered)?;
    }
    if crate::version::version_get_from(root).is_none() && !root.join("VERSION").exists() {
        crate::version::version_set_at(root, &version)?;
    }
    let mut outcomes = Vec::new();
    for (index, label) in labels.iter().enumerate() {
        match installed.iter().position(|item| item.label == *label) {
            Some(at) => outcomes.push(installed.remove(at)),
            None => outcomes.push(crate::get::Installed {
                label: label.clone(),
                path: paths[index].clone(),
                status: "verified",
            }),
        }
    }
    Ok(RestoreOutcome {
        version: Some(version),
        source: consulted,
        labels: outcomes,
    })
}

/// The remote `restore --destination` names. A registered GitHub title uses the
/// destination its build recorded; an unregistered name is a concrete tag in the
/// checkout's repository; `local:<title>` reads a registered local remote.
fn remote_source(
    root: &Utf8Path,
    raw: &Value,
    version: &str,
    request: &RestoreRequest,
    transfer: &crate::get::Transfer<'_>,
) -> anyhow::Result<Option<crate::get::Source>> {
    let destination = request.destination.as_deref().unwrap_or("archive");
    anyhow::ensure!(
        !destination.trim().is_empty() && !destination.chars().any(char::is_control),
        "Invalid restore destination"
    );
    let registered = |provider: &str, title: &str| raw["remotes"][provider].get(title).cloned();
    let record = crate::get::project_record(root, Some(version))?;
    let head = crate::template::Templates::checkout(root);
    let templates =
        crate::template::Templates::for_record(Some(version.to_owned()), head, record.uuid.clone());
    if let Some(title) = destination.strip_prefix("local:") {
        let value =
            registered("local", title).with_context(|| format!("Unknown local remote: {title}"))?;
        anyhow::ensure!(
            !matches!(
                value.get("structure").and_then(Value::as_str),
                Some("cas" | "xet")
            ),
            "CAS/Xet remotes restore explicit snapshots with proj remote restore --snapshot"
        );
        let definition = definition(&value)?;
        let base = match record.local.get(title) {
            Some(path) if Utf8Path::new(path).is_absolute() => Utf8PathBuf::from(path),
            Some(path) => root.join(path),
            None => crate::publication::local_base(root, definition.path.as_deref(), &templates)?.1,
        };
        return Ok(Some(crate::get::local_source(
            destination.to_owned(),
            base,
            !matches!(definition.structure, Some(Layout::Latest)),
            definition.path_append_label.unwrap_or(true),
        )));
    }
    let (title, explicit) = match destination.strip_prefix("github:") {
        Some(title) => (title, true),
        None => (destination, false),
    };
    let api = crate::get::github_api();
    let origin = || {
        repository_in(
            root,
            None,
            &api,
            &crate::run_environment::RunEnvironment::inherited(),
        )
    };
    // A title the build recorded is still its Release, even if the current
    // configuration no longer registers it (projr R/restore-build.R:101-103).
    let target = match registered("github", title) {
        None if record.github.contains_key(title) => {
            let (repository, tag) = record.github[title].clone();
            crate::get::GithubTarget {
                repository: match repository {
                    Some(repository) => repository,
                    None => origin()?,
                },
                tag,
                prefix: None,
                append: true,
                archive: true,
                direct_tag: false,
            }
        }
        Some(value) => {
            anyhow::ensure!(
                !matches!(
                    value.get("structure").and_then(Value::as_str),
                    Some("cas" | "xet")
                ),
                "CAS/Xet remotes restore explicit snapshots with proj remote restore --snapshot"
            );
            let definition = definition(&value)?;
            let (repository, tag) = match record.github.get(title) {
                Some((repository, tag)) => (repository.clone(), tag.clone()),
                None => (
                    None,
                    crate::github_publication::release_tag(title, &definition, &templates)?,
                ),
            };
            crate::get::GithubTarget {
                repository: match repository.or(definition.repo.clone()) {
                    Some(repository) => repository,
                    None => origin()?,
                },
                tag,
                prefix: definition.path.clone(),
                append: definition.path_append_label.unwrap_or(true),
                archive: !matches!(definition.structure, Some(Layout::Latest)),
                direct_tag: false,
            }
        }
        None => {
            anyhow::ensure!(!explicit, "Unknown github remote: {title}");
            crate::get::GithubTarget {
                repository: origin()?,
                tag: title.to_owned(),
                prefix: None,
                append: true,
                archive: true,
                direct_tag: true,
            }
        }
    };
    crate::get::github_source(target, transfer)
}
