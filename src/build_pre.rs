use crate::run_environment::RunEnvironment;
use std::io::{self, BufRead, IsTerminal};
use anyhow::Context;
use crate::git::{get_github_token_in, execute_authenticated_git_in, create_git_provider_in};
use crate::yml::GlobalConfig;

pub fn pre_flight_git_check(config: &GlobalConfig, repo_dir: &camino::Utf8Path) -> anyhow::Result<()> {
    pre_flight_git_check_in(config, repo_dir, &RunEnvironment::inherited())
}

fn pre_flight_git_check_in(config: &GlobalConfig, repo_dir: &camino::Utf8Path, environment: &RunEnvironment) -> anyhow::Result<()> {
    let provider = create_git_provider_in(config.git.engine, repo_dir.to_path_buf(), environment)?;

    // Check if user has context configurations mapped out
    let name = provider.get_user_name().unwrap_or_else(|| "Unknown".to_string());
    let email = provider.get_user_email().unwrap_or_else(|| "unknown@example.com".to_string());

    println!("Git context verified successfully for R workflow: {} <{}>", name, email);

    if provider.is_behind_remote("origin", "main").unwrap_or(false) {
        println!("⚠️ Warning: Current local HEAD is behind origin/main.");
    }

    Ok(())
}

/// Extracts the YAML frontmatter sequence from the beginning of a document.
pub fn extract_frontmatter(content: &str) -> Option<String> {
    let mut lines = content.lines();
    if let Some(first) = lines.next() {
        if first.trim() != "---" {
            return None;
        }
    } else {
        return None;
    }

    let mut frontmatter = String::new();
    for line in lines {
        if line.trim() == "---" {
            return Some(frontmatter);
        }
        frontmatter.push_str(line);
        frontmatter.push('\n');
    }
    None
}

/// Parses the frontmatter to extract the target format and output-file keys.
pub fn parse_frontmatter_options(content: &str) -> (Option<String>, Option<String>) {
    let mut format = None;
    let mut output_file = None;

    if let Some(fm) = extract_frontmatter(content) {
        if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&fm) {
            if let Some(map) = yaml.as_mapping() {
                if let Some(f) = map.get("format").and_then(|v| v.as_str()) {
                    format = Some(f.to_string());
                } else if let Some(o) = map.get("output") {
                    if let Some(s) = o.as_str() {
                        format = Some(s.to_string());
                    } else if let Some(omap) = o.as_mapping() {
                        if let Some(k) = omap.keys().next() {
                            if let Some(s) = k.as_str() {
                                format = Some(s.to_string());
                            }
                        }
                    }
                }

                if let Some(of) = map.get("output-file").or_else(|| map.get("output_file")) {
                    if let Some(s) = of.as_str() {
                        output_file = Some(s.to_string());
                    }
                }
            }
        }
    }

    (format, output_file)
}

/// Maps a document format to its output file extension.
pub fn map_format_to_extension(format: Option<&str>) -> anyhow::Result<String> {
    let extension = match format.unwrap_or("html") {
        "html" | "html_document" | "revealjs" => "html",
        "html_notebook" => "nb.html",
        "word_document" | "docx" => "docx",
        "pdf" | "pdf_document" | "beamer" | "beamer_presentation" | "typst" => "pdf",
        "pptx" => "pptx",
        "epub" => "epub",
        _ => anyhow::bail!("Unsupported individual document format; supported layouts are HTML, PDF, Word, PowerPoint and EPUB"),
    };
    Ok(extension.to_owned())
}

/// Model one document-local format. External metadata and multiple formats need a separate plan.
pub(crate) fn individual_output_options(content: &str, is_rmd: bool) -> anyhow::Result<(String, Option<String>)> {
    let mut lines = content.lines();
    let metadata = if lines.next().is_some_and(|line| line.trim() == "---") {
        let mut header = String::new();
        let mut closed = false;
        for line in lines {
            if line.trim() == "---" { closed = true; break; }
            header.push_str(line); header.push('\n');
        }
        if !closed { anyhow::bail!("Document YAML frontmatter is not closed"); }
        let value: serde_yaml::Value = serde_yaml::from_str(&header).context("Invalid document YAML frontmatter")?;
        if value.is_null() { serde_yaml::Value::Mapping(Default::default()) } else { value }
    } else { serde_yaml::Value::Mapping(Default::default()) };
    let mapping = metadata.as_mapping().context("Document YAML frontmatter must be a mapping")?;
    let get = |key: &str| mapping.get(serde_yaml::Value::String(key.to_owned()));
    for key in ["output-dir", "output_dir", "metadata-files", "output_yaml", "output-yaml"] {
        if get(key).is_some() { anyhow::bail!("Individual rendering does not support external metadata or output-directory overrides"); }
    }
    if is_rmd && get("runtime").is_some_and(|value| !matches!(value.as_str(), Some("auto" | "static"))) {
        anyhow::bail!("Only static individual R Markdown runtime layouts are supported");
    }
    let key = if is_rmd { "output" } else { "format" };
    let format = match get(key) {
        None => if is_rmd { "html_document" } else { "html" }.to_owned(),
        Some(serde_yaml::Value::String(name)) => name.clone(),
        Some(serde_yaml::Value::Mapping(formats)) if formats.len() == 1 => {
            let (name, options) = formats.iter().next().unwrap();
            if let Some(options) = options.as_mapping() {
                if ["output-file", "output_file", "output-dir", "output_dir"].iter().any(|key| options.contains_key(serde_yaml::Value::String((*key).to_owned()))) {
                    anyhow::bail!("Format-specific output overrides are unsupported for individual rendering");
                }
            } else if !options.is_null() && options.as_str() != Some("default") {
                anyhow::bail!("Individual format options must be a mapping or default");
            }
            name.as_str().context("Individual format name must be a string")?.to_owned()
        }
        _ => anyhow::bail!("Individual rendering requires exactly one named output format"),
    };
    let supported = if is_rmd {
        ["html_document", "html_notebook", "pdf_document", "word_document", "beamer_presentation"].contains(&format.as_str())
    } else { ["html", "pdf", "docx", "typst", "beamer", "revealjs", "pptx", "epub"].contains(&format.as_str()) };
    if !supported { anyhow::bail!("Unsupported individual document format for this renderer"); }
    let first = get("output-file"); let second = get("output_file");
    if first.is_some() && second.is_some() { anyhow::bail!("Document output filename aliases are ambiguous"); }
    if is_rmd && (first.is_some() || second.is_some()) {
        anyhow::bail!("Individual R Markdown output filename overrides are unsupported; use the input-basename layout");
    }
    let output = first.or(second).map(|value| value.as_str().context("Document output filename must be a string").map(str::to_owned)).transpose()?;
    Ok((format, output))
}

pub fn find_python_command() -> Option<String> {
    find_python_command_in(&RunEnvironment::inherited())
}

fn find_python_command_in(environment: &RunEnvironment) -> Option<String> {
    let variants = ["python3", "python"];
    for cmd in variants {
        if let Ok(output) = environment.command(cmd).arg("--version").output() {
            if output.status.success() {
                return Some(cmd.to_string());
            }
        }
    }
    None
}

use crate::yml::ValidatedConfig;

pub fn run_pre_flight_checks(
    project_root: &camino::Utf8Path,
    config: &ValidatedConfig,
    is_prod_run: bool,
    resolved_files: &[camino::Utf8PathBuf],
    quarto_exists: bool,
    bookdown_exists: bool,
) -> anyhow::Result<(Option<String>, Option<String>)> {
    run_pre_flight_checks_in(project_root, config, is_prod_run, resolved_files, quarto_exists, bookdown_exists, &RunEnvironment::inherited())
}

pub(crate) fn run_pre_flight_checks_in(
    project_root: &camino::Utf8Path,
    config: &ValidatedConfig,
    is_prod_run: bool,
    resolved_files: &[camino::Utf8PathBuf],
    quarto_exists: bool,
    bookdown_exists: bool,
    environment: &RunEnvironment,
) -> anyhow::Result<(Option<String>, Option<String>)> {
    let mut resolved_token = None;
    let needs_remote = config.git.push || (is_prod_run && config.restrictions.not_behind == Some(true)) || (is_prod_run && config.restrictions.not_behind.is_none());

    if needs_remote && config.config.git.use_proj_cred_helper {
        let token = get_github_token_in(environment).context("Failed to retrieve GitHub token")?;
        resolved_token = Some(token);
    }

    if is_prod_run {
        let is_git_repo = project_root.join(".git").exists();

        // Run the new pre_flight_git_check if it's a git repo and git is configured
        if is_git_repo {
            pre_flight_git_check_in(&config.config, project_root, environment)?;
        }

        let mut current_branch = None;
        if is_git_repo {
            let mut cmd = environment.command("git");
            cmd.args(["symbolic-ref", "--short", "HEAD"]);
            cmd.current_dir(project_root);
            if let Ok(output) = cmd.output() {
                if output.status.success() {
                    current_branch = Some(String::from_utf8_lossy(&output.stdout).trim().to_string());
                }
            }
        }

        // Branch constraints
        if let Some(only_branches) = &config.restrictions.only_branches {
            if !is_git_repo {
                anyhow::bail!("Output builds are restricted to specific branches, but the project is not inside a Git repository");
            }
            if let Some(branch) = &current_branch {
                if !only_branches.contains(branch) {
                    anyhow::bail!("Output builds are restricted to branches {:?} as per build.restrictions.only_branches, but current branch is '{}'", only_branches, branch);
                }
            } else {
                anyhow::bail!("Output builds are restricted to specific branches, but could not determine current branch");
            }
        }

        if let Some(not_branches) = &config.restrictions.not_branches {
            if let Some(branch) = &current_branch {
                if not_branches.contains(branch) {
                    anyhow::bail!("Output builds are restricted on branch '{}' as per build.restrictions.not_branches", branch);
                }
            }
        }

        // Upstream synchronicity
        let not_behind = config.restrictions.not_behind;
        if not_behind != Some(false) {
            if not_behind == Some(true) && (!is_git_repo || !has_tracking_remote(project_root, environment)) {
                anyhow::bail!("build.restrictions.not_behind is explicitly set to true, but no Git remote is configured");
            }

            if is_git_repo {
                if !has_tracking_remote(project_root, environment) {
                    if not_behind.is_none() {
                        anyhow::bail!("Git repository detected but no upstream tracking remote is configured. Configure a remote or set build.restrictions.not_behind to false");
                    }
                } else {
                    if is_behind_remote(project_root, resolved_token.as_deref(), environment)? {
                        anyhow::bail!("The local branch is behind its tracking remote. Please pull or merge changes before building");
                    }
                }
            }
        }
    }

    let mut r_needed = false;
    let mut quarto_needed = quarto_exists;
    let mut python_needed = false;
    let mut rmd_present = false;
    if bookdown_exists {
        r_needed = true;
    }

    for file in resolved_files {
        let ext = file.extension().unwrap_or("");
        match ext {
            "Rmd" | "rmd" => {
                r_needed = true;
                rmd_present = true;
            }
            "qmd" => {
                quarto_needed = true;
            }
            "R" | "r" => {
                r_needed = true;
            }
            "py" => {
                python_needed = true;
            }
            _ => {}
        }
    }

    // A. R Validation
    if r_needed {
        if environment.command("Rscript").arg("--version").output().is_err() {
            anyhow::bail!("'Rscript' executable not found on the system PATH. A working R installation is required to build this project");
        }

        let mut required_packages = Vec::new();
        if bookdown_exists {
            required_packages.push("bookdown");
        } else if rmd_present {
            required_packages.push("rmarkdown");
        }

        for package_name in required_packages {
            let mut probe = environment.command("Rscript");
            probe.args(&[
                "-e",
                &format!("if (!requireNamespace('{}', quietly = TRUE)) q(status = 1)", package_name),
            ]);
            if environment.non_interactive() {
                probe.stdin(std::process::Stdio::null());
            }
            let status = probe.status();

            let is_installed = status.map(|s| s.success()).unwrap_or(false);

            if !is_installed {
                let auto_install = environment.text("PROJR_AUTO_INSTALL").unwrap_or_default();
                let should_install = if environment.non_interactive() {
                    false
                } else if auto_install == "true" {
                    true
                } else if auto_install == "false" {
                    false
                } else {
                    let is_terminal = io::stdin().is_terminal();
                    if is_terminal {
                        let mut input = String::new();
                        let stdin = io::stdin();
                        loop {
                            println!("Required R package '{}' is not installed. Would you like to install it now? (y/n): ", package_name);
                            input.clear();
                            match stdin.lock().read_line(&mut input) {
                                Ok(0) => break false, // EOF
                                Ok(_) => {
                                    let trimmed = input.trim().to_lowercase();
                                    if trimmed == "y" || trimmed == "yes" {
                                        break true;
                                    } else if trimmed == "n" || trimmed == "no" {
                                        break false;
                                    }
                                }
                                Err(_) => break false,
                            }
                        }
                    } else {
                        false
                    }
                };

                if should_install {
                    let install_status = environment.command("Rscript")
                        .args(&[
                            "-e",
                            &format!("install.packages('{}', repos='https://cloud.r-project.org')", package_name),
                        ])
                        .status()
                        .context(format!("Failed to execute Rscript to install package '{}'", package_name))?;

                    if !install_status.success() {
                        anyhow::bail!("Failed to install R package '{}'", package_name);
                    }
                } else {
                    anyhow::bail!(
                        "Required R package '{}' is not installed.\n\
                        To install this dependency programmatically, run:\n    \
                        Rscript -e \"install.packages('{}', repos='https://cloud.r-project.org')\"",
                        package_name, package_name
                    );
                }
            }
        }
    }

    // B. Quarto Validation
    if quarto_needed {
        if environment.command("quarto").arg("--version").output().is_err() {
            anyhow::bail!("'quarto' binary not found on the system PATH. \n\
                        Please download and install Quarto before proceeding: https://quarto.org/docs/get-started/");
        }
    }

    // C. Python Validation
    let mut resolved_python_cmd = None;
    if python_needed {
        match find_python_command_in(environment) {
            Some(cmd) => {
                resolved_python_cmd = Some(cmd);
            }
            None => {
                anyhow::bail!("Python interpreter not found on the system PATH. \
                            Please ensure either 'python3' or 'python' is installed and accessible");
            }
        }
    }

    Ok((resolved_token, resolved_python_cmd))
}

fn has_tracking_remote(project_root: &camino::Utf8Path, environment: &RunEnvironment) -> bool {
    let mut cmd = environment.command("git");
    cmd.args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]);
    cmd.current_dir(project_root);
    if let Ok(output) = cmd.output() {
        output.status.success() && !String::from_utf8_lossy(&output.stdout).trim().is_empty()
    } else {
        false
    }
}

fn is_behind_remote(project_root: &camino::Utf8Path, token: Option<&str>, environment: &RunEnvironment) -> anyhow::Result<bool> {
    // Perform fetch
    if let Some(t) = token {
        execute_authenticated_git_in(&["fetch"], t, Some(project_root), environment).context("Failed to perform authenticated git fetch")?;
    } else {
        let mut fetch_cmd = environment.command("git");
        fetch_cmd.args(["fetch"]);
        fetch_cmd.current_dir(project_root);
        let fetch_out = fetch_cmd.output().context("Failed to execute git fetch command")?;
        if !fetch_out.status.success() {
            anyhow::bail!("Failed to fetch from remote: {}", String::from_utf8_lossy(&fetch_out.stderr));
        }
    }

    // Check rev-list --count HEAD..@{u}
    let mut cmd = environment.command("git");
    cmd.args(["rev-list", "--count", "HEAD..@{u}"]);
    cmd.current_dir(project_root);
    let output = cmd.output().context("Failed to execute git rev-list command")?;

    if output.status.success() {
        let count_str = String::from_utf8_lossy(&output.stdout);
        if let Ok(count) = count_str.trim().parse::<u32>() {
            return Ok(count > 0);
        }
    }

    // If the above fails (e.g., no upstream configured, though we check it prior), assume not behind or return error
    anyhow::bail!("Failed to determine if the local branch is behind the remote");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_frontmatter() {
        let content = "---\ntitle: abc\nformat: html_notebook\n---\nBody";
        assert_eq!(extract_frontmatter(content).unwrap(), "title: abc\nformat: html_notebook\n");

        let content2 = "No frontmatter";
        assert!(extract_frontmatter(content2).is_none());
    }

    #[test]
    fn test_parse_frontmatter_options() {
        let content = "---\ntitle: abc\nformat: html_notebook\noutput-file: out.html\n---\nBody";
        let (fmt, of) = parse_frontmatter_options(content);
        assert_eq!(fmt, Some("html_notebook".to_string()));
        assert_eq!(of, Some("out.html".to_string()));

        let content_rmd = "---\noutput:\n  word_document: default\noutput_file: test.docx\n---\nBody";
        let (fmt, of) = parse_frontmatter_options(content_rmd);
        assert_eq!(fmt, Some("word_document".to_string()));
        assert_eq!(of, Some("test.docx".to_string()));
    }
}
