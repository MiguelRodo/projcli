use std::path::{Path, PathBuf};
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
/// std::fs::write(temp.path().join("_proj.yml"), "build:\n  scripts: []\n  git:\n    commit: false\n    push: false\nconfig:\n  git:\n    use_proj_cred_helper: false").unwrap();
/// build_project(camino::Utf8Path::from_path(temp.path()).unwrap(), BuildMode::ProdPatch, None, None).unwrap();
/// ```
use crate::git::{is_git_installed, check_git_profile, git_commit_all, create_git_provider};
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
    let combined = crate::yml::get_combined_yml(None, project_root)
        .context("Failed to read project configuration")?;
    let project_config: ProjConfig = serde_json::from_value(combined)
        .context("Failed to deserialize combined configuration")?;
    let mut config = project_config.validate_and_resolve(project_root, is_dev)?;
    if !is_dev {
        if project_root.join("_environment.local").exists() {
            crate::ignore::add_manual_ignores(project_root, &["_environment.local".to_string()], true, crate::ignore::IgnoreType::All)?;
        }
        crate::ignore::update_ignores(project_root, &config).context("Failed to update ignores")?;
    }

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
    let quarto_exists = quarto_exists && run_analysis;
    let bookdown_exists = bookdown_exists && run_analysis;

    let mut validation_files = resolved_files.clone();
    validation_files.extend(resolved_hooks.clone());

    let (_resolved_token, resolved_python_cmd) = run_pre_flight_checks(
        project_root,
        &config,
        is_prod_run,
        &validation_files,
        quarto_exists,
        bookdown_exists,
    )?;

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
                if !is_dev {
                    version_set_at(project_root, &v.to_string(true)).context("Failed to initialize a VERSION file")?;
                }
                v
            }
        }
    };

    let version_before_build = initial_version.clone();

    // 4.5. Workspace Pre-clearing Hooks
    let current_version = initial_version.to_string(false);
    let clear_output_env = std::env::var("PROJR_CLEAR_OUTPUT").ok();
    let clear_output_val = clear_output_env.as_deref().or(config.clear_output.as_deref());

    if !is_dev && run_analysis {
        crate::clear::clear_old(project_root, &current_version, is_dev, config.old_dev_remove)?;
        crate::clear::clear_pre(project_root, &current_version, &config, clear_output_val)?;
    }

    // 5. Version Bump
    if !is_dev {
        match mode {
            BuildMode::ProdMajor => {
                initial_version.major += 1;
                initial_version.minor = 0;
                initial_version.patch = 0;
                initial_version.dev = 0;
            }
            BuildMode::ProdMinor => {
                initial_version.minor += 1;
                initial_version.patch = 0;
                initial_version.dev = 0;
            }
            BuildMode::ProdPatch => {
                initial_version.patch += 1;
                initial_version.dev = 0;
            }
            _ => {}
        }
        version_set_at(project_root, &initial_version.to_string(false))?;
    }

    // Git auto-ignore synchronization was requested explicitly during config audit.

    // 6.5 Sidecar Output Redirection
    let mut original_docs_path: Option<PathBuf> = None;
    let mut original_docs_path_str = "docs".to_string(); // default if missing or can't compute
    let mut actual_docs_key = "docs".to_string();

    if quarto_exists || bookdown_exists {
        if let Some((k, docs_config)) = config.directories.iter().find(|(k, _)| k.to_lowercase() == "docs") {
            actual_docs_key = k.clone();
            let path = docs_config.path.clone();
            original_docs_path_str = path.strip_prefix(project_root).unwrap_or(path.as_path()).as_str().to_string(); /* TODO: migrate to camino */
            original_docs_path = Some(path.into()); /* TODO: migrate to camino */
        }

        let isolated_docs_path = project_root.join("_tmp").join("projr").join(&current_version).join("docs");
        let isolated_docs_path_str = isolated_docs_path.strip_prefix(project_root).unwrap_or(&isolated_docs_path).as_str().to_string();

        if quarto_exists {
            rewrite_engine_output_dir(project_root.as_std_path(), "quarto", &isolated_docs_path_str)?;
        }
        if bookdown_exists {
            rewrite_engine_output_dir(project_root.as_std_path(), "bookdown", &isolated_docs_path_str)?;
        }

        if original_docs_path.is_some() {
            if let Some(docs_mut) = config.directories.get_mut(&actual_docs_key) {
                docs_mut.path = isolated_docs_path.into();
            }
        } else {
            config.directories.insert("docs".to_string(), crate::yml::ResolvedDir {
                path: isolated_docs_path.into(),
                ignore: crate::yml::IgnoreConfig::Single("git".to_string()),
            });
        }
    }

    // Execute Pre-Build Hooks
    if let Some(pre_hooks) = &hooks_config.pre {
        let pre_resolved = resolve_explicit_scripts(project_root, pre_hooks)?;
        for hook in pre_resolved {
            execute_script(project_root, &hook, profile.as_deref(), resolved_python_cmd.as_deref())?;
        }
    }
    if let Some(both_hooks) = &hooks_config.both {
        let both_resolved = resolve_explicit_scripts(project_root, both_hooks)?;
        for hook in both_resolved {
            execute_script(project_root, &hook, profile.as_deref(), resolved_python_cmd.as_deref())?;
        }
    }

    // 6. Git Pre-Snapshot
    if config.git.commit {
        git_commit_all("Snapshot pre-build", Some(project_root)).context("Failed to snapshot pre-build")?;
    }

    // ==========================================
    // STEP C: Core Document Compilation
    // ==========================================

    // Isolate execution logic inside catch_unwind
    let pr = project_root.to_path_buf();
    let profile_clone = profile.clone();
    let build_scripts_clone = build_scripts.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        execute_build_pipeline(&camino::Utf8PathBuf::try_from(pr).unwrap(), build_scripts_clone, profile_clone, quarto_exists, bookdown_exists, resolved_python_cmd.as_deref())
    }));

    match result {
        Ok(Ok(())) => {
            // ==========================================
            // STEP D: Post-Build Operations
            // ==========================================

            // Restore Sidecar Configs
            if quarto_exists {
                let _ = rewrite_engine_output_dir(project_root.as_std_path(), "quarto", &original_docs_path_str);
            }
            if bookdown_exists {
                let _ = rewrite_engine_output_dir(project_root.as_std_path(), "bookdown", &original_docs_path_str);
            }
            if quarto_exists || bookdown_exists {
                if let Some(orig_path) = original_docs_path.as_ref() {
                    if let Some(docs_mut) = config.directories.get_mut(&actual_docs_key) {
                        docs_mut.path = camino::Utf8PathBuf::from_path_buf(orig_path.clone()).unwrap(); /* TODO: migrate to camino */
                    }
                } else {
                    config.directories.remove(&actual_docs_key);
                }
            }

            let is_single_doc_engine = !quarto_exists && !bookdown_exists;
            let clear_output_env = std::env::var("PROJR_CLEAR_OUTPUT").ok();
            let clear_output_val = clear_output_env.as_deref().or(config.clear_output.as_deref());
            if !is_dev && run_analysis {
                crate::clear::clear_post(project_root, is_dev, &config, clear_output_val, is_single_doc_engine)?;
            }

            // Copy docs
            let should_run_output = config.output_run.unwrap_or(true);
            if should_run_output && !is_dev && run_analysis {
                let final_docs_dir = original_docs_path.clone().unwrap_or_else(|| project_root.join("docs").into());
                let cache_docs_dir = project_root.join("_tmp").join("projr").join(&current_version).join("docs");

                // TODO: migrate to camino
                if quarto_exists {
                    let _ = copy_global_quarto_project(&camino::Utf8PathBuf::try_from(cache_docs_dir).unwrap(), &camino::Utf8PathBuf::try_from(final_docs_dir).unwrap());
                } else if bookdown_exists {
                    let _ = copy_global_bookdown(&camino::Utf8PathBuf::try_from(cache_docs_dir).unwrap(), &camino::Utf8PathBuf::try_from(final_docs_dir).unwrap(), project_root);
                } else {
                    // It's a mixed/individual file run
                    if let Some(scripts) = &build_scripts {
                        if let Ok(resolved_files) = resolve_explicit_scripts(project_root, scripts) {
                            for file in resolved_files {
                                let ext = file.extension().unwrap_or("");
                                if ext == "qmd" {
                                    // TODO: migrate to camino
                                    let _ = copy_individual_quarto(&file, camino::Utf8Path::from_path(final_docs_dir.as_path()).unwrap(), project_root);
                                } else if ext == "Rmd" || ext == "rmd" {
                                    // TODO: migrate to camino
                                    let _ = copy_individual_rmd(&file, camino::Utf8Path::from_path(final_docs_dir.as_path()).unwrap(), project_root);
                                }
                            }
                        }
                    } else {
                        if let Ok(resolved_files) = resolve_fallback_scripts(project_root) {
                            for file in resolved_files {
                                let ext = file.extension().unwrap_or("");
                                if ext == "qmd" {
                                    // TODO: migrate to camino
                                    let _ = copy_individual_quarto(&file, camino::Utf8Path::from_path(final_docs_dir.as_path()).unwrap(), project_root);
                                } else if ext == "Rmd" || ext == "rmd" {
                                    // TODO: migrate to camino
                                    let _ = copy_individual_rmd(&file, camino::Utf8Path::from_path(final_docs_dir.as_path()).unwrap(), project_root);
                                }
                            }
                        }
                    }
                }
            }

            if config.git.commit {
                let ver_str = initial_version.to_string(false);
                let final_message = match description {
                    Some(desc) if !desc.trim().is_empty() => format!("Build v{}: {}", ver_str, desc.trim()),
                    _ => format!("Build v{}", ver_str),
                };

                let provider = create_git_provider(config.config.git.engine, project_root.to_path_buf())?;
                provider.commit_all(&final_message)?;
            }

            // Execute Post-Build Hooks (after post-build commit, before push)
            if let Some(post_hooks) = &hooks_config.post {
                let post_resolved = resolve_explicit_scripts(project_root, post_hooks)?;
                for hook in post_resolved {
                    execute_script(project_root, &hook, profile.as_deref(), resolved_python_cmd.as_deref())?;
                }
            }
            if let Some(both_hooks) = &hooks_config.both {
                let both_resolved = resolve_explicit_scripts(project_root, both_hooks)?;
                for hook in both_resolved {
                    execute_script(project_root, &hook, profile.as_deref(), resolved_python_cmd.as_deref())?;
                }
            }

            // Execute Remote Export Pipelines
            if !is_dev && !config.dest.is_empty() {
                if let Some(remotes) = &config.remotes.local {
                    for dest_target in &config.dest {
                        if let Some(remote) = remotes.get(dest_target) {
                            for tag in &remote.content {
                                if let Ok(dir_path) = config.get_path(project_root, tag) /* TODO: migrate to camino */ {
                                    if dir_path.exists() {
                                        crate::cas::ingest_directory(
                                            project_root,
                                            remote.path.as_path(),
                                            tag,
                                            dir_path.as_path(),
                                            &initial_version.to_string(false)
                                        ).context(format!("Failed remote CAS export for {}", tag))?;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if config.git.commit {
                if config.git.push {
                    post_build_sync(&config.config, project_root.to_path_buf())?;
                }
            }
            Ok(())
        },
        Ok(Err(e)) => {
            // Restore Sidecar Configs on failure
            if quarto_exists {
                let _ = rewrite_engine_output_dir(project_root.as_std_path(), "quarto", &original_docs_path_str);
            }
            if bookdown_exists {
                let _ = rewrite_engine_output_dir(project_root.as_std_path(), "bookdown", &original_docs_path_str);
            }
            if quarto_exists || bookdown_exists {
                if let Some(orig_path) = original_docs_path.as_ref() {
                    if let Some(docs_mut) = config.directories.get_mut(&actual_docs_key) {
                        docs_mut.path = camino::Utf8PathBuf::from_path_buf(orig_path.clone()).unwrap(); /* TODO: migrate to camino */
                    }
                } else {
                    config.directories.remove(&actual_docs_key);
                }
            }

            if is_prod_run {
                // Revert to Development State of Previous Baseline
                let mut reverted = version_before_build.clone();
                if reverted.dev == 0 {
                    reverted.dev = 1;
                }
                let _ = version_set_at(project_root, &reverted.to_string(false));
            }
            Err(e)
        }
        Err(_) => {
            // Restore Sidecar Configs on failure
            if quarto_exists {
                let _ = rewrite_engine_output_dir(project_root.as_std_path(), "quarto", &original_docs_path_str);
            }
            if bookdown_exists {
                let _ = rewrite_engine_output_dir(project_root.as_std_path(), "bookdown", &original_docs_path_str);
            }
            if quarto_exists || bookdown_exists {
                if let Some(orig_path) = original_docs_path.as_ref() {
                    if let Some(docs_mut) = config.directories.get_mut(&actual_docs_key) {
                        docs_mut.path = camino::Utf8PathBuf::from_path_buf(orig_path.clone()).unwrap(); /* TODO: migrate to camino */
                    }
                } else {
                    config.directories.remove(&actual_docs_key);
                }
            }

            if is_prod_run {
                // Revert to Development State of Previous Baseline
                let mut reverted = version_before_build.clone();
                if reverted.dev == 0 {
                    reverted.dev = 1;
                }
            let _ = version_set_at(project_root, &reverted.to_string(false));
            }
            anyhow::bail!("Build pipeline panicked unexpectedly.");
        }
    }
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
        // If there's no directory prefix (e.g. *.qmd), it should only match at the root.
        let is_root_only = !pattern.contains('/');
        let target_pattern = if is_root_only {
            project_root.join(pattern).to_string()
        } else {
            project_root.join(pattern).to_string()
        };

        // TODO: migrate to camino
        let paths = glob::glob(&target_pattern)
            .context(format!("Invalid glob pattern '{}'", target_pattern))?;

        for entry in paths {
            if let Ok(path) = entry {
                if path.is_file() {
                    matched_files.push(camino::Utf8PathBuf::try_from(path).context("Non-UTF-8 path encountered")?);
                }
            }
        }
    }

    // Process excludes
    let mut final_files = Vec::new();
    for file in matched_files {
        let mut is_excluded = false;

        for exclude in &excludes {
            // Very simple matcher for excludes
            let is_root_only = !exclude.contains('/');
            let exclude_pattern = if is_root_only {
                project_root.join(exclude).to_string()
            } else {
                project_root.join(exclude).to_string()
            };

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

pub fn copy_individual_rmd(file_path: &camino::Utf8Path, docs_path: &camino::Utf8Path, project_root: &camino::Utf8Path) -> anyhow::Result<()> {
    let content = fs::read_to_string(file_path).unwrap_or_default();
    let (format, output_file) = crate::build_pre::parse_frontmatter_options(&content);
    let ext = crate::build_pre::map_format_to_extension(format.as_deref());

    let file_stem = file_path.file_stem().unwrap_or("");
    let parent_dir = file_path.parent().unwrap_or(project_root);

    // Default Rmd output is usually <stem>.html or <output_file> in the same dir as the .Rmd file
    let target_file_name = if let Some(of) = output_file {
        of
    } else {
        format!("{}.{}", file_stem, ext)
    };
    let target_file = parent_dir.join(&target_file_name);

    let files_dir_name = format!("{}_files", file_stem);
    let files_dir = parent_dir.join(&files_dir_name);

    if !docs_path.exists() {
        let _ = fs::create_dir_all(docs_path);
    }

    if target_file.exists() {
        let dest_file = docs_path.join(&target_file_name);
        fs::rename(&target_file, &dest_file).context(format!("Failed to move target file {}", target_file))?;
    }

    if files_dir.exists() {
        let dest_files_dir = docs_path.join(&files_dir_name);
        crate::fs_utils::dir_move_exact(&files_dir, &dest_files_dir)?;
    }

    Ok(())
}

pub fn copy_individual_quarto(file_path: &camino::Utf8Path, docs_path: &camino::Utf8Path, project_root: &camino::Utf8Path) -> anyhow::Result<()> {
    let content = fs::read_to_string(file_path).unwrap_or_default();
    let (format, output_file) = crate::build_pre::parse_frontmatter_options(&content);
    let ext = crate::build_pre::map_format_to_extension(format.as_deref());

    let file_stem = file_path.file_stem().unwrap_or("");
    let parent_dir = file_path.parent().unwrap_or(project_root);

    let target_file_name = if let Some(of) = output_file {
        of
    } else {
        format!("{}.{}", file_stem, ext)
    };
    let target_file = parent_dir.join(&target_file_name);

    let files_dir_name = format!("{}_files", file_stem);
    let files_dir = parent_dir.join(&files_dir_name);

    if !docs_path.exists() {
        let _ = fs::create_dir_all(docs_path);
    }

    if target_file.exists() {
        let dest_file = docs_path.join(&target_file_name);
        fs::rename(&target_file, &dest_file).context(format!("Failed to move target file {}", target_file))?;
    }

    if files_dir.exists() {
        let dest_files_dir = docs_path.join(&files_dir_name);
        crate::fs_utils::dir_move_exact(&files_dir, &dest_files_dir)?;
    }

    Ok(())
}

pub fn copy_global_bookdown(cache_docs_dir: &camino::Utf8Path, final_docs_dir: &camino::Utf8Path, project_root: &camino::Utf8Path) -> anyhow::Result<()> {
    crate::fs_utils::dir_move_exact(cache_docs_dir, final_docs_dir)?;

    // We also need to locate <book_filename>_files and move to final target context.
    // _bookdown.yml specifies book_filename, defaulting to _main
    let mut book_filename = "_main".to_string();
    let bookdown_yml = project_root.join("_bookdown.yml");
    if bookdown_yml.exists() {
        let content = fs::read_to_string(&bookdown_yml).unwrap_or_default();
        if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
            if let Some(map) = yaml.as_mapping() {
                if let Some(bf) = map.get("book_filename").and_then(|v| v.as_str()) {
                    book_filename = bf.to_string();
                }
            }
        }
    }

    let files_dir_name = format!("{}_files", book_filename);
    let files_dir = project_root.join(&files_dir_name);
    if files_dir.exists() {
        let dest_files_dir = final_docs_dir.join(&files_dir_name);
        crate::fs_utils::dir_move_exact(&files_dir, &dest_files_dir)?;
    }

    Ok(())
}

pub fn copy_global_quarto_project(cache_docs_dir: &camino::Utf8Path, final_docs_dir: &camino::Utf8Path) -> anyhow::Result<()> {
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
