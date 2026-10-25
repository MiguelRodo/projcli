use std::{fs, process::Command};
use serde_json::{Value, json};

fn query(root: &std::path::Path, arguments: &[&str], ok: bool) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["--machine", "--non-interactive", "--project"]).arg(root)
        .args(arguments).env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
    assert_eq!(output.status.success(), ok, "query failed");
    assert!(output.stderr.is_empty(), "machine queries must not log raw configuration");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], ok);
    value
}

#[test]
fn explicit_getters_return_merged_typed_values_and_literal_keys_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::write(root.join("_proj.yml"), r#"{
      "directories":{"output":{"path":"not-created"}},
      "build":{"profile":"team"},
      "parameters":{"base":1,"shared":"base","array":[1,true,null],
        "--project":"literal flag","x);system('unexpected')":"literal expression",
        "private":"explicit-query-canary"}
    }"#).unwrap();
    fs::write(root.join("_projr-team.yml"), r#"{"params":{"shared":"profile","selected":"team"}}"#).unwrap();
    fs::write(root.join("_projr-other.yml"), r#"{"parameter":{"selected":"other"}}"#).unwrap();
    fs::write(root.join("_projr-local.yml"), r#"{"param":{"shared":"local"}}"#).unwrap();
    let before: Vec<_> = fs::read_dir(&root).unwrap().map(|entry| {
        let path = entry.unwrap().path(); (path.clone(),fs::read(path).unwrap())
    }).collect();
    let get = query(&root, &["yml","get"], true);
    assert_eq!(get["result"]["configuration"]["parameters"]["shared"], "local");
    assert_eq!(get["result"]["configuration"]["parameters"]["selected"], "team");
    assert_eq!(get["result"]["configuration"]["parameters"]["private"], "explicit-query-canary");
    assert_eq!(query(&root,&["yml","get","--profile","other","--check"],true)
        ["result"]["configuration"]["parameters"]["selected"], "other");
    for (key, expected) in [("array",json!([1,true,null])),("--project",json!("literal flag")),
        ("x);system('unexpected')",json!("literal expression")),("missing",Value::Null)] {
        assert_eq!(query(&root,&["yml","parameter","--",key],true)["result"]["value"],expected);
    }
    assert!(query(&root,&["yml","parameter"],true)["result"]["value"].is_object());
    let check = query(&root,&["yml","read"],true);
    assert_eq!(check["result"],json!({"validated":true}));
    assert!(!serde_json::to_string(&check).unwrap().contains("explicit-query-canary"));
    assert_eq!(fs::read_dir(&root).unwrap().count(),before.len());
    for (path, bytes) in before { assert_eq!(fs::read(path).unwrap(),bytes); }
    assert!(!root.join("not-created").exists());
}

#[test]
fn configuration_errors_are_typed_without_secrets_and_checked_mode_is_deliberate() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for content in [
        r#"{"parameters":{},"param":{"private":"failure-query-canary"}}"#,
        "parameters: [failure-query-canary",
        "failure-query-canary",
    ] {
        fs::write(root.join("_proj.yml"),content).unwrap();
        let reply=query(root,&["yml","get"],false);
        assert_eq!(reply["error"]["code"],"query_failed");
        assert!(!serde_json::to_string(&reply).unwrap().contains("failure-query-canary"));
        assert_eq!(fs::read_to_string(root.join("_proj.yml")).unwrap(),content);
    }
    fs::write(root.join("_proj.yml"),r#"{"r_only_scaffolding":{"private":"failure-query-canary"}}"#).unwrap();
    assert!(query(root,&["yml","get"],true)["result"]["configuration"]["r_only_scaffolding"].is_object());
    assert_eq!(query(root,&["yml","get","--check"],false)["error"]["code"],"query_failed");
    assert_eq!(query(root,&["yml","parameter"],false)["error"]["code"],"query_failed");
    fs::write(root.join("_proj.yml"),r#"{"par":{"value":2}}"#).unwrap();
    assert_eq!(query(root,&["yml","parameter","value"],false)["error"]["code"],"query_failed");
}


#[test]
fn new_human_query_failures_withhold_private_deserialization_values() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("_proj.yml"),
        r#"{"build":{"scripts":"private-human-query-canary"}}"#).unwrap();
    for args in [vec!["yml","get","--check"],vec!["yml","parameter","private-key-canary"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_proj"))
            .arg("--non-interactive").arg("--project").arg(temp.path()).args(args)
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
            .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr=String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("private details were withheld"));
        assert!(!stderr.contains("private-human-query-canary"));
        assert!(!stderr.contains("private-key-canary"));
    }
}
