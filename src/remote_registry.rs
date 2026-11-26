//! projr's remotes schema (SATVILab/projr@ab3ce8a `R/yml-check.R:288-380`): named
//! definitions under `remotes.<type>.<title>` are selected by `build.<type>.<title>`
//! (optionally narrowing `content`); an unregistered `build.<type>.<title>` is an
//! inline definition. Registered definitions stay available to restore whether or
//! not a build sends to them.
use crate::yml::{LocalRemoteConfig, ProjConfig, RemotesConfig};
use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

/// Named definitions retained even when no build sends to them; legacy CAS remains separately typed.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    /// Named local latest/archive definitions, using the existing inline definition schema.
    pub local: Map<String, Value>,
    /// Named GitHub Release definitions, using the existing inline definition schema.
    pub github: Map<String, Value>,
    /// Modern local CAS definitions; legacy accepted CAS forms remain separately typed.
    pub cas: Map<String, Value>,
    /// Experimental local Xet definitions, independent of current uploads.
    pub xet: Map<String, Value>,
}

/// Compatibility name for the initially ordinary-only registry type.
pub type OrdinaryRegistry = Registry;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryWire {
    local: Option<Map<String, Value>>,
    github: Option<Map<String, Value>>,
}
impl<'de> Deserialize<'de> for RemotesConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire = RegistryWire::deserialize(deserializer)?;
        let mut legacy = HashMap::new();
        let mut registry = Registry::default();
        for (name, value) in wire.local.unwrap_or_default() {
            if value.get("structure").and_then(Value::as_str) == Some("xet") {
                crate::xet_remote::definition(&value).map_err(serde::de::Error::custom)?;
                registry.xet.insert(name, value);
            } else if value.get("structure").and_then(Value::as_str) == Some("cas") {
                if crate::cas_remote::legacy_shape(&value) {
                    let remote: LocalRemoteConfig =
                        serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                    legacy.insert(name, remote);
                } else {
                    crate::cas_remote::Definition::parse(&value)
                        .map_err(serde::de::Error::custom)?;
                    registry.cas.insert(name, value);
                }
            } else {
                crate::restore::definition(&value).map_err(serde::de::Error::custom)?;
                crate::restore::local_only_fields(&value).map_err(serde::de::Error::custom)?;
                registry.local.insert(name, value);
            }
        }
        registry.github = wire.github.unwrap_or_default();
        for value in registry.github.values() {
            if value.get("structure").and_then(Value::as_str) == Some("xet") {
                crate::configured_xet_github::definition(value)
                    .map_err(serde::de::Error::custom)?;
            } else if value.get("structure").and_then(Value::as_str) == Some("cas") {
                crate::configured_cas_github::definition(value)
                    .map_err(serde::de::Error::custom)?;
            } else {
                crate::restore::definition(value).map_err(serde::de::Error::custom)?;
            }
        }
        Ok(RemotesConfig {
            local: if legacy.is_empty() {
                None
            } else {
                Some(legacy)
            },
            registry,
        })
    }
}

/// Selected definitions, without credentials, filesystem effects or transport activity.
pub(crate) struct Selected {
    pub(crate) local: Option<Map<String, Value>>,
    pub(crate) github: Option<Map<String, Value>>,
    pub(crate) legacy_cas: Vec<String>,
    pub(crate) legacy_content: HashMap<String, Vec<String>>,
    pub(crate) cas: Vec<crate::cas_remote::Selection>,
    pub(crate) xet: Vec<crate::xet_remote::Selection>,
    pub(crate) github_xet: Map<String, Value>,
    pub(crate) github_cas: Map<String, Value>,
}
fn name(value: &str) -> Result<String> {
    anyhow::ensure!(
        !value.trim().is_empty() && !value.chars().any(char::is_control),
        "Remote names must be nonempty printable text"
    );
    Ok(icu_normalizer::ComposingNormalizer::new_nfc()
        .normalize(&value.to_uppercase())
        .into_owned())
}
fn legacy_only(config: &ProjConfig) -> bool {
    config.remotes.registry.local.is_empty()
        && config.remotes.registry.github.is_empty()
        && config.remotes.registry.cas.is_empty()
        && config.remotes.registry.xet.is_empty()
}
/// Validate registry/legacy lookup names without resolving upload selection.
pub(crate) fn validate_names(config: &ProjConfig) -> Result<()> {
    if legacy_only(config) {
        return Ok(());
    }
    let mut names = HashSet::new();
    for title in config
        .remotes
        .registry
        .local
        .keys()
        .chain(config.remotes.registry.github.keys())
        .chain(config.remotes.registry.cas.keys())
        .chain(config.remotes.registry.xet.keys())
        .chain(config.remotes.local.iter().flat_map(|values| values.keys()))
    {
        anyhow::ensure!(
            names.insert(name(title)?),
            "Remote names are ambiguous across providers or portable spelling"
        );
    }
    Ok(())
}
/// A registered remote's build entry: bare/`{}` (all eligible labels) or `{content}`.
fn subset(provider: &str, title: &str, value: &Value) -> Result<Option<Vec<String>>> {
    if value.is_null() {
        return Ok(None);
    }
    let mapping = value.as_object().with_context(|| {
        format!("build.{provider}.{title} must be a mapping with optional content")
    })?;
    anyhow::ensure!(
        mapping.keys().all(|key| key == "content"),
        "build.{provider}.{title} only accepts content; put remote settings under remotes.{provider}.{title}"
    );
    mapping
        .get("content")
        .map(|value| {
            value
                .as_array()
                .with_context(|| {
                    format!("build.{provider}.{title}.content must list the labels to publish; omit it for all eligible labels")
                })?
                .iter()
                .map(|label| {
                    label
                        .as_str()
                        .map(str::to_owned)
                        .context("Destination content labels must be strings")
                })
                .collect()
        })
        .transpose()
}
fn validate_definition(value: &Value) -> Result<()> {
    if value.get("structure").and_then(Value::as_str) == Some("xet") {
        if value.get("path").is_some() {
            crate::xet_remote::definition(value)?;
        } else {
            crate::configured_xet_github::definition(value)?;
        }
    } else if value.get("structure").and_then(Value::as_str) == Some("cas") {
        if crate::cas_remote::legacy_shape(value) {
            let _: LocalRemoteConfig = serde_json::from_value(value.clone())?;
        } else if value.get("path").is_some() {
            crate::cas_remote::Definition::parse(value)?;
        } else {
            crate::configured_cas_github::definition(value)?;
        }
    } else {
        crate::restore::definition(value)?;
    }
    Ok(())
}
fn selected_definition(
    config: &ProjConfig,
    value: &Value,
    subset: Option<Vec<String>>,
) -> Result<Value> {
    validate_definition(value)?;
    let declared_content = value.get("content");
    let mut value = value.clone();
    let mapping = value
        .as_object_mut()
        .context("Remote definition must be a mapping")?;
    if let Some(labels) = subset {
        crate::path_safety::validate_directory_labels(labels.iter().map(String::as_str))?;
        let context = serde_json::json!({"directories":config.directories.keys().map(|label|(label.clone(),Value::Null)).collect::<Map<_,_>>()});
        for label in &labels {
            anyhow::ensure!(
                config.directories.contains_key(label),
                "Destination references an undeclared label: {label}"
            );
            if let Some(content) = declared_content {
                anyhow::ensure!(
                    crate::restore::content_has(Some(content), label, &context)?,
                    "Destination label '{label}' is excluded by the registered remote's content"
                );
            }
        }
        mapping.insert("content".into(), serde_json::to_value(labels)?);
    } else if declared_content.is_none() {
        mapping.insert("content".into(), Value::Bool(true));
    }
    Ok(value)
}
fn registered(config: &ProjConfig, provider: &str, title: &str) -> bool {
    let registry = &config.remotes.registry;
    match provider {
        "local" => {
            registry.local.contains_key(title)
                || registry.cas.contains_key(title)
                || registry.xet.contains_key(title)
                || config
                    .remotes
                    .local
                    .as_ref()
                    .is_some_and(|values| values.contains_key(title))
        }
        _ => registry.github.contains_key(title),
    }
}
/// The definition for `build.<provider>.<title>` or a registered title: the registered
/// definition wins; otherwise the inline build entry. `None` when the title is unknown.
pub(crate) fn definition_of<'a>(
    config: &'a ProjConfig,
    provider: &str,
    title: &str,
) -> Option<&'a Value> {
    let registry = &config.remotes.registry;
    let found = match provider {
        "local" => registry
            .local
            .get(title)
            .or_else(|| registry.cas.get(title))
            .or_else(|| registry.xet.get(title)),
        _ => registry.github.get(title),
    };
    found.or_else(|| {
        if registered(config, provider, title) {
            return None;
        }
        match provider {
            "local" => config.build.local.as_ref(),
            _ => config.build.github.as_ref(),
        }
        .and_then(|values| values.get(title))
    })
}
/// Titles known for a provider: registered names first, then inline build titles.
pub(crate) fn titles(config: &ProjConfig, provider: &str) -> Vec<String> {
    let registry = &config.remotes.registry;
    let mut titles: Vec<String> = match provider {
        "local" => registry
            .local
            .keys()
            .chain(registry.cas.keys())
            .chain(registry.xet.keys())
            .chain(config.remotes.local.iter().flat_map(|values| values.keys()))
            .chain(config.build.local.iter().flat_map(|values| values.keys()))
            .cloned()
            .collect(),
        _ => registry
            .github
            .keys()
            .chain(config.build.github.iter().flat_map(|values| values.keys()))
            .cloned()
            .collect(),
    };
    let mut seen = HashSet::new();
    titles.retain(|title| seen.insert(title.clone()));
    titles
}
/// Validate the build entries that select registered remotes (only `content`).
pub(crate) fn validate_selections(config: &ProjConfig) -> Result<()> {
    for (provider, entries) in [
        ("local", config.build.local.as_ref()),
        ("github", config.build.github.as_ref()),
    ] {
        for (title, value) in entries.into_iter().flatten() {
            if registered(config, provider, title) {
                subset(provider, title, value)?;
            }
        }
    }
    Ok(())
}
/// Freeze selected registered destinations and inline definitions, without
/// credentials, filesystem effects or transport activity. A registered remote is
/// sent to only when its title appears under `build.<type>`.
pub(crate) fn selected(config: &ProjConfig) -> Result<Selected> {
    validate_names(config)?;
    validate_sources(config)?;
    let mut local = Map::new();
    let mut github = Map::new();
    let mut legacy_cas = Vec::new();
    let mut legacy_content = HashMap::new();
    let mut cas = Vec::new();
    let mut xet = Vec::new();
    let mut github_xet = Map::new();
    let mut github_cas = Map::new();
    let mut selected_names = HashSet::new();
    let mut choices: Vec<(&str, String, Option<Vec<String>>)> = Vec::new();
    for (provider, entries) in [
        ("local", config.build.local.as_ref()),
        ("github", config.build.github.as_ref()),
    ] {
        for (title, value) in entries.into_iter().flatten() {
            if registered(config, provider, title) {
                choices.push((provider, title.clone(), subset(provider, title, value)?));
            } else if provider == "local" {
                // projr: an empty inline local entry names a remote that was never defined.
                anyhow::ensure!(
                    value.as_object().is_some_and(|value| !value.is_empty()),
                    "Define remotes.local.{title}.path before selecting build.local.{title}"
                );
                local.insert(title.clone(), value.clone());
            } else {
                github.insert(title.clone(), value.clone());
            }
        }
    }
    for (provider, title, subset) in choices {
        anyhow::ensure!(
            selected_names.insert(if legacy_only(config) {
                title.clone()
            } else {
                name(&title)?
            }),
            "Duplicate/aliased build destination name"
        );
        if provider == "github" {
            let value = &config.remotes.registry.github[&title];
            let mut selected = selected_definition(config, value, subset)?;
            if value.get("structure").and_then(Value::as_str) == Some("xet") {
                github_xet.insert(title, selected);
            } else if value.get("structure").and_then(Value::as_str) == Some("cas") {
                github_cas.insert(title, selected);
            } else {
                selected
                    .as_object_mut()
                    .context("Remote definition must be a mapping")?
                    .entry("manifest-style")
                    .or_insert(Value::String("flat".into()));
                github.insert(title, selected);
            }
        } else if let Some(value) = config.remotes.registry.local.get(&title) {
            let mut selected = selected_definition(config, value, subset)?;
            selected
                .as_object_mut()
                .context("Remote definition must be a mapping")?
                .entry("manifest-style")
                .or_insert(Value::String("flat".into()));
            local.insert(title, selected);
        } else if let Some(value) = config.remotes.registry.xet.get(&title) {
            let value = selected_definition(config, value, subset)?;
            xet.push(crate::xet_remote::Selection {
                title,
                definition: crate::xet_remote::definition(&value)?,
            });
        } else if let Some(value) = config.remotes.registry.cas.get(&title) {
            let value = selected_definition(config, value, subset)?;
            cas.push(crate::cas_remote::Selection {
                title,
                definition: crate::cas_remote::Definition::parse(&value)?,
            });
        } else {
            let remote = config
                .remotes
                .local
                .as_ref()
                .and_then(|values| values.get(&title))
                .context("Selected legacy CAS remote disappeared")?;
            let content = if let Some(labels) = subset {
                crate::path_safety::validate_directory_labels(labels.iter().map(String::as_str))?;
                let mut content = Vec::new();
                for label in labels {
                    anyhow::ensure!(
                        config
                            .directories
                            .keys()
                            .any(|name| name.eq_ignore_ascii_case(&label)),
                        "Destination references an undeclared label: {label}"
                    );
                    let original = remote
                        .content
                        .iter()
                        .find(|value| *value == &label)
                        .or_else(|| {
                            remote
                                .content
                                .iter()
                                .find(|value| value.eq_ignore_ascii_case(&label))
                        })
                        .context("Destination label is excluded by the legacy CAS definition")?;
                    content.push(original.clone());
                }
                content
            } else {
                remote.content.clone()
            };
            if !content.is_empty() {
                legacy_cas.push(title.clone());
            }
            legacy_content.insert(title, content);
        }
    }
    Ok(Selected {
        local: Some(local),
        github: Some(github),
        legacy_cas,
        legacy_content,
        cas,
        xet,
        github_xet,
        github_cas,
    })
}

fn shared(config: &ProjConfig, provider: &str, title: &str) -> Result<Option<Value>> {
    name(title)?;
    let registered = match provider {
        "local" => config
            .remotes
            .registry
            .local
            .get(title)
            .or_else(|| config.remotes.registry.cas.get(title))
            .or_else(|| config.remotes.registry.xet.get(title))
            .cloned()
            .or_else(|| {
                config
                    .remotes
                    .local
                    .as_ref()
                    .and_then(|values| values.get(title))
                    .map(crate::cas_remote::legacy_value)
            }),
        "github" => config.remotes.registry.github.get(title).cloned(),
        _ => anyhow::bail!("Unsupported directory source provider"),
    };
    if registered.is_some() {
        // A build entry for a registered title is an upload selection, not a definition.
        return Ok(registered);
    }
    Ok(match provider {
        "local" => config.build.local.as_ref(),
        "github" => config.build.github.as_ref(),
        _ => None,
    }
    .and_then(|values| values.get(title))
    .cloned())
}

fn source_reference(
    config: &ProjConfig,
    provider: &str,
    title: &str,
    inline: Option<&Value>,
) -> Result<()> {
    let value = match (shared(config, provider, title)?, inline) {
        (Some(value), Some(inline)) => crate::yml::deep_merge(value, inline.clone()),
        (Some(value), None) => value,
        (None, Some(inline)) => inline.clone(),
        (None, None) => anyhow::bail!("Directory source references an undefined remote: {title}"),
    };
    validate_definition(&value)?;
    Ok(())
}
/// Validate source names and shape without restoring files or loading credentials.
fn validate_sources(config: &ProjConfig) -> Result<()> {
    for directory in config.directories.values() {
        let Some(sources) = directory.source.as_ref().filter(|value| !value.is_null()) else {
            continue;
        };
        for (provider, entries) in sources
            .as_object()
            .context("Directory source must be a provider mapping")?
        {
            anyhow::ensure!(
                matches!(provider.as_str(), "local" | "github"),
                "Unsupported directory source provider"
            );
            match entries {
                Value::Null => {}
                Value::String(title) => source_reference(config, provider, title, None)?,
                Value::Array(titles) => {
                    for title in titles {
                        source_reference(
                            config,
                            provider,
                            title
                                .as_str()
                                .context("Source references must be strings")?,
                            None,
                        )?;
                    }
                }
                Value::Object(titles) => {
                    for (title, inline) in titles {
                        source_reference(config, provider, title, Some(inline))?;
                    }
                }
                _ => anyhow::bail!(
                    "Directory source must contain a title, title list or inline definitions"
                ),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn example() -> ProjConfig {
        serde_json::from_value(serde_json::json!({
            "directories":{"raw":{"path":"input"},"output":{"path":"results"},"cache":{"path":"cache"}},
            "remotes":{"local":{"A":{"path":"../A","content":["raw"]},"B":{"path":"../B"}}},
            "build":{"scripts":[],"git":false,"local":{"B":{"content":["output"]}}}
        })).unwrap()
    }
    fn select(config: &mut ProjConfig, local: Value) {
        config.build.local = Some(serde_json::from_value(local).unwrap());
    }
    #[test]
    fn registered_sources_are_preserved_when_only_another_remote_is_selected() {
        let config = example();
        let targets = selected(&config).unwrap();
        assert_eq!(
            config
                .remotes
                .registry
                .local
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["A", "B"]
        );
        let local = targets.local.unwrap();
        assert_eq!(local.keys().map(String::as_str).collect::<Vec<_>>(), ["B"]);
        assert_eq!(local["B"]["content"], serde_json::json!(["output"]));
        assert!(targets.legacy_cas.is_empty());
    }
    #[test]
    fn bare_empty_and_omitted_selections_follow_projr() {
        let mut config = example();
        for bare in [serde_json::json!({"B":{}}), serde_json::json!({"B":null})] {
            select(&mut config, bare);
            assert_eq!(
                selected(&config).unwrap().local.unwrap()["B"]["content"],
                true
            );
        }
        select(&mut config, serde_json::json!({"A":{}}));
        assert_eq!(
            selected(&config).unwrap().local.unwrap()["A"]["content"],
            serde_json::json!(["raw"])
        );
        // Omitting a registered title sends nothing to it.
        select(&mut config, serde_json::json!({}));
        assert!(selected(&config).unwrap().local.unwrap().is_empty());
        config.build.local = None;
        assert!(selected(&config).unwrap().local.unwrap().is_empty());
        select(&mut config, serde_json::json!({"B":{"content":[]}}));
        assert_eq!(
            selected(&config).unwrap().local.unwrap()["B"]["content"],
            serde_json::json!([])
        );
    }
    #[test]
    fn explicit_remote_null_remains_disabled_and_subsets_cannot_override_it() {
        let mut config = example();
        config.remotes.registry.local.get_mut("B").unwrap()["content"] = Value::Null;
        select(&mut config, serde_json::json!({"B":{}}));
        assert!(selected(&config).unwrap().local.unwrap()["B"]["content"].is_null());
        select(&mut config, serde_json::json!({"B":{"content":["output"]}}));
        assert!(selected(&config).is_err());
    }
    #[test]
    fn invalid_or_excluded_selections_fail_purely() {
        for local in [
            serde_json::json!({"B":{"content":["undeclared"]}}),
            serde_json::json!({"A":{"content":["output"]}}),
            serde_json::json!({"B":{"content":null}}),
            serde_json::json!({"B":{"path":"../elsewhere"}}),
            serde_json::json!({"B":{"content":["raw","raw"]}}),
            serde_json::json!({"B":"raw"}),
        ] {
            let mut config = example();
            select(&mut config, local.clone());
            assert!(selected(&config).is_err(), "{local}");
        }
        let mut config = example();
        config
            .remotes
            .registry
            .github
            .insert("a".into(), serde_json::json!({}));
        assert!(selected(&config).is_err());
    }
    #[test]
    fn registered_selection_and_inline_definitions_combine() {
        let mut config = example();
        select(
            &mut config,
            serde_json::json!({"B":{},"old":{"path":"../old","content":["raw"]}}),
        );
        let local = selected(&config).unwrap().local.unwrap();
        assert_eq!(local["B"]["manifest-style"], "flat");
        assert_eq!(
            local["old"],
            serde_json::json!({"path":"../old","content":["raw"]})
        );
        // A GitHub selection never matches a local registration, and vice versa.
        config.build.github = Some(serde_json::from_value(serde_json::json!({"B":{}})).unwrap());
        let github = selected(&config).unwrap().github.unwrap();
        assert_eq!(github["B"], serde_json::json!({}));
        let legacy: ProjConfig = serde_json::from_value(serde_json::json!({
            "directories":{"raw":{"path":"input"}},
            "remotes":{"local":{
                "a":{"path":"../a","structure":"cas","content":["raw"],"inspect":"manifest"},
                "A":{"path":"../A","structure":"cas","content":["raw"],"inspect":"file"}}},
            "build":{"local":{"a":{},"A":{},"old":{"path":"../old","content":["raw"]}}}
        }))
        .unwrap();
        let targets = selected(&legacy).unwrap();
        assert_eq!(targets.legacy_cas, ["a", "A"]);
        assert!(targets.local.unwrap().contains_key("old"));
    }
    #[test]
    fn registered_definitions_win_lookup_over_their_build_selection() {
        let mut config = example();
        select(&mut config, serde_json::json!({"B":{"content":["output"]}}));
        assert_eq!(
            definition_of(&config, "local", "B"),
            Some(&serde_json::json!({"path":"../B"}))
        );
        assert!(definition_of(&config, "github", "B").is_none());
        assert_eq!(titles(&config, "local"), ["A", "B"]);
    }
    #[test]
    fn directory_source_references_validate_without_auto_restore_or_upload_selection() {
        let mut config = example();
        select(&mut config, serde_json::json!({"B":{}}));
        config.directories.get_mut("raw").unwrap().source = Some(serde_json::json!({"local":"A"}));
        assert!(selected(&config).is_ok());
        config.directories.get_mut("raw").unwrap().source = Some(serde_json::json!({"local":"B"}));
        assert!(selected(&config).is_ok());
        config.directories.get_mut("raw").unwrap().source =
            Some(serde_json::json!({"local":"missing"}));
        assert!(selected(&config).is_err());
        config.directories.get_mut("raw").unwrap().source =
            Some(serde_json::json!({"local":{"custom":{"path":"../custom"}}}));
        assert!(selected(&config).is_ok());
        config.directories.get_mut("raw").unwrap().source = Some(serde_json::json!({"s3":"A"}));
        assert!(selected(&config).is_err());
    }
    #[test]
    fn unsupported_new_policy_values_fail_instead_of_activating_silent_defaults() {
        for remote in [
            serde_json::json!({"path":"../A","manifest":"unsupported"}),
            serde_json::json!({"path":"../A","manifest-style":"unknown"}),
            serde_json::json!({"path":"../A","manifest-format":null}),
            serde_json::json!({"path":"../A","repo":"owner/repo"}),
            serde_json::json!({"path":"../A","branch":"main"}),
        ] {
            assert!(
                serde_json::from_value::<ProjConfig>(
                    serde_json::json!({"remotes":{"local":{"A":remote}}})
                )
                .is_err()
            );
        }
        for remote in [
            serde_json::json!({"repo":"owner/repo/extra"}),
            serde_json::json!({"repo":"../repo"}),
            serde_json::json!({"tag":""}),
            serde_json::json!({"branch":3}),
        ] {
            assert!(
                serde_json::from_value::<ProjConfig>(
                    serde_json::json!({"remotes":{"github":{"A":remote}}})
                )
                .is_err()
            );
        }
        assert!(
            serde_json::from_value::<ProjConfig>(serde_json::json!({"remotes":{"github":{
                "A":{"repo":"team/results","tag":"figures-@branch","branch":"@branch"}}}}))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<ProjConfig>(serde_json::json!({"remotes":{"s3":{}}})).is_err()
        );
        assert!(
            serde_json::from_value::<ProjConfig>(serde_json::json!({"build":{"dest":["A"]}}))
                .is_err()
        );
    }
}

#[cfg(test)]
mod cas_selection_tests {
    use super::*;
    #[test]
    fn modern_defaults_and_legacy_mapping_subsets_remain_distinct() {
        let modern: ProjConfig = serde_json::from_value(serde_json::json!({
            "directories":{"raw":{"path":"input"},"output":{"path":"output"}},
            "remotes":{"local":{"new":{"path":"../new","structure":"cas"}}},
            "build":{"local":{"new":{"content":["raw"]}}}
        }))
        .unwrap();
        let selected = selected(&modern).unwrap();
        assert!(selected.legacy_cas.is_empty());
        assert_eq!(selected.cas.len(), 1);
        assert_eq!(
            selected.cas[0].definition.content,
            serde_json::json!(["raw"])
        );
        assert_eq!(
            selected.cas[0].definition.manifest,
            crate::cas_remote::Placement::Auto
        );
        assert_eq!(
            selected.cas[0].definition.manifest_style,
            crate::cas_remote::Style::Tree
        );

        let legacy:ProjConfig=serde_json::from_value(serde_json::json!({
            "directories":{"raw":{"path":"input"},"output":{"path":"output"}},
            "remotes":{"local":{"old":{"path":"../old","structure":"cas","content":["RAW","output"],"inspect":"file"}}},
            "build":{"local":{"old":{"content":["raw"]}}}
        })).unwrap();
        let selected = super::selected(&legacy).unwrap();
        assert!(selected.cas.is_empty());
        assert_eq!(selected.legacy_cas, ["old"]);
        assert_eq!(selected.legacy_content["old"], ["RAW"]);
    }
    #[test]
    fn empty_cas_mapping_does_not_turn_into_a_configured_publication() {
        let config:ProjConfig=serde_json::from_value(serde_json::json!({
            "directories":{"raw":{"path":"input"}},
            "remotes":{"local":{"old":{"path":"../old","structure":"cas","content":["raw"],"inspect":"manifest"}}},
            "build":{"local":{"old":{"content":[]}}}
        })).unwrap();
        assert!(selected(&config).unwrap().legacy_cas.is_empty());
    }
}
