//! projr's pre-render archive version check (SATVILab/projr@ab3ce8a
//! `R/build-remote-versions.R`): before hooks and rendering, a production build
//! refuses a proposed version that a selected archive already has or has passed.
//! Only read-only lookups are made; nothing is created or written remotely.
use crate::github_release::GithubReleaseClient;
use crate::local_remote::{compare_versions, version_numbers};
use crate::yml::{ProjConfig, RemoteCheck, RemoteType, VersionRule};
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use std::{cmp::Ordering, fs};

/// One archive whose completed project version is compared.
pub(crate) enum Archive {
    /// A local directory destination's resolved base.
    Local { title: String, base: Utf8PathBuf },
    /// A GitHub Release destination.
    Github {
        title: String,
        client: GithubReleaseClient,
        tag: String,
    },
}

impl Archive {
    fn label(&self) -> String {
        match self {
            Self::Local { title, .. } => format!("local.{title}"),
            Self::Github { title, .. } => format!("github.{title}"),
        }
    }
    fn version(&self) -> anyhow::Result<Option<String>> {
        match self {
            Self::Local { base, .. } => local_version(base),
            Self::Github { client, tag, .. } => {
                crate::github_publication::project_version(client, tag)
            }
        }
    }
}

/// Completed version of a local archive; an absent or empty folder has none.
fn local_version(base: &Utf8Path) -> anyhow::Result<Option<String>> {
    match fs::symlink_metadata(base) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("Cannot inspect {base}")),
        Ok(metadata) => anyhow::ensure!(
            metadata.is_dir(),
            "Archive destination must be a real directory: {base}"
        ),
    }
    if let Some(version) = crate::publication::project_version(&base.join("VERSION"))? {
        return Ok(Some(version));
    }
    anyhow::ensure!(
        fs::read_dir(base)?.next().is_none(),
        "The destination contains files but has no valid project VERSION; inspect it for an incomplete upload before retrying"
    );
    Ok(None)
}

/// Archives named by `not_behind_remote` titles, resolved whether or not this
/// build sends to them. Each title must be unambiguous across remote types.
fn named(
    root: &Utf8Path,
    project: &ProjConfig,
    titles: Vec<(Option<RemoteType>, String)>,
    templates: &crate::template::Templates,
    environment: &crate::run_environment::RunEnvironment,
) -> anyhow::Result<Vec<Archive>> {
    let mut archives = Vec::new();
    let mut seen = Vec::new();
    for (kind, title) in titles {
        let kind = match kind {
            Some(kind) => {
                anyhow::ensure!(
                    crate::remote_registry::titles(project, provider(kind)).contains(&title),
                    "Unknown {} remote title in build.restrictions.not_behind_remote: {title}",
                    provider(kind)
                );
                kind
            }
            None => {
                let kinds: Vec<_> = [RemoteType::Local, RemoteType::Github]
                    .into_iter()
                    .filter(|kind| {
                        crate::remote_registry::titles(project, provider(*kind)).contains(&title)
                    })
                    .collect();
                match kinds.as_slice() {
                    [kind] => *kind,
                    [] => anyhow::bail!(
                        "Unknown remote title in build.restrictions.not_behind_remote: {title}"
                    ),
                    _ => anyhow::bail!(
                        "Ambiguous remote title in build.restrictions.not_behind_remote: {title}. Qualify it by type, for example {{github: [{title}]}}"
                    ),
                }
            }
        };
        if seen.contains(&(kind, title.clone())) {
            continue;
        }
        seen.push((kind, title.clone()));
        let value = crate::remote_registry::definition_of(project, provider(kind), &title)
            .filter(|value| {
                !matches!(
                    value.get("structure").and_then(serde_json::Value::as_str),
                    Some("cas" | "xet")
                )
            })
            .with_context(|| {
                format!(
                    "Archive version checks cover ordinary local and GitHub destinations; {title} is a CAS/Xet store"
                )
            })?;
        let definition = crate::restore::definition(value)?;
        archives.push(match kind {
            RemoteType::Local => Archive::Local {
                base: crate::publication::local_base(root, definition.path.as_deref(), templates)?
                    .1,
                title,
            },
            RemoteType::Github => {
                let tag = crate::github_publication::release_tag(&title, &definition, templates)?;
                let (_, client) = crate::github_publication::connection(
                    root,
                    environment,
                    definition.repo.as_deref(),
                )?;
                Archive::Github { title, client, tag }
            }
        });
    }
    Ok(archives)
}

fn provider(kind: RemoteType) -> &'static str {
    match kind {
        RemoteType::Local => "local",
        RemoteType::Github => "github",
    }
}

/// Archives selected by `not_behind_remote` for a publishing production build: by
/// default the ordinary local and GitHub destinations it publishes to; `false`
/// selects none.
pub(crate) fn selected(
    root: &Utf8Path,
    project: &ProjConfig,
    ordinary: &[crate::publication::LocalPublication],
    github: &[crate::github_publication::Target],
    templates: &crate::template::Templates,
    environment: &crate::run_environment::RunEnvironment,
) -> anyhow::Result<Vec<Archive>> {
    let titles = match &project.build.restrictions.not_behind_remote {
        None | Some(RemoteCheck::All(true)) => {
            let mut archives: Vec<Archive> = ordinary
                .iter()
                .map(|target| Archive::Local {
                    title: target.title.clone(),
                    base: target.base.clone(),
                })
                .collect();
            archives.extend(github.iter().map(|target| Archive::Github {
                title: target.title().to_owned(),
                client: target.client.clone(),
                tag: target.tag.clone(),
            }));
            return Ok(archives);
        }
        Some(RemoteCheck::All(false)) => return Ok(Vec::new()),
        Some(RemoteCheck::Titles(titles)) => {
            titles.iter().map(|title| (None, title.clone())).collect()
        }
        Some(RemoteCheck::ByType(map)) => map
            .iter()
            .flat_map(|(kind, titles)| titles.iter().map(|title| (Some(*kind), title.clone())))
            .collect(),
    };
    named(root, project, titles, templates, environment)
}

/// Compare each archive's completed version with the proposed build version and,
/// under the base rule, with the last completed local version. All archives are
/// reported; any failure stops the build before hooks run.
pub(crate) fn check(
    archives: &[Archive],
    proposed: &str,
    rule: VersionRule,
    base: impl FnOnce() -> anyhow::Result<Option<String>>,
) -> anyhow::Result<()> {
    if archives.is_empty() {
        return Ok(());
    }
    let proposed_numbers = version_numbers(proposed).context("Invalid proposed version")?;
    let mut base = Some(base);
    let mut base_value: Option<anyhow::Result<Option<String>>> = None;
    let mut report = Vec::new();
    let mut failed = false;
    for archive in archives {
        let outcome = match archive.version() {
            Err(error) => {
                failed = true;
                format!("could not verify: {error:#}")
            }
            Ok(None) => "empty, check passed".to_owned(),
            Ok(Some(version)) => {
                let remote = version_numbers(&version).context("Invalid archive version")?;
                let reason = if compare_versions(&proposed_numbers, &remote) != Ordering::Greater {
                    Some("proposed version must be newer".to_owned())
                } else if rule == VersionRule::Base {
                    let local = base_value.get_or_insert_with(|| (base.take().unwrap())());
                    match local {
                        Err(error) => Some(format!("could not read the local base: {error:#}")),
                        Ok(Some(local))
                            if version_numbers(local).is_some_and(|local| {
                                compare_versions(&local, &remote) != Ordering::Less
                            }) =>
                        {
                            None
                        }
                        Ok(_) => Some("local base is behind".to_owned()),
                    }
                } else {
                    None
                };
                failed |= reason.is_some();
                format!("{version}, {}", reason.as_deref().unwrap_or("check passed"))
            }
        };
        report.push(format!("{}: {outcome}", archive.label()));
    }
    let base_text = match &base_value {
        Some(Ok(Some(value))) => value.clone(),
        Some(_) => "none".into(),
        None => "not needed".into(),
    };
    let heading = format!(
        "Archive version check. Local base: {base_text}, proposed: {proposed} ({}).",
        match rule {
            VersionRule::Base => "base_version_not_behind",
            VersionRule::Target => "target_version_not_behind",
        }
    );
    if failed {
        anyhow::bail!(
            "{heading}\n{}\nSet build.restrictions.rule: target to publish from an older checkout, or build.restrictions.not_behind_remote: false to skip the check",
            report.join("\n")
        );
    }
    eprintln!("{heading}");
    for line in report {
        eprintln!("{line}");
    }
    Ok(())
}
