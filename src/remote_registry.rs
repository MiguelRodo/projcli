//! projr 0.9 remotes schema (SATVILab/projr@5f915ef `R/yml-dest.R:569-761`,
//! `R/yml-check.R:302-386`): archives are defined only under
//! `remotes.<type>.<title>` and a production build publishes to those listed under
//! `build.dest`. A title may repeat across types; `<type>:<title>` disambiguates.
//! Registered definitions stay available to restore whether or not a build sends
//! to them.
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
/// Option words share the mapping form of `build.dest` with remote names
/// (SATVILab/projr@5f915ef `R/yml-dest.R:573-577`).
const RESERVED: [&str; 6] = [
    "content", "cue", "inspect", "strategy", "conflict", "version",
];
/// Settings `build.dest` accepts globally or per remote.
const DEST_OPTIONS: [&str; 4] = ["content", "cue", "inspect", "strategy"];
fn local_titles(config: &ProjConfig) -> impl Iterator<Item = &String> {
    let registry = &config.remotes.registry;
    registry
        .local
        .keys()
        .chain(registry.cas.keys())
        .chain(registry.xet.keys())
        .chain(config.remotes.local.iter().flat_map(|values| values.keys()))
}
/// Validate registry names: printable, not an option word, unique within a type by
/// portable spelling. A title may repeat across types (projr 0.9), except for
/// CAS/Xet stores, whose project metadata is keyed by title alone.
pub(crate) fn validate_names(config: &ProjConfig) -> Result<()> {
    let registry = &config.remotes.registry;
    for (provider, titles) in [
        ("local", local_titles(config).collect::<Vec<_>>()),
        ("github", registry.github.keys().collect()),
    ] {
        let mut names = HashSet::new();
        for title in titles {
            anyhow::ensure!(
                !RESERVED.contains(&title.as_str()),
                "remotes.{provider}.{title}: {title} is a reserved option name. Rename the remote"
            );
            anyhow::ensure!(
                names.insert(name(title)?),
                "Remote names under remotes.{provider} are ambiguous by portable spelling"
            );
        }
    }
    let github: HashSet<String> = registry
        .github
        .keys()
        .map(|t| name(t))
        .collect::<Result<_>>()?;
    let local: HashSet<String> = local_titles(config)
        .map(|t| name(t))
        .collect::<Result<_>>()?;
    let local_stores = registry
        .cas
        .keys()
        .chain(registry.xet.keys())
        .chain(config.remotes.local.iter().flat_map(|values| values.keys()))
        .map(|title| (title, &github));
    let github_stores = registry
        .github
        .iter()
        .filter(|(_, value)| {
            matches!(
                value.get("structure").and_then(Value::as_str),
                Some("cas" | "xet")
            )
        })
        .map(|(title, _)| (title, &local));
    for (title, other) in local_stores.chain(github_stores) {
        anyhow::ensure!(
            !other.contains(&name(title)?),
            "CAS/Xet store '{title}' cannot share its name with a remote of the other type"
        );
    }
    Ok(())
}
/// Resolve `title` or `<type>:<title>` to a registered remote (projr
/// `.yml_remote_ref_resolve`, SATVILab/projr@5f915ef `R/yml-dest.R:661-703`).
pub(crate) fn resolve(
    config: &ProjConfig,
    reference: &str,
    key: &str,
) -> Result<(&'static str, String)> {
    let declared = |provider: &str, title: &str| match provider {
        "local" => local_titles(config).any(|known| known == title),
        _ => config.remotes.registry.github.contains_key(title),
    };
    let (candidates, title): (&[&'static str], &str) = match reference.split_once(':') {
        Some(("local", title)) if !title.is_empty() => (&["local"], title),
        Some(("github", title)) if !title.is_empty() => (&["github"], title),
        _ => (&["local", "github"], reference),
    };
    let types: Vec<&'static str> = candidates
        .iter()
        .copied()
        .filter(|provider| declared(provider, title))
        .collect();
    match types.as_slice() {
        [provider] => Ok((*provider, title.to_owned())),
        [] => anyhow::bail!("{key} names {reference}, which is not declared under remotes"),
        _ => anyhow::bail!(
            "{key} names {reference}, which is declared under both remotes.local and remotes.github. Use local:{reference} or github:{reference}"
        ),
    }
}
/// One `build.dest` selection: optional `content` and `send` overrides.
struct Choice {
    provider: &'static str,
    title: String,
    content: Option<Vec<String>>,
    send: Map<String, Value>,
}
/// Parse `build.dest` (projr `.yml_build_role_parse`, SATVILab/projr@5f915ef
/// `R/yml-dest.R:615-659`): a name, a list of names or one-entry mappings, or a
/// mapping of global options and remote names. Inline `build.local`/`build.github`
/// archives are rejected with projr's guidance.
fn choices(config: &ProjConfig) -> Result<Vec<Choice>> {
    for (provider, value) in [
        ("local", &config.build.local),
        ("github", &config.build.github),
    ] {
        anyhow::ensure!(
            value.is_none(),
            "build.{provider} is no longer supported. Define each archive under remotes.{provider}.<title> and select it with build.dest"
        );
    }
    let Some(dest) = &config.build.dest else {
        return Ok(Vec::new());
    };
    let mut global = Map::new();
    let mut entries: Vec<(String, Value)> = Vec::new();
    let mut push = |name: String, entry: Value| -> Result<()> {
        anyhow::ensure!(
            !entries.iter().any(|(previous, _)| *previous == name),
            "build.dest names remote {name} more than once"
        );
        entries.push((name, entry));
        Ok(())
    };
    match dest {
        Value::String(name) => push(name.clone(), Value::Null)?,
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::String(name) => {
                        anyhow::ensure!(
                            !DEST_OPTIONS.contains(&name.as_str()),
                            "build.dest options belong in a mapping, not a list"
                        );
                        push(name.clone(), Value::Null)?
                    }
                    Value::Object(map) if map.len() == 1 => {
                        let (name, entry) = map.iter().next().unwrap();
                        anyhow::ensure!(
                            !DEST_OPTIONS.contains(&name.as_str()),
                            "build.dest options belong in a mapping, not a list"
                        );
                        push(name.clone(), entry.clone())?
                    }
                    _ => anyhow::bail!(
                        "build.dest entries must be remote names or one-entry mappings"
                    ),
                }
            }
        }
        Value::Object(map) => {
            for (name, entry) in map {
                if DEST_OPTIONS.contains(&name.as_str()) {
                    global.insert(name.clone(), entry.clone());
                } else {
                    push(name.clone(), entry.clone())?
                }
            }
        }
        Value::Null => {}
        _ => anyhow::bail!("build.dest must name remotes declared under remotes"),
    }
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for (reference, entry) in entries {
        let mut settings = global.clone();
        match entry {
            Value::Null => {}
            Value::Object(map) => {
                for key in map.keys() {
                    anyhow::ensure!(
                        DEST_OPTIONS.contains(&key.as_str()),
                        "build.dest.{reference} only accepts content, cue, inspect and strategy; put remote settings under remotes"
                    );
                }
                // projr's modifyList drops NULL entries, so `content: ~` inherits.
                settings.extend(map.into_iter().filter(|(_, value)| !value.is_null()));
            }
            _ => anyhow::bail!("build.dest.{reference} must be a YAML mapping"),
        }
        let (provider, title) = resolve(config, &reference, "build.dest")?;
        anyhow::ensure!(
            seen.insert((provider, title.clone())),
            "build.dest names remote {provider}:{title} more than once"
        );
        let content = settings
            .remove("content")
            .map(|value| -> Result<Vec<String>> {
                value
                    .as_array()
                    .with_context(|| {
                        format!("build.dest.{reference}.content must list the labels to publish")
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
            .transpose()?;
        result.push(Choice {
            provider,
            title,
            content,
            send: settings,
        });
    }
    Ok(result)
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
    send: Map<String, Value>,
) -> Result<Value> {
    validate_definition(value)?;
    let declared_content = value.get("content");
    let mut value = value.clone();
    let mapping = value
        .as_object_mut()
        .context("Remote definition must be a mapping")?;
    if !send.is_empty() {
        anyhow::ensure!(
            !matches!(
                mapping.get("structure").and_then(Value::as_str),
                Some("cas" | "xet")
            ),
            "build.dest cue/inspect/strategy apply to ordinary local and GitHub archives"
        );
        // projr: per-build send settings override the remote's `send`.
        let merged = mapping
            .entry("send")
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .context("send must be a mapping")?;
        merged.extend(send);
    }
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
/// The registered definition for a provider and title, if any.
pub(crate) fn definition_of<'a>(
    config: &'a ProjConfig,
    provider: &str,
    title: &str,
) -> Option<&'a Value> {
    let registry = &config.remotes.registry;
    match provider {
        "local" => registry
            .local
            .get(title)
            .or_else(|| registry.cas.get(title))
            .or_else(|| registry.xet.get(title)),
        _ => registry.github.get(title),
    }
}
/// Reject projr 0.9's removed inline archives (restore projection); upload
/// selection itself never limits restore sources.
pub(crate) fn reject_inline(config: &ProjConfig) -> Result<()> {
    let mut projected = config.clone();
    projected.build.dest = None;
    choices(&projected).map(|_| ())
}
/// Freeze the remotes `build.dest` selects, without credentials, filesystem effects
/// or transport activity. Registered remotes not listed send nothing.
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
    for Choice {
        provider,
        title,
        content: subset,
        send,
    } in choices(config)?
    {
        if provider == "github" {
            let value = &config.remotes.registry.github[&title];
            let selected = selected_definition(config, value, subset, send)?;
            match value.get("structure").and_then(Value::as_str) {
                Some("xet") => github_xet.insert(title, selected),
                Some("cas") => github_cas.insert(title, selected),
                _ => github.insert(title, selected),
            };
        } else if let Some(value) = config.remotes.registry.local.get(&title) {
            local.insert(title, selected_definition(config, value, subset, send)?);
        } else if let Some(value) = config.remotes.registry.xet.get(&title) {
            let value = selected_definition(config, value, subset, send)?;
            xet.push(crate::xet_remote::Selection {
                title,
                definition: crate::xet_remote::definition(&value)?,
            });
        } else if let Some(value) = config.remotes.registry.cas.get(&title) {
            let value = selected_definition(config, value, subset, send)?;
            cas.push(crate::cas_remote::Selection {
                title,
                definition: crate::cas_remote::Definition::parse(&value)?,
            });
        } else {
            anyhow::ensure!(
                send.is_empty(),
                "build.dest cue/inspect/strategy apply to ordinary local and GitHub archives"
            );
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
    Ok(match provider {
        "local" => definition_of(config, provider, title).cloned().or_else(|| {
            config
                .remotes
                .local
                .as_ref()
                .and_then(|values| values.get(title))
                .map(crate::cas_remote::legacy_value)
        }),
        "github" => definition_of(config, provider, title).cloned(),
        _ => anyhow::bail!("Unsupported directory source provider"),
    })
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
            "build":{"scripts":[],"git":false,"dest":{"B":{"content":["output"]}}}
        })).unwrap()
    }
    fn dest(config: &mut ProjConfig, value: Value) {
        config.build.dest = Some(value);
    }
    #[test]
    fn registered_sources_are_preserved_when_only_another_remote_is_selected() {
        let config = example();
        let targets = selected(&config).unwrap();
        let local = targets.local.unwrap();
        assert_eq!(local.keys().map(String::as_str).collect::<Vec<_>>(), ["B"]);
        assert_eq!(local["B"]["content"], serde_json::json!(["output"]));
        assert!(targets.legacy_cas.is_empty());
        assert!(definition_of(&config, "local", "A").is_some());
    }
    #[test]
    fn name_list_and_mapping_forms_follow_projr() {
        let mut config = example();
        for form in [
            serde_json::json!("B"),
            serde_json::json!(["B"]),
            serde_json::json!({"B":null}),
            serde_json::json!({"B":{}}),
            serde_json::json!([{"B":{}}]),
        ] {
            dest(&mut config, form.clone());
            assert_eq!(
                selected(&config).unwrap().local.unwrap()["B"]["content"],
                true,
                "{form}"
            );
        }
        dest(
            &mut config,
            serde_json::json!(["A", {"B":{"content":["raw"]}}]),
        );
        let local = selected(&config).unwrap().local.unwrap();
        assert_eq!(local["A"]["content"], serde_json::json!(["raw"]));
        assert_eq!(local["B"]["content"], serde_json::json!(["raw"]));
        // Global options apply to every remote; per-remote values win.
        dest(
            &mut config,
            serde_json::json!({"cue":"always","content":["output"],"B":{"inspect":"file"},"A":{"content":["raw"]}}),
        );
        let local = selected(&config).unwrap().local.unwrap();
        assert_eq!(
            local["B"]["send"],
            serde_json::json!({"cue":"always","inspect":"file"})
        );
        assert_eq!(local["B"]["content"], serde_json::json!(["output"]));
        assert_eq!(local["A"]["content"], serde_json::json!(["raw"]));
        assert!(local["B"].get("manifest-style").is_none());
        // projr: a null setting inherits the global value.
        dest(
            &mut config,
            serde_json::json!({"content":["output"],"B":{"content":null}}),
        );
        assert_eq!(
            selected(&config).unwrap().local.unwrap()["B"]["content"],
            serde_json::json!(["output"])
        );
        config.build.dest = None;
        assert!(selected(&config).unwrap().local.unwrap().is_empty());
        dest(&mut config, serde_json::json!({"B":{"content":[]}}));
        assert_eq!(
            selected(&config).unwrap().local.unwrap()["B"]["content"],
            serde_json::json!([])
        );
    }
    #[test]
    fn qualified_names_disambiguate_titles_registered_for_both_types() {
        let mut config = example();
        config
            .remotes
            .registry
            .github
            .insert("B".into(), serde_json::json!({}));
        dest(&mut config, serde_json::json!(["B"]));
        let error = selected(&config).err().unwrap().to_string();
        assert!(error.contains("Use local:B or github:B"), "{error}");
        dest(&mut config, serde_json::json!(["github:B", "local:B"]));
        let targets = selected(&config).unwrap();
        assert!(targets.local.unwrap().contains_key("B"));
        assert!(targets.github.unwrap().contains_key("B"));
        assert_eq!(
            resolve(&config, "github:B", "x").unwrap(),
            ("github", "B".into())
        );
        assert!(resolve(&config, "github:A", "x").is_err());
        dest(&mut config, serde_json::json!(["local:B", "local:B"]));
        assert!(selected(&config).is_err());
    }
    #[test]
    fn explicit_remote_null_remains_disabled_and_subsets_cannot_override_it() {
        let mut config = example();
        config.remotes.registry.local.get_mut("B").unwrap()["content"] = Value::Null;
        dest(&mut config, serde_json::json!({"B":{}}));
        assert!(selected(&config).unwrap().local.unwrap()["B"]["content"].is_null());
        dest(&mut config, serde_json::json!({"B":{"content":["output"]}}));
        assert!(selected(&config).is_err());
    }
    #[test]
    fn invalid_selections_and_inline_archives_fail_purely() {
        for value in [
            serde_json::json!({"missing":{}}),
            serde_json::json!({"B":{"content":["undeclared"]}}),
            serde_json::json!({"A":{"content":["output"]}}),
            serde_json::json!({"B":{"path":"../elsewhere"}}),
            serde_json::json!({"B":{"content":["raw","raw"]}}),
            serde_json::json!(["cue"]),
            serde_json::json!([{"A":{},"B":{}}]),
            serde_json::json!(["B", "B"]),
            serde_json::json!(3),
        ] {
            let mut config = example();
            dest(&mut config, value.clone());
            assert!(selected(&config).is_err(), "{value}");
        }
        let mut config = example();
        config.build.local = Some(serde_json::json!({"old":{"path":"../old"}}));
        let error = selected(&config).err().unwrap().to_string();
        assert!(
            error.contains("build.local is no longer supported. Define each archive under remotes.local.<title> and select it with build.dest"),
            "{error}"
        );
        let mut config = example();
        config
            .remotes
            .registry
            .local
            .insert("b".into(), serde_json::json!({"path":"../b"}));
        assert!(selected(&config).is_err());
    }
    #[test]
    fn reserved_and_store_titles_are_rejected() {
        let reserved: Result<ProjConfig, _> = serde_json::from_value(
            serde_json::json!({"remotes":{"local":{"content":{"path":"../x"}}}}),
        );
        assert!(validate_names(&reserved.unwrap()).is_err());
        let mut config = example();
        config
            .remotes
            .registry
            .github
            .insert("A".into(), serde_json::json!({"structure":"cas"}));
        assert!(validate_names(&config).is_err());
    }
    #[test]
    fn legacy_cas_selection_uses_build_dest() {
        let legacy: ProjConfig = serde_json::from_value(serde_json::json!({
            "directories":{"raw":{"path":"input"}},
            "remotes":{"local":{
                "a":{"path":"../a","structure":"cas","content":["raw"],"inspect":"manifest"},
                "A":{"path":"../A","structure":"cas","content":["raw"],"inspect":"file"}}},
            "build":{"dest":["a","A"]}
        }))
        .unwrap();
        // Names differing only in case are ambiguous within a type.
        assert!(selected(&legacy).is_err());
        let legacy: ProjConfig = serde_json::from_value(serde_json::json!({
            "directories":{"raw":{"path":"input"}},
            "remotes":{"local":{
                "a":{"path":"../a","structure":"cas","content":["raw"],"inspect":"manifest"}}},
            "build":{"dest":{"a":{"content":["raw"]}}}
        }))
        .unwrap();
        let targets = selected(&legacy).unwrap();
        assert_eq!(targets.legacy_cas, ["a"]);
        assert_eq!(targets.legacy_content["a"], ["raw"]);
    }
    #[test]
    fn directory_source_references_validate_without_auto_restore_or_upload_selection() {
        let mut config = example();
        config.directories.get_mut("raw").unwrap().source = Some(serde_json::json!({"local":"A"}));
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
            serde_json::json!({"title":"inline only"}),
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
            "build":{"dest":{"new":{"content":["raw"]}}}
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
            selected.cas[0].definition.manifest_style,
            crate::cas_remote::Style::Tree
        );
        let mut with_send = modern.clone();
        with_send.build.dest = Some(serde_json::json!({"new":{"cue":"always"}}));
        assert!(super::selected(&with_send).is_err());

        let legacy:ProjConfig=serde_json::from_value(serde_json::json!({
            "directories":{"raw":{"path":"input"},"output":{"path":"output"}},
            "remotes":{"local":{"old":{"path":"../old","structure":"cas","content":["RAW","output"],"inspect":"file"}}},
            "build":{"dest":{"old":{"content":["raw"]}}}
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
            "build":{"dest":{"old":{"content":[]}}}
        })).unwrap();
        assert!(selected(&config).unwrap().legacy_cas.is_empty());
    }
}
