//! Opt-in experimental local Xet routing; content structure is always "xet".
use crate::cas_remote::{Definition, Placement, Style};
use crate::path_safety::portable_paths_overlap as overlaps;
use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use serde_json::Value;
/// Validate experimental local options without filesystem/transport effects.
pub(crate) fn definition(value: &Value) -> Result<Definition> {
    anyhow::ensure!(
        value.get("structure").and_then(Value::as_str) == Some("xet"),
        "Expected structure: xet"
    );
    // Shared shape validation only; never reinterpret legacy CAS IDs or store bytes.
    let mut wire = value.clone();
    wire["structure"] = Value::String("cas".into());
    let parsed = Definition::parse(&wire).context("Invalid experimental Xet definition")?;
    anyhow::ensure!(
        matches!(parsed.manifest, Placement::Auto | Placement::Remote),
        "Experimental Xet currently requires remote full metadata (manifest: auto or remote)"
    );
    anyhow::ensure!(
        parsed.manifest_style == Style::Tree
            && parsed
                .manifest_format
                .is_none_or(|f| f == crate::catalogue::Format::Json),
        "Experimental Xet currently uses tree identities and a complete JSON binding"
    );
    Ok(parsed)
}
/// Named selected definition before physical resolution.
pub(crate) struct Selection {
    pub(crate) title: String,
    pub(crate) definition: Definition,
}
/// Frozen physical store, logical sources and optional production helper.
pub(crate) struct Target {
    pub(crate) base: Utf8PathBuf,
    project: Utf8PathBuf,
    sources: Vec<(String, Utf8PathBuf)>,
    engine: Option<crate::xet::Engine>,
}
fn physical(path: &Utf8Path) -> Result<Utf8PathBuf> {
    Ok(Utf8PathBuf::try_from(crate::path_safety::resolve_path(
        path.as_std_path(),
        true,
    )?)?)
}
fn references(root: &Utf8Path, project: &Utf8Path) -> Result<Utf8PathBuf> {
    let namespace = crate::cas_tree::native(project, "xet-references/v1")?;
    crate::cas_tree::project_target(root, &namespace, true)?;
    crate::cas::checked_directory(&namespace)?;
    Ok(namespace)
}
/// Validate mappings before hooks. Preview never starts/requires the optional helper.
pub(crate) fn configured(
    root: &Utf8Path,
    selections: &[Selection],
    config: &crate::yml::ValidatedConfig,
    ordinary: &[crate::publication::LocalPublication],
    cas: &[crate::cas_remote::Target],
    environment: &crate::run_environment::RunEnvironment,
    preview: bool,
) -> Result<Vec<Target>> {
    let mut result: Vec<Target> = Vec::new();
    let mut engine = None;
    for selection in selections {
        let labels = crate::cas_remote::selected_labels(config, &selection.definition.content)?;
        if labels.is_empty() {
            continue;
        }
        let path = &selection.definition.path;
        let base = physical(&if path.is_absolute() {
            path.clone()
        } else {
            root.join(path)
        })?;
        crate::cas_tree::project_target(root, &base, false)?;
        for label in config.directories.keys() {
            anyhow::ensure!(
                !overlaps(&base, &physical(&config.get_path(root, label)?)?),
                "Xet store overlaps declared resource"
            );
        }
        for target in ordinary {
            anyhow::ensure!(
                !overlaps(&base, &target.base),
                "Xet store overlaps ordinary publication"
            );
        }
        for target in cas {
            anyhow::ensure!(
                !overlaps(&base, &target.base),
                "Xet store overlaps CAS publication"
            );
        }
        for remote in config
            .remotes
            .local
            .iter()
            .flat_map(|values| values.values())
        {
            anyhow::ensure!(
                !overlaps(&base, &physical(&remote.path)?),
                "Xet store overlaps legacy CAS"
            );
        }
        for target in &result {
            anyhow::ensure!(!overlaps(&base, &target.base), "Xet stores overlap");
        }
        let sources = labels
            .into_iter()
            .map(|label| Ok((label.clone(), config.get_path(root, &label)?)))
            .collect::<Result<Vec<_>>>()?;
        crate::cas_tree::preflight_storage(
            root,
            &base,
            &base,
            &sources.iter().map(|(_, p)| p.clone()).collect::<Vec<_>>(),
        )?;
        let project = crate::cas_remote::project_metadata(root, &selection.title);
        crate::cas_tree::preflight_project_reference(root, &project)?;
        references(root, &project)?;
        if !preview {
            crate::xet::preflight(root, &base)?;
            if engine.is_none() {
                engine = Some(crate::xet_engine::Engine::discover(environment)?);
            }
        }
        result.push(Target {
            base,
            project,
            sources,
            engine: engine.clone(),
        });
    }
    Ok(result)
}
/// Recheck sources before the distribution boundary.
pub(crate) fn prepare(root: &Utf8Path, target: &Target) -> Result<()> {
    crate::cas_tree::validate_sources(
        root,
        &target.base,
        &target.base,
        &target
            .sources
            .iter()
            .map(|(_, p)| p.clone())
            .collect::<Vec<_>>(),
    )?;
    crate::xet::preflight(root, &target.base)
}
/// Store finalization/verification and full snapshot precede the small Git root reference.
pub(crate) fn publish(root: &Utf8Path, target: &Target, version: &str) -> Result<String> {
    let engine = target
        .engine
        .as_ref()
        .context("Preview Xet target cannot publish")?;
    let id = crate::xet::publish(root, &target.base, &target.sources, version, engine)?;
    let snapshot = crate::xet::read_snapshot(root, &target.base, &id)?;
    let path = crate::cas_tree::native(
        &references(root, &target.project)?,
        &format!("{}/{}.json", &id[..2], &id[2..]),
    )?;
    crate::cas_tree::project_target(root, &path, true)?;
    let bytes = serde_json::to_vec_pretty(
        &serde_json::json!({"schema":"projcli.xet-project-reference.v1",
        "backend":crate::xet::BACKEND,"upstream":crate::xet::UPSTREAM,"snapshot":id,"roots":snapshot.roots}),
    )?;
    crate::cas::write_metadata(&path, &bytes, true).context(
        "Xet store snapshot published, but project root reference failed; retain store and retry",
    )?;
    Ok(id)
}
