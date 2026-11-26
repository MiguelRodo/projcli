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
