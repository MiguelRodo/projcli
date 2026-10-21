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


#[test]
fn profile_and_local_updates_preserve_first_cache_position_and_only_append_new_keys() {
    for local in [false,true] {
        let project=tempfile::tempdir().unwrap();let root=project.path();
        fs::write(root.join("VERSION"),"v1.2.3-4").unwrap();
        fs::write(root.join("_proj.yml"),"directories:\n  cache-z:\n    path: initial-first\n  cache-a:\n    path: second\n  output:\n    path: released\n").unwrap();
        fs::write(root.join("_projr-analysis.yml"),"directories:\n  cache-z:\n    path: profile-first\n  cache-new:\n    path: appended\n").unwrap();
        if local {fs::write(root.join("_projr-local.yml"),"directories:\n  cache-z:\n    path: local-first\n").unwrap();}
        let output=Command::new(env!("CARGO_BIN_EXE_proj")).args(["--machine","path","get","--label","output","--target","safe","--project"])
            .arg(root).current_dir(root).env("PROJR_PROFILE","analysis").env_remove("QUARTO_PROFILE")
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV).output().unwrap();
        let expected=root.canonicalize().unwrap().join(if local {"local-first"} else {"profile-first"}).join("projr/v1.2.3-4/output");
        assert_eq!(path(output),expected);
        let merged=proj::get_combined_yml(Some("analysis"),camino::Utf8Path::from_path(root).unwrap()).unwrap();
        let order:Vec<_>=merged["directories"].as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(order,["cache-z","cache-a","output","cache-new"]);
    }
}


#[test]
fn data_final_bytes_and_absence_are_restored_for_failures_and_all_preview_outcomes() {
    for external in [false,true] {
      for exists in [false,true] {
       for (mode,clear,phase) in [("build","pre","pre"),("build","post","post"),("build-dev","pre","none"),("build-dev","pre","pre"),("build-dev","post","post")] {
        let project=tempfile::tempdir().unwrap();let root=project.path();let outside=tempfile::tempdir().unwrap();
        let data=if external {outside.path().canonicalize().unwrap().join("data")} else {root.canonicalize().unwrap().join("data")};
        if exists {fs::create_dir_all(data.join("nested/empty")).unwrap();fs::write(data.join("previous"),b"old\0data bytes").unwrap();}
        fs::create_dir(root.join("cache")).unwrap();fs::write(root.join("VERSION"),"v1.2.3-4\n").unwrap();
        let config=format!("directories:\n  cache:\n    path: cache\n  data:\n    path: {}\nbuild:\n  scripts: [analysis.py]\n  hooks:\n    pre: [before.py]\n    post: [after.py]\n  clear_output: {clear}\n  git: false\n  restrictions:\n    not_behind: false\n",serde_json::to_string(&data.to_string_lossy()).unwrap());
        fs::write(root.join("_proj.yml"),config).unwrap();
        for (file,stage) in [("before.py","pre"),("analysis.py","analysis"),("after.py","post")] {
            let code=format!(r#"import json,subprocess
from pathlib import Path
data=Path({data})
data.mkdir(parents=True,exist_ok=True)
(data/'previous').write_text('changed')
(data/'new').write_text('new')
value=json.loads(subprocess.check_output([{binary},'--machine','path','get','--label','data','--target','safe','artifact.txt'],text=True))
target=Path(value['result']['path']);target.parent.mkdir(parents=True,exist_ok=True);target.write_text('retained cache bytes')
raise SystemExit({exit_code})
"#,data=serde_json::to_string(&data.to_string_lossy()).unwrap(),binary=serde_json::to_string(env!("CARGO_BIN_EXE_proj")).unwrap(),exit_code=if phase==stage {17} else {0});
            fs::write(root.join(file),code).unwrap();
        }
        let output=Command::new(env!("CARGO_BIN_EXE_proj")).args([mode,"--non-interactive","--project"]).arg(root).current_dir(root)
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT").output().unwrap();
        assert_eq!(output.status.success(),phase=="none","{mode}/{clear}/{phase}: {} {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        if phase != "none" {assert!(String::from_utf8_lossy(&output.stderr).contains("17"),"Fixture did not reach intended hook failure: {mode}/{clear}/{phase}/external={external}/exists={exists}: {}",String::from_utf8_lossy(&output.stderr));}
        if exists {
            assert_eq!(fs::read(data.join("previous")).unwrap(),b"old\0data bytes");assert!(data.join("nested/empty").is_dir());assert!(!data.join("new").exists());
        } else {assert!(!data.exists(),"previously absent data survived recovery");}
        assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(),"v1.2.3-4\n");
        let key=if mode=="build-dev" {"v1.2.3-4"} else {"v1.2.4"};
        let artifact=root.join("cache/projr").join(key).join("data/artifact.txt");
        assert!(artifact.is_file(),"Missing retained artifact {artifact:?}: {mode}/{clear}/{phase}/external={external}/exists={exists}: {}",String::from_utf8_lossy(&output.stderr));
        assert_eq!(fs::read_to_string(artifact).unwrap(),"retained cache bytes");
       }
      }
    }
}

#[test]
fn every_cache_class_is_protected_from_absent_recovery_parent_expansion() {
    for literal in [false,true] {
      for existing_parent in [false,true] {
        let project=tempfile::tempdir().unwrap();let root=project.path();
        if existing_parent {fs::create_dir(root.join("state")).unwrap();}
        let extra=if literal {"  cache:\n    path: other-cache\n"} else {""};
        let config=format!("directories:\n  cache-z:\n    path: state/cache\n{extra}  output:\n    path: released\nremotes:\n  local:\n    archive:\n      path: state/archive\n      structure: cas\n      inspect: manifest\n      content: [output]\ndev:\n  scripts: [analysis.py]\n");
        fs::write(root.join("_proj.yml"),config).unwrap();
        let script=format!(r#"import json,subprocess
from pathlib import Path
value=json.loads(subprocess.check_output([{binary},'--machine','path','get','--label','output','--target','safe','result.txt'],text=True))
target=Path(value['result']['path']);target.parent.mkdir(parents=True,exist_ok=True);target.write_text('safe cache retained')
"#,binary=serde_json::to_string(env!("CARGO_BIN_EXE_proj")).unwrap());
        fs::write(root.join("analysis.py"),script).unwrap();
        let output=Command::new(env!("CARGO_BIN_EXE_proj")).args(["build-dev","--non-interactive","--project"]).arg(root).current_dir(root)
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
        if existing_parent {
            assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
            assert_eq!(fs::read_to_string(root.join("state/cache/projr/v0.0.1-1/output/result.txt")).unwrap(),"safe cache retained");
        } else {
            assert!(!output.status.success());assert!(String::from_utf8_lossy(&output.stderr).contains("overlaps cache"));
            assert!(!root.join("state").exists(),"recovery overlap was not rejected before analysis");
        }
      }
    }
}

#[cfg(unix)]
#[test]
fn safe_role_lookup_rejects_linked_version_leaves_without_touching_external_bytes() {
    let project=tempfile::tempdir().unwrap();let root=project.path();let outside=tempfile::tempdir().unwrap();
    fs::create_dir_all(root.join("cache/projr")).unwrap();fs::write(outside.path().join("sentinel"),"external").unwrap();
    std::os::unix::fs::symlink(outside.path(),root.join("cache/projr/v1.2.3")).unwrap();
    fs::write(root.join("VERSION"),"v1.2.3").unwrap();fs::write(root.join("_proj.yml"),"directories:\n  cache:\n    path: cache\n  output:\n    path: released\n").unwrap();
    assert!(!query(root,"output","safe",&["sentinel"]).status.success());assert_eq!(fs::read_to_string(outside.path().join("sentinel")).unwrap(),"external");
}


#[test]
fn trailing_dot_or_space_role_labels_are_rejected_without_restricting_unicode() {
    let project=tempfile::tempdir().unwrap();let root=project.path();
    // Keep an ordinary (non-verbatim) absolute Windows cache path in this regression.
    let cache=root.join("cache");
    fs::write(root.join("VERSION"),"v1.2.3").unwrap();
    for alias in ["output.","output "] {
        fs::write(root.join("_proj.yml"),format!("directories:\n  cache:\n    path: {}\n  output:\n    path: released\n  {}:\n    path: other-release\n",serde_json::to_string(&cache.to_string_lossy()).unwrap(),serde_json::to_string(alias).unwrap())).unwrap();
        assert!(!query(root,"output","safe",&[]).status.success());assert!(!cache.exists());
    }
    fs::write(root.join("_proj.yml"),"directories:\n  cache:\n    path: cache\n  output-é:\n    path: released\n").unwrap();
    assert!(query(root,"output-é","safe",&[]).status.success());
}

#[cfg(unix)]
#[test]
fn implicit_project_sources_in_new_data_roles_fail_before_cleanup_and_renderer_launch() {
    for label in ["data","data-derived"] {
        let project=tempfile::tempdir().unwrap();let root=project.path();let outside=tempfile::tempdir().unwrap();
        fs::write(outside.path().join("analysis.qmd"),"source sentinel").unwrap();
        fs::write(root.join("VERSION"),"v1.2.3").unwrap();
        fs::write(root.join("_quarto.yml"),"project: {type: default}\n").unwrap();
        fs::write(root.join("_proj.yml"),format!("directories:\n  cache:\n    path: cache\n  docs:\n    path: published\n  {label}:\n    path: {}\nbuild:\n  git: false\n  clear_output: pre\n  restrictions:\n    not_behind: false\n",serde_json::to_string(&outside.path().canonicalize().unwrap().to_string_lossy()).unwrap())).unwrap();
        let before=fs::read(root.join("VERSION")).unwrap();
        let output=Command::new(env!("CARGO_BIN_EXE_proj")).args(["build","--non-interactive","--project"]).arg(root).current_dir(root)
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
        assert!(!output.status.success());assert!(String::from_utf8_lossy(&output.stderr).contains("source-like document"));
        assert_eq!(fs::read_to_string(outside.path().join("analysis.qmd")).unwrap(),"source sentinel");assert_eq!(fs::read(root.join("VERSION")).unwrap(),before);
        assert!(!root.join("cache").exists());assert!(!root.join(".gitignore").exists());
    }
}


#[cfg(windows)]
#[test]
fn unicode_case_alias_roles_cannot_write_the_same_file_under_an_ordinary_absolute_cache() {
    let project=tempfile::tempdir().unwrap();let root=project.path();
    let cache=root.join("cache");let cache_text=cache.to_string_lossy();
    let ordinary=cache_text.strip_prefix(r"\\?\").unwrap_or(&cache_text);
    fs::write(root.join("VERSION"),"v1.2.3").unwrap();
    for folder in ["released-upper","released-lower"] {
        fs::create_dir(root.join(folder)).unwrap();fs::write(root.join(folder).join("result.txt"),folder).unwrap();
    }
    fs::write(root.join("_proj.yml"),format!("directories:\n  cache:\n    path: {}\n  output-Ä:\n    path: released-upper\n  output-ä:\n    path: released-lower\ndev:\n  scripts: [analysis.py]\n",serde_json::to_string(ordinary).unwrap())).unwrap();
    let script=format!(r#"import json,subprocess
from pathlib import Path
Path('analysis-ran').touch()
for label in ['output-Ä','output-ä']:
 value=json.loads(subprocess.check_output([{binary},'--machine','path','get','--label',label,'--target','safe','result.txt'],text=True))
 target=Path(value['result']['path']);target.parent.mkdir(parents=True,exist_ok=True);target.write_text(label)
"#,binary=serde_json::to_string(env!("CARGO_BIN_EXE_proj")).unwrap());
    fs::write(root.join("analysis.py"),script).unwrap();
    for label in ["output-Ä","output-ä"] {assert!(!query(root,label,"safe",&["result.txt"]).status.success());}
    let output=Command::new(env!("CARGO_BIN_EXE_proj")).args(["build-dev","--non-interactive","--project"]).arg(root).current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
    assert!(!output.status.success());assert!(String::from_utf8_lossy(&output.stderr).contains("portable case comparison"));
    assert!(!root.join("analysis-ran").exists());assert!(!cache.exists());
    for folder in ["released-upper","released-lower"] {assert_eq!(fs::read_to_string(root.join(folder).join("result.txt")).unwrap(),folder);}
}
