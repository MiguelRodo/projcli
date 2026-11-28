//! projr named pipelines: ordered `scripts` and `hooks` under top-level
//! `pipelines`, selected by `build.pipeline`, `dev.pipeline` or `--pipeline`
//! (SATVILab/projr@5f915ef `R/pipeline.R`).
//!
//! Inline `scripts` and `hooks` combine with the selected pipelines. Whether they
//! run before or after the pipelines' entries follows their YAML key position
//! relative to `pipeline` in the highest-precedence layer that sets `pipeline`, as
//! in projr. **Different from projr (user decision D4, SATVILab/projr#1074):** a
//! development build that inherits `build.pipeline` runs its scripts but not its
//! hooks; pipeline hooks run in a development build only when `dev.pipeline` or
//! `--pipeline` selects them explicitly.
use crate::yml::{HooksConfig, ProjConfig};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// One named pipeline: its scripts (entries as in `build.scripts`) and hooks.
///
/// ```rust
/// use proj::pipeline::PipelineConfig;
/// let pipeline: PipelineConfig =
///     serde_json::from_value(serde_json::json!({"scripts": ["a.R", {"exclude": "b.R"}]})).unwrap();
/// assert_eq!(pipeline.scripts, Some(vec!["a.R".to_string(), "!b.R".to_string()]));
/// ```
#[derive(Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PipelineConfig {
    /// Script paths, globs and exclusions; omitted adds none.
    #[serde(default, deserialize_with = "crate::yml::script_list")]
    pub scripts: Option<Vec<String>>,
    /// `pre`/`post`/`both` hook paths.
    #[serde(default)]
    pub hooks: Option<HooksConfig>,
}

/// Top-level `pipelines`, in YAML key order. Names cannot be empty or contain
/// `:` or `,` (projr passes several names comma-separated).
///
/// # Errors
///
/// Deserialization fails for a non-mapping, an invalid name or an invalid pipeline.
///
/// ```rust
/// use proj::pipeline::Pipelines;
/// let pipelines: Pipelines =
///     serde_json::from_value(serde_json::json!({"b": {"scripts": "b.R"}, "a": null})).unwrap();
/// assert_eq!(pipelines.names().collect::<Vec<_>>(), ["b", "a"]);
/// assert!(serde_json::from_value::<Pipelines>(serde_json::json!({"a,b": {}})).is_err());
/// ```
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Pipelines(pub Vec<(String, PipelineConfig)>);

impl Pipelines {
    /// Pipeline names in definition order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(name, _)| name.as_str())
    }

    fn get(&self, name: &str) -> Option<&PipelineConfig> {
        self.0
            .iter()
            .find(|(defined, _)| defined == name)
            .map(|(_, pipeline)| pipeline)
    }
}

impl<'de> Deserialize<'de> for Pipelines {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let Some(map) = Option::<serde_json::Map<String, Value>>::deserialize(deserializer)
            .map_err(|_| D::Error::custom("pipelines must be a YAML mapping"))?
        else {
            return Ok(Self::default());
        };
        map.into_iter()
            .map(|(name, value)| {
                if name.is_empty() {
                    return Err(D::Error::custom(
                        "pipelines: pipeline names cannot be empty.",
                    ));
                }
                if name.contains([':', ',']) {
                    return Err(D::Error::custom(format!(
                        "pipelines.{name}: pipeline names cannot contain ':' or ','."
                    )));
                }
                let pipeline = if value.is_null() {
                    PipelineConfig::default()
                } else {
                    serde_json::from_value(value)
                        .map_err(|error| D::Error::custom(format!("pipelines.{name}: {error}")))?
                };
                Ok((name, pipeline))
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }
}

/// A `build.pipeline`/`dev.pipeline`/`--pipeline` value.
///
/// `false` or `[]` selects no pipeline; a name or glob, or a list of them, selects
/// the matching pipelines. An omitted key (or YAML null) is `None` in the
/// containing `Option`.
///
/// ```rust
/// use proj::pipeline::Selection;
/// let none: Selection = serde_json::from_value(serde_json::json!(false)).unwrap();
/// assert_eq!(none, Selection::None);
/// let one: Selection = serde_json::from_value(serde_json::json!("report*")).unwrap();
/// assert_eq!(one, Selection::Entries(vec!["report*".into()]));
/// assert!(serde_json::from_value::<Selection>(serde_json::json!(true)).is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// No pipeline.
    None,
    /// Pipeline names or globs, in order.
    Entries(Vec<String>),
}

const SELECTION_HELP: &str =
    "pipeline selects pipeline names or globs (a name or a list), or false for none";

impl<'de> Deserialize<'de> for Selection {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let entry = |value: Value| match value {
            Value::String(text) if !text.trim().is_empty() => Ok(text),
            _ => Err(D::Error::custom(SELECTION_HELP)),
        };
        match Value::deserialize(deserializer)? {
            Value::Bool(false) => Ok(Self::None),
            Value::Array(entries) if entries.is_empty() => Ok(Self::None),
            Value::Array(entries) => entries
                .into_iter()
                .map(entry)
                .collect::<Result<_, _>>()
                .map(Self::Entries),
            value => entry(value).map(|text| Self::Entries(vec![text])),
        }
    }
}

impl Selection {
    /// Parses the `--pipeline` argument: `false` selects none; otherwise one or
    /// more comma-separated names or globs.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty argument or entry.
    ///
    /// ```rust
    /// use proj::pipeline::Selection;
    /// assert_eq!(Selection::parse_argument("false").unwrap(), Selection::None);
    /// assert_eq!(
    ///     Selection::parse_argument("prep, report*").unwrap(),
    ///     Selection::Entries(vec!["prep".into(), "report*".into()])
    /// );
    /// assert!(Selection::parse_argument("a,,b").is_err());
    /// ```
    pub fn parse_argument(text: &str) -> anyhow::Result<Self> {
        if text.trim() == "false" {
            return Ok(Self::None);
        }
        text.split(',')
            .map(|entry| {
                let entry = entry.trim();
                anyhow::ensure!(
                    !entry.is_empty(),
                    "--pipeline takes pipeline names or globs separated by commas, or false"
                );
                Ok(entry.to_owned())
            })
            .collect::<anyhow::Result<_>>()
            .map(Self::Entries)
    }
}

/// projr's `glob2rx` matching for pipeline names: `*` and `?` are wildcards and
/// every other character is literal.
fn glob_matches(pattern: &[char], name: &[char]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some(('*', rest)) => (0..=name.len()).any(|skip| glob_matches(rest, &name[skip..])),
        Some((first, rest)) => name.split_first().is_some_and(|(head, tail)| {
            (*first == '?' || first == head) && glob_matches(rest, tail)
        }),
    }
}

/// Pipeline names for the selected entries: entries in the order given, a glob's
/// matches in definition order, each pipeline once at its first position.
fn resolve(pipelines: &Pipelines, entries: &[String], key: &str) -> anyhow::Result<Vec<String>> {
    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        let pattern: Vec<char> = entry.chars().collect();
        let matched: Vec<&str> = pipelines
            .names()
            .filter(|name| glob_matches(&pattern, &name.chars().collect::<Vec<_>>()))
            .collect();
        anyhow::ensure!(
            !matched.is_empty(),
            "{key}: {entry} matches no pipeline defined under pipelines."
        );
        for name in matched {
            if !names.iter().any(|known| known == name) {
                names.push(name.to_owned());
            }
        }
    }
    Ok(names)
}

/// Checks that `build.pipeline` and `dev.pipeline` match defined pipelines.
///
/// # Errors
///
/// Returns projr's "matches no pipeline" error for the first unmatched entry.
pub fn validate(config: &ProjConfig) -> anyhow::Result<()> {
    for (key, selection) in [
        ("build.pipeline", &config.build.pipeline),
        ("dev.pipeline", &config.dev.pipeline),
    ] {
        if let Some(Selection::Entries(entries)) = selection {
            resolve(&config.pipelines, entries, key)?;
        }
    }
    Ok(())
}

/// Every hook entry of every defined pipeline, for excluding hook files from
/// automatic script discovery (projr `R/scripts.R:92-99`).
pub fn hook_entries(config: &ProjConfig) -> Vec<String> {
    config
        .pipelines
        .0
        .iter()
        .filter_map(|(_, pipeline)| pipeline.hooks.as_ref())
        .flat_map(|hooks| [&hooks.pre, &hooks.post, &hooks.both])
        .flatten()
        .flatten()
        .cloned()
        .collect()
}

/// Whether inline entries run before the pipelines' entries. They do unless the
/// highest-precedence layer that sets `pipeline` places `key` after it; a
/// development build checks `dev` blocks before `build` blocks
/// (projr `R/pipeline.R:113-130`). `layers` are in precedence order, highest first.
fn inline_first(key: &str, dev: bool, layers: &[Value]) -> bool {
    let blocks: &[&str] = if dev { &["dev", "build"] } else { &["build"] };
    for block in blocks {
        for layer in layers {
            let Some(keys) = layer.get(*block).and_then(Value::as_object) else {
                continue;
            };
            let position = |wanted: &str| keys.keys().position(|name| name == wanted);
            let Some(pipeline) = position("pipeline") else {
                continue;
            };
            return position(key).is_none_or(|position| position < pipeline);
        }
    }
    true
}

/// Placement of inline entries relative to the pipelines' entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Inline entries run first.
    InlineFirst,
    /// The pipelines' entries run first.
    PipelineFirst,
}

impl Placement {
    fn from_inline_first(inline_first: bool) -> Self {
        if inline_first {
            Self::InlineFirst
        } else {
            Self::PipelineFirst
        }
    }

    /// Stable record spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InlineFirst => "inline_first",
            Self::PipelineFirst => "pipeline_first",
        }
    }
}

/// The scripts and hooks a build runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Selected pipelines, in run order; empty without a pipeline.
    pub names: Vec<String>,
    /// Script entries, or `None` for automatic discovery. With a pipeline this is
    /// always explicit, so a pipeline without scripts and no inline scripts runs none.
    pub scripts: Option<Vec<String>>,
    /// Hooks to run. With pipeline hooks, `pre`/`post` hold the combined entries
    /// (each once) and `both` is empty.
    pub hooks: HooksConfig,
    /// Script placement, when a pipeline is selected.
    pub scripts_placement: Option<Placement>,
    /// Hook placement, when pipeline hooks are selected.
    pub hooks_placement: Option<Placement>,
}

fn stage(hooks: Option<&HooksConfig>, stage: &Option<Vec<String>>) -> Vec<String> {
    let both = hooks.and_then(|hooks| hooks.both.as_ref());
    stage.iter().chain(both).flatten().cloned().collect()
}

/// Resolves the scripts and hooks for a build.
///
/// `argument` is `--pipeline` (it overrides `build.pipeline`/`dev.pipeline`);
/// `scripts` is `--scripts`, which takes the place of the inline scripts and
/// combines with any pipeline. `layers` are the configuration layers in precedence
/// order (highest first), read with their YAML key order.
///
/// # Errors
///
/// Returns an error when a selected entry matches no defined pipeline.
pub fn plan(
    config: &ProjConfig,
    layers: &[Value],
    dev: bool,
    argument: Option<&Selection>,
    scripts: Option<Vec<String>>,
) -> anyhow::Result<Plan> {
    let (inline_scripts, inline_hooks) = if dev {
        (
            config.dev.scripts.clone().or(config.build.scripts.clone()),
            config.dev.hooks.clone().unwrap_or_default(),
        )
    } else {
        (config.build.scripts.clone(), config.build.hooks.clone())
    };
    let inline_scripts = scripts.or(inline_scripts);
    // D4: a development build inherits build.pipeline's scripts, not its hooks.
    let (key, selection, explicit) = match (argument, dev, &config.dev.pipeline) {
        (Some(argument), _, _) => ("--pipeline", Some(argument), true),
        (None, true, Some(selection)) => ("dev.pipeline", Some(selection), true),
        (None, true, None) => ("build.pipeline", config.build.pipeline.as_ref(), false),
        (None, false, _) => ("build.pipeline", config.build.pipeline.as_ref(), true),
    };
    let Some(Selection::Entries(entries)) = selection else {
        return Ok(Plan {
            names: Vec::new(),
            scripts: inline_scripts,
            hooks: inline_hooks,
            scripts_placement: None,
            hooks_placement: None,
        });
    };
    let names = resolve(&config.pipelines, entries, key)?;
    let pipelines: Vec<&PipelineConfig> = names
        .iter()
        .filter_map(|name| config.pipelines.get(name))
        .collect();
    let own: Vec<String> = pipelines
        .iter()
        .flat_map(|pipeline| pipeline.scripts.iter().flatten().cloned())
        .collect();
    let inline = inline_scripts.unwrap_or_default();
    let scripts_first = inline_first("scripts", dev, layers);
    let scripts = if scripts_first {
        [inline, own].concat()
    } else {
        [own, inline].concat()
    };
    let (hooks, hooks_placement) = if explicit {
        let hooks_first = inline_first("hooks", dev, layers);
        let combine = |select: fn(&HooksConfig) -> &Option<Vec<String>>| {
            let own: Vec<String> = pipelines
                .iter()
                .filter_map(|pipeline| pipeline.hooks.as_ref())
                .flat_map(|hooks| stage(Some(hooks), select(hooks)))
                .collect();
            let inline = stage(Some(&inline_hooks), select(&inline_hooks));
            let mut combined: Vec<String> = Vec::new();
            let ordered = if hooks_first {
                [inline, own]
            } else {
                [own, inline]
            };
            // A hook named more than once runs once, at its first position.
            for hook in ordered.into_iter().flatten() {
                if !combined.contains(&hook) {
                    combined.push(hook);
                }
            }
            Some(combined)
        };
        (
            HooksConfig {
                pre: combine(|hooks| &hooks.pre),
                post: combine(|hooks| &hooks.post),
                both: None,
            },
            Some(Placement::from_inline_first(hooks_first)),
        )
    } else {
        (inline_hooks, None)
    };
    Ok(Plan {
        names,
        scripts: Some(scripts),
        hooks,
        scripts_placement: Some(Placement::from_inline_first(scripts_first)),
        hooks_placement,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(value: Value) -> ProjConfig {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn globs_match_whole_names_with_star_and_question_mark_only() {
        let matches = |pattern: &str, name: &str| {
            glob_matches(
                &pattern.chars().collect::<Vec<_>>(),
                &name.chars().collect::<Vec<_>>(),
            )
        };
        assert!(matches("report*", "report-a"));
        assert!(matches("r?port", "report"));
        assert!(matches("*", ""));
        assert!(!matches("report", "report-a"));
        assert!(!matches("[r]eport", "report"));
        assert!(matches("a.b", "a.b"));
        assert!(!matches("a.b", "axb"));
    }

    #[test]
    fn resolution_keeps_entry_order_and_runs_each_pipeline_once() {
        let config = config(json!({"pipelines": {"b": {}, "a1": {}, "a2": {}}}));
        let names = resolve(
            &config.pipelines,
            &["a2".into(), "a*".into(), "b".into()],
            "k",
        );
        assert_eq!(names.unwrap(), ["a2", "a1", "b"]);
        let error = resolve(&config.pipelines, &["c*".into()], "build.pipeline").unwrap_err();
        assert_eq!(
            error.to_string(),
            "build.pipeline: c* matches no pipeline defined under pipelines."
        );
    }

    #[test]
    fn placement_follows_key_order_in_the_highest_layer_that_sets_pipeline() {
        let after = json!({"build": {"pipeline": "p", "scripts": ["s"]}});
        let before = json!({"build": {"scripts": ["s"], "pipeline": "p"}});
        let unset = json!({"build": {"scripts": ["s"]}});
        assert!(!inline_first(
            "scripts",
            false,
            std::slice::from_ref(&after)
        ));
        assert!(inline_first(
            "scripts",
            false,
            std::slice::from_ref(&before)
        ));
        assert!(inline_first("hooks", false, std::slice::from_ref(&after)));
        assert!(inline_first(
            "scripts",
            false,
            &[unset.clone(), before.clone()]
        ));
        assert!(!inline_first(
            "scripts",
            false,
            &[unset, after.clone(), before.clone()]
        ));
        // A development build reads dev blocks before build blocks.
        let dev = json!({"dev": {"scripts": ["s"], "pipeline": "p"}});
        assert!(inline_first("scripts", true, &[after.clone(), dev]));
        assert!(!inline_first("scripts", true, &[after]));
    }

    #[test]
    fn development_builds_inherit_production_scripts_but_not_hooks() {
        let config = config(json!({
            "pipelines": {"p": {"scripts": ["p.R"], "hooks": {"pre": ["p-hook.R"]}}},
            "build": {"pipeline": "p", "scripts": ["b.R"], "hooks": {"pre": ["b-hook.R"]}},
            "dev": {"hooks": {"pre": ["d-hook.R"]}},
        }));
        let layers = [json!({"build": {"pipeline": "p", "scripts": ["b.R"]}})];
        let inherited = plan(&config, &layers, true, None, None).unwrap();
        assert_eq!(inherited.names, ["p"]);
        assert_eq!(inherited.scripts.unwrap(), ["p.R", "b.R"]);
        assert_eq!(inherited.hooks.pre.unwrap(), ["d-hook.R"]);
        assert_eq!(inherited.hooks_placement, None);
        let explicit = plan(
            &config,
            &layers,
            true,
            Some(&Selection::Entries(vec!["p".into()])),
            None,
        )
        .unwrap();
        assert_eq!(explicit.hooks.pre.unwrap(), ["d-hook.R", "p-hook.R"]);
        let production = plan(&config, &layers, false, None, None).unwrap();
        assert_eq!(production.hooks.pre.unwrap(), ["b-hook.R", "p-hook.R"]);
        let none = plan(
            &config,
            &layers,
            false,
            Some(&Selection::None),
            Some(vec!["x.R".into()]),
        )
        .unwrap();
        assert!(none.names.is_empty());
        assert_eq!(none.scripts.unwrap(), ["x.R"]);
        assert_eq!(none.hooks.pre.unwrap(), ["b-hook.R"]);
    }

    #[test]
    fn pipeline_hooks_combine_stages_and_run_once() {
        let config = config(json!({
            "pipelines": {
                "a": {"hooks": {"pre": ["x.R"], "both": ["y.R"]}},
                "b": {"hooks": {"post": ["x.R"], "both": ["y.R", "z.R"]}}
            },
            "build": {"pipeline": ["a", "b"], "hooks": {"both": ["x.R"]}},
        }));
        let plan = plan(&config, &[], false, None, None).unwrap();
        assert_eq!(plan.hooks.pre.unwrap(), ["x.R", "y.R", "z.R"]);
        assert_eq!(plan.hooks.post.unwrap(), ["x.R", "y.R", "z.R"]);
        assert_eq!(plan.hooks.both, None);
        // A pipeline without scripts and no inline scripts selects none.
        assert_eq!(plan.scripts, Some(vec![]));
    }
}
