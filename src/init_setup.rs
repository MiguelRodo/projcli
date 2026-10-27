//! Create missing project metadata. Existing DESCRIPTION Version alone follows sync policy; no package scaffold or dependency installation.
use anyhow::Context;
use camino::Utf8Path;
use serde::Serialize;
use std::{
    fs,
    io::{self, Write},
    process::Command,
};

/// General-init choices. Optional destructive/external actions are never inferred in noninteractive calls.
#[derive(clap::Args, Clone, Debug, Default, PartialEq, Eq)]
pub struct InitOptions {
    /// Create DESCRIPTION only when explicitly requested; never create R/ or NAMESPACE.
    #[arg(long = "description-file")]
    pub desc: bool,
    /// Choose README.Rmd instead of the default README.md; no rendering at init.
    #[arg(long)]
    pub readme_rmd: bool,
    /// Explicitly call the separate installed-renv setup action.
    #[arg(long)]
    pub renv: bool,
    /// Create a generic CITATION.cff with explicitly supplied authors.
    #[arg(long)]
    pub citation: bool,
    /// Project title; defaults to the selected folder name.
    #[arg(long)]
    pub title: Option<String>,
    /// Project purpose paragraph, placed near the top of the README.
    #[arg(long)]
    pub purpose: Option<String>,
    /// Project contact name; not automatically a citation author.
    #[arg(long)]
    pub contact_name: Option<String>,
    /// Project contact email; not automatically a citation author.
    #[arg(long)]
    pub contact_email: Option<String>,
    /// Optional root code licence: ccby, apache, cc0, proprietary, or none.
    #[arg(long)]
    pub code_license: Option<String>,
    /// Explicit copyright holder for a proprietary code licence.
    #[arg(long)]
    pub copyright_holder: Option<String>,
    /// Citation author's family names; repeat in the intended author order.
    #[arg(long)]
    pub author_family: Vec<String>,
    /// Citation author's given names; omit all or supply one per family-name entry.
    #[arg(long)]
    pub author_given: Vec<String>,
    /// Explicit team/organisation citation author names, after person authors.
    #[arg(long)]
    pub author_name: Vec<String>,
    /// Initialize Git if needed. Omitted noninteractive calls skip it.
    #[arg(long,num_args=0..=1,require_equals=true,default_missing_value="true")]
    pub git: Option<bool>,
    /// Offer GitHub only as a route when no Git remote exists.
    #[arg(long,num_args=0..=1,require_equals=true,default_missing_value="true")]
    pub github: Option<bool>,
}

/// Read-only state needed by interactive frontends, without remote URLs or personal metadata.
#[derive(Serialize)]
pub struct InitStatus {
    /// Whether the native Git command is available.
    pub git_available: bool,
    /// Includes enclosing repositories and linked Git worktrees.
    pub git_repository: bool,
    /// Any configured remote suffices, regardless of hosting provider.
    pub remote: bool,
    /// Existing README.md.
    pub readme_md: bool,
    /// Existing README.Rmd.
    pub readme_rmd: bool,
    /// Existing DESCRIPTION.
    pub description: bool,
    /// Existing citation file.
    pub citation: bool,
}
fn git(root: &Utf8Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(root);
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(key);
    }
    command
}
/// Inspect setup without creating files or contacting a server. Fail on broken existing Git state.
pub fn inspect(root: &Utf8Path) -> anyhow::Result<InitStatus> {
    crate::path_safety::resolve_path(root.as_std_path(), true)?;
    let available = git(root)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let mut repository = false;
    let mut remote = false;
    if available {
        let output = git(root)
            .args(["rev-parse", "--is-inside-work-tree"])
            .output()?;
        repository = output.status.success() && output.stdout.starts_with(b"true");
        if !repository {
            let has_marker = root.ancestors().any(|path| path.join(".git").exists());
            anyhow::ensure!(
                !has_marker,
                "Cannot inspect the existing Git repository; setup stopped"
            );
            let bare = git(root)
                .args(["rev-parse", "--is-bare-repository"])
                .output()?;
            anyhow::ensure!(
                !(bare.status.success() && bare.stdout.starts_with(b"true")),
                "Cannot initialize project files inside a bare Git repository"
            );
        } else {
            let output = git(root).arg("remote").output()?;
            anyhow::ensure!(
                output.status.success(),
                "Cannot inspect existing Git remotes"
            );
            remote = !output.stdout.iter().all(u8::is_ascii_whitespace);
        }
    }
    Ok(InitStatus {
        git_available: available,
        git_repository: repository,
        remote,
        readme_md: root.join("README.md").exists(),
        readme_rmd: root.join("README.Rmd").exists(),
        description: root.join("DESCRIPTION").exists(),
        citation: root.join("CITATION.cff").exists(),
    })
}
fn optional(prompt: &str, default: Option<&str>) -> anyhow::Result<Option<String>> {
    if !crate::init::is_prompt_enabled() {
        return Ok(default.map(str::to_owned));
    }
    print!(
        "{prompt}{}: ",
        default.map(|s| format!(" [{s}]")).unwrap_or_default()
    );
    io::stdout().flush()?;
    let mut value = String::new();
    anyhow::ensure!(
        io::stdin().read_line(&mut value)? > 0,
        "Initialization cancelled at end of input"
    );
    let value = value.trim();
    Ok(if value.is_empty() {
        default.map(str::to_owned)
    } else {
        Some(value.to_owned())
    })
}
fn yes(prompt: &str) -> anyhow::Result<bool> {
    if !crate::init::is_prompt_enabled() {
        return Ok(false);
    }
    loop {
        match optional(&format!("{prompt} [y/N]"), Some("n"))?.as_deref() {
            Some("y" | "Y" | "yes" | "Yes") => return Ok(true),
            Some("n" | "N" | "no" | "No") => return Ok(false),
            _ => eprintln!("Please enter yes or no."),
        }
    }
}
fn single(value: &str, field: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !value.trim().is_empty() && !value.chars().any(char::is_control),
        "{field} must be nonempty single-line text"
    );
    Ok(())
}
/// Create one guarded metadata file with no overwrite, including during concurrent creation.
pub(crate) fn create(root: &Utf8Path, name: &str, bytes: &[u8]) -> anyhow::Result<bool> {
    let path = root.join(name);
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_file(),
                "Existing metadata is not a regular file: {path}"
            );
            return Ok(false);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut temp = tempfile::NamedTempFile::new_in(root)?;
    temp.write_all(bytes)?;
    temp.flush()?;
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    temp.persist_noclobber(&path)
        .map_err(|error| error.error)
        .context("Cannot create metadata without overwriting an existing file")?;
    Ok(true)
}
fn licence_body(choice: &str, holder: Option<&str>) -> anyhow::Result<Option<String>> {
    let text = match choice.to_ascii_lowercase().as_str() {
        "none" | "later" => return Ok(None),
        "ccby" | "cc-by" => include_str!("../templates/licenses/CC-BY-4.0.txt").to_owned(),
        "apache" | "apache-2.0" | "apache 2.0" => {
            include_str!("../templates/licenses/Apache-2.0.txt").to_owned()
        }
        "cc0" => include_str!("../templates/licenses/CC0-1.0.txt").to_owned(),
        "proprietary" => {
            let holder =
                holder.context("A proprietary licence requires an explicit copyright holder")?;
            single(holder, "Copyright holder")?;
            format!(
                "Copyright (c) {} {holder}. All rights reserved.\n",
                chrono::Utc::now().format("%Y")
            )
        }
        _ => anyhow::bail!("Code licence must be ccby, apache, cc0, proprietary or none"),
    };
    Ok(Some(text))
}
/// Root init licenses source code only, independently of data and third-party material.
pub(crate) fn licence(choice: &str, holder: Option<&str>) -> anyhow::Result<Option<String>> {
    Ok(licence_body(choice,holder)?.map(|text|format!(
        "Scope: this licence applies to the project's source code. Input data and third-party materials are not licensed by this notice.\n\n{text}")))
}
/// Explicit label licensing concerns only the selected directory.
pub(crate) fn directory_licence(
    choice: &str,
    holder: Option<&str>,
) -> anyhow::Result<Option<String>> {
    Ok(licence_body(choice,holder)?.map(|text|format!(
        "Scope: this licence applies only to material in this directory. Third-party rights are unchanged.\n\n{text}")))
}
fn dcf(field: &str, text: &str) -> String {
    let lines = text
        .lines()
        .map(|line| if line.is_empty() { "." } else { line })
        .collect::<Vec<_>>();
    format!("{field}: {}\n", lines.join("\n    "))
}

/// Initialize Git only when explicitly requested after state inspection; never commit user files.
pub(crate) fn create_repository(root: &Utf8Path) -> anyhow::Result<()> {
    if inspect(root)?.git_repository {
        return Ok(());
    }
    let output = git(root)
        .arg("init")
        .output()
        .context("Git is required for the selected setup action")?;
    anyhow::ensure!(output.status.success(), "Git initialization failed");
    Ok(())
}
/// Create an optional private GitHub remote with the already installed native gh command.
/// General init does not push or make a commit; existing remotes are never replaced.
pub(crate) fn create_github(root: &Utf8Path) -> anyhow::Result<()> {
    let state = inspect(root)?;
    if state.remote {
        return Ok(());
    }
    anyhow::ensure!(
        state.git_repository,
        "GitHub setup requires an existing Git repository or --git=true"
    );
    let mut command = Command::new("gh");
    command.current_dir(root).env_remove("GH_REPO");
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(key);
    }
    let output = command
        .args(["repo", "create", "--source=.", "--private"])
        .output()
        .context(
            "GitHub setup requires the existing gh command and its configured authentication",
        )?;
    anyhow::ensure!(
        output.status.success(),
        "GitHub setup failed; inspect native gh authentication/repository state before retrying"
    );
    Ok(())
}
fn authors(options: &mut InitOptions) -> anyhow::Result<Vec<serde_json::Value>> {
    if options.author_family.is_empty()
        && options.author_name.is_empty()
        && crate::init::is_prompt_enabled()
    {
        loop {
            let family = optional(
                "Citation author family names (contact is not assumed to be an author)",
                None,
            )?
            .context("Citation creation requires explicit authors")?;
            let given =
                optional("Citation author given names (optional)", None)?.unwrap_or_default();
            options.author_family.push(family);
            options.author_given.push(given);
            if !yes("Add another citation author?")? {
                break;
            }
        }
    }
    anyhow::ensure!(
        !options.author_family.is_empty() || !options.author_name.is_empty(),
        "Citation creation requires --author-family or --author-name; contact details are not authors"
    );
    anyhow::ensure!(
        options.author_given.is_empty()
            || options.author_given.len() == options.author_family.len(),
        "Supply one --author-given per --author-family, or omit all given names"
    );
    let mut values = Vec::new();
    for (i, family) in options.author_family.iter().enumerate() {
        single(family, "Author family names")?;
        let mut author = serde_json::json!({"family-names":family});
        if let Some(given) = options
            .author_given
            .get(i)
            .filter(|given| !given.is_empty())
        {
            single(given, "Author given names")?;
            author["given-names"] = given.clone().into();
        }
        values.push(author);
    }
    for name in &options.author_name {
        single(name, "Author/team name")?;
        values.push(serde_json::json!({"name":name}));
    }
    Ok(values)
}
/// Set up missing metadata. Existing DESCRIPTION Version follows settings; other files/remotes survive.
/// Local successful setup steps remain if a later optional external action fails; this is not a transaction.
pub fn initialize(mut options: InitOptions) -> anyhow::Result<()> {
    let root = crate::ignore::root().context("Cannot select the initialization directory")?;
    let state = inspect(&root)?;
    let readme = if options.readme_rmd {
        "README.Rmd"
    } else {
        "README.md"
    };
    let mut guarded = vec!["VERSION", readme];
    if state.description || options.desc {
        guarded.push("DESCRIPTION");
    }
    if options.citation {
        guarded.push("CITATION.cff");
    }
    if options.code_license.is_some() {
        guarded.extend(["LICENSE", "LICENSE.md"]);
    }
    for name in guarded {
        let path = root.join(name);
        crate::path_safety::resolve_path(path.as_std_path(), true)?;
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            anyhow::ensure!(
                metadata.is_file(),
                "Initialization metadata must be a regular file: {path}"
            );
        }
    }
    let combined = crate::yml::merged_configuration_from(&root, None, false)?;
    let settings: crate::yml::SettingsConfig = combined
        .get("settings")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    let sync_description = settings.sync_description_version.unwrap_or(true);
    crate::version::diagnose_description(&root, sync_description)?;
    let need_metadata = !root.join(readme).exists()
        || (options.desc && !state.description)
        || (options.citation && !state.citation);
    let folder = root.file_name().unwrap_or("Project");
    single(folder, "Project folder name")?;
    let known = if state.description {
        Some(crate::fs_utils::read_managed_text(
            root.join("DESCRIPTION"),
        )?)
    } else {
        None
    };
    let known_title = known.as_deref().and_then(|text| {
        text.lines()
            .find_map(|line| line.strip_prefix("Title:").map(str::trim))
    });
    let default_title = known_title
        .filter(|value| !value.is_empty())
        .unwrap_or(folder);
    let title = if need_metadata {
        match options.title.take() {
            Some(value) => value,
            None => optional("Project title", Some(default_title))?
                .unwrap_or_else(|| default_title.to_owned()),
        }
    } else {
        options
            .title
            .take()
            .unwrap_or_else(|| default_title.to_owned())
    };
    single(&title, "Project title")?;
    let purpose = if need_metadata && options.purpose.is_none() {
        optional(
            &format!("Please finish: The purpose of {folder} is to... (blank to skip)"),
            None,
        )?
        .map(|value| format!("The purpose of {folder} is to {value}"))
    } else {
        options.purpose.take()
    };
    if let Some(value) = &purpose {
        anyhow::ensure!(!value.contains('\0'), "Project purpose cannot contain NUL");
    }
    let contact = if need_metadata && options.contact_name.is_none() {
        optional("Project contact name (optional)", None)?
    } else {
        options.contact_name.take()
    };
    let email = if need_metadata && options.contact_email.is_none() {
        optional("Project contact email (optional)", None)?
    } else {
        options.contact_email.take()
    };
    for (value, name) in [(&contact, "Contact name"), (&email, "Contact email")] {
        if let Some(value) = value {
            single(value, name)?;
        }
    }
    let mut selected = options.code_license.take();
    if selected.is_none()
        && !root.join("LICENSE").exists()
        && !root.join("LICENSE.md").exists()
        && crate::init::is_prompt_enabled()
    {
        loop {
            selected = optional(
                "Code licence: ccby / apache / cc0 / proprietary / later",
                Some("later"),
            )?;
            if selected.as_deref().is_some_and(|value| {
                ["ccby", "apache", "cc0", "proprietary", "none", "later"]
                    .contains(&value.to_ascii_lowercase().as_str())
            }) {
                break;
            }
            eprintln!("Choose one of the four licences, or later.");
        }
    }
    if selected
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("proprietary"))
        && options.copyright_holder.is_none()
    {
        options.copyright_holder = optional("Copyright holder for proprietary code licence", None)?;
    }
    let license = selected
        .as_deref()
        .map(|choice| licence(choice, options.copyright_holder.as_deref()))
        .transpose()?
        .flatten();
    let citation = if options.citation && !state.citation {
        let authors = authors(&mut options)?;
        let mut value = serde_json::json!({
            "cff-version":"1.2.0","message":"Please cite this project using the metadata in this file.",
            "title":title,"authors":authors
        });
        if let Some(purpose) = &purpose {
            value["abstract"] = purpose.clone().into();
        }
        Some(serde_yaml::to_string(&value)?)
    } else {
        None
    };
    let want_git = if state.git_repository {
        false
    } else {
        match options.git {
            Some(value) => value,
            None => yes("Initialize a Git repository?")?,
        }
    };
    let want_github = if state.remote {
        false
    } else if state.git_repository || want_git {
        match options.github {
            Some(value) => value,
            None => yes(
                "Create a private GitHub repository using existing gh authentication? (no push)",
            )?,
        }
    } else {
        anyhow::ensure!(
            options.github != Some(true),
            "GitHub setup requires --git=true or an existing Git repository"
        );
        false
    };
    anyhow::ensure!(
        !want_git || state.git_available,
        "Install real Git before requesting Git initialization"
    );
    if root.join("VERSION").exists() {
        anyhow::ensure!(
            crate::version::version_get_from(&root).is_some(),
            "Existing VERSION is invalid; setup stopped"
        );
    }
    // Validate generated metadata before any writes.
    if options.desc && !state.description {
        let version = crate::version::version_get_from(&root).unwrap_or(crate::ProjVersion {
            major: 0,
            minor: 0,
            patch: 1,
            dev: 0,
        });
        let package_name = folder.len() >= 2
            && folder.as_bytes()[0].is_ascii_alphabetic()
            && !folder.ends_with('.')
            && folder
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.');
        let mut description = if package_name {
            dcf("Package", folder)
        } else {
            String::new()
        };
        description.push_str(&(dcf("Title", &title) + &dcf("Version", &version.to_string(false))));
        if let Some(value) = &purpose {
            description.push_str(&dcf("Description", value));
        }
        if let Some(value) = &contact {
            let maintainer = match &email {
                Some(email) => format!("{value} <{email}>"),
                None => value.clone(),
            };
            description.push_str(&dcf("Maintainer", &maintainer));
        } else if let Some(value) = &email {
            description.push_str(&dcf("Maintainer", value));
        }
        description.push_str("Encoding: UTF-8\n");
        create(&root, "DESCRIPTION", description.as_bytes())?;
    }
    crate::init::init_version()?;
    if sync_description {
        let version = crate::version::version_get_from(&root)
            .context("Initialization requires valid VERSION")?;
        if let Some(bytes) = crate::version::description_bytes(&root, &version)?
            && fs::read(root.join("DESCRIPTION"))? != bytes
        {
            crate::fs_utils::replace_managed_file(&root.join("DESCRIPTION"), bytes)?;
        }
    }
    crate::init::init_directories()?;
    let mut body = if options.readme_rmd {
        format!(
            "---\n{}---\n\n# {title}\n\n",
            serde_yaml::to_string(&serde_json::json!({"title":title,"output":"md_document"}))?
        )
    } else {
        format!("# {title}\n\n")
    };
    if let Some(value) = purpose {
        body.push_str(&value);
        body.push_str("\n\n");
    }
    if contact.is_some() || email.is_some() {
        body.push_str("## Contact\n\n");
        if let Some(value) = contact {
            body.push_str(&value);
            body.push('\n');
        }
        if let Some(value) = email {
            body.push_str(&value);
            body.push('\n');
        }
        body.push('\n');
    }
    create(&root, readme, body.as_bytes())?;
    if !root.join("LICENSE.md").exists()
        && let Some(text) = license
    {
        create(&root, "LICENSE", text.as_bytes())?;
    }
    if let Some(text) = citation {
        create(&root, "CITATION.cff", text.as_bytes())?;
    }
    if options.renv {
        crate::init::init_renv()?;
    }
    if want_git {
        create_repository(&root)?;
    }
    if want_github {
        create_github(&root)?;
    }
    Ok(())
}
