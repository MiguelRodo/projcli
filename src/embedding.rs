//! HTML defaults resolved without overriding either native embedding spelling.
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};

/// Whether a supported individual HTML document leaves embedding unspecified.
/// Existing individual-layout validation rejects external/inherited metadata.
pub(crate) fn individual_fallback(content: &str, is_rmd: bool) -> anyhow::Result<bool> {
    let (format, _) = crate::build_pre::individual_output_options(content, is_rmd)?;
    if !(if is_rmd { ["html_document", "html_notebook"].contains(&format.as_str()) }
        else { ["html", "revealjs"].contains(&format.as_str()) }) { return Ok(false); }
    let metadata: serde_yaml::Value = serde_yaml::from_str(
        &crate::build_pre::extract_frontmatter(content).unwrap_or_default())?;
    let key = if is_rmd { "output" } else { "format" };
    let options = metadata.get(key).and_then(|formats| formats.get(format.as_str()));
    let keys: &[&str] = if is_rmd { &["self_contained"] } else { &["embed-resources", "self-contained"] };
    Ok(!keys.iter().any(|key| metadata.get(*key).is_some() || options.and_then(|options| options.get(*key)).is_some()))
}

fn inspect(root: &Utf8Path, input: &Utf8Path, environment: &crate::run_environment::RunEnvironment) -> anyhow::Result<serde_json::Value> {
    let output = environment.command("quarto").current_dir(root).arg("inspect").arg(input).output()
        .context("Cannot inspect effective Quarto metadata")?;
    anyhow::ensure!(output.status.success(), "Quarto metadata inspection failed");
    serde_json::from_slice(&output.stdout).context("Unsupported Quarto inspection JSON")
}

fn project_inputs(root: &Utf8Path, environment: &crate::run_environment::RunEnvironment) -> anyhow::Result<Vec<Utf8PathBuf>> {
    let inspection = inspect(root, root, environment)?;
    let inputs = inspection.get("files").and_then(|files| files.get("input")).and_then(serde_json::Value::as_array)
        .context("Quarto inspection must identify project inputs")?;
    let mut paths = Vec::new();
    for input in inputs {
        let path = Utf8Path::new(input.as_str().context("Quarto input must be a path")?);
        let path = if path.is_absolute() { path.to_owned() } else { root.join(path) };
        let path = Utf8PathBuf::try_from(crate::path_safety::resolve_path(path.as_std_path(), true)?)?;
        anyhow::ensure!(path.starts_with(root) && path.is_file(), "Quarto embedding inputs must be regular project files");
        paths.push(path);
    }
    paths.sort(); paths.dedup();
    Ok(paths)
}

fn unspecified_html(inspection: &serde_json::Value) -> anyhow::Result<Vec<String>> {
    let formats = inspection.get("formats").and_then(serde_json::Value::as_object)
        .context("Quarto document inspection must identify effective formats")?;
    let mut selected = Vec::new();
    for (name, format) in formats {
        if !["html", "revealjs"].contains(&name.as_str()) { continue; }
        anyhow::ensure!(format.get("pandoc").is_some_and(serde_json::Value::is_object), "Unsupported Quarto format inspection");
        let mut specified = false;
        for bucket in ["pandoc", "render", "metadata"] {
            if let Some(options) = format.get(bucket) {
                let options = options.as_object().context("Unsupported Quarto format option inspection")?;
                specified |= ["embed-resources", "self-contained"].iter().any(|key| options.contains_key(*key));
            }
        }
        if !specified { selected.push(name.clone()); }
    }
    Ok(selected)
}

fn with_defaults(content: &str, format_order: &[String], formats: &[String]) -> anyhow::Result<String> {
    let mut metadata: serde_yaml::Value = serde_yaml::from_str(
        &crate::build_pre::extract_frontmatter(content).unwrap_or_default())?;
    if metadata.is_null() { metadata = serde_yaml::Value::Mapping(Default::default()); }
    let mapping = metadata.as_mapping_mut().context("Quarto document metadata must be a mapping")?;
    let key = serde_yaml::Value::from("format");
    let mut configured = match mapping.remove(&key) {
        Some(serde_yaml::Value::Mapping(mapping)) => mapping,
        Some(serde_yaml::Value::String(name)) => serde_yaml::Mapping::from_iter([(serde_yaml::Value::from(name), serde_yaml::Value::Null)]),
        None | Some(serde_yaml::Value::Null) => Default::default(),
        _ => anyhow::bail!("Unsupported Quarto format declaration for embedding fallback"),
    };
    let mut ordered = serde_yaml::Mapping::new();
    for format in format_order {
        let key = serde_yaml::Value::from(format.as_str());
        ordered.insert(key.clone(), configured.remove(&key).unwrap_or_else(|| serde_yaml::Value::Mapping(Default::default())));
    }
    let mut configured = ordered;
    for format in formats {
        let options = configured.entry(serde_yaml::Value::from(format.as_str())).or_insert(serde_yaml::Value::Null);
        if options.is_null() || options.as_str() == Some("default") { *options = serde_yaml::Value::Mapping(Default::default()); }
        let options = options.as_mapping_mut().context("Quarto format options must be a mapping")?;
        options.insert(serde_yaml::Value::from("embed-resources"), serde_yaml::Value::Bool(true));
    }
    mapping.insert(key, serde_yaml::Value::Mapping(configured));
    let body = if content.lines().next().is_some_and(|line| line.trim() == "---") {
        let mut end = 0;
        let mut lines = content.split_inclusive('\n');
        end += lines.next().unwrap().len();
        let mut closed = false;
        for line in lines {
            end += line.len();
            if line.trim() == "---" { closed = true; break; }
        }
        anyhow::ensure!(closed, "Unclosed Quarto document frontmatter");
        &content[end..]
    } else { content };
    Ok(format!("---\n{}---\n{}", serde_yaml::to_string(&metadata)?, body))
}

fn profile_files(root: &Utf8Path) -> anyhow::Result<Vec<Utf8PathBuf>> {
    let mut profiles = vec![root.join("_quarto.yml.local"), root.join("_quarto.yaml.local")];
    for entry in std::fs::read_dir(root)? {
        let path = Utf8PathBuf::try_from(entry?.path())?;
        let name = path.file_name().unwrap_or_default().to_ascii_lowercase();
        if (name == "_quarto.yml" || name == "_quarto.yaml" || name.starts_with("_quarto-"))
            && (name.ends_with(".yml") || name.ends_with(".yaml")) { profiles.push(path); }
    }
    profiles.sort();
    Ok(profiles)
}

fn metadata_bytes(path: &Utf8Path) -> anyhow::Result<Option<Vec<u8>>> {
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            anyhow::ensure!(metadata.is_file(), "Quarto inherited metadata must be a regular file: {path}");
            Ok(Some(std::fs::read(path)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn include_paths(owner: &Utf8Path, value: &serde_yaml::Value, queue: &mut Vec<Utf8PathBuf>) -> anyhow::Result<()> {
    for key in ["metadata-file", "metadata-files"] {
        if let Some(includes) = value.get(key) {
            if includes.is_null() { continue; }
            let values = if let Some(values) = includes.as_sequence() { values.iter().collect::<Vec<_>>() }
                else { vec![includes] };
            for value in values {
                let path = Utf8Path::new(value.as_str().context("Embedding preparation requires literal metadata-file/metadata-files paths")?);
                queue.push(if path.is_absolute() { path.to_owned() } else { owner.parent().context("Metadata has no parent")?.join(path) });
            }
        }
    }
    Ok(())
}

fn inherited_metadata(root: &Utf8Path, edits: &[(Utf8PathBuf, String, String)]) -> anyhow::Result<(Vec<Utf8PathBuf>, std::collections::BTreeMap<Utf8PathBuf, Option<Vec<u8>>>)> {
    if edits.is_empty() { return Ok((Vec::new(), Default::default())); }
    let profiles = profile_files(root)?;
    let mut queue = profiles.clone();
    for (input, original, _) in edits {
        let metadata: serde_yaml::Value = serde_yaml::from_str(&crate::build_pre::extract_frontmatter(original).unwrap_or_default())?;
        include_paths(input, &metadata, &mut queue)?;
        let mut parent = input.parent();
        while let Some(directory) = parent {
            queue.extend([directory.join("_metadata.yml"), directory.join("_metadata.yaml")]);
            if directory == root { break; }
            parent = directory.parent();
        }
    }
    let mut snapshots = std::collections::BTreeMap::new();
    while let Some(path) = queue.pop() {
        let path = Utf8PathBuf::try_from(crate::path_safety::resolve_path(path.as_std_path(), true)?)?;
        anyhow::ensure!(!path.components().any(|part| part.as_str().eq_ignore_ascii_case(".git")), "Quarto inherited metadata in VCS internals is unsupported");
        if snapshots.contains_key(&path) { continue; }
        let bytes = metadata_bytes(&path)?;
        if let Some(bytes) = &bytes {
            let metadata: serde_yaml::Value = serde_yaml::from_slice(bytes)?;
            include_paths(&path, &metadata, &mut queue)?;
        }
        snapshots.insert(path, bytes);
    }
    Ok((profiles, snapshots))
}

/// Exact-byte temporary header preparation, using native effective metadata.
/// No global default is injected: legacy and canonical false can coexist across
/// documents, formats, profiles, directory metadata and included metadata files.
pub(crate) struct ProjectEmbedding {
    inputs: Vec<Utf8PathBuf>,
    edits: Vec<(Utf8PathBuf, String, String)>,
    backup: crate::build_state::BuildState,
    inherited: std::collections::BTreeMap<Utf8PathBuf, Option<Vec<u8>>>,
    profiles: Vec<Utf8PathBuf>,
}

impl ProjectEmbedding {
    /// Inspect and capture before any source edit; the caller retains this guard
    /// across preparation/rendering and restores it on success, error or panic.
    pub(crate) fn capture(root: &Utf8Path, environment: &crate::run_environment::RunEnvironment, caches: &[std::path::PathBuf]) -> anyhow::Result<Self> {
        let inputs = project_inputs(root, environment)?;
        let mut edits = Vec::new();
        for input in &inputs {
            let inspection = inspect(root, input, environment)?;
            let formats = unspecified_html(&inspection)?;
            let format_order = inspection["formats"].as_object().unwrap().keys().cloned().collect::<Vec<_>>();
            if formats.is_empty() { continue; }
            anyhow::ensure!(matches!(input.extension(), Some("qmd" | "md" | "Rmd" | "rmd")),
                "Quarto HTML fallback requires a modeled text document; configure embedding explicitly for other input types");
            let original = crate::fs_utils::read_managed_text(input)?;
            let prepared = with_defaults(&original, &format_order, &formats)?;
            edits.push((input.clone(), original, prepared));
        }
        let (profiles, inherited) = inherited_metadata(root, &edits)?;
        let mut paths = edits.iter().map(|(path, _, _)| path.clone().into_std_path_buf()).collect::<Vec<_>>();
        paths.extend(inherited.keys().filter(|path| path.starts_with(root)).map(|path| path.clone().into_std_path_buf()));
        let backup = crate::build_state::BuildState::capture(caches, &paths)?;
        Ok(Self { inputs, edits, backup, inherited, profiles })
    }

    /// Apply only captured edits; a changed or substituted source is rejected.
    pub(crate) fn apply(&self) -> anyhow::Result<()> {
        for (path, original, prepared) in &self.edits {
            anyhow::ensure!(crate::fs_utils::read_managed_text(path)? == *original, "Quarto source changed during embedding preparation");
            crate::fs_utils::replace_managed_file(path, prepared)?;
        }
        Ok(())
    }

    /// Recheck inherited metadata bytes, input set and prepared headers at launch.
    /// Changing a prepared document after preparation is unsupported.
    pub(crate) fn verify(&self, root: &Utf8Path, environment: &crate::run_environment::RunEnvironment) -> anyhow::Result<()> {
        if !self.edits.is_empty() {
            anyhow::ensure!(profile_files(root)? == self.profiles, "Quarto inherited metadata inventory changed after embedding preparation");
            for (path, expected) in &self.inherited {
                anyhow::ensure!(metadata_bytes(path)? == *expected, "Quarto inherited metadata changed after embedding preparation: {path}");
            }
        }
        anyhow::ensure!(project_inputs(root, environment)? == self.inputs, "Quarto inputs changed after embedding preparation");
        for (path, _, prepared) in &self.edits {
            anyhow::ensure!(crate::fs_utils::read_managed_text(path)? == *prepared, "Quarto source changed after embedding preparation");
        }
        Ok(())
    }

    /// Restore original source bytes on all outcomes, propagating recovery errors.
    pub(crate) fn restore_result(self, result: anyhow::Result<()>) -> anyhow::Result<()> {
        self.backup.restore_result(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn individual_defaults_only_apply_to_unspecified_html_options() {
        for (rmd, format, option) in [(false,"html","embed-resources"),(true,"html_document","self_contained")] {
            let key=if rmd {"output"} else {"format"};
            assert!(individual_fallback(&format!("---\n{key}: {format}\n---\n"),rmd).unwrap());
            for value in ["true","false"] {
                assert!(!individual_fallback(&format!("---\n{key}:\n  {format}:\n    {option}: {value}\n---\n"),rmd).unwrap());
            }
        }
        assert!(!individual_fallback("---\nformat: pdf\n---\n",false).unwrap());
        assert!(individual_fallback("---\nformat: html\nembed-sources: false\n---\n",false).unwrap());
        assert!(!individual_fallback("---\nformat: html\nself-contained: false\n---\n",false).unwrap());
    }
    #[test]
    fn native_effective_presence_respects_both_spellings_and_non_html_formats() {
        for key in ["embed-resources","self-contained"] {
            for value in [false,true] {
                let inspection = serde_json::json!({"formats":{"html":{"pandoc":{(key):value}},"pdf":{"pandoc":{}}}});
                assert!(unspecified_html(&inspection).unwrap().is_empty());
            }
        }
        assert_eq!(unspecified_html(&serde_json::json!({"formats":{"html":{"pandoc":{}},"pdf":{"pandoc":{}}}})).unwrap(),["html"]);
    }
    #[test]
    fn inherited_format_choices_and_order_are_retained_when_only_html_needs_fallback() {
        let prepared = with_defaults("---\ntitle: mixed formats\n---\nBody\n",
            &["docx".into(),"html".into()], &["html".into()]).unwrap();
        let metadata:serde_yaml::Value = serde_yaml::from_str(&crate::build_pre::extract_frontmatter(&prepared).unwrap()).unwrap();
        let order = metadata["format"].as_mapping().unwrap().keys().map(|key| key.as_str().unwrap()).collect::<Vec<_>>();
        assert_eq!(order,["docx","html"]);
        assert_eq!(metadata["format"]["html"]["embed-resources"],true);
        assert!(metadata["format"]["docx"].as_mapping().unwrap().is_empty());
    }
    #[test]
    fn temporary_headers_keep_body_and_other_format_options() {
        let content = "---\r\nformat:\r\n  html:\r\n    toc: true\r\n  pdf: default\r\n---\r\n![literal](../image.png)\r\n";
        let prepared = with_defaults(content, &["html".into(),"pdf".into()], &["html".into()]).unwrap();
        assert!(prepared.ends_with("![literal](../image.png)\r\n"));
        let metadata:serde_yaml::Value = serde_yaml::from_str(&crate::build_pre::extract_frontmatter(&prepared).unwrap()).unwrap();
        assert_eq!(metadata["format"]["html"]["toc"],true);
        assert_eq!(metadata["format"]["html"]["embed-resources"],true);
        assert_eq!(metadata["format"]["pdf"],"default");
    }
}
