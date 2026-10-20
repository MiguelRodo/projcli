use std::{fs, process::{Command, Output}};
use serde_json::Value;

fn command(root: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_proj"));
    command.arg("--machine").arg("--project").arg(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE")
        .env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT");
    command
}

fn envelope(output: &Output, success: bool) -> Value {
    assert_eq!(output.status.success(), success, "{} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let value: Value = serde_json::from_slice(&output.stdout).expect("stdout must be exactly one JSON value without prose");
    assert_eq!(value["protocol_version"], 1);
    assert_eq!(value["cli_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(value["ok"], success);
    assert_eq!(value["error"].is_null(), success);
    assert_eq!(value["result"].is_null(), !success);
    value
}

#[test]
fn capability_negotiation_does_not_load_a_project() {
    let temp = tempfile::tempdir().unwrap();
    let output = command(&temp.path().join("missing")).arg("capabilities").output().unwrap();
    let result = envelope(&output, true);
    assert_eq!(result["result"]["capability_version"], 1);
    assert_eq!(result["result"]["preview_output_isolation"], false);
    assert!(result["result"]["machine_commands"].as_array().unwrap().iter().any(|command| command == "build-dev"));
}

#[test]
fn queries_produce_typed_results_without_progress_or_configuration_secrets() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    fs::write(root.join("_proj.yml"), "directories:\n  output:\n    path: output data\nparameters:\n  private_token: machine-secret-sentinel\n").unwrap();
    for args in [vec!["path", "get", "--label", "output"], vec!["version", "get"], vec!["version", "get", "--format", "json"], vec!["yml", "read"]] {
        let output = command(&root).args(&args).output().unwrap();
        let value = envelope(&output, true);
        assert!(!String::from_utf8_lossy(&output.stdout).contains("machine-secret-sentinel"));
        assert!(output.stderr.is_empty());
        match args[0] {
            "path" => assert!(value["result"]["path"].as_str().unwrap().ends_with("output data")),
            "version" => { assert_eq!(value["result"]["version"]["major"], 1); assert_eq!(value["result"]["text"], "v1.2.3"); }
            "yml" => assert_eq!(value["result"]["validated"], true),
            _ => unreachable!()
        }
    }
}

#[test]
fn argument_project_and_query_failures_are_structured_and_nonzero() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let output = command(root).args(["unsupported-command", "--private-token=machine-secret-sentinel"]).output().unwrap();
    assert_eq!(envelope(&output, false)["error"]["code"], "invalid_arguments");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("machine-secret-sentinel"));
    assert!(output.stderr.is_empty());
    let output = command(&root.join("missing")).args(["version", "get"]).output().unwrap();
    assert_eq!(envelope(&output, false)["error"]["code"], "project_selection_failed");
    fs::write(root.join("_proj.yml"), "parameters: [machine-secret-sentinel\n").unwrap();
    let output = command(root).args(["yml", "read"]).output().unwrap();
    assert_eq!(envelope(&output, false)["error"]["code"], "query_failed");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("machine-secret-sentinel"));
    let output = command(root).args(["version", "get", "--format", "unsupported"]).output().unwrap();
    assert_eq!(envelope(&output, false)["error"]["code"], "invalid_arguments");
}

#[test]
fn child_stdout_and_stderr_cannot_pollute_protocol_stdout() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("_proj.yml"), "build:\n  scripts: [analysis.py]\n").unwrap();
    for fail in [false, true] {
        fs::write(root.join("analysis.py"), format!("import sys\nprint('child-out-marker')\nprint('child-err-marker', file=sys.stderr)\nraise SystemExit({})\n", if fail { 17 } else { 0 })).unwrap();
        let output = command(root).arg("build-dev").output().unwrap();
        let value = envelope(&output, !fail);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("child-out-marker"));
        assert!(stderr.contains("child-err-marker"));
        if fail { assert_eq!(value["error"]["code"], "command_failed"); }
        else { assert_eq!(value["result"]["completed"], true); }
    }
    #[cfg(unix)]
    {
        fs::write(root.join("analysis.py"), "raise SystemExit(0)\n").unwrap();
        let empty_path = root.join("empty-path");
        fs::create_dir(&empty_path).unwrap();
        let output = command(root).arg("build-dev").env("PATH", empty_path).output().unwrap();
        assert_eq!(envelope(&output, false)["error"]["code"], "command_failed");
        assert!(String::from_utf8_lossy(&output.stderr).contains("Python"));
    }
}

#[test]
fn local_init_and_version_mutation_reuse_existing_commands_with_json_completion() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let output = command(root).args(["init", "directories"]).output().unwrap();
    assert_eq!(envelope(&output, true)["result"]["completed"], true);
    assert!(root.join("_proj.yml").exists());
    assert!(!root.join(".git").exists());
    let output = command(root).args(["version", "set", "v2.3.4"]).output().unwrap();
    assert_eq!(envelope(&output, true)["result"]["completed"], true);
    let output = command(root).args(["version", "get"]).output().unwrap();
    assert_eq!(envelope(&output, true)["result"]["version"]["major"], 2);
}


#[test]
fn worker_preserves_terminator_and_literal_option_values() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let output = command(root).args(["ignore", "add", "--", "ordinary-path"]).output().unwrap();
    envelope(&output, true);
    let ignores = fs::read_to_string(root.join(".gitignore")).unwrap();
    assert!(ignores.contains("ordinary-path"));
    assert!(!ignores.contains("--non-interactive"));
    let output = command(root).args(["ignore", "add", "--", "--machine", "--non-interactive"]).output().unwrap();
    envelope(&output, true);
    let ignores = fs::read_to_string(root.join(".gitignore")).unwrap();
    assert!(ignores.contains("--machine"));
    assert!(ignores.contains("--non-interactive"));
    let output = command(root).args(["init", "readme", "--title=literal", "--description=--machine"]).output().unwrap();
    envelope(&output, true);
    assert!(fs::read_to_string(root.join("README.md")).unwrap().contains("--machine"));
}
