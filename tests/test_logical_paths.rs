use std::{fs, path::Path, process::Command};

fn query(root: &Path, label: &str, target: &str, parts: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_proj")).args(["--machine","path","get","--label",label,"--target",target,"--project"])
        .arg(root).args(parts).current_dir(root).env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap()
}

fn path(output: std::process::Output) -> std::path::PathBuf {
    assert!(output.status.success(), "{} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    value["result"]["path"].as_str().unwrap().into()
}

#[test]
fn safe_roles_preserve_exact_labels_first_cache_order_and_external_final_paths_without_creating_directories() {
    let project = tempfile::tempdir().unwrap(); let root = project.path();
    let external = tempfile::tempdir().unwrap();
    let config = format!("directories:\n  cache-z:\n    path: first-cache\n  cache-a:\n    path: other-cache\n  output-model:\n    path: {}\n  data:\n    path: final-data\n  docs:\n    path: final-docs\n  raw-data:\n    path: {}\n",serde_json::to_string(&external.path().to_string_lossy()).unwrap(),serde_json::to_string(&external.path().to_string_lossy()).unwrap());
    fs::write(root.join("_proj.yml"), config).unwrap(); fs::write(root.join("VERSION"), "v1.2.3.4").unwrap();
    for label in ["output-model","data","docs"] {
        let result = path(query(root,label,"safe", &["figures","sentinel.png"]));
        let expected = root.canonicalize().unwrap().join("first-cache/projr/v1.2.3-4").join(label).join("figures/sentinel.png");
        assert_eq!(result,expected);
    }
    assert_eq!(path(query(root,"OUTPUT-MODEL","safe", &[])),root.canonicalize().unwrap().join("first-cache/projr/v1.2.3-4/output-model"));
    assert_eq!(path(query(root,"output-model","final", &[])).canonicalize().unwrap(),external.path().canonicalize().unwrap());
    assert_eq!(path(query(root,"raw-data","safe", &[])).canonicalize().unwrap(),external.path().canonicalize().unwrap());
    assert!(!root.join("first-cache").exists()); assert!(!root.join("other-cache").exists());
}

#[test]
fn fragments_cannot_replace_or_escape_role_roots_and_missing_inputs_fail_without_creating_cache() {
    let project=tempfile::tempdir().unwrap(); let root=project.path();
    fs::write(root.join("_proj.yml"),"directories:\n  cache:\n    path: cache\n  output:\n    path: final\n").unwrap();
    fs::write(root.join("VERSION"),"v1.2.3").unwrap();
    for fragments in [vec![".."],vec!["child/../escape"],vec!["/absolute"],vec!["C:/drive"],vec!["file:stream"]] {
        assert!(!query(root,"output","safe",&fragments).status.success());
    }
    assert!(!query(root,"output-extra","safe",&[]).status.success());
    fs::remove_file(root.join("VERSION")).unwrap();
    assert!(!query(root,"output","safe",&[]).status.success());
    assert!(query(root,"output","final",&[]).status.success());
    fs::write(root.join("_proj.yml"),"directories:\n  output:\n    path: final\n").unwrap();
    fs::write(root.join("VERSION"),"v1.2.3").unwrap();
    assert!(!query(root,"output","safe",&[]).status.success());
    assert!(!root.join("cache").exists());
}

#[test]
fn preview_and_release_queries_use_frozen_cache_mapping_and_active_version_with_fragments() {
    for mode in ["build-dev","build"] {
        let project=tempfile::tempdir().unwrap(); let root=project.path();
        fs::create_dir(root.join("cache")).unwrap();
        fs::create_dir(root.join("released")).unwrap();
        fs::write(root.join("released/previous"),"old release bytes").unwrap();
        fs::write(root.join("VERSION"),"v1.2.3-4\n").unwrap();
        fs::write(root.join("_proj.yml"),"directories:\n  cache:\n    path: cache\n  output:\n    path: released\nbuild:\n  scripts: [analysis.py]\n  git: false\n  restrictions:\n    not_behind: false\n").unwrap();
        let script=format!(r#"import json,os,subprocess
from pathlib import Path
root=Path(os.environ['PROJ_PROJECT_ROOT'])
(root/'VERSION').write_text('v9.9.9')
(root/'_proj.yml').write_text('invalid: changed config')
os.environ['PROJR_PROFILE']='changed-profile'
os.chdir(root.parent)
value=json.loads(subprocess.check_output([{binary},'--machine','path','get','--label','output','--target','safe','figures','result.txt'],text=True))
target=Path(value['result']['path'])
target.parent.mkdir(parents=True,exist_ok=True)
target.write_text('inspectable cache artifact')
(root/'cache/observed').write_text(str(target))
"#,binary=serde_json::to_string(env!("CARGO_BIN_EXE_proj")).unwrap());
        fs::write(root.join("analysis.py"),script).unwrap();
        let output=Command::new(env!("CARGO_BIN_EXE_proj")).args([mode,"--non-interactive","--project"]).arg(root).current_dir(root)
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT").output().unwrap();
        assert!(output.status.success(),"{} {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        let key=if mode=="build-dev" {"v1.2.3-4"} else {"v1.2.4"};
        let expected=root.canonicalize().unwrap().join("cache/projr").join(key).join("output/figures/result.txt");
        assert_eq!(fs::read_to_string(&expected).unwrap(),"inspectable cache artifact");
        assert_eq!(fs::read_to_string(root.join("released/previous")).unwrap(),"old release bytes");
        if mode=="build-dev" {assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(),"v1.2.3-4\n");}
    }
}
