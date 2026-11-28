//! projr named pipelines run through the CLI: selection, placement relative to
//! inline entries, decision D4 for development builds and early failure.
use std::{fs, path::Path, process::Command};

fn build(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["--non-interactive", "--project"])
        .arg(root)
        .args(args)
        .current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV)
        .env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE")
        .env_remove("QUARTO_PROFILE")
        .env_remove("PROJR_CLEAR_OUTPUT")
        .output()
        .unwrap()
}

/// A project whose scripts and hooks each append their name to `order.txt`.
fn project(config: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap().join("project");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("_projr.yml"), config).unwrap();
    for name in [
        "inline", "own", "other", "arg", "b-hook", "d-hook", "p-hook", "q-hook",
    ] {
        fs::write(
            root.join(format!("{name}.py")),
            format!("with open('order.txt', 'a') as f:\n    f.write('{name}\\n')\n"),
        )
        .unwrap();
    }
    (temporary, root)
}

fn order(root: &Path) -> Vec<String> {
    fs::read_to_string(root.join("order.txt"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

const PIPELINES: &str = "directories:\n  output: {path: released}\npipelines:\n  p:\n    scripts: [own.py]\n    hooks: {pre: [p-hook.py]}\n  q:\n    scripts: [other.py]\n    hooks: {post: [q-hook.py]}\n";

#[test]
fn development_builds_inherit_pipeline_scripts_but_only_explicit_selection_runs_its_hooks() {
    let config = format!(
        "{PIPELINES}build:\n  git: false\n  pipeline: p\n  scripts: [inline.py]\n  hooks: {{pre: [b-hook.py]}}\ndev:\n  hooks: {{pre: [d-hook.py]}}\n"
    );

    // build.scripts follows pipeline, so the pipeline's scripts run first.
    let (_temporary, root) = project(&config);
    let output = build(&root, &["build-dev"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(order(&root), ["d-hook", "own", "inline"]);

    let (_temporary, root) = project(&config);
    let output = build(&root, &["build-dev", "--pipeline", "p,q"]);
    assert!(output.status.success(), "{output:?}");
    // dev sets no pipeline, so placement comes from the build block, where hooks
    // follow pipeline: pipeline hooks run before dev.hooks (projr).
    assert_eq!(
        order(&root),
        ["p-hook", "d-hook", "own", "other", "inline", "q-hook"]
    );

    let (_temporary, root) = project(&config);
    let output = build(
        &root,
        &["build-dev", "--pipeline", "false", "--scripts", "arg.py"],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(order(&root), ["d-hook", "arg"]);

    // --scripts takes the place of the inline scripts and keeps their position.
    let (_temporary, root) = project(&config);
    let output = build(&root, &["build-dev", "--scripts", "arg.py"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(order(&root), ["d-hook", "own", "arg"]);
}

#[test]
fn production_builds_combine_inline_and_pipeline_entries_in_key_order() {
    let config = format!(
        "{PIPELINES}build:\n  git: false\n  scripts: [inline.py]\n  hooks: {{pre: [b-hook.py, p-hook.py]}}\n  pipeline: [q, p]\n"
    );
    let (_temporary, root) = project(&config);
    let output = build(&root, &["build", "--patch"]);
    assert!(output.status.success(), "{output:?}");
    // Inline entries precede pipeline; a repeated hook runs once, first.
    assert_eq!(
        order(&root),
        ["b-hook", "p-hook", "inline", "other", "own", "q-hook"]
    );
    let records: Vec<serde_json::Value> = fs::read_dir(root.join(".projr/attempts"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| serde_json::from_slice(&fs::read(path).unwrap()).unwrap())
        .collect();
    assert!(!records.is_empty());
    for record in records {
        assert_eq!(
            record["pipelines"],
            serde_json::json!({
                "names": ["q", "p"],
                "omitted_names": 0,
                "scripts_placement": "inline_first",
                "hooks_placement": "inline_first"
            })
        );
    }
}

#[test]
fn unmatched_pipeline_selection_fails_before_any_hook_or_script() {
    for (config, args) in [
        (
            format!("{PIPELINES}build:\n  git: false\n  pipeline: missing*\n"),
            vec!["build-dev"],
        ),
        (
            format!("{PIPELINES}build:\n  git: false\n  hooks: {{pre: [b-hook.py]}}\n"),
            vec!["build", "--pipeline", "p,zzz"],
        ),
    ] {
        let (_temporary, root) = project(&config);
        let output = build(&root, &args);
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{args:?}: {diagnostic}");
        assert!(
            diagnostic.contains("matches no pipeline defined under pipelines"),
            "{diagnostic}"
        );
        assert!(order(&root).is_empty(), "{args:?}");
        assert!(!root.join("released").exists(), "{args:?}");
        assert!(!root.join("VERSION").exists(), "{args:?}");
    }
}

#[test]
fn invalid_pipeline_definitions_are_rejected() {
    for (pipelines, message) in [
        ("pipelines:\n  'a,b': {}\n", "cannot contain ':' or ','"),
        ("pipelines:\n  a: {render: true}\n", "unknown field"),
        ("pipelines: [a]\n", "pipelines must be a YAML mapping"),
        (
            "pipelines: {}\nbuild:\n  pipeline: true\n",
            "or false for none",
        ),
    ] {
        let (_temporary, root) = project(pipelines);
        let output = build(&root, &["build-dev"]);
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{pipelines}");
        assert!(diagnostic.contains(message), "{pipelines}: {diagnostic}");
        assert!(order(&root).is_empty());
    }
}
