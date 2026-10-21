//! HTML embedding defaults without overriding renderer-owned explicit options.
use anyhow::Context;

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

/// Add the lowest project-level default; Quarto merges profiles, directory and
/// document metadata natively. Format-specific settings also retain precedence.
/// Call only on the temporarily prepared, recoverable project configuration.
pub(crate) fn project_fallback(metadata: &mut serde_yaml::Value) -> anyhow::Result<()> {
    let mapping = metadata.as_mapping_mut().context("Quarto project metadata must be a mapping")?;
    if !["embed-resources", "self-contained"].iter().any(|key| mapping.contains_key(serde_yaml::Value::from(*key))) {
        mapping.insert(serde_yaml::Value::from("embed-resources"), serde_yaml::Value::Bool(true));
    }
    Ok(())
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
        assert!(!individual_fallback("---\noutput: pdf_document\n---\n",true).unwrap());
        assert!(individual_fallback("---\nformat: html\nembed-sources: false\n---\n",false).unwrap());
        assert!(!individual_fallback("---\nformat: html\nself-contained: false\n---\n",false).unwrap());
    }

    #[test]
    fn project_default_keeps_explicit_top_level_and_format_specific_options() {
        for text in ["format: html\n","embed-resources: false\nformat: html\n","format:\n  html:\n    embed-resources: false\n"] {
            let mut metadata:serde_yaml::Value=serde_yaml::from_str(text).unwrap();
            let original=metadata.clone();
            project_fallback(&mut metadata).unwrap();
            assert_eq!(metadata["format"],original["format"]);
            assert_eq!(metadata["embed-resources"].as_bool(),Some(original.get("embed-resources").and_then(serde_yaml::Value::as_bool).unwrap_or(true)));
        }
    }
}
