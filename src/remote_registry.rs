//! Ordinary named remote definitions are independent of current upload selection.
//! Legacy CAS definitions/list destinations and inline ordinary builds remain readable.
use anyhow::{Context,Result};
use serde::{Deserialize,Deserializer};
use serde_json::{Map,Value};
use std::collections::{HashMap,HashSet};
use crate::yml::{ProjConfig,LocalRemoteConfig,RemotesConfig};

/// Named definitions retained even when no build sends to them; legacy CAS remains separately typed.
#[derive(Clone,Debug,Default)]
pub struct Registry {
    /// Named local latest/archive definitions, using the existing inline definition schema.
    pub local:Map<String,Value>,
    /// Named GitHub Release definitions, using the existing inline definition schema.
    pub github:Map<String,Value>,
    /// Modern local CAS definitions; legacy accepted CAS forms remain separately typed.
    pub cas:Map<String,Value>,
}

/// Compatibility name for the initially ordinary-only registry type.
pub type OrdinaryRegistry=Registry;

/// Legacy names or explicitly named registry targets with optional label subsets.
#[derive(Clone,Debug,Deserialize)]
#[serde(untagged)]
pub enum Destinations {
    /// Existing CAS name lists also permit registered ordinary names.
    Names(Vec<String>),
    /// Each value is a mapping with optional content list; omitted means remote-eligible labels.
    Named(Map<String,Value>),
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryWire {
    local:Option<Map<String,Value>>,
    github:Option<Map<String,Value>>,
}
impl<'de> Deserialize<'de> for RemotesConfig {
    fn deserialize<D:Deserializer<'de>>(deserializer:D)->std::result::Result<Self,D::Error> {
        let wire=RegistryWire::deserialize(deserializer)?;
        let mut legacy=HashMap::new();let mut registry=Registry::default();
        for (name,value) in wire.local.unwrap_or_default() {
            if value.get("structure").and_then(Value::as_str)==Some("cas") {
                if crate::cas_remote::legacy_shape(&value) {
                    let remote:LocalRemoteConfig=serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                    legacy.insert(name,remote);
                }else{
                    crate::cas_remote::Definition::parse(&value).map_err(serde::de::Error::custom)?;
                    registry.cas.insert(name,value);
                }
            }else{
                crate::restore::definition(&value).map_err(serde::de::Error::custom)?;
                registry.local.insert(name,value);
            }
        }
        registry.github=wire.github.unwrap_or_default();
        for value in registry.github.values() {
            crate::restore::definition(value).map_err(serde::de::Error::custom)?;
        }
        Ok(RemotesConfig {local:if legacy.is_empty(){None}else{Some(legacy)},registry})
    }
}

/// Selected definitions, without credentials, filesystem effects or transport activity.
pub(crate) struct Selected {
    pub(crate) local:Option<Map<String,Value>>,
    pub(crate) github:Option<Map<String,Value>>,
    pub(crate) legacy_cas:Vec<String>,
    pub(crate) legacy_content:HashMap<String,Vec<String>>,
    pub(crate) cas:Vec<crate::cas_remote::Selection>,
}
fn name(value:&str)->Result<String> {
    anyhow::ensure!(!value.trim().is_empty()&&!value.chars().any(char::is_control),"Remote names must be nonempty printable text");
    Ok(icu_normalizer::ComposingNormalizer::new_nfc().normalize(&value.to_uppercase()).into_owned())
}
fn legacy_only(config:&ProjConfig)->bool {
    config.remotes.registry.local.is_empty()&&config.remotes.registry.github.is_empty()&&config.remotes.registry.cas.is_empty()
        &&!matches!(&config.build.dest,Some(Destinations::Named(_)))
}
/// Validate registry/legacy lookup names without resolving upload selection.
pub(crate) fn validate_names(config:&ProjConfig)->Result<()> {
    if legacy_only(config){return Ok(())}
    let mut names=HashSet::new();
    for title in config.remotes.registry.local.keys().chain(config.remotes.registry.github.keys()).chain(config.remotes.registry.cas.keys())
        .chain(config.remotes.local.iter().flat_map(|values|values.keys())) {
        anyhow::ensure!(names.insert(name(title)?),"Remote names are ambiguous across providers or portable spelling");
    }
    for (provider,registered,inline) in [("local",&config.remotes.registry.local,config.build.local.as_ref()),
        ("local",&config.remotes.registry.cas,config.build.local.as_ref()),
        ("github",&config.remotes.registry.github,config.build.github.as_ref())] {
        for title in registered.keys() {
            anyhow::ensure!(!inline.is_some_and(|values|values.contains_key(title)),
                "Remote '{title}' is defined in both remotes.{provider} and build.{provider}; keep one definition");
        }
    }
    Ok(())
}
fn subset(value:&Value)->Result<Option<Vec<String>>> {
    let mapping=value.as_object().context("build.dest entries must be mappings with optional content")?;
    anyhow::ensure!(mapping.keys().all(|key|key=="content"),"build.dest entries only accept content");
    mapping.get("content").map(|value| {
        value.as_array().context("build.dest content must be a label list; omit it for all eligible labels")?
            .iter().map(|label|label.as_str().map(str::to_owned).context("Destination content labels must be strings")).collect()
    }).transpose()
}
fn validate_definition(value:&Value)->Result<()> {
    if value.get("structure").and_then(Value::as_str)==Some("cas") {
        if crate::cas_remote::legacy_shape(value) {let _:LocalRemoteConfig=serde_json::from_value(value.clone())?;}
        else {crate::cas_remote::Definition::parse(value)?;}
    }else{crate::restore::definition(value)?;}
    Ok(())
}
fn selected_definition(config:&ProjConfig,value:&Value,subset:Option<Vec<String>>)->Result<Value> {
    validate_definition(value)?;
    let declared_content=value.get("content");
    let mut value=value.clone();let mapping=value.as_object_mut().context("Remote definition must be a mapping")?;
    if let Some(labels)=subset {
        crate::path_safety::validate_directory_labels(labels.iter().map(String::as_str))?;
        let context=serde_json::json!({"directories":config.directories.keys().map(|label|(label.clone(),Value::Null)).collect::<Map<_,_>>()});
        for label in &labels {
            anyhow::ensure!(config.directories.contains_key(label),"Destination references an undeclared label: {label}");
            if let Some(content)=declared_content {
                anyhow::ensure!(crate::restore::content_has(Some(content),label,&context)?,
                    "Destination label '{label}' is excluded by the registered remote's content");
            }
        }
        mapping.insert("content".into(),serde_json::to_value(labels)?);
    } else if declared_content.is_none() {
        mapping.insert("content".into(),Value::Bool(true));
    }
    Ok(value)
}
/// Freeze ordinary destinations from names while preserving the legacy inline execution path.
/// New mapping selection never implicitly uploads every registered remote.
pub(crate) fn selected(config:&ProjConfig)->Result<Selected> {
    validate_names(config)?;
    validate_sources(config)?;
    let mut local=Map::new();let mut github=Map::new();let mut legacy_cas=Vec::new();
    let mut legacy_content=HashMap::new();let mut cas=Vec::new();
    let mut selected_names=HashSet::new();
    let choices:Vec<(String,Option<Vec<String>>)>=match config.build.dest.as_ref() {
        None=>vec![],
        Some(Destinations::Names(names))=>names.iter().map(|name|(name.clone(),None)).collect(),
        Some(Destinations::Named(names))=>names.iter().map(|(name,value)|Ok((name.clone(),subset(value)?))).collect::<Result<_>>()?,
    };
    for (title,subset) in choices {
        anyhow::ensure!(selected_names.insert(if legacy_only(config){title.clone()}else{name(&title)?}),"Duplicate/aliased build destination name");
        if let Some(value)=config.remotes.registry.local.get(&title) {
            let mut selected=selected_definition(config,value,subset)?;
            selected.as_object_mut().context("Remote definition must be a mapping")?
                .entry("manifest-style").or_insert(Value::String("flat".into()));
            local.insert(title,selected);
        } else if let Some(value)=config.remotes.registry.github.get(&title) {
            let mut selected=selected_definition(config,value,subset)?;
            selected.as_object_mut().context("Remote definition must be a mapping")?
                .entry("manifest-style").or_insert(Value::String("flat".into()));
            github.insert(title,selected);
        } else if let Some(value)=config.remotes.registry.cas.get(&title) {
            let value=selected_definition(config,value,subset)?;
            cas.push(crate::cas_remote::Selection {title,definition:crate::cas_remote::Definition::parse(&value)?});
        } else if let Some(remote)=config.remotes.local.as_ref().and_then(|values|values.get(&title)) {
            let content=if let Some(labels)=subset {
                crate::path_safety::validate_directory_labels(labels.iter().map(String::as_str))?;
                let mut content=Vec::new();
                for label in labels {
                    anyhow::ensure!(config.directories.keys().any(|name|name.eq_ignore_ascii_case(&label)),
                        "Destination references an undeclared label: {label}");
                    let original=remote.content.iter().find(|value|*value==&label)
                        .or_else(||remote.content.iter().find(|value|value.eq_ignore_ascii_case(&label)))
                        .context("Destination label is excluded by the legacy CAS definition")?;
                    content.push(original.clone());
                }
                content
            }else{remote.content.clone()};
            if !content.is_empty()||matches!(&config.build.dest,Some(Destinations::Names(_))) {
                legacy_cas.push(title.clone());
            }
            legacy_content.insert(title,content);
        } else {anyhow::bail!("Build destination '{title}' is not registered in remotes");}
    }
    let modern=!local.is_empty()||!github.is_empty()||!cas.is_empty()||matches!(&config.build.dest,Some(Destinations::Named(_)));
    let inline=config.build.local.as_ref().is_some_and(|values|!values.is_empty())
        ||config.build.github.as_ref().is_some_and(|values|!values.is_empty());
    anyhow::ensure!(!(modern&&inline),"Registry destination selection cannot be mixed with inline build.local/build.github; move the definitions to remotes");
    if !modern {
        return Ok(Selected {local:config.build.local.clone(),github:config.build.github.clone(),legacy_cas,legacy_content,cas});
    }
    Ok(Selected {local:Some(local),github:Some(github),legacy_cas,legacy_content,cas})
}


fn shared(config:&ProjConfig,provider:&str,title:&str)->Result<Option<Value>> {
    name(title)?;
    let registered=match provider {
        "local"=>config.remotes.registry.local.get(title).or_else(||config.remotes.registry.cas.get(title))
            .cloned().or_else(||config.remotes.local.as_ref().and_then(|values|values.get(title)).map(crate::cas_remote::legacy_value)),
        "github"=>config.remotes.registry.github.get(title).cloned(),
        _=>anyhow::bail!("Unsupported directory source provider"),
    };
    let inline=match provider {"local"=>config.build.local.as_ref(),"github"=>config.build.github.as_ref(),_=>None}
        .and_then(|values|values.get(title)).cloned();
    anyhow::ensure!(!(registered.is_some()&&inline.is_some()),"Source name exists in both registry and inline build definitions");
    Ok(registered.or(inline))
}

fn source_reference(config:&ProjConfig,provider:&str,title:&str,inline:Option<&Value>)->Result<()> {
    let value=match (shared(config,provider,title)?,inline) {
        (Some(value),Some(inline))=>crate::yml::deep_merge(value,inline.clone()),
        (Some(value),None)=>value,
        (None,Some(inline))=>inline.clone(),
        (None,None)=>anyhow::bail!("Directory source references an undefined remote: {title}"),
    };
    validate_definition(&value)?;Ok(())
}
/// Validate source names and shape without restoring files or loading credentials.
fn validate_sources(config:&ProjConfig)->Result<()> {
    for directory in config.directories.values() {
        let Some(sources)=directory.source.as_ref().filter(|value|!value.is_null()) else {continue};
        for (provider,entries) in sources.as_object().context("Directory source must be a provider mapping")? {
            anyhow::ensure!(matches!(provider.as_str(),"local"|"github"),"Unsupported directory source provider");
            match entries {
                Value::Null=>{},
                Value::String(title)=>source_reference(config,provider,title,None)?,
                Value::Array(titles)=>for title in titles {
                    source_reference(config,provider,title.as_str().context("Source references must be strings")?,None)?;
                },
                Value::Object(titles)=>for (title,inline) in titles {
                    source_reference(config,provider,title,Some(inline))?;
                },
                _=>anyhow::bail!("Directory source must contain a title, title list or inline definitions"),
            }
        }
    }
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    fn example()->ProjConfig {
        serde_json::from_value(serde_json::json!({
            "directories":{"raw":{"path":"input"},"output":{"path":"results"},"cache":{"path":"cache"}},
            "remotes":{"local":{"A":{"path":"../A","content":["raw"]},"B":{"path":"../B"}}},
            "build":{"scripts":[],"git":false,"dest":{"B":{"content":["output"]}}}
        })).unwrap()
    }
    #[test]
    fn registered_sources_are_preserved_when_only_another_remote_is_selected() {
        let config=example();let targets=selected(&config).unwrap();
        assert_eq!(config.remotes.registry.local.keys().map(String::as_str).collect::<Vec<_>>(),["A","B"]);
        let local=targets.local.unwrap();assert_eq!(local.keys().map(String::as_str).collect::<Vec<_>>(),["B"]);
        assert_eq!(local["B"]["content"],serde_json::json!(["output"]));assert!(targets.legacy_cas.is_empty());
    }
    #[test]
    fn omitted_subset_and_explicit_empty_selection_do_not_mean_registry_wide_uploads() {
        let mut config=example();
        config.build.dest=Some(Destinations::Named(serde_json::from_value(serde_json::json!({"B":{}})).unwrap()));
        assert_eq!(selected(&config).unwrap().local.unwrap()["B"]["content"],true);
        config.build.dest=Some(Destinations::Named(serde_json::from_value(serde_json::json!({"A":{}})).unwrap()));
        assert_eq!(selected(&config).unwrap().local.unwrap()["A"]["content"],serde_json::json!(["raw"]));
        config.build.dest=Some(Destinations::Named(Map::new()));
        assert!(selected(&config).unwrap().local.unwrap().is_empty());
        config.build.dest=None;assert!(selected(&config).unwrap().local.is_none());
        config.build.dest=Some(Destinations::Names(vec![]));assert!(selected(&config).unwrap().local.is_none());
        config.build.dest=Some(Destinations::Named(serde_json::from_value(serde_json::json!({"B":{"content":[]}})).unwrap()));
        assert_eq!(selected(&config).unwrap().local.unwrap()["B"]["content"],serde_json::json!([]));
    }
    #[test]
    fn explicit_remote_null_remains_disabled_and_subsets_cannot_override_it() {
        let mut config=example();
        config.remotes.registry.local.get_mut("B").unwrap()["content"]=Value::Null;
        config.build.dest=Some(Destinations::Named(serde_json::from_value(serde_json::json!({"B":{}})).unwrap()));
        assert!(selected(&config).unwrap().local.unwrap()["B"]["content"].is_null());
        config.build.dest=Some(Destinations::Named(serde_json::from_value(serde_json::json!({"B":{"content":["output"]}})).unwrap()));
        assert!(selected(&config).is_err());
    }
    #[test]
    fn unknown_ambiguous_invalid_or_excluded_selections_fail_purely() {
        for dest in [serde_json::json!({"missing":{}}),serde_json::json!({"B":{"content":["undeclared"]}}),
            serde_json::json!({"A":{"content":["output"]}}),serde_json::json!({"B":{"content":null}}),
            serde_json::json!({"B":null}),serde_json::json!({"B":{"directories":["raw"]}}),
            serde_json::json!({"B":{"content":["raw","raw"]}})] {
            let mut config=example();config.build.dest=Some(Destinations::Named(serde_json::from_value(dest).unwrap()));
            assert!(selected(&config).is_err());
        }
        let mut config=example();config.remotes.registry.github.insert("a".into(),serde_json::json!({}));
        assert!(selected(&config).is_err());
        let mut config=example();config.build.dest=Some(Destinations::Names(vec!["B".into(),"B".into()]));
        assert!(selected(&config).is_err());
    }
    #[test]
    fn registry_selection_rejects_inline_duplicates_without_changing_the_legacy_path() {
        let mut config=example();config.build.local=Some(serde_json::from_value(serde_json::json!({"old":{"path":"../old"}})).unwrap());
        assert!(selected(&config).is_err());
        config.build.dest=Some(Destinations::Named(Map::new()));assert!(selected(&config).is_err());
        config.build.dest=None;assert!(selected(&config).unwrap().local.unwrap().contains_key("old"));
        config.build.local=Some(serde_json::from_value(serde_json::json!({"A":{"path":"../duplicate"}})).unwrap());
        assert!(selected(&config).is_err());
        let legacy:ProjConfig=serde_json::from_value(serde_json::json!({
            "directories":{"raw":{"path":"input"}},
            "remotes":{"local":{
                "a":{"path":"../a","structure":"cas","content":["raw"],"inspect":"manifest"},
                "A":{"path":"../A","structure":"cas","content":["raw"],"inspect":"file"}}},
            "build":{"dest":["a","A"],"local":{"old":{"path":"../old","content":["raw"]}}}
        })).unwrap();
        let targets=selected(&legacy).unwrap();assert_eq!(targets.legacy_cas,["a","A"]);
        assert!(targets.local.unwrap().contains_key("old"));
    }
    #[test]
    fn directory_source_references_validate_without_auto_restore_or_upload_selection() {
        let mut config=example();config.build.dest=Some(Destinations::Named(Map::new()));
        config.directories.get_mut("raw").unwrap().source=Some(serde_json::json!({"local":"A"}));
        assert!(selected(&config).is_ok());
        config.directories.get_mut("raw").unwrap().source=Some(serde_json::json!({"local":"missing"}));
        assert!(selected(&config).is_err());
        config.directories.get_mut("raw").unwrap().source=Some(serde_json::json!({"local":{"custom":{"path":"../custom"}}}));
        assert!(selected(&config).is_ok());
        config.directories.get_mut("raw").unwrap().source=Some(serde_json::json!({"s3":"A"}));
        assert!(selected(&config).is_err());
    }
    #[test]
    fn unsupported_new_policy_values_fail_instead_of_activating_silent_defaults() {
        for remote in [serde_json::json!({"path":"../A","manifest":"unsupported"}),
            serde_json::json!({"path":"../A","manifest-style":"unknown"}),
            serde_json::json!({"path":"../A","manifest-format":null})] {
            assert!(serde_json::from_value::<ProjConfig>(serde_json::json!({"remotes":{"local":{"A":remote}}})).is_err());
        }
        assert!(serde_json::from_value::<ProjConfig>(serde_json::json!({"remotes":{"s3":{}}})).is_err());
    }
}


#[cfg(test)]
mod cas_selection_tests {
    use super::*;
    #[test]
    fn modern_defaults_and_legacy_mapping_subsets_remain_distinct() {
        let modern:ProjConfig=serde_json::from_value(serde_json::json!({
            "directories":{"raw":{"path":"input"},"output":{"path":"output"}},
            "remotes":{"local":{"new":{"path":"../new","structure":"cas"}}},
            "build":{"dest":{"new":{"content":["raw"]}}}
        })).unwrap();
        let selected=selected(&modern).unwrap();assert!(selected.legacy_cas.is_empty());
        assert_eq!(selected.cas.len(),1);assert_eq!(selected.cas[0].definition.content,serde_json::json!(["raw"]));
        assert_eq!(selected.cas[0].definition.manifest,crate::cas_remote::Placement::Auto);
        assert_eq!(selected.cas[0].definition.manifest_style,crate::cas_remote::Style::Tree);

        let legacy:ProjConfig=serde_json::from_value(serde_json::json!({
            "directories":{"raw":{"path":"input"},"output":{"path":"output"}},
            "remotes":{"local":{"old":{"path":"../old","structure":"cas","content":["RAW","output"],"inspect":"file"}}},
            "build":{"dest":{"old":{"content":["raw"]}}}
        })).unwrap();
        let selected=super::selected(&legacy).unwrap();assert!(selected.cas.is_empty());
        assert_eq!(selected.legacy_cas,["old"]);assert_eq!(selected.legacy_content["old"],["RAW"]);
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
