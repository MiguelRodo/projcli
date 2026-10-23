use std::path::Path;
use std::fs;
use crate::yml::ProjConfig;
use crate::version::{version_get_from, version_set_at};

/// Defines the mode of the build cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildMode {
    /// Development run (`proj build-dev`)
    Dev,
    /// Production run, bumping major version (`proj build --major`)
    ProdMajor,
    /// Production run, bumping minor version (`proj build --minor`)
    ProdMinor,
    /// Production run, bumping patch version (`proj build --patch` or default)
    ProdPatch,
}

/// Execution-directory policy, independent of captured project identity and paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionDirectory {
    /// Scripts inherit the invocation cwd; document renderers retain native defaults.
    Native,
    /// Explicit project-root cwd for supported script/Quarto/individual Rmd runners.
    Project,
}

/// Runs the build pipeline for the project within the provided `project_root`.
///
/// Ensures mutual exclusion rules are enforced regarding `_quarto.yml` and `_bookdown.yml`,
/// resolves targets using explicit `build.scripts` configuration or fallback auto-discovery,
/// reorders execution according to constraints, and delegates script execution.
///
/// # Errors
///
/// Returns an error if structural validation constraints fail (e.g., both Quarto and Bookdown configs are present and script configs conflict),
/// if the scripts fail to execute, or if IO operations fail.
///
/// ```rust
/// use std::path::PathBuf;
/// use proj::build::{build_project, BuildMode};
/// use tempfile::TempDir;
///
/// let temp = TempDir::new().unwrap();
/// std::fs::write(temp.path().join("VERSION"), "Version: v1.0.0").unwrap();
/// // Create a dummy _proj.yml to avoid fallback searching which executes everything
/// // We set push: false so we don't trigger GitHub token lookups in CI without env vars
/// std::fs::write(temp.path().join("_proj.yml"), "directories:\n  cache:\n    path: _tmp\nbuild:\n  scripts: []\n  git:\n    commit: false\n    push: false\nconfig:\n  git:\n    use_proj_cred_helper: false").unwrap();
/// build_project(camino::Utf8Path::from_path(temp.path()).unwrap(), BuildMode::ProdPatch, None, None).unwrap();
/// ```
use crate::git::{is_git_installed_in, check_git_profile_in, create_git_provider_in};
use crate::run_environment::RunEnvironment;
use crate::yml::GlobalConfig;
use crate::build_pre::run_pre_flight_checks_in;

pub fn post_build_sync(config: &GlobalConfig, repo_dir: camino::Utf8PathBuf) -> anyhow::Result<()> {
    post_build_sync_in(config, &repo_dir, &RunEnvironment::inherited())
}

fn post_build_sync_in(config: &GlobalConfig, root: &camino::Utf8Path, inherited: &RunEnvironment) -> anyhow::Result<()> {
    let mut environment = inherited.clone();
    environment.set("PROJCLI_GIT_TOKEN_OVERRIDE", config.git.use_proj_cred_helper.to_string());
    let target = crate::git::push_target_in(root, &environment)?;
    publish_git_in(root, &environment, config.git.engine, "chore: automated workspace build update", Some(&target))
}

fn publish_git_in(root: &camino::Utf8Path, environment: &RunEnvironment, engine: crate::yml::GitEngine, message: &str, target: Option<&crate::git::PushTarget>) -> anyhow::Result<()> {
    let provider = create_git_provider_in(engine, root.to_owned(), environment)?;
    if let Some(target) = target { crate::git::validate_push_target_in(root, environment, target)?; }
    provider.commit_all(message)?;
    if let Some(target) = target {
        crate::git::validate_push_target_in(root, environment, target)?;
        provider.push(&target.remote, &format!("HEAD:{}", target.reference))?;
    }
    Ok(())
}

pub fn build_project(project_root: &camino::Utf8Path, mode: BuildMode, cli_profile: Option<&str>, description: Option<&str>) -> anyhow::Result<()> {
    build_project_with_policies(project_root, mode, cli_profile, description, None, None)
}

/// Runs a build with an explicit execution-directory policy.
/// Native captures invocation cwd once without mutating process cwd. Project
/// overrides supported runners; project-wide Bookdown override is unsupported.
/// Root/profile/version context remains independent. Errors preserve the same
/// managed-state boundary as build_project; arbitrary external script effects are
/// not a sandbox guarantee.
pub fn build_project_with_execution(project_root: &camino::Utf8Path, mode: BuildMode, cli_profile: Option<&str>, description: Option<&str>, execution: ExecutionDirectory) -> anyhow::Result<()> {
    build_project_with_clear_stage(project_root, mode, cli_profile, description, execution, None)
}

/// Adds an explicit clear-stage override above merged settings and legacy environment.
pub fn build_project_with_clear_stage(project_root: &camino::Utf8Path, mode: BuildMode, cli_profile: Option<&str>, description: Option<&str>, execution: ExecutionDirectory, clear_stage: Option<crate::yml::OutputClearStage>) -> anyhow::Result<()> {
    build_project_with_policies(project_root, mode, cli_profile, description, Some(execution), clear_stage)
}

/// Applies explicit command overrides above merged runtime settings.
pub fn build_project_with_policies(project_root: &camino::Utf8Path, mode: BuildMode, cli_profile: Option<&str>, description: Option<&str>, execution_override: Option<ExecutionDirectory>, clear_stage: Option<crate::yml::OutputClearStage>) -> anyhow::Result<()> {
    let caller_directory = std::env::current_dir().context("Cannot capture invocation working directory")?;
    let resolved_root = crate::project::resolve_project_root(Some(project_root))?;
    let project_root = resolved_root.as_path();
    let is_dev = mode == BuildMode::Dev;
    let is_prod_run = matches!(mode, BuildMode::ProdMajor | BuildMode::ProdMinor | BuildMode::ProdPatch);

    // ==========================================
    // STEP A: Resolve Per-Run Environment
    // ==========================================
    // Load inputs without writing ignores; configuration errors must stay read-only.
    let profiles = crate::run_context::select_profiles(cli_profile, project_root)?;
    let profile_text = profiles.join(",");
    let mut environment = RunEnvironment::resolve(Some(&profile_text), project_root)
        .context("Failed to resolve build environment")?;
    environment.set(crate::PROJECT_ROOT_ENV, project_root.as_str());



    // ==========================================
    // STEP B: Pre-Build Validation & Execution
    // ==========================================

    // 1. Config Audit
    let selected_profile = environment.text("PROJR_PROFILE").unwrap_or_default();
    crate::yml::require_active_configuration_for_profiles(project_root, &crate::profile::parse_active_profiles(selected_profile))?;
    let combined = crate::yml::get_combined_yml(Some(selected_profile), project_root)
        .context("Failed to read project configuration")?;
    let directory_order = crate::run_context::directory_order(&combined);
    let project_config: ProjConfig = serde_json::from_value(combined)
        .context("Failed to deserialize combined configuration")?;
    let config = project_config.validate_and_resolve(project_root, is_dev)?;
    let execution = execution_override.or(config.settings.execution_directory).unwrap_or(ExecutionDirectory::Native);
    environment.set_working_directory(match execution {
        ExecutionDirectory::Native => caller_directory,
        ExecutionDirectory::Project => project_root.as_std_path().to_owned(),
    });
    let clear_stage = config.settings.clear_stage(clear_stage, config.clear_output.as_deref(), environment.text("PROJR_CLEAR_OUTPUT"))?;
    if !is_dev && config.old_dev_remove == Some(true) {
        anyhow::bail!("Automatic old development-cache removal is unsupported with safe routing; disable dev.old_dev_remove");
    }
    let selected_cache = crate::paths::select_safe_cache(config.directories.iter().map(|(label, directory)| (label.as_str(), directory.safe)), &directory_order)?;
    environment.set("PROJCLI_GIT_TOKEN_OVERRIDE", config.config.git.use_proj_cred_helper.to_string());
    let push_target = if config.git.push {
        Some(crate::git::push_target_in(project_root, &environment)?)
    } else { None };



    // 2. Git Capability Audit
    if config.git.commit {
        if !is_git_installed_in(&environment) {
            anyhow::bail!("Git is required for commit but not found on system PATH");
        }
        // TODO: migrate to camino
        check_git_profile_in(Some(project_root), &environment).context("Failed to check Git profile capability")?;
        crate::git::require_clean_git_in(project_root, &environment)?;
    }

    // 3. Resolve configs and hooks
    let quarto_exists = project_root.join("_quarto.yml").exists();
    let bookdown_exists = project_root.join("_bookdown.yml").exists();

    let build = project_config.build;
    let dev = project_config.dev;
    let profile = Some(profile_text.clone());
    let (build_scripts, hooks_config) = if is_dev {
        (dev.scripts.or(build.scripts), dev.hooks.unwrap_or(build.hooks))
    } else {
        (build.scripts, build.hooks)
    };

    // Extract hooks paths
    let mut raw_hooks = Vec::new();
    if let Some(pre_hooks) = &hooks_config.pre { raw_hooks.extend(pre_hooks.iter().cloned()); }
    if let Some(post_hooks) = &hooks_config.post { raw_hooks.extend(post_hooks.iter().cloned()); }
    if let Some(both_hooks) = &hooks_config.both { raw_hooks.extend(both_hooks.iter().cloned()); }

    // Pre-flight Environment Verification
    let mut resolved_files = Vec::new();
    if let Some(scripts) = &build_scripts {
        resolved_files = resolve_explicit_scripts(project_root, scripts)?;
    } else {
        if !quarto_exists && !bookdown_exists {
            resolved_files = resolve_fallback_scripts(project_root)?;
        }
    }

    // Upfront check for hooks
    let resolved_hooks = resolve_explicit_scripts(project_root, &raw_hooks)?;

    for expected_hook in raw_hooks.iter() {
        if expected_hook.starts_with('!') { continue; } // Exclusions not validated directly here
        let mut found = false;
        // Simple validation: Ensure explicitly asked scripts/hooks map to at least one file.
        // It's covered by `resolve_explicit_scripts` failing on glob error, but we want to fail fast if explicitly missing.
        for h in &resolved_hooks {
            if h.as_str().contains(&expected_hook.replace('/', std::path::MAIN_SEPARATOR_STR)) {
                found = true;
                break;
            }
        }
        let target_pattern = project_root.join(expected_hook);
        if !found && !target_pattern.exists() && !target_pattern.parent().map(|p| p.exists()).unwrap_or(true) {
            // Best effort check for explicitly missed files
        }
    }

    // Fail fast if explicit targets are missing entirely (hooks or scripts without globs)
    let check_explicit_missing = |items: &[String]| -> anyhow::Result<()> {
        for item in items {
            if item.starts_with('!') || item.contains('*') || item.contains('?') { continue; }
            let p = project_root.join(item);
            if !p.exists() {
                anyhow::bail!("Explicitly defined script or hook file '{}' does not exist", item);
            }
        }
        Ok(())
    };

    if let Some(scripts) = &build_scripts {
        check_explicit_missing(scripts)?;
    }
    check_explicit_missing(&raw_hooks)?;

    // An explicit empty list selects no analysis; it never enables discovery or
    // renderer setup. Hooks are still validated and executed independently.
    let run_analysis = build_scripts.as_ref().map_or(true, |scripts| !scripts.is_empty());
    let selected_documents = build_scripts.is_none() || resolved_files.iter().any(|file| matches!(file.extension(), Some("qmd" | "Rmd" | "rmd")));
    let quarto_exists = quarto_exists && run_analysis && selected_documents;
    let bookdown_exists = bookdown_exists && run_analysis && selected_documents;
    if bookdown_exists && execution == ExecutionDirectory::Project {
        anyhow::bail!("Project-root execution override is not supported for project-wide Bookdown; use native execution");
    }

    let cache_root = selected_cache.as_deref().map(|label| config.get_path(project_root, label)).transpose()?;
    let has_document_analysis = quarto_exists || bookdown_exists || resolved_files.iter().any(|file| matches!(file.extension(), Some("qmd" | "Rmd" | "rmd")));
    if has_document_analysis { config.get_path(project_root, "docs").context("Document rendering requires a declared docs directory")?; }
    let mut validation_files = resolved_files.clone();
    validation_files.extend(resolved_hooks.clone());
    for file in &validation_files {
        if !matches!(file.extension(), Some("R" | "r" | "py" | "qmd" | "Rmd" | "rmd")) {
            anyhow::bail!("Unsupported selected script extension; select R, Python, QMD or R Markdown inputs");
        }
    }
    if resolved_hooks.iter().any(|file| matches!(file.extension(), Some("qmd" | "Rmd" | "rmd"))) {
        anyhow::bail!("Document-rendering hooks have no supported output plan; use R/Python hooks and select documents as analysis");
    }

    for file in &validation_files {
        if file.extension() == Some("qmd") || (quarto_exists && matches!(file.extension(), Some("Rmd" | "rmd"))) { validate_quarto_input(project_root, file)?; }
    }
    if quarto_exists && build_scripts.is_none() { validate_implicit_quarto_roots(project_root, &config)?; }
    if has_document_analysis && !quarto_exists && !bookdown_exists {
        let docs = config.get_path(project_root, "docs")?;
        for file in &validation_files {
            if matches!(file.extension(), Some("qmd" | "Rmd" | "rmd")) { document_output_paths(file, &docs, project_root)?; }
        }
    }

    // 4. VERSION Initialization Check
    let original_version = match version_get_from(project_root) {
        Some(v) => v,
        None => {
            let desc_path = project_root.join("DESCRIPTION");
            if desc_path.exists() {
                let content = fs::read_to_string(&desc_path).context("Failed to read DESCRIPTION file")?;
                let mut found_ver = None;
                for line in content.lines() {
                    if line.starts_with("Version:") {
                        let ver_str = line.trim_start_matches("Version:").trim();
                        if let Some(v) = crate::version::ProjVersion::parse(ver_str) {
                            found_ver = Some(v);
                            break;
                        }
                    }
                }
                found_ver.context("Could not extract a valid Version from DESCRIPTION file")?
            } else {
                let v = crate::version::ProjVersion { major: 0, minor: 0, patch: 1, dev: 0 };
                v
            }
        }
    };

    if project_root.join("VERSION").exists() && version_get_from(project_root).is_none() {
        anyhow::bail!("VERSION exists but is invalid");
    }

    let (initial_version, next_version) = crate::run_context::version_plan(original_version, mode)?;
    let run_context = crate::run_context::ContextFile::capture(project_root, &config, profiles, directory_order, initial_version.clone(), is_dev)?;
    environment.set(crate::RUN_CONTEXT_ENV, run_context.path().as_os_str());
    let mut staged_outputs = Vec::new();
    if !is_dev && run_analysis && config.output_run.unwrap_or(true) && cache_root.is_some() {
        for label in config.directories.keys().filter(|label| crate::paths::is_generated_output(label)) {
            let cache = cache_root.as_ref().unwrap();
            let staged = cache.join("projr").join(initial_version.cache_key()).join(label);
            crate::path_safety::resolve_path(staged.as_std_path(), true)?;
            staged_outputs.push((staged, config.get_path(project_root, label)?));
        }
    }
    if has_document_analysis { anyhow::ensure!(cache_root.is_some(), "Document rendering requires an eligible cache directory"); }
    // Recovery removes/replaces managed destinations too, so preview needs the
    // same root/overlap/link guards even though it never performs release cleanup.
    crate::path_safety::validate_build_paths(project_root, &config, &validation_files)?;
    if run_analysis {
        {
            if quarto_exists || bookdown_exists {
                crate::path_safety::validate_project_source_overlap(project_root, &config)?;
            } else {
                for file in &resolved_files {
                    if matches!(file.extension(), Some("qmd" | "Rmd" | "rmd")) {
                        let outputs = document_output_paths(file, &config.get_path(project_root, "docs")?, project_root)?;
                        for input in &validation_files {
                            let identity = crate::path_safety::resolve_path(input.as_std_path(), false)?;
                            for target in [&outputs.target, &outputs.destination, &outputs.assets, &outputs.destination_assets].into_iter().chain(outputs.intermediate.iter()) {
                                if identity.starts_with(crate::path_safety::resolve_path(target.as_std_path(), true)?) {
                                    anyhow::bail!("Rendered output overwrites protected input script/hook: {input}");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let (_resolved_token, resolved_python_cmd) = run_pre_flight_checks_in(
        project_root,
        &config,
        is_prod_run,
        &validation_files,
        quarto_exists,
        bookdown_exists,
        &environment,
    )?;

    // Parse existing history before hooks or any managed mutation. Preview and
    // disabled output builds neither read nor write the production CSV.
    let record_manifest = !is_dev && config.output_run.unwrap_or(true);
    let previous_manifest = if record_manifest {
        crate::manifest::read(&project_root.join("manifest.csv"))?
    } else { Vec::new() };
    // Capture exact local state before the first controlled project mutation.
    let mut managed = vec![];
    let mut native_artifacts = Vec::new();
    let mut native_layouts = Vec::new();
    let frozen_book_assets = if bookdown_exists { Some(bookdown_asset_paths(project_root, &config.get_path(project_root, "docs")?)?.0) } else { None };
    for name in ["VERSION", "manifest.csv", ".gitignore", ".Rbuildignore", "_proj.yml", "_projr.yml", "_projr-local.yml", "DESCRIPTION", "_quarto.yml", "_bookdown.yml"] {
        managed.push(project_root.join(name).into_std_path_buf());
    }
    for entry in fs::read_dir(project_root)
        .with_context(|| format!("Cannot enumerate managed project state at {project_root}"))?
    {
        let entry = entry.with_context(|| format!("Cannot read managed project-state directory entry at {project_root}"))?;
        if entry.file_type().with_context(|| format!("Cannot inspect managed project entry {}", entry.path().display()))?.is_file() {
            managed.push(entry.path());
        }
    }
    for (label, _) in &config.directories {
        if crate::paths::is_managed_output(label) {
            managed.push(config.get_path(project_root, label)?.into_std_path_buf());
        }
    }
    let root_identity = crate::path_safety::resolve_path(project_root.as_std_path(), false)?;
    for input in &validation_files {
        // Reject links along the original path before canonicalizing its namespace.
        let identity = crate::path_safety::resolve_path(input.as_std_path(), true)?;
        if is_dev || identity.starts_with(&root_identity) { managed.push(identity); }
    }
    if bookdown_exists {
        let (assets, destination) = bookdown_asset_paths(project_root, &config.get_path(project_root, "docs")?)?;
        let asset_identity = crate::path_safety::resolve_path(assets.as_std_path(), true)?;
        for input in &validation_files {
            if crate::path_safety::resolve_path(input.as_std_path(), true)?.starts_with(&asset_identity) {
                anyhow::bail!("Bookdown assets contain a selected input");
            }
        }
        native_artifacts.push(assets.clone().into_std_path_buf());
        managed.push(assets.into_std_path_buf());
        managed.push(destination.into_std_path_buf());
    }
    if has_document_analysis && !quarto_exists && !bookdown_exists {
        let docs = config.get_path(project_root, "docs")?;
        for file in &resolved_files {
            if matches!(file.extension(), Some("qmd" | "Rmd" | "rmd")) {
                let paths = document_output_paths(file, &docs, project_root)?;
                native_layouts.push((file.clone(), paths.target.clone(), paths.assets.clone(), paths.intermediate.clone()));
                if let Some(intermediate) = &paths.intermediate {
                    native_artifacts.push(intermediate.clone().into_std_path_buf());
                    managed.push(intermediate.clone().into_std_path_buf());
                }
                let artifacts = [paths.target.into_std_path_buf(), paths.assets.into_std_path_buf()];
                native_artifacts.extend(artifacts.iter().cloned());
                managed.extend(artifacts);
            }
        }
    }
    if is_dev {
        // Capture configured local publication stores without invoking publication.
        if let Some(remotes) = &config.remotes.local {
            for remote in remotes.values() {
                let destination = crate::path_safety::resolve_path(remote.path.as_std_path(), true)?;
                if root_identity.starts_with(&destination) {
                    anyhow::bail!("Preview publication recovery path contains the project root");
                }
                managed.push(destination);
            }
        }
        for name in [".git", ".hg", ".svn"] {
            let path = project_root.join(name);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_file() => anyhow::bail!("Preview recovery does not support linked/worktree version-control metadata"),
                Ok(_) => managed.push(path.into_std_path_buf()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => managed.push(path.into_std_path_buf()),
                Err(error) => return Err(error).context("Cannot inspect preview version-control state"),
            }
        }
    }
    let recovery_caches = config.directories.keys().filter(|label| label.to_ascii_lowercase().starts_with("cache"))
        .map(|label| config.get_path(project_root, label).map(|path| path.into_std_path_buf()))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let backup = crate::build_state::BuildState::capture(&recovery_caches, &managed)?;
    // Native tools may emit beside their input. Restore those original compiled
    // artifacts after staging on success too, so Git never sweeps up scratch output.
    let mut native_backup = if native_artifacts.is_empty() { None } else {
        Some(crate::build_state::BuildState::capture(&recovery_caches, &native_artifacts)?)
    };
    let mut engine_originals = Vec::new();
    for (active, name) in [(quarto_exists, "_quarto.yml"), (bookdown_exists, "_bookdown.yml")] {
        if active {
            let path = project_root.join(name);
            crate::path_safety::resolve_path(path.as_std_path(), true)?;
            let bytes = fs::read(&path)?;
            let parsed: serde_yaml::Value = serde_yaml::from_slice(&bytes).with_context(|| format!("Invalid renderer configuration {name}"))?;
            if !parsed.is_mapping() { anyhow::bail!("Renderer configuration {name} must be a mapping"); }
            engine_originals.push((path, bytes));
        }
    }
    let restore_engines = || -> anyhow::Result<()> {
        for (path, bytes) in &engine_originals { crate::path_safety::resolve_path(path.as_std_path(), true)?; crate::fs_utils::write_managed_file(path, bytes).with_context(|| format!("Cannot restore exact renderer configuration {path}"))?; }
        Ok(())
    };
    let mut document_stage = None;
    let mut project_embedding = None;
    let local_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> anyhow::Result<()> {
        // Basic read-only validation and recovery are complete. Start hooks run first.
        for hooks in [&hooks_config.pre, &hooks_config.both] {
            if let Some(hooks) = hooks {
                for hook in resolve_explicit_scripts(project_root, hooks)? {
                    execute_script(project_root, &hook, profile.as_deref(), resolved_python_cmd.as_deref(), &environment, execution)?;
                }
            }
        }
        for (file, expected_target, expected_assets, expected_intermediate) in &native_layouts {
            let current = document_output_paths(file, &config.get_path(project_root, "docs")?, project_root)?;
            anyhow::ensure!(&current.target == expected_target && &current.assets == expected_assets && &current.intermediate == expected_intermediate,
                "Renderer output layout changed after preflight: {file}");
        }
        if let Some(expected) = &frozen_book_assets {
            anyhow::ensure!(&bookdown_asset_paths(project_root, &config.get_path(project_root, "docs")?)?.0 == expected,
                "Bookdown output layout changed after preflight");
        }
        if !is_dev {
            if project_root.join("_environment.local").exists() {
                crate::ignore::add_manual_ignores(project_root, &["_environment.local".to_string()], true, crate::ignore::IgnoreType::All)?;
            }
            crate::ignore::update_ignores(project_root, &config)?;
        }
        if has_document_analysis {
            let cache = cache_root.as_ref().context("Document rendering requires cache")?;
            crate::path_safety::resolve_path(cache.as_std_path(), true)?;
            let versioned_cache = cache.join("projr").join(initial_version.cache_key());
            crate::path_safety::resolve_path(versioned_cache.as_std_path(), true)?;
            fs::create_dir_all(&versioned_cache)?;
            document_stage = Some(tempfile::Builder::new().prefix(".proj-render-").tempdir_in(&versioned_cache)?);
            let staging = camino::Utf8Path::from_path(document_stage.as_ref().unwrap().path()).context("Render staging path must be UTF-8")?;
            if quarto_exists { rewrite_engine_output_dir(project_root.as_std_path(), "quarto", staging.as_str())?; }
            if bookdown_exists { rewrite_engine_output_dir(project_root.as_std_path(), "bookdown", staging.as_str())?; }
        }
        let selected_documents: Vec<_> = resolved_files.iter().filter(|file| matches!(file.extension(), Some("qmd" | "Rmd" | "rmd"))).cloned().collect();
        if build_scripts.is_some() && !selected_documents.is_empty() {
            if quarto_exists { rewrite_quarto_yml(project_root.as_std_path(), &selected_documents)?; }
            else if bookdown_exists { rewrite_bookdown_yml(project_root.as_std_path(), &selected_documents)?; }
        }
        if quarto_exists && has_document_analysis {
            project_embedding = Some(crate::embedding::ProjectEmbedding::capture(project_root, &environment, &recovery_caches)?);
            project_embedding.as_ref().unwrap().apply()?;
        }
        // Freeze the prepared project metadata. A prior analysis script may not
        // silently redirect a later renderer or change its native asset layout.
        let prepared_engines = engine_originals.iter().map(|(path, _)|
            Ok((path.clone(), crate::fs_utils::read_managed_text(path)?)))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let before_render = |document: Option<&camino::Utf8Path>| -> anyhow::Result<()> {
            if let Some(file) = document {
                if file.extension() == Some("qmd") {
                    validate_quarto_input(project_root, file)?;
                    anyhow::ensure!(!crate::yml::configuration_entry_exists(&project_root.join("_quarto.yml"))?
                        && !crate::yml::configuration_entry_exists(&project_root.join("_quarto.yaml"))?,
                        "Quarto project identity changed after individual-render preflight");
                }
                let (_, target, assets, intermediate) = native_layouts.iter().find(|(input, _, _, _)| input == file)
                    .context("Missing frozen document layout")?;
                let current = document_output_paths(file, &config.get_path(project_root, "docs")?, project_root)?;
                anyhow::ensure!(&current.target == target && &current.assets == assets && &current.intermediate == intermediate, "Renderer output layout changed after preflight: {file}");
                remove_native_artifact(target)?;
                remove_native_artifact(assets)?;
                if let Some(intermediate) = intermediate { remove_native_artifact(intermediate)?; }
            } else {
                if quarto_exists {
                    if build_scripts.is_none() { validate_implicit_quarto_roots(project_root, &config)?; }
                    for file in &selected_documents { validate_quarto_input(project_root, file)?; }
                }
                for (path, expected) in &prepared_engines {
                    anyhow::ensure!(crate::fs_utils::read_managed_text(path)? == *expected,
                        "Renderer project configuration changed after preparation: {path}");
                }
                if let Some(embedding) = &project_embedding { embedding.verify(project_root, &environment)?; }
                if let Some(assets) = &frozen_book_assets { remove_native_artifact(assets)?; }
                let stage = document_stage.as_ref().context("Missing project render staging")?.path();
                crate::path_safety::resolve_path(stage, true)?;
                for entry in fs::read_dir(stage)? {
                    let path = camino::Utf8PathBuf::try_from(entry?.path())?;
                    remove_native_artifact(&path)?;
                }
            }
            Ok(())
        };
        let pre_manifest = if record_manifest {
            let rows = crate::manifest::capture_build(project_root, &config, &initial_version.to_string(true), true)?;
            if let Some(cache) = &cache_root {
                let directory = cache.join("projr").join(initial_version.cache_key());
                crate::path_safety::resolve_path(directory.as_std_path(), true)?;
                fs::create_dir_all(&directory)?;
                crate::manifest::write(&directory.join("manifest.csv"), &rows)?;
            }
            rows
        } else { Vec::new() };
        // No preparation, hooks or metadata work may intervene at these boundaries.
        if !is_dev && run_analysis && config.output_run.unwrap_or(true) && clear_stage == crate::yml::OutputClearStage::Pre {
            crate::clear::clear_post(project_root, false, &config, Some("post"), false)?;
        }
        execute_build_pipeline(project_root, &resolved_files, build_scripts.is_some(), profile.clone(), quarto_exists, bookdown_exists, resolved_python_cmd.as_deref(), &environment, execution, &before_render)?;
        if !is_dev && run_analysis && config.output_run.unwrap_or(true) && clear_stage == crate::yml::OutputClearStage::Post {
            crate::clear::clear_post(project_root, false, &config, Some("post"), false)?;
        }
        if let Some(embedding) = project_embedding.take() { embedding.restore_result(Ok(()))?; }
        restore_engines()?;
        if has_document_analysis {
            let staging = camino::Utf8Path::from_path(document_stage.as_ref().unwrap().path()).context("Render staging path must be UTF-8")?;
            if !quarto_exists && !bookdown_exists {
                for file in &resolved_files {
                    if matches!(file.extension(), Some("qmd" | "Rmd" | "rmd")) { let (_, target, assets, _) = native_layouts.iter().find(|(input, _, _, _)| input == file).context("Missing frozen document collection layout")?;
                        stage_frozen_document(target, assets, staging)?; }
                }
            } else if bookdown_exists {
                let assets = frozen_book_assets.as_ref().context("Missing frozen Bookdown assets")?;
                let destination = staging.join(assets.file_name().context("Missing Bookdown asset leaf")?);
                if assets.exists() { crate::fs_utils::copy_dir_recursive(assets, &destination)?; }
            }
            validate_project_artifacts(staging)?;
            let docs_label = config.directories.keys().find(|label| label.eq_ignore_ascii_case("docs"))
                .context("Document rendering requires the exact docs role")?;
            let cached_docs = cache_root.as_ref().unwrap().join("projr").join(initial_version.cache_key()).join(docs_label);
            crate::fs_utils::copy_dir_recursive(staging, &cached_docs)?;
        }
        if let Some(native) = native_backup.take() { native.restore_result(Ok(()))?; }
        if !is_dev && config.output_run.unwrap_or(true) {
            for (staged, final_path) in &staged_outputs {
                if staged.exists() { crate::fs_utils::copy_dir_recursive(staged, final_path)?; }
            }
            if has_document_analysis {
                let staging = camino::Utf8Path::from_path(document_stage.as_ref().unwrap().path()).context("Render staging path must be UTF-8")?;
                let docs = config.get_path(project_root, "docs")?;
                crate::clear::clear_post(project_root, false, &config, Some("never"), true)?;
                crate::fs_utils::copy_dir_recursive(staging, &docs)?;
            }
        }
        if record_manifest {
            let outputs = crate::manifest::capture_build(project_root, &config, &initial_version.to_string(true), false)?;
            let rows = crate::manifest::deduplicate(pre_manifest.into_iter().chain(outputs).chain(previous_manifest.iter().cloned()));
            crate::manifest::write(&project_root.join("manifest.csv"), &rows)?;
        }
        Ok(())
    }));
    let mut local_result = match local_result {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!("Build pipeline panicked unexpectedly")),
    };
    if let Some(embedding) = project_embedding.take() { local_result = embedding.restore_result(local_result); }
    if let Err(restore_error) = restore_engines() {
        local_result = Err(match local_result {
            Err(primary) => primary.context(format!("Renderer configuration restoration also failed: {restore_error:#}")),
            Ok(()) => restore_error,
        });
    }
    if let Err(mut error) = local_result {
        if let Some(stage) = document_stage.take() {
            let retained = stage.keep();
            error = error.context(format!("Renderer scratch retained for inspection: {}", retained.display()));
        }
        return Err(backup.restore(error));
    }
    let end_hooks = |transition: Option<&crate::git::DevTransition>| -> anyhow::Result<()> {
        for hooks in [&hooks_config.post, &hooks_config.both] {
            if let Some(hooks) = hooks {
                for hook in resolve_explicit_scripts(project_root, hooks)? {
                    execute_script(project_root, &hook, profile.as_deref(), resolved_python_cmd.as_deref(), &environment, execution)?;
                }
            }
        }
        // End hooks are immediately before the next development-version bump.
        if !is_dev {
            crate::path_safety::resolve_path(project_root.join("VERSION").as_std_path(), true)?;
            if let Some(transition) = transition { crate::git::record_dev_transition(project_root, &environment, transition)?; }
            version_set_at(project_root, &next_version.to_string(false))?;
        }
        Ok(())
    };
    let publish = !is_dev && (config.git.commit || !config.dest.is_empty());
    if !publish {
        let finish = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| end_hooks(None))) {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!("End hook panicked unexpectedly")),
        };
        return if is_dev { backup.restore_result(finish) } else {
            match finish { Ok(()) => Ok(()), Err(error) => Err(backup.restore(error)) }
        };
    }
    // Publication is irreversible. Make the successful local release explicit;
    // later publication/end-hook errors preserve it, never claim remote rollback.
    if let Err(error) = version_set_at(project_root, &initial_version.to_string(false)) { return Err(backup.restore(error)); }
    drop(backup);
    let publication = (|| -> anyhow::Result<()> {
        if let Some(remotes) = &config.remotes.local {
            for target in &config.dest {
                let remote = remotes.get(target).context("Validated publication target disappeared")?;
                for label in &remote.content {
                    let source = config.get_path(project_root, label)?;
                    if !source.is_dir() { anyhow::bail!("Publication source is not a directory: {source}"); }
                    crate::cas::ingest_directory(project_root, &remote.path, label, &source, &initial_version.to_string(false))?;
                }
            }
        }
        let transition = if config.git.commit {
            let version = initial_version.to_string(false);
            let message = match description {
                Some(description) if !description.trim().is_empty() => format!("Build v{version}: {}", description.trim()),
                _ => format!("Build v{version}"),
            };
            publish_git_in(project_root, &environment, config.config.git.engine, &message, push_target.as_ref())?;
            crate::git::capture_dev_transition(project_root, &environment, &initial_version, &next_version)?
        } else { None };
        end_hooks(transition.as_ref())
    })();
    publication.context("Local build succeeded; publication or end-hook finalization failed (local release preserved)")

}

fn execute_build_pipeline(
    project_root: &camino::Utf8Path,
    resolved_files: &[camino::Utf8PathBuf],
    explicit: bool,
    profile: Option<String>,
    quarto_exists: bool,
    bookdown_exists: bool,
    resolved_python_cmd: Option<&str>,
    environment: &RunEnvironment,
    execution: ExecutionDirectory,
    before_render: &impl Fn(Option<&camino::Utf8Path>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    if explicit {
        // Path A: Explicit Configuration via build.scripts


        // Mutual Exclusion Error (Rule 1 & 2)
        if quarto_exists && bookdown_exists {
            let has_docs = resolved_files.iter().any(|f| {
                let ext = f.extension().unwrap_or("");
                ext == "qmd" || ext == "Rmd"
            });
            if has_docs {
                anyhow::bail!("Mutual Exclusion Error: Both _quarto.yml and _bookdown.yml exist in the workspace, and the build.scripts key in _proj.yml is omitted or includes .qmd or .Rmd files. Aborting build.");
            }
        }

        if resolved_files.is_empty() {
            // The Explicit Empty Exception ("Don't Run Anything")
            return Ok(());
        }

        let mut doc_files = Vec::new();
        for file in resolved_files {
            let ext = file.extension().unwrap_or("");
            if ext == "qmd" || ext == "Rmd" {
                doc_files.push(file.clone());
            }
        }

        // Reordering constraint
        if quarto_exists || bookdown_exists {
            let mut pre_scripts = Vec::new();
            let mut post_scripts = Vec::new();
            let mut seen_doc = false;

            for file in resolved_files {
                let ext = file.extension().unwrap_or("");
                if ext == "qmd" || ext == "Rmd" {
                    seen_doc = true;
                } else if ext == "R" || ext == "py" {
                    if seen_doc {
                        post_scripts.push(file);
                    } else {
                        pre_scripts.push(file);
                    }
                }
            }

            // Execute pre-scripts
            for script in pre_scripts {
                let p = profile.clone();
                execute_script(project_root, &script, p.as_deref(), resolved_python_cmd, environment, execution)?;
            }

            // Execute doc clusters
            if quarto_exists && !doc_files.is_empty() {
                let p = profile.clone();
                before_render(None)?;
                execute_project_render(project_root, true, p.as_deref(), environment, execution)?;
            } else if bookdown_exists && !doc_files.is_empty() {
                let p = profile.clone();
                before_render(None)?;
                execute_project_render(project_root, false, p.as_deref(), environment, execution)?;
            }

            // Execute post-scripts
            for script in post_scripts {
                let p = profile.clone();
                execute_script(project_root, &script, p.as_deref(), resolved_python_cmd, environment, execution)?;
            }
        } else {
            // No project files, just execute in exact relative order
            for file in resolved_files {
                let p = profile.clone();
                if matches!(file.extension(), Some("qmd" | "Rmd" | "rmd")) { before_render(Some(file))?; }
                execute_script(project_root, &file, p.as_deref(), resolved_python_cmd, environment, execution)?;
            }
        }

    } else {
        // Mutual Exclusion Error (Rule 1 & 2)
        if quarto_exists && bookdown_exists {
            anyhow::bail!("Mutual Exclusion Error: Both _quarto.yml and _bookdown.yml exist in the workspace, and the build.scripts key in _proj.yml is omitted or includes .qmd or .Rmd files. Aborting build.");
        }

        // Path B: Fallback Automated Auto-Discovery
        if quarto_exists {
            before_render(None)?;
            execute_project_render(project_root, true, profile.as_deref(), environment, execution)?;
        } else if bookdown_exists {
            before_render(None)?;
            execute_project_render(project_root, false, profile.as_deref(), environment, execution)?;
        } else {

            for file in resolved_files {
                if matches!(file.extension(), Some("qmd" | "Rmd" | "rmd")) { before_render(Some(file))?; }
                execute_script(project_root, &file, profile.as_deref(), resolved_python_cmd, environment, execution)?;
            }
        }
    }

    Ok(())
}

// Use a consistent filesystem namespace for glob enumeration and exclusions only.
// The captured project-root identity remains canonical and unchanged.
fn glob_path_text(path: &camino::Utf8Path) -> anyhow::Result<String> {
    #[cfg(windows)]
    if let Some(std::path::Component::Prefix(prefix)) = path.as_std_path().components().next() {
        match prefix.kind() {
            std::path::Prefix::VerbatimDisk(_) => return Ok(path.as_str()[4..].to_string()),
            std::path::Prefix::VerbatimUNC(server, share) => {
                let tail = &path.as_str()[prefix.as_os_str().to_str().context("Non-UTF-8 glob prefix")?.len()..];
                return Ok(format!("\\\\{}\\{}{}", server.to_str().context("Non-UTF-8 UNC server")?, share.to_str().context("Non-UTF-8 UNC share")?, tail));
            }
            std::path::Prefix::Verbatim(_) | std::path::Prefix::DeviceNS(_) => anyhow::bail!("Unsupported device namespace for script glob selection"),
            _ => {}
        }
    }
    Ok(path.to_string())
}

fn script_glob_pattern(root: &camino::Utf8Path, pattern: &str) -> anyhow::Result<String> {
    if camino::Utf8Path::new(pattern).is_absolute() {
        return glob_path_text(camino::Utf8Path::new(pattern));
    }
    let root = glob_path_text(root)?;
    let prefix_len = {
        #[cfg(windows)]
        { match std::path::Path::new(&root).components().next() {
            Some(std::path::Component::Prefix(prefix)) => prefix.as_os_str().to_str().context("Non-UTF-8 glob prefix")?.len(),
            _ => 0,
        } }
        #[cfg(not(windows))]
        { 0 }
    };
    let escaped = format!("{}{}", &root[..prefix_len], glob::Pattern::escape(&root[prefix_len..]));
    Ok(camino::Utf8Path::new(&escaped).join(pattern).into_string())
}

/// Evaluates Path A: Explicit Configuration via `build.scripts`.
///
/// Supports exclusion globs starting with `!`, strictly matches root documents when no sub-directory
/// prefix is provided, and captures valid formats under sub-directories.
/// Malformed include or exclusion patterns return errors before execution; every
/// exclusion is validated even if no include selects a file.
pub(crate) fn resolve_explicit_scripts(project_root: &camino::Utf8Path, scripts: &[String]) -> anyhow::Result<Vec<camino::Utf8PathBuf>> {
    if scripts.is_empty() {
        return Ok(Vec::new()); // The Explicit Empty Exception ("Don't Run Anything")
    }

    let mut includes = Vec::new();
    let mut excludes = Vec::new();

    for script in scripts {
        if script.starts_with('!') {
            excludes.push(&script[1..]);
        } else {
            includes.push(script.clone());
        }
    }

    // Validate every exclusion before enumeration, including exclusion-only lists.
    let excludes = excludes.iter().map(|exclude| {
        let pattern = script_glob_pattern(project_root, exclude)?;
        glob::Pattern::new(&pattern)
            .with_context(|| format!("Invalid script/hook exclusion pattern '!{exclude}'"))
    }).collect::<anyhow::Result<Vec<_>>>()?;

    // Convert patterns to absolute globbing targets
    let mut matched_files = Vec::new();
    for pattern in &includes {
        let target_pattern = script_glob_pattern(project_root, pattern)?;

        // TODO: migrate to camino
        let paths = glob::glob(&target_pattern)
            .context(format!("Invalid glob pattern '{}'", target_pattern))?;

        let mut matched = false;
        for entry in paths {
            let path = entry.with_context(|| format!("Failed to read script glob '{pattern}'"))?;
            if path.is_file() {
                matched = true;
                matched_files.push(camino::Utf8PathBuf::try_from(path).context("Non-UTF-8 path encountered")?);
            }
        }
        if !matched { anyhow::bail!("Explicit script/hook pattern matched no files: {pattern}"); }
    }

    // Preserve first-match order while excluding and deduplicating selected files.
    let mut final_files = Vec::new();
    for file in matched_files {
        if !excludes.iter().any(|pattern| pattern.matches_path(file.as_std_path()))
            && !final_files.contains(&file)
        {
            final_files.push(file);
        }
    }

    // Keep the relative order of elements provided in build.scripts roughly based on include pattern order.
    // As multiple patterns might overlap, we just filter unique ordered by first match.
    Ok(final_files)
}

/// Dynamically modifies the `project.render` array in `_quarto.yml`.
fn rewrite_quarto_yml(project_root: &Path, qmd_files: &[camino::Utf8PathBuf]) -> anyhow::Result<()> {
    let qmd_paths: Vec<String> = qmd_files.iter()
        .map(|p| p.strip_prefix(project_root.to_string_lossy().to_string()).unwrap_or(p).to_string())
        .collect();

    let quarto_path = project_root.join("_quarto.yml");
    let content = crate::fs_utils::read_managed_text(&quarto_path).context("Failed to read _quarto.yml")?;

    let mut yaml: serde_yaml::Value = serde_yaml::from_str(&content).context("Failed to parse _quarto.yml")?;

    if let serde_yaml::Value::Mapping(ref mut map) = yaml {
        let proj_key = serde_yaml::Value::String("project".to_string());
        if !map.contains_key(&proj_key) {
            map.insert(proj_key.clone(), serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
        }

        if let Some(serde_yaml::Value::Mapping(proj_map)) = map.get_mut(&proj_key) {
            let render_key = serde_yaml::Value::String("render".to_string());

            let seq: Vec<serde_yaml::Value> = qmd_paths.into_iter().map(|s| serde_yaml::Value::String(s)).collect();
            proj_map.insert(render_key, serde_yaml::Value::Sequence(seq));
        }
    }

    let out_content = serde_yaml::to_string(&yaml).context("Failed to serialize _quarto.yml")?;
    crate::fs_utils::write_managed_file(&quarto_path, out_content).context("Failed to write _quarto.yml")?;
    Ok(())
}

/// Dynamically rewrites the output directory for external engines (`_quarto.yml` or `_bookdown.yml`).
pub(crate) fn rewrite_engine_output_dir(project_root: &Path, engine: &str, out_dir: &str) -> anyhow::Result<()> {
    if engine == "quarto" {
        let quarto_path = project_root.join("_quarto.yml");
        if !quarto_path.exists() {
            return Ok(());
        }
        let content = crate::fs_utils::read_managed_text(&quarto_path).context("Failed to read _quarto.yml")?;

        let mut yaml: serde_yaml::Value = serde_yaml::from_str(&content).context("Invalid renderer configuration")?;

        if !yaml.is_mapping() {
            anyhow::bail!("Renderer configuration must be a mapping");
        }

        if let serde_yaml::Value::Mapping(ref mut map) = yaml {
            let proj_key = serde_yaml::Value::String("project".to_string());
            if !map.contains_key(&proj_key) {
                map.insert(proj_key.clone(), serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
            }

            if let Some(serde_yaml::Value::Mapping(proj_map)) = map.get_mut(&proj_key) {
                let out_dir_key = serde_yaml::Value::String("output-dir".to_string());
                proj_map.insert(out_dir_key, serde_yaml::Value::String(out_dir.to_string()));
            }
        }

        let out_content = serde_yaml::to_string(&yaml).context("Failed to serialize _quarto.yml")?;
        crate::fs_utils::write_managed_file(&quarto_path, out_content).context("Failed to write _quarto.yml")?;

    } else if engine == "bookdown" {
        let bookdown_path = project_root.join("_bookdown.yml");
        if !bookdown_path.exists() {
            return Ok(());
        }
        let content = crate::fs_utils::read_managed_text(&bookdown_path).context("Failed to read _bookdown.yml")?;

        let mut yaml: serde_yaml::Value = serde_yaml::from_str(&content).context("Invalid renderer configuration")?;

        if !yaml.is_mapping() {
            anyhow::bail!("Renderer configuration must be a mapping");
        }

        if let serde_yaml::Value::Mapping(ref mut map) = yaml {
            let out_dir_key = serde_yaml::Value::String("output_dir".to_string());
            map.insert(out_dir_key, serde_yaml::Value::String(out_dir.to_string()));
        }

        let out_content = serde_yaml::to_string(&yaml).context("Failed to serialize _bookdown.yml")?;
        crate::fs_utils::write_managed_file(&bookdown_path, out_content).context("Failed to write _bookdown.yml")?;
    }

    Ok(())
}

/// Dynamically modifies the `rmd_files` array in `_bookdown.yml`.
fn rewrite_bookdown_yml(project_root: &Path, rmd_files: &[camino::Utf8PathBuf]) -> anyhow::Result<()> {
    let rmd_paths: Vec<String> = rmd_files.iter()
        .map(|p| p.strip_prefix(project_root.to_string_lossy().to_string()).unwrap_or(p).to_string())
        .collect();

    let bookdown_path = project_root.join("_bookdown.yml");
    let content = crate::fs_utils::read_managed_text(&bookdown_path).context("Failed to read _bookdown.yml")?;

    let mut yaml: serde_yaml::Value = serde_yaml::from_str(&content).context("Failed to parse _bookdown.yml")?;

    if let serde_yaml::Value::Mapping(ref mut map) = yaml {
        let key = serde_yaml::Value::String("rmd_files".to_string());
        let seq: Vec<serde_yaml::Value> = rmd_paths.into_iter().map(|s| serde_yaml::Value::String(s)).collect();
        map.insert(key, serde_yaml::Value::Sequence(seq));
    }

    let out_content = serde_yaml::to_string(&yaml).context("Failed to serialize _bookdown.yml")?;
    crate::fs_utils::write_managed_file(&bookdown_path, out_content).context("Failed to write _bookdown.yml")?;
    Ok(())
}

/// Evaluates Path B: Fallback Automated Auto-Discovery.
///
/// Sweeps for regular .qmd, .Rmd, .R, .py files in that strict order,
/// alphabetically within each kind. The sweep is shallow and does not follow
/// links. Directory/entry read errors and non-UTF-8 paths return errors rather
/// than becoming an empty or incomplete successful selection.
pub(crate) fn resolve_fallback_scripts(project_root: &camino::Utf8Path) -> anyhow::Result<Vec<camino::Utf8PathBuf>> {
    let entries = fs::read_dir(project_root)
        .with_context(|| format!("Cannot enumerate fallback scripts at {project_root}"))?;
    let mut qmd = Vec::new();
    let mut rmd = Vec::new();
    let mut r = Vec::new();
    let mut py = Vec::new();

    for entry in entries {
        let entry = entry.with_context(|| format!("Cannot read fallback script directory entry at {project_root}"))?;
        let file_type = entry.file_type()
            .with_context(|| format!("Cannot inspect fallback script entry {}", entry.path().display()))?;
        if !file_type.is_file() { continue; }
        let path = camino::Utf8PathBuf::try_from(entry.path()).context("Non-UTF-8 path encountered")?;
        match path.extension() {
            Some("qmd") => qmd.push(path),
            Some("Rmd") => rmd.push(path),
            Some("R") => r.push(path),
            Some("py") => py.push(path),
            _ => {}
        }
    }

    qmd.sort();
    rmd.sort();
    r.sort();
    py.sort();
    let mut files = qmd;
    files.extend(rmd);
    files.extend(r);
    files.extend(py);
    Ok(files)
}

use anyhow::Context;

/// Promotes one rendered R Markdown document after validating all destinations.
/// Missing outputs, unsafe paths and filesystem failures return errors before rename.
pub fn copy_individual_rmd(file_path: &camino::Utf8Path, docs_path: &camino::Utf8Path, project_root: &camino::Utf8Path) -> anyhow::Result<()> {
    copy_individual_document(file_path, docs_path, project_root)
}

/// Promotes one rendered Quarto document with the same guarded destination rules.
pub fn copy_individual_quarto(file_path: &camino::Utf8Path, docs_path: &camino::Utf8Path, project_root: &camino::Utf8Path) -> anyhow::Result<()> {
    copy_individual_document(file_path, docs_path, project_root)
}

// Without an explicit render list, conservatively inspect source directories for
// unsupported nested project roots. Declared resource trees and VCS are not sources.
fn validate_implicit_quarto_roots(root: &camino::Utf8Path, config: &crate::yml::ValidatedConfig) -> anyhow::Result<()> {
    let mut excluded = Vec::new();
    for label in config.directories.keys() {
        let directory = config.get_path(root, label)?;
        excluded.push(crate::path_safety::resolve_path(directory.as_std_path(), false)?);
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).context("Cannot inspect Quarto source directories")? {
            let entry = entry?;
            if matches!(entry.file_name().to_str(), Some(".git" | ".svn" | ".hg")) { continue; }
            let kind = entry.file_type()?;
            let path = camino::Utf8PathBuf::try_from(entry.path()).context("Non-UTF-8 Quarto source path")?;
            let identity = crate::path_safety::resolve_path(path.as_std_path(), false)?;
            if excluded.iter().any(|resource| identity.starts_with(resource)) { continue; }
            if kind.is_symlink() { anyhow::bail!("Implicit Quarto input discovery through linked entries is unsupported; select explicit source documents"); }
            if kind.is_dir() { pending.push(path); }
            else if directory != root && matches!(path.file_name(), Some("_quarto.yml" | "_quarto.yaml")) {
                anyhow::bail!("Nested Quarto project roots are unsupported; Quarto and proj roots must match");
            }
        }
    }
    Ok(())
}

fn validate_quarto_input(root: &camino::Utf8Path, input: &camino::Utf8Path) -> anyhow::Result<()> {
    // execute-dir controls evaluation cwd, not Quarto's ancestor project selection.
    let local_project = crate::yml::configuration_entry_exists(&root.join("_quarto.yml"))?
        || crate::yml::configuration_entry_exists(&root.join("_quarto.yaml"))?;
    if !local_project {
        for ancestor in root.ancestors().skip(1) {
            if crate::yml::configuration_entry_exists(&ancestor.join("_quarto.yml"))?
                || crate::yml::configuration_entry_exists(&ancestor.join("_quarto.yaml"))? {
                anyhow::bail!("Effective ancestor Quarto project root differs from the captured proj root");
            }
        }
    }
    let root = crate::path_safety::resolve_path(root.as_std_path(), false)?;
    let input = crate::path_safety::resolve_path(input.as_std_path(), true)?;
    if !input.starts_with(&root) { anyhow::bail!("Quarto inputs must lie inside the captured project root"); }
    let mut directory = input.parent().context("Quarto input has no parent")?;
    while directory != root.as_path() {
        for name in ["_quarto.yml", "_quarto.yaml"] {
            match fs::symlink_metadata(directory.join(name)) {
                Ok(_) => anyhow::bail!("Nested Quarto project roots are unsupported; Quarto and proj roots must match"),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("Cannot inspect Quarto root metadata"),
            }
        }
        directory = directory.parent().context("Quarto input root mismatch")?;
    }
    Ok(())
}

fn validate_individual_metadata(root: &camino::Utf8Path, input: &camino::Utf8Path) -> anyhow::Result<()> {
    let normalized_root = camino::Utf8PathBuf::try_from(crate::path_safety::resolve_path(root.as_std_path(), false)?)?;
    let normalized_input = camino::Utf8PathBuf::try_from(crate::path_safety::resolve_path(input.as_std_path(), true)?)?;
    let root = normalized_root.as_path();
    let input = normalized_input.as_path();
    if crate::yml::configuration_entry_exists(&root.join("_quarto.yaml"))? { anyhow::bail!("Individual rendering under _quarto.yaml project metadata is unsupported; use the modeled _quarto.yml project layout"); }
    let is_rmd = matches!(input.extension(), Some("Rmd" | "rmd"));
    let mut directory = input.parent();
    while let Some(parent) = directory {
        let names = if is_rmd { ["_output.yml", "_output.yaml"] } else { ["_metadata.yml", "_metadata.yaml"] };
        for name in names {
            if crate::yml::configuration_entry_exists(&parent.join(name))? { anyhow::bail!("Inherited individual-renderer metadata is unsupported; declare one format in the input document"); }
        }
        if parent == root { break; }
        directory = parent.parent();
    }
    Ok(())
}

struct DocumentOutputPaths {
    intermediate: Option<camino::Utf8PathBuf>,
    target: camino::Utf8PathBuf,
    destination: camino::Utf8PathBuf,
    assets: camino::Utf8PathBuf,
    destination_assets: camino::Utf8PathBuf,
}

fn document_output_paths(file_path: &camino::Utf8Path, docs_path: &camino::Utf8Path, project_root: &camino::Utf8Path) -> anyhow::Result<DocumentOutputPaths> {
    crate::path_safety::validate_document_destination(project_root, file_path, docs_path)?;
    let content = crate::fs_utils::read_managed_text(file_path).context("Failed to read document input")?;
    validate_individual_metadata(project_root, file_path)?;
    let (format, output_file) = crate::build_pre::individual_output_options(&content, matches!(file_path.extension(), Some("Rmd" | "rmd")))?;
    let ext = crate::build_pre::map_format_to_extension(Some(&format))?;
    let stem = file_path.file_stem().context("Document input has no filename stem")?;
    let parent = file_path.parent().unwrap_or(project_root);
    let name = output_file.unwrap_or_else(|| format!("{stem}.{ext}"));
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains(':') || name == "." || name == ".." {
        anyhow::bail!("Individual output-file must be a leaf filename");
    }
    let lower_name = name.to_ascii_lowercase();
    if ["version", "description", ".git", ".svn", ".gitignore", ".rbuildignore", ".rprofile", ".renviron", "pyproject.toml", "project.toml", "juliaproject.toml", "renv.lock", "_bookdown.yml", "_output.yml"].contains(&lower_name.as_str())
        || ((lower_name.starts_with("_proj") || lower_name.starts_with("_quarto")) && (lower_name.ends_with(".yml") || lower_name.ends_with(".yaml")))
        || lower_name == "_environment" || lower_name.starts_with("_environment.") || lower_name.starts_with("_environment-")
    {
        anyhow::bail!("Individual output-file must not replace framework configuration/state");
    }
    if std::path::Path::new(&name).extension().is_none() { anyhow::bail!("Individual output filenames require an explicit extension"); }
    let target = parent.join(&name);
    let target_identity = crate::path_safety::resolve_path(target.as_std_path(), true)?;
    if target.exists() && !target.is_file() { anyhow::bail!("Expected document output path is not a file"); }
    let destination = docs_path.join(&name);
    if target_identity == crate::path_safety::resolve_path(file_path.as_std_path(), true)?
        || target_identity == crate::path_safety::resolve_path(destination.as_std_path(), true)? {
        anyhow::bail!("Document output aliases the input or promotion destination");
    }
    let assets = parent.join(format!("{stem}_files"));
    let destination_assets = docs_path.join(format!("{stem}_files"));
    crate::path_safety::resolve_path(assets.as_std_path(), true)?;
    crate::path_safety::resolve_path(destination_assets.as_std_path(), true)?;
    if assets.exists() { crate::path_safety::validate_directory_move(&assets, &destination_assets)?; }
    // html_notebook first writes ordinary HTML, then renames it in its native
    // postprocessor. That transient sibling is managed too, even on success.
    let intermediate = if format == "html_notebook" {
        let path = parent.join(format!("{stem}.html"));
        crate::path_safety::resolve_path(path.as_std_path(), true)?;
        anyhow::ensure!(!path.exists() || path.is_file(), "Notebook intermediate must be a regular file or absent");
        Some(path)
    } else { None };
    Ok(DocumentOutputPaths { intermediate, target, destination, assets, destination_assets })
}

fn copy_individual_document(file_path: &camino::Utf8Path, docs_path: &camino::Utf8Path, project_root: &camino::Utf8Path) -> anyhow::Result<()> {
    let paths = document_output_paths(file_path, docs_path, project_root)?;
    if !paths.target.is_file() || fs::metadata(&paths.target)?.len() == 0 { anyhow::bail!("Expected rendered output is missing or empty: {}", paths.target); }
    fs::create_dir_all(docs_path).context("Failed to create document destination")?;
    fs::rename(&paths.target, &paths.destination).with_context(|| format!("Failed to move rendered output {}", paths.target))?;
    if paths.assets.exists() { crate::fs_utils::dir_move_exact(&paths.assets, &paths.destination_assets)?; }
    Ok(())
}

// Project render success requires document bytes, not merely an empty staging tree.
fn validate_project_artifacts(directory: &camino::Utf8Path) -> anyhow::Result<()> {
    fn contains_document(directory: &std::path::Path) -> anyhow::Result<bool> {
        crate::path_safety::resolve_path(directory, true)?;
        let mut found = false;
        for entry in fs::read_dir(directory).context("Cannot inspect project render artifacts")? {
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() { anyhow::bail!("Project render artifacts contain a symlink"); }
            if metadata.is_dir() { found |= contains_document(&path)?; }
            else if metadata.is_file() && metadata.len() > 0 {
                found |= matches!(path.extension().and_then(|value| value.to_str()), Some("html" | "htm" | "pdf" | "docx" | "epub" | "pptx"));
            } else if !metadata.is_file() { anyhow::bail!("Unsupported project render artifact"); }
        }
        Ok(found)
    }
    if !contains_document(directory.as_std_path())? {
        anyhow::bail!("Project renderer produced no supported nonempty document artifacts");
    }
    Ok(())
}

fn bookdown_asset_paths(root: &camino::Utf8Path, docs: &camino::Utf8Path) -> anyhow::Result<(camino::Utf8PathBuf, camino::Utf8PathBuf)> {
    let path = root.join("_bookdown.yml");
    let content = crate::fs_utils::read_managed_text(&path).context("Cannot read Bookdown configuration")?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&content).context("Invalid Bookdown configuration")?;
    let name = match yaml.get("book_filename") {
        Some(value) => value.as_str().context("Bookdown book_filename must be a string")?,
        None => "_main",
    };
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') || name.contains(':') {
        anyhow::bail!("Bookdown book_filename must be a nonempty portable filename stem");
    }
    let leaf = format!("{name}_files");
    let source = root.join(&leaf);
    let destination = docs.join(&leaf);
    crate::path_safety::resolve_path(source.as_std_path(), true)?;
    crate::path_safety::resolve_path(destination.as_std_path(), true)?;
    if source.exists() { crate::path_safety::validate_directory_move(&source, &destination)?; }
    Ok((source, destination))
}

pub fn copy_global_bookdown(cache_docs_dir: &camino::Utf8Path, final_docs_dir: &camino::Utf8Path, project_root: &camino::Utf8Path) -> anyhow::Result<()> {
    validate_project_artifacts(cache_docs_dir)?;
    let (files_dir, dest_files_dir) = bookdown_asset_paths(project_root, final_docs_dir)?;
    crate::fs_utils::dir_move_exact(cache_docs_dir, final_docs_dir)?;
    if files_dir.exists() {
        crate::fs_utils::dir_move_exact(&files_dir, &dest_files_dir)?;
    }

    Ok(())
}

pub fn copy_global_quarto_project(cache_docs_dir: &camino::Utf8Path, final_docs_dir: &camino::Utf8Path) -> anyhow::Result<()> {
    validate_project_artifacts(cache_docs_dir)?;
    crate::fs_utils::dir_move_exact(cache_docs_dir, final_docs_dir)?;
    Ok(())
}

/// Executes a single script in an isolated subprocess.
fn execute_script(project_root: &camino::Utf8Path, script_path: &camino::Utf8Path, profile: Option<&str>, resolved_python_cmd: Option<&str>, environment: &RunEnvironment, execution: ExecutionDirectory) -> anyhow::Result<()> {
    let ext = script_path.extension().unwrap_or("");
    let mut render_context = None;
    let mut cmd = match ext {
        "R" | "r" => {
            let mut c = environment.command("Rscript");
            c.arg(script_path.as_std_path());
            c
        },
        "py" => {
            let python_exe = resolved_python_cmd.unwrap_or("python");
            let mut c = environment.command(python_exe);
            c.arg(script_path.as_std_path());
            c
        },
        "qmd" => {
            render_context = Some(crate::run_context::ContextFile::for_render(environment.value(crate::RUN_CONTEXT_ENV).context("Missing managed render context")?, crate::run_context::RenderEngine::Quarto, Some(script_path))?);
            let mut c = environment.command("quarto");
            c.arg("render").arg(script_path.as_std_path());
            let content = crate::fs_utils::read_managed_text(script_path)?;
            if crate::embedding::individual_fallback(&content, false)? { c.arg("--embed-resources"); }
            if execution == ExecutionDirectory::Project { c.arg("--execute-dir").arg(project_root.as_std_path()); }
            c
        },
        "Rmd" | "rmd" => {
            render_context = Some(crate::run_context::ContextFile::for_render(environment.value(crate::RUN_CONTEXT_ENV).context("Missing managed render context")?, crate::run_context::RenderEngine::RMarkdown, Some(script_path))?);
            let content = crate::fs_utils::read_managed_text(script_path)?;
            let (format, _) = crate::build_pre::individual_output_options(&content, true)?;
            let extension = crate::build_pre::map_format_to_extension(Some(&format))?;
            let output_name = format!("{}.{}", script_path.file_stem().context("Document input has no filename stem")?, if format == "html_notebook" { "html" } else { extension.as_str() });
            let mut c = environment.command("Rscript");
            // rmarkdown sanitizes shell characters in input basenames by copying
            // into its intermediates directory. Pin that directory to the source
            // parent and the expected output leaf, retaining native document cwd
            // even when the driver was launched elsewhere. All filenames are data.
            c.arg("-e").arg("args <- commandArgs(trailingOnly = TRUE); rmarkdown::render(input = args[[1]], output_file = args[[4]], output_dir = dirname(args[[1]]), intermediates_dir = dirname(args[[1]]), knit_root_dir = if (args[[2]] == 'project') args[[3]] else NULL, output_options = if (args[[5]] == 'embed') list(self_contained = TRUE) else NULL)")
                .arg(script_path.as_std_path()).arg(if execution == ExecutionDirectory::Project { "project" } else { "native" }).arg(project_root.as_std_path()).arg(output_name).arg(if crate::embedding::individual_fallback(&content, true)? { "embed" } else { "native" });
            c
        },
        _ => {
            anyhow::bail!("Unsupported script extension for execution: {}", script_path);
        }
    };

    if environment.non_interactive() {
        cmd.stdin(std::process::Stdio::null());
    }
    cmd.env(crate::project::PROJECT_ROOT_ENV, project_root.as_str());
    if let Some(p) = profile {
        cmd.env("PROJR_PROFILE", p);
    }

    if let Some(context) = &render_context { cmd.env(crate::RUN_CONTEXT_ENV, context.path()); }
    let status = cmd.status().context(format!("Failed to execute script {}", script_path))?;
    if !status.success() {
        anyhow::bail!("Script {} failed with status: {}", script_path, status);
    }

    Ok(())
}

/// Executes a quarto or bookdown project render.
fn execute_project_render(project_root: &camino::Utf8Path, is_quarto: bool, profile: Option<&str>, environment: &RunEnvironment, execution: ExecutionDirectory) -> anyhow::Result<()> {
    let render_context = crate::run_context::ContextFile::for_render(environment.value(crate::RUN_CONTEXT_ENV).context("Missing managed render context")?,
        if is_quarto { crate::run_context::RenderEngine::Quarto } else { crate::run_context::RenderEngine::Bookdown }, None)?;
    let mut cmd = if is_quarto {
        let mut c = environment.command("quarto");
        c.arg("render");
        if execution == ExecutionDirectory::Project { c.arg("--execute-dir").arg(project_root.as_std_path()); }
        c
    } else {
        let mut c = environment.command("Rscript");
        c.arg("-e").arg("rmarkdown::render_site()");
        c
    };

    // TODO: migrate to camino
    cmd.current_dir(project_root.as_std_path());
    if environment.non_interactive() {
        cmd.stdin(std::process::Stdio::null());
    }
    cmd.env(crate::project::PROJECT_ROOT_ENV, project_root.as_str());
    if let Some(p) = profile {
        cmd.env("PROJR_PROFILE", p);
    }

    cmd.env(crate::RUN_CONTEXT_ENV, render_context.path());
    let status = cmd.status().context("Failed to execute project render")?;
    if !status.success() {
        anyhow::bail!("Project render failed with status: {}", status);
    }

    Ok(())
}


#[cfg(all(test, windows))]
mod glob_namespace_tests {
    use super::{glob_path_text, script_glob_pattern};
    use camino::Utf8Path;

    #[test]
    fn canonical_disk_and_unc_prefixes_are_filesystem_syntax_not_glob_tokens() {
        assert_eq!(glob_path_text(Utf8Path::new(r"\\?\C:\work [1]")).unwrap(), r"C:\work [1]");
        assert_eq!(glob_path_text(Utf8Path::new(r"\\?\UNC\server\share\work [1]")).unwrap(), r"\\server\share\work [1]");
        let pattern = script_glob_pattern(Utf8Path::new(r"\\?\C:\work [1]"), "*.py").unwrap();
        assert!(pattern.starts_with(r"C:\"));
        assert!(pattern.contains(&glob::Pattern::escape("work [1]")));
        assert!(pattern.ends_with(r"\*.py"));
    }
}


#[cfg(all(test, unix))]
mod publication_sync_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn post_build_sync_refuses_a_commit_hooks_endpoint_change() {
        let project = tempfile::tempdir().unwrap();
        let root_path = project.path().canonicalize().unwrap();
        let root = camino::Utf8Path::from_path(&root_path).unwrap();
        fs::create_dir(root.join("bin")).unwrap();
        let fake = r#"#!/bin/sh
case "$1" in
--version) exit 0 ;;
symbolic-ref) echo feature ;;
check-ref-format) exit 0 ;;
config)
 for key do :; done
 case "$key" in
  *.remote) echo upstream ;;
  *.merge) echo refs/heads/release ;;
  *.mirror) echo false ;;
  *) exit 1 ;;
 esac ;;
remote)
 if [ -f endpoint-changed ]; then echo https://other.example/changed.git; else echo https://github.com/example/repo.git; fi ;;
add) printf 'add\n' >> events ;;
diff) exit 1 ;;
commit) printf 'commit\n' >> events; : > endpoint-changed ;;
push) printf 'push\n' >> events ;;
esac
"#;
        let git = root.join("bin/git"); fs::write(&git, fake).unwrap();
        fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
        let mut environment = RunEnvironment::inherited(); environment.set("PATH", root.join("bin").as_str());
        let result = post_build_sync_in(&GlobalConfig::default(), root, &environment);
        assert!(result.is_err());
        assert!(format!("{:#}", result.unwrap_err()).contains("changed"));
        assert_eq!(fs::read_to_string(root.join("events")).unwrap(), "add\ncommit\n");
    }
}

fn remove_native_artifact(path: &camino::Utf8Path) -> anyhow::Result<()> {
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => fs::remove_file(path)?,
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path)?,
        Ok(_) => anyhow::bail!("Unsupported native render artifact: {path}"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn stage_frozen_document(target: &camino::Utf8Path, assets: &camino::Utf8Path, staging: &camino::Utf8Path) -> anyhow::Result<()> {
    crate::path_safety::resolve_path(target.as_std_path(), true)?;
    anyhow::ensure!(target.is_file() && fs::metadata(target)?.len() > 0, "Expected rendered output is missing or empty: {target}");
    let destination = staging.join(target.file_name().context("Missing document output leaf")?);
    crate::path_safety::resolve_path(destination.as_std_path(), true)?;
    fs::copy(target, destination)?;
    if assets.exists() { crate::fs_utils::copy_dir_recursive(assets, &staging.join(assets.file_name().context("Missing document asset leaf")?))?; }
    Ok(())
}
