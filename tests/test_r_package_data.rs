use std::{fs, path::Path, process::Command};

fn command(root: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_proj"));
    cmd.current_dir(root).env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT");
    cmd
}

fn query(root: &Path, target: &str) -> std::process::Output {
    command(root).args(["--machine","path","get","--label","output-r-data","--target",target,"--format","absolute","--project"])
        .arg(root).output().unwrap()
}

fn result_path(output: std::process::Output) -> std::path::PathBuf {
    assert!(output.status.success(), "{} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["result"]["path"].as_str().unwrap().into()
}

#[test]
fn builtin_lookup_never_activates_or_creates_data() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    assert_eq!(result_path(query(&root,"final")), root.join("data"));
    assert!(!query(&root,"safe").status.success());
    fs::write(root.join("VERSION"),"v1.2.3-4\n").unwrap();
    fs::write(root.join("_proj.yml"),"directories:\n  cache:\n    path: cache\n").unwrap();
    assert_eq!(result_path(query(&root,"safe")), root.join("cache/projr/v1.2.3-4/output-r-data"));
    let combined = proj::get_combined_yml(Some(""),camino::Utf8Path::from_path(&root).unwrap()).unwrap();
    assert!(combined["directories"].get("output-r-data").is_none());
    assert!(!root.join("cache").exists());
    assert!(!root.join("data").exists());
    assert!(!root.join(".gitignore").exists());
    assert!(!root.join(".Rbuildignore").exists());
}

#[test]
fn normalize_each_legacy_layer_before_merge_and_reject_conflicts_or_redirection() {
    let project = tempfile::tempdir().unwrap();
    let root = camino::Utf8Path::from_path(project.path()).unwrap();
    fs::write(root.join("_proj.yml"),"directories:\n  output-r-data:\n    ignore: git\n").unwrap();
    fs::write(root.join("_projr-alpha.yml"),"directories:\n  data:\n    path: ./data\n    ignore: rbuild\n").unwrap();
    fs::write(root.join("_projr-local.yml"),"directories:\n  DATA:\n    ignore: []\n").unwrap();
    let merged = proj::get_combined_yml(Some("alpha"),root).unwrap();
    assert_eq!(merged["directories"].as_object().unwrap().len(),1);
    assert_eq!(merged["directories"]["output-r-data"]["path"],"data");
    assert_eq!(merged["directories"]["output-r-data"]["ignore"],serde_json::json!([]));
    fs::remove_file(root.join("_projr-local.yml")).unwrap();
    for invalid in [
        "directories:\n  data: {}\n  output-r-data: {}\n",
        "directories:\n  data:\n    path: elsewhere\n",
        "directories:\n  data:\n    path: null\n",
        "directories:\n  output-r-data:\n    path: ../data\n",
    ] {
        fs::write(root.join("_projr-alpha.yml"),invalid).unwrap();
        assert!(proj::get_combined_yml(Some("alpha"),root).is_err(),"{invalid}");
    }
    fs::remove_file(root.join("_projr-alpha.yml")).unwrap();
    fs::write(root.join("_proj.yml"),"directories:\n  data: {}\n").unwrap();
    assert!(proj::get_combined_yml(Some(""),root).is_err());
    fs::write(root.join("_proj.yml"),"directories:\n  data-derived:\n    path: data\n").unwrap();
    assert!(!query(project.path(),"final").status.success());
    assert!(!root.join("data").exists());
}

#[test]
fn legacy_remote_content_uses_the_canonical_role() {
    let project = tempfile::tempdir().unwrap();
    let root = camino::Utf8Path::from_path(project.path()).unwrap();
    fs::write(root.join("_projr.yml"),"directories:\n  data: {}\nremotes:\n  local:\n    archive:\n      path: archive\n      structure: cas\n      content: [data]\n      inspect: file\n").unwrap();
    let merged = proj::get_combined_yml(Some(""),root).unwrap();
    assert_eq!(merged["remotes"]["local"]["archive"]["content"],serde_json::json!(["output-r-data"]));
    let config: proj::ProjConfig = serde_json::from_value(merged).unwrap();
    assert!(config.validate_and_resolve(root,false).is_ok());
}

#[test]
fn only_declared_r_data_is_cleared_and_promoted_and_failures_preserve_final_bytes() {
    for declaration in ["undeclared","canonical","legacy"] {
        for (mode,stage,fail) in [
            ("build","none",false), ("build","pre",false), ("build","post",false),
            ("build","pre",true), ("build","post",true),
            ("build-dev","pre",false), ("build-dev","post",true),
        ] {
            let project = tempfile::tempdir().unwrap();
            let root = project.path().canonicalize().unwrap();
            fs::create_dir(root.join("cache")).unwrap();
            fs::create_dir(root.join("data")).unwrap();
            fs::write(root.join("data/previous.rda"),b"previous\0package data").unwrap();
            fs::write(root.join("VERSION"),"v1.2.3-4\n").unwrap();
            let role = match declaration {
                "canonical" => "  output-r-data: {}\n",
                "legacy" => "  data:\n    path: data\n",
                _ => "",
            };
            let config = format!("directories:\n  cache:\n    path: cache\n{role}settings:\n  output-clear-stage: {stage}\nbuild:\n  scripts: [analysis.py]\n  git: false\n  restrictions:\n    not_behind: false\n");
            fs::write(root.join(if declaration=="legacy" {"_projr.yml"} else {"_proj.yml"}),config).unwrap();
            let script = format!(r#"import json,subprocess
from pathlib import Path
result=json.loads(subprocess.check_output([{binary},'--machine','path','get','--label','output-r-data','--format','absolute'],text=True))
target=Path(result['result']['path'])
target.mkdir(parents=True,exist_ok=True)
(target/'generated.rda').write_bytes(b'generated package data')
raise SystemExit({exit})
"#,binary=serde_json::to_string(env!("CARGO_BIN_EXE_proj")).unwrap(),exit=if fail {17} else {0});
            fs::write(root.join("analysis.py"),script).unwrap();
            let output = command(&root).args([mode,"--non-interactive","--project"]).arg(&root).output().unwrap();
            let log = format!("{} {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
            assert_eq!(output.status.success(),!fail,"{declaration} {mode} {stage}: {log}");
            if fail { assert!(log.contains("17"),"failure must reach analysis: {log}"); }
            let promoted = declaration!="undeclared" && mode=="build" && !fail;
            assert_eq!(root.join("data/generated.rda").exists(),promoted,"{declaration} {mode} {stage}: {log}");
            let cleared = promoted && stage!="none";
            assert_eq!(root.join("data/previous.rda").exists(),!cleared);
            if !cleared { assert_eq!(fs::read(root.join("data/previous.rda")).unwrap(),b"previous\0package data"); }
            let version = if mode=="build" {"v1.2.4"} else {"v1.2.3-4"};
            assert_eq!(fs::read(root.join("cache/projr").join(version).join("output-r-data/generated.rda")).unwrap(),b"generated package data");
            if fail || mode=="build-dev" { assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(),"v1.2.3-4\n"); }
        }
    }
}
