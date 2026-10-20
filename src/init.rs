use std::io::{self, BufRead, IsTerminal, Write};

use std::fs;
use crate::version::version_set;
use crate::ignore::root;

pub fn is_prompt_enabled() -> bool {
    if crate::env::is_non_interactive() {
        return false;
    }
    if !std::io::stdin().is_terminal() {
        return false;
    }
    std::env::var("PROJR_INIT_PROMPT")
        .map(|v| v.to_uppercase() != "FALSE")
        .unwrap_or(true)
}

pub fn ask_yes_no(prompt: &str, default: bool) -> bool {
    if !is_prompt_enabled() {
        return default;
    }
    let default_str = if default { "[Y/n]" } else { "[y/N]" };
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut input = String::new();
    loop {
        print!("{} {} ", prompt, default_str);
        stdout.flush().unwrap();
        input.clear();
        if stdin.lock().read_line(&mut input).is_ok() {
            let trimmed = input.trim().to_lowercase();
            if trimmed.is_empty() {
                return default;
            }
            if trimmed == "y" || trimmed == "yes" {
                return true;
            }
            if trimmed == "n" || trimmed == "no" {
                return false;
            }
        }
        println!("Please enter 'y' or 'n'.");
    }
}

pub fn ask_string(prompt: &str, default: Option<&str>) -> String {
    if !is_prompt_enabled() {
        return default.unwrap_or("").to_string();
    }
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut input = String::new();
    let default_prompt = default.map(|d| format!(" [{}]", d)).unwrap_or_default();
    loop {
        print!("{}{}: ", prompt, default_prompt);
        stdout.flush().unwrap();
        input.clear();
        if stdin.lock().read_line(&mut input).is_ok() {
            let trimmed = input.trim();
            if trimmed.is_empty() {
                if let Some(d) = default {
                    return d.to_string();
                } else {
                    println!("A value is required.");
                    continue;
                }
            }
            return trimmed.to_string();
        }
    }
}

pub fn ask_choice(prompt: &str, choices: &[&str], default: Option<&str>) -> String {
    if !is_prompt_enabled() {
        return default.unwrap_or(choices[0]).to_string();
    }
    let default_prompt = default.map(|d| format!(" [{}]", d)).unwrap_or_default();
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut input = String::new();
    loop {
        print!("{} ({}) {}: ", prompt, choices.join("/"), default_prompt);
        stdout.flush().unwrap();
        input.clear();
        if stdin.lock().read_line(&mut input).is_ok() {
            let trimmed = input.trim().to_lowercase();
            if trimmed.is_empty() {
                if let Some(d) = default {
                    return d.to_string();
                }
            }
            if choices.iter().any(|c| c.to_lowercase() == trimmed) {
                return trimmed;
            }
            println!("Invalid choice. Please select from: {}", choices.join(", "));
        }
    }
}

use anyhow::Context;

pub fn init_version() -> anyhow::Result<()> {
    let project_root = root().context("Failed to find project root.")?;
    let version_file = project_root.join("VERSION");
    let desc_file = project_root.join("DESCRIPTION");

    if !version_file.exists() {
        let mut initial_version = "v0.0.1".to_string();

        if desc_file.exists() {
            if let Ok(content) = fs::read_to_string(desc_file.as_std_path()) {
                for line in content.lines() {
                    if line.starts_with("Version:") {
                        initial_version = line.trim_start_matches("Version:").trim().to_string();
                        if !initial_version.starts_with('v') {
                            initial_version = format!("v{}", initial_version);
                        }
                        break;
                    }
                }
            }
        }

        println!("Initializing VERSION file with {}", initial_version);
        version_set(&initial_version).context("Failed to set initial version.")?;
    } else {
        println!("VERSION file already exists. Skipping.");
    }

    Ok(())
}

/// Initializes only explicitly declared directories; creates a missing base config once.
/// The preset declares projr's raw-data/cache/output paths without a scripts value.
/// Existing base/profile files are preserved, and Git/GitHub setup is not invoked.
pub fn init_directories() -> anyhow::Result<()> {
    let project_root = root().context("Failed to find project root.")?;
    const PRESET: &str = "directories:\n  raw-data:\n    path: _raw_data\n  cache:\n    path: _tmp\n  output:\n    path: _output\n";
    let base = crate::yml::base_configuration_path(&project_root)?;
    let mut active_profile = false;
    for profile in crate::profile::get_active_profiles() {
        active_profile |= crate::yml::configuration_entry_exists(&project_root.join(format!("_projr-{profile}.yml")))?;
    }
    let create_base = base.is_none() && !active_profile;
    let combined = crate::yml::get_combined_yml(None, &project_root)?;
    let combined = if create_base {
        crate::yml::deep_merge(serde_yaml::from_str::<serde_json::Value>(PRESET)?, combined)
    } else { combined };
    let raw: crate::yml::ProjConfig = serde_json::from_value(combined).context("Invalid directory initialization configuration")?;
    let config = raw.validate_and_resolve(&project_root, false)?;
    crate::path_safety::validate_build_paths(&project_root, &config, &[])?;
    for name in [".gitignore", ".Rbuildignore"] {
        let path = project_root.join(name);
        crate::path_safety::resolve_path(path.as_std_path(), true)?;
        if path.exists() { fs::read_to_string(&path).with_context(|| format!("Failed to read existing {name}"))?; }
    }
    for label in config.directories.keys() {
        let path = config.get_path(&project_root, label)?;
        if path.exists() && !path.is_dir() { anyhow::bail!("Declared directory '{label}' is not a directory"); }
    }
    if create_base {
        let mut file = tempfile::NamedTempFile::new_in(project_root.as_std_path())?;
        file.write_all(PRESET.as_bytes())?;
        file.persist_noclobber(project_root.join("_proj.yml")).map_err(|error| error.error)
            .context("Failed to create _proj.yml without overwriting existing configuration")?;
        println!("Created _proj.yml with explicit directory paths.");
    }
    crate::ignore::update_ignores(&project_root, &config)
        .context("Failed to update project ignores during initialization")?;
    let mut labels: Vec<_> = config.directories.keys().collect();
    labels.sort();
    for label in labels {
        let path = config.get_path(&project_root, label)?;
        if !path.exists() {
            fs::create_dir_all(&path).with_context(|| format!("Failed to create declared directory '{label}'"))?;
            println!("Created directory: {path}");
        }
    }
    Ok(())
}

pub fn init_readme(title_opt: Option<String>, description_opt: Option<String>) -> anyhow::Result<()> {
    let project_root = root().context("Failed to find project root.")?;
    let readme_path = project_root.join("README.md");

    if !readme_path.exists() {
        if ask_yes_no("Create a README.md?", true) {
            let title = title_opt.unwrap_or_else(|| ask_string("Project Title", Some("My Project")));
            let description = description_opt.unwrap_or_else(|| ask_string("Project Description", Some("A projr project.")));

            let content = format!("# {}\n\n{}\n", title, description);
            fs::write(readme_path.as_std_path(), content).context("Failed to write README.md.")?;
            println!("Created README.md.");
        }
    } else {
        println!("README.md already exists. Skipping.");
    }

    Ok(())
}

pub fn init_license(license_opt: Option<String>, first_name_opt: Option<String>, last_name_opt: Option<String>) -> anyhow::Result<()> {
    let project_root = root().context("Failed to find project root.")?;
    let license_path = project_root.join("LICENSE");

    if !license_path.exists() {
        if ask_yes_no("Add a LICENSE?", true) {
            let choice = license_opt.unwrap_or_else(|| ask_choice("Choose a license", &["ccby", "apache", "cc0", "proprietary"], Some("ccby")));
            let content = match choice.as_str() {
                "ccby" => "Creative Commons Attribution 4.0 International\n\n[CC-BY 4.0 License Text]\n".to_string(),
                "apache" => "Apache License\nVersion 2.0, January 2004\n\n[Apache License Text]\n".to_string(),
                "cc0" => "Creative Commons Zero v1.0 Universal\n\n[CC0 License Text]\n".to_string(),
                "proprietary" => {
                    let first_name = first_name_opt.unwrap_or_else(|| ask_string("First Name", None));
                    let last_name = last_name_opt.unwrap_or_else(|| ask_string("Last Name", None));
                    let year = chrono::Utc::now().format("%Y").to_string();
                    format!("Copyright (c) {} {} {}. All rights reserved.\n", year, first_name, last_name)
                },
                _ => "".to_string(),
            };

            fs::write(license_path.as_std_path(), content).context("Failed to write LICENSE.")?;
            println!("Created LICENSE ({})", choice);
        }
    } else {
        println!("LICENSE already exists. Skipping.");
    }

    Ok(())
}

use std::process::Command;

pub fn init_git(commit_opt: Option<bool>) -> anyhow::Result<()> {
    let project_root = root().context("Failed to find project root.")?;
    let git_dir = project_root.join(".git");

    let mut is_new = false;
    if !git_dir.exists() {
        if ask_yes_no("Initialize a Git repository?", true) {
            let output = Command::new("git")
                .arg("init")
                .current_dir(project_root.as_std_path())
                .output()
                .context("Failed to execute git init.")?;

            if !output.status.success() {
                anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).into_owned());
            }
            println!("Initialized empty Git repository.");
            is_new = true;
        }
    } else {
        println!("Git repository already initialized.");
        is_new = true; // Offer commit even if it was already initialized, if requested
    }

    let should_commit = commit_opt.unwrap_or_else(|| ask_yes_no("Commit initial changes?", true));
    if is_new && should_commit {
        let add_output = Command::new("git")
            .args(["add", "."])
            .current_dir(project_root.as_std_path())
            .output()
            .context("Failed to execute git add")?;
        anyhow::ensure!(add_output.status.success(), "Failed to stage initial files: {}",
            String::from_utf8_lossy(&add_output.stderr));

        let commit_output = Command::new("git")
            .args(["commit", "-m", "Initial commit from proj init"])
            .current_dir(project_root.as_std_path())
            .output()
            .context("Failed to execute git commit.")?;

        if commit_output.status.success() {
            println!("Initial commit created.");
        } else {
            let err = String::from_utf8_lossy(&commit_output.stderr);
            if err.contains("nothing to commit") || String::from_utf8_lossy(&commit_output.stdout).contains("nothing to commit") {
                println!("Nothing to commit.");
            } else {
                anyhow::bail!("Failed to create initial commit: {}", err);
            }
        }
    }

    Ok(())
}

pub fn init_github(public_opt: Option<bool>) -> anyhow::Result<()> {
    let project_root = root().context("Failed to find project root.")?;

    let remote_output = Command::new("git")
        .arg("remote")
        .current_dir(project_root.as_std_path())
        .output()
        .context("Failed to check git remotes.")?;

    anyhow::ensure!(remote_output.status.success(), "Failed to check git remotes: {}",
        String::from_utf8_lossy(&remote_output.stderr));
    let remotes = String::from_utf8_lossy(&remote_output.stdout);
    if remotes.trim().is_empty() {
        if ask_yes_no("Create a GitHub repository?", true) {
            let is_public = public_opt.unwrap_or_else(|| ask_choice("Visibility", &["public", "private"], Some("private")) == "public");

            let mut args = vec!["repo", "create", "--source=.", "--push"];
            if is_public {
                args.push("--public");
            } else {
                args.push("--private");
            }

            println!("Creating GitHub repository...");
            let gh_output = Command::new("gh")
                .args(&args)
                .current_dir(project_root.as_std_path())
                .output()
                .context("Failed to execute gh repo create.")?;

            if gh_output.status.success() {
                println!("Successfully created and pushed to GitHub repository.");
            } else {
                let err = String::from_utf8_lossy(&gh_output.stderr);
                anyhow::bail!("Failed to create GitHub repository: {}", err);
            }
        }
    } else {
        println!("Git remotes already configured. Skipping GitHub creation.");
    }

    Ok(())
}

pub fn init_full() -> anyhow::Result<()> {
    println!("Starting full proj initialization...");
    init_version()?;
    init_directories()?;
    init_readme(None, None)?;
    init_license(None, None, None)?;
    init_git(None)?;
    init_github(None)?;
    println!("Initialization complete.");
    Ok(())
}
