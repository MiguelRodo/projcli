//! projr 0.9 configuration layout: the `profile` key, accepted keys and
//! local-config ignore rules (SATVILab/projr@5f915ef).
use std::{fs, path::Path, process::Command};

fn put(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn proj(root: &Path, args: &[&str], projr_profile: Option<&str>) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_proj"));
    command
        .args(["--non-interactive", "--project"])
        .arg(root)
        .args(args)
        .current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV)
        .env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE")
        .env_remove("QUARTO_PROFILE");
    if let Some(profile) = projr_profile {
        command.env("PROJR_PROFILE", profile);
    }
    command.output().unwrap()
}

/// The merged `build.scripts` value selected by `proj yml get`.
fn selected(root: &Path, args: &[&str], projr_profile: Option<&str>) -> serde_json::Value {
    let mut full = vec!["yml", "get"];
    full.extend(args);
    let output = proj(root, &full, projr_profile);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    value["build"]["scripts"].clone()
}

#[test]
fn profile_key_follows_projr_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    put(
        root,
        "_projr.yml",
        "profile: from-default\nbuild:\n  scripts: [base.R]\n",
    );
    put(
        root,
        "_projr-from-default.yml",
        "build:\n  scripts: [default.R]\n",
    );
    put(
        root,
        "_projr-from-local.yml",
        "build:\n  scripts: [local.R]\n",
    );
    put(root, "_projr-from-env.yml", "build:\n  scripts: [env.R]\n");
    put(root, "_projr-from-cli.yml", "build:\n  scripts: [cli.R]\n");
    assert_eq!(selected(root, &[], None), serde_json::json!(["default.R"]));
    // A local mapping naming only Quarto profiles leaves the default's projr profile.
    put(
        root,
        ".projr/config/local.yml",
        "profile:\n  quarto: print\n",
    );
    assert_eq!(selected(root, &[], None), serde_json::json!(["default.R"]));
    put(
        root,
        ".projr/config/local.yml",
        "profile:\n  projr: [from-local]\n  quarto: [print]\n",
    );
    assert_eq!(selected(root, &[], None), serde_json::json!(["local.R"]));
    assert_eq!(
        selected(root, &[], Some("from-env")),
        serde_json::json!(["env.R"])
    );
    assert_eq!(
        selected(root, &["--profile", "from-cli"], Some("from-env")),
        serde_json::json!(["cli.R"])
    );
}

#[test]
fn profile_key_is_validated_and_forbidden_in_named_profiles() {
    for (file, content, message) in [
        (
            "_projr.yml",
            "profile:\n  renv: x\n",
            "profile.renv is not supported yet.",
        ),
        (
            "_projr.yml",
            "profile:\n  other: x\n",
            "Unknown profile key",
        ),
        (
            "_projr.yml",
            "profile: 3\n",
            "profile must be a string or a list of strings",
        ),
        (
            "_projr-local.yml",
            "profile:\n  quarto: [1]\n",
            "profile.quarto must be a string",
        ),
        (
            "_projr-named.yml",
            "profile: other\n",
            "The profile key is read only from the default and local files, not from _projr-named.yml.",
        ),
        (
            ".projr/config/named.yml",
            "profile: other\n",
            "not from named.yml.",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        if file != "_projr.yml" {
            put(root, "_projr.yml", "build:\n  scripts: []\n");
        }
        put(root, file, content);
        let utf8 = camino::Utf8Path::from_path(root).unwrap();
        let error = format!(
            "{:#}",
            proj::yml::get_combined_yml(Some("named"), utf8).unwrap_err()
        );
        assert!(error.contains(message), "{file}: {error}");
        assert!(
            !proj(root, &["yml", "get", "--profile", "named"], None)
                .status
                .success()
        );
    }
}

#[test]
fn build_freezes_the_configured_quarto_profile_in_the_run_context() {
    let script = "import json, os\nfrom pathlib import Path\ncontext = json.loads(Path(os.environ['PROJ_RUN_CONTEXT']).read_text())\nPath('quarto.json').write_text(json.dumps(context['quarto_profiles']))\n";
    for (environment, expected) in [(None, "[\"print\", \"web\"]"), (Some("native"), "[]")] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        put(
            root,
            "_projr.yml",
            "profile:\n  quarto: [print, web]\ndirectories:\n  cache:\n    path: _tmp\nbuild:\n  scripts: [record.py]\n  git: false\n",
        );
        put(root, "record.py", script);
        let mut command = Command::new(env!("CARGO_BIN_EXE_proj"));
        command
            .args(["--non-interactive", "--project"])
            .arg(root)
            .arg("build-dev")
            .current_dir(root)
            .env_remove(proj::PROJECT_ROOT_ENV)
            .env_remove(proj::RUN_CONTEXT_ENV)
            .env_remove("PROJR_PROFILE")
            .env_remove("QUARTO_PROFILE");
        if let Some(value) = environment {
            command.env("QUARTO_PROFILE", value);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(root.join("quarto.json")).unwrap(),
            expected
        );
    }
}

#[test]
fn accepted_projr_keys_do_not_block_builds() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    put(
        root,
        "_projr.yml",
        "settings:\n  hash-engine: legacy\ndirectories:\n  cache:\n    path: _tmp\nbuild:\n  scripts: []\n  cite: {cff: false}\n  package: true\n  git:\n    commit: false\n    add-untracked: false\n",
    );
    let output = proj(root, &["build-dev"], None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = proj(root, &["yml", "get", "--check"], None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    // projr's original spelling is normalised into settings.hashing.approach.
    assert_eq!(
        value["settings"],
        serde_json::json!({"hashing": {"approach": "legacy"}})
    );
    put(
        root,
        "_projr.yml",
        "settings:\n  hashing:\n    approach: rust\n",
    );
    assert!(
        !proj(root, &["yml", "get", "--check"], None)
            .status
            .success()
    );
}

#[test]
fn managed_block_ignores_projr_state_and_local_configs_by_profile() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    put(
        root,
        ".projr/config/default.yml",
        "directories:\n  cache:\n    path: _tmp\nbuild:\n  git: false\n",
    );
    put(root, ".projr/config/local.yml", "build:\n  scripts: []\n");
    put(
        root,
        "_projr/config/_projr-local.yml",
        "dev:\n  scripts: []\n",
    );
    put(
        root,
        ".projr/config/analysis.yml",
        "build:\n  scripts: []\n",
    );
    fs::create_dir(root.join(".quarto")).unwrap();
    let output = proj(root, &["init", "directories"], None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
    for rule in [
        "/.projr/manifests/.versions.lock",
        "/.projr/logs/**",
        "/.projr/source/**",
        ".projr/config/local.yml",
        "_projr/config/_projr-local.yml",
        ".quarto/**",
    ] {
        assert!(
            gitignore.lines().any(|line| line == rule),
            "missing {rule}:\n{gitignore}"
        );
    }
    assert!(!gitignore.contains("analysis.yml"), "{gitignore}");
    assert!(
        !gitignore.contains(".projr/config/default.yml"),
        "{gitignore}"
    );
    // Real Git agrees, when available.
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
    };
    if git(&["init", "-q"]).is_ok_and(|output| output.status.success()) {
        let ignored = git(&["check-ignore", "-q", ".projr/config/local.yml"]).unwrap();
        assert!(ignored.status.success());
        let tracked = git(&["check-ignore", "-q", ".projr/config/default.yml"]).unwrap();
        assert!(!tracked.status.success());
    }
}

fn isolated_vcs(root: &Path, args: &[&str]) -> Option<std::process::Output> {
    Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .ok()
        .filter(|output| output.status.success())
}

#[test]
fn starter_disables_push_without_an_upstream_and_keeps_defaults_with_one() {
    const STARTER: &str = "directories:\n  raw-data:\n    path: _raw_data\n  cache:\n    path: _tmp\n  output:\n    path: _output\n  docs:\n    path: docs\n";
    for upstream in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        if isolated_vcs(root, &["init", "-q", "-b", "main"]).is_none() {
            return; // No Git available: the no-repository branch is covered elsewhere.
        }
        if upstream {
            isolated_vcs(root, &["config", "branch.main.remote", "origin"]).unwrap();
            isolated_vcs(root, &["config", "branch.main.merge", "refs/heads/main"]).unwrap();
        }
        put(root, "_projr-local.yml", "build:\n  scripts: []\n");
        let output = Command::new(env!("CARGO_BIN_EXE_proj"))
            .args(["--non-interactive", "--project"])
            .arg(root)
            .arg("build-dev")
            .current_dir(root)
            .env_remove(proj::PROJECT_ROOT_ENV)
            .env_remove(proj::RUN_CONTEXT_ENV)
            .env_remove("PROJR_PROFILE")
            .env_remove("QUARTO_PROFILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected = if upstream {
            STARTER.to_owned()
        } else {
            format!("{STARTER}build:\n  git:\n    push: false\n")
        };
        assert_eq!(
            fs::read_to_string(root.join("_projr.yml")).unwrap(),
            expected
        );
    }
}

#[test]
fn legacy_config_directory_is_protected_from_managed_outputs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    put(
        root,
        "_projr.yml",
        "directories:\n  cache:\n    path: _tmp\n  output:\n    path: _projr\nbuild:\n  scripts: []\n  git: false\n",
    );
    put(root, "_projr/config/local.yml", "dev:\n  scripts: []\n");
    let output = proj(root, &["build", "--patch"], None);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("protected project metadata"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join("_projr/config/local.yml")).unwrap(),
        "dev:\n  scripts: []\n"
    );
}

#[test]
fn hash_engine_fills_a_null_hashing_block() {
    let temp = tempfile::tempdir().unwrap();
    let root = camino::Utf8Path::from_path(temp.path()).unwrap();
    put(
        root.as_std_path(),
        "_projr.yml",
        "settings:\n  hash-engine: legacy\n  hashing: ~\n",
    );
    let merged = proj::yml::get_combined_yml(Some(""), root).unwrap();
    assert_eq!(
        merged["settings"],
        serde_json::json!({"hashing": {"approach": "legacy"}})
    );
}

fn first_preview(root: &Path) -> std::process::Output {
    put(root, "_projr-local.yml", "build:\n  scripts: []\n");
    proj(root, &["build-dev"], None)
}

#[test]
fn starter_seeding_keeps_user_lines_and_existing_blocks() {
    // Existing .gitignore without a block: missing defaults, then the empty block,
    // then the user's lines (so a later exception still wins).
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    put(root, ".gitignore", ".DS_Store\n!keep.txt\n");
    let output = first_preview(root);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
    assert_eq!(
        gitignore
            .lines()
            .filter(|line| *line == ".DS_Store")
            .count(),
        1
    );
    assert!(
        gitignore.ends_with("# End of projr section\n.DS_Store\n!keep.txt\n"),
        "{gitignore}"
    );
    assert!(!root.join(".Rbuildignore").exists());

    // An existing managed block is left byte-identical; a package gets .Rbuildignore.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let existing = "# Start of projr section: kept\n/custom/**\n# End of projr section\n";
    put(root, ".gitignore", existing);
    put(root, "DESCRIPTION", "Package: demo\nVersion: 0.0.1\n");
    let output = first_preview(root);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join(".gitignore")).unwrap(),
        existing
    );
    let rbuild = fs::read_to_string(root.join(".Rbuildignore")).unwrap();
    assert!(
        rbuild.lines().any(|line| line == r"^_projr-local\.yml$"),
        "{rbuild}"
    );
}

#[test]
fn malformed_ignore_markers_fail_before_the_starter_is_written() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let broken = "# End of projr section\n# Start of projr section\n";
    put(root, ".gitignore", broken);
    let output = first_preview(root);
    assert!(!output.status.success());
    assert!(!root.join("_projr.yml").exists());
    assert_eq!(fs::read_to_string(root.join(".gitignore")).unwrap(), broken);
}

#[test]
fn init_directories_from_a_subfolder_targets_the_configured_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    put(
        root,
        ".projr/config/default.yml",
        "directories:\n  output:\n    path: _results\n",
    );
    let nested = root.join("analysis");
    fs::create_dir(&nested).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["--non-interactive", "init", "directories"])
        .current_dir(&nested)
        .env_remove(proj::PROJECT_ROOT_ENV)
        .env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE")
        .env_remove("QUARTO_PROFILE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.join("_results").is_dir());
    assert!(!nested.join("_projr.yml").exists() && !root.join("_projr.yml").exists());
}
