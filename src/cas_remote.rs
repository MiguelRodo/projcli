//! Configured local recursive CAS publication; ordinary provider policies remain separate.
use crate::catalogue::Format;
use crate::path_safety::portable_paths_overlap as overlaps;
use crate::snapshot_format::{DirectoryRoot, FlatSnapshot, TreeSnapshot};
use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

/// Full metadata placement, independent of file-object storage and manifest style.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Placement {
    #[default]
    Auto,
    Project,
    Remote,
    Both,
}
/// New CAS defaults to recursive metadata; legacy flat definitions are recognized separately.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Style {
    Flat,
    #[default]
    Tree,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Structure {
    Cas,
}
fn all() -> Value {
    Value::Bool(true)
}
/// Modern CAS write policy; full immutable tree integrity is always verified.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct Definition {
    pub(crate) path: Utf8PathBuf,
    #[serde(rename = "structure")]
    _structure: Structure,
    #[serde(default = "all")]
    pub(crate) content: Value,
    #[serde(default)]
    pub(crate) manifest: Placement,
    #[serde(default)]
    pub(crate) manifest_style: Style,
    #[serde(default)]
    pub(crate) manifest_format: Option<Format>,
    pub(crate) source: Option<Value>,
}
impl Definition {
    /// Validate metadata options and label-list shapes without filesystem or transport effects.
    pub(crate) fn parse(value: &Value) -> Result<Self> {
        anyhow::ensure!(
            !value.get("manifest-format").is_some_and(Value::is_null),
            "manifest-format must be an explicit supported mode or omitted"
        );
        let result: Self =
            serde_json::from_value(value.clone()).context("Invalid configured CAS definition")?;
        anyhow::ensure!(
            result.manifest_style == Style::Flat
                || result
                    .manifest_format
                    .is_none_or(|format| format == Format::Json),
            "Tree manifests use fixed v1 JSON; flat CSV/JSON/dictionary modes require manifest-style: flat"
        );
        anyhow::ensure!(
            !result.path.as_str().trim().is_empty(),
            "Configured CAS requires an explicit nonempty path"
        );
        for (kind, value) in [
            ("content", Some(&result.content)),
            ("source", result.source.as_ref()),
        ] {
            match value {
                None | Some(Value::Null) | Some(Value::Bool(_)) => {}
                Some(Value::Array(labels)) => {
                    let labels = labels
                        .iter()
                        .map(|label| label.as_str().context("CAS label lists require strings"))
                        .collect::<Result<Vec<_>>>()?;
                    crate::path_safety::validate_directory_labels(labels.iter().copied())?;
                }
                _ => anyhow::bail!("CAS {kind} must be boolean, null or a label list"),
            }
        }
        Ok(result)
    }
}
/// Existing accepted path/structure/content/inspect definitions retain their legacy encoding.
pub(crate) fn legacy_shape(value: &Value) -> bool {
    value.as_object().is_some_and(|map| {
        map.contains_key("content")
            && map.contains_key("inspect")
            && map
                .keys()
                .all(|key| matches!(key.as_str(), "path" | "structure" | "content" | "inspect"))
    })
}
/// Reconstruct the old accepted wire definition for compatible per-build subset selection.
pub(crate) fn legacy_value(value: &crate::yml::LocalRemoteConfig) -> Value {
    serde_json::json!({"path":value.path,"structure":"cas","content":value.content,
        "inspect":match &value.inspect {crate::yml::InspectStrategy::File=>"file",crate::yml::InspectStrategy::Manifest=>"manifest"}})
}
/// Named selected definition; no credentials or filesystem writes are captured.
pub(crate) struct Selection {
    pub(crate) title: String,
    pub(crate) definition: Definition,
}
/// Frozen physical target and selected resources, captured before hooks.
pub(crate) struct Target {
    pub(crate) base: Utf8PathBuf,
    project_metadata: Utf8PathBuf,
    metadata: Vec<Utf8PathBuf>,
    history: Vec<Utf8PathBuf>,
    labels: Vec<(String, Utf8PathBuf)>,
    style: Style,
    format: Format,
}
fn physical(path: &Utf8Path) -> Result<Utf8PathBuf> {
    Ok(Utf8PathBuf::try_from(crate::path_safety::resolve_path(
        path.as_std_path(),
        true,
    )?)?)
}
/// Stable opaque namespace: no physical machine/user path is embedded in Git references.
pub(crate) fn project_metadata(root: &Utf8Path, title: &str) -> Utf8PathBuf {
    let key = crate::path_safety::portable_path_key(Utf8Path::new(title));
    let mut hash = blake3::Hasher::new_derive_key("projcli.remote-name.v1");
    hash.update(key.as_str().as_bytes());
    let id = hash.finalize().to_hex().to_string();
    root.join(".proj/remotes").join(&id[..2]).join(&id[2..])
}
pub(crate) fn selected_labels(
    config: &crate::yml::ValidatedConfig,
    content: &Value,
) -> Result<Vec<String>> {
    let mut labels = match content {
        Value::Null | Value::Bool(false) => vec![],
        Value::Bool(true) => {
            let mut labels: Vec<_> = config
                .directories
                .keys()
                .filter(|label| !label.to_ascii_lowercase().starts_with("cache"))
                .cloned()
                .collect();
            labels.sort();
            labels
        }
        Value::Array(values) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .context("CAS content labels must be strings")
            })
            .collect::<Result<Vec<_>>>()?,
        _ => anyhow::bail!("CAS content must be boolean, null or a label list"),
    };
    crate::path_safety::validate_directory_labels(labels.iter().map(String::as_str))?;
    for label in &labels {
        anyhow::ensure!(
            config.directories.contains_key(label),
            "CAS label is undeclared: {label}"
        );
    }
    // Preserve explicit list order; the snapshot codec independently sorts identity metadata.
    labels.shrink_to_fit();
    Ok(labels)
}
/// Validate every selected namespace before hooks; no directories or blobs are created.
pub(crate) fn configured(
    root: &Utf8Path,
    selections: &[Selection],
    config: &crate::yml::ValidatedConfig,
    ordinary: &[crate::publication::LocalPublication],
) -> Result<Vec<Target>> {
    if selections.is_empty() {
        return Ok(Vec::new());
    }
    let root = physical(root)?;
    let mut targets: Vec<Target> = Vec::new();
    let mut namespaces = HashSet::new();
    for selection in selections {
        let labels = selected_labels(config, &selection.definition.content)?;
        if labels.is_empty() {
            continue;
        }
        TreeSnapshot::new(
            "preflight".into(),
            None,
            labels
                .iter()
                .map(|label| DirectoryRoot {
                    label: label.clone(),
                    tree: "0".repeat(64),
                })
                .collect(),
        )?;
        let input = &selection.definition.path;
        let base = physical(&if input.is_absolute() {
            input.clone()
        } else {
            root.join(input)
        })?;
        anyhow::ensure!(
            !root.starts_with(&base),
            "Configured CAS store contains the project root"
        );
        let project_metadata = project_metadata(&root, &selection.title);
        anyhow::ensure!(
            namespaces.insert(project_metadata.clone()),
            "CAS project metadata namespaces collide"
        );
        for resource in config.directories.keys() {
            anyhow::ensure!(
                !overlaps(&base, &physical(&config.get_path(&root, resource)?)?),
                "Configured CAS store overlaps a declared resource"
            );
        }
        for target in ordinary {
            anyhow::ensure!(
                !overlaps(&base, &target.base),
                "Configured CAS store overlaps ordinary publication"
            );
        }
        if let Some(legacy) = &config.remotes.local {
            for name in &config.dest {
                anyhow::ensure!(
                    !overlaps(
                        &base,
                        &physical(
                            &legacy
                                .get(name)
                                .context("Legacy CAS target disappeared")?
                                .path
                        )?
                    ),
                    "Configured CAS stores overlap"
                );
            }
        }
        for target in &targets {
            anyhow::ensure!(
                !overlaps(&base, &target.base),
                "Configured CAS stores overlap"
            );
        }
        let labels = labels
            .into_iter()
            .map(|label| Ok((label.clone(), physical(&config.get_path(&root, &label)?)?)))
            .collect::<Result<Vec<_>>>()?;
        let sources = labels
            .iter()
            .map(|(_, path)| path.clone())
            .collect::<Vec<_>>();
        let metadata = match selection.definition.manifest {
            Placement::Auto | Placement::Remote => vec![base.clone()],
            Placement::Project => vec![project_metadata.clone()],
            Placement::Both => vec![base.clone(), project_metadata.clone()],
        };
        let history = vec![base.clone(), project_metadata.clone()];
        let locations = if selection.definition.manifest_style == Style::Flat {
            &history
        } else {
            &metadata
        };
        for location in locations {
            crate::cas_tree::preflight_storage(&root, &base, location, &sources)?;
            if selection.definition.manifest_style == Style::Flat {
                crate::cas_flat::preflight(&root, &base, location)?;
            }
        }
        if selection.definition.manifest_style == Style::Flat {
            crate::cas_flat::load_history(&history)?;
            crate::cas_flat::preflight_project_reference(&root, &project_metadata)?;
        } else {
            crate::cas_tree::preflight_project_reference(&root, &project_metadata)?;
        }
        targets.push(Target {
            base,
            project_metadata,
            metadata,
            history,
            labels,
            style: selection.definition.manifest_style,
            format: selection.definition.manifest_format.unwrap_or(
                if selection.definition.manifest_style == Style::Flat {
                    Format::Csv
                } else {
                    Format::Json
                },
            ),
        });
    }
    Ok(targets)
}
/// Source validation remains before actual provider entry and never creates store paths.
pub(crate) fn prepare(root: &Utf8Path, target: &Target, version: &str) -> Result<()> {
    TreeSnapshot::new(
        version.into(),
        None,
        target
            .labels
            .iter()
            .map(|(label, _)| DirectoryRoot {
                label: label.clone(),
                tree: "0".repeat(64),
            })
            .collect(),
    )?;
    crate::cas_tree::validate_sources(
        root,
        &target.base,
        &target.metadata[0],
        &target
            .labels
            .iter()
            .map(|(_, path)| path.clone())
            .collect::<Vec<_>>(),
    )?;
    if target.style == Style::Flat {
        let directories = target
            .labels
            .iter()
            .map(|(label, path)| {
                crate::cas_tree::observe_flat_directory(root, &target.base, path, label)
            })
            .collect::<Result<_>>()?;
        let snapshot = FlatSnapshot::new(version.into(), None, directories)?;
        crate::cas_flat::prepare(
            root,
            &target.base,
            &target.metadata,
            &target.history,
            &snapshot,
            target.format,
        )?;
    }
    Ok(())
}
/// Static evidence provider name; never copied remote titles, physical paths or credentials.
pub(crate) fn provider(target: &Target) -> &'static str {
    if target.style == Style::Flat {
        "cas_flat"
    } else {
        "cas_tree"
    }
}
/// Add immutable file/tree objects, then required full references, then the lightweight project ref.
/// Snapshot metadata has no inferred clock; identical supplied version/roots are retry-idempotent.
/// Copy/reference failure may leave partial effects; no cross-location rollback or GC is attempted.
pub(crate) fn publish(root: &Utf8Path, target: &Target, version: &str) -> Result<String> {
    if target.style == Style::Flat {
        let directories = target
            .labels
            .iter()
            .map(|(label, path)| {
                crate::cas_tree::ingest_flat_directory(root, &target.base, path, label)
            })
            .collect::<Result<_>>()?;
        let snapshot = FlatSnapshot::new(version.into(), None, directories)?;
        return crate::cas_flat::publish_with_history(
            root,
            &target.base,
            &target.metadata,
            &target.history,
            Some(&target.project_metadata),
            &snapshot,
            target.format,
        );
    }
    let primary = &target.metadata[0];
    let mut roots = Vec::new();
    for (label, path) in &target.labels {
        roots.push(DirectoryRoot {
            label: label.clone(),
            tree: crate::cas_tree::ingest_directory(root, &target.base, primary, path)?,
        });
    }
    let snapshot = TreeSnapshot::new(version.into(), None, roots)?;
    let id = snapshot.identity()?;
    let result = (|| -> Result<()> {
        for location in target.metadata.iter().skip(1) {
            crate::cas_tree::copy_graph(root, &target.base, primary, location, &snapshot)?;
        }
        for location in &target.metadata {
            crate::cas_tree::publish_snapshot(root, &target.base, location, &snapshot)?;
        }
        crate::cas_tree::write_project_reference(root, &target.project_metadata, &snapshot)?;
        Ok(())
    })();
    result.with_context(|| {
        format!("CAS snapshot {id} may be partially published; inspect before retry")
    })?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modern_policy_defaults_and_explicit_null_rules_are_unambiguous() {
        let raw = serde_json::json!({"path":"../store","structure":"cas"});
        let definition = Definition::parse(&raw).unwrap();
        assert_eq!(definition.manifest, Placement::Auto);
        assert_eq!(definition.manifest_style, Style::Tree);
        assert_eq!(definition.content, Value::Bool(true));
        assert!(!legacy_shape(&raw));
        let mut disabled = raw.clone();
        disabled["content"] = Value::Null;
        assert!(Definition::parse(&disabled).unwrap().content.is_null());
        for (key, value) in [
            ("manifest", Value::Null),
            ("manifest-style", Value::Null),
            ("manifest-format", serde_json::json!("csv")),
            ("inspect", serde_json::json!("file")),
            ("send", serde_json::json!({"strategy":"sync-purge"})),
        ] {
            let mut invalid = raw.clone();
            invalid[key] = value;
            assert!(Definition::parse(&invalid).is_err(), "{key}");
        }
        let old = serde_json::json!({"path":"../store","structure":"cas","content":["raw"],"inspect":"file"});
        assert!(legacy_shape(&old));
    }
    #[test]
    fn content_and_source_shapes_do_not_load_credentials_or_interpret_unknown_options() {
        for key in ["content", "source"] {
            let mut valid = serde_json::json!({"path":"../store","structure":"cas"});
            valid[key] = serde_json::json!(["raw", "output"]);
            assert!(Definition::parse(&valid).is_ok());
            for value in [
                serde_json::json!(["raw", "RAW"]),
                serde_json::json!({"raw":true}),
                serde_json::json!([1]),
            ] {
                let mut raw = serde_json::json!({"path":"../store","structure":"cas"});
                raw[key] = value;
                assert!(Definition::parse(&raw).is_err());
            }
        }
        assert!(Definition::parse(&serde_json::json!({"path":" ","structure":"cas"})).is_err());
    }
    #[test]
    fn flat_modes_are_explicit_and_tree_only_accepts_its_json_representation() {
        for mode in ["csv", "json", "csv-dedupe-file", "json-dedupe-file"] {
            assert!(
                Definition::parse(&serde_json::json!({"path":"../store","structure":"cas",
                "manifest-style":"flat","manifest-format":mode}))
                .is_ok()
            );
        }
        assert!(
            Definition::parse(
                &serde_json::json!({"path":"../store","structure":"cas","manifest-format":"json"})
            )
            .is_ok()
        );
        assert!(Definition::parse(&serde_json::json!({"path":"../store","structure":"cas","manifest-style":"flat","manifest-format":null})).is_err());
    }
}
