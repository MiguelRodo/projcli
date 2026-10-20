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
use crate::git::{is_git_installed, check_git_profile, create_git_provider};
use crate::yml::GlobalConfig;
use crate::build_pre::run_pre_flight_checks;

pub fn post_build_sync(config: &GlobalConfig, repo_dir: camino::Utf8PathBuf) -> anyhow::Result<()> {
    let provider = create_git_provider(config.git.engine, repo_dir)?;

    println!("Staging build artifacts and committing mutations...");
    provider.commit_all("chore: automated workspace build update [compiled asset tracking]")?;

    println!("Pushing local branch mutations to remote host...");
    provider.push("origin", "main")?;

    Ok(())
}

pub fn build_project(project_root: &camino::Utf8Path, mode: BuildMode, cli_profile: Option<&str>, description: Option<&str>) -> anyhow::Result<()> {
    let resolved_root = crate::project::resolve_project_root(Some(project_root))?;
    let project_root = resolved_root.as_path();
    let is_dev = mode == BuildMode::Dev;
    let is_prod_run = matches!(mode, BuildMode::ProdMajor | BuildMode::ProdMinor | BuildMode::ProdPatch);

    // ==========================================
    // STEP A: Activate Environment Guard
    // ==========================================
    // Load inputs without writing ignores; configuration errors must stay read-only.
    let _env_guard = crate::env::EnvGuard::activate_for_preview(cli_profile, project_root)
        .context("Failed to activate environment guard")?;

    // ==========================================
    // STEP B: Pre-Build Validation & Execution
    // ==========================================

    // 1. Config Audit
    crate::yml::require_active_configuration(project_root)?;
    let combined = crate::yml::get_combined_yml(None, project_root)
        .context("Failed to read project configuration")?;
    let project_config: ProjConfig = serde_json::from_value(combined)
        .context("Failed to deserialize combined configuration")?;
    let config = project_config.validate_and_resolve(project_root, is_dev)?;


    // 2. Git Capability Audit
    if config.git.commit {
        if !is_git_installed() {
            anyhow::bail!("Git is required for commit but not found on system PATH");
        }
        // TODO: migrate to camino
        check_git_profile(Some(project_root)).context("Failed to check Git profile capability")?;
    }

    // 3. Resolve configs and hooks
    let quarto_exists = project_root.join("_quarto.yml").exists();
    let bookdown_exists = project_root.join("_bookdown.yml").exists();

    let build = project_config.build;
    let dev = project_config.dev;
    let profile = cli_profile.map(str::to_string).or(build.profile);
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

    let cache_root = if (!is_dev && run_analysis) || quarto_exists || bookdown_exists {
        Some(config.get_path(project_root, "cache").context("Selected cleanup or project rendering requires a declared cache directory")?)
    } else { None };
    let has_document_analysis = quarto_exists || bookdown_exists || resolved_files.iter().any(|file| matches!(file.extension(), Some("qmd" | "Rmd" | "rmd")));
    if has_document_analysis { config.get_path(project_root, "docs").context("Document rendering requires a declared docs directory")?; }
    let mut validation_files = resolved_files.clone();
    validation_files.extend(resolved_hooks.clone());
    // 4. VERSION Initialization Check
    let mut initial_version = match version_get_from(project_root) {
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

    let current_version = initial_version.to_string(false);
    if !is_dev {
        crate::path_safety::validate_build_paths(project_root, &config, &validation_files)?;
        if run_analysis {
            crate::path_safety::validate_cleanup_paths(project_root, &config, &current_version)?;
            if quarto_exists || bookdown_exists {
                crate::path_safety::validate_project_source_overlap(project_root, &config)?;
            } else {
                for file in &resolved_files {
                    if matches!(file.extension(), Some("qmd" | "Rmd" | "rmd")) {
                        let outputs = document_output_paths(file, &config.get_path(project_root, "docs")?, project_root)?;
                        for input in &validation_files {
                            let identity = crate::path_safety::resolve_path(input.as_std_path(), false)?;
                            for target in [&outputs.target, &outputs.destination, &outputs.assets, &outputs.destination_assets] {
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

    let (_resolved_token, resolved_python_cmd) = run_pre_flight_checks(
        project_root,
        &config,
        is_prod_run,
        &validation_files,
        quarto_exists,
        bookdown_exists,
    )?;

    // Capture exact local state before the first controlled project mutation.
    let mut managed = vec![];
    for name in ["VERSION", ".gitignore", ".Rbuildignore", "_proj.yml", "_projr.yml", "_projr-local.yml", "DESCRIPTION", "_quarto.yml", "_bookdown.yml"] {
        managed.push(project_root.join(name).into_std_path_buf());
    }
    for entry in fs::read_dir(project_root)? {
        let entry = entry?;
        if entry.file_type()?.is_file() { managed.push(entry.path()); }
    }
    for (label, _) in &config.directories {
        let lower = label.to_ascii_lowercase();
        if lower.starts_with("output") || lower.starts_with("docs") {
            managed.push(config.get_path(project_root, label)?.into_std_path_buf());
        }
    }
    let root_identity = crate::path_safety::resolve_path(project_root.as_std_path(), false)?;
    for input in &validation_files {
        // Reject links along the original path before canonicalizing its namespace.
        let identity = crate::path_safety::resolve_path(input.as_std_path(), true)?;
        if identity.starts_with(&root_identity) { managed.push(identity); }
    }
    if bookdown_exists {
        let (assets, destination) = bookdown_asset_paths(project_root, &config.get_path(project_root, "docs")?)?;
        let asset_identity = crate::path_safety::resolve_path(assets.as_std_path(), true)?;
        for input in &validation_files {
            if crate::path_safety::resolve_path(input.as_std_path(), true)?.starts_with(&asset_identity) {
                anyhow::bail!("Bookdown assets contain a selected input");
            }
        }
        managed.push(assets.into_std_path_buf());
        managed.push(destination.into_std_path_buf());
    }
    if has_document_analysis && !quarto_exists && !bookdown_exists {
        let docs = config.get_path(project_root, "docs")?;
        for file in &resolved_files {
            if matches!(file.extension(), Some("qmd" | "Rmd" | "rmd")) {
                let paths = document_output_paths(file, &docs, project_root)?;
                managed.extend([paths.target.into_std_path_buf(), paths.assets.into_std_path_buf()]);
            }
        }
    }
    let backup = if !is_dev {
        Some(crate::build_state::BuildState::capture(cache_root.as_ref().map(|path| path.as_std_path()), &managed)?)
    } else { None };
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
        for (path, bytes) in &engine_originals { crate::path_safety::resolve_path(path.as_std_path(), true)?; fs::write(path, bytes).with_context(|| format!("Cannot restore exact renderer configuration {path}"))?; }
        Ok(())
    };
    let local_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> anyhow::Result<()> {
        if !is_dev {
            if project_root.join("_environment.local").exists() {
                crate::ignore::add_manual_ignores(project_root, &["_environment.local".to_string()], true, crate::ignore::IgnoreType::All)?;
            }
            crate::ignore::update_ignores(project_root, &config)?;
        }
        let clear_output_env = std::env::var("PROJR_CLEAR_OUTPUT").ok();
        let clear_output_val = clear_output_env.as_deref().or(config.clear_output.as_deref());
        if !is_dev && run_analysis {
            crate::clear::clear_old(project_root, &config, &current_version, false, config.old_dev_remove)?;
            crate::clear::clear_pre(project_root, &current_version, &config, clear_output_val)?;
        }
        if !is_dev {
            match mode {
                BuildMode::ProdMajor => { initial_version.major += 1; initial_version.minor = 0; initial_version.patch = 0; }
                BuildMode::ProdMinor => { initial_version.minor += 1; initial_version.patch = 0; }
                BuildMode::ProdPatch => { initial_version.patch += 1; }
                BuildMode::Dev => {}
            }
            initial_version.dev = 0;
        }
        if quarto_exists || bookdown_exists {
            let staging = cache_root.as_ref().context("Project rendering requires cache")?.join("projr").join(&current_version).join("docs");
            if quarto_exists { rewrite_engine_output_dir(project_root.as_std_path(), "quarto", staging.as_str())?; }
            if bookdown_exists { rewrite_engine_output_dir(project_root.as_std_path(), "bookdown", staging.as_str())?; }
        }
        for hooks in [&hooks_config.pre, &hooks_config.both] {
            if let Some(hooks) = hooks {
                for hook in resolve_explicit_scripts(project_root, hooks)? {
                    execute_script(project_root, &hook, profile.as_deref(), resolved_python_cmd.as_deref())?;
                }
            }
        }
        // Artifacts from a previous run or pre-hook cannot prove renderer success.
        if !is_dev && has_document_analysis {
            if quarto_exists || bookdown_exists {
                let staging = cache_root.as_ref().context("Project rendering requires cache")?.join("projr").join(&current_version).join("docs");
                crate::path_safety::resolve_path(staging.as_std_path(), true)?;
                if staging.exists() { fs::remove_dir_all(&staging)?; }
            } else {
                let docs = config.get_path(project_root, "docs")?;
                for file in &resolved_files {
                    if matches!(file.extension(), Some("qmd" | "Rmd" | "rmd")) {
                        let paths = document_output_paths(file, &docs, project_root)?;
                        if paths.target.exists() { fs::remove_file(&paths.target)?; }
                        if paths.assets.exists() { fs::remove_dir_all(&paths.assets)?; }
                    }
                }
            }
        }
        execute_build_pipeline(project_root, build_scripts.clone(), profile.clone(), quarto_exists, bookdown_exists, resolved_python_cmd.as_deref())?;
        restore_engines()?;
        if !is_dev && run_analysis && has_document_analysis && config.output_run.unwrap_or(true) {
            let docs = config.get_path(project_root, "docs")?;
            crate::clear::clear_post(project_root, false, &config, Some("never"), !quarto_exists && !bookdown_exists)?;
            if quarto_exists || bookdown_exists {
                let staging = cache_root.as_ref().context("Project rendering requires cache")?.join("projr").join(&current_version).join("docs");
                if quarto_exists { copy_global_quarto_project(&staging, &docs)?; }
                else { copy_global_bookdown(&staging, &docs, project_root)?; }
            } else {
                for file in &resolved_files {
                    match file.extension() {
                        Some("qmd") => copy_individual_quarto(file, &docs, project_root)?,
                        Some("Rmd" | "rmd") => copy_individual_rmd(file, &docs, project_root)?,
                        _ => {}
                    }
                }
            }
        }
        for hooks in [&hooks_config.post, &hooks_config.both] {
            if let Some(hooks) = hooks {
                for hook in resolve_explicit_scripts(project_root, hooks)? {
                    execute_script(project_root, &hook, profile.as_deref(), resolved_python_cmd.as_deref())?;
                }
            }
        }
        // VERSION is authoritative only after hooks, rendering and promotion succeeded.
        if !is_dev {
            crate::path_safety::resolve_path(project_root.join("VERSION").as_std_path(), true)?;
            version_set_at(project_root, &initial_version.to_string(false))?;
        }
        Ok(())
    }));
    let mut local_result = match local_result {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!("Build pipeline panicked unexpectedly")),
    };
    if let Err(restore_error) = restore_engines() {
        local_result = Err(match local_result {
            Err(primary) => primary.context(format!("Renderer configuration restoration also failed: {restore_error:#}")),
            Ok(()) => restore_error,
        });
    }
    if let Err(error) = local_result {
        return Err(match backup { Some(backup) => backup.restore(error), None => error });
    }
    drop(backup); // Local release is complete. Publication cannot undo that success.
    if is_dev { return Ok(()); }
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
        // Export the built bytes before requested post-output cleanup.
        if run_analysis {
            let clear_output_env = std::env::var("PROJR_CLEAR_OUTPUT").ok();
            crate::clear::clear_post(project_root, false, &config, clear_output_env.as_deref().or(config.clear_output.as_deref()), false)?;
        }
        if config.git.commit {
            let version = initial_version.to_string(false);
            let message = match description {
                Some(description) if !description.trim().is_empty() => format!("Build v{version}: {}", description.trim()),
                _ => format!("Build v{version}"),
            };
            let provider = create_git_provider(config.config.git.engine, project_root.to_path_buf())?;
            provider.commit_all(&message)?;
            if config.git.push { provider.push("origin", "main")?; }
        }
        Ok(())
    })();
    publication.context("Local build succeeded; publication or post-publication cleanup failed")
}

fn execute_build_pipeline(
    project_root: &camino::Utf8Path,
    build_scripts: Option<Vec<String>>,
    profile: Option<String>,
    quarto_exists: bool,
    bookdown_exists: bool,
    resolved_python_cmd: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(scripts) = build_scripts {
        // Path A: Explicit Configuration via build.scripts
        let resolved_files = resolve_explicit_scripts(project_root, &scripts)?;

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
        for file in &resolved_files {
            let ext = file.extension().unwrap_or("");
            if ext == "qmd" || ext == "Rmd" {
                doc_files.push(file.clone());
            }
        }

        if quarto_exists && !doc_files.is_empty() {
            rewrite_quarto_yml(project_root.as_std_path(), &doc_files)?;
        } else if bookdown_exists && !doc_files.is_empty() {
            rewrite_bookdown_yml(project_root.as_std_path(), &doc_files)?;
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
                execute_script(project_root, &script, p.as_deref(), resolved_python_cmd)?;
            }

            // Execute doc clusters
            if quarto_exists && !doc_files.is_empty() {
                let p = profile.clone();
                execute_project_render(project_root, true, p.as_deref())?;
            } else if bookdown_exists && !doc_files.is_empty() {
                let p = profile.clone();
                execute_project_render(project_root, false, p.as_deref())?;
            }

            // Execute post-scripts
            for script in post_scripts {
                let p = profile.clone();
                execute_script(project_root, &script, p.as_deref(), resolved_python_cmd)?;
            }
        } else {
            // No project files, just execute in exact relative order
            for file in resolved_files {
                let p = profile.clone();
                execute_script(project_root, &file, p.as_deref(), resolved_python_cmd)?;
            }
        }

    } else {
        // Mutual Exclusion Error (Rule 1 & 2)
        if quarto_exists && bookdown_exists {
            anyhow::bail!("Mutual Exclusion Error: Both _quarto.yml and _bookdown.yml exist in the workspace, and the build.scripts key in _proj.yml is omitted or includes .qmd or .Rmd files. Aborting build.");
        }

        // Path B: Fallback Automated Auto-Discovery
        if quarto_exists {
            execute_project_render(project_root, true, profile.as_deref())?;
        } else if bookdown_exists {
            execute_project_render(project_root, false, profile.as_deref())?;
        } else {
            let resolved_files = resolve_fallback_scripts(project_root)?;
            for file in resolved_files {
                execute_script(project_root, &file, profile.as_deref(), resolved_python_cmd)?;
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

    // Process excludes
    let mut final_files = Vec::new();
    for file in matched_files {
        let mut is_excluded = false;

        for exclude in &excludes {
            let exclude_pattern = script_glob_pattern(project_root, exclude)?;

            // TODO: migrate to camino
            if let Ok(ex_pattern) = glob::Pattern::new(&exclude_pattern) {
                if ex_pattern.matches_path(file.as_std_path()) {
                    is_excluded = true;
                    break;
                }
            }
        }

        if !is_excluded {
            if !final_files.contains(&file) {
                final_files.push(file);
            }
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
    let content = fs::read_to_string(&quarto_path).context("Failed to read _quarto.yml")?;

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
    fs::write(&quarto_path, out_content).context("Failed to write _quarto.yml")?;
    Ok(())
}

/// Dynamically rewrites the output directory for external engines (`_quarto.yml` or `_bookdown.yml`).
pub(crate) fn rewrite_engine_output_dir(project_root: &Path, engine: &str, out_dir: &str) -> anyhow::Result<()> {
    if engine == "quarto" {
        let quarto_path = project_root.join("_quarto.yml");
        if !quarto_path.exists() {
            return Ok(());
        }
        let content = fs::read_to_string(&quarto_path).context("Failed to read _quarto.yml")?;

        let mut yaml: serde_yaml::Value = serde_yaml::from_str(&content).unwrap_or(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));

        if !yaml.is_mapping() {
            yaml = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
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
        fs::write(&quarto_path, out_content).context("Failed to write _quarto.yml")?;

    } else if engine == "bookdown" {
        let bookdown_path = project_root.join("_bookdown.yml");
        if !bookdown_path.exists() {
            return Ok(());
        }
        let content = fs::read_to_string(&bookdown_path).context("Failed to read _bookdown.yml")?;

        let mut yaml: serde_yaml::Value = serde_yaml::from_str(&content).unwrap_or(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));

        if !yaml.is_mapping() {
            yaml = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
        }

        if let serde_yaml::Value::Mapping(ref mut map) = yaml {
            let out_dir_key = serde_yaml::Value::String("output_dir".to_string());
            map.insert(out_dir_key, serde_yaml::Value::String(out_dir.to_string()));
        }

        let out_content = serde_yaml::to_string(&yaml).context("Failed to serialize _bookdown.yml")?;
        fs::write(&bookdown_path, out_content).context("Failed to write _bookdown.yml")?;
    }

    Ok(())
}

/// Dynamically modifies the `rmd_files` array in `_bookdown.yml`.
fn rewrite_bookdown_yml(project_root: &Path, rmd_files: &[camino::Utf8PathBuf]) -> anyhow::Result<()> {
    let rmd_paths: Vec<String> = rmd_files.iter()
        .map(|p| p.strip_prefix(project_root.to_string_lossy().to_string()).unwrap_or(p).to_string())
        .collect();

    let bookdown_path = project_root.join("_bookdown.yml");
    let content = fs::read_to_string(&bookdown_path).context("Failed to read _bookdown.yml")?;

    let mut yaml: serde_yaml::Value = serde_yaml::from_str(&content).context("Failed to parse _bookdown.yml")?;

    if let serde_yaml::Value::Mapping(ref mut map) = yaml {
        let key = serde_yaml::Value::String("rmd_files".to_string());
        let seq: Vec<serde_yaml::Value> = rmd_paths.into_iter().map(|s| serde_yaml::Value::String(s)).collect();
        map.insert(key, serde_yaml::Value::Sequence(seq));
    }

    let out_content = serde_yaml::to_string(&yaml).context("Failed to serialize _bookdown.yml")?;
    fs::write(&bookdown_path, out_content).context("Failed to write _bookdown.yml")?;
    Ok(())
}

/// Evaluates Path B: Fallback Automated Auto-Discovery
///
/// Sweeps for `.qmd`, `.Rmd`, `.R`, `.py` in that strict order.
pub(crate) fn resolve_fallback_scripts(project_root: &camino::Utf8Path) -> anyhow::Result<Vec<camino::Utf8PathBuf>> {
    let mut files = Vec::new();

    // Shallow directory sweep
    // TODO: migrate to camino
    if let Ok(entries) = fs::read_dir(project_root.as_std_path()) {
        let mut qmd = Vec::new();
        let mut rmd = Vec::new();
        let mut r = Vec::new();
        let mut py = Vec::new();

        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type() {
                if file_type.is_file() {
                    let path = camino::Utf8PathBuf::try_from(entry.path()).context("Non-UTF-8 path encountered")?;
                    if let Some(ext) = path.extension() {
                        match ext {
                            "qmd" => qmd.push(path),
                            "Rmd" => rmd.push(path),
                            "R" => r.push(path),
                            "py" => py.push(path),
                            _ => {}
                        }
                    }
                }
            }
        }

        // Sort alphabetically to be deterministic
        qmd.sort();
        rmd.sort();
        r.sort();
        py.sort();

        files.extend(qmd);
        files.extend(rmd);
        files.extend(r);
        files.extend(py);
    }

    Ok(files)
}

use std::process::Command;
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

struct DocumentOutputPaths {
    target: camino::Utf8PathBuf,
    destination: camino::Utf8PathBuf,
    assets: camino::Utf8PathBuf,
    destination_assets: camino::Utf8PathBuf,
}

fn document_output_paths(file_path: &camino::Utf8Path, docs_path: &camino::Utf8Path, project_root: &camino::Utf8Path) -> anyhow::Result<DocumentOutputPaths> {
    crate::path_safety::validate_document_destination(project_root, file_path, docs_path)?;
    let content = fs::read_to_string(file_path).context("Failed to read document input")?;
    let (format, output_file) = crate::build_pre::parse_frontmatter_options(&content);
    let ext = crate::build_pre::map_format_to_extension(format.as_deref());
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
    Ok(DocumentOutputPaths { target, destination, assets, destination_assets })
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
    let content = fs::read_to_string(&path).context("Cannot read Bookdown configuration")?;
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
fn execute_script(project_root: &camino::Utf8Path, script_path: &camino::Utf8Path, profile: Option<&str>, resolved_python_cmd: Option<&str>) -> anyhow::Result<()> {
    let parent_dir = script_path.parent().unwrap_or(camino::Utf8Path::new(""));
    let ext = script_path.extension().unwrap_or("");
    let mut cmd = match ext {
        "R" => {
            let mut c = Command::new("Rscript");
            c.arg(script_path.as_std_path());
            c
        },
        "py" => {
            let python_exe = resolved_python_cmd.unwrap_or("python");
            let mut c = Command::new(python_exe);
            c.arg(script_path.as_std_path());
            c
        },
        "qmd" => {
            let mut c = Command::new("quarto");
            c.arg("render").arg(script_path.as_std_path());
            c
        },
        "Rmd" => {
            let mut c = Command::new("Rscript");
            c.arg("-e").arg(format!("rmarkdown::render('{}')", script_path.file_name().unwrap()));
            c
        },
        _ => {
            anyhow::bail!("Unsupported script extension for execution: {}", script_path);
        }
    };

    // TODO: migrate to camino
    cmd.current_dir(parent_dir.as_std_path());
    if crate::env::is_non_interactive() {
        cmd.stdin(std::process::Stdio::null());
    }
    cmd.env(crate::project::PROJECT_ROOT_ENV, project_root.as_str());
    if let Some(p) = profile {
        cmd.env("PROJR_PROFILE", p);
    }

    let status = cmd.status().context(format!("Failed to execute script {}", script_path))?;
    if !status.success() {
        anyhow::bail!("Script {} failed with status: {}", script_path, status);
    }

    Ok(())
}

/// Executes a quarto or bookdown project render.
fn execute_project_render(project_root: &camino::Utf8Path, is_quarto: bool, profile: Option<&str>) -> anyhow::Result<()> {
    let mut cmd = if is_quarto {
        let mut c = Command::new("quarto");
        c.arg("render");
        c
    } else {
        let mut c = Command::new("Rscript");
        c.arg("-e").arg("rmarkdown::render_site()");
        c
    };

    // TODO: migrate to camino
    cmd.current_dir(project_root.as_std_path());
    if crate::env::is_non_interactive() {
        cmd.stdin(std::process::Stdio::null());
    }
    cmd.env(crate::project::PROJECT_ROOT_ENV, project_root.as_str());
    if let Some(p) = profile {
        cmd.env("PROJR_PROFILE", p);
    }

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
