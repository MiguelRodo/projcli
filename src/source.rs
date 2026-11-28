//! Raw-input sources: `build.source` and `proj source` (SATVILab/projr@5f915ef
//! `R/source.R`, `R/yml-dest.R:699-747`), plus `proj restore-repo`
//! (`R/restore-repo.R`).
//!
//! A source is a registered remote. Its `get` settings, then `build.source`'s
//! global options, then the per-remote entry choose the version (`latest`, `x`,
//! `x.y` or `x.y.z`), cue, inspect, strategy and conflict policy. Planning reads
//! remote metadata and downloads into a private workspace, verifying every file
//! against the remote's build snapshot, and changes nothing locally; local edits
//! since the last fetch (recorded in projr's Git-ignored `.projr/source/<label>.json`)
//! are conflicts. Installation then stages each changed label beside its
//! directory and replaces all of them with the shared rename-swap from
//! [`crate::get`], so a failure part-way restores the earlier labels.
use crate::get::{self, Files, Provider};
use crate::yml::ProjConfig;
use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use serde_json::{Map, Value};
use std::{collections::BTreeMap, fs};

/// Fetch settings and their allowed values, the default first (projr
/// `.opt_remote_fetch_get`, SATVILab/projr@5f915ef `R/misc.R:787-797`).
const FETCH: [(&str, &[&str]); 4] = [
    ("cue", &["if-empty", "never", "if-change", "always"]),
    ("inspect", &["manifest", "file", "none"]),
    (
        "strategy",
        &[
            "sync-diff",
            "sync-purge",
            "download-all",
            "download-missing",
        ],
    ),
    ("conflict", &["error", "local", "remote"]),
];

/// Keys `build.source` accepts globally or per remote.
const OPTIONS: [&str; 6] = [
    "content", "version", "cue", "inspect", "strategy", "conflict",
];

fn version_request(text: &str) -> bool {
    text == "latest"
        || (text.split('.').count() <= 3
            && text
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())))
}

/// Validates fetch settings (`remotes.<type>.<title>.get` or one `build.source`
/// remote): `version` is `latest` or an `x`, `x.y` or `x.y.z` version; the others
/// take projr's values (SATVILab/projr@5f915ef `R/yml-check.R:437-456`).
pub(crate) fn check_fetch(settings: &Map<String, Value>, key: &str) -> Result<()> {
    for (name, value) in settings {
        if name == "content" {
            continue;
        }
        if value.is_null() {
            continue;
        }
        let text = value.as_str().with_context(|| {
            format!("{key}.{name} must be a string (quote versions such as \"1.2\")")
        })?;
        if name == "version" {
            anyhow::ensure!(
                version_request(text),
                "{key}.version must be latest or an x, x.y or x.y.z version"
            );
            continue;
        }
        let allowed = FETCH
            .iter()
            .find(|(option, _)| option == name)
            .map(|(_, allowed)| *allowed)
            .with_context(|| {
                format!("{key} accepts version, cue, inspect, strategy and conflict")
            })?;
        anyhow::ensure!(
            allowed.contains(&text),
            "{key}.{name} must be one of {}",
            allowed.join(", ")
        );
    }
    Ok(())
}

/// Effective fetch settings for one source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Settings {
    /// `latest`, `x`, `x.y` or `x.y.z`.
    pub version: String,
    /// `if-empty`, `never`, `if-change` or `always`.
    pub cue: String,
    /// `manifest`, `file` or `none`.
    pub inspect: String,
    /// `sync-diff`, `sync-purge`, `download-all` or `download-missing`.
    pub strategy: String,
    /// `error`, `local` or `remote`.
    pub conflict: String,
}

impl Settings {
    fn apply(&mut self, layer: &Map<String, Value>) {
        for (name, value) in layer {
            let Some(text) = value.as_str() else { continue };
            let slot = match name.as_str() {
                "version" => &mut self.version,
                "cue" => &mut self.cue,
                "inspect" => &mut self.inspect,
                "strategy" => &mut self.strategy,
                "conflict" => &mut self.conflict,
                _ => continue,
            };
            *slot = text.to_owned();
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: "latest".into(),
            cue: "if-empty".into(),
            inspect: "manifest".into(),
            strategy: "sync-diff".into(),
            conflict: "error".into(),
        }
    }
}

/// One remote that supplies raw inputs, keyed `<type>:<title>`.
#[derive(Clone, Debug)]
pub(crate) struct Target {
    provider: &'static str,
    title: String,
    /// Labels fetched from this remote.
    content: Vec<String>,
    /// Whether the labels were named (a label missing at the remote is then an error).
    explicit: bool,
    settings: Settings,
}

impl Target {
    fn id(&self) -> String {
        format!("{}:{}", self.provider, self.title)
    }
}

fn raw_labels(config: &ProjConfig) -> Vec<String> {
    config
        .directories
        .keys()
        .filter(|label| get::is_raw(label))
        .cloned()
        .collect()
}

fn ordinary(config: &ProjConfig, provider: &str, title: &str) -> bool {
    crate::remote_registry::definition_of(config, provider, title).is_some_and(|value| {
        !matches!(
            value.get("structure").and_then(Value::as_str),
            Some("cas" | "xet")
        )
    }) && !(provider == "local"
        && config
            .remotes
            .local
            .as_ref()
            .is_some_and(|legacy| legacy.contains_key(title)))
}

/// Resolves a `build.source`-shaped value to its targets (projr
/// `.yml_build_source_get` and `.yml_build_check_source`). Precedence: the
/// per-remote entry, then the global options, then the remote's `get`, then
/// projr's defaults. Content defaults to every declared raw directory.
fn targets(config: &ProjConfig, value: &Value) -> Result<Vec<Target>> {
    let (global, entries) = crate::remote_registry::role_parse(value, "build.source", &OPTIONS)?;
    let raw = raw_labels(config);
    let mut result: Vec<Target> = Vec::new();
    for (reference, entry) in entries {
        let (provider, title) =
            crate::remote_registry::resolve(config, &reference, "build.source")?;
        let id = format!("{provider}:{title}");
        anyhow::ensure!(
            !result.iter().any(|target| target.id() == id),
            "build.source names remote {id} more than once"
        );
        anyhow::ensure!(
            ordinary(config, provider, &title),
            "build.source names {id}, which is a CAS or Xet store; raw inputs come from ordinary local or GitHub archives"
        );
        let get = crate::remote_registry::definition_of(config, provider, &title)
            .and_then(|definition| definition.get("get"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let key = format!("build.source.{id}");
        let mut merged = get;
        for layer in [&global, &entry] {
            for (name, value) in layer {
                if !value.is_null() {
                    merged.insert(name.clone(), value.clone());
                }
            }
        }
        check_fetch(&merged, &key)?;
        let mut settings = Settings::default();
        settings.apply(&merged);
        let (content, explicit) = match merged.get("content") {
            None | Some(Value::Null) => (raw.clone(), false),
            Some(Value::String(label)) => (vec![label.clone()], true),
            Some(Value::Array(labels)) => (
                labels
                    .iter()
                    .map(|label| {
                        label
                            .as_str()
                            .map(str::to_owned)
                            .with_context(|| format!("{key}.content must list label names"))
                    })
                    .collect::<Result<Vec<_>>>()?,
                true,
            ),
            Some(_) => anyhow::bail!("{key}.content must list label names"),
        };
        for label in &content {
            anyhow::ensure!(
                raw.contains(label),
                "{key}.content must name declared raw directories; {label} is not one"
            );
        }
        result.push(Target {
            provider,
            title,
            content,
            explicit,
            settings,
        });
    }
    Ok(result)
}

/// The remotes selected under `build.source`, validated without any transport.
pub(crate) fn configured(config: &ProjConfig) -> Result<Vec<Target>> {
    match &config.build.source {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(value) => targets(config, value),
    }
}

/// projr's fetch record, `.projr/source/<label>.json` (Git-ignored).
#[derive(Default)]
struct Record {
    version: Option<String>,
    files: Option<Files>,
}

fn record_path(root: &Utf8Path, label: &str) -> Utf8PathBuf {
    root.join(".projr/source").join(format!("{label}.json"))
}

/// Reads a fetch record written by projr or projcli. A malformed record is an
/// error rather than a silently missing baseline.
fn read_record(root: &Utf8Path, label: &str) -> Result<Record> {
    let path = record_path(root, label);
    if !crate::yml::configuration_entry_exists(&path)? {
        return Ok(Record::default());
    }
    let text = crate::fs_utils::read_managed_text(&path)?;
    let value: Value =
        serde_json::from_str(&text).with_context(|| format!("Invalid fetch record {path}"))?;
    let invalid = || format!("Invalid fetch record {path}");
    let files = match value.get("files") {
        None | Some(Value::Null) => None,
        // jsonlite writes an empty named list as [].
        Some(Value::Array(items)) if items.is_empty() => Some(Vec::new()),
        Some(Value::Object(map)) => {
            let mut files = Vec::new();
            for (name, hash) in map {
                files.push((
                    name.clone(),
                    hash.as_str().with_context(invalid)?.to_owned(),
                ));
            }
            files.sort();
            Some(files)
        }
        Some(_) => anyhow::bail!(invalid()),
    };
    Ok(Record {
        version: match value.get("version") {
            Some(Value::String(version)) => Some(version.clone()),
            None | Some(Value::Null) => None,
            Some(_) => anyhow::bail!(invalid()),
        },
        files,
    })
}

fn write_record(root: &Utf8Path, plan: &LabelPlan) -> Result<()> {
    let directory = root.join(".projr/source");
    crate::path_safety::resolve_path(directory.as_std_path(), true)?;
    fs::create_dir_all(&directory).with_context(|| format!("Cannot create {directory}"))?;
    let files: Map<String, Value> = plan
        .files
        .iter()
        .flatten()
        .map(|(name, hash)| (name.clone(), Value::String(hash.clone())))
        .collect();
    let record = serde_json::json!({
        "label": plan.label,
        "remote": plan.remote,
        "version": plan.version,
        "snapshot": plan.snapshot,
        "fetched": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        "kept": if plan.kept.is_empty() { Value::Null } else { serde_json::to_value(&plan.kept)? },
        "files": files,
    });
    let mut bytes = serde_json::to_vec_pretty(&record)?;
    bytes.push(b'\n');
    crate::fs_utils::write_managed_file(record_path(root, &plan.label), bytes)
}

/// What a label's fetch does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    /// `if-empty` with local files: left alone.
    Present,
    /// The pinned exact version is already in place: no network access.
    Pinned,
    /// Already matches the remote.
    Current,
    /// Files are written or removed.
    Update,
}

impl Status {
    fn name(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Pinned => "pinned",
            Self::Current => "current",
            Self::Update => "update",
        }
    }
}

struct LabelPlan {
    label: String,
    remote: String,
    path: Utf8PathBuf,
    version: Option<String>,
    previous: Option<String>,
    status: Status,
    snapshot: Option<String>,
    /// The remote's files (for the record).
    files: Option<Files>,
    /// Every local file when planned; installation fails if they change.
    local: Files,
    write: Vec<String>,
    delete: Vec<String>,
    kept: Vec<String>,
    staging: Option<Utf8PathBuf>,
}

/// A checked fetch plan. Its private workspace holds the verified downloads
/// until [`install`] applies them.
pub(crate) struct Plan {
    labels: Vec<LabelPlan>,
    _workspace: Option<tempfile::TempDir>,
}

impl Plan {
    /// A plan that fetches nothing.
    pub(crate) fn empty() -> Self {
        Self {
            labels: Vec::new(),
            _workspace: None,
        }
    }
}

/// Local edits that a fetch would overwrite or delete, under `conflict: error`.
#[derive(Debug)]
pub struct Conflict(String);

impl std::fmt::Display for Conflict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Conflict {}

fn versions_of(source: &get::Source, label: Option<&str>) -> Result<Vec<String>> {
    let mut versions: Vec<String> = if crate::manifest_store::ready(&source.store)? {
        crate::manifest_store::read_index(&source.store, label)?
            .into_iter()
            .map(|entry| entry.version)
            .collect()
    } else {
        crate::manifest::read(&source.csv)?
            .into_iter()
            .filter(|row| label.is_none_or(|label| row.label == label))
            .map(|row| row.version)
            .collect()
    };
    versions.sort_by_key(|version| get::version_key(version));
    versions.dedup();
    Ok(versions)
}

/// The newest completed version within a requested major (`x`), minor (`x.y`)
/// or exact (`x.y.z`) version (projr `.source_version_match`).
fn version_match(source: &get::Source, requested: &str, id: &str) -> Result<String> {
    let prefix = get::version_key(requested);
    versions_of(source, None)?
        .into_iter()
        .rfind(|version| {
            let parts = get::version_key(version);
            if prefix.len() == 3 {
                parts == prefix
            } else {
                parts.len() >= prefix.len() && parts[..prefix.len()] == prefix[..]
            }
        })
        .with_context(|| {
            format!("build.source: {id} has no completed build matching version {requested}.")
        })
}

/// The label's state at `version`: its own row, or the newest earlier row when
/// that build did not resend an unchanged label (projr `.source_label_version`).
fn label_version(source: &get::Source, label: &str, version: &str) -> Result<Option<String>> {
    let limit = get::version_key(version);
    Ok(versions_of(source, Some(label))?
        .into_iter()
        .rfind(|candidate| get::version_key(candidate) <= limit))
}

struct Planning<'a> {
    root: &'a Utf8Path,
    config: &'a crate::yml::ValidatedConfig,
    profile: Option<&'a str>,
    workspace: &'a Utf8Path,
}

fn local_files(context: &Planning<'_>, label: &str) -> Result<(Utf8PathBuf, Files)> {
    let path = context.config.get_path(context.root, label)?;
    let files = get::hash_dir(&path, label)?;
    Ok((path, files))
}

fn plan_remote(
    context: &Planning<'_>,
    target: &Target,
    plans: &mut Vec<LabelPlan>,
    conflicts: &mut Vec<String>,
) -> Result<()> {
    let id = target.id();
    let settings = &target.settings;
    let bare = |label: &str, status: Status| -> Result<LabelPlan> {
        let (path, local) = local_files(context, label)?;
        Ok(LabelPlan {
            label: label.to_owned(),
            remote: id.clone(),
            path,
            version: None,
            previous: None,
            status,
            snapshot: None,
            files: None,
            local,
            write: Vec::new(),
            delete: Vec::new(),
            kept: Vec::new(),
            staging: None,
        })
    };
    if settings.cue == "if-empty" {
        // Folders with files are left alone, so the remote is skipped (works offline).
        let mut present = true;
        for label in &target.content {
            present &= !local_files(context, label)?.1.is_empty();
        }
        if present {
            for label in &target.content {
                plans.push(bare(label, Status::Present)?);
            }
            return Ok(());
        }
    }
    let requested = &settings.version;
    if requested.split('.').count() == 3 && settings.cue != "always" {
        // A pinned build already in place needs no network access.
        let mut current = true;
        for label in &target.content {
            let record = read_record(context.root, label)?;
            current &= record.version.as_deref() == Some(get::v_add(requested).as_str())
                && record.files.as_ref() == Some(&local_files(context, label)?.1);
        }
        if current {
            for label in &target.content {
                plans.push(bare(label, Status::Pinned)?);
            }
            return Ok(());
        }
    }
    let transfer = get::Transfer {
        workspace: context.workspace,
    };
    let provider = if target.provider == "local" {
        Provider::Local
    } else {
        Provider::Github
    };
    let resolved = get::resolve(
        context.root,
        provider,
        &get::GetRequest {
            direct: None,
            remote: Some(id.clone()),
            tag: None,
            labels: target.content.clone(),
            version: None,
            out: Utf8PathBuf::new(),
            overwrite: false,
            profile: context.profile.map(str::to_owned),
        },
        &transfer,
    )?
    .with_context(|| format!("build.source: {id} has not published a manifest."))?;
    let store = &resolved.sources[0];
    let version = if requested == "latest" {
        resolved.version.clone()
    } else {
        version_match(store, requested, &id)?
    };
    for label in &target.content {
        let Some(at) = label_version(store, label, &version)? else {
            let message = format!("{id} has no {label} in build {version}.");
            anyhow::ensure!(!target.explicit, "build.source: {message}");
            eprintln!("Skipping {label}: {message}");
            continue;
        };
        if at != version {
            eprintln!("{id} last published {label} in {at}; using it for build {version}.");
        }
        let plan = plan_label(context, target, label, &at, &resolved)?;
        if let Some(conflict) = plan.1 {
            conflicts.push(conflict);
        }
        plans.push(plan.0);
    }
    Ok(())
}

fn plan_label(
    context: &Planning<'_>,
    target: &Target,
    label: &str,
    version: &str,
    resolved: &get::Resolved,
) -> Result<(LabelPlan, Option<String>)> {
    let settings = &target.settings;
    let id = target.id();
    let (path, local) = local_files(context, label)?;
    let record = read_record(context.root, label)?;
    let mut plan = LabelPlan {
        label: label.to_owned(),
        remote: id.clone(),
        path,
        version: Some(version.to_owned()),
        previous: record.version.clone(),
        status: Status::Current,
        snapshot: None,
        files: None,
        local: local.clone(),
        write: Vec::new(),
        delete: Vec::new(),
        kept: Vec::new(),
        staging: None,
    };
    if settings.cue == "if-empty" && !local.is_empty() {
        plan.status = Status::Present;
        plan.version = None;
        plan.previous = None;
        return Ok((plan, None));
    }
    let store = &resolved.sources[0];
    let snapshot = get::snapshot(&store.store, &store.csv, label, version)?
        .with_context(|| format!("{id} has no {label} snapshot at {version}."))?;
    plan.snapshot = snapshot.id.clone();
    let staging = context.workspace.join("files").join(label);
    let mut downloaded = false;
    let download = |downloaded: &mut bool| -> Result<()> {
        if !*downloaded {
            fs::create_dir_all(context.workspace.join("files"))?;
            get::install(
                get::Install {
                    sources: &resolved.sources,
                    labels: &[label.to_owned()],
                    paths: std::slice::from_ref(&staging),
                    version,
                    overwrite: true,
                    option: "--conflict remote",
                },
                &get::Transfer {
                    workspace: context.workspace,
                },
            )?;
            *downloaded = true;
        }
        Ok(())
    };
    // inspect: file confirms even an empty build from the remote's own files.
    let remote = match (&snapshot.files, settings.inspect.as_str()) {
        (Some(files), "manifest") => files.clone(),
        _ => {
            download(&mut downloaded)?;
            get::hash_dir(&staging, label)?
        }
    };
    plan.files = Some(remote.clone());
    if settings.cue == "if-change" && settings.inspect != "none" && local == remote {
        return Ok((plan, None));
    }
    let remote_map: BTreeMap<&str, &str> = remote
        .iter()
        .map(|(name, hash)| (name.as_str(), hash.as_str()))
        .collect();
    let local_map: BTreeMap<&str, &str> = local
        .iter()
        .map(|(name, hash)| (name.as_str(), hash.as_str()))
        .collect();
    let changed: Vec<String> = remote
        .iter()
        .filter(|(name, hash)| local_map.get(name.as_str()) != Some(&hash.as_str()))
        .map(|(name, _)| name.clone())
        .collect();
    let mut write: Vec<String> = if settings.strategy == "download-missing" {
        remote
            .iter()
            .filter(|(name, _)| !local_map.contains_key(name.as_str()))
            .map(|(name, _)| name.clone())
            .collect()
    } else {
        changed
    };
    // sync-purge behaves as sync-diff: the resulting files are the same (projr).
    let mut delete: Vec<String> = if settings.strategy.starts_with("sync-") {
        local
            .iter()
            .filter(|(name, _)| !remote_map.contains_key(name.as_str()))
            .map(|(name, _)| name.clone())
            .collect()
    } else {
        Vec::new()
    };
    let touched: Vec<&String> = write
        .iter()
        .filter(|name| local_map.contains_key(name.as_str()))
        .chain(delete.iter())
        .collect();
    let edited: Vec<String> = match &record.files {
        None => touched.into_iter().cloned().collect(),
        Some(base) => {
            let base: BTreeMap<&str, &str> = base
                .iter()
                .map(|(name, hash)| (name.as_str(), hash.as_str()))
                .collect();
            touched
                .into_iter()
                .filter(|name| base.get(name.as_str()) != local_map.get(name.as_str()))
                .cloned()
                .collect()
        }
    };
    let mut conflict = None;
    if !edited.is_empty() {
        match settings.conflict.as_str() {
            "error" => {
                let more = if edited.len() > 5 {
                    format!(" and {} more", edited.len() - 5)
                } else {
                    String::new()
                };
                conflict = Some(format!(
                    "{label} ({id}): local changes would be overwritten or deleted: {}{more}.",
                    edited
                        .iter()
                        .take(5)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            "local" => {
                write.retain(|name| !edited.contains(name));
                delete.retain(|name| !edited.contains(name));
                plan.kept = edited;
            }
            _ => {}
        }
    }
    plan.status = if write.is_empty() && delete.is_empty() {
        Status::Current
    } else {
        Status::Update
    };
    if !write.is_empty() && conflict.is_none() {
        download(&mut downloaded)?;
        plan.staging = Some(staging);
    }
    plan.write = write;
    plan.delete = delete;
    Ok((plan, conflict))
}

/// Plans a fetch for `targets` (projr `.source_plan`): checks every selected
/// label and downloads what is needed into a private workspace, verified
/// against the remote's build snapshot. Nothing local changes. Labels supplied
/// by two remotes, or overlapping directories, are errors; so are local edits
/// under `conflict: error` (a [`Conflict`]).
pub(crate) fn plan(
    root: &Utf8Path,
    config: &crate::yml::ValidatedConfig,
    profile: Option<&str>,
    targets: &[Target],
) -> Result<Plan> {
    let targets: Vec<&Target> = targets
        .iter()
        .filter(|target| target.settings.cue != "never")
        .collect();
    if targets.is_empty() {
        return Ok(Plan::empty());
    }
    eprintln!("Checking raw-input sources");
    let mut labels: Vec<&String> = Vec::new();
    for target in &targets {
        for label in &target.content {
            anyhow::ensure!(
                !labels.contains(&label),
                "build.source selects {label} from more than one remote."
            );
            labels.push(label);
        }
    }
    let owned: Vec<String> = labels.iter().map(|label| (*label).clone()).collect();
    get::check_labels(&owned)?;
    let paths = owned
        .iter()
        .map(|label| config.get_path(root, label))
        .collect::<Result<Vec<_>>>()?;
    get::check_destinations(Some(root), &paths, &[])
        .context("build.source selects overlapping or protected directories")?;
    let (guard, workspace) = get::workspace()?;
    let context = Planning {
        root,
        config,
        profile,
        workspace: &workspace,
    };
    let mut plans = Vec::new();
    let mut conflicts = Vec::new();
    for target in targets {
        plan_remote(&context, target, &mut plans, &mut conflicts)?;
    }
    if !conflicts.is_empty() {
        return Err(Conflict(format!(
            "{}\nRun proj source to choose, or set conflict: local to keep these files or conflict: remote to replace them.",
            conflicts.join("\n")
        ))
        .into());
    }
    Ok(Plan {
        labels: plans,
        _workspace: Some(guard),
    })
}

/// One label's fetch outcome.
#[derive(Debug, Serialize)]
pub struct LabelOutcome {
    /// Directory label.
    pub label: String,
    /// The remote, `<type>:<title>`.
    pub remote: String,
    /// The build the files come from; None when the label was left alone.
    pub version: Option<String>,
    /// `present`, `pinned`, `current` or `update`.
    pub status: &'static str,
    /// Files written.
    pub written: usize,
    /// Files removed.
    pub removed: usize,
    /// Changed local files kept under `conflict: local`.
    pub kept: usize,
}

fn prune_empty(root: &Utf8Path, relative: &str) -> Result<()> {
    let mut current = Utf8Path::new(relative).parent();
    while let Some(directory) = current.filter(|directory| !directory.as_str().is_empty()) {
        let path = root.join(directory);
        if fs::read_dir(&path)?.next().is_some() {
            break;
        }
        fs::remove_dir(&path)?;
        current = directory.parent();
    }
    Ok(())
}

/// Applies a checked plan (projr `.source_install`): every changed label is
/// staged beside its directory (a copy of the local files with the plan's
/// deletions and verified writes) and checked against the expected result, then
/// all are replaced together; a directory that changed since planning stops the
/// fetch before anything is replaced. Fetch records are written for current and
/// updated labels.
pub(crate) fn install(root: &Utf8Path, plan: &Plan) -> Result<Vec<LabelOutcome>> {
    let mut stages = Vec::new();
    let labels: Vec<String> = plan.labels.iter().map(|p| p.label.clone()).collect();
    let paths: Vec<Utf8PathBuf> = plan.labels.iter().map(|p| p.path.clone()).collect();
    for (index, label) in plan.labels.iter().enumerate() {
        if label.status != Status::Update {
            continue;
        }
        let stage = get::Stage::beside(&label.path, index, label.local.clone())?;
        let new = stage.new_dir()?;
        if label.path.is_dir() {
            crate::fs_utils::copy_dir_recursive(&label.path, &new)?;
        } else {
            fs::create_dir(&new)?;
        }
        let mut expected: BTreeMap<String, String> = label.local.iter().cloned().collect();
        for name in &label.delete {
            fs::remove_file(new.join(name)).with_context(|| format!("Cannot stage {name}"))?;
            prune_empty(&new, name)?;
            expected.remove(name);
        }
        let remote: BTreeMap<&str, &str> = label
            .files
            .iter()
            .flatten()
            .map(|(name, hash)| (name.as_str(), hash.as_str()))
            .collect();
        for name in &label.write {
            let staging = label
                .staging
                .as_ref()
                .context("Missing verified download")?;
            let target = new.join(name);
            fs::create_dir_all(target.parent().context("Invalid file path")?)?;
            crate::path_safety::resolve_path(target.as_std_path(), true)?;
            fs::copy(staging.join(name), &target)
                .with_context(|| format!("Cannot stage {name}"))?;
            expected.insert(name.clone(), remote[name.as_str()].to_owned());
        }
        let expected: Files = expected.into_iter().collect();
        anyhow::ensure!(
            get::hash_dir(&new, &label.label)? == expected,
            "Staged {} does not match the planned files; nothing was replaced",
            label.label
        );
        stages.push(stage);
    }
    get::commit(stages, &labels, &paths)?;
    let mut outcomes = Vec::new();
    for label in &plan.labels {
        if matches!(label.status, Status::Current | Status::Update) {
            write_record(root, label)?;
        }
        if label.status == Status::Update {
            eprintln!(
                "Updated {} from {}: {} -> {} ({} written, {} removed{}).",
                label.label,
                label.remote,
                label.previous.as_deref().unwrap_or("none"),
                label.version.as_deref().unwrap_or("unknown"),
                label.write.len(),
                label.delete.len(),
                if label.kept.is_empty() {
                    String::new()
                } else {
                    format!(", {} local kept", label.kept.len())
                }
            );
        }
        outcomes.push(LabelOutcome {
            label: label.label.clone(),
            remote: label.remote.clone(),
            version: label.version.clone(),
            status: label.status.name(),
            written: label.write.len(),
            removed: label.delete.len(),
            kept: label.kept.len(),
        });
    }
    Ok(outcomes)
}

/// `proj source` arguments (projr `projr_source`).
#[derive(Default)]
pub struct SourceRequest {
    /// Labels to fetch; empty uses each source's selection.
    pub labels: Vec<String>,
    /// A registered remote, `title` or `<type>:<title>`.
    pub remote: Option<String>,
    /// `latest`, `x`, `x.y`, `x.y.z` or `current`.
    pub version: Option<String>,
    /// Overrides the configured cue; default `if-change`.
    pub cue: Option<String>,
    /// Overrides the configured strategy.
    pub strategy: Option<String>,
    /// Overrides the configured inspect setting.
    pub inspect: Option<String>,
    /// Overrides the configured conflict policy; when absent, a terminal asks.
    pub conflict: Option<String>,
    /// Explicit active profiles.
    pub profile: Option<String>,
}

/// The result of `proj source`.
#[derive(Debug, Serialize)]
pub struct SourceOutcome {
    /// Each selected label.
    pub labels: Vec<LabelOutcome>,
}

/// Loads the project configuration for source and restore-style commands,
/// ignoring upload selection (it never limits sources).
fn load(
    root: &Utf8Path,
    profile: Option<&str>,
) -> Result<(Vec<String>, ProjConfig, crate::yml::ValidatedConfig)> {
    let profiles = crate::run_context::select_profiles(profile, root)?;
    crate::yml::require_active_configuration_for_profiles(root, &profiles)?;
    let raw = crate::yml::get_combined_yml(Some(&profiles.join(",")), root)?;
    let config: ProjConfig =
        serde_json::from_value(raw.clone()).context("Invalid project configuration")?;
    let validated = crate::restore::projected_config(&raw, root)?;
    Ok((profiles, config, validated))
}

/// Without `build.source` or a named remote, each label comes from the one
/// registered remote that has published it (projr `.source_targets_discover`).
/// Unlike projr, a remote that cannot be listed is an error rather than
/// "not published" (decision D5); a missing Release or archive is not published.
fn discover(
    root: &Utf8Path,
    config: &ProjConfig,
    labels: &[String],
    profile: Option<&str>,
) -> Result<Vec<Target>> {
    let wanted = if labels.is_empty() {
        raw_labels(config)
    } else {
        labels.to_vec()
    };
    let mut ids = Vec::new();
    for title in config.remotes.registry.local.keys() {
        ids.push(("local", title.clone()));
    }
    for (title, value) in &config.remotes.registry.github {
        if !matches!(
            value.get("structure").and_then(Value::as_str),
            Some("cas" | "xet")
        ) {
            ids.push(("github", title.clone()));
        }
    }
    anyhow::ensure!(!ids.is_empty(), "No remotes are registered under remotes.");
    let (_guard, workspace) = get::workspace()?;
    let mut holders: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (provider, title) in ids {
        let id = format!("{provider}:{title}");
        let directory = tempfile::Builder::new()
            .prefix("discover-")
            .tempdir_in(&workspace)?;
        let transfer = get::Transfer {
            workspace: Utf8Path::from_path(directory.path()).context("Workspace must be UTF-8")?,
        };
        let resolved = get::resolve(
            root,
            if provider == "local" {
                Provider::Local
            } else {
                Provider::Github
            },
            &get::GetRequest {
                direct: None,
                remote: Some(id.clone()),
                tag: None,
                labels: Vec::new(),
                version: None,
                out: Utf8PathBuf::new(),
                overwrite: false,
                profile: profile.map(str::to_owned),
            },
            &transfer,
        )
        .with_context(|| format!("Cannot list {id}"))?;
        for label in resolved.map(|resolved| resolved.labels).unwrap_or_default() {
            if wanted.contains(&label) {
                holders.entry(label).or_default().push(id.clone());
            }
        }
    }
    let missing: Vec<&String> = wanted
        .iter()
        .filter(|label| !holders.contains_key(*label))
        .collect();
    if !labels.is_empty() && !missing.is_empty() {
        anyhow::bail!(
            "No registered remote has published {}.",
            missing
                .iter()
                .map(|label| label.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let shared: Vec<String> = holders
        .iter()
        .filter(|(_, ids)| ids.len() > 1)
        .map(|(label, ids)| format!("{label} is published by {}", ids.join(" and ")))
        .collect();
    anyhow::ensure!(
        shared.is_empty(),
        "{}. Choose one with --remote.",
        shared.join("; ")
    );
    anyhow::ensure!(
        !holders.is_empty(),
        "No registered remote has published these labels."
    );
    let mut assigned: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for label in &wanted {
        if let Some(ids) = holders.get(label) {
            assigned
                .entry(ids[0].clone())
                .or_default()
                .push(label.clone());
        }
    }
    let mut result = Vec::new();
    for (id, content) in assigned {
        let mut entry = Map::new();
        entry.insert(id, serde_json::json!({ "content": content }));
        let mut target = targets(config, &Value::Object(entry))?
            .pop()
            .context("Missing discovered source")?;
        target.explicit = !labels.is_empty();
        result.push(target);
    }
    Ok(result)
}

/// The sources a `proj source` call fetches from (projr `.source_targets`).
fn request_targets(
    root: &Utf8Path,
    config: &ProjConfig,
    request: &SourceRequest,
) -> Result<Vec<Target>> {
    let configured = configured(config)?;
    if let Some(remote) = &request.remote {
        let (provider, title) = crate::remote_registry::resolve(config, remote, "--remote")?;
        let id = format!("{provider}:{title}");
        let mut target = match configured.into_iter().find(|target| target.id() == id) {
            Some(target) => target,
            None => targets(config, &Value::String(id.clone()))?
                .pop()
                .context("Missing source")?,
        };
        if !request.labels.is_empty() {
            target.content = request.labels.clone();
            target.explicit = true;
        }
        return Ok(vec![target]);
    }
    if !configured.is_empty() {
        if request.labels.is_empty() {
            return Ok(configured);
        }
        let mut result = Vec::new();
        for mut target in configured {
            target
                .content
                .retain(|label| request.labels.contains(label));
            target.explicit = true;
            if !target.content.is_empty() {
                result.push(target);
            }
        }
        let missing: Vec<&str> = request
            .labels
            .iter()
            .filter(|label| !result.iter().any(|target| target.content.contains(label)))
            .map(String::as_str)
            .collect();
        anyhow::ensure!(
            missing.is_empty(),
            "build.source does not supply {}. Name a remote.",
            missing.join(", ")
        );
        return Ok(result);
    }
    discover(root, config, &request.labels, request.profile.as_deref())
}

fn check_choice(value: &Option<String>, name: &str) -> Result<()> {
    if let Some(value) = value {
        let allowed = FETCH
            .iter()
            .find(|(option, _)| *option == name)
            .map(|(_, allowed)| *allowed)
            .unwrap_or_default();
        anyhow::ensure!(
            allowed.contains(&value.as_str()),
            "--{name} must be one of {}",
            allowed.join(", ")
        );
    }
    Ok(())
}

/// Asks on a terminal how to handle conflicting local files; None cancels.
fn ask(message: &str) -> Result<Option<String>> {
    use std::io::{BufRead, Write};
    let mut stderr = std::io::stderr();
    writeln!(
        stderr,
        "{message}\nHow should these files be handled?\n1: Keep my local files\n2: Replace them with the remote's\n3: Cancel"
    )?;
    write!(stderr, "Selection: ")?;
    stderr.flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(match line.trim() {
        "1" => Some("local".into()),
        "2" => Some("remote".into()),
        _ => None,
    })
}

/// Checks and fetches raw inputs from their remotes, outside a build (projr
/// `projr_source`, SATVILab/projr@5f915ef `R/source.R:39-103`).
///
/// Without `remote`, the remotes under `build.source` are used, else each label
/// comes from the one registered remote that published it. Arguments override
/// the configured `get` settings; the cue defaults to `if-change`. Files that
/// still match the last fetch are updated; files changed locally since then are
/// conflicts: a terminal asks whether to keep or replace them, and otherwise
/// `conflict` decides. Nothing changes until every selected label has been
/// checked. `version: current` restores the inputs recorded for this checkout's
/// build with [`crate::restore::restore`]. Holds `.projr/build.lock`.
pub fn source(
    root: &Utf8Path,
    request: &SourceRequest,
    interactive: bool,
) -> Result<SourceOutcome> {
    if let Some(version) = &request.version {
        anyhow::ensure!(
            version == "current" || version_request(version),
            "--version must be latest, current or an x, x.y or x.y.z version."
        );
    }
    for (value, name) in [
        (&request.cue, "cue"),
        (&request.strategy, "strategy"),
        (&request.inspect, "inspect"),
        (&request.conflict, "conflict"),
    ] {
        check_choice(value, name)?;
    }
    get::check_labels(&request.labels)?;
    let (profiles, config, validated) = load(root, request.profile.as_deref())?;
    let profile = profiles.join(",");
    if request.version.as_deref() == Some("current") {
        return current(root, &config, request);
    }
    let _claim = crate::attempt::Attempt::acquire(root)
        .context("Cannot fetch sources while another build or restore owns this project")?;
    let mut targets = request_targets(root, &config, request)?;
    for target in &mut targets {
        let settings = &mut target.settings;
        if let Some(version) = &request.version {
            settings.version = version.clone();
        }
        settings.cue = request.cue.clone().unwrap_or_else(|| "if-change".into());
        for (value, slot) in [
            (&request.strategy, &mut settings.strategy),
            (&request.inspect, &mut settings.inspect),
            (&request.conflict, &mut settings.conflict),
        ] {
            if let Some(value) = value {
                *slot = value.clone();
            }
        }
    }
    let plan = match plan(root, &validated, Some(&profile), &targets) {
        Err(error) if error.is::<Conflict>() && request.conflict.is_none() && interactive => {
            let Some(choice) = ask(&error.to_string())? else {
                anyhow::bail!("Cancelled. No files were changed.");
            };
            for target in &mut targets {
                target.settings.conflict = choice.clone();
            }
            plan(root, &validated, Some(&profile), &targets)?
        }
        other => other?,
    };
    Ok(SourceOutcome {
        labels: install(root, &plan)?,
    })
}

/// `version: current` restores the inputs this checkout recorded (projr
/// `.source_current`). projcli also accepts a registered local remote, as
/// `proj restore --destination local:<title>` does.
fn current(root: &Utf8Path, config: &ProjConfig, request: &SourceRequest) -> Result<SourceOutcome> {
    anyhow::ensure!(
        request.conflict.as_deref() != Some("local"),
        "--version current restores the recorded files; use --conflict error or remote."
    );
    let destination = match &request.remote {
        None => "archive".to_owned(),
        Some(remote) => match crate::remote_registry::resolve(config, remote, "--remote")? {
            ("github", title) => title,
            (_, title) => format!("local:{title}"),
        },
    };
    let outcome = crate::restore::restore(
        root,
        &crate::restore::RestoreRequest {
            labels: request.labels.clone(),
            version: None,
            destination: Some(destination.clone()),
            clear: request.conflict.as_deref() == Some("remote"),
            profile: request.profile.clone(),
        },
    )?;
    Ok(SourceOutcome {
        labels: outcome
            .labels
            .into_iter()
            .map(|installed| LabelOutcome {
                remote: outcome
                    .source
                    .clone()
                    .unwrap_or_else(|| destination.clone()),
                version: outcome.version.clone(),
                status: if installed.status == "installed" {
                    "update"
                } else {
                    "current"
                },
                label: installed.label,
                written: 0,
                removed: 0,
                kept: 0,
            })
            .collect(),
    })
}

/// `proj restore-repo` arguments (projr `projr_restore_repo`).
pub struct RestoreRepoRequest {
    /// `owner/repo`, or `repo` for one owned by the authenticated user.
    pub repo: String,
    /// Directory to clone into; defaults to the repository name.
    pub path: Option<Utf8PathBuf>,
    /// Labels to fetch.
    pub labels: Vec<String>,
    /// A registered remote, `title` or `<type>:<title>`.
    pub remote: Option<String>,
}

/// The result of `proj restore-repo`.
#[derive(Debug, Serialize)]
pub struct RestoreRepoOutcome {
    /// The clone.
    pub path: Utf8PathBuf,
    /// The fetch.
    pub source: SourceOutcome,
}

/// Clones a GitHub repository with the installed `git`, then fetches its raw
/// inputs with `cue: always` and `conflict: error` (projr `projr_restore_repo`,
/// SATVILab/projr@5f915ef `R/restore-repo.R`, with projr 6acb9b8's `remote`).
/// The clone URL is `<GITHUB_SERVER_URL or https://github.com>/<owner>/<repo>.git`;
/// Git's own credential configuration applies. A failure is reported, not
/// turned into a `FALSE` return as in projr; a completed clone is left in place.
pub fn restore_repo(request: &RestoreRepoRequest) -> Result<RestoreRepoOutcome> {
    let repo = if request.repo.contains('/') {
        request.repo.clone()
    } else {
        let token = ["GITHUB_PAT", "GH_TOKEN", "GITHUB_TOKEN"]
            .iter()
            .find_map(|key| std::env::var(key).ok().filter(|v| !v.trim().is_empty()))
            .context("A repository without an owner needs a GitHub token to find the authenticated user; name it as owner/repo")?;
        // The client needs a repository; /user does not use it.
        let login = crate::github_release::GithubReleaseClient::new(
            "user/login",
            &get::github_api(),
            Some(token),
        )?
        .login()?;
        format!("{login}/{}", request.repo)
    };
    let parts: Vec<&str> = repo.split('/').collect();
    anyhow::ensure!(
        parts.len() == 2
            && parts.iter().all(|part| {
                !part.is_empty()
                    && *part != "."
                    && *part != ".."
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            }),
        "--repo must be owner/repo or repo."
    );
    let path = request
        .path
        .clone()
        .unwrap_or_else(|| Utf8PathBuf::from(parts[1]));
    let server = std::env::var("GITHUB_SERVER_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "https://github.com".into());
    let url = format!("{}/{repo}.git", server.trim_end_matches('/'));
    let environment = crate::run_environment::RunEnvironment::inherited();
    let mut command = crate::git::git_command_in(None, &environment)?;
    if crate::is_non_interactive() {
        // A noninteractive call never waits for a credential prompt.
        command.env("GIT_TERMINAL_PROMPT", "0");
    }
    let status = command
        .args(["clone", "--quiet", "--"])
        .arg(&url)
        .arg(path.as_std_path())
        .status()
        .context("Cannot run git")?;
    anyhow::ensure!(status.success(), "git clone of {repo} failed");
    let root =
        Utf8PathBuf::try_from(fs::canonicalize(&path)?).context("Clone path must be UTF-8")?;
    let outcome = source(
        &root,
        &SourceRequest {
            labels: request.labels.clone(),
            remote: request.remote.clone(),
            cue: Some("always".into()),
            conflict: Some("error".into()),
            ..Default::default()
        },
        false,
    )
    .with_context(|| format!("Cloned into {path}, but fetching its inputs failed"))?;
    Ok(RestoreRepoOutcome {
        path: root,
        source: outcome,
    })
}
